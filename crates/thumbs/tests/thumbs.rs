// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use fsguard::{LibraryRoot, ensure_writable};
use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use thumbs::{ThumbError, Thumbs};

fn write_rgb_image(path: &Path, w: u32, h: u32, format: ImageFormat) -> String {
    let mut img = RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
    }
    img.save_with_format(path, format).unwrap();
    scanner::sha256_file(path).unwrap()
}

fn write_rgba_png(path: &Path, w: u32, h: u32) -> String {
    let mut img = RgbaImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        // Left half fully transparent, right half opaque green.
        let alpha = if x < w / 2 { 0 } else { 255 };
        *px = Rgba([0, 200, 40, alpha]);
        let _ = y;
    }
    img.save_with_format(path, ImageFormat::Png).unwrap();
    scanner::sha256_file(path).unwrap()
}

/// Builds a real JPEG where the top half is red and the bottom half is blue, then splices a valid
/// EXIF APP1 segment with Orientation = 6 (90° clockwise) right after the SOI marker.
fn write_oriented_jpeg(path: &Path, w: u32, h: u32) -> String {
    let mut img = RgbImage::new(w, h);
    for (_x, y, px) in img.enumerate_pixels_mut() {
        *px = if y < h / 2 {
            Rgb([220, 20, 20])
        } else {
            Rgb([20, 20, 220])
        };
    }
    let mut raw = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut Cursor::new(&mut raw), ImageFormat::Jpeg)
        .unwrap();
    assert_eq!(&raw[..2], &[0xFF, 0xD8]);

    // Minimal big-endian TIFF/EXIF payload with 1 IFD0 entry: tag 0x0112 (Orientation), SHORT, count 1, value 6.
    let exif_payload: &[u8] = &[
        b'E', b'x', b'i', b'f', 0x00, 0x00, // Exif\0\0 header
        b'M', b'M', 0x00, 0x2A, // Big-endian TIFF magic (42)
        0x00, 0x00, 0x00, 0x08, // Offset to IFD0 = 8
        0x00, 0x01, // 1 directory entry
        0x01, 0x12, // Tag: Orientation (0x0112)
        0x00, 0x03, // Type: SHORT (3)
        0x00, 0x00, 0x00, 0x01, // Count: 1
        0x00, 0x06, 0x00, 0x00, // Value: 6 (Rotate 90 CW)
        0x00, 0x00, 0x00, 0x00, // Next IFD offset = 0
    ];
    let app1_len = (exif_payload.len() + 2) as u16;
    let mut with_exif = Vec::with_capacity(raw.len() + exif_payload.len() + 4);
    with_exif.extend_from_slice(&raw[..2]);
    with_exif.extend_from_slice(&[0xFF, 0xE1]);
    with_exif.extend_from_slice(&app1_len.to_be_bytes());
    with_exif.extend_from_slice(exif_payload);
    with_exif.extend_from_slice(&raw[2..]);

    fs::write(path, &with_exif).unwrap();
    scanner::sha256_file(path).unwrap()
}

/// Minimal syntactically valid PNG header (signature + IHDR + IEND) with arbitrary dimensions,
/// used to test dimension/allocation limits without allocating gigabytes in the test.
fn write_png_header_with_dims(path: &Path, width: u32, height: u32) -> String {
    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in bytes {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn push_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        let mut crc_input = Vec::with_capacity(4 + data.len());
        crc_input.extend_from_slice(chunk_type);
        crc_input.extend_from_slice(data);
        out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    }

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IEND", &[]);
    fs::write(path, &png).unwrap();
    scanner::sha256_file(path).unwrap()
}

fn cache_entries(dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect()
}

fn assert_no_tmp_files(dir: &Path) {
    for entry in cache_entries(dir) {
        let name = entry.file_name().unwrap().to_string_lossy();
        assert!(
            !name.contains(".tmp"),
            "unexpected leftover temp file: {name}"
        );
    }
}

fn setup_dirs() -> (tempfile::TempDir, PathBuf, fsguard::DestPath) {
    let tmp = tempfile::tempdir().unwrap();
    let src_dir = tmp.path().join("library");
    fs::create_dir_all(&src_dir).unwrap();
    let root = LibraryRoot::new(&src_dir).unwrap();
    let cache_dir = ensure_writable(&tmp.path().join("cache").join("thumbs"), &[root]).unwrap();
    (tmp, src_dir, cache_dir)
}

