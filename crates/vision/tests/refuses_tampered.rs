mod common;

use vision::manifest::{SIGLIP2_CONFIG, YUNET_MODEL};
use vision::{SessionOptions, SigLip2Vision, YuNet};

#[test]
#[allow(clippy::disallowed_methods)] // copies a small model into a temporary folder to tamper with it
fn refuses_tampered_and_missing_models() {
    let Some(models) = common::vision_models_dir() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("yunet")).unwrap();
    let target = dir.path().join(YUNET_MODEL);
    std::fs::copy(models.join(YUNET_MODEL), &target).unwrap();

    let mut bytes = std::fs::read(&target).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&target, bytes).unwrap();
    let err = YuNet::load(dir.path(), &SessionOptions::default())
        .err()
        .expect("a tampered YuNet file must be refused");
    assert!(err.contains("sha256"), "{err}");

    std::fs::remove_file(&target).unwrap();
    let err = YuNet::load(dir.path(), &SessionOptions::default())
        .err()
        .expect("a missing YuNet file must be refused");
    assert!(err.contains(YUNET_MODEL), "{err}");

    // SigLIP 2 also refuses an incomplete folder before touching ONNX Runtime.
    let err = SigLip2Vision::load(dir.path(), &SessionOptions::default())
        .err()
        .expect("an empty SigLIP 2 folder must be refused");
    assert!(err.contains(SIGLIP2_CONFIG), "{err}");
}
