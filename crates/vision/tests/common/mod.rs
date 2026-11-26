use std::path::PathBuf;

/// The folder with the pinned vision models (`siglip2/` and `yunet/`), or `None` so tests skip
/// cleanly when models are absent.
pub fn vision_models_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHOTOSORT_VISION_MODELS").map(PathBuf::from);
    if dir.is_none() {
        eprintln!(
            "SKIP: set PHOTOSORT_VISION_MODELS to the pinned vision models root to run this test"
        );
    }
    dir
}

#[allow(dead_code)]
pub fn ocr_models_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHOTOSORT_OCR_MODELS").map(PathBuf::from);
    if dir.is_none() {
        eprintln!(
            "SKIP: set PHOTOSORT_OCR_MODELS to the pinned OCR models folder to run this test"
        );
    }
    dir
}
