// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AD_CATEGORY, AssetId, CHAPTER_PAGE_CATEGORY, CREDITS_CATEGORY, Catalog, LabelId, LabelKind,
    MANGA_COVER_CATEGORY, OcrLine, OcrRecognizer, PROMO_CATEGORY, ParsedWorkFilename,
    RECRUITMENT_CATEGORY, SCANLATION_PAGE_CATEGORIES, SCREENSHOT_CATEGORY, Source, Status,
    WorkEntry, WorkMatcher, extract_work_title_from_stem, parse_work_from_stem, work_match_key,
};
use fsguard::{LibraryRoot, ensure_writable};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
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
    let mut expected = [kept, category];
    expected.sort();
    assert_eq!(of(&cat, a), expected);
    assert_eq!(of(&cat, b), []);
    let works = cat.list_works().unwrap();
    assert_eq!(works.len(), 1);
    assert_eq!((works[0].label, works[0].aliases), (kept, 1));
    assert!(cat.work_aliases(doomed).unwrap().is_empty());
}

#[test]
fn credits_and_recruitment_detected_from_ocr_without_flagging_dialogue() {
    let matcher = WorkMatcher::build(&[]);

    // 1. Spanish credits page
    let credits_es = matcher.match_asset(
        &[
            line("CRÉDITOS", 0.96, [20.0, 20.0, 180.0, 44.0]),
            line("Traducción: Alex", 0.94, [20.0, 60.0, 200.0, 82.0]),
            line("Limpieza: Sam", 0.92, [20.0, 90.0, 200.0, 112.0]),
            line("Typesetting: Pat", 0.91, [20.0, 120.0, 210.0, 142.0]),
        ],
        false,
    );
    assert!(credits_es.credits_confidence.is_some());
    assert!(credits_es.recruitment_confidence.is_none());

    // 2. English credits with abbreviated roles
    let credits_en = matcher.match_asset(
        &[
            line("TL: Alice", 0.92, [20.0, 40.0, 120.0, 62.0]),
            line("CL: Bob", 0.91, [20.0, 70.0, 120.0, 92.0]),
            line("TS: Carol", 0.90, [20.0, 100.0, 120.0, 122.0]),
        ],
        false,
    );
    assert!(credits_en.credits_confidence.is_some());

    // 3. Spanish recruitment page
    let recruit_es = matcher.match_asset(
        &[
            line("RECLUTAMIENTO", 0.95, [20.0, 20.0, 240.0, 44.0]),
            line(
                "Buscamos traductores, cleaners y redibujadores",
                0.92,
                [20.0, 70.0, 320.0, 94.0],
            ),
            line(
                "Únete al equipo en Discord",
                0.90,
                [20.0, 110.0, 260.0, 134.0],
            ),
        ],
        false,
    );
    assert!(recruit_es.recruitment_confidence.is_some());

    // 4. English recruitment page
    let recruit_en = matcher.match_asset(
        &[
            line("WE ARE HIRING!", 0.95, [20.0, 20.0, 220.0, 44.0]),
            line(
                "Join our team: need redrawers and proofreaders",
                0.91,
                [20.0, 70.0, 340.0, 94.0],
            ),
        ],
        false,
    );
    assert!(recruit_en.recruitment_confidence.is_some());

    // 5. Negatives: ordinary manga speech-bubble dialogue must NOT be flagged.
    let dialogue_cases: &[&[&str]] = &[
        &[
            "¡Hola! ¿Viste al traductor ayer?",
            "Dijo que el editor no vino hoy.",
        ],
        &[
            "Necesito hacer limpieza en mi cuarto",
            "antes de terminar la edición del video.",
        ],
        &["¡Únete a nosotros y conquistaremos el mundo!"],
        &["Join us or perish! The dark lord awaits."],
        &["We are hiring a new chef for the tavern."],
        &["¿Qué estás haciendo aquí? ¡Vámonos rápido!"],
    ];
    for case in dialogue_cases {
        let lines: Vec<OcrLine> = case
            .iter()
            .enumerate()
            .map(|(i, t)| {
                line(
                    t,
                    0.95,
                    [20.0, (i as f32) * 40.0, 280.0, (i as f32) * 40.0 + 22.0],
                )
            })
            .collect();
        let res = matcher.match_asset(&lines, false);
        assert!(
            res.credits_confidence.is_none(),
            "ordinary dialogue must not trigger Créditos: {case:?}"
        );
        assert!(
            res.recruitment_confidence.is_none(),
            "ordinary dialogue must not trigger Reclutamiento: {case:?}"
        );
        assert!(
            res.promo_confidence.is_none(),
            "ordinary dialogue must not trigger Página promocional: {case:?}"
        );
    }

    // 6. Idempotence and user decisions in apply_work_matches
    let (_tmp, mut cat, ids) = setup_catalog(&[("credits.png", b"c1"), ("recruit.png", b"r1")]);
    let c_id = ids["credits.png"];
    let r_id = ids["recruit.png"];
    cat.save_ocr_lines(
        c_id,
        &[
            line("Translator: Bob", 0.92, [10.0, 20.0, 180.0, 42.0]),
            line("Typesetter: Alice", 0.91, [10.0, 50.0, 180.0, 72.0]),
        ],
    )
    .unwrap();
    cat.save_ocr_lines(
        r_id,
        &[line(
            "Buscamos staff para el fansub",
            0.93,
            [10.0, 20.0, 240.0, 42.0],
        )],
    )
    .unwrap();

    let m = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(c_id, &m).unwrap();
    cat.apply_work_matches(r_id, &m).unwrap();

    let c_lbl = cat
        .labels_of(c_id)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == LabelKind::Category && a.name == CREDITS_CATEGORY)
        .expect("Créditos suggested");
    assert_eq!(
        (c_lbl.source, c_lbl.status),
        (Source::Ocr, Status::Suggested)
    );

    let r_lbl = cat
        .labels_of(r_id)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == LabelKind::Category && a.name == RECRUITMENT_CATEGORY)
        .expect("Reclutamiento suggested");
    assert_eq!(
        (r_lbl.source, r_lbl.status),
        (Source::Ocr, Status::Suggested)
    );

    // Reject Créditos, accept Reclutamiento, re-apply -> decisions preserved!
    cat.reject(c_id, c_lbl.label).unwrap();
    cat.accept(r_id, r_lbl.label).unwrap();
    cat.apply_work_matches(c_id, &m).unwrap();
    cat.apply_work_matches(r_id, &m).unwrap();
    assert_eq!(
        cat.labels_of(c_id)
            .unwrap()
            .iter()
            .find(|a| a.label == c_lbl.label)
            .unwrap()
            .status,
        Status::Rejected
    );
    assert_eq!(
        cat.labels_of(r_id)
            .unwrap()
            .iter()
            .find(|a| a.label == r_lbl.label)
            .unwrap()
            .status,
        Status::Accepted
    );
}

