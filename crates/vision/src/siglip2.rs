//! SigLIP 2 (base-patch16-256) image and text embeddings, L2-normalized.
//!
//! The split ONNX exports (`vision_model_fp16.onnx`, `text_model_int8.onnx`) do not contain
//! `logit_scale` or `logit_bias`, so this module exposes cosine similarity only — never invented
//! probabilities.

use crate::manifest::{SIGLIP2_TEXT, SIGLIP2_TOKENIZER, SIGLIP2_VISION, verify_siglip2};
use crate::session::{SessionOptions, build_session};
use image::RgbImage;
use image::imageops::{FilterType, resize};
use ort::session::Session;
use ort::value::TensorRef;
use std::path::Path;
use tokenizers::Tokenizer;

pub const IMAGE_SIZE: u32 = 256;
pub const EMBEDDING_DIM: usize = 768;
pub const MAX_TEXT_TOKENS: usize = 64;
pub const PAD_TOKEN_ID: i64 = 0;
pub const EOS_TOKEN_ID: i64 = 1;

/// Preprocesses one RGB image per `preprocessor_config.json`:
/// squash to 256×256 (no crop) with antialiased bilinear (`FilterType::Triangle`),
/// `x / 255.0`, `(x - 0.5) / 0.5`, NCHW layout (`3 * 256 * 256` floats).
pub fn preprocess_image(image: &RgbImage) -> Vec<f32> {
    let resized = if image.width() == IMAGE_SIZE && image.height() == IMAGE_SIZE {
        std::borrow::Cow::Borrowed(image)
    } else {
        std::borrow::Cow::Owned(resize(image, IMAGE_SIZE, IMAGE_SIZE, FilterType::Triangle))
    };
    let plane = (IMAGE_SIZE * IMAGE_SIZE) as usize;
    let mut out = vec![0.0f32; 3 * plane];
    let (r_plane, rest) = out.split_at_mut(plane);
    let (g_plane, b_plane) = rest.split_at_mut(plane);
    for (idx, px) in resized.pixels().enumerate() {
        r_plane[idx] = (f32::from(px[0]) / 255.0 - 0.5) / 0.5;
        g_plane[idx] = (f32::from(px[1]) / 255.0 - 0.5) / 0.5;
        b_plane[idx] = (f32::from(px[2]) / 255.0 - 0.5) / 0.5;
    }
    out
}

/// Preprocesses a batch of images into a contiguous `[N, 3, 256, 256]` f32 buffer.
pub fn preprocess_batch(images: &[&RgbImage]) -> Vec<f32> {
    let per_image = 3 * (IMAGE_SIZE * IMAGE_SIZE) as usize;
    let mut out = Vec::with_capacity(images.len() * per_image);
    for img in images {
        out.extend_from_slice(&preprocess_image(img));
    }
    out
}

/// L2-normalizes a vector in place. Leaves a zero vector unchanged.
pub fn l2_normalize(v: &mut [f32]) {
    let norm_sq: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
    let norm = norm_sq.sqrt();
    if norm > 0.0 {
        let inv = (1.0 / norm) as f32;
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

/// Cosine similarity between two equal-length embeddings.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len(), "embedding length mismatch");
    let mut dot = 0.0f64;
    let mut na = 0.0f64;
    let mut nb = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let xf = f64::from(x);
        let yf = f64::from(y);
        dot += xf * yf;
        na += xf * xf;
        nb += yf * yf;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (dot / denom) as f32
    }
}

