// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{Assignment, LabelId, LabelKind, Source, Status};
use export::{
    Control, ExportItem, ExportOptions, Layout, Outcome, export, export_with, export_with_options,
    extract_xmp_from_jpeg, preview, preview_with, preview_with_options,
};
use fsguard::{LibraryRoot, ensure_export_dir};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use walkdir::WalkDir;

type Snapshot = BTreeMap<
    PathBuf,
    (
        bool,
        u64,
        Option<SystemTime>,
        Option<SystemTime>,
        Option<String>,
    ),
>;

/// Everything observable about a tree: kind, size, modified and created times and, for files, the SHA-256.
fn snapshot(root: &Path) -> Snapshot {
    WalkDir::new(root)
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            let hash = md
                .is_file()
                .then(|| scanner::sha256_file(e.path()).unwrap());
            (
                e.path().to_path_buf(),
                (
                    md.is_dir(),
                    md.len(),
                    md.modified().ok(),
                    md.created().ok(),
                    hash,
                ),
            )
        })
        .collect()
}

fn assign(kind: LabelKind, name: &str) -> Assignment {
    Assignment {
        label: LabelId(1),
        kind,
        name: name.into(),
        source: Source::User,
        confidence: 1.0,
        status: Status::Accepted,
    }
}

/// Constructs a valid synthetic JPEG with SOI, Exif APP1, DQT, SOS, and scan data with EOI.
fn synthetic_jpeg(exif_payload: &[u8], scan_data: &[u8]) -> Vec<u8> {
    let mut jpeg = Vec::new();
    jpeg.extend_from_slice(&[0xFF, 0xD8]); // SOI

    // APP1 Exif
    let mut exif_payload_full = Vec::new();
    exif_payload_full.extend_from_slice(b"Exif\0\0");
    exif_payload_full.extend_from_slice(exif_payload);
    let exif_len = (exif_payload_full.len() + 2) as u16;
    jpeg.extend_from_slice(&[0xFF, 0xE1]);
    jpeg.extend_from_slice(&exif_len.to_be_bytes());
    jpeg.extend_from_slice(&exif_payload_full);

    // DQT (Quantization table)
    jpeg.extend_from_slice(&[0xFF, 0xDB]);
    jpeg.extend_from_slice(&5u16.to_be_bytes());
    jpeg.extend_from_slice(&[0x00, 0x01, 0x02]);

    // SOS (Start of Scan)
    jpeg.extend_from_slice(&[0xFF, 0xDA]);
    jpeg.extend_from_slice(&4u16.to_be_bytes());
    jpeg.extend_from_slice(&[0x01, 0x02]);

    // Compressed scan data
    jpeg.extend_from_slice(scan_data);

    // EOI
    jpeg.extend_from_slice(&[0xFF, 0xD9]);
    jpeg
}

#[test]
fn previewing_and_exporting_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let mut items = vec![];
    for (rel, bytes) in [
        ("a.jpg", &b"jpeg bytes"[..]),
        ("sub/b.png", b"png bytes"),
        ("sub/deep/c.webp", b"webp bytes"),
    ] {
        let p = library.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        items.push(ExportItem {
            source: p,
            labels: vec![],
            belongs_to: None,
            root: Some(library.clone()),
            ..Default::default()
        });
    }
    let root = LibraryRoot::new(&library).unwrap();
    let dest = ensure_export_dir(&tmp.path().join("export"), &[root]).unwrap();

    // NTFS refreshes a directory's modified time lazily the first time it is listed from its parent,
    // so one throwaway read settles it. Without this the test compares a stale time with a fresh one.
    snapshot(&library);
    let before = snapshot(&library);

    preview(&items, &dest).unwrap();
    export(&items, &dest).unwrap();
    export(&items, &dest).unwrap();

    assert_eq!(before, snapshot(&library));
}