#[test]
fn parse_work_from_stem_extracts_title_chapter_and_page() {
    assert_eq!(
        parse_work_from_stem("¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7"),
        Some(ParsedWorkFilename {
            title: "¡Quiero ser alabado por una Gal Gamer!".into(),
            chapter: "23.00".into(),
            page: Some(7),
        })
    );
    assert_eq!(
        parse_work_from_stem("Onee-chan ga Kita - Cap. 12 - 03"),
        Some(ParsedWorkFilename {
            title: "Onee-chan ga Kita".into(),
            chapter: "12".into(),
            page: Some(3),
        })
    );
    assert_eq!(
        parse_work_from_stem("My Series Chapter 5"),
        Some(ParsedWorkFilename {
            title: "My Series".into(),
            chapter: "5".into(),
            page: None,
        })
    );
    assert_eq!(
        parse_work_from_stem("My Series Ch. 04.5 - p12"),
        Some(ParsedWorkFilename {
            title: "My Series".into(),
            chapter: "04.5".into(),
            page: Some(12),
        })
    );
}

#[test]
fn work_ownership_belongs_to_is_separate_from_mentions_idempotent_ordered_and_respects_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let lib_a = tmp.path().join("lib_a");
    let lib_b = tmp.path().join("lib_b");
    fs::create_dir_all(&lib_a).unwrap();
    fs::create_dir_all(&lib_b).unwrap();

    let files_a = [
        (
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7.jpg",
            b"p23-7" as &[u8],
        ),
        (
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 2.00 - 10.jpg",
            b"p2-10",
        ),
        (
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 2.00 - 2.jpg",
            b"p2-2",
        ),
        ("Gal Gamer Alias - Capítulo 10.00 - 1.jpg", b"p10-1"),
        ("Obra Desconocida - Capítulo 1 - 1.jpg", b"unknown"),
        ("solo_ocr.jpg", b"ocr-only"),
    ];
    for (name, bytes) in files_a {
        fs::write(lib_a.join(name), bytes).unwrap();
    }
    fs::write(
        lib_b.join("¡Quiero ser alabado por una Gal Gamer! - Capítulo 99 - 1.jpg"),
        b"p99-1",
    )
    .unwrap();

    let roots_guard = [
        LibraryRoot::new(&lib_a).unwrap(),
        LibraryRoot::new(&lib_b).unwrap(),
    ];
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), &roots_guard).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&roots_guard[0]).unwrap();
    cat.index(&roots_guard[1]).unwrap();
    let root_a = &roots_guard[0];

    let work_a = cat
        .create_work("¡Quiero ser alabado por una Gal Gamer!")
        .unwrap();
    cat.add_work_alias(work_a, "Gal Gamer Alias").unwrap();
    let work_b = cat.create_work("Academia Nebulosa").unwrap();
    let work_c = cat.create_work("Isla de Cristal").unwrap();

    // Promo page belongs to work A while mentioning A, B, and C via OCR.
    let promo_path = dunce::canonicalize(
        lib_a.join("¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7.jpg"),
    )
    .unwrap();
    let promo_asset = cat.asset_of(&promo_path).unwrap().unwrap();
    cat.save_ocr_lines(
        promo_asset,
        &[
            line("PROYECTOS ACTIVOS", 0.95, [10.0, 10.0, 220.0, 32.0]),
            line(
                "¡Quiero ser alabado por una Gal Gamer!",
                0.93,
                [10.0, 60.0, 300.0, 82.0],
            ),
            line("Academia Nebulosa", 0.92, [10.0, 100.0, 200.0, 122.0]),
            line("Isla de Cristal", 0.91, [10.0, 140.0, 200.0, 162.0]),
        ],
    )
    .unwrap();

    // OCR-only image mentions work A, must NEVER get belongs_to from OCR.
    let ocr_only_asset = cat
        .asset_of(&dunce::canonicalize(lib_a.join("solo_ocr.jpg")).unwrap())
        .unwrap()
        .unwrap();
    cat.save_ocr_lines(
        ocr_only_asset,
        &[line(
            "¡Quiero ser alabado por una Gal Gamer!",
            0.95,
            [10.0, 20.0, 300.0, 42.0],
        )],
    )
    .unwrap();

    let matcher = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(promo_asset, &matcher).unwrap();
    cat.apply_work_matches(ocr_only_asset, &matcher).unwrap();
    cat.apply_filename_works(None).unwrap();

    // Must never auto-create "Obra Desconocida".
    assert_eq!(cat.list_works().unwrap().len(), 3);
    // OCR-only image has no belongs_to.
    assert!(cat.asset_work(ocr_only_asset).unwrap().is_none());

    // Promo page belongs to work A (chapter 23.00, page 7) AND mentions A, B, C!
    let aw = cat.asset_work(promo_asset).unwrap().unwrap();
    assert_eq!(aw.work, work_a);
    assert_eq!(aw.chapter, "23.00");
    assert_eq!(aw.page, Some(7));
    assert_eq!(aw.status, Status::Suggested);
    let mentioned_works: HashSet<LabelId> = cat
        .labels_of(promo_asset)
        .unwrap()
        .into_iter()
        .filter(|a| a.kind == LabelKind::Entity)
        .map(|a| a.label)
        .collect();
    assert!(mentioned_works.contains(&work_a));
    assert!(mentioned_works.contains(&work_b));
    assert!(mentioned_works.contains(&work_c));

    // Ordering by chapter then page (only root_a): 2.00 p2 -> 2.00 p10 -> 10.00 p1 -> 23.00 p7
    let roots_a = vec![root_a.as_path().to_path_buf()];
    let ordered = cat
        .entries_belonging_to_work(work_a, Some(&roots_a))
        .unwrap();
    let ordered_names: Vec<String> = ordered
        .iter()
        .map(|e| e.path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        ordered_names,
        vec![
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 2.00 - 2.jpg",
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 2.00 - 10.jpg",
            "Gal Gamer Alias - Capítulo 10.00 - 1.jpg",
            "¡Quiero ser alabado por una Gal Gamer! - Capítulo 23.00 - 7.jpg",
        ]
    );

    // Counts separate belongs_to from mentions and hide removed folder root_b.
    let works_a = cat.list_works_in_roots(&roots_a).unwrap();
    let wa = works_a.iter().find(|w| w.label == work_a).unwrap();
    assert_eq!(wa.belongs_to, 4);
    assert_eq!(wa.belongs_to_suggested, 4);
    assert_eq!(wa.images, 2); // promo_asset + ocr_only_asset

    // Reject one, accept all remaining in root_a -> rejected stays rejected, root_b stays suggested.
    let p10_asset = cat
        .asset_of(
            &dunce::canonicalize(lib_a.join("Gal Gamer Alias - Capítulo 10.00 - 1.jpg")).unwrap(),
        )
        .unwrap()
        .unwrap();
    cat.reject_asset_work(p10_asset).unwrap();
    let confirmed = cat
        .accept_all_suggested_work_ownership(work_a, Some(&roots_a))
        .unwrap();
    assert_eq!(confirmed, 3);
    assert_eq!(
        cat.asset_work(p10_asset).unwrap().unwrap().status,
        Status::Rejected
    );
    assert_eq!(
        cat.asset_work(promo_asset).unwrap().unwrap().status,
        Status::Accepted
    );

    // Re-running apply_filename_works is idempotent and never overwrites accepted/rejected.
    cat.apply_filename_works(None).unwrap();
    assert_eq!(
        cat.asset_work(p10_asset).unwrap().unwrap().status,
        Status::Rejected
    );
    assert_eq!(
        cat.asset_work(promo_asset).unwrap().unwrap().status,
        Status::Accepted
    );

    // Deleting work_a removes its asset_work rows.
    assert!(cat.delete_work(work_a).unwrap());
    assert!(cat.asset_work(promo_asset).unwrap().is_none());
}

