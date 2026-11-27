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
    let p1 = s.cat.group_face(faces_a[0].id, &e_base, 0.363).unwrap();
    let person1 = s.cat.get_person(p1).unwrap().unwrap();
    assert_eq!(person1.name, None);
    assert_eq!(person1.photo_count, 1);

    // 2. Group face B (similar) -> assigns to person 1
    let p2 = s.cat.group_face(faces_b[0].id, &e_similar, 0.363).unwrap();
    assert_eq!(p1, p2);
    let person1_updated = s.cat.get_person(p1).unwrap().unwrap();
    assert_eq!(person1_updated.photo_count, 2);

    // 3. Group face C (different) -> creates person 2
    let p3 = s
        .cat
        .group_face(faces_c[0].id, &e_different, 0.363)
        .unwrap();
    assert_ne!(p1, p3);
    let person2 = s.cat.get_person(p3).unwrap().unwrap();
    assert_eq!(person2.photo_count, 1);

    let all_persons = s.cat.list_persons(true).unwrap();
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
    let assigned = s.cat.group_face(faces_a[0].id, &e_alice, 0.363).unwrap();
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

    // Face in `a` is unassigned
    let faces_a_after = s.cat.asset_face_details(a).unwrap();
    assert_eq!(faces_a_after[0].person_id, None);
    assert!(!faces_a_after[0].user_assigned);

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
    let candidates = s.cat.unnamed_persons_with_min_photos(3).unwrap();
    assert!(candidates.is_empty());

    // Add 3rd photo
    s.cat.assign_face_to_person(f3, p_unnamed, false).unwrap();
    let candidates = s.cat.unnamed_persons_with_min_photos(3).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, p_unnamed);
    assert_eq!(candidates[0].photo_count, 3);

    // If ignored, must not propose
    s.cat.set_person_ignored(p_unnamed, true).unwrap();
    let candidates = s.cat.unnamed_persons_with_min_photos(3).unwrap();
    assert!(candidates.is_empty());

    // Un-ignore
    s.cat.set_person_ignored(p_unnamed, false).unwrap();
    assert_eq!(s.cat.unnamed_persons_with_min_photos(3).unwrap().len(), 1);

    // Once named, must not propose
    s.cat.rename_person(p_unnamed, Some("David")).unwrap();
    assert!(s.cat.unnamed_persons_with_min_photos(3).unwrap().is_empty());
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
    assert!(s.cat.list_persons(true).unwrap().is_empty());

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
