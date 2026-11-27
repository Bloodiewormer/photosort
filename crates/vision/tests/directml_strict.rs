//! Separate test binary: the ONNX Runtime environment, and so the provider, is per process.
mod common;

use vision::{Provider, SessionOptions, SigLip2Vision};

#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn a_directml_device_that_cannot_register_is_an_error_not_a_cpu_fallback() {
    let models = common::vision_models_dir();
    let options = SessionOptions {
        provider: Provider::DirectMl { device: 99 },
        ..SessionOptions::default()
    };
    let error = SigLip2Vision::load(&models, &options)
        .err()
        .expect("device 99 does not exist: loading must fail");
    println!("{error}");
    assert!(
        error.contains("dml") || error.contains("DML") || error.contains("DirectML"),
        "{error}"
    );
}
