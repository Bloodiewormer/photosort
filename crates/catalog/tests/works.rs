// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, LabelId, LabelKind, OcrLine, OcrRecognizer, PROMO_CATEGORY,
    SCREENSHOT_CATEGORY, Source, Status, WorkEntry, WorkMatcher, extract_work_title_from_stem,
    work_match_key,
};
use fsguard::{LibraryRoot, ensure_writable};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::time::Instant;

fn line(text: &str, score: f32, rect: [f32; 4]) -> OcrLine {
    OcrLine {
        text: text.to_string(),
        score,
        recognizer: OcrRecognizer::V6,
        rect,
        top_strip: false,
    }
}

fn setup_catalog(
    files: &[(&str, &[u8])],
) -> (tempfile::TempDir, Catalog, HashMap<String, AssetId>) {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    fs::create_dir_all(&lib).unwrap();
    for (name, content) in files {
        let p = lib.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&p, content).unwrap();
    }
    let root = LibraryRoot::new(&lib).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();

    let mut ids = HashMap::new();
    for (name, _) in files {
        let canonical = dunce::canonicalize(lib.join(name)).unwrap();
        let id = cat.asset_of(&canonical).unwrap().unwrap();
        ids.insert((*name).to_string(), id);
    }
    (tmp, cat, ids)
}

#[test]
fn work_match_key_strips_punctuation_and_keeps_kana_dakuten_cjk_hangul() {
    assert_eq!(
        work_match_key("¡Quiero ser alabado por una Gal Gamer!"),
        "quieroseralabadoporunagalgamer"
    );
    assert_eq!(
        work_match_key("Please, Don't Livestream It!"),
        "pleasedontlivestreamit"
    );
    assert_eq!(work_match_key("14-sai no Koi"), "14sainokoi");
    // Kana with dakuten/handakuten, prolonged sound mark, CJK, and Hangul must stay intact.
    assert_eq!(work_match_key("お姉ちゃんが来た！"), "お姉ちゃんが来た");
    assert_eq!(work_match_key("パーティー・ヴ"), "パーティーヴ");
    assert_eq!(work_match_key("바람 공주 · 3화!"), "바람공주3화");
}

#[test]
fn aliases_crud_and_list_works_counts() {
    let (_tmp, cat, ids) = setup_catalog(&[("a.png", b"img-a"), ("b.png", b"img-b")]);
    let w1 = cat.create_work("Crónicas del Relojero").unwrap();
    let w2 = cat.create_work("Isla de Cristal").unwrap();

    assert!(cat.add_work_alias(w1, "Clockmaker Chronicles").unwrap());
    // Duplicate normalized key is ignored.
    assert!(!cat.add_work_alias(w1, "clockmaker-chronicles!").unwrap());
    // Too short (<2 alphanumeric chars) is ignored.
    assert!(!cat.add_work_alias(w1, "A!").unwrap());
    assert!(cat.add_work_alias(w1, "Relojero").unwrap());

    let aliases = cat.work_aliases(w1).unwrap();
    assert_eq!(aliases.len(), 2);

    // Image counts include accepted and suggested, exclude rejected.
    cat.suggest(ids["a.png"], w1, Source::Ocr, 0.9).unwrap();
    cat.accept(ids["b.png"], w1).unwrap();
    cat.reject(ids["a.png"], w2).unwrap();

    let works = cat.list_works().unwrap();
    assert_eq!(works.len(), 2);
    let s1 = works.iter().find(|w| w.label == w1).unwrap();
    let s2 = works.iter().find(|w| w.label == w2).unwrap();
    assert_eq!(s1.aliases, 2);
    assert_eq!(s1.images, 2);
    assert_eq!(s2.aliases, 0);
    assert_eq!(s2.images, 0);

    assert!(cat.remove_work_alias(w1, "relojero").unwrap());
    assert_eq!(cat.work_aliases(w1).unwrap().len(), 1);
}

