use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

pub const IMAGE_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "webp", "gif", "bmp", "tif", "tiff", "heic", "heif", "avif",
    "jxl",
];

pub const VIDEO_EXTENSIONS: &[&str] = &["mp4", "webm", "3gp", "mkv", "wmv", "mov", "avi"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum MediaType {
    #[default]
    Image,
    Video,
}

impl std::str::FromStr for MediaType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "image" => Ok(Self::Image),
            "video" => Ok(Self::Video),
            _ => Err(()),
        }
    }
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "image" => Some(Self::Image),
            "video" => Some(Self::Video),
            _ => None,
        }
    }
}

/// A media file found under a library root. Nothing about it is ever modified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub media_type: MediaType,
}

#[derive(Debug, Default)]
pub struct ScanReport {
    pub files: Vec<Found>,
    /// Unreadable entries. They are reported and never stop the scan.
    pub errors: Vec<(PathBuf, String)>,
}

/// Lists the media files (images and videos) under `root`, read-only. Symlinks and junctions are
/// not followed, so a link can neither loop the walk nor lead it outside the library.
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
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(media_type) = media_type_for_path(entry.path()) else {
            continue;
        };
        match entry.metadata() {
            Ok(md) => report.files.push(Found {
                path: entry.into_path(),
                size: md.len(),
                modified: md.modified().ok(),
                media_type,
            }),
            Err(e) => report.errors.push((entry.into_path(), e.to_string())),
        }
    }
    Ok(report)
}

pub fn media_type_for_path(path: &Path) -> Option<MediaType> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaType::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(MediaType::Video)
    } else {
        None
    }
}

pub fn is_image(path: &Path) -> bool {
    media_type_for_path(path) == Some(MediaType::Image)
}

pub fn is_video(path: &Path) -> bool {
    media_type_for_path(path) == Some(MediaType::Video)
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
