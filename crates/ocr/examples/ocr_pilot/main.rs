//! OCR over a small random sample of real photos, for the user to review locally.
//!
//! ```text
//! ocr_pilot --folder <dir> [--folder <dir>...] --out <new dir> --models <dir>
//!           [--count 150] [--seed 1] [--provider dml:0|cpu]
//! ```
//!
//! Privacy: standard output and standard error carry only counts and timings. OCR text, file names
//! and paths go to the local report and nowhere else.

mod report;

use fsguard::{DestPath, LibraryRoot, ensure_export_dir};
use ocr::engine::{Engine, Options, Provider};
use ocr::eval::chosen;
use ocr::select::{RECOMMENDED, REVIEW_BELOW, Source, Strategy, choose};
use std::collections::BTreeSet;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Instant, SystemTime};
use thumbs::Thumbs;

const MAX_COUNT: usize = 200;
/// Large enough to check small text by eye in the report.
const THUMB_PX: u32 = 1600;
const EXTENSIONS: [&str; 8] = ["jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff"];

struct Config {
    folders: Vec<PathBuf>,
    out: PathBuf,
    models: PathBuf,
    count: usize,
    seed: u64,
    provider: Provider,
}

fn parse(args: &[String]) -> Result<Config, String> {
    let (mut folders, mut out, mut models) = (Vec::new(), None, None);
    let (mut count, mut seed) = (150, 1);
    let mut provider = Provider::DirectMl { device: 0 };
    for pair in args.chunks(2) {
        let [name, value] = pair else {
            return Err("every option needs a value".into());
        };
        // Values are never echoed back: they can be private paths.
        let bad = || format!("invalid value for {name}");
        match name.as_str() {
            "--folder" => folders.push(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            "--models" => models = Some(PathBuf::from(value)),
            "--count" => count = value.parse().map_err(|_| bad())?,
            "--seed" => seed = value.parse().map_err(|_| bad())?,
            "--provider" => provider = value.parse().map_err(|_| bad())?,
            _ => return Err("unknown option".into()),
        }
    }
    if folders.is_empty() {
        return Err("at least one --folder is required".into());
    }
    if !(1..=MAX_COUNT).contains(&count) {
        return Err(format!("--count must be between 1 and {MAX_COUNT}"));
    }
    Ok(Config {
        folders,
        out: out.ok_or("--out is required")?,
        models: models.ok_or("--models is required")?,
        count,
        seed,
        provider,
    })
}

/// The output folder, proven to be outside every sampled folder and not to contain one.
fn guard_out(folders: &[PathBuf], out: &Path) -> Result<DestPath, String> {
    let roots = folders
        .iter()
        .enumerate()
        .map(|(i, folder)| {
            LibraryRoot::new(folder).map_err(|e| format!("--folder number {}: {}", i + 1, e.kind()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    ensure_export_dir(out, &roots)
        .map_err(|_| "--out must be a plain folder outside every --folder".into())
}

/// `count` files in a pseudo-random order that depends only on `seed`, taking one from each folder
/// in turn so that every folder gets an equal share while it has files left.
fn pick(folders: Vec<Vec<PathBuf>>, count: usize, seed: u64) -> Vec<PathBuf> {
    // shortcut: `DefaultHasher` is only stable within one Rust version, which is pinned.
    let rank = |path: &PathBuf| {
        let mut hasher = DefaultHasher::new();
        (seed, path).hash(&mut hasher);
        hasher.finish()
    };
    let mut queues: Vec<_> = folders
        .into_iter()
        .map(|mut files| {
            files.sort_by_cached_key(rank);
            files.into_iter()
        })
        .collect();
    let (mut picked, mut seen) = (Vec::new(), BTreeSet::new());
    while picked.len() < count {
        let before = seen.len();
        for queue in &mut queues {
            if picked.len() < count
                && let Some(path) = queue.next()
                && seen.insert(path.clone())
            {
                picked.push(path);
            }
        }
        if seen.len() == before && queues.iter().all(|q| q.len() == 0) {
            break;
        }
    }
    picked
}

/// What must be identical before and after reading an original.
#[derive(PartialEq)]
struct Fingerprint {
    sha256: String,
    size: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
}

fn fingerprint(path: &Path) -> std::io::Result<Fingerprint> {
    let metadata = std::fs::metadata(path)?;
    Ok(Fingerprint {
        sha256: scanner::sha256_file(path)?,
        size: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
    })
}

fn analyse(
    engine: &Engine,
    thumbs: &Thumbs,
    path: &Path,
    sha256: &str,
) -> Result<report::Image, String> {
    let pixels = thumbs::decode(path, u32::MAX)
        .map_err(|e| e.to_string())?
        .to_rgb8();
    let thumb = thumbs.get(path, sha256).map_err(|e| e.to_string())?;
    let start = Instant::now();
    let (lines, _) = engine.read(&pixels, |_| true, &mut Vec::new())?;
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    let strategy = Strategy::Both(RECOMMENDED);
    let lines = lines
        .iter()
        .filter_map(|line| {
            let reading = chosen(strategy, line);
            (!reading.text.trim().is_empty()).then(|| report::Line {
                rect: line.rect,
                text: reading.text.clone(),
                score: reading.score,
                korean: choose(strategy, &line.v6, line.korean.as_ref()) == Source::Korean,
            })
        })
        .collect();
    Ok(report::Image {
        thumb: thumb
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        width: pixels.width(),
        height: pixels.height(),
        ms,
        lines,
    })
}

fn run(config: &Config, say: &mut dyn Write) -> Result<(), String> {
    let out = guard_out(&config.folders, &config.out)?;
    let report_path = out.join_name("report.html".as_ref()).ok_or("bad name")?;
    if report_path.as_path().exists() {
        return Err("--out already has a report; choose a new folder".into());
    }
    let say_line =
        |say: &mut dyn Write, text: String| writeln!(say, "{text}").map_err(|e| e.to_string());

    let mut found = Vec::new();
    for (i, folder) in config.folders.iter().enumerate() {
        let scan = scanner::scan(folder)
            .map_err(|e| format!("--folder number {}: {}", i + 1, e.kind()))?;
        let files: Vec<PathBuf> = scan
            .files
            .into_iter()
            .map(|f| f.path)
            .filter(|p| {
                p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
            })
            .collect();
        say_line(
            say,
            format!(
                "folder {}: {} supported images, {} unreadable entries",
                i + 1,
                files.len(),
                scan.errors.len()
            ),
        )?;
        found.push(files);
    }
    let sample = pick(found, config.count, config.seed);
    if sample.is_empty() {
        return Err("no supported images found".into());
    }

    let engine = Engine::load(
        &config.models,
        &Options {
            provider: config.provider,
            ..Options::default()
        },
    )?;
    let thumbs_dir = out.join_name("thumbs".as_ref()).ok_or("bad name")?;
    // Creates `out` too. The error is not shown: it can hold a path.
    let thumbs = Thumbs::new(&thumbs_dir, THUMB_PX).map_err(|_| "cannot create --out")?;

    let started = Instant::now();
    let mut entries = Vec::new();
    let mut changed = 0;
    for (i, path) in sample.iter().enumerate() {
        let outcome = match fingerprint(path) {
            Err(e) => Err(e.to_string()),
            Ok(before) => {
                // A corrupt file must not stop the batch, even if a decoder panics on it.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    analyse(&engine, &thumbs, path, &before.sha256)
                }))
                .unwrap_or_else(|_| Err("internal error while reading this image".into()));
                changed += usize::from(fingerprint(path).ok() != Some(before));
                outcome
            }
        };
        entries.push(report::Entry {
            name: path.display().to_string(),
            outcome,
        });
        if (i + 1) % 10 == 0 || i + 1 == sample.len() {
            say_line(say, format!("{}/{}", i + 1, sample.len()))?;
        }
    }

    let summary = report::Summary::of(&entries, config.provider.to_string());
    report::write(&report_path, &entries, &summary, REVIEW_BELOW, config.seed)?;
    say_line(say, summary.to_string())?;
    say_line(
        say,
        format!("total {:.1} s", started.elapsed().as_secs_f64()),
    )?;
    if changed > 0 {
        return Err(format!(
            "{changed} originals differ after being read: stop and check them"
        ));
    }
    say_line(
        say,
        "originals checked after reading: content, size, modified and created dates unchanged"
            .into(),
    )?;
    say_line(say, "report.html written in --out".into())
}

fn main() -> ExitCode {
    // The default panic message can quote the data being processed.
    std::panic::set_hook(Box::new(|info| {
        eprintln!("internal error at {:?}", info.location());
    }));
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args).and_then(|config| run(&config, &mut std::io::stdout())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests;
