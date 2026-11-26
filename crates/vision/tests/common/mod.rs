use std::path::PathBuf;

// The tests that need these folders are `#[ignore]`d; asking for them without the models is a
// failure, never a silent pass.

/// The folder with the pinned vision models (`siglip2/` and `yunet/`).
pub fn vision_models_dir() -> PathBuf {
    std::env::var_os("PHOTOSORT_VISION_MODELS")
        .map(PathBuf::from)
        .expect("set PHOTOSORT_VISION_MODELS to the pinned vision models root")
}

#[allow(dead_code)]
pub fn ocr_models_dir() -> PathBuf {
    std::env::var_os("PHOTOSORT_OCR_MODELS")
        .map(PathBuf::from)
        .expect("set PHOTOSORT_OCR_MODELS to the pinned OCR models folder")
}
