//! Shared ONNX Runtime session builder using the single process-wide environment in `ocr`.

use ocr::engine::{Provider, init_runtime};
use ort::logging::{LogLevel, LoggerFunction};
use ort::session::Session;
use std::path::Path;

#[derive(Clone)]
pub struct SessionOptions {
    pub provider: Provider,
    /// `None` leaves ONNX Runtime's default intra-op thread count.
    pub intra_threads: Option<usize>,
    /// Optional verbose ONNX Runtime logger (e.g. for node placement inspection).
    pub log: Option<LoggerFunction>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            provider: Provider::Cpu,
            intra_threads: None,
            log: None,
        }
    }
}

/// Initializes the shared process-wide ONNX Runtime environment and builds a session with the
/// options required by DirectML (sequential execution, memory pattern disabled on DirectML).
pub fn build_session(model_path: &Path, options: &SessionOptions) -> Result<Session, String> {
    let env = init_runtime(options.provider, options.log.clone())?;
    let mut builder = Session::builder(&env)
        .map_err(|e| e.to_string())?
        .with_parallel_execution(false)
        .map_err(|e| e.to_string())?;
    if let Some(threads) = options.intra_threads {
        builder = builder
            .with_intra_threads(threads)
            .map_err(|e| e.to_string())?;
    }
    if matches!(options.provider, Provider::DirectMl { .. }) {
        builder = builder
            .with_memory_pattern(false)
            .map_err(|e| e.to_string())?;
    }
    if options.log.is_some() {
        builder = builder
            .with_log_level(LogLevel::Verbose)
            .map_err(|e| e.to_string())?;
    }
    builder
        .commit_from_file(model_path)
        .map_err(|e| format!("{}: {e}", model_path.display()))
}
