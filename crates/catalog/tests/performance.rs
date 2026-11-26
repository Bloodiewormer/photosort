// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, Control, Counts, Entry, Filter, LabelCount, LabelKind, Sort, Source,
};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    db: DestPath,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

fn set_mtime(path: &Path, secs: u64) {
    let f = fs::File::options().write(true).open(path).unwrap();
    let times = FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(secs));
    f.set_times(times).unwrap();
}

fn setup_rich() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");

    let p1 = write(&library, "manga/a_page.jpg", b"content-one");
    let p2 = write(&library, "manga/b_dup.jpg", b"content-one"); // exact duplicate of p1
    let p3 = write(&library, "memes/c_funny.png", b"content-two-longer");
    let p4 = write(&library, "shots/d_chat.png", b"content-one-tie"); // same size as p1? len=15 vs 11
    let p5 = write(&library, "aaa/e_tie.jpg", b"content-11!"); // 11 bytes, ties p1/p2
    let p6 = write(&library, "gone/f_removed.jpg", b"will-be-deleted");

    set_mtime(&p1, 1_700_000_200);
    set_mtime(&p2, 1_700_000_200);
    set_mtime(&p3, 1_700_000_100);
    set_mtime(&p4, 1_700_000_300);
    set_mtime(&p5, 1_700_000_200);
    set_mtime(&p6, 1_700_000_150);

    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();

    Setup {
        _tmp: tmp,
        library,
        root,
        db,
        cat,
    }
}

impl Setup {
    fn asset(&self, rel: &str) -> AssetId {
        let path = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }

    fn rel_entries(&self, entries: &[Entry]) -> Vec<String> {
        let base = dunce::canonicalize(&self.library).unwrap();
        entries
            .iter()
            .map(|e| {
                e.path
                    .strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }
}

#[test]
fn explain_query_plans_for_all_interactive_queries() {
    let s = setup_rich();
    let l1 = s.cat.label(LabelKind::Category, "Manga").unwrap();
    let l2 = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let l3 = s.cat.label(LabelKind::Tag, "captura").unwrap();
    let l4 = s.cat.label(LabelKind::Tag, "descartar").unwrap();

    let a1 = s.asset("manga/a_page.jpg");
    let a3 = s.asset("memes/c_funny.png");
    s.cat.accept(a1, l1).unwrap();
    s.cat.suggest(a1, l2, Source::Ai, 0.45).unwrap();
    s.cat.suggest(a3, l3, Source::Rules, 0.50).unwrap();
    s.cat.reject(a3, l4).unwrap();

    let plans: Vec<(&str, Vec<String>)> = vec![
        (
            "entries(empty, PathAsc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::PathAsc)
                .unwrap(),
        ),
        (
            "entries(empty, PathDesc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::PathDesc)
                .unwrap(),
        ),
        (
            "entries(empty, SizeAsc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::SizeAsc)
                .unwrap(),
        ),
        (
            "entries(empty, SizeDesc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::SizeDesc)
                .unwrap(),
        ),
        (
            "entries(empty, ModifiedAsc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::ModifiedAsc)
                .unwrap(),
        ),
        (
            "entries(empty, ModifiedDesc)",
            s.cat
                .explain_entries(&Filter::default(), Sort::ModifiedDesc)
                .unwrap(),
        ),
        (
            "entries(1 require)",
            s.cat
                .explain_entries(
                    &Filter {
                        require: vec![l1],
                        ..Default::default()
                    },
                    Sort::PathAsc,
                )
                .unwrap(),
        ),
        (
            "entries(3 req+any+excl)",
            s.cat
                .explain_entries(
                    &Filter {
                        require: vec![l1],
                        any: vec![l2, l3],
                        exclude: vec![l4],
                        ..Default::default()
                    },
                    Sort::PathAsc,
                )
                .unwrap(),
        ),
        (
            "entries(path_contains)",
            s.cat
                .explain_entries(
                    &Filter {
                        path_contains: Some("manga".into()),
                        ..Default::default()
                    },
                    Sort::PathAsc,
                )
                .unwrap(),
        ),
        ("labels_in_use", s.cat.explain_labels_in_use().unwrap()),
        (
            "duplicate_groups",
            s.cat.explain_duplicate_groups().unwrap(),
        ),
        ("review_queue", s.cat.explain_review_queue(0.60).unwrap()),
        ("counts", s.cat.explain_counts().unwrap()),
    ];

    for (name, lines) in &plans {
        assert!(!lines.is_empty(), "empty plan for {name}");
        println!("=== PLAN: {name} ===");
        for line in lines {
            println!("  {line}");
        }
    }
}

#[test]
fn optimized_queries_preserve_exact_semantics_for_filters_sorts_labels_review_dupes_and_counts() {
    let mut s = setup_rich();

    let a_manga = s.asset("manga/a_page.jpg");
    let a_funny = s.asset("memes/c_funny.png");
    let a_chat = s.asset("shots/d_chat.png");
    let a_tie = s.asset("aaa/e_tie.jpg");
    let a_gone = s.asset("gone/f_removed.jpg");

    let cat_manga = s.cat.label(LabelKind::Category, "Manga").unwrap();
    let tag_meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let tag_shot = s.cat.label(LabelKind::Tag, "screenshot").unwrap();
    let tag_excl = s.cat.label(LabelKind::Tag, "descartado").unwrap();

    // Seed varied statuses and confidences across present, duplicate, and soon-to-be-missing assets.
    s.cat.accept(a_manga, cat_manga).unwrap();
    s.cat.suggest(a_manga, tag_meme, Source::Ai, 0.45).unwrap();
    s.cat.suggest(a_funny, tag_meme, Source::Ocr, 0.30).unwrap();
    s.cat
        .suggest(a_funny, tag_shot, Source::Rules, 0.85)
        .unwrap();
    s.cat.reject(a_funny, tag_excl).unwrap();
    s.cat.accept(a_chat, tag_shot).unwrap();
    s.cat.accept(a_chat, tag_excl).unwrap();
    s.cat.suggest(a_tie, cat_manga, Source::Ai, 0.55).unwrap();
    s.cat.suggest(a_gone, tag_meme, Source::Ai, 0.20).unwrap();
    s.cat.accept(a_gone, cat_manga).unwrap();

    // Remove f_removed.jpg and reindex so it becomes missing.
    fs::remove_file(s.library.join("gone/f_removed.jpg")).unwrap();
    let re = s.cat.index(&s.root).unwrap();
    assert_eq!(
        (re.new, re.changed, re.unchanged, re.missing, re.hashed),
        (0, 0, 5, 1, 0)
    );

    // 1. Counts
    assert_eq!(
        s.cat.counts().unwrap(),
        Counts {
            assets: 5,
            locations: 6,
            missing: 1,
        }
    );

    // 2. All 6 Sort orders with tie-breaking by path ascending
    let order = |sort: Sort| s.rel_entries(&s.cat.entries(&Filter::default(), sort).unwrap());
    assert_eq!(
        order(Sort::PathAsc),
        [
            "aaa/e_tie.jpg",
            "manga/a_page.jpg",
            "manga/b_dup.jpg",
            "memes/c_funny.png",
            "shots/d_chat.png",
        ]
    );
    assert_eq!(
        order(Sort::PathDesc),
        [
            "shots/d_chat.png",
            "memes/c_funny.png",
            "manga/b_dup.jpg",
            "manga/a_page.jpg",
            "aaa/e_tie.jpg",
        ]
    );
    assert_eq!(
        order(Sort::SizeAsc),
        [
            "aaa/e_tie.jpg",
            "manga/a_page.jpg",
            "manga/b_dup.jpg",
            "shots/d_chat.png",
            "memes/c_funny.png",
        ]
    );
    assert_eq!(
        order(Sort::SizeDesc),
        [
            "memes/c_funny.png",
            "shots/d_chat.png",
            "aaa/e_tie.jpg",
            "manga/a_page.jpg",
            "manga/b_dup.jpg",
        ]
    );
    assert_eq!(
        order(Sort::ModifiedAsc),
        [
            "memes/c_funny.png",
            "aaa/e_tie.jpg",
            "manga/a_page.jpg",
            "manga/b_dup.jpg",
            "shots/d_chat.png",
        ]
    );
    assert_eq!(
        order(Sort::ModifiedDesc),
        [
            "shots/d_chat.png",
            "aaa/e_tie.jpg",
            "manga/a_page.jpg",
            "manga/b_dup.jpg",
            "memes/c_funny.png",
        ]
    );

    // 3. Multi-label filters (require + any + exclude + path_contains)
    let f3 = Filter {
        require: vec![cat_manga],
        any: vec![tag_meme, tag_shot],
        exclude: vec![tag_excl],
        ..Default::default()
    };
    assert_eq!(
        s.rel_entries(&s.cat.entries(&f3, Sort::PathAsc).unwrap()),
        ["manga/a_page.jpg", "manga/b_dup.jpg"]
    );

    // Rejected label on a_funny must NOT exclude it when excluded? Wait: a_funny has tag_excl
    // REJECTED, so it does NOT carry tag_excl, hence exclude:[tag_excl] keeps a_funny and drops a_chat.
    let f_excl = Filter {
        any: vec![tag_shot],
        exclude: vec![tag_excl],
        ..Default::default()
    };
    assert_eq!(
        s.rel_entries(&s.cat.entries(&f_excl, Sort::PathAsc).unwrap()),
        ["memes/c_funny.png"]
    );

    // 4. labels_in_use: distinct non-missing assets, ignoring rejected and missing-only assets
    assert_eq!(
        s.cat.labels_in_use().unwrap(),
        vec![
            LabelCount {
                label: cat_manga,
                kind: LabelKind::Category,
                name: "Manga".into(),
                images: 2, // a_manga (2 locations counts once) + a_tie; a_gone is missing
            },
            LabelCount {
                label: tag_excl,
                kind: LabelKind::Tag,
                name: "descartado".into(),
                images: 1, // a_chat only; a_funny is rejected
            },
            LabelCount {
                label: tag_meme,
                kind: LabelKind::Tag,
                name: "meme".into(),
                images: 2, // a_manga + a_funny; a_gone is missing
            },
            LabelCount {
                label: tag_shot,
                kind: LabelKind::Tag,
                name: "screenshot".into(),
                images: 2, // a_funny + a_chat
            },
        ]
    );

    // 5. review_queue: lowest confidence first, excluding missing assets and >= threshold,
    // picking MIN(path) for duplicates.
    let queue = s.cat.review_queue(0.60).unwrap();
    assert_eq!(queue.len(), 3);
    assert_eq!((queue[0].asset, queue[0].label), (a_funny, tag_meme));
    assert_eq!((queue[1].asset, queue[1].label), (a_manga, tag_meme));
    assert!(queue[1].path.ends_with("a_page.jpg"));
    assert_eq!((queue[2].asset, queue[2].label), (a_tie, cat_manga));

    // 6. duplicate_groups
    let dupes = s.cat.duplicate_groups().unwrap();
    assert_eq!(dupes.len(), 1);
    assert_eq!(
        s.rel_entries(&dupes[0].entries),
        ["manga/a_page.jpg", "manga/b_dup.jpg"]
    );

    // 7. Reopening database preserves counts and second index hashes nothing
    let mut reopened = Catalog::open(&s.db).unwrap();
    let second = reopened.index(&s.root).unwrap();
    assert_eq!((second.unchanged, second.hashed, second.missing), (5, 0, 0));
}

#[test]
fn batched_index_with_commits_across_batch_boundaries_and_stops_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    // 600 files crosses a 512-file commit batch boundary.
    for i in 0..600usize {
        write(
            &library,
            &format!("f_{i:04}.jpg"),
            format!("img-{i}").as_bytes(),
        );
    }
    let root = LibraryRoot::new(&library).unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();

    // Stop partway into the second batch (at 530 files): all 530 must be committed.
    let stopped = cat
        .index_with(&root, &mut |p| {
            if p.done >= 530 {
                Control::Stop
            } else {
                Control::Continue
            }
        })
        .unwrap();
    assert!(stopped.stopped);
    assert_eq!(stopped.new, 530);
    assert_eq!(stopped.hashed, 530);
    assert_eq!(stopped.missing, 0);
    assert_eq!(cat.counts().unwrap().locations, 530);

    // Resuming finishes only the remaining 70 files without rehashing the 530.
    let resumed = cat.index(&root).unwrap();
    assert!(!resumed.stopped);
    assert_eq!(
        (resumed.new, resumed.unchanged, resumed.hashed),
        (70, 530, 70)
    );
    assert_eq!(cat.counts().unwrap().locations, 600);

    // Full unchanged reindex hashes nothing across both batches.
    let unchanged = cat.index(&root).unwrap();
    assert_eq!(
        (
            unchanged.new,
            unchanged.changed,
            unchanged.unchanged,
            unchanged.hashed
        ),
        (0, 0, 600, 0)
    );
}

#[test]
#[ignore = "heavy 45k-asset x 13-line OCR search benchmark; run with --release -- --ignored --nocapture"]
fn benchmark_ocr_search_45k_assets_13_lines() {
    use catalog::{SCREENSHOT_CATEGORY, ocr_search_key};
    use rusqlite::{Connection, params};
    use std::time::Instant;

    const ASSETS: usize = 45_000;
    const LINES_PER_ASSET: usize = 13;
    const ITERS: usize = 20;

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("ocr_bench.sqlite");
    let db = ensure_writable(&db_path, &[]).unwrap();

    // Initialize schema through Catalog::open
    let cat = Catalog::open(&db).unwrap();
    drop(cat);

    // Seed 45,000 synthetic assets + locations + 585,000 OCR lines in one transaction
    let seed_start = Instant::now();
    {
        let mut conn = Connection::open(db.as_path()).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut ins_asset = tx
                .prepare("INSERT INTO asset (id, sha256, size) VALUES (?1, ?2, ?3)")
                .unwrap();
            let mut ins_loc = tx
                .prepare(
                    "INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, 0)",
                )
                .unwrap();
            let mut ins_ocr = tx
                .prepare(
                    "INSERT INTO ocr_line
                         (asset_id, line, text, search_key, score, recognizer, x0, y0, x1, y1, top_strip)
                     VALUES (?1, ?2, ?3, ?4, 0.92, 'v6', 10.0, 20.0, 200.0, 50.0, ?5)",
                )
                .unwrap();

            let sample_lines = [
                "Crónicas del Relojero Capítulo",
                "Academia Nebulosa Volumen",
                "El Jardín de Papel Especial",
                "Estudio Cometa Traducción",
                "Isla de Cristal Portada",
                "Lista de proyectos activos",
                "Próximo episodio viernes",
                "Créditos limpieza y edición",
                "Resumen semanal de lectura",
                "Notas del traductor página",
                "Colección ilustrada edición",
                "Índice general de tomos",
                "14:32 5G 84%",
            ];
            let sample_keys: Vec<String> = sample_lines.iter().map(|s| ocr_search_key(s)).collect();

            for i in 1..=(ASSETS as i64) {
                let sha = format!("{i:064x}");
                ins_asset.execute(params![i, sha, 102_400i64]).unwrap();
                let path = format!("C:/synthetic/folder_{:03}/img_{i:06}.jpg", i % 200);
                ins_loc
                    .execute(params![
                        i,
                        "C:/synthetic",
                        path,
                        102_400i64,
                        1_700_000_000_000_000_000i64
                    ])
                    .unwrap();
                for (line_idx, raw) in sample_lines.iter().enumerate().take(LINES_PER_ASSET) {
                    let is_rare = i % 5000 == 0 && line_idx == 4;
                    let (text, key) = if is_rare {
                        ("ANDTHEPAPERCROWN", "andthepapercrown".to_string())
                    } else {
                        (*raw, format!("{}{}", sample_keys[line_idx], i % 97))
                    };
                    let top_strip = i64::from(line_idx == 12);
                    ins_ocr
                        .execute(params![i, line_idx as i64, text, key, top_strip])
                        .unwrap();
                }
            }
        }
        tx.commit().unwrap();
    }
    println!(
        "Seeded {ASSETS} assets x {LINES_PER_ASSET} lines ({} lines) in {:.2} s",
        ASSETS * LINES_PER_ASSET,
        seed_start.elapsed().as_secs_f64()
    );