/// Tokenizes text per `tokenizer_config.json`:
/// 1. Lowercase first (`do_lower_case: true`; `tokenizer.json` does not lowercase).
/// 2. Encode without special tokens, truncate to at most `63` tokens.
/// 3. Append `<eos>` (`1`) and right-pad with `<pad>` (`0`) to exactly `64` tokens.
pub fn tokenize_text(tokenizer: &Tokenizer, text: &str) -> Result<[i64; MAX_TEXT_TOKENS], String> {
    let lowered = text.to_lowercase();
    let encoding = tokenizer
        .encode(lowered, false)
        .map_err(|e| format!("tokenize: {e}"))?;
    let raw = encoding.get_ids();
    // Ignore any trailing pad tokens that a tokenizer with fixed padding might have added.
    let len = raw
        .iter()
        .rposition(|&id| i64::from(id) != PAD_TOKEN_ID)
        .map_or(0, |last| last + 1);
    let keep = len.min(MAX_TEXT_TOKENS - 1);
    let mut out = [PAD_TOKEN_ID; MAX_TEXT_TOKENS];
    for (dst, &src) in out.iter_mut().zip(&raw[..keep]) {
        *dst = i64::from(src);
    }
    out[keep] = EOS_TOKEN_ID;
    Ok(out)
}

pub struct SigLip2Vision {
    session: Session,
}

impl SigLip2Vision {
    /// Verifies all pinned SigLIP 2 files and loads the vision encoder session.
    pub fn load(models_root: &Path, options: &SessionOptions) -> Result<Self, String> {
        verify_siglip2(models_root)
            .map_err(|e| format!("refusing to load SigLIP 2 models: {}", e.join("; ")))?;
        let session = build_session(&models_root.join(SIGLIP2_VISION), options)?;
        Ok(Self { session })
    }

    /// Preprocesses and embeds a batch of RGB images, returning one L2-normalized 768-d vector each.
    pub fn embed_images(&mut self, images: &[&RgbImage]) -> Result<Vec<Vec<f32>>, String> {
        if images.is_empty() {
            return Ok(Vec::new());
        }
        let nchw = preprocess_batch(images);
        self.embed_preprocessed(images.len(), &nchw)
    }

    /// Runs inference on an already preprocessed `[batch, 3, 256, 256]` f32 NCHW buffer.
    pub fn embed_preprocessed(
        &mut self,
        batch: usize,
        nchw: &[f32],
    ) -> Result<Vec<Vec<f32>>, String> {
        if batch == 0 {
            return Ok(Vec::new());
        }
        let expected = batch * 3 * (IMAGE_SIZE * IMAGE_SIZE) as usize;
        if nchw.len() != expected {
            return Err(format!(
                "pixel_values length {}, expected {expected} for batch {batch}",
                nchw.len()
            ));
        }
        let input = TensorRef::from_array_view((
            [batch, 3, IMAGE_SIZE as usize, IMAGE_SIZE as usize],
            nchw,
        ))
        .map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["pixel_values" => input])
            .map_err(|e| e.to_string())?;
        extract_pooled(&outputs, batch)
    }
}

pub struct SigLip2Text {
    session: Session,
    tokenizer: Tokenizer,
}

impl SigLip2Text {
    /// Verifies all pinned SigLIP 2 files and loads the tokenizer and int8 text encoder.
    pub fn load(models_root: &Path, options: &SessionOptions) -> Result<Self, String> {
        verify_siglip2(models_root)
            .map_err(|e| format!("refusing to load SigLIP 2 models: {}", e.join("; ")))?;
        let mut tokenizer = Tokenizer::from_file(models_root.join(SIGLIP2_TOKENIZER))
            .map_err(|e| format!("tokenizer: {e}"))?;
        // We control truncation and right-padding to 64 explicitly in `tokenize_text`.
        tokenizer.with_padding(None);
        let _ = tokenizer.with_truncation(None);
        let session = build_session(&models_root.join(SIGLIP2_TEXT), options)?;
        Ok(Self { session, tokenizer })
    }

    pub fn tokenize(&self, text: &str) -> Result<[i64; MAX_TEXT_TOKENS], String> {
        tokenize_text(&self.tokenizer, text)
    }