#[test]
fn rejecting_one_work_does_not_block_a_different_one_and_never_comes_back() {
    let (_tmp, mut cat, ids) = setup_catalog(&[("Mi Obra - Capítulo 3 - 4.jpg", b"page")]);
    let asset = ids["Mi Obra - Capítulo 3 - 4.jpg"];
    let wrong = cat.create_work("Otra Cosa").unwrap();
    cat.add_work_alias(wrong, "Mi Obra").unwrap();
    cat.apply_filename_works(None).unwrap();
    assert_eq!(cat.asset_work(asset).unwrap().unwrap().work, wrong);
    assert!(cat.reject_asset_work(asset).unwrap());

    // The user deletes the wrong work's alias by recreating the right one.
    cat.remove_work_alias(wrong, "Mi Obra").unwrap();
    let right = cat.create_work("Mi Obra").unwrap();
    cat.apply_filename_works(None).unwrap();
    let now = cat.asset_work(asset).unwrap().unwrap();
    assert_eq!((now.work, now.status), (right, Status::Suggested));

    // Pointing the name back at the rejected work must not suggest it again.
    cat.delete_work(right).unwrap();
    cat.add_work_alias(wrong, "Mi Obra").unwrap();
    cat.apply_filename_works(None).unwrap();
    let after = cat.asset_work(asset).unwrap().unwrap();
    assert_eq!((after.work, after.status), (wrong, Status::Rejected));
}

