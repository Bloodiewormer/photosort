// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::Catalog;
use fsguard::{LibraryRoot, ensure_writable};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, Frame, ImageEncoder, RgbaImage};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

type Snapshot = BTreeMap<
    PathBuf,
    (
        bool,
        u64,
        Option<SystemTime>,
        Option<SystemTime>,
        Option<String>,
    ),
>;

/// Everything observable about a tree: kind, size, modified and created times and, for files, the SHA-256.
fn snapshot(root: &Path) -> Snapshot {
    WalkDir::new(root)
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            let hash = md
                .is_file()
                .then(|| scanner::sha256_file(e.path()).unwrap());
            (
                e.path().to_path_buf(),
                (
                    md.is_dir(),
                    md.len(),
                    md.modified().ok(),
                    md.created().ok(),
                    hash,
                ),
            )
        })
        .collect()
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

/// Inserts a minimal valid TIFF/EXIF APP1 segment with `DateTimeOriginal` into a JPEG byte stream.
fn jpeg_with_exif_datetime(raw_jpeg: &[u8], datetime_ascii: &[u8; 20]) -> Vec<u8> {
    assert!(raw_jpeg.len() >= 2 && raw_jpeg[0] == 0xFF && raw_jpeg[1] == 0xD8);
    // Minimal little-endian TIFF with IFD0 (1 entry: ExifIFDPointer -> offset 26)
    // and ExifIFD (1 entry: DateTimeOriginal 0x9003 -> offset 44, 20 bytes ASCII).
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II"); // little-endian
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset = 8
    // IFD0 at offset 8 (2 + 12 + 4 = 18 bytes -> ends at 26)
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769u16.to_le_bytes()); // ExifIFDPointer
    tiff.extend_from_slice(&4u16.to_le_bytes()); // LONG
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&26u32.to_le_bytes()); // ExifIFD offset = 26
    tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
    // ExifIFD at offset 26 (2 + 12 + 4 = 18 bytes -> ends at 44)
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003u16.to_le_bytes()); // DateTimeOriginal
    tiff.extend_from_slice(&2u16.to_le_bytes()); // ASCII
    tiff.extend_from_slice(&20u32.to_le_bytes());
    tiff.extend_from_slice(&44u32.to_le_bytes()); // string offset = 44
    tiff.extend_from_slice(&0u32.to_le_bytes()); // next IFD = 0
    // ASCII value at offset 44
    tiff.extend_from_slice(datetime_ascii);

    let mut payload = Vec::new();
    payload.extend_from_slice(b"Exif\0\0");
    payload.extend_from_slice(&tiff);
    let seg_len = (payload.len() + 2) as u16;

    let mut out = Vec::with_capacity(raw_jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&raw_jpeg[..2]); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]); // APP1
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&raw_jpeg[2..]);
    out
}

#[test]
fn running_rules_leaves_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");

    let mut jpg = Vec::new();
    JpegEncoder::new(&mut jpg)
        .encode(&vec![64u8; 64 * 64 * 3], 64, 64, ColorType::Rgb8.into())
        .unwrap();
    let jpg_exif = jpeg_with_exif_datetime(&jpg, b"2023:07:14 10:22:33\0");
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&vec![128u8; 100 * 50 * 3], 100, 50, ColorType::Rgb8.into())
        .unwrap();
    let mut gif = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut gif);
        enc.encode_frame(Frame::new(RgbaImage::new(32, 96)))
            .unwrap();
    }

    write(&library, "IMG-20240101-WA0001.jpg", &jpg_exif);
    write(&library, "sub/Screenshot_1.png", &png);
    write(&library, "sub/deep/strip.gif", &gif);
    write(&library, "sub/corrupt.jpg", b"not a valid jpeg");

    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut catalog = Catalog::open(&db).unwrap();
    catalog.index(&root).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    rules::run(&catalog).unwrap();
    rules::run(&catalog).unwrap();

    assert_eq!(before, snapshot(&library));

    // Verify metadata (dimensions, format, EXIF capture date) was stored cleanly in read-only pass.
    let jpg_path = dunce::canonicalize(library.join("IMG-20240101-WA0001.jpg")).unwrap();
    let jpg_asset = catalog.asset_of(&jpg_path).unwrap().unwrap();
    let jpg_entry = catalog.entry_for(jpg_asset).unwrap().unwrap();
    assert_eq!(jpg_entry.width, Some(64));
    assert_eq!(jpg_entry.height, Some(64));
    assert_eq!(jpg_entry.format.as_deref(), Some("JPEG"));
    assert_eq!(
        jpg_entry.captured_at_ns,
        rules::parse_exif_datetime_ns("2023:07:14 10:22:33").map(|(ns, _)| ns)
    );
}
