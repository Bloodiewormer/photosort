mod common;

use image::RgbImage;
use vision::{
    ARCFACE_DST_LANDMARKS, SFACE_SIMILARITY_THRESHOLD, SFace, SessionOptions, align_face,
};

#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn sface_loads_and_extracts_normalized_feature() {
    let models = common::vision_models_dir();
    let opts = SessionOptions::default();
    let mut sface = match SFace::load(&models, &opts) {
        Ok(s) => s,
        Err(e) => panic!("failed to load SFace: {e}"),
    };

    let img = RgbImage::new(112, 112);
    let aligned = align_face(&img, &ARCFACE_DST_LANDMARKS);
    let feat = sface
        .feature(&aligned)
        .expect("feature extraction should succeed");
    assert_eq!(feat.len(), 128);

    // Verify L2 norm is ~1.0
    let norm_sq: f32 = feat.iter().map(|x| x * x).sum();
    let norm = norm_sq.sqrt();
    assert!(
        (norm - 1.0).abs() < 1e-4,
        "SFace feature should be L2 normalized, got norm {norm}"
    );

    // Self similarity of normalized feature is 1.0 >= SFACE_SIMILARITY_THRESHOLD
    let self_sim: f32 = feat.iter().zip(feat.iter()).map(|(a, b)| a * b).sum();
    assert!(self_sim >= SFACE_SIMILARITY_THRESHOLD);
}