#[test]
fn resizes_landscape_and_portrait_keeping_aspect_ratio() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();
    assert!(cache_dir.as_path().is_dir());

    let land_src = src_dir.join("landscape.jpg");
    let land_sha = write_rgb_image(&land_src, 400, 200, ImageFormat::Jpeg);
    let land_thumb = thumbs.get(&land_src, &land_sha).unwrap();
    assert_eq!(
        land_thumb.file_name().unwrap().to_string_lossy(),
        format!("{land_sha}_100.jpg")
    );
    let decoded = image::open(&land_thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (100, 50));

    let port_src = src_dir.join("portrait.png");
    let port_sha = write_rgb_image(&port_src, 200, 400, ImageFormat::Png);
    let port_thumb = thumbs.get(&port_src, &port_sha).unwrap();
    assert_eq!(
        port_thumb.file_name().unwrap().to_string_lossy(),
        format!("{port_sha}_100.jpg")
    );
    let decoded = image::open(&port_thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (50, 100));
    assert_no_tmp_files(cache_dir.as_path());
}

#[test]
fn does_not_upscale_small_image() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 200).unwrap();

    let small_src = src_dir.join("small.jpg");
    let sha = write_rgb_image(&small_src, 60, 40, ImageFormat::Jpeg);
    let thumb = thumbs.get(&small_src, &sha).unwrap();

    let decoded = image::open(&thumb).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (60, 40));
}

#[test]
fn applies_exif_orientation_in_memory() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    let src = src_dir.join("rotated.jpg");
    let sha = write_oriented_jpeg(&src, 200, 100);
    let thumb = thumbs.get(&src, &sha).unwrap();

    let decoded = image::open(&thumb).unwrap().to_rgb8();
    // 200x100 rotated 90° CW becomes 100x200, then resized to max_px=100 -> 50x100.
    assert_eq!((decoded.width(), decoded.height()), (50, 100));

    // Original top half (red) moved to the right half; bottom half (blue) moved to the left half.
    let left_px = decoded.get_pixel(10, 50);
    let right_px = decoded.get_pixel(40, 50);
    assert!(
        left_px[2] > 150 && left_px[0] < 80,
        "expected blue on left, got {left_px:?}"
    );
    assert!(
        right_px[0] > 150 && right_px[2] < 80,
        "expected red on right, got {right_px:?}"
    );
}

#[test]
fn alpha_png_stays_png_with_alpha_preserved() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 80).unwrap();

    let src = src_dir.join("transparent.png");
    let sha = write_rgba_png(&src, 160, 80);
    let thumb = thumbs.get(&src, &sha).unwrap();

    assert_eq!(
        thumb.file_name().unwrap().to_string_lossy(),
        format!("{sha}_80.png")
    );
    let decoded = image::open(&thumb).unwrap();
    assert!(decoded.color().has_alpha());
    assert_eq!((decoded.width(), decoded.height()), (80, 40));

    let rgba = decoded.to_rgba8();
    assert_eq!(rgba.get_pixel(5, 20)[3], 0);
    assert_eq!(rgba.get_pixel(70, 20)[3], 255);
}

#[test]
fn second_call_returns_cached_file_without_regenerating() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 128).unwrap();

    let src = src_dir.join("photo.jpg");
    let sha = write_rgb_image(&src, 300, 150, ImageFormat::Jpeg);

    let first = thumbs.get(&src, &sha).unwrap();
    let first_mtime = fs::metadata(&first).unwrap().modified().unwrap();

    thread::sleep(Duration::from_millis(50));
    let second = thumbs.get(&src, &sha).unwrap();
    let second_mtime = fs::metadata(&second).unwrap().modified().unwrap();

    assert_eq!(first, second);
    assert_eq!(first_mtime, second_mtime);

    // Even if the source path doesn't exist, a cache hit returns without trying to open/decode.
    let missing = src_dir.join("does_not_exist.jpg");
    let third = thumbs.get(&missing, &sha).unwrap();
    assert_eq!(first, third);
}

#[test]
fn same_content_in_two_paths_shares_one_thumbnail() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    let path_a = src_dir.join("a.jpg");
    let path_b = src_dir.join("copy_of_a.jpg");
    let sha = write_rgb_image(&path_a, 240, 120, ImageFormat::Jpeg);
    fs::write(&path_b, fs::read(&path_a).unwrap()).unwrap();

    let thumb_a = thumbs.get(&path_a, &sha).unwrap();
    let thumb_b = thumbs.get(&path_b, &sha).unwrap();

    assert_eq!(thumb_a, thumb_b);
    assert_eq!(cache_entries(cache_dir.as_path()).len(), 1);
}

