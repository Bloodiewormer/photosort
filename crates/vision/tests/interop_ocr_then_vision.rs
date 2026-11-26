//! Separate test binary: proves loading OCR first, then vision, shares one ONNX Runtime
//! environment and enforces the one-provider-per-process rule.
mod common;

use vision::{Provider, SessionOptions, YuNet};

#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn ocr_then_vision_shares_runtime_and_rejects_provider_switch() {
    let (ocr_dir, vision_dir) = (common::ocr_models_dir(), common::vision_models_dir());

    let _ocr = ocr::engine::Engine::load(&ocr_dir, &ocr::engine::Options::default())
        .expect("OCR load on CPU must succeed");
    let _yunet = YuNet::load(&vision_dir, &SessionOptions::default())
        .expect("YuNet load on same CPU provider must succeed after OCR");

    let err = YuNet::load(
        &vision_dir,
        &SessionOptions {
            provider: Provider::DirectMl { device: 0 },
            ..SessionOptions::default()
        },
    )
    .err()
    .expect("switching provider in the same process must fail");
    assert!(err.contains("already runs on cpu"), "{err}");
}
