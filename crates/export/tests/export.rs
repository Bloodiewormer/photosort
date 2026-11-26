// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Assignment, LabelId, LabelKind, Source, Status};
use export::{ExportItem, Outcome, Preview, export, preview};
use fsguard::{DestPath, LibraryRoot, ensure_export_dir};
use std::fs::{self, FileTimes};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    dest: DestPath,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "a.jpg", b"alpha");
    write(&library, "sub/b.png", b"bravo");
    let root = LibraryRoot::new(&library).unwrap();
    let dest = ensure_export_dir(&tmp.path().join("export"), &[root]).unwrap();
    Setup {
        _tmp: tmp,
        library,
        dest,
    }
}

impl Setup {
    fn item(&self, rel: &str) -> ExportItem {
        ExportItem {
            source: self.library.join(rel),
            labels: vec![],
        }
    }

    fn dest_dir(&self) -> &Path {
        self.dest.as_path()
    }

    /// Names in the destination folder, sorted.
    fn names(&self) -> Vec<String> {
        let Ok(rd) = fs::read_dir(self.dest_dir()) else {
            return vec![];
        };
        let mut v: Vec<String> = rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn outcome(report: &export::ExportReport, i: usize) -> &Outcome {
        &report.outcomes[i].1
    }
}

fn label(kind: LabelKind, name: &str, status: Status) -> Assignment {
    Assignment {
        label: LabelId(1),
        kind,
        name: name.into(),
        source: Source::Ai,
        confidence: 0.8,
        status,
    }
}

#[test]
fn the_copy_is_identical_to_the_original() {
    let s = setup();
    let report = export(&[s.item("a.jpg"), s.item("sub/b.png")], &s.dest).unwrap();
    assert!(matches!(Setup::outcome(&report, 0), Outcome::Copied(_)));
    assert_eq!(fs::read(s.dest_dir().join("a.jpg")).unwrap(), b"alpha");
    assert_eq!(fs::read(s.dest_dir().join("b.png")).unwrap(), b"bravo");
    assert_eq!(
        scanner::sha256_file(&s.dest_dir().join("a.jpg")).unwrap(),
        scanner::sha256_file(&s.library.join("a.jpg")).unwrap()
    );
}

#[test]
fn the_destination_folder_is_created_when_missing() {
    let s = setup();
    assert!(!s.dest_dir().exists());
    export(&[s.item("a.jpg")], &s.dest).unwrap();
    assert!(s.dest_dir().join("a.jpg").exists());
}

#[test]
fn the_copy_keeps_the_original_dates() {
    let s = setup();
    let src = s.library.join("a.jpg");
    let modified = UNIX_EPOCH + Duration::from_secs(1_577_836_800); // 2020-01-01
    let created = UNIX_EPOCH + Duration::from_secs(1_546_300_800); // 2019-01-01
    let times = FileTimes::new()
        .set_modified(modified)
        .set_accessed(modified);
    #[cfg(windows)]
    let times = std::os::windows::fs::FileTimesExt::set_created(times, created);
    fs::OpenOptions::new()
        .write(true)
        .open(&src)
        .unwrap()
        .set_times(times)
        .unwrap();

    export(&[s.item("a.jpg")], &s.dest).unwrap();

    let md = fs::metadata(s.dest_dir().join("a.jpg")).unwrap();
    assert_eq!(md.modified().unwrap(), modified);
    #[cfg(windows)]
    assert_eq!(md.created().unwrap(), created);
    let _ = created;
}

#[test]
fn a_different_file_with_the_same_name_is_never_overwritten() {
    let s = setup();
    write(s.dest_dir(), "a.jpg", b"someone else's file");
    let report = export(&[s.item("a.jpg")], &s.dest).unwrap();
    assert_eq!(
        s.names(),
        ["a (1).jpg", "a.jpg", "manifest.csv", "manifest.json"]
    );
    assert_eq!(
        fs::read(s.dest_dir().join("a.jpg")).unwrap(),
        b"someone else's file"
    );
    assert_eq!(fs::read(s.dest_dir().join("a (1).jpg")).unwrap(), b"alpha");
    assert!(matches!(Setup::outcome(&report, 0), Outcome::Copied(p) if p.ends_with("a (1).jpg")));
}

#[test]
fn an_identical_file_already_there_is_not_copied_again() {
    let s = setup();
    write(s.dest_dir(), "a.jpg", b"alpha");
    let report = export(&[s.item("a.jpg")], &s.dest).unwrap();
    assert!(matches!(
        Setup::outcome(&report, 0),
        Outcome::AlreadyPresent(_)
    ));
    assert_eq!(s.names(), ["a.jpg", "manifest.csv", "manifest.json"]);
}

#[test]
fn two_sources_with_the_same_name_get_different_names() {
    let s = setup();
    write(&s.library, "x/p.jpg", b"first");
    write(&s.library, "y/p.jpg", b"second");
    export(&[s.item("x/p.jpg"), s.item("y/p.jpg")], &s.dest).unwrap();
    assert_eq!(fs::read(s.dest_dir().join("p.jpg")).unwrap(), b"first");
    assert_eq!(fs::read(s.dest_dir().join("p (1).jpg")).unwrap(), b"second");
}

#[test]
fn exporting_twice_creates_no_duplicates() {
    let s = setup();
    let items = [s.item("a.jpg"), s.item("sub/b.png")];
    export(&items, &s.dest).unwrap();
    let second = export(&items, &s.dest).unwrap();
    assert!(
        second
            .outcomes
            .iter()
            .all(|(_, o)| matches!(o, Outcome::AlreadyPresent(_)))
    );
    let images: Vec<String> = s
        .names()
        .into_iter()
        .filter(|n| !n.starts_with("manifest"))
        .collect();
    assert_eq!(images, ["a.jpg", "b.png"]);
}

#[test]
fn a_failed_item_is_reported_and_the_rest_is_still_exported() {
    let s = setup();
    let report = export(&[s.item("missing.jpg"), s.item("a.jpg")], &s.dest).unwrap();
    assert!(matches!(Setup::outcome(&report, 0), Outcome::Failed(_)));
    assert!(matches!(Setup::outcome(&report, 1), Outcome::Copied(_)));
}

#[test]
fn no_temporary_file_is_left_behind() {
    let s = setup();
    export(&[s.item("a.jpg"), s.item("missing.jpg")], &s.dest).unwrap();
    assert!(
        s.names().iter().all(|n| !n.ends_with(".part")),
        "{:?}",
        s.names()
    );
}

#[test]
fn the_json_manifest_lists_each_file_with_its_hash_and_applied_labels() {
    let s = setup();
    let mut item = s.item("a.jpg");
    item.labels = vec![
        label(LabelKind::Tag, "meme", Status::Accepted),
        label(LabelKind::Category, "Anime", Status::Suggested),
        label(LabelKind::Tag, "screenshot", Status::Rejected),
    ];
    let report = export(&[item, s.item("missing.jpg")], &s.dest).unwrap();

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&report.manifest_json).unwrap()).unwrap();
    let first = &json["files"][0];
    assert_eq!(first["status"], "copied");
    assert_eq!(first["size"], 5);
    assert_eq!(
        first["sha256"],
        scanner::sha256_file(&s.library.join("a.jpg")).unwrap()
    );
    assert!(first["source_path"].as_str().unwrap().ends_with("a.jpg"));
    assert!(first["dest_path"].as_str().unwrap().ends_with("a.jpg"));
    let labels: Vec<(&str, &str)> = first["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| (l["kind"].as_str().unwrap(), l["name"].as_str().unwrap()))
        .collect();
    assert_eq!(labels, [("tag", "meme"), ("category", "Anime")]); // the rejected one is not applied
    assert!(
        json["files"][1]["status"]
            .as_str()
            .unwrap()
            .starts_with("failed")
    );
    assert_eq!(json["summary"]["copied"], 1);
    assert_eq!(json["summary"]["failed"], 1);
    assert_eq!(json["summary"]["bytes"], 5);
}

