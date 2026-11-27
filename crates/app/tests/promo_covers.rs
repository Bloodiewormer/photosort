// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::promo_covers::{detect_and_associate, detect_cover_regions};
use app::worker::{IndexEvent, IndexWorker};
use catalog::{Catalog, OcrLine, OcrRecognizer, Status};
use fsguard::{LibraryRoot, ensure_writable};
use image::{DynamicImage, Rgba, RgbaImage};
use std::fs;

fn create_synthetic_promo_grid() -> DynamicImage {
    let w = 240u32;
    let h = 360u32;
    let mut img = RgbaImage::from_pixel(w, h, Rgba([248, 248, 252, 255]));

    // Header banner
    for y in 14..32 {
        for x in 14..226 {
            img.put_pixel(x, y, Rgba([220, 226, 238, 255]));
        }
    }

    let cols = [(20, 110), (130, 220)];
    let rows = [(46, 118), (150, 222), (254, 326)];
    let colors = [
        [224, 92, 84, 255],   // Card 0: coral
        [66, 153, 225, 255],  // Card 1: blue
        [72, 187, 120, 255],  // Card 2: green
        [237, 173, 64, 255],  // Card 3: amber
        [159, 122, 234, 255], // Card 4: violet
        [56, 178, 172, 255],  // Card 5: teal
    ];

    let mut idx = 0;
    for &(ry0, ry1) in &rows {
        for &(cx0, cx1) in &cols {
            let color = colors[idx];
            idx += 1;
            for y in ry0..ry1 {
                for x in cx0..cx1 {
                    let on_border = x == cx0 || x + 1 == cx1 || y == ry0 || y + 1 == ry1;
                    if on_border {
                        img.put_pixel(x, y, Rgba([40, 45, 55, 255]));
                    } else {
                        img.put_pixel(x, y, Rgba(color));
                    }
                }
            }

            // Title bar below card
            for y in (ry1 + 4)..(ry1 + 16) {
                for x in (cx0 + 4)..(cx1 - 4) {
                    img.put_pixel(x, y, Rgba([50, 55, 65, 255]));
                }
            }
        }
    }

    DynamicImage::ImageRgba8(img)
}