#[test]
fn matcher_rules_by_length_digits_confidence_and_subsumption() {
    let entries = vec![
        WorkEntry {
            label: LabelId(1),
            name: "X".into(), // <2 chars: ignored
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(2),
            name: "Koi".into(), // 3 chars (2..=5): whole candidate only
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(3),
            name: "Senpai".into(), // 6 chars: substring allowed
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(4),
            name: "Uchi no Kaisha no Chiisai Senpai no Hanashi".into(), // longer, contains "senpai"
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(5),
            name: "Chronicle Season 2".into(), // digits must match exactly
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(6),
            name: "Nebula Academy".into(), // 13 chars -> max_err = 1
            aliases: vec![],
        },
    ];
    let matcher = WorkMatcher::build(&entries);

    // 1. Short alias (2..=5) matches whole candidate, NOT substring.
    let res = matcher.match_asset(
        &[line("14-sai no Koi", 0.9, [0.0, 0.0, 100.0, 20.0])],
        false,
    );
    assert!(
        !res.works.iter().any(|w| w.label == LabelId(2)),
        "2..=5 alias must not match as substring"
    );
    let res = matcher.match_asset(&[line("Koi!", 0.85, [0.0, 0.0, 40.0, 20.0])], false);
    assert_eq!(res.works.len(), 1);
    assert_eq!(res.works[0].label, LabelId(2));
    assert!((res.works[0].confidence - 0.85).abs() < 1e-5);

    // 2. Single-char work is always ignored.
    let res = matcher.match_asset(&[line("X", 0.99, [0.0, 0.0, 20.0, 20.0])], false);
    assert!(res.works.is_empty());

    // 3. Subsumption: "Senpai" inside "Uchi no Kaisha no Chiisai Senpai no Hanashi" on same lines.
    let res = matcher.match_asset(
        &[
            line("Uchi no Kaisha no Chiisai", 0.90, [10.0, 10.0, 220.0, 30.0]),
            line("Senpai no Hanashi", 0.80, [30.0, 38.0, 200.0, 58.0]),
        ],
        false,
    );
    assert_eq!(res.works.len(), 1, "shorter subsumed work must be dropped");
    assert_eq!(res.works[0].label, LabelId(4));
    assert!(
        (res.works[0].confidence - 0.80).abs() < 1e-5,
        "exact chain confidence must be min OCR score of lines used"
    );

    // 4. Digits must match exactly ("Season 1" must NOT match "Season 2").
    let res = matcher.match_asset(
        &[line("Chronicle Season 1", 0.95, [0.0, 0.0, 200.0, 20.0])],
        false,
    );
    assert!(
        !res.works.iter().any(|w| w.label == LabelId(5)),
        "Season 1 must not match Season 2"
    );
    let res = matcher.match_asset(
        &[line("Chronicle Season 20", 0.95, [0.0, 0.0, 200.0, 20.0])],
        false,
    );
    assert!(
        !res.works.iter().any(|w| w.label == LabelId(5)),
        "Season 20 must not match Season 2"
    );
    // Non-digit typo in "Chronicle Season 2" (1 error out of 16 chars <= 1) SHOULD match.
    let res = matcher.match_asset(
        &[line("Chrcnicle Season 2", 0.90, [0.0, 0.0, 200.0, 20.0])],
        false,
    );
    assert_eq!(res.works.len(), 1);
    assert_eq!(res.works[0].label, LabelId(5));
    let expected_conf = 0.90 * (1.0 - 1.0 / 16.0);
    assert!((res.works[0].confidence - expected_conf).abs() < 1e-4);

    // 5. Approximate match (13 chars, 1 error allowed, 2 errors rejected).
    let res_1err = matcher.match_asset(
        &[line("Nebula Acaderny", 0.80, [0.0, 0.0, 150.0, 20.0])], // m -> rn (2 edits: sub+ins)
        false,
    );
    assert!(
        res_1err.works.is_empty(),
        "2 edits on 13-char key exceeds len/10 = 1"
    );
    let res_ok = matcher.match_asset(
        &[line("Nebula Acadeny", 0.80, [0.0, 0.0, 150.0, 20.0])], // 1 edit: m -> n
        false,
    );
    assert_eq!(res_ok.works.len(), 1);
    assert_eq!(res_ok.works[0].label, LabelId(6));
    let expected = 0.80 * (1.0 - 1.0 / 13.0);
    assert!((res_ok.works[0].confidence - expected).abs() < 1e-4);
}

