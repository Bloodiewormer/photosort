use std::path::PathBuf;

/// The folder with the real models, or `None` (with a message) so the suite passes without them.
pub fn models_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("PHOTOSORT_OCR_MODELS").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("SKIP: set PHOTOSORT_OCR_MODELS to the pinned model folder to run this test");
    }
    dir
}