#[test]
fn different_max_px_creates_distinct_cached_files() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let small = Thumbs::new(&cache_dir, 64).unwrap();
    let large = Thumbs::new(&cache_dir, 128).unwrap();

    let src = src_dir.join("photo.jpg");
    let sha = write_rgb_image(&src, 256, 128, ImageFormat::Jpeg);

    let p64 = small.get(&src, &sha).unwrap();
    let p128 = large.get(&src, &sha).unwrap();
    assert_ne!(p64, p128);
    assert_eq!(image::image_dimensions(&p64).unwrap(), (64, 32));
    assert_eq!(image::image_dimensions(&p128).unwrap(), (128, 64));
}

#[test]
fn corrupt_and_truncated_files_return_err_without_tmp_leftovers() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    let corrupt = src_dir.join("corrupt.jpg");
    fs::write(&corrupt, b"not a jpeg at all").unwrap();
    let corrupt_sha = scanner::sha256_file(&corrupt).unwrap();
    assert!(matches!(
        thumbs.get(&corrupt, &corrupt_sha),
        Err(ThumbError::Decode(_))
    ));

    let full_jpg = src_dir.join("full.jpg");
    write_rgb_image(&full_jpg, 120, 120, ImageFormat::Jpeg);
    let truncated_jpg = src_dir.join("truncated.jpg");
    let bytes = fs::read(&full_jpg).unwrap();
    fs::write(&truncated_jpg, &bytes[..24.min(bytes.len())]).unwrap();
    let trunc_sha = scanner::sha256_file(&truncated_jpg).unwrap();
    assert!(thumbs.get(&truncated_jpg, &trunc_sha).is_err());

    let full_png = src_dir.join("full.png");
    write_rgba_png(&full_png, 120, 120);
    let truncated_png = src_dir.join("truncated.png");
    let png_bytes = fs::read(&full_png).unwrap();
    fs::write(&truncated_png, &png_bytes[..24.min(png_bytes.len())]).unwrap();
    let trunc_png_sha = scanner::sha256_file(&truncated_png).unwrap();
    assert!(thumbs.get(&truncated_png, &trunc_png_sha).is_err());

    assert_no_tmp_files(cache_dir.as_path());
    assert!(cache_entries(cache_dir.as_path()).is_empty());
}

#[test]
fn unsupported_extensions_return_unsupported() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();
    let valid_sha = "a".repeat(64);

    for name in ["photo.raw", "notes.txt", "no_extension"] {
        let p = src_dir.join(name);
        fs::write(&p, b"dummy").unwrap();
        let err = thumbs.get(&p, &valid_sha).unwrap_err();
        assert!(
            matches!(err, ThumbError::Unsupported(_)),
            "expected Unsupported for {name}, got {err:?}"
        );
    }
}

#[test]
fn bad_sha256_is_rejected() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();
    let src = src_dir.join("valid.jpg");
    write_rgb_image(&src, 80, 80, ImageFormat::Jpeg);

    let uppercase = "A".repeat(64);
    let short = "a".repeat(63);
    let long = "a".repeat(65);
    let non_hex = format!("{}g", "0".repeat(63));
    let traversal = format!("../{}", "a".repeat(61));
    let with_slash = format!("aa/{}", "a".repeat(61));

    for bad in [
        "",
        &uppercase,
        &short,
        &long,
        &non_hex,
        &traversal,
        &with_slash,
    ] {
        assert!(
            thumbs.get(&src, bad).is_err(),
            "expected error for bad sha256: {bad:?}"
        );
    }
    assert!(cache_entries(cache_dir.as_path()).is_empty());
}

#[test]
fn oversized_dimensions_return_err_without_exhausting_memory() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    // Exceeds MAX_DIMENSION (30000x30000).
    let huge = src_dir.join("huge.png");
    let sha = write_png_header_with_dims(&huge, 30_000, 30_000);
    let err = thumbs.get(&huge, &sha).unwrap_err();
    assert!(
        matches!(err, ThumbError::Decode(_)),
        "expected Decode error for 30000x30000 PNG, got {err:?}"
    );

    // Within MAX_DIMENSION but exceeds 512 MiB buffer limit (15000*15000*4 ≈ 858 MiB).
    let wide_alloc = src_dir.join("alloc_bomb.png");
    let alloc_sha = write_png_header_with_dims(&wide_alloc, 15_000, 15_000);
    let err = thumbs.get(&wide_alloc, &alloc_sha).unwrap_err();
    assert!(
        matches!(err, ThumbError::Decode(_)),
        "expected Decode error for 15000x15000 RGBA PNG, got {err:?}"
    );
    assert_no_tmp_files(cache_dir.as_path());
}