#[test]
fn two_column_interleaved_lines_chain_vertically_never_across_columns() {
    let entries = vec![
        WorkEntry {
            label: LabelId(1),
            name: "Alpha Beta Gamma".into(),
            aliases: vec![],
        },
        WorkEntry {
            label: LabelId(2),
            name: "Delta Epsilon Zeta".into(),
            aliases: vec![],
        },
        // Cross-column bogus combination that MUST NOT match:
        WorkEntry {
            label: LabelId(3),
            name: "Alpha Delta".into(),
            aliases: vec![],
        },
    ];
    let matcher = WorkMatcher::build(&entries);

    // Interleaved line order as produced by a detector scanning top-to-bottom across 2 columns.
    let lines = vec![
        line("Alpha", 0.92, [20.0, 100.0, 120.0, 120.0]),
        line("Delta", 0.91, [300.0, 102.0, 400.0, 122.0]),
        line("Beta", 0.88, [25.0, 128.0, 115.0, 148.0]),
        line("Epsilon", 0.87, [295.0, 130.0, 405.0, 150.0]),
        line("Gamma", 0.95, [22.0, 156.0, 118.0, 176.0]),
        line("Zeta", 0.85, [310.0, 158.0, 390.0, 178.0]),
    ];

    let res = matcher.match_asset(&lines, false);
    let matched: HashSet<LabelId> = res.works.iter().map(|w| w.label).collect();
    assert!(
        matched.contains(&LabelId(1)),
        "left 3-line column must match"
    );
    assert!(
        matched.contains(&LabelId(2)),
        "right 3-line column must match"
    );
    assert!(
        !matched.contains(&LabelId(3)),
        "lines from different columns must never be chained"
    );
}

#[test]
fn screenshot_top_strip_excluded_only_on_screenshots() {
    let entries = vec![WorkEntry {
        label: LabelId(1),
        name: "Academia Nebulosa".into(),
        aliases: vec![],
    }];
    let matcher = WorkMatcher::build(&entries);

    let top_line = OcrLine {
        text: "Academia Nebulosa".into(),
        score: 0.95,
        recognizer: OcrRecognizer::V6,
        rect: [10.0, 4.0, 200.0, 20.0],
        top_strip: true,
    };

    assert!(
        matcher
            .match_asset(std::slice::from_ref(&top_line), true)
            .works
            .is_empty(),
        "top_strip must be ignored on screenshots"
    );
    assert_eq!(
        matcher.match_asset(&[top_line], false).works.len(),
        1,
        "top_strip must still match on non-screenshots"
    );
}

