// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thumbs::Thumbs;
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

#[test]
fn generating_thumbnails_leaves_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(library.join("sub")).unwrap();

    let mut rgb = RgbImage::new(200, 100);
    for (x, y, px) in rgb.enumerate_pixels_mut() {
        *px = Rgb([(x % 256) as u8, (y % 256) as u8, 64]);
    }
    let jpg = library.join("a.jpg");
    rgb.save_with_format(&jpg, ImageFormat::Jpeg).unwrap();

    // Also include a JPEG with an EXIF Orientation=6 tag so safety covers in-memory rotation.
    let oriented_jpg = library.join("oriented.jpg");
    {
        let mut raw = Vec::new();
        image::DynamicImage::ImageRgb8(rgb.clone())
            .write_to(&mut std::io::Cursor::new(&mut raw), ImageFormat::Jpeg)
            .unwrap();
        let exif: &[u8] = &[
            b'E', b'x', b'i', b'f', 0, 0, b'M', b'M', 0, 0x2A, 0, 0, 0, 8, 0, 1, 0x01, 0x12, 0, 3,
            0, 0, 0, 1, 0, 6, 0, 0, 0, 0, 0, 0,
        ];
        let len = (exif.len() + 2) as u16;
        let mut with_exif = Vec::with_capacity(raw.len() + exif.len() + 4);
        with_exif.extend_from_slice(&raw[..2]);
        with_exif.extend_from_slice(&[0xFF, 0xE1]);
        with_exif.extend_from_slice(&len.to_be_bytes());
        with_exif.extend_from_slice(exif);
        with_exif.extend_from_slice(&raw[2..]);
        fs::write(&oriented_jpg, &with_exif).unwrap();
    }

    let webp = library.join("sub/c.webp");
    rgb.save_with_format(&webp, ImageFormat::WebP).unwrap();

    let mut rgba = RgbaImage::new(120, 120);
    for (x, y, px) in rgba.enumerate_pixels_mut() {
        *px = Rgba([(x % 256) as u8, (y % 256) as u8, 200, 128]);
    }
    let png = library.join("sub/b.png");
    rgba.save_with_format(&png, ImageFormat::Png).unwrap();

    let corrupt = library.join("sub/corrupt.jpg");
    fs::write(&corrupt, b"corrupt image payload").unwrap();

    let unsupported = library.join("sub/raw.heic");
    fs::write(&unsupported, b"heic bytes").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    // Guard must refuse placing the thumbnail cache inside the library root.
    assert!(ensure_writable(&library.join("cache"), &[root]).is_err());

    let root = LibraryRoot::new(&library).unwrap();
    let cache = ensure_writable(&tmp.path().join("cache"), &[root]).unwrap();
    let thumbs = Thumbs::new(&cache, 100).unwrap();

    let valid_files = [
        (&jpg, scanner::sha256_file(&jpg).unwrap()),
        (&oriented_jpg, scanner::sha256_file(&oriented_jpg).unwrap()),
        (&png, scanner::sha256_file(&png).unwrap()),
        (&webp, scanner::sha256_file(&webp).unwrap()),
    ];
    let corrupt_sha = scanner::sha256_file(&corrupt).unwrap();
    let unsupported_sha = scanner::sha256_file(&unsupported).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    for (path, sha) in &valid_files {
        thumbs.get(path, sha).unwrap();
        thumbs.get(path, sha).unwrap();
    }
    assert!(thumbs.get(&corrupt, &corrupt_sha).is_err());
    assert!(thumbs.get(&unsupported, &unsupported_sha).is_err());

    assert_eq!(before, snapshot(&library));
}

#[test]
fn reading_video_frames_leaves_the_original_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();

    let mp4 = library.join("clip.mp4");
    let mp4_dest = ensure_writable(&mp4, &[]).unwrap();
    let create_res = thumbs::create_synthetic_video(&mp4_dest, 320, 240, 30, 5);
    #[cfg(windows)]
    create_res.expect("Media Foundation synthetic video creation must succeed on Windows");
    #[cfg(not(windows))]
    if create_res.is_err() {
        return;
    }

    let corrupt_mp4 = library.join("corrupt.mp4");
    fs::write(&corrupt_mp4, b"not a valid mp4 container").unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    // Guard must refuse creating a synthetic video inside a registered library root.
    assert!(ensure_writable(&library.join("forbidden.mp4"), std::slice::from_ref(&root)).is_err());

    let cache = ensure_writable(&tmp.path().join("cache"), &[root]).unwrap();
    let thumbs = Thumbs::new(&cache, 100).unwrap();

    let sha = scanner::sha256_file(&mp4).unwrap();
    let corrupt_sha = scanner::sha256_file(&corrupt_mp4).unwrap();

    // NTFS refreshes lazy directory attributes on first listing.
    snapshot(&library);
    let before = snapshot(&library);

    // 1. Extract 10% frame
    let frame_10 = thumbs::extract_frame(&mp4, 0.1, 0).expect("extract 10% frame");
    assert!(frame_10.width() > 0 && frame_10.height() > 0);

    // 2. Extract multiple frames
    let frames = thumbs::extract_frames(&mp4, 4, 0).expect("extract 4 frames");
    assert_eq!(frames.len(), 4);

    // 3. Thumbnails cache get
    let thumb_res1 = thumbs.get(&mp4, &sha).expect("thumb get 1");
    let thumb_res2 = thumbs.get(&mp4, &sha).expect("thumb get 2");
    assert_eq!(thumb_res1, thumb_res2);

    // 4. Corrupt video fails cleanly without touching the file
    assert!(thumbs::read_metadata(&corrupt_mp4).is_err());
    assert!(thumbs::extract_frame(&corrupt_mp4, 0.1, 100).is_err());
    assert!(thumbs::extract_frames(&corrupt_mp4, 4, 100).is_err());
    assert!(thumbs.get(&corrupt_mp4, &corrupt_sha).is_err());

    // 5. Assert library originals are 100% byte-for-byte untouched and no sidecars created
    assert_eq!(before, snapshot(&library));
}
