use crate::worker::{NoThumbnails, ThumbnailProvider};
use fsguard::{LibraryRoot, ensure_export_dir};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Longest side of a cached thumbnail; a little above the grid cell so it stays sharp on scaled screens.
const THUMB_PX: u32 = 256;

impl ThumbnailProvider for thumbs::Thumbs {
    fn get(&self, source: &Path, sha256: &str) -> io::Result<PathBuf> {
        thumbs::Thumbs::get(self, source, sha256).map_err(io::Error::other)
    }
}

/// Opens the thumbnail cache at `<data_dir>/cache`. When that folder is not safe or cannot be created the
/// gallery still works, with placeholders, and the second value says why.
pub fn open_thumbnails(
    data_dir: &Path,
    roots: &[LibraryRoot],
) -> (Arc<dyn ThumbnailProvider>, Option<String>) {
    let opened = ensure_export_dir(&data_dir.join("cache"), roots)
        .map_err(|e| e.to_string())
        .and_then(|dir| thumbs::Thumbs::new(&dir, THUMB_PX).map_err(|e| e.to_string()));
    match opened {
        Ok(thumbs) => (Arc::new(thumbs), None),
        Err(why) => (Arc::new(NoThumbnails), Some(why)),
    }
}
