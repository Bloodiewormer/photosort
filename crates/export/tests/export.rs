// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Assignment, LabelId, LabelKind, Source, Status};
use export::{
    Control, ExportItem, ExportProgress, Layout, Outcome, Preview, export, export_with, preview,
    preview_with, sanitize_folder_name,
};
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

#[test]
fn sanitize_folder_name_enforces_every_rule() {
    // Replaces <>:"/\|?* and control characters with '_'
    assert_eq!(
        sanitize_folder_name("a<b>c:d\"e/f\\g|h?i*j"),
        "a_b_c_d_e_f_g_h_i_j"
    );
    assert_eq!(
        sanitize_folder_name("line1\nline2\t\x00\x1f"),
        "line1_line2___"
    );

    // Trims trailing dots and spaces
    assert_eq!(sanitize_folder_name("fotos. . "), "fotos");
    assert_eq!(sanitize_folder_name("normal..."), "normal");

    // Windows reserved device names (case-insensitive, with or without extension)
    for reserved in [
        "CON",
        "con",
        "PRN",
        "AUX",
        "NUL",
        "COM1",
        "COM9",
        "LPT1",
        "LPT9",
        "CON.txt",
        "nul.jpg",
        "lpt5.tar.gz",
        "Aux .png",
    ] {
        let got = sanitize_folder_name(reserved);
        assert!(
            got.starts_with('_'),
            "reserved name {reserved:?} was not prefixed: {got:?}"
        );
    }
    // Non-reserved lookalikes remain untouched
    for safe in ["CON1", "COM0", "LPT0", "COM10", "CONSOLE", "NULLED"] {
        assert_eq!(sanitize_folder_name(safe), safe);
    }

    // Capped at 80 chars (including multibyte UTF-8 and trailing dots exposed by truncation)
    let long_utf8 = "á".repeat(100);
    assert_eq!(sanitize_folder_name(&long_utf8).chars().count(), 80);

    let trunc_dot = format!("{}.trailing", "a".repeat(79));
    assert_eq!(sanitize_folder_name(&trunc_dot), "a".repeat(79));

    let long_reserved = format!("CON.{}", "x".repeat(90));
    let sanitized_reserved = sanitize_folder_name(&long_reserved);
    assert!(sanitized_reserved.starts_with("_CON."));
    assert!(sanitized_reserved.chars().count() <= 80);

    // Empty or only dots/spaces -> "_"
    for empty in ["", "   ", "...", " . . "] {
        assert_eq!(sanitize_folder_name(empty), "_");
    }
}

fn set_modified_secs(path: &Path, secs: u64) {
    let t = UNIX_EPOCH + Duration::from_secs(secs);
    let times = FileTimes::new().set_modified(t).set_accessed(t);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

#[test]
fn by_category_uses_first_non_rejected_category_alphabetically_or_fallback() {
    let s = setup();
    let mut a = s.item("a.jpg");
    a.labels = vec![
        label(LabelKind::Category, "AaaRechazada", Status::Rejected),
        label(LabelKind::Tag, "AaaEtiqueta", Status::Accepted),
        label(LabelKind::Category, "Manga", Status::Suggested),
        label(LabelKind::Category, "Anime", Status::Accepted),
    ];
    let mut b = s.item("sub/b.png");
    b.labels = vec![label(
        LabelKind::Category,
        "SoloRechazada",
        Status::Rejected,
    )];

    let report = export_with(&[a, b], &s.dest, Layout::ByCategory, &mut |_| {
        Control::Continue
    })
    .unwrap();
    assert!(!report.stopped);
    assert_eq!(
        fs::read(s.dest_dir().join("Anime").join("a.jpg")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(s.dest_dir().join("Sin categoría").join("b.png")).unwrap(),
        b"bravo"
    );

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&report.manifest_json).unwrap()).unwrap();
    let d0 = json["files"][0]["dest_path"].as_str().unwrap();
    let d1 = json["files"][1]["dest_path"].as_str().unwrap();
    assert!(
        d0.ends_with(&format!("Anime{}a.jpg", std::path::MAIN_SEPARATOR)),
        "{d0}"
    );
    assert!(
        d1.ends_with(&format!("Sin categoría{}b.png", std::path::MAIN_SEPARATOR)),
        "{d1}"
    );

    let csv = fs::read_to_string(&report.manifest_csv).unwrap();
    let lines: Vec<&str> = csv.lines().collect();
    assert!(csv_fields(lines[1])[1].contains("Anime"));
    assert!(csv_fields(lines[2])[1].contains("Sin categoría"));
}

