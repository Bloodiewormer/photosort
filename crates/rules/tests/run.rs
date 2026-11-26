// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Assignment, Catalog, LabelKind, Source, Status};
use fsguard::{LibraryRoot, ensure_writable};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::{ColorType, Frame, ImageEncoder, RgbaImage};
use rules::{Control, RunProgress, RunReport, STAGE, VERSION, read_facts, run, run_with};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const TAG_MAKE: u16 = 0x010F;
const TAG_MODEL: u16 = 0x0110;

fn tiff_single_ascii_tag(tag: u16, value: &str) -> Vec<u8> {
    let mut val = value.as_bytes().to_vec();
    val.push(0);
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM");
    tiff.extend_from_slice(&42u16.to_be_bytes());
    tiff.extend_from_slice(&8u32.to_be_bytes()); // IFD0 offset
    tiff.extend_from_slice(&1u16.to_be_bytes()); // 1 entry
    tiff.extend_from_slice(&tag.to_be_bytes());
    tiff.extend_from_slice(&2u16.to_be_bytes()); // ASCII
    tiff.extend_from_slice(&(val.len() as u32).to_be_bytes());
    if val.len() <= 4 {
        let mut inline = [0u8; 4];
        inline[..val.len()].copy_from_slice(&val);
        tiff.extend_from_slice(&inline);
        tiff.extend_from_slice(&0u32.to_be_bytes()); // next IFD
    } else {
        tiff.extend_from_slice(&26u32.to_be_bytes()); // offset past IFD0
        tiff.extend_from_slice(&0u32.to_be_bytes()); // next IFD
        tiff.extend_from_slice(&val);
    }
    tiff
}

fn jpeg_bytes(width: u32, height: u32, exif_tag: Option<(u16, &str)>) -> Vec<u8> {
    let pixels = vec![128u8; (width * height * 3) as usize];
    let mut raw = Vec::new();
    JpegEncoder::new(&mut raw)
        .encode(&pixels, width, height, ColorType::Rgb8.into())
        .unwrap();
    assert_eq!(&raw[..2], &[0xFF, 0xD8]);
    let Some((tag, value)) = exif_tag else {
        return raw;
    };
    let tiff = tiff_single_ascii_tag(tag, value);
    let mut app1 = Vec::new();
    app1.extend_from_slice(b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let len = u16::try_from(app1.len() + 2).unwrap();
    let mut out = Vec::with_capacity(raw.len() + app1.len() + 4);
    out.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE1]);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&raw[2..]);
    out
}

fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

fn png_bytes(width: u32, height: u32, exif_tag: Option<(u16, &str)>) -> Vec<u8> {
    let pixels = vec![128u8; (width * height * 3) as usize];
    let mut raw = Vec::new();
    PngEncoder::new(&mut raw)
        .write_image(&pixels, width, height, ColorType::Rgb8.into())
        .unwrap();
    let Some((tag, value)) = exif_tag else {
        return raw;
    };
    let tiff = tiff_single_ascii_tag(tag, value);
    let iend_start = raw.len() - 12;
    assert_eq!(&raw[iend_start + 4..iend_start + 8], b"IEND");
    let mut chunk_body = Vec::with_capacity(4 + tiff.len());
    chunk_body.extend_from_slice(b"eXIf");
    chunk_body.extend_from_slice(&tiff);
    let crc = crc32_ieee(&chunk_body);
    let mut out = Vec::with_capacity(raw.len() + 12 + tiff.len());
    out.extend_from_slice(&raw[..iend_start]);
    out.extend_from_slice(&(tiff.len() as u32).to_be_bytes());
    out.extend_from_slice(&chunk_body);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&raw[iend_start..]);
    out
}

fn gif_bytes(width: u32, height: u32) -> Vec<u8> {
    let img = RgbaImage::new(width, height);
    let mut out = Vec::new();
    {
        let mut enc = GifEncoder::new(&mut out);
        enc.encode_frame(Frame::new(img)).unwrap();
    }
    out
}

fn write_file(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    cat: Catalog,
}

