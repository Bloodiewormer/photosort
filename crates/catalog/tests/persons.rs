// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Catalog, DetectedFace, EMBEDDING_DIM, LabelKind, SIGLIP2_MODEL_ID,
    cosine_similarity_128,
};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    _root: LibraryRoot,
    _db: DestPath,
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
        _root: root,
        _db: db,
        cat,
    }
}

impl Setup {
    fn roots(&self) -> Vec<PathBuf> {
        vec![self._root.as_path().to_path_buf()]
    }

    fn asset(&self, rel: &str) -> AssetId {
        let p = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&p).unwrap().unwrap()
    }
}

fn unit_vector_128(dim: usize) -> [f32; 128] {
    let mut v = [0.0f32; 128];
    v[dim] = 1.0;
    v
}

fn blended_vector_128(dim1: usize, dim2: usize, weight1: f32, weight2: f32) -> [f32; 128] {
    let mut v = [0.0f32; 128];
    v[dim1] = weight1;
    v[dim2] = weight2;
    let norm = (weight1 * weight1 + weight2 * weight2).sqrt();
    v[dim1] /= norm;
    v[dim2] /= norm;
    v
}

#[test]
fn incremental_grouping_assigns_same_person_above_threshold_and_creates_new_below() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2"), ("c.jpg", b"3")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let c = s.asset("c.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(c, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let faces_a = s.cat.asset_face_details(a).unwrap();
    let faces_b = s.cat.asset_face_details(b).unwrap();
    let faces_c = s.cat.asset_face_details(c).unwrap();

    assert_eq!(faces_a.len(), 1);
    assert_eq!(faces_b.len(), 1);
    assert_eq!(faces_c.len(), 1);

    let e_base = unit_vector_128(0);
    // e_similar has cosine similarity ~0.8 with e_base (well above 0.363 threshold)
    let e_similar = blended_vector_128(0, 1, 0.8, 0.6);
    assert!(cosine_similarity_128(&e_base, &e_similar) > 0.7);

    // e_different is orthogonal to e_base (cosine similarity 0.0 < 0.363)
    let e_different = unit_vector_128(2);
    assert_eq!(cosine_similarity_128(&e_base, &e_different), 0.0);

    // 1. Group face A -> creates person 1 (unnamed)
    let p1 = s
        .cat
        .group_face(faces_a[0].id, &e_base, 0.363)
        .unwrap()
        .unwrap();
    let person1 = s.cat.get_person(p1).unwrap().unwrap();
    assert_eq!(person1.name, None);
    assert_eq!(person1.photo_count, 1);

    // 2. Group face B (similar) -> assigns to person 1
    let p2 = s
        .cat
        .group_face(faces_b[0].id, &e_similar, 0.363)
        .unwrap()
        .unwrap();
    assert_eq!(p1, p2);
    let person1_updated = s.cat.get_person(p1).unwrap().unwrap();
    assert_eq!(person1_updated.photo_count, 2);

    // 3. Group face C (different) -> creates person 2
    let p3 = s
        .cat
        .group_face(faces_c[0].id, &e_different, 0.363)
        .unwrap()
        .unwrap();
    assert_ne!(p1, p3);
    let person2 = s.cat.get_person(p3).unwrap().unwrap();
    assert_eq!(person2.photo_count, 1);

    let all_persons = s.cat.list_persons_in_roots(true, &s.roots()).unwrap();
    assert_eq!(all_persons.len(), 2);
}

#[test]
fn user_assigned_face_is_never_overridden_by_incremental_grouping() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let faces_a = s.cat.asset_face_details(a).unwrap();
    let faces_b = s.cat.asset_face_details(b).unwrap();

    let _p1 = s.cat.create_person(Some("Alice")).unwrap();
    let p2 = s.cat.create_person(Some("Bob")).unwrap();

    // User explicitly assigns face A to Bob
    s.cat
        .assign_face_to_person(faces_a[0].id, p2, true)
        .unwrap();

    // Verify user_assigned is true
    let updated_faces = s.cat.asset_face_details(a).unwrap();
    assert_eq!(updated_faces[0].person_id, Some(p2));
    assert!(updated_faces[0].user_assigned);

    // Group face B with embedding matching Alice
    let e_alice = unit_vector_128(0);
    s.cat.group_face(faces_b[0].id, &e_alice, 0.363).unwrap();

    // Now try to run group_face on face A with Alice's embedding
    let assigned = s
        .cat
        .group_face(faces_a[0].id, &e_alice, 0.363)
        .unwrap()
        .unwrap();
    assert_eq!(assigned, p2, "must keep user assigned person Bob");

    let face_a_after = s.cat.asset_face_details(a).unwrap();
    assert_eq!(face_a_after[0].person_id, Some(p2));
    assert!(face_a_after[0].user_assigned);
}

