//! Evaluation and benchmark harness for `crates/vision`.
//!
//! Subcommands:
//!   inspect    — prints real ONNX inputs, outputs, dtypes and shapes for all 3 models
//!   nodes      — prints ONNX Runtime node placement (DirectML vs CPU) and loaded DLLs
//!   retrieval  — runs synthetic EN/ES retrieval (T->I and I->T) and YuNet on synthetic images
//!   agree      — compares CPU vs DirectML embeddings in separate processes
//!   bench-one  — benchmarks one (task, provider, threads, batch) configuration (--release)
//!   matrix     — runs the full benchmark matrix across >= 3 processes per configuration (--release)

mod bench;
mod probe;
mod retrieval;

use ort::session::Session;
use std::path::PathBuf;
use std::str::FromStr;
use vision::manifest::{SIGLIP2_TEXT, SIGLIP2_VISION, YUNET_MODEL, verify_all};
use vision::{Provider, init_runtime};

pub struct Args {
    items: Vec<String>,
}

impl Args {
    pub fn value(&self, flag: &str) -> Option<&str> {
        self.items
            .windows(2)
            .find(|w| w[0] == flag)
            .map(|w| w[1].as_str())
    }

    pub fn parsed<T: FromStr>(&self, flag: &str, default: T) -> Result<T, String>
    where
        T::Err: std::fmt::Display,
    {
        match self.value(flag) {
            None => Ok(default),
            Some(v) => v.parse().map_err(|e| format!("{flag} {v}: {e}")),
        }
    }

    pub fn models(&self) -> Result<PathBuf, String> {
        self.value("--models")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("PHOTOSORT_VISION_MODELS").map(PathBuf::from))
            .ok_or_else(|| "pass --models <dir> or set PHOTOSORT_VISION_MODELS".into())
    }
}

fn inspect(args: &Args) -> Result<(), String> {
    let models = args.models()?;
    verify_all(&models).map_err(|e| e.join("; "))?;
    let env = init_runtime(Provider::Cpu, None)?;
    for rel in [SIGLIP2_VISION, SIGLIP2_TEXT, YUNET_MODEL] {
        let session = Session::builder(&env)
            .map_err(|e| e.to_string())?
            .with_parallel_execution(false)
            .map_err(|e| e.to_string())?
            .commit_from_file(models.join(rel))
            .map_err(|e| format!("{rel}: {e}"))?;
        println!("=== {rel} ===");
        for inp in session.inputs() {
            println!("  IN : {} -> {:?}", inp.name(), inp.dtype());
        }
        for out in session.outputs() {
            println!("  OUT: {} -> {:?}", out.name(), out.dtype());
        }
    }
    Ok(())
}

fn main() -> Result<(), String> {
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() {
        return Err("usage: vision_eval <inspect|nodes|retrieval|agree|embed-dump|bench-one|matrix> [flags]".into());
    }
    let cmd = raw.remove(0);
    let args = Args { items: raw };
    match cmd.as_str() {
        "inspect" => inspect(&args),
        "nodes" => probe::nodes(&args),
        "retrieval" => retrieval::retrieval(&args),
        "embed-dump" => retrieval::embed_dump(&args),
        "agree" => retrieval::agree(&args),
        "bench-one" => bench::one(&args),
        "matrix" => bench::matrix(&args),
        other => Err(format!("unknown subcommand `{other}`")),
    }
}