#[test]
fn a_second_copy_with_a_plain_name_keeps_the_ownership_suggestion() {
    let (_tmp, mut cat, ids) = setup_catalog(&[
        ("a/scan001.jpg", b"same"),
        ("b/Mi Obra - Capítulo 3 - 4.jpg", b"same"),
    ]);
    let asset = ids["a/scan001.jpg"];
    let work = cat.create_work("Mi Obra").unwrap();
    cat.apply_filename_works(None).unwrap();
    let aw = cat
        .asset_work(asset)
        .unwrap()
        .expect("suggested from the named copy");
    assert_eq!(aw.work, work);
}

#[test]
fn match_asset_detects_ad_pages_and_rejects_single_word_mentions() {
    let matcher = WorkMatcher::build(&[]);

    // 1. Synthetic Spanish ad page
    let ad_es = matcher.match_asset(
        &[
            line("¡Apóyanos en Patreon!", 0.95, [20.0, 20.0, 240.0, 44.0]),
            line(
                "Visita nuestra web para capítulos adelantados",
                0.93,
                [20.0, 60.0, 360.0, 84.0],
            ),
            line("Síguenos en Discord", 0.91, [20.0, 100.0, 200.0, 124.0]),
        ],
        false,
    );
    assert!(ad_es.ad_confidence.is_some(), "Spanish ad must be detected");
    assert!(ad_es.recruitment_confidence.is_none());

    // 2. Synthetic English ad page
    let ad_en = matcher.match_asset(
        &[
            line("Support us on Patreon!", 0.96, [20.0, 20.0, 240.0, 44.0]),
            line(
                "Visit our website to read ahead",
                0.94,
                [20.0, 60.0, 320.0, 84.0],
            ),
            line("Join our Discord server", 0.92, [20.0, 100.0, 240.0, 124.0]),
        ],
        false,
    );
    assert!(ad_en.ad_confidence.is_some(), "English ad must be detected");

    // 3. Negative case: a dialogue that mentions Discord once is NOT an ad
    let discord_dialogue = matcher.match_asset(
        &[
            line(
                "Oye, ¿viste lo que dijeron en Discord ayer?",
                0.95,
                [20.0, 20.0, 320.0, 44.0],
            ),
            line(
                "No, estuve ocupado todo el día.",
                0.94,
                [20.0, 60.0, 260.0, 84.0],
            ),
        ],
        false,
    );
    assert!(
        discord_dialogue.ad_confidence.is_none(),
        "single mention of Discord in dialogue must not trigger ad"
    );

    // Negative case: dialogue with 'síguenos' alone without supporting platforms
    let follow_dialogue = matcher.match_asset(
        &[line(
            "¡Síguenos rápido o nos atraparán!",
            0.95,
            [20.0, 20.0, 300.0, 44.0],
        )],
        false,
    );
    assert!(
        follow_dialogue.ad_confidence.is_none(),
        "'síguenos' in ordinary dialogue must not trigger ad"
    );

    // 4. Recruitment takes precedence over ad: if recruitment matches, ad is not suggested
    let recruit_and_discord = matcher.match_asset(
        &[
            line(
                "¡Buscamos traductores y cleaners!",
                0.95,
                [20.0, 20.0, 320.0, 44.0],
            ),
            line("Únete a nuestro Discord", 0.92, [20.0, 60.0, 240.0, 84.0]),
        ],
        false,
    );
    assert!(
        recruit_and_discord.recruitment_confidence.is_some(),
        "recruitment must be detected"
    );
    assert!(
        recruit_and_discord.ad_confidence.is_none(),
        "if recruitment matches, ad must not be suggested"
    );
}

