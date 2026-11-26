//! One shared text detector and the two recognizers, on ONNX Runtime through `oar-ocr`.

use crate::manifest;
use crate::metrics::Rect;
use crate::select::Reading;
use image::RgbImage;
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::domain::tasks::TextDetectionConfig;
use oar_ocr::predictors::{TextDetectionPredictor, TextRecognitionPredictor};
use oar_ocr::processors::BoundingBox;
use ort::environment::Environment;
use ort::ep::DirectML;
use ort::logging::{LogLevel, LoggerFunction};
use std::fmt;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Official PP-OCRv6 detection post-processing, as `oar-ocr`'s own preset sets it.
pub const SCORE_THRESHOLD: f32 = 0.2;
pub const BOX_THRESHOLD: f32 = 0.45;
pub const UNCLIP_RATIO: f32 = 1.4;
pub const MAX_CANDIDATES: usize = 3000;
/// `oar-ocr` general-text default: an image is shrunk until its longer side fits.
pub const LIMIT_SIDE_LEN: u32 = 960;

/// Recognizer input: 48 px high, 320 to 3200 px wide.
const REC_HEIGHT: usize = 48;
const REC_MIN_WIDTH: usize = 320;
const REC_MAX_WIDTH: usize = 3200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Cpu,
    DirectMl { device: i32 },
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cpu => write!(f, "cpu"),
            Self::DirectMl { device } => write!(f, "dml:{device}"),
        }
    }
}

impl std::str::FromStr for Provider {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, String> {
        match text.split_once(':') {
            None if text == "cpu" => Ok(Self::Cpu),
            None if text == "dml" => Ok(Self::DirectMl { device: 0 }),
            Some(("dml", device)) => device
                .parse()
                .map(|device| Self::DirectMl { device })
                .map_err(|e| format!("bad DirectML device `{device}`: {e}")),
            _ => Err(format!(
                "unknown provider `{text}`: use cpu or dml[:device]"
            )),
        }
    }
}

#[derive(Clone)]
pub struct Options {
    pub provider: Provider,
    /// `None` leaves ONNX Runtime's default.
    pub intra_threads: Option<usize>,
    pub rec_batch: usize,
    pub limit_side_len: u32,
    /// Receives ONNX Runtime's verbose log, which includes where each node was placed.
    pub log: Option<LoggerFunction>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            provider: Provider::Cpu,
            intra_threads: None,
            rec_batch: 6,
            limit_side_len: LIMIT_SIDE_LEN,
            log: None,
        }
    }
}

/// Builds the process-wide ONNX Runtime environment, with telemetry off.
///
/// DirectML is registered here, on the environment, with `error_on_failure`. `oar-ocr` 0.11.0
/// only sets that flag in its private probe: a provider listed in its session config falls back
/// to the CPU in silence. ONNX Runtime applies the environment's providers to every session, so
/// both `ocr` and `vision` share this single initialization and its one-provider-per-process rule.
pub fn init_runtime(
    provider: Provider,
    log: Option<LoggerFunction>,
) -> Result<Environment, String> {
    static ACTIVE: Mutex<Option<(Provider, Environment)>> = Mutex::new(None);
    let mut active = ACTIVE.lock().map_err(|e| e.to_string())?;
    if let Some((first, env)) = active.as_ref() {
        return if *first == provider {
            Ok(env.clone())
        } else {
            Err(format!(
                "this process already runs on {first}; {provider} needs another process"
            ))
        };
    }
    let mut builder = ort::init().with_name("photosort").with_telemetry(false);
    if let Provider::DirectMl { device } = provider {
        builder = builder.with_execution_providers([DirectML::default()
            .with_device_id(device)
            .build()
            .error_on_failure()]);
    }
    let verbose = log.is_some();
    if let Some(log) = log {
        builder = builder.with_logger(log);
    }
    let environment = builder.build().map_err(|e| e.to_string())?;
    environment.set_log_level(if verbose {
        LogLevel::Verbose
    } else {
        LogLevel::Error
    });
    oar_ocr::core::inference::set_ort_environment(environment.clone())
        .map_err(|e| e.to_string())?;
    *active = Some((provider, environment.clone()));
    Ok(environment)
}

