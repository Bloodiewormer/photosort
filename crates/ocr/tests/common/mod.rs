use std::path::PathBuf;

/// The folder with the real models. The tests that need it are `#[ignore]`d; asking for them
/// without the models is a failure, never a silent pass.
pub fn models_dir() -> PathBuf {
    std::env::var_os("PHOTOSORT_OCR_MODELS")
        .map(PathBuf::from)
        .expect("set PHOTOSORT_OCR_MODELS to the pinned model folder")
}
