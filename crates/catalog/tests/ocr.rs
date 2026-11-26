// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, Filter, LabelKind, OCR_STAGE, OCR_VERSION, OcrLine, OcrRecognizer,
    SCREENSHOT_CATEGORY, Sort, Source, is_top_strip, ocr_search_key,
};
use fsguard::{LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    root_a: PathBuf,
    root_b: PathBuf,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let dir_a = tmp.path().join("lib_a");
    let dir_b = tmp.path().join("lib_b");

    write(&dir_a, "manga/page1.png", b"img-a1");
    write(&dir_a, "shots/phone.png", b"img-a2");
    write(&dir_b, "notes/doc.png", b"img-b1");
    write(&dir_b, "empty/blank.png", b"img-b2");

    let roots = [
        LibraryRoot::new(&dir_a).unwrap(),
        LibraryRoot::new(&dir_b).unwrap(),
    ];
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(&tmp.path().join("data").join("catalog.sqlite"), &roots).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&roots[0]).unwrap();
    cat.index(&roots[1]).unwrap();

    Setup {
        _tmp: tmp,
        root_a: roots[0].as_path().to_path_buf(),
        root_b: roots[1].as_path().to_path_buf(),
        cat,
    }
}

impl Setup {
    fn asset(&self, root: &Path, rel: &str) -> AssetId {
        let path = dunce::canonicalize(root.join(rel)).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }
}

fn line(text: &str, score: f32, recognizer: OcrRecognizer, top_strip: bool) -> OcrLine {
    OcrLine {
        text: text.to_string(),
        score,
        recognizer,
        rect: [10.0, 20.0, 200.0, 50.0],
        top_strip,
    }
}

#[test]
fn search_key_normalization_handles_accents_fullwidth_dakuten_hangul_and_spaces() {
    // Accents stripped (U+0300..=U+036F)
    assert_eq!(
        ocr_search_key("Crónicas del Relojero"),
        "cronicasdelrelojero"
    );
    assert_eq!(ocr_search_key("CAFÉ résumé naïve"), "caferesumenaive");
    assert_eq!(ocr_search_key("Cigüeña"), "ciguena");

    // Full-width ASCII normalized via NFKD + lowercase
    assert_eq!(ocr_search_key("ＡＢＣ １２３"), "abc123");

    // Japanese dakuten (U+3099) MUST stay intact so が does not match か
    let ga = ocr_search_key("がっこう");
    let ka = ocr_search_key("かっこう");
    assert_ne!(ga, ka);
    assert_eq!(ga, "がっこう");
    assert_eq!(ka, "かっこう");

    // Hangul stays composed and intact
    assert_eq!(ocr_search_key("바람 공주"), "바람공주");
    assert_eq!(ocr_search_key("한글 테스트"), "한글테스트");

    // Whitespace stripped so "paper crown" finds "ANDTHEPAPERCROWN"
    let stored = ocr_search_key("ANDTHEPAPERCROWN");
    let query = ocr_search_key("paper crown");
    assert!(stored.contains(&query));

    // Only whitespace or combining marks normalizes to empty
    assert!(ocr_search_key("   \t\n  ").is_empty());
    assert!(ocr_search_key("\u{0301}\u{0308}").is_empty());
}

#[test]
fn is_top_strip_checks_tall_aspect_and_top_five_percent() {
    // Phone screenshot 1080x2400 (aspect 2.22 >= 1.7), 5% height = 120 px
    assert!(is_top_strip(1080, 2400, [10.0, 10.0, 200.0, 80.0]));
    assert!(is_top_strip(1080, 2400, [10.0, 10.0, 200.0, 120.0]));
    assert!(!is_top_strip(1080, 2400, [10.0, 10.0, 200.0, 121.0]));

    // Boundary aspect: 1000x1700 (exact 1.7)
    assert!(is_top_strip(1000, 1700, [0.0, 0.0, 100.0, 85.0]));
    assert!(!is_top_strip(1000, 1699, [0.0, 0.0, 100.0, 80.0]));

    // Landscape or square image: never top_strip
    assert!(!is_top_strip(1920, 1080, [0.0, 0.0, 200.0, 20.0]));
    assert!(!is_top_strip(1000, 1000, [0.0, 0.0, 200.0, 20.0]));
    assert!(!is_top_strip(0, 1000, [0.0, 0.0, 200.0, 20.0]));
}

