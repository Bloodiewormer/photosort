// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::export_flow::{
    ALL_LAYOUTS, ExportPhase, ExportWizard, PreviewState, build_export_items, layout_description,
    layout_label, remember_export_dir, select_entries, validate_destination,
};
use app::model::Selection;
use app::settings::{self, Settings};
use catalog::{Catalog, Entry, Filter, LabelKind, Sort, Source, Status};
use export::{ExportItem, Layout};
use fsguard::{LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

fn write_file(root: &Path, rel: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, bytes).unwrap();
    p
}

fn wait_until(wizard: &mut ExportWizard, pred: impl Fn(&ExportPhase) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        wizard.poll();
        if pred(wizard.phase()) {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for wizard phase; stuck in {:?}",
        wizard.phase()
    );
}

#[test]
fn select_entries_uses_selection_or_all_filtered_entries_when_empty() {
    let entries: Vec<Entry> = (0..3)
        .map(|i| Entry {
            asset: catalog::AssetId(i + 1),
            path: PathBuf::from(format!("img_{i}.jpg")),
            sha256: format!("{i:064x}"),
            size: 10,
            modified_ns: None,
        })
        .collect();

    let empty = Selection::default();
    let all = select_entries(&entries, &empty);
    assert_eq!(all.len(), 3);

    let mut sel = Selection::default();
    sel.click(0, entries.len());
    sel.ctrl_click(2, entries.len());
    let picked = select_entries(&entries, &sel);
    assert_eq!(picked.len(), 2);
    assert_eq!(picked[0].path, PathBuf::from("img_0.jpg"));
    assert_eq!(picked[1].path, PathBuf::from("img_2.jpg"));
}

#[test]
fn build_export_items_loads_labels_from_catalog_for_each_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    write_file(&lib, "a.jpg", b"jpeg-a");
    write_file(&lib, "b.png", b"png-b");

    let root = LibraryRoot::new(&lib).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();

    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    assert_eq!(entries.len(), 2);

    let cat_label = cat.label(LabelKind::Category, "Manga").unwrap();
    let ent_label = cat.label(LabelKind::Entity, "Obra X").unwrap();
    let rej_label = cat.label(LabelKind::Tag, "descartada").unwrap();
    cat.accept(entries[0].asset, cat_label).unwrap();
    cat.suggest(entries[0].asset, ent_label, Source::Rules, 0.9)
        .unwrap();
    cat.reject(entries[0].asset, rej_label).unwrap();

    let refs: Vec<&Entry> = entries.iter().collect();
    let items = build_export_items(&cat, &refs).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].source, entries[0].path);
    assert_eq!(items[0].labels.len(), 3);
    assert!(items[0].labels.iter().any(|l| l.kind == LabelKind::Category
        && l.name == "Manga"
        && l.status == Status::Accepted));
    assert!(items[0].labels.iter().any(|l| l.kind == LabelKind::Entity
        && l.name == "Obra X"
        && l.status == Status::Suggested));
    assert!(items[1].labels.is_empty());
}

#[test]
fn validate_destination_refuses_library_overlap_with_spanish_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    fs::create_dir_all(&lib).unwrap();
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = [root];

    // Empty path
    let empty_err = validate_destination(Path::new(""), &roots).unwrap_err();
    assert!(empty_err.contains("destino"), "{empty_err}");

    // Inside library
    let inside_err = validate_destination(&lib.join("sub"), &roots).unwrap_err();
    assert!(
        inside_err.to_lowercase().contains("biblioteca"),
        "{inside_err}"
    );

    // Library itself
    let same_err = validate_destination(&lib, &roots).unwrap_err();
    assert!(same_err.to_lowercase().contains("biblioteca"), "{same_err}");

    // Contains library (parent dir)
    let parent_err = validate_destination(tmp.path(), &roots).unwrap_err();
    assert!(
        parent_err.to_lowercase().contains("contiene")
            && parent_err.to_lowercase().contains("biblioteca"),
        "{parent_err}"
    );

    // Parent dir traversal `..`
    let dotdot_err = validate_destination(&tmp.path().join("export/../other"), &roots).unwrap_err();
    assert!(dotdot_err.contains(".."), "{dotdot_err}");

    // Valid sibling folder
    let ok = validate_destination(&tmp.path().join("export_ok"), &roots).unwrap();
    assert!(ok.as_path().ends_with("export_ok"));
}

#[test]
fn remember_export_dir_persists_last_export_dir_via_settings_api() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let lib = tmp.path().join("library");
    let export_dir = tmp.path().join("exports");
    fs::create_dir_all(&lib).unwrap();

    let mut cfg = Settings::default();
    cfg.add_library_root(&lib, &data).unwrap();
    let dest = validate_destination(&export_dir, &cfg.valid_roots()).unwrap();

    remember_export_dir(&data, &mut cfg, &dest).unwrap();
    assert_eq!(cfg.last_export_dir.as_deref(), Some(dest.as_path()));

    let reloaded = settings::load(&data);
    assert!(reloaded.warning.is_none());
    assert_eq!(
        reloaded.settings.last_export_dir.as_deref(),
        Some(dest.as_path())
    );
}

