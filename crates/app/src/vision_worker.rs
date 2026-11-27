use crate::settings::OcrDevice;
use crate::worker::Notify;
use catalog::{
    AssetId, Catalog, DetectedFace, RankedAsset, SIGLIP2_MODEL_ID, VISION_STAGE, VISION_VERSION,
};
use fsguard::DestPath;
use image::RgbImage;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;
use vision::{Provider, SessionOptions, SigLip2, SigLip2Text, YuNet, YuNetConfig};

/// Maximum edge in pixels when decoding images for vision analysis (SigLIP 2 uses 256 px,
/// YuNet uses 640 px; EXIF orientation is applied by `thumbs::decode`).
pub const VISION_DECODE_MAX_PX: u32 = 640;

/// Measured optimal batch sizes (`docs/vision-evaluacion.md`): DirectML batch 8, CPU batch 1.
pub const DIRECTML_VISION_BATCH: usize = 8;
pub const CPU_VISION_BATCH: usize = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VisionScope {
    Assets(Vec<AssetId>),
    Roots(Vec<PathBuf>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum VisionEvent {
    LoadingModel,
    Progress {
        done: usize,
        total: usize,
        errors: usize,
        elapsed_secs: f64,
        saved_asset: Option<AssetId>,
    },
    Finished {
        stopped: bool,
        processed: usize,
        errors: usize,
    },
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct VisionImageOutput {
    pub embedding: Vec<f32>,
    pub faces: Vec<DetectedFace>,
}

/// Injectable vision engine so the worker is fully testable without ONNX models.
pub trait VisionReader: Send + 'static {
    fn batch_size(&self) -> usize;
    fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String>;
    fn analyze_images(&mut self, images: &[&RgbImage]) -> Result<Vec<VisionImageOutput>, String>;

    fn decode_image(&self, path: &Path) -> Result<RgbImage, String> {
        thumbs::decode(path, VISION_DECODE_MAX_PX)
            .map(|img| img.to_rgb8())
            .map_err(|_| "no se pudo decodificar la imagen".to_string())
    }

    fn decode_media(&self, path: &Path) -> Result<Vec<RgbImage>, String> {
        if thumbs::is_video_path(path) {
            thumbs::extract_frames(path, 4, VISION_DECODE_MAX_PX)
                .map_err(|e| format!("no se pudieron extraer fotogramas del vídeo: {e}"))
        } else {
            self.decode_image(path).map(|img| vec![img])
        }
    }

    fn analyze_asset_media(&mut self, frames: &[RgbImage]) -> Result<VisionImageOutput, String> {
        if frames.is_empty() {
            return Err("sin fotogramas para analizar".into());
        }
        let frame_refs: Vec<&RgbImage> = frames.iter().collect();
        let outputs = match self.analyze_images(&frame_refs) {
            Ok(out) if !out.is_empty() => out,
            Err(batch_err) if frame_refs.len() > 1 => {
                let mut ok = Vec::with_capacity(frame_refs.len());
                let mut last_err = batch_err;
                for frame in frame_refs {
                    match self.analyze_images(&[frame]) {
                        Ok(mut single) => {
                            if let Some(item) = single.pop() {
                                ok.push(item);
                            }
                        }
                        Err(e) => last_err = e,
                    }
                }
                if ok.is_empty() {
                    return Err(last_err);
                }
                ok
            }
            Err(e) => return Err(e),
            Ok(_) => return Err("resultado de visión vacío".into()),
        };

        let embs: Vec<Vec<f32>> = outputs.iter().map(|o| o.embedding.clone()).collect();
        let combined_emb = combine_embeddings(&embs);
        let norm: f32 = combined_emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Err("embedding visual inválido".into());
        }
        let mut all_faces = Vec::new();
        for out in outputs {
            all_faces.extend(out.faces);
        }
        Ok(VisionImageOutput {
            embedding: combined_emb,
            faces: all_faces,
        })
    }
}

pub fn combine_embeddings(embeddings: &[Vec<f32>]) -> Vec<f32> {
    let Some(first_valid) = embeddings
        .iter()
        .find(|e| !e.is_empty() && e.iter().all(|x| x.is_finite()))
    else {
        return vec![0.0; catalog::EMBEDDING_DIM];
    };
    let dim = first_valid.len();
    let mut avg = vec![0.0f32; dim];
    for emb in embeddings {
        if emb.len() != dim || !emb.iter().all(|x| x.is_finite()) {
            continue;
        }
        for (a, &b) in avg.iter_mut().zip(emb) {
            *a += b;
        }
    }
    let norm: f32 = avg.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm.is_finite() && norm > 0.0 {
        for x in &mut avg {
            *x /= norm;
        }
    } else {
        avg.fill(0.0);
    }
    avg
}

pub fn session_options(device: OcrDevice) -> SessionOptions {
    let provider = match device {
        OcrDevice::DirectMl => Provider::DirectMl { device: 0 },
        OcrDevice::Cpu => Provider::Cpu,
    };
    SessionOptions {
        provider,
        intra_threads: None,
        log: None,
    }
}

pub fn vision_batch_size(device: OcrDevice) -> usize {
    match device {
        OcrDevice::DirectMl => DIRECTML_VISION_BATCH,
        OcrDevice::Cpu => CPU_VISION_BATCH,
    }
}

/// Real vision reader backed by SigLIP 2 (vision + text) and YuNet.
pub struct EngineVisionReader {
    siglip: SigLip2,
    yunet: YuNet,
    batch_size: usize,
}

impl EngineVisionReader {
    pub fn load(models_dir: &Path, device: OcrDevice) -> Result<Self, String> {
        let options = session_options(device);
        let siglip = SigLip2::load(models_dir, &options)?;
        let yunet = YuNet::load(models_dir, &options)?;
        Ok(Self {
            siglip,
            yunet,
            batch_size: vision_batch_size(device),
        })
    }
}

impl VisionReader for EngineVisionReader {
    fn batch_size(&self) -> usize {
        self.batch_size
    }

    fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        self.siglip.embed_texts(texts)
    }

    fn analyze_images(&mut self, images: &[&RgbImage]) -> Result<Vec<VisionImageOutput>, String> {
        let embeddings = self.siglip.embed_images(images)?;
        if embeddings.len() != images.len() {
            return Err("cantidad de embeddings inesperada".into());
        }
        let mut out = Vec::with_capacity(images.len());
        let cfg = YuNetConfig::default();
        for (img, emb) in images.iter().zip(embeddings) {
            let faces = self
                .yunet
                .detect(img, cfg)?
                .into_iter()
                .map(|f| DetectedFace::with_landmarks(f.bbox, f.score, f.landmarks))
                .collect();
            out.push(VisionImageOutput {
                embedding: emb,
                faces,
            });
        }
        Ok(out)
    }
}