#[test]
fn previewing_and_exporting_in_all_five_layouts_leave_the_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    let specs = [
        (
            "a.jpg",
            &b"jpeg bytes"[..],
            vec![
                assign(LabelKind::Category, "Manga"),
                assign(LabelKind::Entity, "Obra A"),
            ],
        ),
        (
            "sub/b.png",
            &b"png bytes"[..],
            vec![assign(LabelKind::Category, "Fotos")],
        ),
        ("sub/deep/c.webp", &b"webp bytes"[..], vec![]),
    ];
    let mut items = vec![];
    for (rel, bytes, labels) in specs {
        let p = library.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, bytes).unwrap();
        items.push(ExportItem {
            source: p,
            labels,
            belongs_to: None,
            root: Some(library.clone()),
            ..Default::default()
        });
    }
    let root = LibraryRoot::new(&library).unwrap();

    // Throwaway read so NTFS settles directory modified times before taking the baseline snapshot.
    snapshot(&library);
    let before = snapshot(&library);

    for (i, layout) in [
        Layout::Flat,
        Layout::ByCategory,
        Layout::ByEntity,
        Layout::ByYear,
        Layout::PreserveFolders,
    ]
    .into_iter()
    .enumerate()
    {
        let dest = ensure_export_dir(
            &tmp.path().join(format!("export_{i}")),
            std::slice::from_ref(&root),
        )
        .unwrap();
        preview_with(&items, &dest, layout).unwrap();
        export_with(&items, &dest, layout, &mut |_| Control::Continue).unwrap();
        export_with(&items, &dest, layout, &mut |_| Control::Continue).unwrap();
    }

    assert_eq!(before, snapshot(&library));
}