#[test]
fn wizard_state_machine_transitions_choose_preview_running_done() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    write_file(&lib, "a.jpg", b"alpha");
    write_file(&lib, "b.png", b"bravo");

    let root = LibraryRoot::new(&lib).unwrap();
    let db = ensure_writable(&data.join("catalog.sqlite"), std::slice::from_ref(&root)).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    let cat_id = cat.label(LabelKind::Category, "Fotos").unwrap();
    cat.accept(entries[0].asset, cat_id).unwrap();

    let export_dir = tmp.path().join("out");
    let mut wizard = ExportWizard::open(
        &cat,
        &entries,
        &Selection::default(),
        Some(export_dir.clone()),
    );

    // Starts in Choose; nothing copied and cannot export yet.
    assert_eq!(wizard.phase(), &ExportPhase::Choose);
    assert_eq!(wizard.item_count(), 2);
    assert!(!wizard.from_selection());
    assert_eq!(wizard.dest_input(), export_dir.as_path());
    assert!(!wizard.can_start_export());
    assert!(wizard.start_export(None).is_err());
    assert!(!export_dir.exists());

    // Pick ByCategory and request off-thread preview.
    wizard.set_layout(Layout::ByCategory);
    wizard
        .request_preview(std::slice::from_ref(&root), None)
        .unwrap();
    assert!(matches!(
        wizard.phase(),
        ExportPhase::Preview(PreviewState::Computing)
    ));

    wait_until(&mut wizard, |p| {
        matches!(p, ExportPhase::Preview(PreviewState::Ready(_)))
    });
    assert!(wizard.can_start_export());
    assert!(!export_dir.exists(), "preview must not create destination");
    if let ExportPhase::Preview(PreviewState::Ready(p)) = wizard.phase() {
        assert_eq!(p.files, 2);
        assert_eq!(p.bytes, 10);
        assert!(p.fits());
    } else {
        panic!("expected ready preview");
    }

    // Explicit click on Exportar starts background export.
    wizard.start_export(None).unwrap();
    assert!(matches!(
        wizard.phase(),
        ExportPhase::Running { total: 2, .. }
    ));

    wait_until(&mut wizard, |p| matches!(p, ExportPhase::Done(_)));
    if let ExportPhase::Done(summary) = wizard.phase() {
        assert_eq!(summary.copied, 2);
        assert_eq!(summary.already_present, 0);
        assert!(summary.failed.is_empty());
        assert!(!summary.stopped);
        assert!(summary.manifest_json.exists());
        assert!(summary.manifest_csv.exists());
        assert_eq!(summary.manifest_json_name(), "manifest.json");
        assert_eq!(summary.manifest_csv_name(), "manifest.csv");
    } else {
        panic!("expected Done phase");
    }
    assert!(export_dir.join("Fotos").join("a.jpg").exists());
    assert!(export_dir.join("Sin categoría").join("b.png").exists());
}

#[test]
fn wizard_rejects_invalid_destination_and_invalidates_preview_on_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    let a = write_file(&lib, "a.jpg", b"alpha");
    let root = LibraryRoot::new(&lib).unwrap();

    let mut wizard = ExportWizard::from_items(
        vec![ExportItem {
            source: a,
            labels: vec![],
            belongs_to: None,
        }],
        true,
        Some(lib.join("inside")),
    );
    assert!(wizard.from_selection());

    let err = wizard
        .request_preview(std::slice::from_ref(&root), None)
        .unwrap_err();
    assert!(err.to_lowercase().contains("biblioteca"));
    assert_eq!(wizard.phase(), &ExportPhase::Choose);
    assert!(wizard.validation_error().is_some());

    // Fix destination -> preview succeeds -> changing layout or going back resets to Choose.
    wizard.set_dest(tmp.path().join("dest_ok"));
    assert!(wizard.validation_error().is_none());
    wizard
        .request_preview(std::slice::from_ref(&root), None)
        .unwrap();
    wait_until(&mut wizard, |p| {
        matches!(p, ExportPhase::Preview(PreviewState::Ready(_)))
    });

    wizard.set_layout(Layout::ByYear);
    assert_eq!(wizard.phase(), &ExportPhase::Choose);
    assert!(!wizard.can_start_export());

    wizard
        .request_preview(std::slice::from_ref(&root), None)
        .unwrap();
    wait_until(&mut wizard, |p| {
        matches!(p, ExportPhase::Preview(PreviewState::Ready(_)))
    });
    wizard.back_to_choose();
    assert_eq!(wizard.phase(), &ExportPhase::Choose);
}

#[test]
fn wizard_cancel_stops_background_export_and_writes_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("library");
    let mut items = Vec::new();
    for i in 0..40 {
        let p = write_file(&lib, &format!("file_{i:02}.jpg"), &vec![i as u8; 4096]);
        items.push(ExportItem {
            source: p,
            labels: vec![],
            belongs_to: None,
        });
    }
    let root = LibraryRoot::new(&lib).unwrap();
    let dest = tmp.path().join("out_cancel");

    let mut wizard = ExportWizard::from_items(items, false, Some(dest));
    wizard
        .request_preview(std::slice::from_ref(&root), None)
        .unwrap();
    wait_until(&mut wizard, |p| {
        matches!(p, ExportPhase::Preview(PreviewState::Ready(_)))
    });

    wizard.start_export(None).unwrap();
    wizard.cancel_export();
    assert!(wizard.is_cancel_requested());

    wait_until(&mut wizard, |p| matches!(p, ExportPhase::Done(_)));
    if let ExportPhase::Done(summary) = wizard.phase() {
        assert!(summary.stopped);
        assert!(summary.manifest_json.exists());
        assert!(summary.manifest_csv.exists());
    } else {
        panic!("expected Done after cancel, got {:?}", wizard.phase());
    }
}

#[test]
fn layout_labels_and_descriptions_are_non_empty_spanish_strings() {
    for layout in ALL_LAYOUTS {
        assert!(!layout_label(layout).is_empty());
        assert!(!layout_description(layout).is_empty());
    }
    assert_eq!(layout_label(Layout::Flat), "Plana");
    assert_eq!(layout_label(Layout::ByCategory), "Por categoría");
    assert_eq!(layout_label(Layout::ByEntity), "Por obra");
    assert_eq!(layout_label(Layout::ByYear), "Por año");
}
