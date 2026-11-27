// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, CAT_ANIMALS, CAT_ANIME, CAT_COMPUTERS_HARDWARE, CAT_MANHWA, CAT_NATURE_LANDSCAPES,
    CAT_TECH_ELECTRONICS, CAT_VIDEOGAMES, CAT_WEBTOON, Catalog, DetectedFace, EMBEDDING_DIM,
    LabelKind, MANGA_COVER_CATEGORY, RuleMode, SCREENSHOT_CATEGORY, SIGLIP2_MODEL_ID, Source,
    Status, TAG_PERSON, TYPE_DOCUMENT, TYPE_ILLUSTRATION, TYPE_MANGA, TYPE_MEME, TYPE_PHOTO,
    VISION_STAGE, VISION_VERSION,
};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    db: DestPath,
    cat: Catalog,
}

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup(files: &[(&str, &[u8])]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    for (rel, bytes) in files {
        write_file(&library, rel, bytes);
    }
    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
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
        let p = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&p).unwrap().unwrap()
    }
}

fn unit_basis(dim: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; EMBEDDING_DIM];
    v[dim] = 1.0;
    v
}

fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

fn seed_orthogonal_rule_embeddings(cat: &mut Catalog) {
    let rules = cat.list_vision_rules().unwrap();
    let mut pairs: Vec<(String, Vec<f32>)> = Vec::new();
    for (idx, rule) in rules.iter().enumerate() {
        let dim = match rule.builtin_key.as_deref() {
            Some("type_manga") => 0,
            Some("type_illustration") => 1,
            Some("type_photo") => 2,
            Some("type_screenshot") => 3,
            Some("type_meme") => 4,
            Some("type_document") => 5,
            Some("tag_person") => 6,
            Some("cat_anime") => 8,
            Some("cat_manhwa") => 9,
            Some("cat_webtoon") => 10,
            Some("cat_animals") => 11,
            Some("cat_computers_hardware") => 12,
            Some("cat_tech_electronics") => 13,
            Some("cat_nature_landscapes") => 14,
            Some("cat_videogames") => 15,
            Some("page_manga_cover") => 16,
            _ => 20 + idx,
        };
        for p in &rule.positives {
            pairs.push((p.clone(), unit_basis(dim)));
        }
        for n in &rule.negatives {
            pairs.push((n.clone(), unit_basis(0))); // manga dim as negative for persona
        }
    }
    let refs: Vec<(&str, &[f32])> = pairs
        .iter()
        .map(|(t, v)| (t.as_str(), v.as_slice()))
        .collect();
    cat.save_text_embeddings(SIGLIP2_MODEL_ID, &refs).unwrap();
}

#[test]
fn builtin_rules_created_once_and_never_recreated_after_edit_or_delete() {
    let s = setup(&[("a.jpg", b"aaa")]);
    let rules = s.cat.list_vision_rules().unwrap();
    assert_eq!(rules.len(), 16);

    let names: Vec<&str> = rules.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            TYPE_MANGA,
            TYPE_ILLUSTRATION,
            TYPE_PHOTO,
            SCREENSHOT_CATEGORY,
            TYPE_MEME,
            TYPE_DOCUMENT,
            TAG_PERSON,
            CAT_ANIME,
            CAT_MANHWA,
            CAT_WEBTOON,
            CAT_ANIMALS,
            CAT_COMPUTERS_HARDWARE,
            CAT_TECH_ELECTRONICS,
            CAT_NATURE_LANDSCAPES,
            CAT_VIDEOGAMES,
            MANGA_COVER_CATEGORY,
        ]
    );

    // Edit Manga threshold and delete Meme + Videojuegos
    let manga = rules.iter().find(|r| r.name == TYPE_MANGA).unwrap();
    let meme = rules.iter().find(|r| r.name == TYPE_MEME).unwrap();
    let games = rules.iter().find(|r| r.name == CAT_VIDEOGAMES).unwrap();
    assert_eq!(games.mode, RuleMode::Independent);
    assert_eq!(games.kind, LabelKind::Category);
    s.cat
        .update_vision_rule(manga.id, true, 0.25, &["custom manga prompt".into()], &[])
        .unwrap();
    assert!(s.cat.delete_vision_rule(meme.id).unwrap());
    assert!(s.cat.delete_vision_rule(games.id).unwrap());

    // Reopen database: Meme and Videojuegos must NOT be recreated, Manga edit must NOT be overwritten
    let reopened = Catalog::open(&s.db).unwrap();
    let after = reopened.list_vision_rules().unwrap();
    assert_eq!(after.len(), 14);
    assert!(!after.iter().any(|r| r.name == TYPE_MEME));
    assert!(!after.iter().any(|r| r.name == CAT_VIDEOGAMES));
    let manga_after = after.iter().find(|r| r.name == TYPE_MANGA).unwrap();
    assert!((manga_after.threshold - 0.25).abs() < 1e-6);
    assert_eq!(manga_after.positives, vec!["custom manga prompt"]);
}

