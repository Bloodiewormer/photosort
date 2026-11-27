use fsguard::DestPath;
pub mod video;
pub mod wic;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageError, ImageFormat,
    ImageReader, Limits,
};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
pub use video::{
    VideoMetadata, create_synthetic_video, extract_frame, extract_frames, is_video_extension,
    is_video_path, read_metadata,
};
pub use wic::{
    ImageFacts, WIC_EXTENSIONS, decode_wic, is_wic_extension, is_wic_path, read_image_metadata,
};

const JPEG_QUALITY: u8 = 85;
const MAX_DIMENSION: u32 = 20_000;
const MAX_ALLOC_BYTES: u64 = 512 * 1024 * 1024;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub struct Thumbs {
    cache_dir: DestPath,
    max_px: u32,
    commit_lock: Mutex<()>,
}

#[derive(Debug)]
pub enum ThumbError {
    Io(io::Error),
    Decode(String),
    Unsupported(String),
}

impl fmt::Display for ThumbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "i/o error: {e}"),
            Self::Decode(msg) => write!(f, "decode error: {msg}"),
            Self::Unsupported(msg) => write!(f, "unsupported image: {msg}"),
        }
    }
}

impl std::error::Error for ThumbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Decode(_) | Self::Unsupported(_) => None,
        }
    }
}

impl From<io::Error> for ThumbError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl Thumbs {
    /// Creates the cache folder if missing. `max_px` is the longest side of a thumbnail.
    pub fn new(cache_dir: &DestPath, max_px: u32) -> Result<Thumbs, ThumbError> {
        if max_px == 0 {
            return Err(ThumbError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "max_px must be greater than 0",
            )));
        }
        create_cache_dir(cache_dir)?;
        Ok(Thumbs {
            cache_dir: cache_dir.clone(),
            max_px,
            commit_lock: Mutex::new(()),
        })
    }

    /// Path of the cached thumbnail for this content, generating it if missing. Key = sha256 + max_px,
    /// so the same content in two paths shares one thumbnail.
    pub fn get(&self, source: &Path, sha256: &str) -> Result<PathBuf, ThumbError> {
        if !is_valid_sha256(sha256) {
            return Err(ThumbError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid sha256: expected 64 lowercase hex characters",
            )));
        }

        let jpg_target = self.join_cache_name(&format!("{sha256}_{}.jpg", self.max_px))?;
        let png_target = self.join_cache_name(&format!("{sha256}_{}.png", self.max_px))?;

        if jpg_target.as_path().is_file() {
            return Ok(jpg_target.as_path().to_path_buf());
        }
        if png_target.as_path().is_file() {
            return Ok(png_target.as_path().to_path_buf());
        }

        let resized = decode(source, self.max_px)?;
        let is_png = if is_video_path(source) {
            false
        } else {
            resized.color().has_alpha()
        };

        let target = if is_png { png_target } else { jpg_target };

        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_name = format!("{sha256}_{}.{}.{seq}.tmp", self.max_px, std::process::id());
        let tmp = self.join_cache_name(&tmp_name)?;

        encode_to_temp(&resized, &tmp, is_png)?;
        commit_temp_file(&tmp, &target, &self.commit_lock)?;

        Ok(target.as_path().to_path_buf())
    }

    fn join_cache_name(&self, name: &str) -> Result<DestPath, ThumbError> {
        self.cache_dir
            .join_name(OsStr::new(name))
            .map_err(|e| ThumbError::Io(io::Error::new(io::ErrorKind::InvalidInput, e)))
    }
}

fn is_valid_sha256(sha256: &str) -> bool {
    sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn format_for_extension(path: &Path) -> Result<ImageFormat, ThumbError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .ok_or_else(|| ThumbError::Unsupported("missing file extension".into()))?;
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" | "jfif" => Ok(ImageFormat::Jpeg),
        "png" => Ok(ImageFormat::Png),
        "webp" => Ok(ImageFormat::WebP),
        "gif" => Ok(ImageFormat::Gif),
        "bmp" => Ok(ImageFormat::Bmp),
        "tif" | "tiff" => Ok(ImageFormat::Tiff),
        _ => Err(ThumbError::Unsupported(format!(
            "unsupported extension: .{ext}"
        ))),
    }
}

fn map_image_error(err: ImageError) -> ThumbError {
    match err {
        ImageError::Unsupported(e) => ThumbError::Unsupported(e.to_string()),
        ImageError::IoError(e) if e.kind() != io::ErrorKind::UnexpectedEof => ThumbError::Io(e),
        other => ThumbError::Decode(other.to_string()),
    }
}