/// Splits one CSV line, honouring quotes.
fn csv_fields(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (vec![], String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

#[test]
fn the_csv_manifest_quotes_fields_that_contain_commas() {
    let s = setup();
    let weird = write(&s.library, "one, two.jpg", b"comma");
    let mut item = s.item("a.jpg");
    item.labels = vec![label(LabelKind::Tag, "meme", Status::Accepted)];
    let report = export(
        &[
            item,
            ExportItem {
                source: weird,
                labels: vec![],
            },
        ],
        &s.dest,
    )
    .unwrap();

    let csv = fs::read_to_string(&report.manifest_csv).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        csv_fields(lines[0]),
        [
            "source_path",
            "dest_path",
            "sha256",
            "size",
            "status",
            "labels"
        ]
    );
    let a = csv_fields(lines[1]);
    assert_eq!(
        (a.len(), a[4].as_str(), a[5].as_str()),
        (6, "copied", "tag:meme")
    );
    let b = csv_fields(lines[2]);
    assert_eq!(b.len(), 6, "{}", lines[2]);
    assert!(b[0].ends_with("one, two.jpg"));
}

#[test]
fn a_manifest_is_never_overwritten() {
    let s = setup();
    let first = export(&[s.item("a.jpg")], &s.dest).unwrap();
    let second = export(&[s.item("sub/b.png")], &s.dest).unwrap();
    assert_ne!(first.manifest_json, second.manifest_json);
    assert!(first.manifest_json.exists() && second.manifest_json.exists());
    assert!(second.manifest_json.ends_with("manifest (1).json"));
}

#[test]
fn the_preview_counts_files_and_bytes_without_writing_anything() {
    let s = setup();
    let p = preview(&[s.item("a.jpg"), s.item("sub/b.png")], &s.dest).unwrap();
    assert!(p.free_bytes.is_some());
    assert!(p.fits());
    assert_eq!(
        p,
        Preview {
            files: 2,
            bytes: 10,
            renamed: 0,
            already_present: 0,
            unreadable: 0,
            free_bytes: p.free_bytes,
        }
    );
    assert!(
        !s.dest_dir().exists(),
        "the preview created {:?}",
        s.names()
    );
}

#[test]
fn the_preview_predicts_renames_present_files_and_unreadable_sources() {
    let s = setup();
    write(s.dest_dir(), "a.jpg", b"different");
    write(s.dest_dir(), "b.png", b"bravo");
    write(&s.library, "z/a.jpg", b"third");
    let items = [
        s.item("a.jpg"),
        s.item("sub/b.png"),
        s.item("z/a.jpg"),
        s.item("gone.jpg"),
    ];
    let p = preview(&items, &s.dest).unwrap();
    // a.jpg -> "a (1).jpg"; b.png already there; z/a.jpg -> "a (2).jpg"; gone.jpg unreadable
    assert_eq!(
        p,
        Preview {
            files: 2,
            bytes: 10,
            renamed: 2,
            already_present: 1,
            unreadable: 1,
            free_bytes: p.free_bytes,
        }
    );
    let before = s.names();
    assert_eq!(before, ["a.jpg", "b.png"], "the preview wrote something");
}

#[test]
fn the_preview_reports_free_space_from_nearest_existing_ancestor_for_nested_missing_dest() {
    let s = setup();
    let root = LibraryRoot::new(&s.library).unwrap();
    let nested = ensure_export_dir(&s._tmp.path().join("does/not/exist/yet"), &[root]).unwrap();
    let p = preview(&[s.item("a.jpg")], &nested).unwrap();
    assert!(
        p.free_bytes.is_some_and(|b| b > 0),
        "expected positive free space on Windows volume, got {:?}",
        p.free_bytes
    );
    assert!(!nested.as_path().exists());
}

#[test]
fn preview_fits_compares_bytes_against_free_bytes_when_known() {
    let fits_exact = Preview {
        files: 1,
        bytes: 100,
        renamed: 0,
        already_present: 0,
        unreadable: 0,
        free_bytes: Some(100),
    };
    assert!(fits_exact.fits());

    let too_big = Preview {
        bytes: 101,
        ..fits_exact.clone()
    };
    assert!(!too_big.fits());

    let unknown = Preview {
        bytes: u64::MAX,
        free_bytes: None,
        ..fits_exact
    };
    assert!(unknown.fits());
}

#[test]
fn the_preview_matches_what_the_export_then_does() {
    let s = setup();
    write(s.dest_dir(), "a.jpg", b"different");
    let items = [s.item("a.jpg"), s.item("sub/b.png")];
    let p = preview(&items, &s.dest).unwrap();
    let report = export(&items, &s.dest).unwrap();
    let copied = report
        .outcomes
        .iter()
        .filter(|(_, o)| matches!(o, Outcome::Copied(_)))
        .count();
    assert_eq!(copied, p.files);
}