#[test]
fn previewing_and_exporting_with_all_options_active_leaves_originals_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");

    let raw_jpeg = synthetic_jpeg(
        b"orientation 6 and test metadata",
        b"original scan bitstream",
    );
    let p_jpeg = library.join("a.jpg");
    let p_png = library.join("sub/b.png");
    let p_webp = library.join("sub/deep/c.webp");

    fs::create_dir_all(p_jpeg.parent().unwrap()).unwrap();
    fs::create_dir_all(p_png.parent().unwrap()).unwrap();
    fs::create_dir_all(p_webp.parent().unwrap()).unwrap();

    fs::write(&p_jpeg, &raw_jpeg).unwrap();
    fs::write(&p_png, b"png image data bytes").unwrap();
    fs::write(&p_webp, b"webp image data bytes").unwrap();

    let items = vec![
        ExportItem {
            source: p_jpeg.clone(),
            labels: vec![
                assign(LabelKind::Category, "Manga"),
                assign(LabelKind::Tag, "portada"),
                assign(LabelKind::Entity, "Obra Fantasia"),
            ],
            belongs_to: None,
            persons: vec!["Heroina".into()],
            hierarchical_categories: vec!["Manga|Scanlation|Página promocional".into()],
            root: Some(library.clone()),
        },
        ExportItem {
            source: p_png.clone(),
            labels: vec![assign(LabelKind::Category, "Fotos")],
            belongs_to: None,
            persons: vec![],
            hierarchical_categories: vec![],
            root: Some(library.clone()),
        },
        ExportItem {
            source: p_webp.clone(),
            labels: vec![],
            belongs_to: None,
            persons: vec![],
            hierarchical_categories: vec![],
            root: Some(library.clone()),
        },
    ];

    let root = LibraryRoot::new(&library).unwrap();
    let dest_path = tmp.path().join("export_all_active");
    let dest = ensure_export_dir(&dest_path, &[root]).unwrap();

    // Settle NTFS metadata before taking the baseline snapshot
    snapshot(&library);
    let before = snapshot(&library);

    let options = ExportOptions {
        layout: Layout::PreserveFolders,
        write_xmp_sidecars: true,
        embed_jpeg_tags: true,
        operation: Default::default(),
    };

    // Preview
    let p = preview_with_options(&items, &dest, options).unwrap();
    assert_eq!(p.files, 3);

    // First export
    let report = export_with_options(&items, &dest, options, &mut |_| Control::Continue).unwrap();

    // The preview of a second export already knows the tagged copies.
    let p2 = preview_with_options(&items, &dest, options).unwrap();
    assert_eq!((p2.files, p2.already_present, p2.renamed), (0, 3, 0));

    // Second export to test re-exporting to same destination
    let report2 = export_with_options(&items, &dest, options, &mut |_| Control::Continue).unwrap();

    // INNEGOCIABLE SAFETY CHECK: The library originals MUST be completely untouched!
    // No bytes changed, no dates changed, no files created beside them (no .xmp sidecars in library).
    assert_eq!(before, snapshot(&library));

    // Check that NO .xmp files were written in library
    let library_files: Vec<_> = WalkDir::new(&library)
        .into_iter()
        .map(|e| e.unwrap().path().to_path_buf())
        .collect();
    for path in &library_files {
        assert!(
            !path.to_string_lossy().ends_with(".xmp"),
            "Sidecar XMP was erroneously created in library: {}",
            path.display()
        );
    }

    // Verify destination files:
    // 1. Structure preserved:
    assert!(dest.as_path().join("a.jpg").exists());
    assert!(dest.as_path().join("sub/b.png").exists());
    assert!(dest.as_path().join("sub/deep/c.webp").exists());

    // 2. XMP sidecars in destination:
    assert!(dest.as_path().join("a.jpg.xmp").exists());
    assert!(dest.as_path().join("sub/b.png.xmp").exists());
    assert!(dest.as_path().join("sub/deep/c.webp.xmp").exists());

    let sidecar_content = fs::read_to_string(dest.as_path().join("a.jpg.xmp")).unwrap();
    assert!(sidecar_content.contains("<dc:subject>"));
    assert!(sidecar_content.contains("<rdf:li>Heroina</rdf:li>"));
    assert!(sidecar_content.contains("<rdf:li>Obra Fantasia</rdf:li>"));
    assert!(sidecar_content.contains("<lr:hierarchicalSubject>"));
    assert!(sidecar_content.contains("<rdf:li>Manga|Scanlation|Página promocional</rdf:li>"));
    assert!(sidecar_content.contains("<iptcExt:PersonInImage>"));

    // 3. JPEG embedded XMP in copy:
    let tagged_jpeg_bytes = fs::read(dest.as_path().join("a.jpg")).unwrap();
    let extracted_xmp = extract_xmp_from_jpeg(&tagged_jpeg_bytes).unwrap().unwrap();
    let extracted_str = String::from_utf8(extracted_xmp).unwrap();
    assert!(extracted_str.contains("<rdf:li>Heroina</rdf:li>"));
    assert!(extracted_str.contains("<rdf:li>Obra Fantasia</rdf:li>"));

    // 4. Exif and scan data preserved in the tagged copy:
    let expected_exif = b"orientation";
    let expected_scan = b"original scan bitstream";
    assert!(
        tagged_jpeg_bytes
            .windows(expected_exif.len())
            .any(|w| w == expected_exif)
    );
    assert!(
        tagged_jpeg_bytes
            .windows(expected_scan.len())
            .any(|w| w == expected_scan)
    );

    // 5. Manifest records both hashes for JPEG:
    let manifest_json_content = fs::read_to_string(&report.manifest_json).unwrap();
    let json: serde_json::Value = serde_json::from_str(&manifest_json_content).unwrap();
    let a_file = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["source_path"].as_str().unwrap().ends_with("a.jpg"))
        .unwrap();

    let orig_sha = scanner::sha256_file(&p_jpeg).unwrap();
    let final_sha = scanner::sha256_file(&dest.as_path().join("a.jpg")).unwrap();

    assert_eq!(a_file["sha256"], orig_sha);
    assert_eq!(a_file["sha256_original"], orig_sha);
    assert_eq!(a_file["sha256_after_tagging"], final_sha);
    assert_ne!(orig_sha, final_sha); // Since XMP was embedded, copy's hash changed

    // Non-JPEG copies do not have sha256_after_tagging
    let b_file = json["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["source_path"].as_str().unwrap().ends_with("b.png"))
        .unwrap();
    assert!(b_file.get("sha256_after_tagging").is_none());

    // CSV also records sha256_after_tagging
    let manifest_csv_content = fs::read_to_string(&report.manifest_csv).unwrap();
    assert!(manifest_csv_content.contains(&orig_sha));
    assert!(manifest_csv_content.contains(&final_sha));

    // The tagged copy keeps the original's dates.
    let src_md = fs::metadata(&p_jpeg).unwrap();
    let dst_md = fs::metadata(dest.as_path().join("a.jpg")).unwrap();
    assert_eq!(src_md.modified().unwrap(), dst_md.modified().unwrap());
    assert_eq!(src_md.created().ok(), dst_md.created().ok());

    // Exporting again adds nothing: no `a (1).jpg`, no second sidecar.
    assert_eq!(report2.outcomes.len(), 3);
    for (src, outcome) in &report2.outcomes {
        assert!(
            matches!(outcome, Outcome::AlreadyPresent(_)),
            "{}: {outcome:?}",
            src.display()
        );
    }
    let mut names: Vec<String> = WalkDir::new(dest.as_path())
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            e.path()
                .strip_prefix(dest.as_path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|n| !n.is_empty() && !n.starts_with("manifest"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "a.jpg",
            "a.jpg.xmp",
            "sub",
            "sub/b.png",
            "sub/b.png.xmp",
            "sub/deep",
            "sub/deep/c.webp",
            "sub/deep/c.webp.xmp"
        ]
    );
    let json2: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&report2.manifest_json).unwrap()).unwrap();
    let a2 = json2["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["source_path"].as_str().unwrap().ends_with("a.jpg"))
        .unwrap();
    assert_eq!(a2["sha256_after_tagging"], final_sha);
}

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

