// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Catalog, DuplicateGroup};
use fsguard::{LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    root: LibraryRoot,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup_with(files: &[(&str, &[u8])]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    for (rel, bytes) in files {
        write(&library, rel, bytes);
    }
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
        cat,
    }
}

impl Setup {
    fn rel_paths(&self, group: &DuplicateGroup) -> Vec<String> {
        let base = dunce::canonicalize(&self.library).unwrap();
        group
            .entries
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
fn library_without_duplicates_returns_empty_groups() {
    let s = setup_with(&[
        ("a.jpg", b"one"),
        ("b.png", b"two"),
        ("sub/c.webp", b"three"),
    ]);
    assert!(s.cat.duplicate_groups().unwrap().is_empty());
}

#[test]
fn duplicate_groups_returns_entries_ordered_by_path_with_metadata() {
    let s = setup_with(&[
        ("z/copy.jpg", b"identical-bytes"),
        ("a/orig.jpg", b"identical-bytes"),
        ("m/unique.png", b"only-once"),
    ]);

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    let expected_sha = scanner::sha256_file(&s.library.join("a/orig.jpg")).unwrap();
    assert_eq!(g.sha256, expected_sha);
    assert_eq!(g.size, b"identical-bytes".len() as u64);
    assert_eq!(s.rel_paths(g), vec!["a/orig.jpg", "z/copy.jpg"]);
    assert!(g.entries.iter().all(|e| e.sha256 == expected_sha));
    assert!(g.entries.iter().all(|e| e.size == g.size));
    assert_eq!(g.entries[0].asset, g.entries[1].asset);
    assert!(g.entries.iter().all(|e| e.modified_ns.is_some()));
}

#[test]
fn duplicate_groups_orders_by_wasted_bytes_desc_then_sha256_asc() {
    let small_3x = [b'S'; 10]; // 3 copies -> wasted = 20
    let tie_2x = [b'T'; 20]; // 2 copies -> wasted = 20 (ties small_3x)
    let big_2x = [b'B'; 25]; // 2 copies -> wasted = 25 (highest)
    let tiny_4x = [b'X'; 4]; // 4 copies -> wasted = 12 (lowest)

    let s = setup_with(&[
        ("s/1.jpg", &small_3x),
        ("s/2.jpg", &small_3x),
        ("s/3.jpg", &small_3x),
        ("t/1.jpg", &tie_2x),
        ("t/2.jpg", &tie_2x),
        ("b/1.jpg", &big_2x),
        ("b/2.jpg", &big_2x),
        ("x/1.jpg", &tiny_4x),
        ("x/2.jpg", &tiny_4x),
        ("x/3.jpg", &tiny_4x),
        ("x/4.jpg", &tiny_4x),
    ]);

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 4);

    let sha_small = scanner::sha256_file(&s.library.join("s/1.jpg")).unwrap();
    let sha_tie = scanner::sha256_file(&s.library.join("t/1.jpg")).unwrap();
    let sha_big = scanner::sha256_file(&s.library.join("b/1.jpg")).unwrap();
    let sha_tiny = scanner::sha256_file(&s.library.join("x/1.jpg")).unwrap();

    let mut tied = [sha_small, sha_tie];
    tied.sort();

    let actual_shas: Vec<&str> = groups.iter().map(|g| g.sha256.as_str()).collect();
    assert_eq!(
        actual_shas,
        vec![
            sha_big.as_str(),
            tied[0].as_str(),
            tied[1].as_str(),
            sha_tiny.as_str(),
        ]
    );
}

#[test]
fn missing_locations_are_excluded_and_shrink_or_drop_groups() {
    let big_3x = [b'A'; 10]; // 3 copies -> wasted 20; after removing 1 -> wasted 10
    let mid_2x = [b'B'; 15]; // 2 copies -> wasted 15
    let pair_2x = [b'C'; 30]; // 2 copies -> wasted 30; after removing 1 -> dropped

    let mut s = setup_with(&[
        ("a/1.jpg", &big_3x),
        ("a/2.jpg", &big_3x),
        ("a/3.jpg", &big_3x),
        ("b/1.jpg", &mid_2x),
        ("b/2.jpg", &mid_2x),
        ("c/1.jpg", &pair_2x),
        ("c/2.jpg", &pair_2x),
    ]);

    assert_eq!(s.cat.duplicate_groups().unwrap().len(), 3);

    // Delete one copy from `c` (leaving 1 copy -> no longer a duplicate group)
    // and one copy from `a` (leaving 2 copies -> wasted drops from 20 to 10, below `b`'s 15).
    fs::remove_file(s.library.join("c/2.jpg")).unwrap();
    fs::remove_file(s.library.join("a/2.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let groups = s.cat.duplicate_groups().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(s.rel_paths(&groups[0]), vec!["b/1.jpg", "b/2.jpg"]);
    assert_eq!(s.rel_paths(&groups[1]), vec!["a/1.jpg", "a/3.jpg"]);
}

#[test]
fn near_duplicates_separates_similar_from_distinct_and_excludes_removed_roots() {
    use catalog::{EMBEDDING_DIM, SIGLIP2_MODEL_ID};
    use std::sync::atomic::AtomicBool;

    let tmp = tempfile::tempdir().unwrap();
    let lib_a = tmp.path().join("lib_a");
    let lib_b = tmp.path().join("lib_b");
    write(&lib_a, "sim1.jpg", b"similar-one");
    write(&lib_a, "sim2.jpg", b"similar-two");
    write(&lib_a, "sim1_exact_copy.jpg", b"similar-one"); // exact duplicate of sim1 -> same asset
    write(&lib_a, "distinct.jpg", b"very-different");
    write(&lib_b, "removed_sim.jpg", b"similar-in-other-root");

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
    let [root_a, root_b] = roots_guard;

    let asset = |p: PathBuf| {
        cat.asset_of(&dunce::canonicalize(p).unwrap())
            .unwrap()
            .unwrap()
    };
    let a1 = asset(lib_a.join("sim1.jpg"));
    let a2 = asset(lib_a.join("sim2.jpg"));
    let a_dist = asset(lib_a.join("distinct.jpg"));
    let a_b = asset(lib_b.join("removed_sim.jpg"));

    let normalize = |mut v: Vec<f32>| {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        for x in &mut v {
            *x /= n;
        }
        v
    };

    // a1 and a2 have cosine ~0.97; a_b is also ~0.97 to a1; a_dist is orthogonal (cosine 0.0).
    let mut v1 = vec![0.0f32; EMBEDDING_DIM];
    v1[0] = 1.0;
    let mut v2 = vec![0.0f32; EMBEDDING_DIM];
    v2[0] = 0.97;
    v2[1] = 0.243;
    let mut vb = vec![0.0f32; EMBEDDING_DIM];
    vb[0] = 0.97;
    vb[2] = 0.243;
    let mut vd = vec![0.0f32; EMBEDDING_DIM];
    vd[5] = 1.0;

    cat.save_vision_result(a1, SIGLIP2_MODEL_ID, &normalize(v1), &[])
        .unwrap();
    cat.save_vision_result(a2, SIGLIP2_MODEL_ID, &normalize(v2), &[])
        .unwrap();
    cat.save_vision_result(a_b, SIGLIP2_MODEL_ID, &normalize(vb), &[])
        .unwrap();
    cat.save_vision_result(a_dist, SIGLIP2_MODEL_ID, &normalize(vd), &[])
        .unwrap();

    let cancel = AtomicBool::new(false);
    let both_roots = vec![
        root_a.as_path().to_path_buf(),
        root_b.as_path().to_path_buf(),
    ];

    // Threshold 0.95 groups a1, a2, a_b together and excludes a_dist.
    let groups = cat
        .compute_and_save_near_duplicates_in_roots(
            SIGLIP2_MODEL_ID,
            &both_roots,
            0.95,
            &cancel,
            &|_| {},
        )
        .unwrap()
        .unwrap();
    assert_eq!(groups.len(), 1);
    let ids: Vec<_> = groups[0].entries.iter().map(|e| e.asset).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&a1) && ids.contains(&a2) && ids.contains(&a_b));
    assert!(!ids.contains(&a_dist));

    // After removing root_b, reading cached groups with only root_a excludes a_b.
    let only_a = vec![root_a.as_path().to_path_buf()];
    let filtered = cat.near_duplicate_groups_in_roots(&only_a).unwrap();
    assert_eq!(filtered.len(), 1);
    let ids_a: Vec<_> = filtered[0].entries.iter().map(|e| e.asset).collect();
    assert_eq!(ids_a.len(), 2);
    assert!(ids_a.contains(&a1) && ids_a.contains(&a2));
    assert!(!ids_a.contains(&a_b));

    // Higher threshold (0.99) yields no groups because the similar pair is ~0.97.
    let strict = cat
        .compute_and_save_near_duplicates_in_roots(
            SIGLIP2_MODEL_ID,
            &only_a,
            0.99,
            &cancel,
            &|_| {},
        )
        .unwrap()
        .unwrap();
    assert!(strict.is_empty());
}
