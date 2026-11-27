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

#[cfg(windows)]
#[test]
fn links_that_appear_under_a_validated_folder_are_refused() {
    let f = fixture();
    let export = f.tmp.path().join("export");
    let elsewhere = f.tmp.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(export.join("real")).unwrap();
    let dest = ensure_writable(&export, &f.roots).unwrap();
    let later = dest.join_name("later".as_ref()).unwrap();
    for (name, target) in [
        ("later", f.library.clone()),
        ("outside", elsewhere),
        ("inside", export.join("real")),
        // Its target does not exist, so nothing proves where it leads.
        ("broken", f.library.join("gone")),
    ] {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(export.join(name))
            .arg(target)
            .output()
            .unwrap();
        assert!(out.status.success(), "could not create junction: {out:?}");
    }

    assert!(is_inside_library(later.verify()));
    assert!(is_inside_library(dest.join_name("later".as_ref())));
    assert!(matches!(
        dest.join_name("outside".as_ref()),
        Err(GuardError::LeavesDestination { .. })
    ));
    assert!(dest.join_name("inside".as_ref()).is_ok());
    assert!(matches!(
        dest.join_name("broken".as_ref()),
        Err(GuardError::BrokenLink(_))
    ));
}

/// Past 260 characters the resolved path keeps the `\\?\` prefix that the short library root lacks.
#[cfg(windows)]
#[test]
fn rejects_a_path_inside_the_library_longer_than_max_path() {
    let f = fixture();
    let deep = f.library.join("a".repeat(150)).join("b".repeat(150));
    fs::create_dir_all(&deep).unwrap();
    assert!(is_inside_library(ensure_writable(
        &deep.join("new.txt"),
        &f.roots
    )));
}

/// Every spelling Windows accepts for a folder in the library leads to the same refusal.
#[cfg(windows)]
#[test]
fn rejects_the_library_under_any_windows_spelling() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("biblioteca de fotos");
    fs::create_dir_all(library.join("sub")).unwrap();
    let roots = vec![LibraryRoot::new(&library).unwrap()];
    let real = roots[0].as_path().to_string_lossy().into_owned();

    // The 8.3 name, when the volume generates them (`cmd` prints the long one otherwise).
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("cmd")
        .raw_arg(format!("/C for %I in (\"{real}\") do @echo %~sI"))
        .output()
        .unwrap();
    let short = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    assert!(!short.is_empty(), "{out:?}");

    for spelling in [
        format!(r"{short}\sub\x.txt"),
        format!(r"{short}\nueva\x.txt"),
        format!("{}/sub/x.txt", real.replace('\\', "/")),
        format!(r"{real}\sub\"),
        format!(r"{real}\\nueva\\"),
        format!(r"\\?\{real}\sub\x.txt"),
        format!(r"\\?\{real}\nueva\x.txt"),
    ] {
        assert!(
            is_inside_library(ensure_writable(&PathBuf::from(&spelling), &roots)),
            "{spelling} was accepted"
        );
    }
}

/// A profile moved to another disk leaves `%LOCALAPPDATA%` behind a junction: the app data folder
/// must keep working there.
#[cfg(windows)]
#[test]
fn a_data_folder_reached_through_a_junction_outside_the_library_is_usable() {
    let f = fixture();
    let moved = f.tmp.path().join("other-disk");
    fs::create_dir_all(moved.join("photosort")).unwrap();
    let appdata = f.tmp.path().join("appdata");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&appdata)
        .arg(&moved)
        .output()
        .unwrap();
    assert!(out.status.success(), "could not create junction: {out:?}");

    let data = fsguard::ensure_export_dir(&appdata.join("photosort"), &f.roots).unwrap();
    let db = data.join_name("catalog.sqlite".as_ref()).unwrap();
    fs::write(db.as_path(), b"x").unwrap();
    assert!(db.verify().is_ok());
    assert!(moved.join("photosort").join("catalog.sqlite").is_file());
}

/// `name:stream` opens a stream of `name`, and NTFS follows a junction to reach it. The guard must
/// refuse it by rule, not because Windows happens to return an error for that spelling.
#[cfg(windows)]
#[test]
fn refuses_a_stream_name_that_a_junction_would_carry_into_the_library() {
    let f = fixture();
    let export = f.tmp.path().join("export");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(export.join("link"))
        .arg(&f.library)
        .output()
        .unwrap();
    assert!(out.status.success(), "could not create junction: {out:?}");
    let dest = ensure_writable(&export, &f.roots).unwrap();

    for name in ["link:s", "link::$DATA", "new:s"] {
        assert!(
            matches!(
                dest.join_name(name.as_ref()),
                Err(GuardError::InvalidName(_))
            ),
            "{name} was accepted"
        );
        assert!(
            matches!(
                ensure_writable(&export.join(name), &f.roots),
                Err(GuardError::InvalidName(_))
            ),
            "{name} was accepted"
        );
    }
}
