// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use fsguard::{GuardError, LibraryRoot, ensure_export_dir, ensure_writable};
use std::ffi::OsStr;
use std::fs;
use tempfile::TempDir;

fn fixture() -> (TempDir, LibraryRoot) {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    fs::create_dir_all(tmp.path().join("export")).unwrap();
    let root = LibraryRoot::new(&library).unwrap();
    (tmp, root)
}

#[test]
fn an_export_folder_that_contains_a_library_is_refused() {
    let (tmp, root) = fixture();
    let result = ensure_export_dir(tmp.path(), std::slice::from_ref(&root));
    assert!(matches!(result, Err(GuardError::ContainsLibrary { .. })));
}

#[test]
fn an_export_folder_inside_a_library_is_refused() {
    let (tmp, root) = fixture();
    let result = ensure_export_dir(&tmp.path().join("library").join("out"), &[root]);
    assert!(matches!(result, Err(GuardError::InsideLibrary { .. })));
}

#[test]
fn an_export_folder_beside_the_library_is_accepted() {
    let (tmp, root) = fixture();
    let dest = ensure_export_dir(&tmp.path().join("export"), &[root]).unwrap();
    assert!(dest.as_path().ends_with("export"));
}

#[test]
fn join_name_accepts_a_plain_file_name() {
    let (tmp, root) = fixture();
    let dir = ensure_writable(&tmp.path().join("export"), &[root]).unwrap();
    let child = dir.join_name(OsStr::new("photo (1).jpg")).unwrap();
    assert!(child.as_path().ends_with("photo (1).jpg"));
}

#[test]
fn join_name_refuses_anything_that_is_not_a_single_name() {
    let (tmp, root) = fixture();
    let dir = ensure_writable(&tmp.path().join("export"), &[root]).unwrap();
    for bad in [
        "",
        ".",
        "..",
        "a/b.jpg",
        "a\\b.jpg",
        "C:\\x.jpg",
        "C:x.jpg",
        "\\x.jpg",
    ] {
        assert!(
            matches!(
                dir.join_name(OsStr::new(bad)),
                Err(GuardError::InvalidName(_))
            ),
            "{bad:?} was accepted"
        );
    }
}
