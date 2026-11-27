//! SFace face recognition (`face_recognition_sface_2021dec.onnx`).
//!
//! Input: aligned 112×112 BGR face crop without normalization (`0.0..=255.0`), shape `[1, 3, 112, 112]`.
//! Output: 128-dimensional L2-normalized feature vector.
//! Cosine similarity threshold for "same person": `0.363`.

use crate::manifest::{SFACE_MODEL, verify_sface};
use crate::session::{SessionOptions, build_session};
use crate::siglip2::l2_normalize;
use image::RgbImage;
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;

pub const SFACE_INPUT_SIZE: u32 = 112;
pub const SFACE_EMBEDDING_DIM: usize = 128;
/// OpenCV cosine similarity threshold for identifying the same person with SFace.
pub const SFACE_SIMILARITY_THRESHOLD: f32 = 0.363;

/// Standard ArcFace 112×112 destination facial landmarks:
/// 1. Left eye: `[38.2946, 51.6963]`
/// 2. Right eye: `[73.5318, 51.5014]`
/// 3. Nose tip: `[56.0252, 71.7366]`
/// 4. Left mouth corner: `[41.5493, 92.3655]`
/// 5. Right mouth corner: `[70.7299, 92.2041]`
pub const ARCFACE_DST_LANDMARKS: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// Computes the optimal 2D similarity transformation mapping `src` to `dst`.
/// Returns `(a, b, tx, ty)` such that:
/// `u = a * x - b * y + tx`
/// `v = b * x + a * y + ty`
pub fn estimate_similarity_transform(
    src: &[[f32; 2]; 5],
    dst: &[[f32; 2]; 5],
) -> Option<(f32, f32, f32, f32)> {
    let mut mean_x = 0.0f32;
    let mut mean_y = 0.0f32;
    let mut mean_u = 0.0f32;
    let mut mean_v = 0.0f32;
    for i in 0..5 {
        mean_x += src[i][0];
        mean_y += src[i][1];
        mean_u += dst[i][0];
        mean_v += dst[i][1];
    }
    mean_x /= 5.0;
    mean_y /= 5.0;
    mean_u /= 5.0;
    mean_v /= 5.0;

    let mut sxx = 0.0f32;
    let mut a_num = 0.0f32;
    let mut b_num = 0.0f32;
    for i in 0..5 {
        let dx = src[i][0] - mean_x;
        let dy = src[i][1] - mean_y;
        let du = dst[i][0] - mean_u;
        let dv = dst[i][1] - mean_v;
        sxx += dx * dx + dy * dy;
        a_num += dx * du + dy * dv;
        b_num += dx * dv - dy * du;
    }

    if sxx < 1e-6 {
        return None;
    }

    let a = a_num / sxx;
    let b = b_num / sxx;
    let tx = mean_u - (a * mean_x - b * mean_y);
    let ty = mean_v - (b * mean_x + a * mean_y);

    Some((a, b, tx, ty))
}

/// Aligns a detected face from `image` using its 5 facial landmarks and returns a 112×112 BGR NCHW
/// buffer (`3 * 112 * 112` floats in `0.0..=255.0`), matching OpenCV `FaceRecognizerSF::alignCrop`.
pub fn align_face(image: &RgbImage, landmarks: &[[f32; 2]; 5]) -> Vec<f32> {
    let size = SFACE_INPUT_SIZE as usize;
    let plane = size * size;
    let mut out = vec![0.0f32; 3 * plane];

    let transform = estimate_similarity_transform(landmarks, &ARCFACE_DST_LANDMARKS);
    let (inv_a, inv_b, inv_tx, inv_ty) = match transform {
        Some((a, b, tx, ty)) => {
            let det = a * a + b * b;
            if det > 1e-6 {
                (a / det, -b / det, tx, ty)
            } else {
                (1.0, 0.0, 0.0, 0.0)
            }
        }
        None => (1.0, 0.0, 0.0, 0.0),
    };

    let w = image.width() as f32;
    let h = image.height() as f32;
    let (b_plane, rest) = out.split_at_mut(plane);
    let (g_plane, r_plane) = rest.split_at_mut(plane);

    for v in 0..size {
        let vf = v as f32;
        let row_offset = v * size;
        for u in 0..size {
            let uf = u as f32;
            let du = uf - inv_tx;
            let dv = vf - inv_ty;
            // Inverse similarity transform:
            let src_x = inv_a * du - inv_b * dv;
            let src_y = inv_b * du + inv_a * dv;

            let (r, g, b) = sample_bilinear(image, src_x, src_y, w, h);
            let idx = row_offset + u;
            // SFace expects BGR order with raw 0..255 values
            b_plane[idx] = b;
            g_plane[idx] = g;
            r_plane[idx] = r;
        }
    }

    out
}