/// Decodes `source` read-only, upright per its EXIF orientation, with the thumbnail size limits.
/// Images larger than `max_px` on a side are scaled down in memory.
/// For video files, extracts a frame near 10% of the video duration.
pub fn decode(source: &Path, max_px: u32) -> Result<DynamicImage, ThumbError> {
    if is_video_path(source) {
        let frame = extract_frame(source, 0.1, max_px)?;
        Ok(DynamicImage::ImageRgb8(frame))
    } else if is_wic_path(source) {
        decode_wic(source, max_px)
    } else {
        decode_and_resize(source, format_for_extension(source)?, max_px)
    }
}

fn decode_and_resize(
    source: &Path,
    ext_format: ImageFormat,
    max_px: u32,
) -> Result<DynamicImage, ThumbError> {
    let file = File::open(source)?;
    let mut reader =
        ImageReader::with_format(BufReader::new(file), ext_format).with_guessed_format()?;

    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC_BYTES);
    reader.limits(limits);

    let mut decoder = reader.into_decoder().map_err(map_image_error)?;
    let (w, h) = decoder.dimensions();
    if w > MAX_DIMENSION || h > MAX_DIMENSION {
        return Err(ThumbError::Decode(format!(
            "image dimensions {w}x{h} exceed limit {MAX_DIMENSION}"
        )));
    }
    let estimated_bytes = u64::from(w)
        .saturating_mul(u64::from(h))
        .saturating_mul(u64::from(decoder.color_type().bytes_per_pixel()));
    if estimated_bytes > MAX_ALLOC_BYTES {
        return Err(ThumbError::Decode(format!(
            "image buffer {estimated_bytes} bytes exceeds limit {MAX_ALLOC_BYTES}"
        )));
    }

    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut img = DynamicImage::from_decoder(decoder).map_err(map_image_error)?;
    img.apply_orientation(orientation);

    if img.width() > max_px || img.height() > max_px {
        Ok(img.resize(max_px, max_px, FilterType::Lanczos3))
    } else {
        Ok(img)
    }
}

fn encode_to_temp(img: &DynamicImage, tmp: &DestPath, is_png: bool) -> Result<(), ThumbError> {
    let mut created = false;
    let res = (|| -> Result<(), ThumbError> {
        let file = create_temp_file(tmp)?;
        created = true;
        let mut writer = BufWriter::new(file);
        if is_png {
            let rgba = img.to_rgba8();
            PngEncoder::new(&mut writer)
                .write_image(&rgba, rgba.width(), rgba.height(), ExtendedColorType::Rgba8)
                .map_err(map_image_error)?;
        } else {
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut writer, JPEG_QUALITY)
                .write_image(&rgb, rgb.width(), rgb.height(), ExtendedColorType::Rgb8)
                .map_err(map_image_error)?;
        }
        writer.flush()?;
        Ok(())
    })();
    if res.is_err() && created {
        remove_temp_file(tmp);
    }
    res
}

#[allow(clippy::disallowed_methods)] // `dir` is a DestPath outside every library root
fn create_cache_dir(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

#[allow(clippy::disallowed_methods)] // `tmp` is a DestPath inside the cache dir
fn create_temp_file(tmp: &DestPath) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp.as_path())
}

#[allow(clippy::disallowed_methods)] // removes only our own temporary file in the cache dir
fn remove_temp_file(tmp: &DestPath) {
    let _ = fs::remove_file(tmp.as_path());
}

// shortcut: checking that `target` is absent and renaming are two steps across separate processes,
// so two processes racing on the same key could replace the file; within one process `lock`
// serializes commits, upgrade to a link-based publish if multi-process cache sharing is needed.
#[allow(clippy::disallowed_methods)] // both `tmp` and `target` are DestPaths in the cache dir
fn commit_temp_file(tmp: &DestPath, target: &DestPath, lock: &Mutex<()>) -> io::Result<()> {
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if fs::symlink_metadata(target.as_path()).is_ok() {
        remove_temp_file(tmp);
        return Ok(());
    }
    match fs::rename(tmp.as_path(), target.as_path()) {
        Ok(()) => Ok(()),
        Err(e) => {
            remove_temp_file(tmp);
            if fs::symlink_metadata(target.as_path()).is_ok() {
                Ok(())
            } else {
                Err(e)
            }
        }
    }
}
