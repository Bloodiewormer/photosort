//! Runs the real vision stage over a sample folder and writes, outside the library:
//! a CSV with every rule's cosine score per image, and numbered contact sheets.
//! Usage: cargo run -p app --release --example calibrar -- <sample folder> <output folder> <models folder>

// Writes only to paths checked by the guard (`ensure_writable`), never inside the sample folder.
#![allow(clippy::disallowed_methods)]

use app::review_engine::{EnginePhase, ReviewConfig, ReviewEngine, ReviewEngineEvent};
use catalog::{Catalog, Filter, SIGLIP2_MODEL_ID, Sort, Source};
use fsguard::{LibraryRoot, ensure_export_dir, ensure_writable};
use image::{Rgb, RgbImage, imageops};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CELL: u32 = 220;
const COLS: u32 = 6;
const ROWS: u32 = 5;

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [sample, out, models] = args.as_slice() else {
        panic!("uso: calibrar <carpeta de muestra> <carpeta de salida> <carpeta de modelos>");
    };
    let root = LibraryRoot::new(sample).expect("carpeta de muestra");
    let roots = [LibraryRoot::new(sample).unwrap()];
    let out_dir = ensure_export_dir(out, &roots).expect("carpeta de salida");
    let db = ensure_writable(&out_dir.as_path().join("catalogo.sqlite"), &roots).unwrap();

    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    println!("{} imágenes indexadas", entries.len());

    let cfg = ReviewConfig {
        enabled: true,
        rules: false,
        vision: true,
        ocr: false,
        roots: vec![root.as_path().to_path_buf()],
        vision_models_dir: Some(models.clone()),
        ..Default::default()
    };
    let start = Instant::now();
    let db2 = ensure_writable(&out_dir.as_path().join("catalogo.sqlite"), &roots).unwrap();
    let engine = ReviewEngine::start(db2, cfg, None);
    loop {
        match engine.recv_timeout(Duration::from_millis(500)) {
            Ok(ReviewEngineEvent::Status(s)) => match s.phase {
                EnginePhase::Idle => break,
                EnginePhase::Blocked(reason) => panic!("bloqueado: {reason}"),
                _ => {}
            },
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(e) => panic!("{e}"),
        }
    }
    drop(engine);
    println!("visión terminada en {:.1} s", start.elapsed().as_secs_f64());

    let (compiled, missing) = cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();
    assert!(missing.is_empty(), "faltan textos: {missing:?}");
    let mut csv = String::from("idx,asset,faces_max,suggested");
    for r in &compiled.rules {
        write!(csv, ",{0}|pos,{0}|neg", r.name).unwrap();
    }
    csv.push('\n');
    for (i, e) in entries.iter().enumerate() {
        let Some((_, v)) = cat.asset_embedding(e.asset).unwrap() else {
            writeln!(csv, "{i},{},,SIN_EMBEDDING", e.asset.0).unwrap();
            continue;
        };
        let faces = cat.asset_faces(e.asset).unwrap();
        let fmax = faces.iter().map(|f| f.score).fold(0.0f32, f32::max);
        let sugg: Vec<String> = cat
            .labels_of(e.asset)
            .unwrap()
            .into_iter()
            .filter(|a| a.source == Source::Ai)
            .map(|a| a.name)
            .collect();
        write!(csv, "{i},{},{fmax:.3},{}", e.asset.0, sugg.join(";")).unwrap();
        for r in &compiled.rules {
            let best = |set: &[[f32; catalog::EMBEDDING_DIM]]| {
                set.iter()
                    .map(|t| t.iter().zip(&v).map(|(a, b)| a * b).sum::<f32>())
                    .fold(f32::NAN, f32::max)
            };
            write!(csv, ",{:.4},{:.4}", best(&r.positives), best(&r.negatives)).unwrap();
        }
        csv.push('\n');
    }
    let csv_path = ensure_writable(&out_dir.as_path().join("puntuaciones.csv"), &roots).unwrap();
    std::fs::write(csv_path.as_path(), csv.as_bytes()).unwrap();

    let per = (COLS * ROWS) as usize;
    for (s, chunk) in entries.chunks(per).enumerate() {
        let mut sheet = RgbImage::from_pixel(COLS * CELL, ROWS * CELL, Rgb([40, 40, 40]));
        for (j, e) in chunk.iter().enumerate() {
            let (x, y) = ((j as u32 % COLS) * CELL, (j as u32 / COLS) * CELL);
            if let Ok(img) = thumbs::decode(&e.path, CELL - 8) {
                let img = img.to_rgb8();
                let (ox, oy) = ((CELL - img.width()) / 2, (CELL - img.height()) / 2);
                imageops::overlay(&mut sheet, &img, i64::from(x + ox), i64::from(y + oy));
            }
            draw_number(&mut sheet, x + 2, y + 2, s * per + j);
        }
        let p =
            ensure_writable(&out_dir.as_path().join(format!("hoja_{s:02}.png")), &roots).unwrap();
        let mut buf = std::io::Cursor::new(Vec::new());
        sheet.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        std::fs::write(p.as_path(), buf.get_ref()).unwrap();
    }
    println!("listo");
}

/// 3x5 digits scaled 4x on a black box, so the index can be read on the sheet.
fn draw_number(img: &mut RgbImage, x: u32, y: u32, n: usize) {
    const D: [u16; 10] = [
        0o75557, 0o22222, 0o71747, 0o71717, 0o55711, 0o74717, 0o74757, 0o71111, 0o75757, 0o75717,
    ];
    let text = n.to_string();
    let w = text.len() as u32 * 16 + 4;
    for py in 0..24 {
        for px in 0..w {
            img.put_pixel(x + px, y + py, Rgb([0, 0, 0]));
        }
    }
    for (k, c) in text.bytes().enumerate() {
        let bits = D[(c - b'0') as usize];
        for row in 0..5u32 {
            for col in 0..3u32 {
                if bits >> ((4 - row) * 3 + (2 - col)) & 1 == 1 {
                    for dy in 0..4 {
                        for dx in 0..4 {
                            img.put_pixel(
                                x + 2 + k as u32 * 16 + col * 4 + dx,
                                y + 2 + row * 4 + dy,
                                Rgb([255, 220, 0]),
                            );
                        }
                    }
                }
            }
        }
    }
}