#[test]
fn by_entity_uses_first_non_rejected_entity_sanitized_or_fallback() {
    let s = setup();
    let mut a = s.item("a.jpg");
    a.labels = vec![
        label(LabelKind::Entity, "Zeta", Status::Accepted),
        label(LabelKind::Entity, "Re:Zero / Parte?", Status::Suggested),
        label(LabelKind::Entity, "000Rechazada", Status::Rejected),
    ];
    let b = s.item("sub/b.png");

    let report = export_with(&[a, b], &s.dest, Layout::ByEntity, &mut |_| {
        Control::Continue
    })
    .unwrap();
    assert!(!report.stopped);
    assert_eq!(
        fs::read(s.dest_dir().join("Re_Zero _ Parte_").join("a.jpg")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(s.dest_dir().join("Sin obra").join("b.png")).unwrap(),
        b"bravo"
    );
}

#[test]
fn by_year_uses_four_digit_modification_year() {
    let s = setup();
    set_modified_secs(&s.library.join("a.jpg"), 1_577_836_800); // 2020-01-01
    set_modified_secs(&s.library.join("sub/b.png"), 1_546_300_800); // 2019-01-01

    let report = export_with(
        &[s.item("a.jpg"), s.item("sub/b.png")],
        &s.dest,
        Layout::ByYear,
        &mut |_| Control::Continue,
    )
    .unwrap();
    assert!(!report.stopped);
    assert_eq!(
        fs::read(s.dest_dir().join("2020").join("a.jpg")).unwrap(),
        b"alpha"
    );
    assert_eq!(
        fs::read(s.dest_dir().join("2019").join("b.png")).unwrap(),
        b"bravo"
    );
}

#[test]
fn conflicts_are_resolved_per_subfolder_without_cross_folder_collisions() {
    let s = setup();
    write(&s.library, "x/p.jpg", b"first");
    write(&s.library, "y/p.jpg", b"second");
    write(&s.library, "z/p.jpg", b"third");

    let mut x = s.item("x/p.jpg");
    x.labels = vec![label(LabelKind::Category, "Anime", Status::Accepted)];
    let mut y = s.item("y/p.jpg");
    y.labels = vec![label(LabelKind::Category, "Manga", Status::Accepted)];
    let mut z = s.item("z/p.jpg");
    z.labels = vec![label(LabelKind::Category, "Anime", Status::Accepted)];

    let items = [x, y, z];
    let first = export_with(&items, &s.dest, Layout::ByCategory, &mut |_| {
        Control::Continue
    })
    .unwrap();
    assert!(
        matches!(Setup::outcome(&first, 0), Outcome::Copied(p) if p.ends_with(Path::new("Anime").join("p.jpg")))
    );
    assert!(
        matches!(Setup::outcome(&first, 1), Outcome::Copied(p) if p.ends_with(Path::new("Manga").join("p.jpg")))
    );
    assert!(
        matches!(Setup::outcome(&first, 2), Outcome::Copied(p) if p.ends_with(Path::new("Anime").join("p (1).jpg")))
    );

    // Second export sees every file as already present.
    let second = export_with(&items, &s.dest, Layout::ByCategory, &mut |_| {
        Control::Continue
    })
    .unwrap();
    assert!(
        second
            .outcomes
            .iter()
            .all(|(_, o)| matches!(o, Outcome::AlreadyPresent(_)))
    );
}

#[test]
fn preview_with_matches_export_with_for_all_four_layouts() {
    for (idx, layout) in [
        Layout::Flat,
        Layout::ByCategory,
        Layout::ByEntity,
        Layout::ByYear,
    ]
    .into_iter()
    .enumerate()
    {
        let s = setup();
        write(&s.library, "x/dup.jpg", b"dup-one");
        write(&s.library, "y/dup.jpg", b"dup-two-longer");
        set_modified_secs(&s.library.join("a.jpg"), 1_577_836_800); // 2020
        set_modified_secs(&s.library.join("sub/b.png"), 1_546_300_800); // 2019
        set_modified_secs(&s.library.join("x/dup.jpg"), 1_577_836_800); // 2020
        set_modified_secs(&s.library.join("y/dup.jpg"), 1_577_836_800); // 2020

        let mut a = s.item("a.jpg");
        a.labels = vec![
            label(LabelKind::Category, "Anime", Status::Accepted),
            label(LabelKind::Entity, "Obra 1", Status::Accepted),
        ];
        let mut b = s.item("sub/b.png");
        b.labels = vec![
            label(LabelKind::Category, "Manga", Status::Accepted),
            label(LabelKind::Entity, "Obra 2", Status::Accepted),
        ];
        let mut x = s.item("x/dup.jpg");
        x.labels = vec![
            label(LabelKind::Category, "Anime", Status::Accepted),
            label(LabelKind::Entity, "Obra 1", Status::Accepted),
        ];
        let mut y = s.item("y/dup.jpg");
        y.labels = vec![
            label(LabelKind::Category, "Anime", Status::Accepted),
            label(LabelKind::Entity, "Obra 2", Status::Accepted),
        ];
        let missing = s.item("gone.jpg");

        let root = LibraryRoot::new(&s.library).unwrap();
        let dest =
            ensure_export_dir(&s._tmp.path().join(format!("layout_{idx}")), &[root]).unwrap();

        // Pre-seed one conflicting file and one identical file in the expected folder for this layout.
        match layout {
            Layout::Flat => {
                write(dest.as_path(), "a.jpg", b"other");
                write(dest.as_path(), "b.png", b"bravo");
            }
            Layout::ByCategory => {
                write(&dest.as_path().join("Anime"), "a.jpg", b"other");
                write(&dest.as_path().join("Manga"), "b.png", b"bravo");
            }
            Layout::ByEntity => {
                write(&dest.as_path().join("Obra 1"), "a.jpg", b"other");
                write(&dest.as_path().join("Obra 2"), "b.png", b"bravo");
            }
            Layout::ByYear => {
                write(&dest.as_path().join("2020"), "a.jpg", b"other");
                write(&dest.as_path().join("2019"), "b.png", b"bravo");
            }
        }

        let items = [a, b, x, y, missing];
        let p = preview_with(&items, &dest, layout).unwrap();
        let report = export_with(&items, &dest, layout, &mut |_| Control::Continue).unwrap();

        let mut copied = 0usize;
        let mut renamed = 0usize;
        let mut present = 0usize;
        let mut failed = 0usize;
        let mut bytes = 0u64;
        for (src, outcome) in &report.outcomes {
            match outcome {
                Outcome::Copied(dst) => {
                    copied += 1;
                    bytes += fs::metadata(dst).unwrap().len();
                    if dst.file_name() != src.file_name() {
                        renamed += 1;
                    }
                }
                Outcome::AlreadyPresent(_) => present += 1,
                Outcome::Failed(_) => failed += 1,
            }
        }

        assert_eq!(p.files, copied, "files mismatch for {layout:?}");
        assert_eq!(p.bytes, bytes, "bytes mismatch for {layout:?}");
        assert_eq!(p.renamed, renamed, "renamed mismatch for {layout:?}");
        assert_eq!(
            p.already_present, present,
            "already_present mismatch for {layout:?}"
        );
        assert_eq!(p.unreadable, failed, "unreadable mismatch for {layout:?}");
    }
}

#[test]
fn export_with_reports_progress_after_each_item_and_honours_stop() {
    let s = setup();
    write(&s.library, "c.jpg", b"charlie");
    let items = [s.item("a.jpg"), s.item("sub/b.png"), s.item("c.jpg")];

    let mut seen = Vec::new();
    let report = export_with(&items, &s.dest, Layout::Flat, &mut |p: &ExportProgress| {
        seen.push((p.done, p.total));
        if p.done == 2 {
            Control::Stop
        } else {
            Control::Continue
        }
    })
    .unwrap();

    assert_eq!(seen, vec![(1, 3), (2, 3)]);
    assert!(report.stopped);
    assert_eq!(report.outcomes.len(), 2);
    assert!(s.dest_dir().join("a.jpg").exists());
    assert!(s.dest_dir().join("b.png").exists());
    assert!(!s.dest_dir().join("c.jpg").exists());

    // Manifest is still written for the 2 items processed.
    assert!(report.manifest_json.exists());
    assert!(report.manifest_csv.exists());
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&report.manifest_json).unwrap()).unwrap();
    assert_eq!(json["files"].as_array().unwrap().len(), 2);
    assert_eq!(json["summary"]["copied"], 2);
}

#[test]
fn export_with_completed_run_reports_stopped_false_and_all_progress_steps() {
    let s = setup();
    write(&s.library, "c.jpg", b"charlie");
    let items = [s.item("a.jpg"), s.item("sub/b.png"), s.item("c.jpg")];

    let mut seen = Vec::new();
    let report = export_with(&items, &s.dest, Layout::Flat, &mut |p| {
        seen.push((p.done, p.total));
        Control::Continue
    })
    .unwrap();

    assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);
    assert!(!report.stopped);
    assert_eq!(report.outcomes.len(), 3);
}