#[test]
fn chapter_page_suggested_from_filename_and_negatives() {
    let (_tmp, mut cat, ids) = setup_catalog(&[
        ("One Piece - Cap 100 - 05.png", b"c1"),
        ("Ilustracion_01.png", b"c2"),
        ("Naruto - Chapter 50 - 01.png", b"c3"),
    ]);
    let ch_id = ids["One Piece - Cap 100 - 05.png"];
    let plain_id = ids["Ilustracion_01.png"];
    let credits_ch_id = ids["Naruto - Chapter 50 - 01.png"];

    // 1. Positive case: filename with chapter marker suggests Página de capítulo
    let changed = cat
        .suggest_chapter_page_from_path(ch_id, Path::new("One Piece - Cap 100 - 05.png"))
        .unwrap();
    assert!(changed, "must suggest chapter page");
    let labels = cat.labels_of(ch_id).unwrap();
    let ch_label = labels
        .iter()
        .find(|a| a.kind == LabelKind::Category && a.name == CHAPTER_PAGE_CATEGORY)
        .expect("Página de capítulo suggested");
    assert_eq!(
        (ch_label.source, ch_label.status),
        (Source::Rules, Status::Suggested)
    );

    // 2. Negative case: filename without chapter marker does NOT suggest Página de capítulo
    let changed_plain = cat
        .suggest_chapter_page_from_path(plain_id, Path::new("Ilustracion_01.png"))
        .unwrap();
    assert!(
        !changed_plain,
        "plain filename must not suggest chapter page"
    );
    let labels_plain = cat.labels_of(plain_id).unwrap();
    assert!(
        !labels_plain.iter().any(|a| a.name == CHAPTER_PAGE_CATEGORY),
        "Página de capítulo must not exist for plain filename"
    );

    // 3. Negative case: filename has chapter marker, but page has credits OCR
    cat.save_ocr_lines(
        credits_ch_id,
        &[
            line("Translator: Bob", 0.95, [10.0, 20.0, 180.0, 42.0]),
            line("Typesetter: Alice", 0.94, [10.0, 50.0, 180.0, 72.0]),
        ],
    )
    .unwrap();
    let m = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(credits_ch_id, &m).unwrap();

    // Now try to suggest chapter page from path: must be rejected because page is credits
    let changed_cred = cat
        .suggest_chapter_page_from_path(credits_ch_id, Path::new("Naruto - Chapter 50 - 01.png"))
        .unwrap();
    assert!(
        !changed_cred,
        "must not suggest chapter page when page is credits"
    );
    let labels_cred = cat.labels_of(credits_ch_id).unwrap();
    assert!(
        labels_cred
            .iter()
            .any(|a| a.name == CREDITS_CATEGORY && a.status == Status::Suggested),
        "Créditos must be suggested"
    );
    assert!(
        !labels_cred.iter().any(|a| a.name == CHAPTER_PAGE_CATEGORY),
        "Página de capítulo must not be suggested when page is credits"
    );
}

