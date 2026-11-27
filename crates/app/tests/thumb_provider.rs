// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::thumb_provider::open_thumbnails;
use fsguard::LibraryRoot;
use image::{ImageBuffer, Rgb};
use std::fs;
use std::path::Path;

const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn write_png(path: &Path, w: u32, h: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    ImageBuffer::<Rgb<u8>, _>::from_pixel(w, h, Rgb([200, 30, 30]))
        .save(path)
        .unwrap();
}

#[test]
fn real_thumbnails_are_made_in_the_data_dir_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = library.join("big.png");
    write_png(&source, 1000, 500);
    let root = LibraryRoot::new(&library).unwrap();
    let data = tmp.path().join("data");

    let (provider, warning) = open_thumbnails(&data, &[root]);
    assert_eq!(warning, None);

    let thumb = provider.get(&source, SHA).unwrap();
    // The cache path comes out of the path guard fully resolved; `TEMP` may not be.
    let cache = dunce::canonicalize(data.join("cache")).unwrap();
    assert!(thumb.starts_with(cache), "{thumb:?}");
    let img = image::open(&thumb).unwrap();
    assert_eq!((img.width(), img.height()), (256, 128));
}

#[test]
fn the_library_is_not_touched_by_making_thumbnails() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = library.join("a.png");
    write_png(&source, 600, 600);
    let root = LibraryRoot::new(&library).unwrap();
    let before = (
        fs::read(&source).unwrap(),
        fs::metadata(&source).unwrap().modified().unwrap(),
    );

    let (provider, _) = open_thumbnails(&tmp.path().join("data"), &[root]);
    provider.get(&source, SHA).unwrap();

    let after = (
        fs::read(&source).unwrap(),
        fs::metadata(&source).unwrap().modified().unwrap(),
    );
    assert_eq!(before, after);
    let names: Vec<_> = fs::read_dir(&library)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(names, ["a.png"], "something was created in the library");
}

#[test]
fn a_data_dir_inside_a_library_turns_thumbnails_off_with_a_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let source = library.join("a.png");
    write_png(&source, 100, 100);
    let root = LibraryRoot::new(&library).unwrap();
    let data_inside = library.join("data");

    let (provider, warning) = open_thumbnails(&data_inside, &[root]);

    assert!(warning.is_some());
    assert!(provider.get(&source, SHA).is_err());
    assert!(
        !data_inside.exists(),
        "the cache was created inside the library"
    );
}