#[test]
fn save_vision_result_stores_embedding_faces_and_marks_stage_done_in_one_tx() {
    let mut s = setup(&[("a.jpg", b"aaa"), ("b.jpg", b"bbb")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    assert_eq!(
        s.cat
            .count_pending_in_root(VISION_STAGE, VISION_VERSION, s.root.as_path())
            .unwrap(),
        2
    );

    let emb = unit_basis(2);
    let faces = vec![DetectedFace {
        bbox: [10.0, 20.0, 100.0, 120.0],
        score: 0.93,
    }];
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &faces)
        .unwrap();

    assert!(s.cat.has_vision_done(a).unwrap());
    assert!(!s.cat.has_vision_done(b).unwrap());
    assert_eq!(
        s.cat
            .count_pending_in_root(VISION_STAGE, VISION_VERSION, s.root.as_path())
            .unwrap(),
        1
    );
    let (model, loaded_emb) = s.cat.asset_embedding(a).unwrap().unwrap();
    assert_eq!(model, SIGLIP2_MODEL_ID);
    assert_eq!(loaded_emb, emb);
    assert_eq!(s.cat.asset_faces(a).unwrap(), faces);
}

#[test]
fn type_exclusivity_and_ordering_confidence() {
    let mut s = setup(&[("clear_manga.jpg", b"m1"), ("ambiguous.jpg", b"m2")]);
    seed_orthogonal_rule_embeddings(&mut s.cat);
    let (compiled, missing) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();
    assert!(missing.is_empty());

    let clear = s.asset("clear_manga.jpg");
    let ambig = s.asset("ambiguous.jpg");

    // Clear manga: strong dim 0
    s.cat
        .save_vision_result(clear, SIGLIP2_MODEL_ID, &unit_basis(0), &[])
        .unwrap();

    // Ambiguous: almost tied between Manga (0) and Illustration (1), both > 0.08
    let mut amb_vec = vec![0.0f32; EMBEDDING_DIM];
    amb_vec[0] = 0.71;
    amb_vec[1] = 0.70;
    s.cat
        .save_vision_result(ambig, SIGLIP2_MODEL_ID, &normalized(amb_vec), &[])
        .unwrap();

    s.cat.apply_vision_rules(clear, &compiled).unwrap();
    s.cat.apply_vision_rules(ambig, &compiled).unwrap();

    let clear_labels = s.cat.labels_of(clear).unwrap();
    let ambig_labels = s.cat.labels_of(ambig).unwrap();

    // Exactly ONE type category each (exclusive)
    assert_eq!(clear_labels.len(), 1);
    assert_eq!(clear_labels[0].name, TYPE_MANGA);
    assert_eq!(ambig_labels.len(), 1);
    assert_eq!(ambig_labels[0].name, TYPE_MANGA);

    // Ambiguous image must have strictly lower ordering confidence so it surfaces first in review
    assert!(ambig_labels[0].confidence < clear_labels[0].confidence);
}

#[test]
fn persona_rule_respects_face_override_on_photo_and_blocks_drawn_face_on_manga() {
    let mut s = setup(&[
        ("photo_face.jpg", b"p1"),
        ("manga_face.jpg", b"p2"),
        ("photo_weak_face.jpg", b"p3"),
        ("screenshot_face.jpg", b"p4"),
    ]);
    seed_orthogonal_rule_embeddings(&mut s.cat);
    let (compiled, _) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();

    let strong_face = vec![DetectedFace {
        bbox: [20.0, 20.0, 80.0, 90.0],
        score: 0.92,
    }];
    let weak_face = vec![DetectedFace {
        bbox: [20.0, 20.0, 80.0, 90.0],
        score: 0.75,
    }];

    // 1. Photo with strong face (even with 0 text similarity on dim 6) -> Foto + persona
    let photo = s.asset("photo_face.jpg");
    s.cat
        .save_vision_result(photo, SIGLIP2_MODEL_ID, &unit_basis(2), &strong_face)
        .unwrap();
    s.cat.apply_vision_rules(photo, &compiled).unwrap();
    let names: Vec<String> = s
        .cat
        .labels_of(photo)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&TYPE_PHOTO.to_string()));
    assert!(names.contains(&TAG_PERSON.to_string()));

    // 2. Manga with strong drawn face -> Manga ONLY, NEVER persona!
    let manga = s.asset("manga_face.jpg");
    s.cat
        .save_vision_result(manga, SIGLIP2_MODEL_ID, &unit_basis(0), &strong_face)
        .unwrap();
    s.cat.apply_vision_rules(manga, &compiled).unwrap();
    let names: Vec<String> = s
        .cat
        .labels_of(manga)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&TYPE_MANGA.to_string()));
    assert!(
        !names.contains(&TAG_PERSON.to_string()),
        "drawn face on manga must NOT trigger persona: {names:?}"
    );

    // 3. Photo with weak face (< 0.8) and no text match -> Foto only
    let weak = s.asset("photo_weak_face.jpg");
    s.cat
        .save_vision_result(weak, SIGLIP2_MODEL_ID, &unit_basis(2), &weak_face)
        .unwrap();
    s.cat.apply_vision_rules(weak, &compiled).unwrap();
    let names: Vec<String> = s
        .cat
        .labels_of(weak)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&TYPE_PHOTO.to_string()));
    assert!(!names.contains(&TAG_PERSON.to_string()));

    // 4. Screenshot with strong face -> Capturas de pantalla + persona
    let shot = s.asset("screenshot_face.jpg");
    s.cat
        .save_vision_result(shot, SIGLIP2_MODEL_ID, &unit_basis(3), &strong_face)
        .unwrap();
    s.cat.apply_vision_rules(shot, &compiled).unwrap();
    let names: Vec<String> = s
        .cat
        .labels_of(shot)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names.contains(&SCREENSHOT_CATEGORY.to_string()));
    assert!(names.contains(&TAG_PERSON.to_string()));
}

