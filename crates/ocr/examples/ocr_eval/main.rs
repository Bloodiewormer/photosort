//! Local OCR evaluation on synthetic images only. Always run with `--release`.
//!
//! ```text
//! ocr_eval samples --out <new folder>
//! ocr_eval eval    --models <dir> [--provider cpu|dml] [--limit-side-len 960]
//! ocr_eval nodes   --models <dir> [--provider dml]
//! ocr_eval matrix  --models <dir> [--reps 3] [--passes 2]
//! ocr_eval bench   --models <dir> --provider cpu|dml --regime fixed|variable
//!                  [--threads default|N] [--rec-batch 6] [--passes 2]
//! ```

mod accuracy;
mod bench;
mod probe;

use fsguard::DestPath;
use image::ImageEncoder;
use image::codecs::png::PngEncoder;
use ocr::engine::{LIMIT_SIDE_LEN, Provider};
use ocr::synth::{Fonts, Split, generate};
use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::str::FromStr;

pub struct Args(Vec<String>);

impl Args {
    pub fn value(&self, name: &str) -> Option<&str> {
        let at = self.0.iter().position(|a| a == name)?;
        self.0.get(at + 1).map(String::as_str)
    }

    pub fn parsed<T: FromStr>(&self, name: &str, default: T) -> Result<T, String>
    where
        T::Err: std::fmt::Display,
    {
        match self.value(name) {
            Some(text) => text.parse().map_err(|e| format!("{name} {text}: {e}")),
            None => Ok(default),
        }
    }

    pub fn models(&self) -> Result<PathBuf, String> {
        self.value("--models")
            .map(PathBuf::from)
            .ok_or_else(|| "--models <dir> is required".into())
    }
}

fn main() -> ExitCode {
    let args = Args(std::env::args().skip(1).collect());
    let result = match args.0.first().map(String::as_str) {
        Some("samples") => samples(&args),
        Some("eval") => (|| {
            let provider = args.parsed("--provider", Provider::Cpu)?;
            let limit = args.parsed("--limit-side-len", LIMIT_SIDE_LEN)?;
            accuracy::run(&args.models()?, provider, limit)
        })(),
        Some("nodes") => probe::nodes(&args),
        Some("bench") => bench::one(&args),
        Some("matrix") => bench::matrix(&args),
        _ => Err("usage: ocr_eval samples|eval|nodes|bench|matrix [options]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Saves the first held-out image of each category as PNG, into a folder of synthetic images.
fn samples(args: &Args) -> Result<(), String> {
    let out = PathBuf::from(args.value("--out").ok_or("--out <folder> is required")?);
    // No library is registered by this tool; it never opens one.
    let out = fsguard::ensure_writable(&out, &[]).map_err(|e| e.to_string())?;
    create_folder(&out)?;
    let mut seen = HashSet::new();
    for sample in generate(Split::HeldOut, &Fonts::load()?) {
        if !seen.insert(sample.category) {
            continue;
        }
        let name = format!("{:?}.png", sample.category).to_lowercase();
        let path = out.join_name(name.as_ref()).map_err(|e| e.to_string())?;
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(
                sample.image.as_raw(),
                sample.image.width(),
                sample.image.height(),
                image::ExtendedColorType::Rgb8,
            )
            .map_err(|e| e.to_string())?;
        write_new(&path, &png)?;
        println!("{}", path.as_path().display());
    }
    Ok(())
}

#[allow(clippy::disallowed_methods)] // creates the output folder, a `DestPath`
fn create_folder(folder: &DestPath) -> Result<(), String> {
    std::fs::create_dir_all(folder.as_path()).map_err(|e| e.to_string())
}

#[allow(clippy::disallowed_methods)] // `create_new` on a `DestPath`: nothing is ever overwritten
fn write_new(path: &DestPath, bytes: &[u8]) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.as_path())
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|e| format!("{}: {e}", path.as_path().display()))
}
