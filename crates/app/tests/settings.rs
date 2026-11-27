// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::settings::{Settings, SettingsError, Theme, load, resolve_data_dir, save};
use std::fs;
use std::path::Path;

#[test]
fn missing_settings_file_loads_defaults_with_dark_theme() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let out = load(&data);
    assert_eq!(out.settings, Settings::default());
    assert_eq!(out.settings.theme, Theme::Dark);
    assert!(out.settings.library_roots.is_empty());
    assert!(out.warning.is_none());
}

#[test]
fn save_and_load_roundtrip_creates_dir_and_leaves_no_temp_file() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let lib = tmp.path().join("photos");
    fs::create_dir_all(&lib).unwrap();

    let mut s = Settings::default();
    let added = s.add_library_root(&lib, &data).unwrap();
    s.theme = Theme::Light;
    s.last_export_dir = Some(tmp.path().join("exports"));

    save(&data, &s).unwrap();
    assert!(!data.join("settings.json.tmp").exists());

    // Saving a second time overwrites atomically without error.
    s.theme = Theme::System;
    save(&data, &s).unwrap();

    let loaded = load(&data);
    assert!(loaded.warning.is_none());
    assert_eq!(loaded.settings.theme, Theme::System);
    assert_eq!(loaded.settings.library_roots, vec![added]);
    assert_eq!(
        loaded.settings.last_export_dir,
        Some(tmp.path().join("exports"))
    );
}

#[test]
fn adding_a_root_that_contains_or_is_inside_data_dir_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("app_data");
    let inside_data = data.join("nested_lib");
    fs::create_dir_all(&inside_data).unwrap();

    let mut s = Settings::default();

    // Candidate is a parent of data_dir (contains data_dir).
    let err_parent = s.add_library_root(tmp.path(), &data).unwrap_err();
    assert!(
        matches!(err_parent, SettingsError::RootContainsDataDir),
        "unexpected error: {err_parent:?}"
    );
    assert!(!err_parent.to_string().is_empty());

    // Candidate equals data_dir itself.
    let err_same = s.add_library_root(&data, &data).unwrap_err();
    assert!(
        matches!(err_same, SettingsError::RootContainsDataDir),
        "unexpected error: {err_same:?}"
    );

    // Candidate is inside data_dir.
    let err_inside = s.add_library_root(&inside_data, &data).unwrap_err();
    assert!(
        matches!(err_inside, SettingsError::RootInsideDataDir),
        "unexpected error: {err_inside:?}"
    );
    assert!(!err_inside.to_string().is_empty());

    assert!(s.library_roots.is_empty());
}

#[test]
fn adding_the_same_library_root_twice_deduplicates_and_remove_works() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let lib = tmp.path().join("photos");
    fs::create_dir_all(&lib).unwrap();

    let mut s = Settings::default();
    let p1 = s.add_library_root(&lib, &data).unwrap();
    let p2 = s.add_library_root(&lib, &data).unwrap();
    assert_eq!(p1, p2);
    assert_eq!(s.library_roots.len(), 1);

    assert!(s.remove_library_root(&p1));
    assert!(s.library_roots.is_empty());
}

#[test]
fn corrupt_settings_falls_back_to_defaults_and_never_overwrites_bad_backup() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();

    // First corruption: moved to settings.json.bad.
    fs::write(data.join("settings.json"), b"{ broken json 1").unwrap();
    let first = load(&data);
    assert_eq!(first.settings, Settings::default());
    assert!(first.warning.is_some());
    assert_eq!(
        fs::read_to_string(data.join("settings.json.bad")).unwrap(),
        "{ broken json 1"
    );

    // Second corruption while settings.json.bad already exists: .bad MUST NOT be overwritten.
    fs::write(data.join("settings.json"), b"{ broken json 2").unwrap();
    let second = load(&data);
    assert_eq!(second.settings, Settings::default());
    assert!(second.warning.is_some());
    assert_eq!(
        fs::read_to_string(data.join("settings.json.bad")).unwrap(),
        "{ broken json 1"
    );
}

#[test]
fn resolve_data_dir_prefers_cli_override() {
    let p = resolve_data_dir(Some(Path::new("custom-dir")));
    assert_eq!(p, Path::new("custom-dir"));
}

#[test]
fn old_settings_without_auto_analyze_loads_with_default_true_and_roundtrips_false() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();

    assert!(Settings::default().auto_analyze);

    // Simulate an older settings.json written before `auto_analyze` existed.
    fs::write(
        data.join("settings.json"),
        br#"{"library_roots":[],"last_export_dir":null,"theme":"Light"}"#,
    )
    .unwrap();

    let loaded = load(&data);
    assert!(loaded.warning.is_none(), "{:?}", loaded.warning);
    assert_eq!(loaded.settings.theme, Theme::Light);
    assert!(loaded.settings.auto_analyze);

    let mut updated = loaded.settings;
    updated.auto_analyze = false;
    save(&data, &updated).unwrap();

    let reloaded = load(&data);
    assert!(reloaded.warning.is_none());
    assert!(!reloaded.settings.auto_analyze);
}

#[test]
fn old_settings_without_background_review_fields_loads_with_safe_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();

    fs::write(
        data.join("settings.json"),
        br#"{"library_roots":[],"last_export_dir":null,"theme":"Dark","auto_analyze":true}"#,
    )
    .unwrap();

    let loaded = load(&data);
    assert!(loaded.warning.is_none(), "{:?}", loaded.warning);
    assert!(!loaded.settings.background_review);
    assert!(loaded.settings.stage_rules);
    assert!(!loaded.settings.stage_vision);
    assert!(!loaded.settings.stage_ocr);
}

#[test]
fn a_data_dir_refused_by_the_guard_is_named_in_the_error() {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();
    let s = Settings {
        library_roots: vec![library.clone()],
        ..Settings::default()
    };

    let err = save(&library.join("data"), &s).unwrap_err();
    assert!(matches!(err, SettingsError::Guard(_)), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("carpeta de datos"), "{text}");
    assert!(text.contains("«library»"), "{text}");
}