#[test]
fn apply_is_idempotent_respects_user_decisions_and_other_sources_and_cleans_disabled_or_deleted_rules()
 {
    let mut s = setup(&[("a.jpg", b"a1"), ("b.jpg", b"b2")]);
    seed_orthogonal_rule_embeddings(&mut s.cat);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    // Pre-existing suggestions from other sources on `a`:
    let shot_lbl = s
        .cat
        .label(LabelKind::Category, SCREENSHOT_CATEGORY)
        .unwrap();
    let rules_tag = s.cat.label(LabelKind::Tag, "regla-local").unwrap();
    let ocr_work = s.cat.label(LabelKind::Entity, "Obra OCR").unwrap();
    s.cat.suggest(a, shot_lbl, Source::Rules, 0.90).unwrap();
    s.cat.suggest(a, rules_tag, Source::Rules, 0.85).unwrap();
    s.cat.suggest(a, ocr_work, Source::Ocr, 0.88).unwrap();

    // `a` looks like Manga (dim 0); `b` looks like Photo (dim 2) + strong face
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &unit_basis(0), &[])
        .unwrap();
    s.cat
        .save_vision_result(
            b,
            SIGLIP2_MODEL_ID,
            &unit_basis(2),
            &[DetectedFace {
                bbox: [0.0, 0.0, 50.0, 50.0],
                score: 0.95,
            }],
        )
        .unwrap();

    let (compiled, _) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();
    s.cat.apply_vision_rules(a, &compiled).unwrap();
    s.cat.apply_vision_rules(b, &compiled).unwrap();

    // Re-applying is idempotent
    let before_a = s.cat.labels_of(a).unwrap();
    s.cat.apply_vision_rules(a, &compiled).unwrap();
    assert_eq!(s.cat.labels_of(a).unwrap(), before_a);

    // Other sources on `a` were untouched
    let a_labels = s.cat.labels_of(a).unwrap();
    assert!(
        a_labels
            .iter()
            .any(|x| x.label == shot_lbl && x.source == Source::Rules)
    );
    assert!(
        a_labels
            .iter()
            .any(|x| x.label == rules_tag && x.source == Source::Rules)
    );
    assert!(
        a_labels
            .iter()
            .any(|x| x.label == ocr_work && x.source == Source::Ocr)
    );

    // User accepts Manga on `a` and rejects Foto on `b`
    let manga_lbl = s.cat.label(LabelKind::Category, TYPE_MANGA).unwrap();
    let photo_lbl = s.cat.label(LabelKind::Category, TYPE_PHOTO).unwrap();
    let person_lbl = s.cat.label(LabelKind::Tag, TAG_PERSON).unwrap();
    s.cat.accept(a, manga_lbl).unwrap();
    s.cat.reject(b, photo_lbl).unwrap();

    // Check accepted/rejected counts on rules
    let rules = s.cat.list_vision_rules().unwrap();
    let manga_rule = rules.iter().find(|r| r.name == TYPE_MANGA).unwrap();
    let photo_rule = rules.iter().find(|r| r.name == TYPE_PHOTO).unwrap();
    let person_rule = rules.iter().find(|r| r.name == TAG_PERSON).unwrap();
    assert_eq!((manga_rule.accepted_ai, manga_rule.rejected_ai), (1, 0));
    assert_eq!((photo_rule.accepted_ai, photo_rule.rejected_ai), (0, 1));

    // Disable persona rule -> re-apply removes persona's AI suggestion on `b`
    s.cat
        .update_vision_rule(
            person_rule.id,
            false,
            person_rule.threshold,
            &person_rule.positives,
            &person_rule.negatives,
        )
        .unwrap();
    // Delete Foto rule -> re-apply keeps label and user's Rejected decision on `b`
    s.cat.delete_vision_rule(photo_rule.id).unwrap();

    let (compiled2, _) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();
    s.cat.apply_vision_rules(a, &compiled2).unwrap();
    s.cat.apply_vision_rules(b, &compiled2).unwrap();

    let b_after = s.cat.labels_of(b).unwrap();
    assert!(
        !b_after.iter().any(|x| x.label == person_lbl),
        "disabled rule's AI suggestion must be removed on next apply"
    );
    assert!(
        b_after
            .iter()
            .any(|x| x.label == photo_lbl && x.status == Status::Rejected),
        "deleted rule must keep user's rejected decision"
    );

    // Accepted count on Manga stays accurate after re-applying
    let rules_after = s.cat.list_vision_rules().unwrap();
    let manga_after = rules_after.iter().find(|r| r.name == TYPE_MANGA).unwrap();
    assert_eq!((manga_after.accepted_ai, manga_after.rejected_ai), (1, 0));
}

