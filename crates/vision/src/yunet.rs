//! YuNet face detector (`face_detection_yunet_2023mar.onnx`), following OpenCV's `FaceDetectorYN`.
//!
//! Fixed input `[1, 3, 640, 640]` in BGR order with no normalization (`0.0..=255.0`).
//! Decodes strides 8, 16 and 32 with `score = sqrt(cls * obj)` and greedy IoU NMS, then maps
//! boxes back from the letterboxed canvas to original image pixel coordinates.

use crate::manifest::{YUNET_MODEL, verify_yunet};
use crate::session::{SessionOptions, build_session};
use image::RgbImage;
use image::imageops::{FilterType, resize};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;

pub const INPUT_SIZE: u32 = 640;
pub const STRIDES: [usize; 3] = [8, 16, 32];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    /// `[x1, y1, x2, y2]` in original image pixel coordinates.
    pub bbox: [f32; 4],
    pub score: f32,
    /// 5 facial landmarks in original image pixel coordinates:
    /// `[right_eye, left_eye, nose_tip, right_mouth_corner, left_mouth_corner]`.
    pub landmarks: [[f32; 2]; 5],
}

#[derive(Debug, Clone, Copy)]
pub struct YuNetConfig {
    pub score_threshold: f32,
    pub nms_threshold: f32,
    pub top_k: usize,
}

