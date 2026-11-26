//! A separate test binary: the ONNX Runtime environment, and so the provider, is per process.
mod common;

use ocr::engine::{Engine, Options, Provider};

#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn a_directml_device_that_cannot_register_is_an_error_not_a_cpu_fallback() {
    let models = common::models_dir();
    let options = Options {
        provider: Provider::DirectMl { device: 99 },
        ..Options::default()
    };
    let error = Engine::load(&models, &options)
        .err()
        .expect("device 99 does not exist: loading must fail");
    println!("{error}");
    // The cause must be the provider's registration, not something wrong with the model file.
    assert!(error.contains("dml") || error.contains("DML"), "{error}");
}