#[test]
fn zero_max_px_is_rejected() {
    let (_tmp, _src_dir, cache_dir) = setup_dirs();
    assert!(Thumbs::new(&cache_dir, 0).is_err());
}

#[test]
fn supports_webp_gif_bmp_and_tiff() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 64).unwrap();

    for (name, fmt) in [
        ("sample.webp", ImageFormat::WebP),
        ("sample.gif", ImageFormat::Gif),
        ("sample.bmp", ImageFormat::Bmp),
        ("sample.tiff", ImageFormat::Tiff),
    ] {
        let p = src_dir.join(name);
        let sha = write_rgb_image(&p, 128, 64, fmt);
        let thumb = thumbs.get(&p, &sha).unwrap();
        let dims = image::image_dimensions(&thumb).unwrap();
        assert_eq!(dims, (64, 32), "unexpected dims for {name}");
    }
}

#[test]
fn concurrent_calls_for_same_key_succeed_with_one_valid_file() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Thumbs>();

    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Arc::new(Thumbs::new(&cache_dir, 96).unwrap());

    let src = src_dir.join("shared.jpg");
    let sha = write_rgb_image(&src, 320, 160, ImageFormat::Jpeg);

    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);

    for _ in 0..threads {
        let thumbs = Arc::clone(&thumbs);
        let src = src.clone();
        let sha = sha.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            thumbs.get(&src, &sha).unwrap()
        }));
    }

    let paths: Vec<PathBuf> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for p in &paths {
        assert_eq!(p, &paths[0]);
    }
    assert_eq!(image::image_dimensions(&paths[0]).unwrap(), (96, 48));

    let entries = cache_entries(cache_dir.as_path());
    assert_eq!(entries.len(), 1, "expected only 1 file, found {entries:?}");
    assert_no_tmp_files(cache_dir.as_path());
}

#[test]
fn wic_decodes_synthetic_png_and_bmp() {
    let (_tmp, src_dir, _cache_dir) = setup_dirs();

    // 1. Synthetic PNG
    let png_path = src_dir.join("synthetic.png");
    write_rgb_image(&png_path, 120, 80, ImageFormat::Png);
    let decoded_png = thumbs::decode_wic(&png_path, 200).expect("WIC decode PNG full");
    assert_eq!(decoded_png.width(), 120);
    assert_eq!(decoded_png.height(), 80);

    let resized_png = thumbs::decode_wic(&png_path, 60).expect("WIC decode PNG resized");
    assert_eq!(resized_png.width(), 60);
    assert_eq!(resized_png.height(), 40);

    // 2. Synthetic BMP
    let bmp_path = src_dir.join("synthetic.bmp");
    write_rgb_image(&bmp_path, 100, 50, ImageFormat::Bmp);
    let decoded_bmp = thumbs::decode_wic(&bmp_path, 200).expect("WIC decode BMP full");
    assert_eq!(decoded_bmp.width(), 100);
    assert_eq!(decoded_bmp.height(), 50);

    let resized_bmp = thumbs::decode_wic(&bmp_path, 50).expect("WIC decode BMP resized");
    assert_eq!(resized_bmp.width(), 50);
    assert_eq!(resized_bmp.height(), 25);
}

#[test]
fn heic_corrupt_returns_decode_error_not_missing_codec() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    for name in ["corrupt_test.heic", "corrupt_test.avif", "corrupt_test.jxl"] {
        let p = src_dir.join(name);
        fs::write(&p, b"not a real container header").unwrap();
        let sha = "e".repeat(64);

        let dec_err = thumbs::decode(&p, 100).unwrap_err();
        assert!(
            matches!(dec_err, ThumbError::Decode(_)),
            "expected Decode error for corrupt {name}, got {dec_err:?}"
        );

        let wic_err = thumbs::decode_wic(&p, 100).unwrap_err();
        assert!(
            matches!(wic_err, ThumbError::Decode(_)),
            "expected Decode error for corrupt {name}, got {wic_err:?}"
        );

        let get_err = thumbs.get(&p, &sha).unwrap_err();
        assert!(
            matches!(get_err, ThumbError::Decode(_)),
            "expected Decode error for corrupt {name}, got {get_err:?}"
        );
    }

    // Valid JXL container header: if WIC lacks a JXL codec it must report missing image codec
    // (Unsupported); if a JXL codec is installed it rejects the truncated payload as Decode.
    let jxl_valid_hdr = src_dir.join("valid_header.jxl");
    fs::write(
        &jxl_valid_hdr,
        [
            0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
        ],
    )
    .unwrap();
    match thumbs::decode(&jxl_valid_hdr, 100).unwrap_err() {
        ThumbError::Unsupported(msg) => {
            assert!(
                msg.contains("códec de imagen no disponible"),
                "unexpected Unsupported message: {msg}"
            );
        }
        ThumbError::Decode(_) => {}
        other => panic!("expected Unsupported or Decode for JXL header, got {other:?}"),
    }
}

