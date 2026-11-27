// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{AssetId, Catalog, Pending};
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

/// A library with three different images, already indexed.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "memes/funny.jpg", b"one");
    write(&library, "shots/chat.png", b"two");
    write(&library, "manga/page.webp", b"three");
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
    fn asset(&self, rel: &str) -> AssetId {
        let path = dunce::canonicalize(self.library.join(rel)).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }

    fn canonical(&self, rel: &str) -> PathBuf {
        dunce::canonicalize(self.library.join(rel)).unwrap()
    }
}

#[test]
fn everything_is_pending_at_first_ordered_by_asset_id() {
    let s = setup();
    let pending: Vec<Pending> = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 3);
    assert!(pending.windows(2).all(|w| w[0].asset.0 < w[1].asset.0));
    for item in &pending {
        assert_eq!(s.cat.asset_of(&item.path).unwrap(), Some(item.asset));
    }
}

#[test]
fn after_mark_done_nothing_is_pending() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
}

#[test]
fn bumping_the_version_makes_everything_pending_again() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
    assert_eq!(s.cat.pending("rules", 2, 10).unwrap().len(), 3);
}

#[test]
fn mark_done_never_lowers_a_recorded_version() {
    let s = setup();
    let funny = s.asset("memes/funny.jpg");
    s.cat.mark_done(funny, "rules", 3).unwrap();
    s.cat.mark_done(funny, "rules", 1).unwrap();
    let pending_v2 = s.cat.pending("rules", 2, 10).unwrap();
    assert!(!pending_v2.iter().any(|p| p.asset == funny));
    let pending_v3 = s.cat.pending("rules", 3, 10).unwrap();
    assert!(!pending_v3.iter().any(|p| p.asset == funny));
    let pending_v4 = s.cat.pending("rules", 4, 10).unwrap();
    assert!(pending_v4.iter().any(|p| p.asset == funny));
}

#[test]
fn a_new_file_appears_as_pending_while_done_ones_stay_done() {
    let mut s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    write(&s.library, "new/extra.jpg", b"four");
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].asset, s.asset("new/extra.jpg"));
    assert_eq!(pending[0].path, s.canonical("new/extra.jpg"));
}