#[test]
fn rank_by_embeddings_orders_descending_and_excludes_missing_assets() {
    let mut s = setup(&[("high.jpg", b"h1"), ("mid.jpg", b"m1"), ("low.jpg", b"l1")]);
    let high = s.asset("high.jpg");
    let mid = s.asset("mid.jpg");
    let low = s.asset("low.jpg");

    let q = unit_basis(0);
    let mut v_mid = vec![0.0f32; EMBEDDING_DIM];
    v_mid[0] = 0.6;
    v_mid[1] = 0.8;

    s.cat
        .save_vision_result(high, SIGLIP2_MODEL_ID, &unit_basis(0), &[])
        .unwrap();
    s.cat
        .save_vision_result(mid, SIGLIP2_MODEL_ID, &v_mid, &[])
        .unwrap();
    s.cat
        .save_vision_result(low, SIGLIP2_MODEL_ID, &unit_basis(1), &[])
        .unwrap();

    let ranked = s
        .cat
        .rank_by_embeddings(SIGLIP2_MODEL_ID, &[&q], 2)
        .unwrap();
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].entry.asset, high);
    assert!((ranked[0].score - 1.0).abs() < 1e-5);
    assert_eq!(ranked[1].entry.asset, mid);
    assert!((ranked[1].score - 0.6).abs() < 1e-5);

    // Remove high.jpg and reindex -> missing asset must be excluded from ranking
    fs::remove_file(s.library.join("high.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();
    let ranked2 = s
        .cat
        .rank_by_embeddings(SIGLIP2_MODEL_ID, &[&q], 10)
        .unwrap();
    assert_eq!(ranked2.len(), 2);
    assert_eq!(ranked2[0].entry.asset, mid);
    assert_eq!(ranked2[1].entry.asset, low);
}

#[test]
fn creating_a_tag_never_replaces_an_existing_rule() {
    let tmp = tempfile::tempdir().unwrap();
    let db = fsguard::ensure_writable(&tmp.path().join("catalog.sqlite"), &[]).unwrap();
    let mut cat = catalog::Catalog::open(&db).unwrap();
    let person = |cat: &catalog::Catalog| {
        cat.list_vision_rules()
            .unwrap()
            .into_iter()
            .find(|r| r.name == catalog::TAG_PERSON)
            .unwrap()
    };
    let before = person(&cat);
    let attempt = cat.create_vision_rule(
        "Persona",
        catalog::LabelKind::Tag,
        catalog::RuleMode::Independent,
        0.3,
        &["something else".into()],
        &[],
    );
    assert!(attempt.is_err());
    assert_eq!(person(&cat), before);
}

#[test]
fn content_categories_combine_independently_with_image_type() {
    let mut s = setup(&[("game_shot.png", b"g1")]);
    seed_orthogonal_rule_embeddings(&mut s.cat);
    let (compiled, _) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();

    // Image aligns with both Capturas de pantalla (dim 3) and Animales (dim 11).
    let mut v = vec![0.0f32; EMBEDDING_DIM];
    v[3] = 0.72;
    v[11] = 0.69;
    let asset = s.asset("game_shot.png");
    s.cat
        .save_vision_result(asset, SIGLIP2_MODEL_ID, &normalized(v), &[])
        .unwrap();
    s.cat.apply_vision_rules(asset, &compiled).unwrap();

    let names: Vec<String> = s
        .cat
        .labels_of(asset)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(
        names.contains(&SCREENSHOT_CATEGORY.to_string()),
        "expected Capturas de pantalla in {names:?}"
    );
    assert!(
        names.contains(&CAT_ANIMALS.to_string()),
        "expected Animales in {names:?}"
    );
}

#[test]
fn manga_cover_only_suggested_when_winning_type_is_manga_or_illustration() {
    let mut s = setup(&[
        ("manga_cover.png", b"mc"),
        ("illus_cover.png", b"ic"),
        ("photo_cover.png", b"pc"),
    ]);
    seed_orthogonal_rule_embeddings(&mut s.cat);
    let (compiled, _) = s.cat.compile_vision_rules(SIGLIP2_MODEL_ID).unwrap();

    // Note: negatives in seed_orthogonal_rule_embeddings use dim 0, so for Manga (dim 0)
    // give dim 16 (cover) slightly higher weight than dim 0 so pos > neg.
    let mut v_manga = vec![0.0f32; EMBEDDING_DIM];
    v_manga[0] = 0.60;
    v_manga[16] = 0.80;
    let a_manga = s.asset("manga_cover.png");
    s.cat
        .save_vision_result(a_manga, SIGLIP2_MODEL_ID, &normalized(v_manga), &[])
        .unwrap();
    s.cat.apply_vision_rules(a_manga, &compiled).unwrap();
    let names_m: Vec<String> = s
        .cat
        .labels_of(a_manga)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names_m.contains(&TYPE_MANGA.to_string()));
    assert!(
        names_m.contains(&MANGA_COVER_CATEGORY.to_string()),
        "expected Portada de manga on Manga: {names_m:?}"
    );

    let mut v_illus = vec![0.0f32; EMBEDDING_DIM];
    v_illus[1] = 0.70;
    v_illus[16] = 0.71;
    let a_illus = s.asset("illus_cover.png");
    s.cat
        .save_vision_result(a_illus, SIGLIP2_MODEL_ID, &normalized(v_illus), &[])
        .unwrap();
    s.cat.apply_vision_rules(a_illus, &compiled).unwrap();
    let names_i: Vec<String> = s
        .cat
        .labels_of(a_illus)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names_i.contains(&TYPE_ILLUSTRATION.to_string()));
    assert!(
        names_i.contains(&MANGA_COVER_CATEGORY.to_string()),
        "expected Portada de manga on Ilustración: {names_i:?}"
    );

    let mut v_photo = vec![0.0f32; EMBEDDING_DIM];
    v_photo[2] = 0.70;
    v_photo[16] = 0.71;
    let a_photo = s.asset("photo_cover.png");
    s.cat
        .save_vision_result(a_photo, SIGLIP2_MODEL_ID, &normalized(v_photo), &[])
        .unwrap();
    s.cat.apply_vision_rules(a_photo, &compiled).unwrap();
    let names_p: Vec<String> = s
        .cat
        .labels_of(a_photo)
        .unwrap()
        .into_iter()
        .map(|a| a.name)
        .collect();
    assert!(names_p.contains(&TYPE_PHOTO.to_string()));
    assert!(
        !names_p.contains(&MANGA_COVER_CATEGORY.to_string()),
        "Portada de manga must NOT trigger on Foto: {names_p:?}"
    );
}