#[test]
fn merging_persons_reassigns_faces_and_deletes_source_person() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let faces_a = s.cat.asset_face_details(a).unwrap();
    let faces_b = s.cat.asset_face_details(b).unwrap();

    let p_target = s.cat.create_person(Some("Target")).unwrap();
    let p_source = s.cat.create_person(None).unwrap();

    s.cat
        .assign_face_to_person(faces_a[0].id, p_target, false)
        .unwrap();
    s.cat
        .assign_face_to_person(faces_b[0].id, p_source, false)
        .unwrap();

    assert_eq!(s.cat.get_person(p_target).unwrap().unwrap().photo_count, 1);
    assert_eq!(s.cat.get_person(p_source).unwrap().unwrap().photo_count, 1);

    // Merge source into target
    s.cat.merge_persons(p_source, p_target).unwrap();

    // Source is deleted
    assert!(s.cat.get_person(p_source).unwrap().is_none());

    // Target now has 2 photos
    let target = s.cat.get_person(p_target).unwrap().unwrap();
    assert_eq!(target.photo_count, 2);

    // Face B is reassigned to target
    let faces_b_after = s.cat.asset_face_details(b).unwrap();
    assert_eq!(faces_b_after[0].person_id, Some(p_target));
}

#[test]
fn remove_asset_from_person_unassigns_only_that_asset_faces() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let faces_a = s.cat.asset_face_details(a).unwrap();
    let faces_b = s.cat.asset_face_details(b).unwrap();

    let p = s.cat.create_person(Some("Carlos")).unwrap();
    s.cat.assign_face_to_person(faces_a[0].id, p, true).unwrap();
    s.cat.assign_face_to_person(faces_b[0].id, p, true).unwrap();

    assert_eq!(s.cat.get_person(p).unwrap().unwrap().photo_count, 2);

    // Remove asset `a` from Carlos
    s.cat.remove_asset_from_person(a, p).unwrap();

    // Face in `a` is unassigned and locked as a user decision
    let faces_a_after = s.cat.asset_face_details(a).unwrap();
    assert_eq!(faces_a_after[0].person_id, None);
    assert!(faces_a_after[0].user_assigned);

    // Face in `b` is still assigned to Carlos
    let faces_b_after = s.cat.asset_face_details(b).unwrap();
    assert_eq!(faces_b_after[0].person_id, Some(p));
    assert!(faces_b_after[0].user_assigned);

    assert_eq!(s.cat.get_person(p).unwrap().unwrap().photo_count, 1);
}

