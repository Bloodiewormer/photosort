// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use fsguard::{GuardError, LibraryRoot, ensure_writable};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    library: PathBuf,
    roots: Vec<LibraryRoot>,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(library.join("manga")).unwrap();
    fs::write(library.join("manga").join("page1.jpg"), b"x").unwrap();
    fs::create_dir_all(tmp.path().join("export")).unwrap();
    let roots = vec![LibraryRoot::new(&library).unwrap()];
    Fixture {
        tmp,
        library,
        roots,
    }
}

fn is_inside_library<T: std::fmt::Debug>(r: Result<T, GuardError>) -> bool {
    matches!(r, Err(GuardError::InsideLibrary { .. }))
}

#[test]
fn rejects_an_existing_original() {
    let f = fixture();
    let target = f.library.join("manga").join("page1.jpg");
    assert!(is_inside_library(ensure_writable(&target, &f.roots)));
}

#[test]
fn rejects_a_new_file_in_a_directory_that_does_not_exist_yet() {
    let f = fixture();
    let target = f.library.join("new_dir").join("deeper").join("a.txt");
    assert!(is_inside_library(ensure_writable(&target, &f.roots)));
}

#[test]
fn rejects_the_root_itself() {
    let f = fixture();
    assert!(is_inside_library(ensure_writable(&f.library, &f.roots)));
}

#[test]
fn accepts_a_path_outside_every_root() {
    let f = fixture();
    let target = f.tmp.path().join("export").join("a.txt");
    let dest = ensure_writable(&target, &f.roots).unwrap();
    assert!(dest.as_path().ends_with("a.txt"));
}

#[test]
fn accepts_a_sibling_whose_name_starts_like_the_root() {
    let f = fixture();
    let sibling = f.tmp.path().join("library2");
    fs::create_dir_all(&sibling).unwrap();
    assert!(ensure_writable(&sibling.join("a.txt"), &f.roots).is_ok());
}

#[test]
fn rejects_a_path_inside_any_of_several_roots() {
    let mut f = fixture();
    let second = f.tmp.path().join("scans");
    fs::create_dir_all(&second).unwrap();
    f.roots.push(LibraryRoot::new(&second).unwrap());
    assert!(is_inside_library(ensure_writable(
        &second.join("x.png"),
        &f.roots
    )));
}

#[test]
fn rejects_parent_dir_components_instead_of_guessing() {
    let f = fixture();
    let sneaky = f
        .tmp
        .path()
        .join("export")
        .join("..")
        .join("library")
        .join("x.txt");
    assert!(matches!(
        ensure_writable(&sneaky, &f.roots),
        Err(GuardError::ParentDirComponent(_))
    ));
}

#[test]
fn a_library_root_must_exist() {
    let f = fixture();
    assert!(LibraryRoot::new(&f.tmp.path().join("does-not-exist")).is_err());
}

#[cfg(windows)]
#[test]
fn rejects_the_same_path_written_with_other_capitalization() {
    let f = fixture();
    let shouting = PathBuf::from(f.library.to_string_lossy().to_uppercase()).join("NEW.TXT");
    assert!(is_inside_library(ensure_writable(&shouting, &f.roots)));
}

#[cfg(windows)]
#[test]
fn rejects_a_path_reached_through_a_junction_into_the_library() {
    let f = fixture();
    let link = f.tmp.path().join("export").join("shortcut");
    let status = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&f.library)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "could not create junction: {status:?}"
    );
    assert!(is_inside_library(ensure_writable(
        &link.join("new.txt"),
        &f.roots
    )));
}