/// Runs vision analysis (SigLIP 2 + YuNet + rule application) on a dedicated background thread.
/// Sessions are dropped when the run finishes or is cancelled.
pub struct VisionWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<VisionEvent>,
    join: Option<JoinHandle<()>>,
}

impl VisionWorker {
    pub fn start(
        db: DestPath,
        scope: VisionScope,
        models_dir: PathBuf,
        device: OcrDevice,
        notify: Option<Notify>,
    ) -> Self {
        Self::start_with_loader(
            db,
            scope,
            move || EngineVisionReader::load(&models_dir, device),
            notify,
        )
    }

    pub fn start_with_loader<R, F>(
        db: DestPath,
        scope: VisionScope,
        loader: F,
        notify: Option<Notify>,
    ) -> Self
    where
        R: VisionReader,
        F: FnOnce() -> Result<R, String> + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: VisionEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(VisionEvent::Failed(e.to_string()));
                    return;
                }
            };

            let pending = match &scope {
                VisionScope::Assets(ids) => {
                    cat.pending_in_assets(VISION_STAGE, VISION_VERSION, ids, usize::MAX)
                }
                VisionScope::Roots(roots) => {
                    cat.pending_in_roots(VISION_STAGE, VISION_VERSION, roots, usize::MAX)
                }
            };
            let pending = match pending {
                Ok(list) => list,
                Err(e) => {
                    emit(VisionEvent::Failed(e.to_string()));
                    return;
                }
            };

            let total = pending.len();
            if total == 0 {
                emit(VisionEvent::Finished {
                    stopped: false,
                    processed: 0,
                    errors: 0,
                });
                return;
            }

            if cancel_bg.load(Ordering::Relaxed) {
                emit(VisionEvent::Finished {
                    stopped: true,
                    processed: 0,
                    errors: 0,
                });
                return;
            }

            emit(VisionEvent::LoadingModel);
            let mut reader = match loader() {
                Ok(r) => r,
                Err(e) => {
                    emit(VisionEvent::Failed(e));
                    return;
                }
            };

            // Ensure text embeddings for all enabled rules are cached before processing images.
            let compiled = match compile_or_embed_rules(&mut cat, |texts| reader.embed_texts(texts))
            {
                Ok(c) => c,
                Err(e) => {
                    drop(reader);
                    emit(VisionEvent::Failed(e));
                    return;
                }
            };

            let batch_size = reader.batch_size().max(1);
            let started = Instant::now();
            let mut done = 0usize;
            let mut processed = 0usize;
            let mut errors = 0usize;
            let mut stopped = false;

            emit(VisionEvent::Progress {
                done: 0,
                total,
                errors: 0,
                elapsed_secs: 0.0,
                saved_asset: None,
            });

            for chunk in pending.chunks(batch_size) {
                if cancel_bg.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }

                // 1. Decode each asset (image or video frames) in the batch independently;
                //    a bad/panicking file does not stop its batch mates.
                let mut decoded: Vec<(AssetId, Vec<RgbImage>)> = Vec::with_capacity(chunk.len());
                for item in chunk {
                    if cancel_bg.load(Ordering::Relaxed) {
                        stopped = true;
                        break;
                    }
                    let dec = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        reader.decode_media(&item.path)
                    }))
                    .unwrap_or_else(|_| Err("error interno al decodificar el archivo".into()));
                    match dec {
                        Ok(frames) => decoded.push((item.asset, frames)),
                        Err(_) => {
                            errors += 1;
                            done += 1;
                            emit(VisionEvent::Progress {
                                done,
                                total,
                                errors,
                                elapsed_secs: started.elapsed().as_secs_f64(),
                                saved_asset: None,
                            });
                        }
                    }
                }

                if stopped && decoded.is_empty() {
                    break;
                }
                if decoded.is_empty() {
                    continue;
                }

                // 2. Run inference on the batch. If all items have 1 frame, run standard multi-image batching.
                //    Otherwise or on failure, fall back to per-asset analysis.
                let outcomes: Vec<(AssetId, Result<VisionImageOutput, String>)> = {
                    let all_single = decoded.iter().all(|(_, f)| f.len() == 1);
                    if all_single {
                        let img_refs: Vec<&RgbImage> = decoded.iter().map(|(_, f)| &f[0]).collect();
                        let batch_res =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                reader.analyze_images(&img_refs)
                            }))
                            .unwrap_or_else(|_| {
                                Err("error interno en inferencia de visión".into())
                            });

                        match batch_res {
                            Ok(outputs) if outputs.len() == decoded.len() => decoded
                                .iter()
                                .zip(outputs)
                                .map(|((asset, _), out)| (*asset, Ok(out)))
                                .collect(),
                            _ => decoded
                                .iter()
                                .map(|(asset, frames)| {
                                    let single =
                                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                            || reader.analyze_asset_media(frames),
                                        ))
                                        .unwrap_or_else(
                                            |_| Err("error interno en inferencia de visión".into()),
                                        );
                                    (*asset, single)
                                })
                                .collect(),
                        }
                    } else {
                        decoded
                            .iter()
                            .map(|(asset, frames)| {
                                let res =
                                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                        reader.analyze_asset_media(frames)
                                    }))
                                    .unwrap_or_else(|_| {
                                        Err("error interno en inferencia de visión".into())
                                    });
                                (*asset, res)
                            })
                            .collect()
                    }
                };

                // 3. Persist each succeeded asset (embedding + faces + vision v1 done) and apply rules.
                for (asset, outcome) in outcomes {
                    let saved_asset = match outcome {
                        Ok(out) => match cat
                            .save_vision_result(asset, SIGLIP2_MODEL_ID, &out.embedding, &out.faces)
                            .and_then(|()| cat.apply_vision_rules(asset, &compiled))
                        {
                            Ok(_) => {
                                processed += 1;
                                Some(asset)
                            }
                            Err(e) => {
                                drop(reader);
                                emit(VisionEvent::Failed(e.to_string()));
                                return;
                            }
                        },
                        Err(_) => {
                            errors += 1;
                            None
                        }
                    };
                    done += 1;
                    emit(VisionEvent::Progress {
                        done,
                        total,
                        errors,
                        elapsed_secs: started.elapsed().as_secs_f64(),
                        saved_asset,
                    });
                }

                if stopped {
                    break;
                }
            }

            // Explicitly release ONNX sessions before signaling completion.
            drop(reader);
            emit(VisionEvent::Finished {
                stopped,
                processed,
                errors,
            });
        });

        Self {
            cancel,
            rx,
            join: Some(join),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn try_recv(&self) -> Option<VisionEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<VisionEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for VisionWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

pub(crate) fn compile_or_embed_rules(
    cat: &mut Catalog,
    mut embed: impl FnMut(&[&str]) -> Result<Vec<Vec<f32>>, String>,
) -> Result<catalog::CompiledVisionRules, String> {
    let (compiled, missing) = cat
        .compile_vision_rules(SIGLIP2_MODEL_ID)
        .map_err(|e| e.to_string())?;
    if missing.is_empty() {
        return Ok(compiled);
    }
    let refs: Vec<&str> = missing.iter().map(String::as_str).collect();
    let vecs = embed(&refs)?;
    if vecs.len() != missing.len() {
        return Err("cantidad inesperada de embeddings de texto".into());
    }
    let pairs: Vec<(&str, &[f32])> = missing
        .iter()
        .zip(&vecs)
        .map(|(t, v)| (t.as_str(), v.as_slice()))
        .collect();
    cat.save_text_embeddings(SIGLIP2_MODEL_ID, &pairs)
        .map_err(|e| e.to_string())?;
    let (recompiled, still_missing) = cat
        .compile_vision_rules(SIGLIP2_MODEL_ID)
        .map_err(|e| e.to_string())?;
    if !still_missing.is_empty() {
        return Err("no se pudieron compilar todas las descripciones de reglas".into());
    }
    Ok(recompiled)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagsApplyEvent {
    LoadingModel,
    Progress {
        done: usize,
        total: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Finished {
        stopped: bool,
        processed: usize,
        matched_assets: usize,
        suggestions: usize,
    },
    Failed(String),
}

/// Recomputes vision tag suggestions from stored embeddings on a background thread.
/// Never reads image files from disk.
pub struct TagsApplyWorker {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<TagsApplyEvent>,
    join: Option<JoinHandle<()>>,
}

impl TagsApplyWorker {
    pub fn start(
        db: DestPath,
        models_dir: Option<PathBuf>,
        device: OcrDevice,
        notify: Option<Notify>,
    ) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_bg = Arc::clone(&cancel);
        let (tx, rx) = mpsc::channel();

        let join = thread::spawn(move || {
            let emit = |ev: TagsApplyEvent| {
                let _ = tx.send(ev);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    emit(TagsApplyEvent::Failed(e.to_string()));
                    return;
                }
            };

            let compiled = match compile_or_embed_rules(&mut cat, |missing| {
                let Some(dir) = models_dir.as_deref() else {
                    return Err(
                        "Faltan descripciones por codificar y no hay carpeta de modelos de visión configurada. Configúrala en Configuración."
                            .into(),
                    );
                };
                emit(TagsApplyEvent::LoadingModel);
                let opts = session_options(device);
                let mut text_model = SigLip2Text::load(dir, &opts)?;
                text_model.embed_texts(missing)
            }) {
                Ok(c) => c,
                Err(e) => {
                    emit(TagsApplyEvent::Failed(e));
                    return;
                }
            };

            let assets = match cat.assets_with_vision() {
                Ok(list) => list,
                Err(e) => {
                    emit(TagsApplyEvent::Failed(e.to_string()));
                    return;
                }
            };

            let total = assets.len();
            if total == 0 {
                emit(TagsApplyEvent::Finished {
                    stopped: false,
                    processed: 0,
                    matched_assets: 0,
                    suggestions: 0,
                });
                return;
            }

            let mut processed = 0usize;
            let mut matched_assets = 0usize;
            let mut suggestions = 0usize;
            let mut stopped = false;

            emit(TagsApplyEvent::Progress {
                done: 0,
                total,
                matched_assets: 0,
                suggestions: 0,
            });

            const APPLY_CHUNK: usize = 256;
            for chunk in assets.chunks(APPLY_CHUNK) {
                if cancel_bg.load(Ordering::Relaxed) {
                    stopped = true;
                    break;
                }
                match cat.apply_vision_rules_batch(chunk, &compiled) {
                    Ok((m, s)) => {
                        processed += chunk.len();
                        matched_assets += m;
                        suggestions += s;
                    }
                    Err(e) => {
                        emit(TagsApplyEvent::Failed(e.to_string()));
                        return;
                    }
                }
                emit(TagsApplyEvent::Progress {
                    done: processed,
                    total,
                    matched_assets,
                    suggestions,
                });
            }

            emit(TagsApplyEvent::Finished {
                stopped,
                processed,
                matched_assets,
                suggestions,
            });
        });

        Self {
            cancel,
            rx,
            join: Some(join),
        }
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn try_recv(&self) -> Option<TagsApplyEvent> {
        self.rx.try_recv().ok()
    }

    pub fn recv(&self) -> Result<TagsApplyEvent, mpsc::RecvError> {
        self.rx.recv()
    }
}

impl Drop for TagsApplyWorker {
    fn drop(&mut self) {
        self.cancel();
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

#[derive(Debug, Clone)]
pub enum VisionRankResult {
    Ready {
        title: String,
        threshold: Option<f32>,
        ranked: Vec<RankedAsset>,
    },
    Failed(String),
}

/// Computes any missing text embeddings on a background thread (never on the UI thread) and ranks
/// present analyzed assets by cosine similarity.
pub struct VisionRankWorker {
    rx: mpsc::Receiver<VisionRankResult>,
    join: Option<JoinHandle<()>>,
}

impl VisionRankWorker {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        db: DestPath,
        models_dir: Option<PathBuf>,
        device: OcrDevice,
        roots: Vec<PathBuf>,
        title: String,
        texts: Vec<String>,
        threshold: Option<f32>,
        limit: usize,
        notify: Option<Notify>,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let join = thread::spawn(move || {
            let send = |res: VisionRankResult| {
                let _ = tx.send(res);
                if let Some(cb) = &notify {
                    cb();
                }
            };

            let clean: Vec<String> = texts
                .into_iter()
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect();
            if clean.is_empty() {
                send(VisionRankResult::Failed(
                    "Escribe al menos una descripción para buscar.".into(),
                ));
                return;
            }

            let mut cat = match Catalog::open(&db) {
                Ok(c) => c,
                Err(e) => {
                    send(VisionRankResult::Failed(e.to_string()));
                    return;
                }
            };

            let mut cached_vecs: Vec<Option<Vec<f32>>> = Vec::with_capacity(clean.len());
            let mut missing_indices: Vec<usize> = Vec::new();
            for (i, text) in clean.iter().enumerate() {
                match cat.cached_text_embedding(SIGLIP2_MODEL_ID, text) {
                    Ok(Some(v)) => cached_vecs.push(Some(v)),
                    Ok(None) => {
                        cached_vecs.push(None);
                        missing_indices.push(i);
                    }
                    Err(e) => {
                        send(VisionRankResult::Failed(e.to_string()));
                        return;
                    }
                }
            }

            if !missing_indices.is_empty() {
                let Some(dir) = models_dir.as_deref() else {
                    send(VisionRankResult::Failed(
                        "Configura primero la carpeta de modelos de visión en Configuración para codificar esta descripción."
                            .into(),
                    ));
                    return;
                };
                let opts = session_options(device);
                let mut text_model = match SigLip2Text::load(dir, &opts) {
                    Ok(m) => m,
                    Err(e) => {
                        send(VisionRankResult::Failed(e));
                        return;
                    }
                };
                let missing_refs: Vec<&str> =
                    missing_indices.iter().map(|&i| clean[i].as_str()).collect();
                let new_vecs = match text_model.embed_texts(&missing_refs) {
                    Ok(v) => v,
                    Err(e) => {
                        send(VisionRankResult::Failed(e));
                        return;
                    }
                };
                drop(text_model);

                let pairs: Vec<(&str, &[f32])> = missing_refs
                    .iter()
                    .zip(&new_vecs)
                    .map(|(&t, v)| (t, v.as_slice()))
                    .collect();
                if let Err(e) = cat.save_text_embeddings(SIGLIP2_MODEL_ID, &pairs) {
                    send(VisionRankResult::Failed(e.to_string()));
                    return;
                }
                for (idx, vec) in missing_indices.into_iter().zip(new_vecs) {
                    cached_vecs[idx] = Some(vec);
                }
            }

            let vecs: Vec<Vec<f32>> = cached_vecs.into_iter().flatten().collect();
            let refs: Vec<&[f32]> = vecs.iter().map(Vec::as_slice).collect();
            let rank_res = if roots.is_empty() {
                cat.rank_by_embeddings(SIGLIP2_MODEL_ID, &refs, limit)
            } else {
                cat.rank_by_embeddings_in_roots(SIGLIP2_MODEL_ID, &refs, &roots, limit)
            };
            match rank_res {
                Ok(ranked) => send(VisionRankResult::Ready {
                    title,
                    threshold,
                    ranked,
                }),
                Err(e) => send(VisionRankResult::Failed(e.to_string())),
            }
        });

        Self {
            rx,
            join: Some(join),
        }
    }

    pub fn try_recv(&self) -> Option<VisionRankResult> {
        self.rx.try_recv().ok()
    }
}

impl Drop for VisionRankWorker {
    fn drop(&mut self) {
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combine_embeddings_normalizes_to_unit_vector() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let v3 = vec![0.0, 0.0, 1.0];
        let combined = combine_embeddings(&[v1, v2, v3]);
        let norm: f32 = combined.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        let expected = 1.0 / (3.0f32).sqrt();
        for val in combined {
            assert!((val - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn combine_embeddings_single_and_empty() {
        let empty = combine_embeddings(&[]);
        assert_eq!(empty.len(), catalog::EMBEDDING_DIM);
        assert!(empty.iter().all(|&x| x == 0.0));

        let single = vec![3.0, 4.0];
        let combined = combine_embeddings(&[single]);
        let norm: f32 = combined.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((combined[0] - 0.6).abs() < 1e-6);
        assert!((combined[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn combine_embeddings_skips_non_finite_and_never_returns_nan() {
        let valid = vec![3.0, 4.0, 0.0];
        let bad_nan = vec![f32::NAN, 1.0, 0.0];
        let bad_inf = vec![f32::INFINITY, 0.0, 0.0];
        let combined = combine_embeddings(&[bad_nan.clone(), valid, bad_inf.clone()]);
        assert!(combined.iter().all(|x| x.is_finite()));
        let norm: f32 = combined.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!((combined[0] - 0.6).abs() < 1e-6);
        assert!((combined[1] - 0.8).abs() < 1e-6);

        let all_bad = combine_embeddings(&[bad_nan, bad_inf]);
        assert!(all_bad.iter().all(|&x| x == 0.0));
    }

    struct PartialFailReader;

    impl VisionReader for PartialFailReader {
        fn batch_size(&self) -> usize {
            4
        }

        fn embed_texts(&mut self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
            Ok(Vec::new())
        }

        fn analyze_images(
            &mut self,
            images: &[&RgbImage],
        ) -> Result<Vec<VisionImageOutput>, String> {
            if images.len() > 1 {
                return Err("batch failed on one bad frame".into());
            }
            let img = images[0];
            // Treat width == 1 as a failing frame, width == 2 as a NaN frame, others as valid.
            if img.width() == 1 {
                return Err("bad frame".into());
            }
            if img.width() == 2 {
                return Ok(vec![VisionImageOutput {
                    embedding: vec![f32::NAN; 3],
                    faces: Vec::new(),
                }]);
            }
            Ok(vec![VisionImageOutput {
                embedding: vec![0.0, 3.0, 4.0],
                faces: Vec::new(),
            }])
        }
    }

    #[test]
    fn analyze_asset_media_survives_single_frame_failure_and_rejects_all_failed() {
        let mut reader = PartialFailReader;
        let bad = RgbImage::new(1, 1);
        let nan = RgbImage::new(2, 2);
        let good = RgbImage::new(4, 4);

        let ok = reader
            .analyze_asset_media(&[bad.clone(), nan.clone(), good])
            .expect("should succeed with remaining valid frame");
        assert!(ok.embedding.iter().all(|x| x.is_finite()));
        let norm: f32 = ok.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);

        let err = reader.analyze_asset_media(&[bad, nan]);
        assert!(err.is_err(), "all frames failing/NaN must return Err");
    }
}
