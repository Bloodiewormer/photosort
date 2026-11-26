mod common;

use ocr::engine::{Engine, Options};
use ocr::manifest::{PINNED, REC_KOREAN};

#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
#[allow(clippy::disallowed_methods)] // copies the models into a temporary folder to tamper with them
fn refuses_to_load_a_tampered_or_incomplete_model_folder() {
    let models = common::models_dir();
    let dir = tempfile::tempdir().unwrap();
    for file in &PINNED {
        std::fs::copy(models.join(file.name), dir.path().join(file.name)).unwrap();
    }
    let target = dir.path().join(REC_KOREAN);
    let mut bytes = std::fs::read(&target).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&target, bytes).unwrap();
    let error = Engine::load(dir.path(), &Options::default())
        .err()
        .expect("a changed byte must be refused");
    assert!(error.contains("sha256"), "{error}");

    std::fs::remove_file(&target).unwrap();
    let error = Engine::load(dir.path(), &Options::default())
        .err()
        .expect("a missing file must be refused");
    assert!(error.contains(REC_KOREAN), "{error}");
}