    /// Tokenizes and embeds a batch of texts, returning one L2-normalized 768-d vector each.
    pub fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(texts.len() * MAX_TEXT_TOKENS);
        for text in texts {
            ids.extend_from_slice(&self.tokenize(text)?);
        }
        self.embed_token_ids(texts.len(), &ids)
    }

    /// Runs inference on an already tokenized `[batch, 64]` i64 buffer.
    pub fn embed_token_ids(&mut self, batch: usize, ids: &[i64]) -> Result<Vec<Vec<f32>>, String> {
        if batch == 0 {
            return Ok(Vec::new());
        }
        let expected = batch * MAX_TEXT_TOKENS;
        if ids.len() != expected {
            return Err(format!(
                "input_ids length {}, expected {expected} for batch {batch}",
                ids.len()
            ));
        }
        let input = TensorRef::from_array_view(([batch, MAX_TEXT_TOKENS], ids))
            .map_err(|e| e.to_string())?;
        let outputs = self
            .session
            .run(ort::inputs!["input_ids" => input])
            .map_err(|e| e.to_string())?;
        extract_pooled(&outputs, batch)
    }
}

pub struct SigLip2 {
    pub vision: SigLip2Vision,
    pub text: SigLip2Text,
}

impl SigLip2 {
    pub fn load(models_root: &Path, options: &SessionOptions) -> Result<Self, String> {
        let vision = SigLip2Vision::load(models_root, options)?;
        let text = SigLip2Text::load(models_root, options)?;
        Ok(Self { vision, text })
    }

    pub fn embed_images(&mut self, images: &[&RgbImage]) -> Result<Vec<Vec<f32>>, String> {
        self.vision.embed_images(images)
    }

    pub fn embed_texts(&mut self, texts: &[&str]) -> Result<Vec<Vec<f32>>, String> {
        self.text.embed_texts(texts)
    }
}

fn extract_pooled(
    outputs: &ort::session::SessionOutputs<'_>,
    batch: usize,
) -> Result<Vec<Vec<f32>>, String> {
    let pooled = outputs
        .get("pooler_output")
        .ok_or("model output missing `pooler_output`")?;
    let (shape, data) = pooled
        .try_extract_tensor::<f32>()
        .map_err(|e| e.to_string())?;
    let dims: &[i64] = shape;
    if dims.len() != 2 || dims[0] as usize != batch || dims[1] as usize != EMBEDDING_DIM {
        return Err(format!(
            "unexpected pooler_output shape {dims:?}, expected [{batch}, {EMBEDDING_DIM}]"
        ));
    }
    let mut rows = Vec::with_capacity(batch);
    for chunk in data.as_chunks::<EMBEDDING_DIM>().0 {
        let mut vec = chunk.to_vec();
        l2_normalize(&mut vec);
        rows.push(vec);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn preprocess_maps_black_gray_and_white_to_minus_one_zero_and_one() {
        let black = RgbImage::from_pixel(16, 16, Rgb([0, 0, 0]));
        let white = RgbImage::from_pixel(16, 16, Rgb([255, 255, 255]));
        let b = preprocess_image(&black);
        let w = preprocess_image(&white);
        assert_eq!(b.len(), 3 * 256 * 256);
        assert!(b.iter().all(|&v| (v - (-1.0)).abs() < 1e-6));
        assert!(w.iter().all(|&v| (v - 1.0).abs() < 1e-6));
    }

    #[test]
    fn preprocess_keeps_rgb_channels_in_nchw_order() {
        let img = RgbImage::from_pixel(256, 256, Rgb([255, 0, 128]));
        let buf = preprocess_image(&img);
        let plane = 256 * 256;
        assert!((buf[0] - 1.0).abs() < 1e-6);
        assert!((buf[plane] - (-1.0)).abs() < 1e-6);
        let expected_b = (128.0f32 / 255.0 - 0.5) / 0.5;
        assert!((buf[2 * plane] - expected_b).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_and_cosine_work() {
        let mut a = vec![3.0, 4.0];
        l2_normalize(&mut a);
        assert!((a[0] - 0.6).abs() < 1e-6);
        assert!((a[1] - 0.8).abs() < 1e-6);
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < 1e-6);
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        assert!((cosine_similarity(&[1.0, 0.0], &[-1.0, 0.0]) + 1.0).abs() < 1e-6);
    }
}