#[test]
fn upgrading_legacy_catalog_adds_new_builtins_once_preserving_edits_deletions_and_existing_labels()
{
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy_catalog.sqlite");

    // Create a legacy v1 database (old schema without asset_work/stage_error, old seeding flag '1'):
    // - Meme was deleted by user
    // - Manga was edited by user (threshold 0.25)
    // - Category "anime" (lowercase) already exists without a rule
    // - Category "VIDEOJUEGOS" (uppercase) already exists WITH a custom rule
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE asset (id INTEGER PRIMARY KEY, sha256 TEXT NOT NULL UNIQUE, size INTEGER NOT NULL);
             CREATE TABLE location (id INTEGER PRIMARY KEY, asset_id INTEGER NOT NULL, root TEXT NOT NULL, path TEXT NOT NULL UNIQUE, size INTEGER NOT NULL, mtime_ns INTEGER, seen_run INTEGER NOT NULL, missing INTEGER NOT NULL DEFAULT 0);
             CREATE TABLE label (id INTEGER PRIMARY KEY, kind TEXT NOT NULL CHECK (kind IN ('category', 'tag', 'entity')), name TEXT NOT NULL COLLATE NOCASE, UNIQUE (kind, name));
             CREATE TABLE asset_label (asset_id INTEGER NOT NULL, label_id INTEGER NOT NULL, source TEXT NOT NULL, confidence REAL NOT NULL, status TEXT NOT NULL, PRIMARY KEY (asset_id, label_id));
             CREATE TABLE analysis (asset_id INTEGER NOT NULL, stage TEXT NOT NULL, version INTEGER NOT NULL, PRIMARY KEY (asset_id, stage));
             CREATE TABLE catalog_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE vision_rule (id INTEGER PRIMARY KEY, label_id INTEGER NOT NULL UNIQUE, mode TEXT NOT NULL, threshold REAL NOT NULL, enabled INTEGER NOT NULL DEFAULT 1, positives TEXT NOT NULL, negatives TEXT NOT NULL DEFAULT '', builtin_key TEXT UNIQUE);
             CREATE TABLE vision_managed_label (label_id INTEGER PRIMARY KEY);
             INSERT INTO catalog_meta (key, value) VALUES ('vision_builtins_seeded', '1');
             INSERT INTO label (id, kind, name) VALUES (1, 'category', 'Manga');
             INSERT INTO vision_rule (label_id, mode, threshold, enabled, positives, negatives, builtin_key)
                 VALUES (1, 'type', 0.25, 1, 'edited manga prompt', '', 'type_manga');
             INSERT INTO label (id, kind, name) VALUES (10, 'category', 'anime');
             INSERT INTO label (id, kind, name) VALUES (11, 'category', 'VIDEOJUEGOS');
             INSERT INTO vision_rule (label_id, mode, threshold, enabled, positives, negatives, builtin_key)
                 VALUES (11, 'independent', 0.33, 1, 'custom game prompt', '', NULL);",
        )
        .unwrap();
    }

    let db = ensure_writable(&db_path, &[]).unwrap();
    let cat = Catalog::open(&db).unwrap();
    let rules = cat.list_vision_rules().unwrap();

    // Old deleted built-ins (e.g. Meme, Foto, Documento, Ilustración, Capturas, persona) must NOT be recreated.
    assert!(!rules.iter().any(|r| r.name == TYPE_MEME));
    assert!(!rules.iter().any(|r| r.name == TYPE_PHOTO));
    assert!(!rules.iter().any(|r| r.name == TAG_PERSON));

    // Edited Manga rule must be preserved untouched.
    let manga = rules.iter().find(|r| r.name == TYPE_MANGA).unwrap();
    assert!((manga.threshold - 0.25).abs() < 1e-6);
    assert_eq!(manga.positives, vec!["edited manga prompt"]);

    // Existing lowercase "anime" label must be reused (label_id == 10) and receive a rule.
    let anime = rules
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(CAT_ANIME))
        .expect("anime rule seeded on existing label");
    assert_eq!(anime.label.0, 10);

    // Existing uppercase "VIDEOJUEGOS" rule must NOT be overwritten.
    let games = rules
        .iter()
        .find(|r| r.name.eq_ignore_ascii_case(CAT_VIDEOGAMES))
        .expect("existing videogames rule kept");
    assert_eq!(games.label.0, 11);
    assert!((games.threshold - 0.33).abs() < 1e-6);
    assert_eq!(games.positives, vec!["custom game prompt"]);

    // New built-ins (e.g. Animales, Manhwa, Webtoon, Portada de manga) were added.
    let animals = rules
        .iter()
        .find(|r| r.name == CAT_ANIMALS)
        .expect("Animales added");
    assert!(rules.iter().any(|r| r.name == CAT_MANHWA));
    assert!(rules.iter().any(|r| r.name == MANGA_COVER_CATEGORY));

    // Delete a newly added built-in (Animales) and reopen: deletion must survive reopen.
    assert!(cat.delete_vision_rule(animals.id).unwrap());
    drop(cat);
    let reopened = Catalog::open(&db).unwrap();
    let after = reopened.list_vision_rules().unwrap();
    assert!(!after.iter().any(|r| r.name == CAT_ANIMALS));
    assert!(!after.iter().any(|r| r.name == TYPE_MEME));
}