#[test]
fn test_synthetic_2x3_promo_grid_covers_and_associations() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    let img_path = library.join("promo_grid.png");

    let dyn_img = create_synthetic_promo_grid();
    dyn_img.save(&img_path).unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");

    let db_index = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_main = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { stopped, .. } = ev {
            assert!(!stopped);
            break;
        }
    }

    let mut cat = Catalog::open(&db_main).unwrap();
    let asset = cat
        .asset_of(&dunce::canonicalize(&img_path).unwrap())
        .unwrap()
        .unwrap();

    // Create works
    let w0 = cat.create_work("Crónicas del Relojero").unwrap();
    let w1 = cat.create_work("Academia Nebulosa").unwrap();
    let w2 = cat.create_work("El Jardín de Papel").unwrap();
    let w3 = cat.create_work("Isla de Cristal").unwrap();
    let w4 = cat.create_work("Estudio Cometa").unwrap();
    let w5 = cat.create_work("Lluvia de Estrellas").unwrap();

    let lines = vec![
        OcrLine {
            text: "PROYECTOS ACTIVOS DE NUESTRO GRUPO".to_string(),
            score: 0.98,
            recognizer: OcrRecognizer::V6,
            rect: [14.0, 14.0, 226.0, 32.0],
            top_strip: false,
        },
        OcrLine {
            text: "Crónicas del Relojero".to_string(),
            score: 0.96,
            recognizer: OcrRecognizer::V6,
            rect: [18.0, 122.0, 112.0, 138.0],
            top_strip: false,
        },
        OcrLine {
            text: "Academia Nebulosa".to_string(),
            score: 0.94,
            recognizer: OcrRecognizer::V6,
            rect: [128.0, 122.0, 222.0, 138.0],
            top_strip: false,
        },
        OcrLine {
            text: "El Jardín de Papel".to_string(),
            score: 0.95,
            recognizer: OcrRecognizer::V6,
            rect: [18.0, 226.0, 112.0, 242.0],
            top_strip: false,
        },
        OcrLine {
            text: "Isla de Cristal".to_string(),
            score: 0.93,
            recognizer: OcrRecognizer::V6,
            rect: [128.0, 226.0, 222.0, 242.0],
            top_strip: false,
        },
        OcrLine {
            text: "Estudio Cometa".to_string(),
            score: 0.92,
            recognizer: OcrRecognizer::V6,
            rect: [18.0, 330.0, 112.0, 346.0],
            top_strip: false,
        },
        OcrLine {
            text: "Lluvia de Estrellas".to_string(),
            score: 0.95,
            recognizer: OcrRecognizer::V6,
            rect: [128.0, 330.0, 222.0, 346.0],
            top_strip: false,
        },
    ];

    cat.save_ocr_lines(asset, &lines).unwrap();

    let matcher = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(asset, &matcher).unwrap();

    assert!(cat.is_promo_page(asset).unwrap());

    let covers = detect_and_associate(asset, &dyn_img, &lines, &matcher, &cat, false);
    assert_eq!(covers.len(), 6, "Must detect all 6 covers in the 2x3 grid");

    // Verify ordering and associations
    assert_eq!(covers[0].work, Some(w0));
    assert_eq!(covers[1].work, Some(w1));
    assert_eq!(covers[2].work, Some(w2));
    assert_eq!(covers[3].work, Some(w3));
    assert_eq!(covers[4].work, Some(w4));
    assert_eq!(covers[5].work, Some(w5));

    for (i, c) in covers.iter().enumerate() {
        assert_eq!(c.cover_idx, i);
        assert_eq!(c.status, Status::Suggested);
        assert!(c.confidence > 0.8);
    }

    // Save to catalog
    cat.save_asset_covers(asset, &covers).unwrap();

    let loaded = cat.asset_covers(asset).unwrap();
    assert_eq!(loaded.len(), 6);
    for (i, c) in loaded.iter().enumerate() {
        assert_eq!(c.cover_idx, i);
        assert_eq!(c.work, covers[i].work);
        assert_eq!(c.work_name, covers[i].work_name);
        assert_eq!(c.status, Status::Suggested);
    }
}

#[test]
fn test_render_demo_image_detection() {
    let spec = app::demo::ImageSpec {
        index: 0,
        rel_path: std::path::PathBuf::from("anime/Crónicas del Relojero - Capítulo 23.00 - 7.png"),
        width: 240,
        height: 360,
        format: app::demo::DemoFormat::Png,
        with_alpha: false,
        pattern: app::demo::PatternKind::PromoGrid,
        duplicate_of: None,
    };
    let bytes = app::demo::render_demo_image(&spec, 0).unwrap();
    let img = image::load_from_memory(&bytes).unwrap();
    let regions = detect_cover_regions(&img);
    println!("REGIONS DETECTED FOR RENDER DEMO IMAGE: {:?}", regions);
    assert_eq!(regions.len(), 6);
}

#[test]
fn test_non_grid_image_produces_no_regions() {
    // 1. Solid color image
    let solid =
        DynamicImage::ImageRgba8(RgbaImage::from_pixel(240, 360, Rgba([200, 200, 200, 255])));
    let regions = detect_cover_regions(&solid);
    assert!(
        regions.is_empty(),
        "Solid color image must not produce cover regions"
    );

    // 2. Horizontal gradient
    let mut grad = RgbaImage::new(240, 360);
    for y in 0..360 {
        for x in 0..240 {
            let v = (x * 255 / 240) as u8;
            grad.put_pixel(x, y, Rgba([v, v, v, 255]));
        }
    }
    let regions_grad = detect_cover_regions(&DynamicImage::ImageRgba8(grad));
    assert!(
        regions_grad.is_empty(),
        "Gradient image must not produce cover regions"
    );
}