#[test]
fn save_ocr_lines_replaces_previous_marks_done_and_handles_empty() {
    let mut s = setup();
    let a1 = s.asset(&s.root_a, "manga/page1.png");
    let blank = s.asset(&s.root_b, "empty/blank.png");

    assert!(!s.cat.has_ocr_done(a1).unwrap());
    assert!(s.cat.ocr_lines(a1).unwrap().is_empty());

    // First save
    s.cat
        .save_ocr_lines(
            a1,
            &[
                line("Primera línea", 0.95, OcrRecognizer::V6, false),
                line("바람 공주", 0.88, OcrRecognizer::Korean, false),
            ],
        )
        .unwrap();
    assert!(s.cat.has_ocr_done(a1).unwrap());
    let stored = s.cat.ocr_lines(a1).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored[0].text, "Primera línea");
    assert_eq!(stored[0].recognizer, OcrRecognizer::V6);
    assert_eq!(stored[1].text, "바람 공주");
    assert_eq!(stored[1].recognizer, OcrRecognizer::Korean);

    // Second save replaces previous lines atomically
    s.cat
        .save_ocr_lines(a1, &[line("Solo una", 0.42, OcrRecognizer::V6, true)])
        .unwrap();
    let replaced = s.cat.ocr_lines(a1).unwrap();
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].text, "Solo una");
    assert!(replaced[0].top_strip);

    // Empty image is still marked done
    s.cat.save_ocr_lines(blank, &[]).unwrap();
    assert!(s.cat.has_ocr_done(blank).unwrap());
    assert!(s.cat.ocr_lines(blank).unwrap().is_empty());

    let pending = s.cat.pending(OCR_STAGE, OCR_VERSION, 100).unwrap();
    assert!(!pending.iter().any(|p| p.asset == a1 || p.asset == blank));
    assert_eq!(pending.len(), 2);
}

