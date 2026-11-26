//! Separate test binary: proves loading vision first, then OCR, shares one ONNX Runtime
//! environment and enforces the one-provider-per-process rule.
mod common;

use vision::{Provider, SessionOptions, YuNet};

#[test]
fn vision_then_ocr_shares_runtime_and_rejects_provider_switch() {
    let (Some(ocr_dir), Some(vision_dir)) = (common::ocr_models_dir(), common::vision_models_dir())
    else {
        return;
    };

    let _yunet = YuNet::load(&vision_dir, &SessionOptions::default())
        .expect("YuNet load on CPU must succeed");
    let _ocr = ocr::engine::Engine::load(&ocr_dir, &ocr::engine::Options::default())
        .expect("OCR load on same CPU provider must succeed after vision");

    let err = ocr::engine::Engine::load(
        &ocr_dir,
        &ocr::engine::Options {
            provider: Provider::DirectMl { device: 0 },
            ..ocr::engine::Options::default()
        },
    )
    .err()
    .expect("switching provider in the same process must fail");
    assert!(err.contains("already runs on cpu"), "{err}");
}
