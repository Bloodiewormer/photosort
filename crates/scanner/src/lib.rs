use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif", "jxl",
];

/// An image file found under a library root. Nothing about it is ever modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<Found>,
    /// Unreadable entries. They are reported and never stop the scan.
    pub errors: Vec<(PathBuf, String)>,
}

/// Lists the images under `root`, read-only. Symlinks and junctions are not followed, so a link
/// can neither loop the walk nor lead it outside the library.
pub fn scan(root: &Path) -> io::Result<ScanReport> {
    if !std::fs::metadata(root)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a directory",
        ));
    }
    let mut report = ScanReport::default();
    for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let path = e.path().unwrap_or(root).to_path_buf();
                report.errors.push((path, e.to_string()));
                continue;
            }
        };
        if !entry.file_type().is_file() || !is_image(entry.path()) {
            continue;
        }
        match entry.metadata() {
            Ok(md) => report.files.push(Found {
                path: entry.into_path(),
                size: md.len(),
                modified: md.modified().ok(),
            }),
            Err(e) => report.errors.push((entry.into_path(), e.to_string())),
        }
    }
    Ok(report)
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Lowercase hex SHA-256 of the file's bytes. The file is only opened for reading.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut block = vec![0u8; 1 << 20];
    loop {
        match file.read(&mut block) {
            Ok(0) => break,
            Ok(n) => hasher.update(&block[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