#[test]
fn unnamed_persons_with_min_photos_proposal_logic() {
    let mut s = setup(&[
        ("1.jpg", b"1"),
        ("2.jpg", b"2"),
        ("3.jpg", b"3"),
        ("4.jpg", b"4"),
    ]);
    let a1 = s.asset("1.jpg");
    let a2 = s.asset("2.jpg");
    let a3 = s.asset("3.jpg");
    let a4 = s.asset("4.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    for a in [a1, a2, a3, a4] {
        s.cat
            .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
            .unwrap();
    }

    let f1 = s.cat.asset_face_details(a1).unwrap()[0].id;
    let f2 = s.cat.asset_face_details(a2).unwrap()[0].id;
    let f3 = s.cat.asset_face_details(a3).unwrap()[0].id;
    let _f4 = s.cat.asset_face_details(a4).unwrap()[0].id;

    let p_unnamed = s.cat.create_person(None).unwrap();
    s.cat.assign_face_to_person(f1, p_unnamed, false).unwrap();
    s.cat.assign_face_to_person(f2, p_unnamed, false).unwrap();

    // With 2 photos, not yet proposed for min_photos = 3
    let candidates = s
        .cat
        .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
        .unwrap();
    assert!(candidates.is_empty());

    // Add 3rd photo
    s.cat.assign_face_to_person(f3, p_unnamed, false).unwrap();
    let candidates = s
        .cat
        .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, p_unnamed);
    assert_eq!(candidates[0].photo_count, 3);

    // If ignored, must not propose
    s.cat.set_person_ignored(p_unnamed, true).unwrap();
    let candidates = s
        .cat
        .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
        .unwrap();
    assert!(candidates.is_empty());

    // Un-ignore
    s.cat.set_person_ignored(p_unnamed, false).unwrap();
    assert_eq!(
        s.cat
            .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
            .unwrap()
            .len(),
        1
    );

    // Once named, must not propose
    s.cat.rename_person(p_unnamed, Some("David")).unwrap();
    assert!(
        s.cat
            .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn clear_face_grouping_data_resets_only_face_grouping_and_keeps_other_data() {
    let mut s = setup(&[("a.jpg", b"1")]);
    let a = s.asset("a.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];

    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let faces = s.cat.asset_face_details(a).unwrap();
    let p = s.cat.create_person(Some("Elena")).unwrap();
    s.cat.assign_face_to_person(faces[0].id, p, true).unwrap();

    // Add a normal tag
    let tag = s.cat.label(LabelKind::Tag, "vacaciones").unwrap();
    s.cat.accept(a, tag).unwrap();

    // Clear face data
    s.cat.clear_face_grouping_data().unwrap();

    // Persons table is empty
    assert!(
        s.cat
            .list_persons_in_roots(true, &s.roots())
            .unwrap()
            .is_empty()
    );

    // Faces still exist in asset_face, but grouping fields are cleared
    let faces_after = s.cat.asset_face_details(a).unwrap();
    assert_eq!(faces_after.len(), 1);
    assert_eq!(faces_after[0].person_id, None);
    assert_eq!(faces_after[0].similarity, None);
    assert!(!faces_after[0].has_embedding);
    assert!(!faces_after[0].user_assigned);

    // The "vacaciones" tag is still intact
    let labels = s.cat.labels_of(a).unwrap();
    assert_eq!(labels.len(), 1);
    assert_eq!(labels[0].name, "vacaciones");
}

#[test]
fn migrating_legacy_asset_face_table_without_person_columns_preserves_faces_and_opens_without_write()
 {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("legacy_faces.sqlite");

    // Old schema where asset_face existed without landmarks/person_id/user_assigned/similarity/embedding
    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE asset (id INTEGER PRIMARY KEY, sha256 TEXT NOT NULL UNIQUE, size INTEGER NOT NULL);
             CREATE TABLE asset_face (
                 id       INTEGER PRIMARY KEY,
                 asset_id INTEGER NOT NULL REFERENCES asset(id),
                 face_idx INTEGER NOT NULL,
                 x1       REAL NOT NULL,
                 y1       REAL NOT NULL,
                 x2       REAL NOT NULL,
                 y2       REAL NOT NULL,
                 score    REAL NOT NULL CHECK (score BETWEEN 0 AND 1),
                 UNIQUE (asset_id, face_idx)
             );
             CREATE INDEX asset_face_asset ON asset_face(asset_id);
             INSERT INTO asset (id, sha256, size) VALUES (1, 'deadbeef', 100);
             INSERT INTO asset_face (asset_id, face_idx, x1, y1, x2, y2, score)
                 VALUES (1, 0, 10.0, 20.0, 110.0, 120.0, 0.91);",
        )
        .unwrap();
    }

    let db = ensure_writable(&db_path, &[]).unwrap();
    let cat = Catalog::open(&db).expect("opening legacy catalog with old asset_face must succeed");
    let faces = cat.asset_faces(AssetId(1)).unwrap();
    assert_eq!(faces.len(), 1);
    assert!((faces[0].score - 0.91).abs() < 1e-5);
    drop(cat);

    // Up-to-date catalog must open without needing a write lock
    let writer = rusqlite::Connection::open(db.as_path()).unwrap();
    writer.execute_batch("BEGIN IMMEDIATE").unwrap();
    let reopened = Catalog::open(&db);
    writer.execute_batch("ROLLBACK").unwrap();
    assert!(reopened.is_ok(), "{:?}", reopened.err());
}