impl Default for YuNetConfig {
    fn default() -> Self {
        Self {
            score_threshold: 0.6,
            nms_threshold: 0.3,
            top_k: 5000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Letterbox {
    pub scale: f32,
    pub orig_w: u32,
    pub orig_h: u32,
}

/// Letterboxes an RGB image onto a 640×640 top-left black canvas and converts it to BGR NCHW
/// float32 without normalization (`0.0..=255.0`), matching OpenCV's `FaceDetectorYN`.
pub fn preprocess_image(image: &RgbImage) -> (Vec<f32>, Letterbox) {
    let orig_w = image.width();
    let orig_h = image.height();
    let size = INPUT_SIZE as usize;
    let plane = size * size;
    let mut out = vec![0.0f32; 3 * plane];
    if orig_w == 0 || orig_h == 0 {
        return (
            out,
            Letterbox {
                scale: 1.0,
                orig_w,
                orig_h,
            },
        );
    }
    let scale = (INPUT_SIZE as f32 / orig_w as f32).min(INPUT_SIZE as f32 / orig_h as f32);
    let new_w = ((orig_w as f32 * scale).round() as u32).clamp(1, INPUT_SIZE);
    let new_h = ((orig_h as f32 * scale).round() as u32).clamp(1, INPUT_SIZE);
    let resized = if new_w == orig_w && new_h == orig_h {
        std::borrow::Cow::Borrowed(image)
    } else {
        std::borrow::Cow::Owned(resize(image, new_w, new_h, FilterType::Triangle))
    };

    let (b_plane, rest) = out.split_at_mut(plane);
    let (g_plane, r_plane) = rest.split_at_mut(plane);
    for y in 0..new_h as usize {
        let row_offset = y * size;
        for x in 0..new_w as usize {
            let px = resized.get_pixel(x as u32, y as u32);
            let idx = row_offset + x;
            // BGR order, raw 0..255 float values.
            b_plane[idx] = f32::from(px[2]);
            g_plane[idx] = f32::from(px[1]);
            r_plane[idx] = f32::from(px[0]);
        }
    }
    (
        out,
        Letterbox {
            scale,
            orig_w,
            orig_h,
        },
    )
}

/// Raw output slices for one stride (`8`, `16` or `32`).
pub struct StrideOutputs<'a> {
    pub stride: usize,
    pub cls: &'a [f32],
    pub obj: &'a [f32],
    pub bbox: &'a [f32],
    pub kps: &'a [f32],
}

/// Decodes multi-stride YuNet tensors, maps boxes and landmarks back to original coordinates, and applies NMS.
pub fn decode_and_nms(
    strides: &[StrideOutputs<'_>],
    letterbox: Letterbox,
    config: YuNetConfig,
) -> Result<Vec<Face>, String> {
    let mut candidates = Vec::new();
    let max_x = letterbox.orig_w as f32;
    let max_y = letterbox.orig_h as f32;
    let inv_scale = if letterbox.scale > 0.0 {
        1.0 / letterbox.scale
    } else {
        1.0
    };

    for head in strides {
        let grid = INPUT_SIZE as usize / head.stride;
        let cells = grid * grid;
        if head.cls.len() != cells
            || head.obj.len() != cells
            || head.bbox.len() != cells * 4
            || head.kps.len() != cells * 10
        {
            return Err(format!(
                "stride {} tensor sizes ({}, {}, {}, {}) do not match {grid}x{grid}",
                head.stride,
                head.cls.len(),
                head.obj.len(),
                head.bbox.len(),
                head.kps.len()
            ));
        }
        let s = head.stride as f32;
        for r in 0..grid {
            for c in 0..grid {
                let idx = r * grid + c;
                let cls_score = head.cls[idx].clamp(0.0, 1.0);
                let obj_score = head.obj[idx].clamp(0.0, 1.0);
                let score = (cls_score * obj_score).sqrt();
                if score < config.score_threshold {
                    continue;
                }
                let b = &head.bbox[idx * 4..idx * 4 + 4];
                let cx = (c as f32 + b[0]) * s;
                let cy = (r as f32 + b[1]) * s;
                let w = b[2].exp() * s;
                let h = b[3].exp() * s;
                let x1 = ((cx - 0.5 * w) * inv_scale).clamp(0.0, max_x);
                let y1 = ((cy - 0.5 * h) * inv_scale).clamp(0.0, max_y);
                let x2 = ((cx + 0.5 * w) * inv_scale).clamp(0.0, max_x);
                let y2 = ((cy + 0.5 * h) * inv_scale).clamp(0.0, max_y);

                let mut landmarks = [[0.0f32; 2]; 5];
                let kps = &head.kps[idx * 10..idx * 10 + 10];
                for k in 0..5 {
                    let kx = (c as f32 + kps[k * 2]) * s;
                    let ky = (r as f32 + kps[k * 2 + 1]) * s;
                    landmarks[k][0] = (kx * inv_scale).clamp(0.0, max_x);
                    landmarks[k][1] = (ky * inv_scale).clamp(0.0, max_y);
                }

                if x2 > x1 && y2 > y1 {
                    candidates.push(Face {
                        bbox: [x1, y1, x2, y2],
                        score,
                        landmarks,
                    });
                }
            }
        }
    }

    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    candidates.truncate(config.top_k);
    Ok(nms(candidates, config.nms_threshold))
}

pub fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix1 = a[0].max(b[0]);
    let iy1 = a[1].max(b[1]);
    let ix2 = a[2].min(b[2]);
    let iy2 = a[3].min(b[3]);
    let inter = (ix2 - ix1).max(0.0) * (iy2 - iy1).max(0.0);
    if inter <= 0.0 {
        return 0.0;
    }
    let area_a = (a[2] - a[0]).max(0.0) * (a[3] - a[1]).max(0.0);
    let area_b = (b[2] - b[0]).max(0.0) * (b[3] - b[1]).max(0.0);
    let union = area_a + area_b - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

fn nms(sorted: Vec<Face>, nms_threshold: f32) -> Vec<Face> {
    let mut kept: Vec<Face> = Vec::new();
    'outer: for face in sorted {
        for prev in &kept {
            if iou(prev.bbox, face.bbox) > nms_threshold {
                continue 'outer;
            }
        }
        kept.push(face);
    }
    kept
}

pub struct YuNet {
    session: Session,
}

impl YuNet {
    /// Verifies the pinned YuNet file and loads the ONNX session.
    pub fn load(models_root: &Path, options: &SessionOptions) -> Result<Self, String> {
        verify_yunet(models_root)
            .map_err(|e| format!("refusing to load YuNet model: {}", e.join("; ")))?;
        let session = build_session(&models_root.join(YUNET_MODEL), options)?;
        Ok(Self { session })
    }

    pub fn detect(&mut self, image: &RgbImage, config: YuNetConfig) -> Result<Vec<Face>, String> {
        let (nchw, letterbox) = preprocess_image(image);
        self.detect_preprocessed(&nchw, letterbox, config)
    }

    pub fn detect_preprocessed(
        &mut self,
        nchw: &[f32],
        letterbox: Letterbox,
        config: YuNetConfig,
    ) -> Result<Vec<Face>, String> {
        let size = INPUT_SIZE as usize;
        let expected = 3 * size * size;
        if nchw.len() != expected {
            return Err(format!(
                "yunet input length {}, expected {expected}",
                nchw.len()
            ));
        }
        let input =
            TensorRef::from_array_view(([1, 3, size, size], nchw)).map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["input" => input])
            .map_err(|e| e.to_string())?;

        let extract = |name: &str| -> Result<&[f32], String> {
            let val = outputs
                .get(name)
                .ok_or_else(|| format!("yunet output missing `{name}`"))?;
            let (_, data) = val.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
            Ok(data)
        };

        let heads = [
            StrideOutputs {
                stride: 8,
                cls: extract("cls_8")?,
                obj: extract("obj_8")?,
                bbox: extract("bbox_8")?,
                kps: extract("kps_8")?,
            },
            StrideOutputs {
                stride: 16,
                cls: extract("cls_16")?,
                obj: extract("obj_16")?,
                bbox: extract("bbox_16")?,
                kps: extract("kps_16")?,
            },
            StrideOutputs {
                stride: 32,
                cls: extract("cls_32")?,
                obj: extract("obj_32")?,
                bbox: extract("bbox_32")?,
                kps: extract("kps_32")?,
            },
        ];
        decode_and_nms(&heads, letterbox, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn letterbox_preserves_aspect_ratio_and_uses_bgr_without_normalization() {
        let img = RgbImage::from_pixel(1280, 640, Rgb([10, 20, 30]));
        let (buf, lb) = preprocess_image(&img);
        assert!((lb.scale - 0.5).abs() < 1e-6);
        assert_eq!(lb.orig_w, 1280);
        assert_eq!(lb.orig_h, 640);
        let size = INPUT_SIZE as usize;
        let plane = size * size;
        // Inside resized area (y=0..320): BGR = (30, 20, 10)
        assert_eq!(buf[0], 30.0);
        assert_eq!(buf[plane], 20.0);
        assert_eq!(buf[2 * plane], 10.0);
        // Below resized area (y=400): black padding = 0.0
        let pad_idx = 400 * size + 100;
        assert_eq!(buf[pad_idx], 0.0);
        assert_eq!(buf[plane + pad_idx], 0.0);
        assert_eq!(buf[2 * plane + pad_idx], 0.0);
    }

    #[test]
    fn decode_computes_sqrt_cls_obj_maps_letterbox_and_applies_nms() {
        let mut cls8 = vec![0.0f32; 80 * 80];
        let mut obj8 = vec![0.0f32; 80 * 80];
        let mut bbox8 = vec![0.0f32; 80 * 80 * 4];
        let mut kps8 = vec![0.0f32; 80 * 80 * 10];

        // Cell (r=10, c=20) at stride 8:
        // cx = (20 + 0.5)*8 = 164, cy = (10 + 0.5)*8 = 84
        // w = exp(ln(4))*8 = 32, h = exp(ln(6))*8 = 48
        // canvas box = [148, 60, 180, 108]; with scale=0.5 -> orig [296, 120, 360, 216]
        let idx_a = 10 * 80 + 20;
        cls8[idx_a] = 0.81;
        obj8[idx_a] = 1.0; // score = sqrt(0.81) = 0.9
        bbox8[idx_a * 4] = 0.5;
        bbox8[idx_a * 4 + 1] = 0.5;
        bbox8[idx_a * 4 + 2] = 4.0f32.ln();
        bbox8[idx_a * 4 + 3] = 6.0f32.ln();
        kps8[idx_a * 10] = 0.2; // Right eye offset x
        kps8[idx_a * 10 + 1] = 0.3; // Right eye offset y

        // Overlapping lower-score box at (r=10, c=21): should be suppressed by NMS.
        let idx_b = 10 * 80 + 21;
        cls8[idx_b] = 0.64;
        obj8[idx_b] = 1.0; // score = 0.8
        bbox8[idx_b * 4] = -0.5; // same cx = 164
        bbox8[idx_b * 4 + 1] = 0.5;
        bbox8[idx_b * 4 + 2] = 4.0f32.ln();
        bbox8[idx_b * 4 + 3] = 6.0f32.ln();

        // Distinct face at stride 32, cell (r=15, c=15):
        // cx = 15*32 = 480, cy = 15*32 = 480, w = 32, h = 32 -> canvas [464, 464, 496, 496]
        // with scale=0.5 on 1280x1000 -> orig [928, 928, 992, 992]
        let mut cls32 = vec![0.0f32; 20 * 20];
        let mut obj32 = vec![0.0f32; 20 * 20];
        let bbox32 = vec![0.0f32; 20 * 20 * 4];
        let kps32 = vec![0.0f32; 20 * 20 * 10];
        let idx_c = 15 * 20 + 15;
        cls32[idx_c] = 0.49;
        obj32[idx_c] = 1.0; // score = 0.7

        let heads = [
            StrideOutputs {
                stride: 8,
                cls: &cls8,
                obj: &obj8,
                bbox: &bbox8,
                kps: &kps8,
            },
            StrideOutputs {
                stride: 16,
                cls: &vec![0.0; 40 * 40],
                obj: &vec![0.0; 40 * 40],
                bbox: &vec![0.0; 40 * 40 * 4],
                kps: &vec![0.0; 40 * 40 * 10],
            },
            StrideOutputs {
                stride: 32,
                cls: &cls32,
                obj: &obj32,
                bbox: &bbox32,
                kps: &kps32,
            },
        ];
        let lb = Letterbox {
            scale: 0.5,
            orig_w: 1280,
            orig_h: 1000,
        };
        let faces = decode_and_nms(&heads, lb, YuNetConfig::default()).unwrap();
        assert_eq!(faces.len(), 2, "{faces:?}");
        assert!((faces[0].score - 0.9).abs() < 1e-5);
        assert!((faces[0].bbox[0] - 296.0).abs() < 1e-3);
        assert!((faces[0].bbox[1] - 120.0).abs() < 1e-3);
        assert!((faces[0].bbox[2] - 360.0).abs() < 1e-3);
        assert!((faces[0].bbox[3] - 216.0).abs() < 1e-3);
        assert!((faces[1].score - 0.7).abs() < 1e-5);
        assert!((faces[1].bbox[0] - 928.0).abs() < 1e-3);
        assert!((faces[1].bbox[3] - 992.0).abs() < 1e-3);
    }
}