fn session_config(options: &Options) -> OrtSessionConfig {
    // Sequential execution for both providers; DirectML also forbids the memory pattern.
    let mut config = OrtSessionConfig::new().with_parallel_execution(false);
    if let Some(threads) = options.intra_threads {
        config = config.with_intra_threads(threads);
    }
    if matches!(options.provider, Provider::DirectMl { .. }) {
        config = config.with_memory_pattern(false);
    }
    if options.log.is_some() {
        config = config.with_log_severity_level(0);
    }
    config
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recognizer {
    V6,
    Korean,
}

#[derive(Debug, Clone)]
pub struct Line {
    /// Axis-aligned bounds of the detected quadrilateral.
    pub rect: Rect,
    pub v6: Reading,
    /// `None` when the Korean model was not asked.
    pub korean: Option<Reading>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Timings {
    pub detect: Duration,
    pub crop: Duration,
    pub v6: Duration,
    pub korean: Duration,
}

/// A recognizer batch as it ran: `(lines in the batch, tensor width)`.
pub type Shape = (usize, usize);

pub struct Engine {
    detector: TextDetectionPredictor,
    v6: TextRecognitionPredictor,
    korean: TextRecognitionPredictor,
    rec_batch: usize,
}

impl Engine {
    /// Refuses to load unless all five pinned files are present and unchanged.
    pub fn load(models: &Path, options: &Options) -> Result<Self, String> {
        manifest::verify_models(models)
            .map_err(|errors| format!("refusing to load models: {}", errors.join("; ")))?;
        if options.rec_batch == 0 {
            return Err("rec_batch must be at least 1".into());
        }
        let _ = init_runtime(options.provider, options.log.clone())?;
        let config = session_config(options);
        let detector = TextDetectionPredictor::builder()
            .with_config(TextDetectionConfig {
                score_threshold: SCORE_THRESHOLD,
                box_threshold: BOX_THRESHOLD,
                unclip_ratio: UNCLIP_RATIO,
                max_candidates: MAX_CANDIDATES,
                limit_side_len: Some(options.limit_side_len),
                ..TextDetectionConfig::default()
            })
            .with_ort_config(config.clone())
            .build(models.join(manifest::DETECTOR))
            .map_err(|e| format!("detector: {}", chain(&e)))?;
        let recognizer = |model: &str, dict: &str| {
            TextRecognitionPredictor::builder()
                .dict_path(models.join(dict))
                .with_ort_config(config.clone())
                .build(models.join(model))
                .map_err(|e| format!("{model}: {}", chain(&e)))
        };
        Ok(Self {
            detector,
            v6: recognizer(manifest::REC_V6, manifest::DICT_V6)?,
            korean: recognizer(manifest::REC_KOREAN, manifest::DICT_KOREAN)?,
            rec_batch: options.rec_batch,
        })
    }

    /// Text line quadrilaterals, in image pixels.
    pub fn detect(&self, image: &RgbImage) -> Result<Vec<BoundingBox>, String> {
        let result = self
            .detector
            .predict(vec![image.clone()])
            .map_err(|e| chain(&e))?;
        Ok(result
            .detections
            .into_iter()
            .flatten()
            .map(|d| d.bbox)
            .collect())
    }

    /// Reads line crops; the readings come in the order given. Crops are batched by aspect ratio
    /// so that each batch needs little padding, as the `oar-ocr` pipeline does.
    pub fn recognize(
        &self,
        which: Recognizer,
        crops: &[&RgbImage],
        shapes: &mut Vec<Shape>,
    ) -> Result<Vec<Reading>, String> {
        let model = match which {
            Recognizer::V6 => &self.v6,
            Recognizer::Korean => &self.korean,
        };
        let aspect = |i: usize| crops[i].width() as f32 / crops[i].height().max(1) as f32;
        let mut order: Vec<usize> = (0..crops.len()).collect();
        order.sort_by(|&a, &b| aspect(a).total_cmp(&aspect(b)));
        let empty = Reading {
            text: String::new(),
            score: 0.0,
        };
        let mut readings = vec![empty; crops.len()];
        for batch in order.chunks(self.rec_batch) {
            let widest = batch.iter().map(|&i| aspect(i)).fold(0.0, f32::max);
            shapes.push((batch.len(), tensor_width(widest)));
            let result = model
                .predict(batch.iter().map(|&i| crops[i].clone()).collect())
                .map_err(|e| chain(&e))?;
            if result.texts.len() != batch.len() || result.scores.len() != batch.len() {
                return Err("the recognizer returned a different number of lines".into());
            }
            for ((&i, text), score) in batch.iter().zip(result.texts).zip(result.scores) {
                readings[i] = Reading { text, score };
            }
        }
        Ok(readings)
    }

    /// Detects lines and reads each with v6, then with the Korean model where `wants_korean`,
    /// given the v6 reading, says so.
    pub fn read(
        &self,
        image: &RgbImage,
        wants_korean: impl Fn(&Reading) -> bool,
        shapes: &mut Vec<Shape>,
    ) -> Result<(Vec<Line>, Timings), String> {
        let mut timings = Timings::default();
        let start = Instant::now();
        let boxes = self.detect(image)?;
        timings.detect = start.elapsed();

        let start = Instant::now();
        let (mut rects, mut crops) = (Vec::new(), Vec::new());
        for quad in &boxes {
            // A degenerate box has no pixels to read.
            if let Ok(crop) = oar_ocr::utils::get_rotate_crop_image(image, &quad.points) {
                rects.push([quad.x_min(), quad.y_min(), quad.x_max(), quad.y_max()]);
                crops.push(crop);
            }
        }
        timings.crop = start.elapsed();

        let start = Instant::now();
        let all: Vec<&RgbImage> = crops.iter().collect();
        let v6 = self.recognize(Recognizer::V6, &all, shapes)?;
        timings.v6 = start.elapsed();

        let start = Instant::now();
        let asked: Vec<usize> = (0..v6.len()).filter(|&i| wants_korean(&v6[i])).collect();
        let subset: Vec<&RgbImage> = asked.iter().map(|&i| &crops[i]).collect();
        let mut korean: Vec<Option<Reading>> = vec![None; v6.len()];
        let readings = self.recognize(Recognizer::Korean, &subset, shapes)?;
        for (i, reading) in asked.into_iter().zip(readings) {
            korean[i] = Some(reading);
        }
        timings.korean = start.elapsed();

        let lines = rects
            .into_iter()
            .zip(v6)
            .zip(korean)
            .map(|((rect, v6), korean)| Line { rect, v6, korean })
            .collect();
        Ok((lines, timings))
    }
}

/// An error with its causes: `oar-ocr` keeps ONNX Runtime's message in the source chain.
fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

/// The width of the recognizer tensor for a batch whose widest crop has `aspect` = width/height.
/// Mirrors `CRNNModel::preprocess_refs`; used only to report which shapes ran.
fn tensor_width(aspect: f32) -> usize {
    let base = REC_MIN_WIDTH as f32 / REC_HEIGHT as f32;
    ((REC_HEIGHT as f32 * aspect.max(base)) as usize).min(REC_MAX_WIDTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_width_pads_short_lines_and_caps_long_ones() {
        assert_eq!(tensor_width(1.0), 320);
        assert_eq!(tensor_width(10.0), 480);
        assert_eq!(tensor_width(500.0), 3200);
    }

    #[test]
    fn provider_names_round_trip() {
        for provider in [Provider::Cpu, Provider::DirectMl { device: 1 }] {
            assert_eq!(provider.to_string().parse(), Ok(provider));
        }
        assert_eq!("dml".parse(), Ok(Provider::DirectMl { device: 0 }));
        assert!("cuda".parse::<Provider>().is_err());
        assert!("cpu:1".parse::<Provider>().is_err());
    }
}