#[test]
fn scanlation_page_type_priority_and_single_suggestion_invariant() {
    let (_tmp, mut cat, ids) = setup_catalog(&[
        ("page_ad_credits.png", b"p1"),
        ("page_credits_chapter.png", b"p2"),
        ("page_cover_chapter.png", b"p3"),
    ]);
    let p1 = ids["page_ad_credits.png"];
    let p2 = ids["page_credits_chapter.png"];
    let p3 = ids["page_cover_chapter.png"];

    // 1. Two candidates: Ad vs Credits. Ad has higher priority (Ad > Credits).
    cat.save_ocr_lines(
        p1,
        &[
            line("Translator: Alex", 0.94, [10.0, 20.0, 180.0, 40.0]),
            line("Cleaner: Sam", 0.93, [10.0, 50.0, 180.0, 70.0]),
            line("Typesetter: Pat", 0.92, [10.0, 80.0, 180.0, 100.0]),
            line("¡Apóyanos en Patreon!", 0.95, [10.0, 120.0, 240.0, 140.0]),
            line("Visita nuestra web", 0.92, [10.0, 150.0, 200.0, 170.0]),
        ],
    )
    .unwrap();
    let m = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(p1, &m).unwrap();

    let p1_labels = cat.labels_of(p1).unwrap();
    let p1_suggested_cats: Vec<_> = p1_labels
        .iter()
        .filter(|a| {
            a.kind == LabelKind::Category
                && a.status == Status::Suggested
                && SCANLATION_PAGE_CATEGORIES.contains(&a.name.as_str())
        })
        .collect();
    assert_eq!(
        p1_suggested_cats.len(),
        1,
        "exactly one page type suggestion allowed"
    );
    assert_eq!(
        p1_suggested_cats[0].name, AD_CATEGORY,
        "Anuncio wins over Créditos"
    );

    // 2. Two candidates: Credits vs Chapter page. Credits has higher priority (Credits > Chapter page).
    // First, suggest chapter page via rules.
    cat.suggest_chapter_page_from_path(p2, Path::new("Manga - Cap 1 - 02.png"))
        .unwrap();
    assert!(
        cat.labels_of(p2)
            .unwrap()
            .iter()
            .any(|a| a.name == CHAPTER_PAGE_CATEGORY && a.status == Status::Suggested),
        "chapter page initially suggested"
    );

    // Now OCR detects credits:
    cat.save_ocr_lines(
        p2,
        &[
            line("TL: Bob", 0.95, [10.0, 20.0, 120.0, 40.0]),
            line("CL: Alice", 0.94, [10.0, 50.0, 120.0, 70.0]),
            line("TS: Charlie", 0.93, [10.0, 80.0, 120.0, 100.0]),
        ],
    )
    .unwrap();
    cat.apply_work_matches(p2, &m).unwrap();

    let p2_labels = cat.labels_of(p2).unwrap();
    let p2_suggested_cats: Vec<_> = p2_labels
        .iter()
        .filter(|a| {
            a.kind == LabelKind::Category
                && a.status == Status::Suggested
                && SCANLATION_PAGE_CATEGORIES.contains(&a.name.as_str())
        })
        .collect();
    assert_eq!(
        p2_suggested_cats.len(),
        1,
        "exactly one page type suggestion allowed"
    );
    assert_eq!(
        p2_suggested_cats[0].name, CREDITS_CATEGORY,
        "Créditos wins over Página de capítulo"
    );

    // 3. Two candidates: Cover (Portada de manga) vs Chapter page. Portada > Chapter page.
    cat.suggest_chapter_page_from_path(p3, Path::new("Series - Cap 10 - 01.png"))
        .unwrap();
    let cover_label = cat
        .label(LabelKind::Category, MANGA_COVER_CATEGORY)
        .unwrap();
    cat.suggest(p3, cover_label, Source::Ai, 0.90).unwrap();
    cat.reconcile_page_types(p3).unwrap();

    let p3_labels = cat.labels_of(p3).unwrap();
    let p3_suggested_cats: Vec<_> = p3_labels
        .iter()
        .filter(|a| {
            a.kind == LabelKind::Category
                && a.status == Status::Suggested
                && SCANLATION_PAGE_CATEGORIES.contains(&a.name.as_str())
        })
        .collect();
    assert_eq!(
        p3_suggested_cats.len(),
        1,
        "exactly one page type suggestion allowed"
    );
    assert_eq!(
        p3_suggested_cats[0].name, MANGA_COVER_CATEGORY,
        "Portada wins over Página de capítulo"
    );

    // 4. User decisions are preserved and never overwritten:
    // Accept chapter page on p3:
    let ch_lbl = cat
        .label(LabelKind::Category, CHAPTER_PAGE_CATEGORY)
        .unwrap();
    cat.accept(p3, ch_lbl).unwrap();
    // Re-running OCR or reconcile must not overwrite accepted decision:
    cat.apply_work_matches(p3, &m).unwrap();
    let p3_after = cat.labels_of(p3).unwrap();
    let ch_after = p3_after.iter().find(|a| a.label == ch_lbl).unwrap();
    assert_eq!(
        ch_after.status,
        Status::Accepted,
        "user acceptance must be preserved"
    );

    // Reject on p2:
    let cred_lbl = cat.label(LabelKind::Category, CREDITS_CATEGORY).unwrap();
    cat.reject(p2, cred_lbl).unwrap();
    cat.apply_work_matches(p2, &m).unwrap();
    let p2_after = cat.labels_of(p2).unwrap();
    let cred_after = p2_after.iter().find(|a| a.label == cred_lbl).unwrap();
    assert_eq!(
        cred_after.status,
        Status::Rejected,
        "user rejection must be preserved"
    );
}