#[test]
fn search_filter_matches_path_or_ocr_and_never_matches_all_on_empty_key() {
    let mut s = setup();
    let a1 = s.asset(&s.root_a, "manga/page1.png");
    let b1 = s.asset(&s.root_b, "notes/doc.png");

    s.cat
        .save_ocr_lines(
            a1,
            &[
                line("ANDTHEPAPERCROWN", 0.91, OcrRecognizer::V6, false),
                line("がっこう", 0.96, OcrRecognizer::V6, false),
            ],
        )
        .unwrap();
    s.cat
        .save_ocr_lines(
            b1,
            &[
                line("Crónicas del Relojero", 0.84, OcrRecognizer::V6, false),
                line("한글 제목", 0.93, OcrRecognizer::Korean, false),
            ],
        )
        .unwrap();

    let search = |q: &str| {
        s.cat
            .entries(
                &Filter {
                    search: Some(q.to_string()),
                    ..Default::default()
                },
                Sort::PathAsc,
            )
            .unwrap()
            .into_iter()
            .map(|e| e.asset)
            .collect::<Vec<_>>()
    };

    // OCR match across lost spaces
    assert_eq!(search("paper crown"), vec![a1]);
    // Accent-insensitive OCR match
    assert_eq!(search("cronicas"), vec![b1]);
    // Hangul match without space
    assert_eq!(search("한글제목"), vec![b1]);
    // Dakuten preserved: か must not match が
    assert_eq!(search("がっこう"), vec![a1]);
    assert!(search("かっこう").is_empty());
    // Path match still works through `search`
    assert_eq!(
        search("blank.png"),
        vec![s.asset(&s.root_b, "empty/blank.png")]
    );
    // Query normalizing to empty (only combining mark) must NOT match everything via OCR
    assert!(search("\u{0301}").is_empty());
    // Whitespace-only query must NOT match everything via OCR
    assert!(search("   ").is_empty());
    // `path_contains` remains path-only
    let by_path = s
        .cat
        .entries(
            &Filter {
                path_contains: Some("paper".into()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap();
    assert!(by_path.is_empty());
}

#[test]
fn status_bar_top_strip_excluded_only_for_active_screenshot_category() {
    let mut s = setup();
    let phone = s.asset(&s.root_a, "shots/phone.png");
    let doc = s.asset(&s.root_b, "notes/doc.png");

    // Both have a top_strip line with "99% 5G" and a normal body line
    s.cat
        .save_ocr_lines(
            phone,
            &[
                line("14:32 99% 5G", 0.72, OcrRecognizer::V6, true),
                line("Mensaje importante", 0.95, OcrRecognizer::V6, false),
            ],
        )
        .unwrap();
    s.cat
        .save_ocr_lines(
            doc,
            &[
                line("14:32 99% 5G", 0.72, OcrRecognizer::V6, true),
                line("Otro contenido", 0.95, OcrRecognizer::V6, false),
            ],
        )
        .unwrap();

    let search = |cat: &Catalog, q: &str| {
        cat.entries(
            &Filter {
                search: Some(q.to_string()),
                ..Default::default()
            },
            Sort::PathAsc,
        )
        .unwrap()
        .into_iter()
        .map(|e| e.asset)
        .collect::<Vec<_>>()
    };

    // Before rules/category: both match the top_strip line
    let before = search(&s.cat, "99%");
    assert_eq!(before.len(), 2);

    // Suggest "Capturas de pantalla" on `phone` (non-rejected) -> excluded from search on `phone`,
    // still found on `doc` (which is not a screenshot), and body line on `phone` still searchable.
    let screenshot_cat = s
        .cat
        .label(LabelKind::Category, SCREENSHOT_CATEGORY)
        .unwrap();
    s.cat
        .suggest(phone, screenshot_cat, Source::Rules, 0.9)
        .unwrap();

    assert_eq!(search(&s.cat, "99%"), vec![doc]);
    // Queries of one or two characters take a different SQL path.
    assert_eq!(search(&s.cat, "5g"), vec![doc]);
    assert_eq!(search(&s.cat, "mensaje"), vec![phone]);

    // Accepting keeps it excluded
    s.cat.accept(phone, screenshot_cat).unwrap();
    assert_eq!(search(&s.cat, "99%"), vec![doc]);

    // Rejecting "Capturas de pantalla" immediately makes the top_strip line searchable again
    s.cat.reject(phone, screenshot_cat).unwrap();
    let after_reject = search(&s.cat, "99%");
    assert_eq!(after_reject.len(), 2);
    assert!(after_reject.contains(&phone));
    assert!(after_reject.contains(&doc));

    // Lines are never deleted from storage
    assert_eq!(s.cat.ocr_lines(phone).unwrap().len(), 2);
}

#[test]
fn pending_queue_restricted_to_roots_and_assets() {
    let mut s = setup();
    let a1 = s.asset(&s.root_a, "manga/page1.png");
    let a2 = s.asset(&s.root_a, "shots/phone.png");
    let b1 = s.asset(&s.root_b, "notes/doc.png");
    let b2 = s.asset(&s.root_b, "empty/blank.png");

    // Counts per root
    assert_eq!(
        s.cat
            .count_pending_in_root(OCR_STAGE, OCR_VERSION, &s.root_a)
            .unwrap(),
        2
    );
    assert_eq!(
        s.cat
            .count_pending_in_root(OCR_STAGE, OCR_VERSION, &s.root_b)
            .unwrap(),
        2
    );

    // Empty roots / empty assets
    assert!(
        s.cat
            .pending_in_roots(OCR_STAGE, OCR_VERSION, &[], 10)
            .unwrap()
            .is_empty()
    );
    assert!(
        s.cat
            .pending_in_assets(OCR_STAGE, OCR_VERSION, &[], 10)
            .unwrap()
            .is_empty()
    );

    // Restricted to root_a
    let only_a: Vec<AssetId> = s
        .cat
        .pending_in_roots(OCR_STAGE, OCR_VERSION, &[s.root_a.clone()], 10)
        .unwrap()
        .into_iter()
        .map(|p| p.asset)
        .collect();
    assert_eq!(only_a.len(), 2);
    assert!(only_a.contains(&a1) && only_a.contains(&a2));

    // Mark a1 done -> root_a pending drops to 1
    s.cat.save_ocr_lines(a1, &[]).unwrap();
    assert_eq!(
        s.cat
            .count_pending_in_root(OCR_STAGE, OCR_VERSION, &s.root_a)
            .unwrap(),
        1
    );

    // Restricted to chosen assets (with duplicates in input)
    let chosen = [a1, a2, b1, b1];
    assert_eq!(
        s.cat
            .count_pending_in_assets(OCR_STAGE, OCR_VERSION, &chosen)
            .unwrap(),
        2
    );
    let pending_assets: Vec<AssetId> = s
        .cat
        .pending_in_assets(OCR_STAGE, OCR_VERSION, &chosen, 10)
        .unwrap()
        .into_iter()
        .map(|p| p.asset)
        .collect();
    assert_eq!(pending_assets.len(), 2);
    assert!(pending_assets.contains(&a2) && pending_assets.contains(&b1));
    assert!(!pending_assets.contains(&b2));
}

/// The search index refers to lines by `id`. It must stay in step with the table after lines are
/// replaced and the database is compacted.
#[test]
fn search_index_matches_the_lines_after_replacing_them_and_compacting() {
    let mut s = setup();
    let first = s.asset(&s.root_a, "manga/page1.png");
    let second = s.asset(&s.root_b, "notes/doc.png");
    let v6 = |text| line(text, 0.9, OcrRecognizer::V6, false);
    s.cat
        .save_ocr_lines(first, &[v6("alpha one"), v6("bravo two")])
        .unwrap();
    s.cat
        .save_ocr_lines(second, &[v6("charlie three")])
        .unwrap();
    s.cat.save_ocr_lines(first, &[v6("delta four")]).unwrap();

    let db = rusqlite::Connection::open(s._tmp.path().join("data/catalog.sqlite")).unwrap();
    let id_is_declared: i64 = db
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('ocr_line')
             WHERE name = 'id' AND pk = 1 AND type = 'INTEGER'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(id_is_declared, 1);
    db.execute_batch(
        "VACUUM;
         INSERT INTO ocr_line_fts(ocr_line_fts, rank) VALUES ('integrity-check', 1);",
    )
    .unwrap();

    let found = |query: &str| {
        let filter = Filter {
            search: Some(query.into()),
            ..Default::default()
        };
        let entries = s.cat.entries(&filter, Sort::PathAsc).unwrap();
        entries.iter().map(|e| e.asset).collect::<Vec<_>>()
    };
    assert_eq!(found("delta"), [first]);
    assert_eq!(found("charlie"), [second]);
    assert_eq!(found("alpha"), []);
}