/// Samples an image using bilinear interpolation. Returns black if completely out of bounds.
fn sample_bilinear(image: &RgbImage, x: f32, y: f32, width: f32, height: f32) -> (f32, f32, f32) {
    if x < -1.0 || x > width || y < -1.0 || y > height {
        return (0.0, 0.0, 0.0);
    }
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let x1 = x0 + 1;
    let y1 = y0 + 1;

    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let w00 = (1.0 - fx) * (1.0 - fy);
    let w10 = fx * (1.0 - fy);
    let w01 = (1.0 - fx) * fy;
    let w11 = fx * fy;

    let get_px = |px: i32, py: i32| -> (f32, f32, f32) {
        if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
            let pixel = image.get_pixel(px as u32, py as u32);
            (
                f32::from(pixel[0]),
                f32::from(pixel[1]),
                f32::from(pixel[2]),
            )
        } else {
            (0.0, 0.0, 0.0)
        }
    };

    let p00 = get_px(x0, y0);
    let p10 = get_px(x1, y0);
    let p01 = get_px(x0, y1);
    let p11 = get_px(x1, y1);

    let r = w00 * p00.0 + w10 * p10.0 + w01 * p01.0 + w11 * p11.0;
    let g = w00 * p00.1 + w10 * p10.1 + w01 * p01.1 + w11 * p11.1;
    let b = w00 * p00.2 + w10 * p10.2 + w01 * p01.2 + w11 * p11.2;

    (r, g, b)
}

pub struct SFace {
    session: Session,
    input_name: String,
    output_name: String,
}

impl SFace {
    /// Verifies the pinned SFace file and loads the ONNX session, validating input and output shapes.
    pub fn load(models_root: &Path, options: &SessionOptions) -> Result<Self, String> {
        verify_sface(models_root)
            .map_err(|e| format!("refusing to load SFace model: {}", e.join("; ")))?;
        let session = build_session(&models_root.join(SFACE_MODEL), options)?;

        let inputs = session.inputs();
        if inputs.is_empty() {
            return Err("SFace ONNX model has no inputs".into());
        }
        let input_name = inputs[0].name().to_string();

        let outputs = session.outputs();
        if outputs.is_empty() {
            return Err("SFace ONNX model has no outputs".into());
        }
        let output_name = outputs[0].name().to_string();

        Ok(Self {
            session,
            input_name,
            output_name,
        })
    }

    /// Extracts a 128-dimensional L2-normalized face embedding from an aligned 112×112 BGR NCHW buffer.
    pub fn feature(
        &mut self,
        aligned_bgr_nchw: &[f32],
    ) -> Result<[f32; SFACE_EMBEDDING_DIM], String> {
        let size = SFACE_INPUT_SIZE as usize;
        let expected = 3 * size * size;
        if aligned_bgr_nchw.len() != expected {
            return Err(format!(
                "aligned face length {}, expected {expected}",
                aligned_bgr_nchw.len()
            ));
        }

        let input = TensorRef::from_array_view(([1, 3, size, size], aligned_bgr_nchw))
            .map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => input])
            .map_err(|e| e.to_string())?;

        let val = outputs
            .get(&self.output_name)
            .ok_or_else(|| format!("sface output missing `{}`", self.output_name))?;
        let (_, data) = val.try_extract_tensor::<f32>().map_err(|e| e.to_string())?;

        if data.len() != SFACE_EMBEDDING_DIM {
            return Err(format!(
                "sface output dimension {}, expected {SFACE_EMBEDDING_DIM}",
                data.len()
            ));
        }

        let mut out = [0.0f32; SFACE_EMBEDDING_DIM];
        out.copy_from_slice(data);
        l2_normalize(&mut out);
        Ok(out)
    }

    /// Aligns a face from `image` and extracts its normalized embedding in one step.
    pub fn embed_face(
        &mut self,
        image: &RgbImage,
        landmarks: &[[f32; 2]; 5],
    ) -> Result<[f32; SFACE_EMBEDDING_DIM], String> {
        let aligned = align_face(image, landmarks);
        self.feature(&aligned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn similarity_transform_recovers_known_affine_transform() {
        // Apply rotation (theta=30 deg), scale (s=1.5), translation (tx=12, ty=-8) to canonical points
        let theta = 30.0f32.to_radians();
        let s = 1.5f32;
        let cos_t = theta.cos() * s;
        let sin_t = theta.sin() * s;
        let tx = 12.0f32;
        let ty = -8.0f32;

        let mut transformed = [[0.0f32; 2]; 5];
        for (i, p) in ARCFACE_DST_LANDMARKS.iter().enumerate() {
            transformed[i][0] = cos_t * p[0] - sin_t * p[1] + tx;
            transformed[i][1] = sin_t * p[0] + cos_t * p[1] + ty;
        }

        // Estimate transform mapping transformed -> ARCFACE_DST_LANDMARKS
        let (a, b, est_tx, est_ty) =
            estimate_similarity_transform(&transformed, &ARCFACE_DST_LANDMARKS)
                .expect("valid transform");

        for (i, p) in transformed.iter().enumerate() {
            let u = a * p[0] - b * p[1] + est_tx;
            let v = b * p[0] + a * p[1] + est_ty;
            let target = &ARCFACE_DST_LANDMARKS[i];
            assert!(
                (u - target[0]).abs() < 1e-3,
                "u mismatch on landmark {i}: {u} vs {}",
                target[0]
            );
            assert!(
                (v - target[1]).abs() < 1e-3,
                "v mismatch on landmark {i}: {v} vs {}",
                target[1]
            );
        }
    }

    #[test]
    fn align_face_generates_correct_buffer_size() {
        let img = RgbImage::new(200, 200);
        let aligned = align_face(&img, &ARCFACE_DST_LANDMARKS);
        assert_eq!(aligned.len(), 3 * 112 * 112);
    }
}