#[test]
fn reference_case_from_domain_md_matches_all_14_works_and_group_distinctly() {
    let titles = [
        "Los Otakus Fansub",
        "OL and Ghost Arms",
        "Itoshi no Sakura-san",
        "Sampakugan-chan wa Tsutaetai",
        "Koisuru Yankee Girl",
        "Tonari no Kimi ga Ichiban Kawaii",
        "Please, Don't Livestream It!",
        "Sanpaku Eyes Extras",
        "14-sai no Koi",
        "A Kind World",
        "Midashitai Giya-san to Midarenai",
        "Onee-chan ga Kita",
        "Uchi no Kaisha no Chiisai Senpai no Hanashi",
        "The Classmate I Hate!",
        "They Were Light Pink",
    ];
    let entries: Vec<WorkEntry> = titles
        .iter()
        .enumerate()
        .map(|(i, &name)| WorkEntry {
            label: LabelId((i + 1) as i64),
            name: name.to_string(),
            aliases: vec![],
        })
        .collect();
    let matcher = WorkMatcher::build(&entries);

    // Multi-column promo layout with wrapped 2- and 3-line titles, interleaved line order.
    let lines = vec![
        line("PROYECTOS ACTIVOS DE:", 0.96, [20.0, 20.0, 420.0, 52.0]),
        line("Los Otakus Fansub", 0.94, [860.0, 22.0, 1280.0, 52.0]),
        // Row 1 (5 columns, interleaved row-by-row)
        line("OL and Ghost", 0.92, [20.0, 400.0, 220.0, 422.0]),
        line("Itoshi no", 0.91, [280.0, 400.0, 460.0, 422.0]),
        line("Sampakugan-chan", 0.90, [540.0, 400.0, 740.0, 422.0]),
        line("Koisuru Yankee", 0.93, [800.0, 400.0, 1000.0, 422.0]),
        line("Tonari no Kimi ga", 0.89, [1060.0, 400.0, 1260.0, 422.0]),
        line("Arms", 0.95, [80.0, 430.0, 160.0, 452.0]),
        line("Sakura-san", 0.92, [300.0, 430.0, 440.0, 452.0]),
        line("wa Tsutaetai", 0.91, [560.0, 430.0, 720.0, 452.0]),
        line("Girl", 0.94, [870.0, 430.0, 930.0, 452.0]),
        line("Ichiban Kawaii", 0.88, [1080.0, 430.0, 1240.0, 452.0]),
        // Row 2 (5 columns)
        line("Please, Don't", 0.92, [20.0, 800.0, 220.0, 822.0]),
        line("Sanpaku Eyes", 0.91, [280.0, 800.0, 480.0, 822.0]),
        line("14-sai no Koi", 0.95, [540.0, 800.0, 740.0, 822.0]),
        line("A Kind World", 0.93, [800.0, 800.0, 1000.0, 822.0]),
        line("Midashitai Giya-san", 0.89, [1060.0, 800.0, 1260.0, 822.0]),
        line("Livestream It!", 0.90, [30.0, 830.0, 210.0, 852.0]),
        line("Extras", 0.94, [330.0, 830.0, 430.0, 852.0]),
        line("to Midarenai", 0.90, [1080.0, 830.0, 1240.0, 852.0]),
        // Row 3 (2 more active + 3-line wrapped title)
        line("Onee-chan ga Kita", 0.93, [20.0, 1100.0, 220.0, 1122.0]),
        line("Uchi no Kaisha no", 0.91, [280.0, 1100.0, 480.0, 1122.0]),
        line("Chiisai Senpai", 0.90, [290.0, 1130.0, 470.0, 1152.0]),
        line("no Hanashi", 0.92, [310.0, 1160.0, 450.0, 1182.0]),
        // Finished section
        line("PROYECTOS TERMINADOS", 0.95, [20.0, 1260.0, 420.0, 1290.0]),
        line("The Classmate", 0.92, [20.0, 1520.0, 220.0, 1542.0]),
        line("They Were", 0.91, [280.0, 1520.0, 480.0, 1542.0]),
        line("I Hate!", 0.94, [70.0, 1550.0, 170.0, 1572.0]),
        line("Light Pink", 0.93, [310.0, 1550.0, 450.0, 1572.0]),
    ];

    let res = matcher.match_asset(&lines, false);
    assert_eq!(res.promo_confidence, Some(0.9));

    let matched: HashSet<LabelId> = res.works.iter().map(|w| w.label).collect();
    for (i, &title) in titles.iter().enumerate() {
        let id = LabelId((i + 1) as i64);
        assert!(
            matched.contains(&id),
            "expected reference work {title:?} (id={id:?}) to match"
        );
    }
    assert_eq!(matched.len(), 15, "all 14 works + group must stay distinct");
}

