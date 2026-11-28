// Fixtures are created in temp dirs, never inside a real library. Nothing here calls the real
// Recycle Bin.
#![allow(clippy::disallowed_methods)]

use fsguard::{
    GuardError, LibraryRoot, ensure_move_dir, ensure_move_source, ensure_writable, move_no_replace,
};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `mklink /J` needs no administrator rights. Both ends are always inside the test's temp folder.
#[cfg(windows)]
fn junction(link: &Path, target: &Path) {
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .unwrap();
    assert!(out.status.success(), "could not create junction: {out:?}");
}

#[test]
fn move_source_refuses_folders_links_and_missing_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("fotos");
    fs::create_dir(&dir).unwrap();
    let photo = dir.join("a.jpg");
    fs::write(&photo, b"a").unwrap();

    assert!(
        ensure_move_source(&photo)
            .unwrap()
            .as_path()
            .ends_with("a.jpg")
    );
    assert!(matches!(
        ensure_move_source(&dir),
        Err(GuardError::NotAMovableFile(_))
    ));
    assert!(ensure_move_source(&dir.join("falta.jpg")).is_err());
    assert!(matches!(
        ensure_move_source(&dir.join("..").join("fotos").join("a.jpg")),
        Err(GuardError::ParentDirComponent(_))
    ));
    #[cfg(windows)]
    {
        let link = tmp.path().join("enlace");
        junction(&link, &dir);
        assert!(matches!(
            ensure_move_source(&link),
            Err(GuardError::NotAMovableFile(_))
        ));
        // A stream of a file is not a file of its own.
        assert!(ensure_move_source(&dir.join("a.jpg:oculto")).is_err());
    }
}

#[test]
fn move_dir_allows_a_library_but_refuses_app_data() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let app_data = tmp.path().join("appdata");
    fs::create_dir_all(library.join("manga")).unwrap();
    fs::create_dir_all(&app_data).unwrap();

    // The same folder that copying refuses.
    let roots = [LibraryRoot::new(&library).unwrap()];
    assert!(ensure_writable(&library.join("manga"), &roots).is_err());
    let dest = ensure_move_dir(&library.join("manga"), &app_data).unwrap();
    assert!(dest.join_name("b.jpg".as_ref()).is_ok());

    for inside in [app_data.clone(), app_data.join("thumbs").join("nueva")] {
        assert!(matches!(
            ensure_move_dir(&inside, &app_data),
            Err(GuardError::InsideAppData { .. })
        ));
    }
    // A parent of the app data is fine, but nothing derived from it may go in.
    let parent = ensure_move_dir(tmp.path(), &app_data).unwrap();
    assert!(parent.join_name("appdata".as_ref()).is_err());
    #[cfg(windows)]
    {
        let link = tmp.path().join("atajo");
        junction(&link, &app_data);
        assert!(matches!(
            ensure_move_dir(&link, &app_data),
            Err(GuardError::InsideAppData { .. })
        ));
        let trap = library.join("manga").join("trampa");
        junction(&trap, &app_data);
        assert!(dest.join_name("trampa".as_ref()).is_err());
    }
}

fn year_2001(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(980_000_000 + offset)
}

#[test]
fn same_volume_move_keeps_bytes_and_all_dates() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("appdata");
    fs::create_dir_all(tmp.path().join("out")).unwrap();
    let photo = tmp.path().join("a.jpg");
    fs::write(&photo, b"bytes de la foto").unwrap();
    let sha_before = fs::read(&photo).unwrap();

    let times = FileTimes::new()
        .set_modified(year_2001(10))
        .set_accessed(year_2001(20));
    #[cfg(windows)]
    let times = std::os::windows::fs::FileTimesExt::set_created(times, year_2001(30));
    OpenOptions::new()
        .write(true)
        .open(&photo)
        .unwrap()
        .set_times(times)
        .unwrap();
    // Read before the move and again right after it, before anything opens the file.
    let before = fs::metadata(&photo).unwrap();

    let dest = ensure_move_dir(&tmp.path().join("out"), &app_data)
        .unwrap()
        .join_name("a.jpg".as_ref())
        .unwrap();
    move_no_replace(&ensure_move_source(&photo).unwrap(), &dest).unwrap();

    let after = fs::metadata(dest.as_path()).unwrap();
    assert!(!photo.exists());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(after.accessed().unwrap(), before.accessed().unwrap());
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        assert_eq!(after.created().unwrap(), before.created().unwrap());
        assert_eq!(after.file_attributes(), before.file_attributes());
    }
    assert_eq!(fs::read(dest.as_path()).unwrap(), sha_before);
}

#[test]
fn move_never_replaces_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("appdata");
    let src = tmp.path().join("a.jpg");
    let taken = tmp.path().join("b.jpg");
    fs::write(&src, b"origen").unwrap();
    File::create(&taken).unwrap();
    fs::write(&taken, b"otro archivo").unwrap();

    let dest = ensure_move_dir(tmp.path(), &app_data)
        .unwrap()
        .join_name("b.jpg".as_ref())
        .unwrap();
    let error = move_no_replace(&ensure_move_source(&src).unwrap(), &dest).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&src).unwrap(), b"origen");
    assert_eq!(fs::read(&taken).unwrap(), b"otro archivo");
}

/// Paths longer than the classic Windows limit still move.
#[test]
fn a_long_path_can_be_moved() {
    let tmp = tempfile::tempdir().unwrap();
    let app_data = tmp.path().join("appdata");
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let mut deep = base.clone();
    for _ in 0..4 {
        deep.push("carpeta-con-un-nombre-bastante-largo-para-pasar-del-limite-clasico");
    }
    // std needs the verbatim form to create something this deep.
    fs::create_dir_all(
        fs::canonicalize(&base)
            .unwrap()
            .join(deep.strip_prefix(&base).unwrap()),
    )
    .unwrap();
    let src = base.join("a.jpg");
    fs::write(&src, b"a").unwrap();

    let dest = ensure_move_dir(&deep, &app_data)
        .unwrap()
        .join_name("a.jpg".as_ref())
        .unwrap();
    move_no_replace(&ensure_move_source(&src).unwrap(), &dest).unwrap();
    assert!(!src.exists());
    let moved = ensure_move_source(dest.as_path()).unwrap();
    assert_eq!(fs::read(moved.as_path()).unwrap(), b"a");
}