#[test]
fn test_user_decision_preserved_across_recalculation() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    let img_path = library.join("promo_grid.png");

    let dyn_img = create_synthetic_promo_grid();
    dyn_img.save(&img_path).unwrap();

    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");

    let db_index = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_main = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        if let IndexEvent::Finished { stopped, .. } = ev {
            assert!(!stopped);
            break;
        }
    }

    let mut cat = Catalog::open(&db_main).unwrap();
    let asset = cat
        .asset_of(&dunce::canonicalize(&img_path).unwrap())
        .unwrap()
        .unwrap();

    let _w0 = cat.create_work("Crónicas del Relojero").unwrap();
    let _w1 = cat.create_work("Academia Nebulosa").unwrap();

    let lines = vec![
        OcrLine {
            text: "PROYECTOS ACTIVOS DE NUESTRO GRUPO".to_string(),
            score: 0.98,
            recognizer: OcrRecognizer::V6,
            rect: [14.0, 14.0, 226.0, 32.0],
            top_strip: false,
        },
        OcrLine {
            text: "Crónicas del Relojero".to_string(),
            score: 0.96,
            recognizer: OcrRecognizer::V6,
            rect: [18.0, 122.0, 112.0, 138.0],
            top_strip: false,
        },
        OcrLine {
            text: "Academia Nebulosa".to_string(),
            score: 0.94,
            recognizer: OcrRecognizer::V6,
            rect: [128.0, 122.0, 222.0, 138.0],
            top_strip: false,
        },
    ];
    cat.save_ocr_lines(asset, &lines).unwrap();

    let matcher = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(asset, &matcher).unwrap();

    let covers = detect_and_associate(asset, &dyn_img, &lines, &matcher, &cat, false);
    assert_eq!(covers.len(), 6);

    cat.save_asset_covers(asset, &covers).unwrap();

    // User accepts cover 0 and rejects cover 1
    cat.accept_asset_cover(asset, 0).unwrap();
    cat.reject_asset_cover(asset, 1).unwrap();

    let loaded_before = cat.asset_covers(asset).unwrap();
    assert_eq!(loaded_before[0].status, Status::Accepted);
    assert_eq!(loaded_before[1].status, Status::Rejected);
    assert_eq!(loaded_before[2].status, Status::Suggested);

    // Recalculate (simulating works_worker running again on catalog)
    let recomputed_covers = detect_and_associate(asset, &dyn_img, &lines, &matcher, &cat, false);
    assert_eq!(recomputed_covers.len(), 6);
    // Notice recomputed_covers all have status = Suggested freshly computed
    assert_eq!(recomputed_covers[0].status, Status::Suggested);
    assert_eq!(recomputed_covers[1].status, Status::Suggested);

    // Saving recomputed covers must preserve user choices
    cat.save_asset_covers(asset, &recomputed_covers).unwrap();

    let loaded_after = cat.asset_covers(asset).unwrap();
    assert_eq!(loaded_after.len(), 6);
    assert_eq!(
        loaded_after[0].status,
        Status::Accepted,
        "User accepted status must be preserved"
    );
    assert_eq!(
        loaded_after[1].status,
        Status::Rejected,
        "User rejected status must be preserved"
    );
    assert_eq!(
        loaded_after[2].status,
        Status::Suggested,
        "Unmodified status remains suggested"
    );
}