#[test]
fn apply_is_idempotent_respects_decisions_and_cleans_stale_suggestions() {
    let (_tmp, mut cat, ids) = setup_catalog(&[("promo.png", b"img-promo")]);
    let asset = ids["promo.png"];

    let w1 = cat.create_work("Academia Nebulosa").unwrap();
    let w2 = cat.create_work("Isla de Cristal").unwrap();
    let w3 = cat.create_work("El Jardín de Papel").unwrap();
    let w_manual = cat.create_work("Obra Manual").unwrap();
    cat.add_work_alias(w2, "Crystal Isle").unwrap();

    // User accepted w1, rejected w3, and AI suggested w_manual.
    cat.accept(asset, w1).unwrap();
    cat.reject(asset, w3).unwrap();
    cat.suggest(asset, w_manual, Source::Ai, 0.77).unwrap();

    cat.save_ocr_lines(
        asset,
        &[
            line("PROYECTOS ACTIVOS", 0.95, [10.0, 10.0, 250.0, 34.0]),
            line("Academia Nebulosa", 0.92, [10.0, 100.0, 200.0, 122.0]),
            line("Crystal Isle", 0.88, [10.0, 180.0, 200.0, 202.0]),
            line("El Jardín de Papel", 0.91, [10.0, 260.0, 200.0, 282.0]),
        ],
    )
    .unwrap();

    let matcher = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(asset, &matcher).unwrap();
    let after_first = cat.labels_of(asset).unwrap();

    // Running twice yields identical state.
    cat.apply_work_matches(asset, &matcher).unwrap();
    let after_second = cat.labels_of(asset).unwrap();
    assert_eq!(after_first, after_second);

    // Check decisions were respected.
    let find = |id: LabelId| after_first.iter().find(|a| a.label == id).cloned();
    assert_eq!(find(w1).unwrap().status, Status::Accepted);
    assert_eq!(find(w3).unwrap().status, Status::Rejected);
    assert_eq!(find(w2).unwrap().status, Status::Suggested);
    assert_eq!(find(w2).unwrap().source, Source::Ocr);
    assert_eq!(find(w_manual).unwrap().source, Source::Ai);

    let promo = after_first
        .iter()
        .find(|a| a.kind == LabelKind::Category && a.name == PROMO_CATEGORY)
        .expect("promo category suggested");
    assert_eq!(promo.status, Status::Suggested);
    assert_eq!(promo.source, Source::Ocr);
    assert!((promo.confidence - 0.9).abs() < 1e-6);

    // Removing the alias removes the stale OCR suggestion on the next run.
    cat.remove_work_alias(w2, "Crystal Isle").unwrap();
    let matcher2 = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(asset, &matcher2).unwrap();
    let after_remove = cat.labels_of(asset).unwrap();
    assert!(
        !after_remove.iter().any(|a| a.label == w2),
        "stale OCR suggestion for removed alias must be deleted"
    );
    // Promo now has 2 works + header -> confidence drops from 0.9 to 0.7.
    let promo2 = after_remove
        .iter()
        .find(|a| a.kind == LabelKind::Category && a.name == PROMO_CATEGORY)
        .unwrap();
    assert!((promo2.confidence - 0.7).abs() < 1e-6);

    // Screenshot category hides top_strip lines during apply_work_matches.
    let ss_cat = cat.label(LabelKind::Category, SCREENSHOT_CATEGORY).unwrap();
    cat.accept(asset, ss_cat).unwrap();
    let mut top = line("Academia Nebulosa", 0.95, [10.0, 2.0, 200.0, 18.0]);
    top.top_strip = true;
    cat.save_ocr_lines(asset, &[top]).unwrap();
    cat.apply_work_matches(asset, &matcher2).unwrap();
    let after_ss = cat.labels_of(asset).unwrap();
    assert!(
        !after_ss
            .iter()
            .any(|a| a.kind == LabelKind::Category && a.name == PROMO_CATEGORY),
        "promo suggestion must be removed when no longer matching"
    );
}

