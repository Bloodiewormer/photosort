//! Measures face detection and grouping on an approved sample folder. Writes, outside the
//! library: `rostros.csv` (one row per detected face) and numbered contact sheets of the face
//! crops, so a person can check which faces are real and which belong together.
//! Usage: cargo run -p app --release --example calibrar_rostros -- <sample> <output> <models>

// Writes only to paths checked by the guard (`ensure_writable`), never inside the sample folder.
#![allow(clippy::disallowed_methods)]

use app::review_engine::{EnginePhase, ReviewConfig, ReviewEngine, ReviewEngineEvent};
use app::vision_worker::{VISION_DECODE_MAX_PX, session_options};
use catalog::{Catalog, Filter, Sort, Source};
use fsguard::{LibraryRoot, ensure_export_dir, ensure_writable};
use image::{Rgb, RgbImage, imageops};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

const CELL: u32 = 110;
const COLS: u32 = 12;
const ROWS: u32 = 8;
const TYPES: [&str; 6] = [
    "Manga",
    "Ilustración",
    "Foto",
    "Capturas de pantalla",
    "Meme",
    "Documento",
];

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [sample, out, models] = args.as_slice() else {
        panic!("uso: calibrar_rostros <muestra> <salida> <modelos>");
    };
    let root = LibraryRoot::new(sample).unwrap();
    let roots = [LibraryRoot::new(sample).unwrap()];
    let out_dir = ensure_export_dir(out, &roots).unwrap();
    let db_path = out_dir.as_path().join("catalogo.sqlite");
    let mut cat = Catalog::open(&ensure_writable(&db_path, &roots).unwrap()).unwrap();
    cat.index(&root).unwrap();

    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: true,
        ocr: false,
        roots: vec![root.as_path().to_path_buf()],
        vision_models_dir: Some(models.clone()),
        ..Default::default()
    };
    let engine = ReviewEngine::start(ensure_writable(&db_path, &roots).unwrap(), cfg, None);
    loop {
        match engine.recv_timeout(Duration::from_millis(500)) {
            Ok(ReviewEngineEvent::Status(s)) => match s.phase {
                EnginePhase::Idle => break,
                EnginePhase::Blocked(r) => panic!("bloqueado: {r}"),
                _ => {}
            },
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(e) => panic!("{e}"),
        }
    }
    drop(engine);

    let mut sface = vision::SFace::load(models, &session_options(Default::default())).unwrap();
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    let mut csv = String::from("face,img,type,score,w,h,emb\n");
    let mut crops: Vec<RgbImage> = Vec::new();
    for (img_idx, e) in entries.iter().enumerate() {
        let faces = cat.asset_face_details(e.asset).unwrap();
        if faces.is_empty() {
            continue;
        }
        let ty = cat
            .labels_of(e.asset)
            .unwrap()
            .into_iter()
            .find(|a| a.source == Source::Ai && TYPES.contains(&a.name.as_str()))
            .map(|a| a.name)
            .unwrap_or_default();
        let img = thumbs::decode(&e.path, VISION_DECODE_MAX_PX)
            .unwrap()
            .to_rgb8();
        for f in faces {
            let emb = sface
                .feature(&vision::align_face(&img, &f.landmarks))
                .unwrap();
            let [x1, y1, x2, y2] = f.bbox;
            let emb_txt: Vec<String> = emb.iter().map(|v| format!("{v:.5}")).collect();
            writeln!(
                csv,
                "{},{img_idx},{ty},{:.3},{:.0},{:.0},{}",
                crops.len(),
                f.score,
                x2 - x1,
                y2 - y1,
                emb_txt.join(" ")
            )
            .unwrap();
            let (x, y) = (x1.max(0.0) as u32, y1.max(0.0) as u32);
            let w = ((x2 - x1) as u32).clamp(1, img.width().saturating_sub(x).max(1));
            let h = ((y2 - y1) as u32).clamp(1, img.height().saturating_sub(y).max(1));
            let crop = imageops::crop_imm(&img, x, y, w, h).to_image();
            crops.push(imageops::resize(
                &crop,
                CELL - 4,
                CELL - 4,
                imageops::FilterType::Triangle,
            ));
        }
    }
    let p = ensure_writable(&out_dir.as_path().join("rostros.csv"), &roots).unwrap();
    std::fs::write(p.as_path(), csv).unwrap();

    let per = (COLS * ROWS) as usize;
    for (s, chunk) in crops.chunks(per).enumerate() {
        let mut sheet = RgbImage::from_pixel(COLS * CELL, ROWS * CELL, Rgb([40, 40, 40]));
        for (j, c) in chunk.iter().enumerate() {
            let (x, y) = ((j as u32 % COLS) * CELL, (j as u32 / COLS) * CELL);
            imageops::overlay(&mut sheet, c, i64::from(x + 2), i64::from(y + 2));
            draw_number(&mut sheet, x + 2, y + 2, s * per + j);
        }
        let p =
            ensure_writable(&out_dir.as_path().join(format!("caras_{s:02}.png")), &roots).unwrap();
        let mut buf = std::io::Cursor::new(Vec::new());
        sheet.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        std::fs::write(p.as_path(), buf.get_ref()).unwrap();
    }
    println!(
        "{} rostros en {} hojas",
        crops.len(),
        crops.len().div_ceil(per)
    );
}

/// 3x5 digits scaled 3x on a black box.
fn draw_number(img: &mut RgbImage, x: u32, y: u32, n: usize) {
    const D: [u16; 10] = [
        0o75557, 0o22222, 0o71747, 0o71717, 0o55711, 0o74717, 0o74757, 0o71111, 0o75757, 0o75717,
    ];
    let text = n.to_string();
    for py in 0..19 {
        for px in 0..(text.len() as u32 * 12 + 4) {
            img.put_pixel(x + px, y + py, Rgb([0, 0, 0]));
        }
    }
    for (k, c) in text.bytes().enumerate() {
        let bits = D[(c - b'0') as usize];
        for row in 0..5u32 {
            for col in 0..3u32 {
                if bits >> ((4 - row) * 3 + (2 - col)) & 1 == 1 {
                    for d in 0..9 {
                        img.put_pixel(
                            x + 2 + k as u32 * 12 + col * 3 + d % 3,
                            y + 2 + row * 3 + d / 3,
                            Rgb([255, 220, 0]),
                        );
                    }
                }
            }
        }
    }
}