#[test]
fn reanalyzing_asset_preserves_person_id_and_user_assignment() {
    let mut s = setup(&[("a.jpg", b"1")]);
    let a = s.asset("a.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 110.0, 110.0], 0.95)];
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let face_id = s.cat.asset_face_details(a).unwrap()[0].id;
    let p = s.cat.create_person(Some("Lucía")).unwrap();
    s.cat.assign_face_to_person(face_id, p, true).unwrap();

    // Re-analyze the same photo with slightly shifted bbox (high IoU)
    let updated_face = vec![DetectedFace::new([12.0, 11.0, 111.0, 109.0], 0.96)];
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &updated_face)
        .unwrap();

    let after = s.cat.asset_face_details(a).unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].person_id,
        Some(p),
        "person_id must survive re-analysis"
    );
    assert!(
        after[0].user_assigned,
        "user_assigned must survive re-analysis"
    );
}

#[test]
fn removed_asset_is_not_readded_by_future_grouping_and_merged_faces_stay_locked() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &face)
        .unwrap();

    let fa = s.cat.asset_face_details(a).unwrap()[0].id;
    let fb = s.cat.asset_face_details(b).unwrap()[0].id;

    let e0 = unit_vector_128(0);
    let e1 = unit_vector_128(5);
    let pa = s.cat.group_face(fa, &e0, 0.363).unwrap().unwrap();
    let pb = s.cat.group_face(fb, &e1, 0.363).unwrap().unwrap();
    assert_ne!(pa, pb);

    // User merges pb into pa -> face b must become user_assigned so re-grouping never splits it
    s.cat.merge_persons(pb, pa).unwrap();
    let fb_after_merge = s.cat.asset_face_details(b).unwrap()[0].clone();
    assert_eq!(fb_after_merge.person_id, Some(pa));
    assert!(
        fb_after_merge.user_assigned,
        "merged face must be locked as user_assigned"
    );

    // User removes asset `a` from `pa` -> must be marked user_assigned = 1 with person_id = None
    s.cat.remove_asset_from_person(a, pa).unwrap();
    let fa_after_remove = s.cat.asset_face_details(a).unwrap()[0].clone();
    assert_eq!(fa_after_remove.person_id, None);
    assert!(
        fa_after_remove.user_assigned,
        "removed face must stay user_assigned so grouping does not re-add it"
    );

    // Future grouping on removed face must NOT re-add it to any person
    let grouped_again = s.cat.group_face(fa, &e0, 0.363).unwrap();
    assert_eq!(grouped_again, None);
    assert_eq!(s.cat.asset_face_details(a).unwrap()[0].person_id, None);
}

