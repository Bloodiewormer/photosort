// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use scanner::{scan, sha256_file};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn names(root: &Path, files: &[scanner::Found]) -> Vec<String> {
    let mut v: Vec<String> = files
        .iter()
        .map(|f| {
            f.path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    v.sort();
    v
}

fn tree() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("library");
    write(&root, "a.jpg", b"aaaa");
    write(&root, "sub/b.PNG", b"bb");
    write(&root, "sub/deep/c.webp", b"c");
    write(&root, "notes.txt", b"not an image");
    write(&root, "sub/clip.mp4", b"not an image either");
    (tmp, root)
}

#[test]
fn finds_images_recursively_and_ignores_other_files() {
    let (_tmp, root) = tree();
    let report = scan(&root).unwrap();
    assert_eq!(
        names(&root, &report.files),
        ["a.jpg", "sub/b.PNG", "sub/deep/c.webp"]
    );
    assert!(report.errors.is_empty());
}

#[test]
fn records_the_size_of_each_file() {
    let (_tmp, root) = tree();
    let report = scan(&root).unwrap();
    let a = report
        .files
        .iter()
        .find(|f| f.path.ends_with("a.jpg"))
        .unwrap();
    assert_eq!(a.size, 4);
    assert!(a.modified.is_some());
}

#[test]
fn an_empty_directory_gives_an_empty_report() {
    let tmp = tempfile::tempdir().unwrap();
    let report = scan(tmp.path()).unwrap();
    assert!(report.files.is_empty() && report.errors.is_empty());
}

#[test]
fn a_root_that_does_not_exist_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(scan(&tmp.path().join("nope")).is_err());
}

#[test]
fn a_root_that_is_a_file_is_an_error() {
    let (_tmp, root) = tree();
    assert!(scan(&root.join("a.jpg")).is_err());
}

#[cfg(windows)]
#[test]
fn does_not_follow_a_junction_out_of_the_library() {
    let (tmp, root) = tree();
    let outside = tmp.path().join("outside");
    write(&outside, "secret.jpg", b"x");
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("link"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(out.status.success(), "could not create junction: {out:?}");
    let report = scan(&root).unwrap();
    assert_eq!(
        names(&root, &report.files),
        ["a.jpg", "sub/b.PNG", "sub/deep/c.webp"]
    );
}

#[test]
fn sha256_matches_the_known_vector() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "abc.bin", b"abc");
    assert_eq!(
        sha256_file(&tmp.path().join("abc.bin")).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn sha256_is_correct_across_read_block_boundaries() {
    let tmp = tempfile::tempdir().unwrap();
    let data: Vec<u8> = (0..(3 * 1024 * 1024 + 7))
        .map(|i| (i % 251) as u8)
        .collect();
    write(tmp.path(), "big.bin", &data);
    assert_eq!(
        sha256_file(&tmp.path().join("big.bin")).unwrap(),
        Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
}

#[test]
fn sha256_of_a_missing_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(sha256_file(&tmp.path().join("missing.jpg")).is_err());
}