#[test]
fn rejecting_higher_priority_page_type_allows_lower_priority_suggestion() {
    let (_tmp, mut cat, ids) = setup_catalog(&[
        ("Manga - Cap 12 - 03.png", b"ch-cred"),
        ("ad_and_credits.png", b"ad-cred"),
    ]);
    let ch_cred = ids["Manga - Cap 12 - 03.png"];
    let ad_cred = ids["ad_and_credits.png"];

    // 1. User rejects Créditos on a chapter page -> Página de capítulo can still be suggested
    //    and Créditos stays Rejected.
    let cred_lbl = cat.label(LabelKind::Category, CREDITS_CATEGORY).unwrap();
    cat.reject(ch_cred, cred_lbl).unwrap();
    let suggested = cat
        .suggest_chapter_page_from_path(ch_cred, Path::new("Manga - Cap 12 - 03.png"))
        .unwrap();
    assert!(
        suggested,
        "rejecting Créditos must not block Página de capítulo"
    );
    let labels = cat.labels_of(ch_cred).unwrap();
    assert!(
        labels
            .iter()
            .any(|a| a.name == CREDITS_CATEGORY && a.status == Status::Rejected),
        "Créditos must remain Rejected"
    );
    assert!(
        labels
            .iter()
            .any(|a| a.name == CHAPTER_PAGE_CATEGORY && a.status == Status::Suggested),
        "Página de capítulo must be Suggested"
    );

    // 2. OCR detects both Anuncio and Créditos. User rejects Anuncio -> re-running OCR
    //    must keep Anuncio Rejected and fall through to suggest Créditos.
    cat.save_ocr_lines(
        ad_cred,
        &[
            line("Translator: Alex", 0.94, [10.0, 20.0, 180.0, 40.0]),
            line("Cleaner: Sam", 0.93, [10.0, 50.0, 180.0, 70.0]),
            line("Typesetter: Pat", 0.92, [10.0, 80.0, 180.0, 100.0]),
            line("¡Apóyanos en Patreon!", 0.95, [10.0, 120.0, 240.0, 140.0]),
        ],
    )
    .unwrap();
    let m = cat.load_work_matcher().unwrap();
    cat.apply_work_matches(ad_cred, &m).unwrap();
    let ad_lbl = cat.label(LabelKind::Category, AD_CATEGORY).unwrap();
    cat.reject(ad_cred, ad_lbl).unwrap();

    cat.apply_work_matches(ad_cred, &m).unwrap();
    let after = cat.labels_of(ad_cred).unwrap();
    assert!(
        after
            .iter()
            .any(|a| a.name == AD_CATEGORY && a.status == Status::Rejected),
        "Anuncio must remain Rejected"
    );
    assert!(
        after
            .iter()
            .any(|a| a.name == CREDITS_CATEGORY && a.status == Status::Suggested),
        "Créditos must be suggested when Anuncio is rejected"
    );
}