#[test]
fn works_worker_decodes_each_promo_page_once_and_stays_in_roots() {
    use app::works_worker::{WorksEvent, WorksWorker};
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    let img_path = library.join("promo_grid.png");
    create_synthetic_promo_grid().save(&img_path).unwrap();
    let root = LibraryRoot::new(&library).unwrap();
    let db_path = tmp.path().join("catalog.sqlite");
    let db = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    let asset = cat
        .asset_of(&dunce::canonicalize(&img_path).unwrap())
        .unwrap()
        .unwrap();
    let line = |text: &str, rect: [f32; 4]| OcrLine {
        text: text.to_string(),
        score: 0.95,
        recognizer: OcrRecognizer::V6,
        rect,
        top_strip: false,
    };
    for title in [
        "Crónicas del Relojero",
        "Academia Nebulosa",
        "El Jardín de Papel",
    ] {
        cat.create_work(title).unwrap();
    }
    cat.save_ocr_lines(
        asset,
        &[
            line(
                "PROYECTOS ACTIVOS DE NUESTRO GRUPO",
                [14.0, 14.0, 226.0, 32.0],
            ),
            line("Crónicas del Relojero", [18.0, 122.0, 112.0, 138.0]),
            line("Academia Nebulosa", [128.0, 122.0, 222.0, 138.0]),
            line("El Jardín de Papel", [18.0, 226.0, 112.0, 242.0]),
        ],
    )
    .unwrap();
    cat.mark_done(asset, catalog::OCR_STAGE, catalog::OCR_VERSION)
        .unwrap();

    let run = |roots: Vec<std::path::PathBuf>| {
        let db = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
        let worker = WorksWorker::start_in_roots(db, Some(roots), None);
        while let Ok(ev) = worker.recv() {
            match ev {
                WorksEvent::Finished { .. } => break,
                WorksEvent::Failed(e) => panic!("{e}"),
                WorksEvent::Progress { .. } => {}
            }
        }
    };

    // Outside the registered folders nothing is computed.
    run(vec![tmp.path().join("other")]);
    assert!(cat.asset_covers(asset).unwrap().is_empty());

    run(vec![root.as_path().to_path_buf()]);
    assert_eq!(cat.asset_covers(asset).unwrap().len(), 6);

    // A second run must not decode the image again: replacing it with a blank one (in this temp
    // fixture) would yield no regions if it did.
    DynamicImage::ImageRgba8(RgbaImage::from_pixel(240, 360, Rgba([200, 200, 200, 255])))
        .save(&img_path)
        .unwrap();
    run(vec![root.as_path().to_path_buf()]);
    let covers = cat.asset_covers(asset).unwrap();
    assert_eq!(covers.len(), 6);
    assert!(covers.iter().filter(|c| c.work.is_some()).count() >= 3);
}

#[test]
fn a_title_equally_close_to_two_covers_is_left_unassigned() {
    let tmp = tempfile::tempdir().unwrap();
    let db = ensure_writable(&tmp.path().join("catalog.sqlite"), &[]).unwrap();
    let cat = Catalog::open(&db).unwrap();
    let work = cat.create_work("Isla de Cristal").unwrap();
    let other = cat.create_work("Academia Nebulosa").unwrap();
    let covers = [
        [0.0, 0.0, 100.0, 150.0],
        [0.0, 170.0, 100.0, 320.0],
        [200.0, 0.0, 300.0, 150.0],
    ];
    let works = [
        // Right in the gap between the two stacked covers.
        catalog::WorkLineMatch {
            label: work,
            confidence: 0.9,
            rect: [10.0, 155.0, 90.0, 165.0],
        },
        // Clearly below the third cover.
        catalog::WorkLineMatch {
            label: other,
            confidence: 0.9,
            rect: [210.0, 155.0, 290.0, 165.0],
        },
    ];
    let out = app::promo_covers::associate_covers(catalog::AssetId(1), &covers, &works, &cat);
    assert_eq!(out[0].work, None, "{out:?}");
    assert_eq!(out[1].work, None, "{out:?}");
    assert_eq!(out[2].work, Some(other), "{out:?}");
}