#[test]
fn missing_files_are_not_pending() {
    let mut s = setup();
    let funny = s.asset("memes/funny.jpg");
    fs::remove_file(s.library.join("memes/funny.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    assert_eq!(pending.len(), 2);
    assert!(!pending.iter().any(|p| p.asset == funny));
}

#[test]
fn duplicate_locations_yield_the_smallest_non_missing_path() {
    let mut s = setup();
    write(&s.library, "aaa/copy.jpg", b"one");
    write(&s.library, "zzz/copy.jpg", b"one");
    s.cat.index(&s.root).unwrap();
    let asset = s.asset("memes/funny.jpg");

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    let item = pending.iter().find(|p| p.asset == asset).unwrap();
    assert_eq!(item.path, s.canonical("aaa/copy.jpg"));

    fs::remove_file(s.library.join("aaa/copy.jpg")).unwrap();
    s.cat.index(&s.root).unwrap();

    let pending = s.cat.pending("rules", 1, 10).unwrap();
    let item = pending.iter().find(|p| p.asset == asset).unwrap();
    assert_eq!(item.path, s.canonical("memes/funny.jpg"));
}

#[test]
fn limit_is_respected() {
    let s = setup();
    assert!(s.cat.pending("rules", 1, 0).unwrap().is_empty());

    let first_two = s.cat.pending("rules", 1, 2).unwrap();
    assert_eq!(first_two.len(), 2);

    for item in &first_two {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    let remaining = s.cat.pending("rules", 1, 2).unwrap();
    assert_eq!(remaining.len(), 1);
}

#[test]
fn stages_are_independent() {
    let s = setup();
    for item in s.cat.pending("rules", 1, 10).unwrap() {
        s.cat.mark_done(item.asset, "rules", 1).unwrap();
    }
    assert!(s.cat.pending("rules", 1, 10).unwrap().is_empty());
    assert_eq!(s.cat.pending("ocr", 1, 10).unwrap().len(), 3);
}

#[test]
fn stage_errors_persist_sanitize_increment_clear_and_hide_removed_folders() {
    use catalog::{OCR_STAGE, VISION_STAGE, sanitize_stage_error_reason};

    // Sanitizer must NEVER leak OCR text, paths, or filenames.
    let nasty =
        r"failed reading C:\Users\Private\secret_photo_99.jpg: OCR text 'Contraseña secreta 12345'";
    let clean = sanitize_stage_error_reason(OCR_STAGE, nasty);
    assert!(!clean.contains("Private"));
    assert!(!clean.contains("secret_photo"));
    assert!(!clean.contains("Contraseña"));
    assert!(!clean.contains("12345"));

    let tmp = tempfile::tempdir().unwrap();
    let lib_a = tmp.path().join("lib_a");
    let lib_b = tmp.path().join("lib_b");
    write(&lib_a, "a.jpg", b"img-a");
    write(&lib_b, "b.jpg", b"img-b");
    let roots_guard = [
        LibraryRoot::new(&lib_a).unwrap(),
        LibraryRoot::new(&lib_b).unwrap(),
    ];
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        &roots_guard,
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&roots_guard[0]).unwrap();
    cat.index(&roots_guard[1]).unwrap();
    let root_a = &roots_guard[0];
    let root_b = &roots_guard[1];

    let a = cat
        .asset_of(&dunce::canonicalize(lib_a.join("a.jpg")).unwrap())
        .unwrap()
        .unwrap();
    let b = cat
        .asset_of(&dunce::canonicalize(lib_b.join("b.jpg")).unwrap())
        .unwrap()
        .unwrap();

    cat.record_stage_error(a, VISION_STAGE, r"decode failed on C:\secret\a.jpg")
        .unwrap();
    cat.record_stage_error(a, VISION_STAGE, r"decode failed on C:\secret\a.jpg")
        .unwrap();
    cat.record_stage_error(b, OCR_STAGE, "timeout reading secret OCR text")
        .unwrap();

    let both = vec![
        root_a.as_path().to_path_buf(),
        root_b.as_path().to_path_buf(),
    ];
    let only_a = vec![root_a.as_path().to_path_buf()];

    assert_eq!(cat.count_stage_error_assets_in_roots(&both).unwrap(), 2);
    assert_eq!(cat.count_stage_error_assets_in_roots(&only_a).unwrap(), 1);

    let list_a = cat.list_stage_errors_in_roots(&only_a).unwrap();
    assert_eq!(list_a.len(), 1);
    assert_eq!(list_a[0].asset, a);
    assert_eq!(list_a[0].stage, VISION_STAGE);
    assert_eq!(list_a[0].attempts, 2);
    assert_eq!(list_a[0].reason, "No se pudo leer la imagen");
    assert!(list_a[0].last_at_secs > 0);

    // Succeeding on that stage clears the error automatically.
    cat.mark_done(a, VISION_STAGE, 1).unwrap();
    assert_eq!(cat.count_stage_error_assets_in_roots(&only_a).unwrap(), 0);
    assert!(cat.list_stage_errors_in_roots(&only_a).unwrap().is_empty());

    // Re-review (`reset_stages`) also clears persisted errors for that asset/stage.
    assert_eq!(cat.count_stage_error_assets_in_roots(&both).unwrap(), 1);
    cat.reset_stages(&[b], &[OCR_STAGE]).unwrap();
    assert_eq!(cat.count_stage_error_assets_in_roots(&both).unwrap(), 0);
}

#[test]
fn missing_video_codec_error_sanitizes_to_dedicated_reason() {
    assert_eq!(
        catalog::sanitize_stage_error_reason(
            catalog::VISION_STAGE,
            "no se pudieron extraer fotogramas del vídeo: unsupported image: códec no disponible o formato no compatible: 0xC00D36C4"
        ),
        "Códec de vídeo no disponible"
    );
    assert_eq!(
        catalog::sanitize_stage_error_reason(
            "rules",
            "unsupported image: códec de vídeo no compatible"
        ),
        "Códec de vídeo no disponible"
    );
}

#[test]
fn missing_image_codec_error_sanitizes_to_dedicated_reason() {
    assert_eq!(
        catalog::sanitize_stage_error_reason(
            catalog::VISION_STAGE,
            "unsupported image: códec de imagen no disponible: 0x88982F50"
        ),
        "Códec de imagen no disponible"
    );
    assert_eq!(
        catalog::sanitize_stage_error_reason(
            "rules",
            "unsupported image: códec de imagen no disponible"
        ),
        "Códec de imagen no disponible"
    );
}