#[test]
fn multiple_faces_of_same_person_in_one_photo_count_as_one_photo_and_hidden_not_proposed() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2"), ("c.jpg", b"3")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let c = s.asset("c.jpg");

    let emb = vec![0.0f32; EMBEDDING_DIM];
    // Asset `a` has 3 faces of the same person
    let three_faces = vec![
        DetectedFace::new([10.0, 10.0, 60.0, 60.0], 0.95),
        DetectedFace::new([70.0, 10.0, 120.0, 60.0], 0.94),
        DetectedFace::new([130.0, 10.0, 180.0, 60.0], 0.93),
    ];
    s.cat
        .save_vision_result(a, SIGLIP2_MODEL_ID, &emb, &three_faces)
        .unwrap();
    s.cat
        .save_vision_result(b, SIGLIP2_MODEL_ID, &emb, &three_faces[..1])
        .unwrap();
    s.cat
        .save_vision_result(c, SIGLIP2_MODEL_ID, &emb, &three_faces[..1])
        .unwrap();

    let e0 = unit_vector_128(0);
    let faces_a = s.cat.asset_face_details(a).unwrap();
    let p0 = s
        .cat
        .group_face(faces_a[0].id, &e0, 0.363)
        .unwrap()
        .unwrap();
    let p1 = s
        .cat
        .group_face(faces_a[1].id, &e0, 0.363)
        .unwrap()
        .unwrap();
    let p2 = s
        .cat
        .group_face(faces_a[2].id, &e0, 0.363)
        .unwrap()
        .unwrap();
    assert_eq!(p0, p1);
    assert_eq!(p0, p2);

    // 3 faces in 1 photo = 1 distinct photo, NOT 3 -> must NOT propose naming
    assert_eq!(s.cat.get_person(p0).unwrap().unwrap().photo_count, 1);
    assert!(
        s.cat
            .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
            .unwrap()
            .is_empty()
    );

    // Add 2 more distinct photos -> now 3 distinct photos -> proposed
    let fb = s.cat.asset_face_details(b).unwrap()[0].id;
    let fc = s.cat.asset_face_details(c).unwrap()[0].id;
    s.cat.group_face(fb, &e0, 0.363).unwrap();
    s.cat.group_face(fc, &e0, 0.363).unwrap();
    assert_eq!(
        s.cat
            .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
            .unwrap()
            .len(),
        1
    );

    // Hidden person must NOT be proposed
    s.cat.set_person_hidden(p0, true).unwrap();
    assert!(
        s.cat
            .unnamed_persons_with_min_photos_in_roots(3, &s.roots())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn only_clear_faces_of_photos_are_grouped() {
    let mut s = setup(&[
        ("photo.jpg", b"1"),
        ("manga.jpg", b"2"),
        ("blurry.jpg", b"3"),
        ("tiny.jpg", b"4"),
    ]);
    let emb = vec![0.0f32; EMBEDDING_DIM];
    for (name, bbox, score) in [
        ("photo.jpg", [10.0, 10.0, 100.0, 100.0], 0.95),
        ("manga.jpg", [10.0, 10.0, 100.0, 100.0], 0.95),
        ("blurry.jpg", [10.0, 10.0, 100.0, 100.0], 0.85),
        ("tiny.jpg", [10.0, 10.0, 30.0, 30.0], 0.95),
    ] {
        let face = vec![DetectedFace::new(bbox, score)];
        s.cat
            .save_vision_result(s.asset(name), SIGLIP2_MODEL_ID, &emb, &face)
            .unwrap();
    }
    let manga = s
        .cat
        .label(catalog::LabelKind::Category, catalog::TYPE_MANGA)
        .unwrap();
    s.cat.accept(s.asset("manga.jpg"), manga).unwrap();
    assert_eq!(
        s.cat.assets_with_faces_needing_embeddings().unwrap(),
        vec![s.asset("photo.jpg")]
    );
}

/// The face grouping worker writes with its own connection while the user edits persons: each
/// read-then-write must wait for the other writer instead of failing with "database is locked".
#[test]
fn person_edits_and_grouping_wait_for_another_writer() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let face = vec![DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.95)];
    let emb = vec![0.0f32; EMBEDDING_DIM];
    for asset in [a, b] {
        s.cat
            .save_vision_result(asset, SIGLIP2_MODEL_ID, &emb, &face)
            .unwrap();
    }
    let face_a = s.cat.asset_face_details(a).unwrap()[0].id;
    let face_b = s.cat.asset_face_details(b).unwrap()[0].id;
    let source = s.cat.create_person(None).unwrap();
    let target = s.cat.create_person(Some("Target")).unwrap();
    s.cat.assign_face_to_person(face_a, source, false).unwrap();

    let hold_write_lock = |db: &Path| {
        let writer = rusqlite::Connection::open(db).unwrap();
        let (locked_tx, locked) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            writer.execute_batch("BEGIN IMMEDIATE").unwrap();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(300));
            writer.execute_batch("COMMIT").unwrap();
        });
        locked.recv().unwrap();
        handle
    };

    let db = s._db.as_path().to_path_buf();
    let other = hold_write_lock(&db);
    s.cat.merge_persons(source, target).unwrap();
    other.join().unwrap();

    let other = hold_write_lock(&db);
    s.cat.group_face(face_b, &unit_vector_128(0), 0.5).unwrap();
    other.join().unwrap();

    assert_eq!(
        s.cat.asset_face_details(a).unwrap()[0].person_id,
        Some(target)
    );
    assert!(s.cat.asset_face_details(b).unwrap()[0].person_id.is_some());
}