#[test]
fn opening_an_up_to_date_catalog_does_not_need_the_write_lock() {
    let s = setup(&[("a.jpg", b"aaa")]);
    let writer = rusqlite::Connection::open(s.db.as_path()).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let reopened = Catalog::open(&s.db);
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(reopened.is_ok(), "{:?}", reopened.err());
}

#[test]
fn calibrated_values_reach_untouched_rules_of_older_catalogs_only() {
    let s = setup(&[("a.jpg", b"aaa")]);
    let raw = rusqlite::Connection::open(s.db.as_path()).unwrap();
    // As an older catalog left them: old defaults, one rule edited by the user.
    raw.execute_batch(
        "UPDATE vision_rule SET threshold = 0.08, enabled = 1 WHERE builtin_key LIKE 'type_%';
         UPDATE vision_rule SET enabled = 1 WHERE builtin_key = 'cat_webtoon';
         UPDATE vision_rule SET threshold = 0.2 WHERE builtin_key = 'type_photo';
         DELETE FROM catalog_meta WHERE key LIKE 'vision_builtins_calibrated:%';",
    )
    .unwrap();
    let rules = Catalog::open(&s.db).unwrap().list_vision_rules().unwrap();
    let rule = |name: &str| rules.iter().find(|r| r.name == name).unwrap();
    assert!((rule(TYPE_MANGA).threshold - 0.06).abs() < 1e-6);
    assert!((rule(TYPE_PHOTO).threshold - 0.2).abs() < 1e-6);
    assert!(!rule(CAT_WEBTOON).enabled);
}