#[cfg(windows)]
#[test]
fn junctions_in_the_destination_never_lead_a_write_into_the_library() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    // An empty library folder: anything that appears in it was written through a junction.
    let trap = library.join("trap");
    fs::create_dir_all(&trap).unwrap();

    let mut items = vec![];
    for (rel, category) in [("manga/obra/a.jpg", "Manga"), ("fotos/b.jpg", "Fotos")] {
        let p = library.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, synthetic_jpeg(rel.as_bytes(), b"scan")).unwrap();
        items.push(ExportItem {
            labels: vec![assign(LabelKind::Category, category)],
            root: Some(library.clone()),
            ..ExportItem::new(p)
        });
    }
    let root = LibraryRoot::new(&library).unwrap();

    snapshot(&library);
    let before = snapshot(&library);

    // The first item goes to the linked folder; the second one must still be exported.
    for (name, layout, linked, copied) in [
        ("preserve", Layout::PreserveFolders, "manga", "fotos/b.jpg"),
        ("category", Layout::ByCategory, "Manga", "Fotos/b.jpg"),
    ] {
        let dest_path = tmp.path().join(name);
        fs::create_dir_all(&dest_path).unwrap();
        junction(&dest_path.join(linked), &trap);
        let dest = ensure_export_dir(&dest_path, std::slice::from_ref(&root)).unwrap();
        let options = ExportOptions {
            layout,
            write_xmp_sidecars: true,
            embed_jpeg_tags: true,
            operation: Default::default(),
        };

        // The blocked item is counted apart from the unreadable ones.
        let p = preview_with_options(&items, &dest, options).unwrap();
        assert_eq!((p.blocked, p.unreadable, p.files), (1, 0, 1), "{name}");
        let report =
            export_with_options(&items, &dest, options, &mut |_| Control::Continue).unwrap();

        assert_eq!(before, snapshot(&library), "{name}: the library changed");
        match &report.outcomes[0].1 {
            Outcome::Blocked(reason) => assert_eq!(
                reason,
                "la ruta lleva dentro de la biblioteca «library», donde no se escribe nada"
            ),
            other => panic!("{name}: expected a blocked item, got {other:?}"),
        }
        assert!(matches!(report.outcomes[1].1, Outcome::Copied(_)));
        assert!(dest_path.join(copied).is_file());
        assert!(dest_path.join(format!("{copied}.xmp")).is_file());

        // The manifest keeps recording a blocked item as failed, and the summary adds up.
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report.manifest_json).unwrap()).unwrap();
        let status = |i: usize| json["files"][i]["status"].as_str().unwrap().to_owned();
        assert!(status(0).starts_with("failed: la ruta lleva"), "{name}");
        assert!(json["files"][0]["dest_path"].is_null(), "{name}");
        assert_eq!(status(1), "copied", "{name}");
        assert_eq!(json["summary"]["failed"], 1, "{name}");
        assert_eq!(json["summary"]["copied"], 1, "{name}");
        assert_eq!(json["summary"]["already_present"], 0, "{name}");
        let csv = fs::read_to_string(&report.manifest_csv).unwrap();
        // Quoted: the reason has a comma.
        assert!(csv.contains(",\"failed: la ruta lleva"), "{name}: {csv}");
    }

    // A link where a sidecar would go: the copy is made and the report carries the warning.
    let dest_path = tmp.path().join("sidecar");
    fs::create_dir_all(&dest_path).unwrap();
    junction(&dest_path.join("b.jpg.xmp"), &trap);
    let dest = ensure_export_dir(&dest_path, std::slice::from_ref(&root)).unwrap();
    let options = ExportOptions {
        write_xmp_sidecars: true,
        ..Default::default()
    };
    let report =
        export_with_options(&items[1..], &dest, options, &mut |_| Control::Continue).unwrap();

    assert_eq!(before, snapshot(&library), "sidecar: the library changed");
    assert!(matches!(report.outcomes[0].1, Outcome::Copied(_)));
    assert!(
        report.warnings[0]
            .1
            .starts_with("no se escribió el archivo XMP: la ruta lleva dentro de la biblioteca"),
        "{:?}",
        report.warnings
    );
}