#[test]
fn deciding_unrelated_label_does_not_reconcile_or_touch_page_types() {
    let (_tmp, cat, ids) = setup_catalog(&[("page.png", b"p")]);
    let id = ids["page.png"];

    let cred = cat.label(LabelKind::Category, CREDITS_CATEGORY).unwrap();
    let ch = cat
        .label(LabelKind::Category, CHAPTER_PAGE_CATEGORY)
        .unwrap();
    let tag = cat.label(LabelKind::Tag, "WhatsApp").unwrap();
    let other_cat = cat
        .label(LabelKind::Category, "Capturas de pantalla")
        .unwrap();

    // Seed two suggested page types directly via suggest() (bypassing reconcile)
    // plus an unrelated accepted category and tag.
    cat.suggest(id, cred, Source::Ocr, 0.85).unwrap();
    cat.suggest(id, ch, Source::Rules, 0.85).unwrap();
    cat.accept(id, tag).unwrap();
    cat.accept(id, other_cat).unwrap();

    // Deciding on unrelated labels must NOT mutate page-type suggestions.
    let labels = cat.labels_of(id).unwrap();
    assert!(
        labels
            .iter()
            .any(|a| a.label == cred && a.status == Status::Suggested),
        "Créditos suggestion must not be touched by unrelated decide()"
    );
    assert!(
        labels
            .iter()
            .any(|a| a.label == ch && a.status == Status::Suggested),
        "Página de capítulo suggestion must not be touched by unrelated decide()"
    );
    assert!(
        labels
            .iter()
            .any(|a| a.label == tag && a.status == Status::Accepted)
    );
    assert!(
        labels
            .iter()
            .any(|a| a.label == other_cat && a.status == Status::Accepted)
    );
}

#[test]
fn ad_detection_avoids_dialogue_false_positives_and_discord_in_credits() {
    let matcher = WorkMatcher::build(&[]);

    let dialogue_cases: &[(&str, &[&str])] = &[
        ("Spanish donate", &["Quiero donar sangre mañana."]),
        (
            "English donate",
            &["We should donate these clothes to the shrine."],
        ),
        ("Spanish donation", &["Hice una donación al templo."]),
        ("English our app", &["Our app crashed right before launch!"]),
        (
            "Spanish nuestra web",
            &["Alguien hackeó nuestra web anoche."],
        ),
        (
            "English buy me a coffee",
            &["Hey, buy me a coffee on your way back!"],
        ),
        (
            "English visit us at",
            &["Come visit us at the school festival tomorrow!"],
        ),
        (
            "Spanish visitanos en",
            &["Visítanos en la cafetería cuando salgas."],
        ),
        (
            "Spanish discord server",
            &["Entra al servidor de Discord esta noche para la raid."],
        ),
        (
            "English discord server",
            &["Check the Discord server before the boss fight!"],
        ),
        (
            "English follow us VIP",
            &["Follow us to the VIP room, please."],
        ),
        (
            "Spanish apoya + redes",
            &["Ella siempre me apoya cuando subo dibujos a las redes."],
        ),
    ];

    for (case_name, texts) in dialogue_cases {
        let lines: Vec<OcrLine> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                line(
                    t,
                    0.95,
                    [10.0, (i as f32) * 30.0, 300.0, (i as f32) * 30.0 + 22.0],
                )
            })
            .collect();
        let res = matcher.match_asset(&lines, false);
        assert!(
            res.ad_confidence.is_none(),
            "dialogue case {case_name:?} must not trigger Anuncio, got {:?}",
            res.ad_confidence
        );
    }

    // Credits page listing a Discord server/handle must stay Créditos, not become Anuncio.
    let credits_with_discord = matcher.match_asset(
        &[
            line("Créditos", 0.96, [10.0, 10.0, 150.0, 32.0]),
            line("Traducción: Ana", 0.95, [10.0, 40.0, 200.0, 62.0]),
            line("Limpieza: Luis", 0.94, [10.0, 70.0, 200.0, 92.0]),
            line("Typesetter: Eva", 0.93, [10.0, 100.0, 200.0, 122.0]),
            line(
                "Servidor de Discord: MiFansub",
                0.92,
                [10.0, 130.0, 260.0, 152.0],
            ),
        ],
        false,
    );
    assert!(
        credits_with_discord.credits_confidence.is_some(),
        "credits page must be detected as Créditos"
    );
    assert!(
        credits_with_discord.ad_confidence.is_none(),
        "Discord listed in credits must not turn credits page into Anuncio"
    );
}