/// Textured cards (`cols` × `rows`) with an optional striped title under each, over `bg`.
fn card_grid(
    (w, h): (u32, u32),
    bg: impl Fn(u32, u32) -> [u8; 3],
    (cols, rows): (u32, u32),
    gutter: u32,
    title: bool,
) -> image::RgbImage {
    let margin = 40;
    let mut img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb(bg(x, y)));
    let cw = (w - 2 * margin - gutter * (cols - 1)) / cols;
    let ch = (h - 2 * margin - gutter * (rows - 1)) / rows;
    for r in 0..rows {
        for c in 0..cols {
            let (x0, y0) = (margin + c * (cw + gutter), margin + r * (ch + gutter));
            let card_h = if title { ch * 3 / 4 } else { ch };
            for y in y0..y0 + card_h {
                for x in x0..x0 + cw {
                    let v = ((x * 7 + y * 13 + (r * cols + c) * 50) % 200) as u8;
                    img.put_pixel(x, y, image::Rgb([v, 255 - v, v / 2 + 40]));
                }
            }
            if title {
                for y in y0 + card_h + 4..y0 + card_h + 14 {
                    for x in (x0 + 4..x0 + cw - 4).filter(|x| (x / 3) % 2 == 0) {
                        img.put_pixel(x, y, image::Rgb([20, 20, 20]));
                    }
                }
            }
        }
    }
    img
}

#[test]
fn harder_layouts_give_plausible_regions_or_none() {
    let detect = |img: image::RgbImage| {
        let (w, h) = img.dimensions();
        let regions = detect_cover_regions(&DynamicImage::ImageRgb8(img));
        // Never the whole page and never thin strips.
        for r in &regions {
            let (rw, rh) = ((r[2] - r[0]) / w as f32, (r[3] - r[1]) / h as f32);
            assert!(rw < 0.61 && rh < 0.71 && rw > 0.07 && rh > 0.07, "{r:?}");
        }
        regions.len()
    };
    let white = |_: u32, _: u32| [250, 250, 250];
    assert_eq!(detect(card_grid((900, 1300), white, (3, 3), 30, true)), 9);
    assert_eq!(detect(card_grid((800, 1000), white, (2, 2), 40, true)), 4);
    // Strong gradient background, cards that touch, one big panel per half: nothing.
    let dark_to_light = |_: u32, y: u32| {
        let v = (40 + y * 180 / 1300) as u8;
        [v, v, v + 20]
    };
    assert_eq!(
        detect(card_grid((900, 1300), dark_to_light, (3, 3), 30, true)),
        0
    );
    assert_eq!(detect(card_grid((900, 1300), white, (3, 3), 0, false)), 0);
    assert_eq!(detect(card_grid((800, 1200), white, (2, 1), 40, false)), 0);
    // A soft gradient may lose some cards, but what it finds is still card-sized (checked above).
    let soft = |_: u32, y: u32| {
        let v = (200 + y * 50 / 1300) as u8;
        [v, v, v]
    };
    assert!(detect(card_grid((900, 1300), soft, (3, 3), 30, true)) <= 9);
    // A decorative frame around the grid hides the page background: nothing.
    let mut framed = card_grid((900, 1300), white, (3, 3), 30, true);
    for (x, y, p) in framed.enumerate_pixels_mut() {
        if x < 20 || y < 20 || x >= 880 || y >= 1280 {
            *p = image::Rgb([120, 30, 30]);
        }
    }
    assert_eq!(detect(framed), 0);
    // Two columns of plain text.
    let mut text = image::RgbImage::from_pixel(900, 1300, image::Rgb([250, 250, 250]));
    for col in 0..2u32 {
        for line in (0..60u32).filter(|l| l % 12 != 11) {
            for y in 60 + line * 20..70 + line * 20 {
                for x in (60 + col * 420..420 + col * 420).filter(|x| (x / 4 + line) % 3 != 0) {
                    text.put_pixel(x, y, image::Rgb([30, 30, 30]));
                }
            }
        }
    }
    assert_eq!(detect(text), 0);
}