#[test]
fn person_covers_give_each_persons_best_face_with_its_photo_in_one_query() {
    let mut s = setup(&[("a.jpg", b"1"), ("b.jpg", b"2")]);
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let emb = vec![0.0f32; EMBEDDING_DIM];
    s.cat
        .save_vision_result(
            a,
            SIGLIP2_MODEL_ID,
            &emb,
            &[DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.92)],
        )
        .unwrap();
    s.cat
        .save_vision_result(
            b,
            SIGLIP2_MODEL_ID,
            &emb,
            &[DetectedFace::new([10.0, 10.0, 100.0, 100.0], 0.97)],
        )
        .unwrap();
    let face_a = s.cat.asset_face_details(a).unwrap()[0].id;
    let face_b = s.cat.asset_face_details(b).unwrap()[0].id;
    let p = s.cat.create_person(Some("Carlos")).unwrap();
    let empty = s.cat.create_person(None).unwrap();
    s.cat.assign_face_to_person(face_a, p, true).unwrap();
    s.cat.assign_face_to_person(face_b, p, true).unwrap();

    let covers = s.cat.person_covers_in_roots(&s.roots()).unwrap();

    assert_eq!(covers.len(), 1, "a person without faces has no cover");
    let (face, path, sha256) = &covers[0];
    assert_eq!(face.id, face_b);
    assert_eq!(face.person_id, Some(p));
    assert_ne!(face.person_id, Some(empty));
    let entry = s.cat.entry_for(b).unwrap().unwrap();
    assert_eq!(path.as_ref(), Some(&entry.path));
    assert_eq!(sha256, &entry.sha256);
}

#[test]
fn persons_only_show_what_the_registered_folders_hold() {
    let mut s = setup(&[("a.jpg", b"1")]);
    let other = s._tmp.path().join("other");
    write_file(&other, "b.jpg", b"2");
    write_file(&other, "c.jpg", b"3");
    let other = LibraryRoot::new(&other).unwrap();
    s.cat.index(&other).unwrap();
    let first = s.roots();
    let both = vec![first[0].clone(), other.as_path().to_path_buf()];

    let asset_in_other = |s: &Setup, rel: &str| {
        let path = other.as_path().join(rel);
        s.cat.asset_of(&path).unwrap().unwrap()
    };
    let a = s.asset("a.jpg");
    let b = asset_in_other(&s, "b.jpg");
    let c = asset_in_other(&s, "c.jpg");
    let emb = vec![0.0f32; EMBEDDING_DIM];
    // The best face of the first person is in the second folder.
    for (asset, score) in [(a, 0.92), (b, 0.99), (c, 0.95)] {
        let face = [DetectedFace::new([10.0, 10.0, 100.0, 100.0], score)];
        s.cat
            .save_vision_result(asset, SIGLIP2_MODEL_ID, &emb, &face)
            .unwrap();
    }
    let ana = s.cat.create_person(Some("Ana")).unwrap();
    let luis = s.cat.create_person(Some("Luis")).unwrap();
    for (asset, person) in [(a, ana), (b, ana), (c, luis)] {
        let face = s.cat.asset_face_details(asset).unwrap()[0].id;
        s.cat.assign_face_to_person(face, person, true).unwrap();
    }

    let persons = s.cat.list_persons_in_roots(true, &first).unwrap();
    assert_eq!(persons.len(), 1);
    assert_eq!(persons[0].id, ana);
    assert_eq!(persons[0].photo_count, 1);
    assert_eq!(persons[0].cover_asset_id, Some(a));
    let covers = s.cat.person_covers_in_roots(&first).unwrap();
    assert_eq!(covers.len(), 1);
    assert_eq!(covers[0].0.asset_id, a);
    assert!(covers[0].1.as_ref().unwrap().starts_with(&first[0]));
    assert_eq!(s.cat.person_faces_in_roots(ana, &first).unwrap().len(), 1);
    assert!(
        s.cat
            .person_faces_in_roots(luis, &first)
            .unwrap()
            .is_empty()
    );

    let persons = s.cat.list_persons_in_roots(true, &both).unwrap();
    assert_eq!(persons.len(), 2);
    assert_eq!(persons[0].photo_count, 2);
    assert_eq!(persons[0].cover_asset_id, Some(b));
    assert_eq!(persons[1].name.as_deref(), Some("Luis"));

    assert!(s.cat.list_persons_in_roots(true, &[]).unwrap().is_empty());
    assert!(s.cat.person_covers_in_roots(&[]).unwrap().is_empty());
    assert!(s.cat.person_faces_in_roots(ana, &[]).unwrap().is_empty());
}