impl Setup {
    fn new(files: &[(&str, Vec<u8>)]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let library = tmp.path().join("library");
        fs::create_dir_all(&library).unwrap();
        for (rel, bytes) in files {
            write_file(&library, rel, bytes);
        }
        let root = LibraryRoot::new(&library).unwrap();
        fs::create_dir_all(tmp.path().join("data")).unwrap();
        let db = ensure_writable(
            &tmp.path().join("data").join("catalog.sqlite"),
            std::slice::from_ref(&root),
        )
        .unwrap();
        let mut cat = Catalog::open(&db).unwrap();
        cat.index(&root).unwrap();
        Self {
            _tmp: tmp,
            library,
            root,
            cat,
        }
    }

    fn labels(&self, rel: &str) -> Vec<Assignment> {
        let path = dunce::canonicalize(self.library.join(rel)).unwrap();
        let asset = self.cat.asset_of(&path).unwrap().expect("indexed");
        self.cat.labels_of(asset).unwrap()
    }
}

#[test]
fn read_facts_reads_jpeg_dimensions_and_camera_exif() {
    let tmp = tempfile::tempdir().unwrap();
    let plain = write_file(tmp.path(), "plain.JPG", &jpeg_bytes(320, 240, None));
    let with_make = write_file(
        tmp.path(),
        "canon.jpg",
        &jpeg_bytes(400, 300, Some((TAG_MAKE, "Canon"))),
    );
    let with_model = write_file(
        tmp.path(),
        "sony.jpg",
        &jpeg_bytes(200, 150, Some((TAG_MODEL, "ILCE-7M4"))),
    );
    let empty_make = write_file(
        tmp.path(),
        "blank.jpg",
        &jpeg_bytes(160, 120, Some((TAG_MAKE, "   "))),
    );

    let f = read_facts(&plain).unwrap();
    assert_eq!(f.file_name, "plain.JPG");
    assert_eq!(f.extension, "jpg");
    assert_eq!((f.width, f.height), (320, 240));
    assert!(!f.has_camera_exif);

    let f = read_facts(&with_make).unwrap();
    assert_eq!((f.width, f.height), (400, 300));
    assert!(f.has_camera_exif);

    let f = read_facts(&with_model).unwrap();
    assert_eq!((f.width, f.height), (200, 150));
    assert!(f.has_camera_exif);

    let f = read_facts(&empty_make).unwrap();
    assert!(!f.has_camera_exif);
}

#[test]
fn read_facts_reads_png_with_and_without_exif_and_gif() {
    let tmp = tempfile::tempdir().unwrap();
    let plain_png = write_file(tmp.path(), "shot.png", &png_bytes(1920, 1080, None));
    let exif_png = write_file(
        tmp.path(),
        "cam.png",
        &png_bytes(640, 480, Some((TAG_MAKE, "Nikon"))),
    );
    let anim_gif = write_file(tmp.path(), "loop.GIF", &gif_bytes(64, 160));
    let bad = write_file(tmp.path(), "broken.jpg", b"not an image");

    let f = read_facts(&plain_png).unwrap();
    assert_eq!((f.width, f.height, f.has_camera_exif), (1920, 1080, false));

    let f = read_facts(&exif_png).unwrap();
    assert_eq!((f.width, f.height, f.has_camera_exif), (640, 480, true));

    let f = read_facts(&anim_gif).unwrap();
    assert_eq!(f.extension, "gif");
    assert_eq!((f.width, f.height, f.has_camera_exif), (64, 160, false));

    assert!(read_facts(&bad).is_err());
}