#[test]
fn wic_preserves_channel_order_alpha_and_exif_orientation() {
    let (_tmp, src_dir, _cache_dir) = setup_dirs();

    // 1. Opaque RGB PNG: channels must stay RGB (not BGR) and drop redundant alpha channel.
    let rgb_path = src_dir.join("channels.png");
    let mut rgb = RgbImage::new(16, 16);
    for px in rgb.pixels_mut() {
        *px = Rgb([220, 40, 10]);
    }
    rgb.save_with_format(&rgb_path, ImageFormat::Png).unwrap();

    let decoded = thumbs::decode_wic(&rgb_path, 100).expect("WIC decode opaque PNG");
    assert!(
        !decoded.color().has_alpha(),
        "opaque image decoded by WIC should not report alpha"
    );
    let px = decoded.to_rgb8().get_pixel(8, 8).0;
    assert_eq!(px, [220, 40, 10], "R/G/B channel order must not be swapped");

    // 2. Transparent RGBA PNG: alpha must be preserved.
    let rgba_path = src_dir.join("alpha.png");
    write_rgba_png(&rgba_path, 32, 16);
    let decoded_rgba = thumbs::decode_wic(&rgba_path, 100).expect("WIC decode RGBA PNG");
    assert!(decoded_rgba.color().has_alpha());
    let rgba = decoded_rgba.to_rgba8();
    assert_eq!(rgba.get_pixel(2, 8)[3], 0);
    assert_eq!(rgba.get_pixel(28, 8).0, [0, 200, 40, 255]);

    // 3. Oriented JPEG (Orientation=6, 90° CW): dimensions and pixel placement must rotate.
    let oriented_path = src_dir.join("oriented_wic.jpg");
    write_oriented_jpeg(&oriented_path, 200, 100);
    let oriented = thumbs::decode_wic(&oriented_path, 100)
        .expect("WIC decode oriented JPEG")
        .to_rgb8();
    assert_eq!((oriented.width(), oriented.height()), (50, 100));
    let left_px = oriented.get_pixel(10, 50);
    let right_px = oriented.get_pixel(40, 50);
    assert!(
        left_px[2] > 150 && left_px[0] < 80,
        "expected blue on left after WIC EXIF rotation, got {left_px:?}"
    );
    assert!(
        right_px[0] > 150 && right_px[2] < 80,
        "expected red on right after WIC EXIF rotation, got {right_px:?}"
    );
}

#[test]
fn wic_decodes_concurrently_across_threads() {
    let (_tmp, src_dir, _cache_dir) = setup_dirs();
    let png_path = src_dir.join("concurrent.png");
    write_rgb_image(&png_path, 80, 40, ImageFormat::Png);

    let threads = 8;
    let barrier = Arc::new(Barrier::new(threads));
    let mut handles = Vec::with_capacity(threads);
    for _ in 0..threads {
        let p = png_path.clone();
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            let img = thumbs::decode_wic(&p, 40).expect("concurrent decode_wic");
            assert_eq!((img.width(), img.height()), (40, 20));
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn jfif_is_indexed_and_decoded() {
    let (_tmp, src_dir, cache_dir) = setup_dirs();
    let thumbs = Thumbs::new(&cache_dir, 100).unwrap();

    let jfif_path = src_dir.join("photo.jfif");
    let sha = write_rgb_image(&jfif_path, 200, 100, ImageFormat::Jpeg);

    // Scanner recognizes jfif as Image
    let scan_report = scanner::scan(&src_dir).unwrap();
    let found = scan_report
        .files
        .iter()
        .find(|f| f.path.ends_with("photo.jfif"))
        .expect("photo.jfif must be found by scanner");
    assert_eq!(found.media_type, scanner::MediaType::Image);

    // decode works
    let img = thumbs::decode(&jfif_path, 100).expect("decode jfif");
    assert_eq!(img.width(), 100);
    assert_eq!(img.height(), 50);

    // get thumbnail works
    let thumb_path = thumbs.get(&jfif_path, &sha).expect("get jfif thumb");
    assert!(thumb_path.is_file());
}