    let cat = Catalog::open(&db).unwrap();
    // Mark ~20% as screenshots to exercise the status-bar exclusion check
    let shot_label = cat.label(LabelKind::Category, SCREENSHOT_CATEGORY).unwrap();
    for i in (1..=(ASSETS as i64)).step_by(5) {
        cat.suggest(AssetId(i), shot_label, Source::Rules, 0.9)
            .unwrap();
    }

    let queries = [
        ("rare_hit (paper crown)", "paper crown"),
        ("no_hit (zzzyyyxxx)", "zzzyyyxxx"),
        ("status_bar (84%)", "84%"),
        ("two_chars_no_hit (zq)", "zq"),
        ("two_chars_frequent (de)", "de"),
    ];

    for (name, q) in queries {
        let filter = Filter {
            search: Some(q.to_string()),
            ..Default::default()
        };
        let plan = cat.explain_entries(&filter, Sort::PathAsc).unwrap();
        let mut times_ms = Vec::with_capacity(ITERS);
        let mut result_count = 0usize;
        for _ in 0..ITERS {
            let t0 = Instant::now();
            let res = cat.entries(&filter, Sort::PathAsc).unwrap();
            times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
            result_count = res.len();
        }
        times_ms.sort_by(f64::total_cmp);
        let mean = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
        let p95 = times_ms[((times_ms.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
        println!(
            "OCR search [{name}]: results={result_count}, mean={mean:.2} ms, p95={p95:.2} ms (target < 150 ms)"
        );
        for line in plan {
            println!("  PLAN: {line}");
        }
    }
}

#[test]
#[ignore = "heavy 45k-asset vision ranking and rule-apply benchmark; run with --release -- --ignored --nocapture"]
fn benchmark_vision_rank_and_apply_45k_assets() {
    use catalog::{EMBEDDING_DIM, SIGLIP2_MODEL_ID, encode_embedding};
    use rusqlite::{Connection, params};
    use std::time::Instant;

    const ASSETS: usize = 45_000;
    const RANK_ITERS: usize = 10;

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("vision_bench.sqlite");
    let db = ensure_writable(&db_path, &[]).unwrap();

    // Initialize schema + built-in vision rules
    let mut cat = Catalog::open(&db).unwrap();
    let rules = cat.list_vision_rules().unwrap();
    let mut text_pairs: Vec<(String, Vec<f32>)> = Vec::new();
    for (idx, rule) in rules.iter().enumerate() {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[idx % 8] = 1.0;
        for p in &rule.positives {
            text_pairs.push((p.clone(), v.clone()));
        }
        let mut nv = vec![0.0f32; EMBEDDING_DIM];
        nv[0] = 1.0;
        for n in &rule.negatives {
            text_pairs.push((n.clone(), nv.clone()));
        }
    }
    let refs: Vec<(&str, &[f32])> = text_pairs
        .iter()
        .map(|(t, v)| (t.as_str(), v.as_slice()))
        .collect();
    cat.save_text_embeddings(SIGLIP2_MODEL_ID, &refs).unwrap();
    drop(cat);

    // Precompute 16 distinct L2-normalized synthetic 768-d blobs so seeding is fast.
    let mut template_blobs: Vec<Vec<u8>> = Vec::with_capacity(16);
    for t in 0..16usize {
        let mut v = vec![0.0f32; EMBEDDING_DIM];
        v[t % 8] = 0.25;
        v[(t + 1) % 8] = 0.12;
        for (j, slot) in v.iter_mut().enumerate().skip(16) {
            *slot = (((t * 31 + j * 17) % 100) as f32 - 50.0) * 0.002;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= norm;
        }
        template_blobs.push(encode_embedding(&v).unwrap());
    }

    let seed_start = Instant::now();
    {
        let mut conn = Connection::open(db.as_path()).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut ins_asset = tx
                .prepare("INSERT INTO asset (id, sha256, size) VALUES (?1, ?2, ?3)")
                .unwrap();
            let mut ins_loc = tx
                .prepare(
                    "INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                     VALUES (?1, ?2, ?3, ?4, ?5, 1, 0)",
                )
                .unwrap();
            let mut ins_emb = tx
                .prepare(
                    "INSERT INTO asset_embedding (asset_id, model, vector) VALUES (?1, ?2, ?3)",
                )
                .unwrap();
            let mut ins_face = tx
                .prepare(
                    "INSERT INTO asset_face (asset_id, face_idx, x1, y1, x2, y2, score)
                     VALUES (?1, 0, 20.0, 20.0, 120.0, 140.0, 0.91)",
                )
                .unwrap();

            for i in 1..=(ASSETS as i64) {
                let sha = format!("{i:064x}");
                ins_asset.execute(params![i, sha, 102_400i64]).unwrap();
                let path = format!("C:/synthetic/folder_{:03}/img_{i:06}.jpg", i % 200);
                ins_loc
                    .execute(params![
                        i,
                        "C:/synthetic",
                        path,
                        102_400i64,
                        1_700_000_000_000_000_000i64
                    ])
                    .unwrap();
                let blob = &template_blobs[(i as usize) % template_blobs.len()];
                ins_emb
                    .execute(params![i, SIGLIP2_MODEL_ID, blob.as_slice()])
                    .unwrap();
                if i % 7 == 0 {
                    ins_face.execute([i]).unwrap();
                }
            }
        }
        tx.commit().unwrap();
    }
    println!(
        "Seeded {ASSETS} vision embeddings in {:.2} s",
        seed_start.elapsed().as_secs_f64()
    );

    let mut cat = Catalog::open(&db).unwrap();
    let mut query = vec![0.0f32; EMBEDDING_DIM];
    query[2] = 1.0;

    let mut rank_ms = Vec::with_capacity(RANK_ITERS);
    let mut top_len = 0usize;
    for _ in 0..RANK_ITERS {
        let t0 = Instant::now();
        let ranked = cat
            .rank_by_embeddings(SIGLIP2_MODEL_ID, &[&query], 200)
            .unwrap();
        rank_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
        top_len = ranked.len();
    }
    rank_ms.sort_by(f64::total_cmp);
    let mean = rank_ms.iter().sum::<f64>() / rank_ms.len() as f64;
    let p95 = rank_ms[((rank_ms.len() as f64 * 0.95).ceil() as usize).saturating_sub(1)];
    println!(
        "Vision rank top-{top_len} over {ASSETS} embeddings: mean={mean:.2} ms, p95={p95:.2} ms"
    );

    let (compiled, missing) = cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();
    assert!(missing.is_empty());
    let assets: Vec<AssetId> = (1..=(ASSETS as i64)).map(AssetId).collect();

    let apply_start = Instant::now();
    let mut total_matched = 0usize;
    let mut total_suggestions = 0usize;
    for chunk in assets.chunks(500) {
        let (m, s) = cat.apply_vision_rules_batch(chunk, &compiled).unwrap();
        total_matched += m;
        total_suggestions += s;
    }
    let apply_secs = apply_start.elapsed().as_secs_f64();
    println!(
        "Vision apply all {} rules to {ASSETS} assets: {:.2} s (matched={total_matched}, suggestions={total_suggestions})",
        compiled.rules.len(),
        apply_secs
    );
}