#[test]
fn filename_seeding_extracts_titles_and_ignores_negatives_and_existing() {
    assert_eq!(
        extract_work_title_from_stem("¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7"),
        Some("¡Quiero ser alabado por una Gal Gamer!".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("Onee-chan ga Kita - Cap. 12 - 03"),
        Some("Onee-chan ga Kita".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("My Series Chapter 5"),
        Some("My Series".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("My Series Ch. 04.5"),
        Some("My Series".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("Mi Serie - EPISODIO 10"),
        Some("Mi Serie".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("Another Show_Episode_02"),
        Some("Another Show".to_string())
    );
    assert_eq!(
        extract_work_title_from_stem("Short Tale Ep. 3"),
        Some("Short Tale".to_string())
    );

    // Negatives: camera names, screenshots, words starting with Cap/Ep, missing title.
    for neg in [
        "IMG_20240101_123456",
        "DSC00123",
        "Screenshot_2024-01-02",
        "IMG-20240101-WA0001",
        "Capitan America 2",
        "Epoch 2024",
        "Ch. 5",
        "pagina_0001",
    ] {
        assert_eq!(
            extract_work_title_from_stem(neg),
            None,
            "expected no proposal for {neg:?}"
        );
    }

    let (_tmp, cat, _) = setup_catalog(&[
        (
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7.jpg",
            b"a",
        ),
        (
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 24.00 - 1.jpg",
            b"b",
        ),
        ("Obra Existente - Cap. 1 - 1.png", b"c"),
        ("Alias Existente - Ch. 2 - 1.png", b"d"),
        ("IMG_20240101_123456.jpg", b"e"),
        ("Screenshot_2024-01-02.png", b"f"),
    ]);
    let existing = cat.create_work("Obra Existente").unwrap();
    cat.add_work_alias(existing, "Alias Existente").unwrap();

    let proposals = cat.propose_works_from_filenames().unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].title, "¡Quiero ser alabado por una Gal Gamer!");
    assert_eq!(proposals[0].files, 2);
}

#[test]
#[ignore = "release benchmark: cargo test -p catalog --release --test works bench_match_500_works_45000_assets -- --ignored --nocapture"]
fn bench_match_500_works_45000_assets() {
    let entries: Vec<WorkEntry> = (0..500)
        .map(|i| WorkEntry {
            label: LabelId((i + 1) as i64),
            name: format!("Chronicle of the Starlight Kingdom Volume {i:03}"),
            aliases: vec![format!("Starlight Tale {i:03}")],
        })
        .collect();
    let matcher = WorkMatcher::build(&entries);

    // Pre-generate 64 distinct synthetic 13-line pages and cycle across 45,000 assets.
    let templates: Vec<Vec<OcrLine>> = (0..64)
        .map(|t| {
            let mut lines = Vec::with_capacity(13);
            for l in 0..13 {
                let text = if l == 0 && t % 5 == 0 {
                    format!(
                        "Chronicle of the Starlight Kingdon Volume {:03}",
                        (t * 7) % 500
                    )
                } else if l == 6 && t % 3 == 0 {
                    format!("Starlight Tale {:03}", (t * 11) % 500)
                } else {
                    format!("Unrelated synthetic line number {t:02} row {l:02} text")
                };
                lines.push(line(
                    &text,
                    0.91,
                    [20.0, (l as f32) * 50.0, 260.0, (l as f32) * 50.0 + 22.0],
                ));
            }
            lines
        })
        .collect();

    let start = Instant::now();
    let mut total_matches = 0usize;
    for asset_idx in 0..45_000 {
        let res = matcher.match_asset(&templates[asset_idx % templates.len()], false);
        total_matches += res.works.len();
    }
    let elapsed = start.elapsed();
    println!(
        "bench_match_500_works_45000_assets: 500 works x 45,000 assets x 13 lines = {:.3} s ({total_matches} matches)",
        elapsed.as_secs_f64()
    );
    assert!(total_matches > 0);
}

#[test]
fn deleting_a_work_removes_only_that_work() {
    let (_tmp, mut cat, ids) = setup_catalog(&[("a.png", b"img-a"), ("b.png", b"img-b")]);
    let (a, b) = (ids["a.png"], ids["b.png"]);
    let doomed = cat.create_work("Isla de Cristal").unwrap();
    let kept = cat.create_work("Academia Nebulosa").unwrap();
    cat.add_work_alias(doomed, "Crystal Isle").unwrap();
    cat.add_work_alias(kept, "Nebula Academy").unwrap();
    let category = cat.label(LabelKind::Category, "Manga").unwrap();
    cat.accept(a, doomed).unwrap();
    cat.reject(b, doomed).unwrap();
    cat.accept(a, kept).unwrap();
    cat.accept(a, category).unwrap();

    // Something that is not a work is left alone, even when asked for by id.
    assert!(!cat.delete_work(category).unwrap());
    assert!(cat.delete_work(doomed).unwrap());
    assert!(!cat.delete_work(doomed).unwrap());

    let of = |cat: &Catalog, asset| {
        let mut labels: Vec<LabelId> = cat
            .labels_of(asset)
            .unwrap()
            .iter()
            .map(|l| l.label)
            .collect();
        labels.sort();
        labels
    };
    assert_eq!(of(&cat, a), [kept, category]);
    assert_eq!(of(&cat, b), []);
    let works = cat.list_works().unwrap();
    assert_eq!(works.len(), 1);
    assert_eq!((works[0].label, works[0].aliases), (kept, 1));
    assert!(cat.work_aliases(doomed).unwrap().is_empty());
}