#[test]
fn run_populates_rules_suggestions_and_second_run_analyzes_nothing() {
    let s = Setup::new(&[
        ("shots/Screenshot_01.png", png_bytes(1920, 1080, None)),
        (
            "camera/DSC001.jpg",
            jpeg_bytes(400, 300, Some((TAG_MAKE, "Fujifilm"))),
        ),
        ("chat/IMG-20240501-WA0003.jpg", jpeg_bytes(400, 400, None)),
        ("strips/chapter.gif", gif_bytes(40, 120)),
        ("misc/plain.png", png_bytes(200, 200, None)),
    ]);

    let first: RunReport = run(&s.cat).unwrap();
    assert_eq!(first.analyzed, 5);
    assert_eq!(first.suggestions, 5);
    assert!(first.errors.is_empty(), "{:?}", first.errors);

    let shot = s.labels("shots/Screenshot_01.png");
    assert_eq!(shot.len(), 1);
    assert_eq!(
        (
            shot[0].kind,
            shot[0].name.as_str(),
            shot[0].source,
            shot[0].confidence,
            shot[0].status
        ),
        (
            LabelKind::Category,
            "Capturas de pantalla",
            Source::Rules,
            0.90,
            Status::Suggested
        )
    );

    let cam = s.labels("camera/DSC001.jpg");
    assert_eq!(cam.len(), 1);
    assert_eq!(
        (
            cam[0].kind,
            cam[0].name.as_str(),
            cam[0].source,
            cam[0].status
        ),
        (
            LabelKind::Tag,
            "foto de cámara",
            Source::Rules,
            Status::Suggested
        )
    );

    let wa = s.labels("chat/IMG-20240501-WA0003.jpg");
    assert_eq!(wa.len(), 1);
    assert_eq!(
        (
            wa[0].kind,
            wa[0].name.as_str(),
            wa[0].source,
            wa[0].confidence
        ),
        (LabelKind::Tag, "WhatsApp", Source::Rules, 0.95)
    );

    let strip = s.labels("strips/chapter.gif");
    assert_eq!(strip.len(), 2);
    assert!(
        strip
            .iter()
            .any(|a| a.name == "GIF" && a.confidence == 0.95)
    );
    assert!(
        strip
            .iter()
            .any(|a| a.name == "formato vertical largo" && a.confidence == 0.50)
    );

    assert!(s.labels("misc/plain.png").is_empty());
    assert!(s.cat.pending(STAGE, VERSION, 100).unwrap().is_empty());

    let second = run(&s.cat).unwrap();
    assert_eq!(second, RunReport::default());
}

#[test]
fn run_never_resurrects_a_user_rejected_label() {
    let s = Setup::new(&[("shots/captura_01.png", png_bytes(800, 600, None))]);
    let path = dunce::canonicalize(s.library.join("shots/captura_01.png")).unwrap();
    let asset = s.cat.asset_of(&path).unwrap().unwrap();
    let label = s
        .cat
        .label(LabelKind::Category, "Capturas de pantalla")
        .unwrap();
    s.cat.reject(asset, label).unwrap();

    let report = run(&s.cat).unwrap();
    assert_eq!(report.analyzed, 1);

    let labels = s.labels("shots/captura_01.png");
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].status, Status::Rejected);
    assert_eq!(labels[0].source, Source::User);
}

#[test]
fn run_reports_unreadable_files_without_marking_them_done_and_retries_next_run() {
    let mut s = Setup::new(&[
        ("a_corrupt.jpg", b"garbage bytes".to_vec()),
        ("b_good.gif", gif_bytes(32, 32)),
    ]);

    let first = run(&s.cat).unwrap();
    assert_eq!(first.analyzed, 1);
    assert_eq!(first.suggestions, 1);
    assert_eq!(first.errors.len(), 1);
    assert!(first.errors[0].0.ends_with("a_corrupt.jpg"));

    // Still pending because it failed; a second run retries it without touching the done file.
    let second = run(&s.cat).unwrap();
    assert_eq!(second.analyzed, 0);
    assert_eq!(second.errors.len(), 1);

    // Replacing the corrupt file with a valid image and re-indexing lets the next run succeed.
    write_file(
        &s.library,
        "a_corrupt.jpg",
        &jpeg_bytes(400, 300, Some((TAG_MAKE, "Canon"))),
    );
    s.cat.index(&s.root).unwrap();

    let third = run(&s.cat).unwrap();
    assert_eq!(third.analyzed, 1);
    assert_eq!(third.suggestions, 1);
    assert!(third.errors.is_empty());
    assert!(s.cat.pending(STAGE, VERSION, 100).unwrap().is_empty());
}

#[test]
fn run_retries_the_same_asset_once_a_temporarily_unreadable_file_returns() {
    let bytes = jpeg_bytes(320, 240, Some((TAG_MODEL, "Pixel 8")));
    let s = Setup::new(&[("temp.jpg", bytes.clone())]);
    let file_path = s.library.join("temp.jpg");

    fs::remove_file(&file_path).unwrap();
    let failed = run(&s.cat).unwrap();
    assert_eq!(failed.analyzed, 0);
    assert_eq!(failed.errors.len(), 1);

    fs::write(&file_path, &bytes).unwrap();
    let recovered = run(&s.cat).unwrap();
    assert_eq!(recovered.analyzed, 1);
    assert_eq!(recovered.suggestions, 1);
    assert!(recovered.errors.is_empty());
    assert_eq!(s.labels("temp.jpg")[0].name, "foto de cámara");
    assert!(s.cat.pending(STAGE, VERSION, 10).unwrap().is_empty());
}

#[test]
fn run_with_reports_monotonic_progress_and_matches_run_results() {
    let files = [
        ("shots/Screenshot_01.png", png_bytes(1920, 1080, None)),
        (
            "camera/DSC001.jpg",
            jpeg_bytes(400, 300, Some((TAG_MAKE, "Fujifilm"))),
        ),
        ("chat/IMG-20240501-WA0003.jpg", jpeg_bytes(400, 400, None)),
        ("strips/chapter.gif", gif_bytes(40, 120)),
        ("misc/plain.png", png_bytes(200, 200, None)),
    ];
    let baseline = Setup::new(&files);
    let with_cb = Setup::new(&files);

    let r_base = run(&baseline.cat).unwrap();
    let mut seen: Vec<RunProgress> = Vec::new();
    let r_with = run_with(&with_cb.cat, &mut |p: &RunProgress| {
        seen.push(*p);
        Control::Continue
    })
    .unwrap();

    assert_eq!(r_with, r_base);
    assert!(!r_with.stopped);
    assert_eq!(
        seen.first(),
        Some(&RunProgress {
            analyzed: 0,
            pending: 5
        })
    );
    assert_eq!(
        seen.last(),
        Some(&RunProgress {
            analyzed: 5,
            pending: 0
        })
    );
    assert!(seen.windows(2).all(|w| w[0].analyzed <= w[1].analyzed));
    assert!(seen.windows(2).all(|w| w[0].pending >= w[1].pending));

    for (rel, _) in &files {
        assert_eq!(with_cb.labels(rel), baseline.labels(rel), "file {rel}");
    }

    let mut second_seen = Vec::new();
    let second = run_with(&with_cb.cat, &mut |p| {
        second_seen.push(*p);
        Control::Continue
    })
    .unwrap();
    assert_eq!(second, RunReport::default());
    assert_eq!(
        second_seen,
        vec![RunProgress {
            analyzed: 0,
            pending: 0
        }]
    );
}

#[test]
fn run_with_stop_midway_keeps_done_work_and_later_run_completes_identically() {
    let files = [
        ("shots/Screenshot_01.png", png_bytes(1920, 1080, None)),
        (
            "camera/DSC001.jpg",
            jpeg_bytes(400, 300, Some((TAG_MAKE, "Fujifilm"))),
        ),
        ("chat/IMG-20240501-WA0003.jpg", jpeg_bytes(400, 400, None)),
        ("strips/chapter.gif", gif_bytes(40, 120)),
        ("misc/plain.png", png_bytes(200, 200, None)),
    ];
    let baseline = Setup::new(&files);
    let partial = Setup::new(&files);

    // Stop immediately at 0: nothing analyzed, all 5 still pending.
    let zero = run_with(&partial.cat, &mut |_| Control::Stop).unwrap();
    assert!(zero.stopped);
    assert_eq!(zero.analyzed, 0);
    assert_eq!(partial.cat.pending(STAGE, VERSION, 100).unwrap().len(), 5);

    // Stop mid-way after 2 items: those 2 stay marked done.
    let first = run_with(&partial.cat, &mut |p| {
        if p.analyzed >= 2 {
            Control::Stop
        } else {
            Control::Continue
        }
    })
    .unwrap();
    assert!(first.stopped);
    assert_eq!(first.analyzed, 2);
    assert_eq!(partial.cat.pending(STAGE, VERSION, 100).unwrap().len(), 3);

    // Complete the remaining 3 items in a second full run.
    let rest = run_with(&partial.cat, &mut |_| Control::Continue).unwrap();
    assert!(!rest.stopped);
    assert_eq!(rest.analyzed, 3);
    assert!(partial.cat.pending(STAGE, VERSION, 100).unwrap().is_empty());

    let expected = run(&baseline.cat).unwrap();
    assert_eq!(first.analyzed + rest.analyzed, expected.analyzed);
    assert_eq!(first.suggestions + rest.suggestions, expected.suggestions);
    for (rel, _) in &files {
        assert_eq!(partial.labels(rel), baseline.labels(rel), "file {rel}");
    }
}
