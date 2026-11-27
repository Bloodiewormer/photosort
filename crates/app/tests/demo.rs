// Tests inspect synthetic demo output inside temp directories only.
#![allow(clippy::disallowed_methods)]

use app::demo::{
    DEFAULT_DEMO_COUNT, DEFAULT_SEED, DEMO_EXPORT_SUBDIR, DemoConfig, DemoFormat, MAX_DEMO_COUNT,
    PatternKind, SeedDecision, demo_export_dir, plan_demo_images, plan_label_seeds, prepare_demo,
    render_demo_image, validate_count,
};
use app::settings::{self, Theme};
use catalog::{Catalog, Filter, LabelKind, Sort, Source, Status};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use walkdir::WalkDir;

#[test]
fn validate_count_enforces_bounds() {
    assert_eq!(validate_count(1), Ok(1));
    assert_eq!(validate_count(DEFAULT_DEMO_COUNT), Ok(DEFAULT_DEMO_COUNT));
    assert_eq!(validate_count(MAX_DEMO_COUNT), Ok(MAX_DEMO_COUNT));
    assert!(validate_count(0).is_err());
    assert!(validate_count(MAX_DEMO_COUNT + 1).is_err());
}

#[test]
fn plan_demo_images_is_deterministic_and_covers_required_variety() {
    let a = plan_demo_images(48, DEFAULT_SEED);
    let b = plan_demo_images(48, DEFAULT_SEED);
    assert_eq!(a, b);
    assert_eq!(a.len(), 48);

    // Distinct relative paths across multiple subfolders.
    let rel_paths: HashSet<_> = a.iter().map(|s| s.rel_path.clone()).collect();
    assert_eq!(rel_paths.len(), 48);
    let folders: HashSet<_> = a
        .iter()
        .filter_map(|s| s.rel_path.parent().map(Path::to_path_buf))
        .collect();
    assert!(folders.len() >= 4, "expected >=4 folders, got {folders:?}");

    // Aspect ratios & dimensions: square, 16:9, 9:16, tall >= 1:2.5, tiny.
    assert!(a.iter().any(|s| s.width == s.height && s.width >= 120));
    assert!(a.iter().any(|s| s.width * 9 == s.height * 16));
    assert!(a.iter().any(|s| s.height * 9 == s.width * 16));
    assert!(a.iter().any(|s| s.height >= s.width * 3));
    assert!(a.iter().any(|s| s.width <= 64 && s.height <= 64));

    // Formats, alpha, patterns, and exact duplicates.
    assert!(a.iter().any(|s| s.format == DemoFormat::Png));
    assert!(a.iter().any(|s| s.format == DemoFormat::Jpeg));
    assert!(a.iter().any(|s| s.format == DemoFormat::Gif));
    assert!(a.iter().any(|s| s.with_alpha));
    for pat in [
        PatternKind::Gradient,
        PatternKind::Stripes,
        PatternKind::Checker,
        PatternKind::ColorBlocks,
    ] {
        assert!(
            a.iter().any(|s| s.pattern == pat),
            "missing pattern {pat:?}"
        );
    }
    assert!(
        a.iter().filter(|s| s.duplicate_of.is_some()).count() >= 2,
        "expected at least 2 exact duplicate specs"
    );

    // Filenames exercising rules crate.
    let names: Vec<String> = a
        .iter()
        .map(|s| {
            s.rel_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(names.iter().any(|n| n == "Screenshot_2024-01-02.png"));
    assert!(names.iter().any(|n| n == "IMG-20240101-WA0001.jpg"));
}

#[test]
fn render_demo_image_produces_valid_deterministic_png_jpeg_and_gif_bytes() {
    let specs = plan_demo_images(24, DEFAULT_SEED);

    let png_spec = specs
        .iter()
        .find(|s| s.format == DemoFormat::Png && s.with_alpha)
        .expect("alpha png spec");
    let jpg_spec = specs
        .iter()
        .find(|s| s.format == DemoFormat::Jpeg)
        .expect("jpeg spec");
    let gif_spec = specs
        .iter()
        .find(|s| s.format == DemoFormat::Gif)
        .expect("gif spec");

    for spec in [png_spec, jpg_spec, gif_spec] {
        let bytes1 = render_demo_image(spec, DEFAULT_SEED).unwrap();
        let bytes2 = render_demo_image(spec, DEFAULT_SEED).unwrap();
        assert_eq!(
            bytes1, bytes2,
            "non-deterministic render for {:?}",
            spec.rel_path
        );

        let format = image::guess_format(&bytes1).unwrap();
        let expected_fmt = match spec.format {
            DemoFormat::Png => image::ImageFormat::Png,
            DemoFormat::Jpeg => image::ImageFormat::Jpeg,
            DemoFormat::Gif => image::ImageFormat::Gif,
        };
        assert_eq!(format, expected_fmt);

        let decoded = image::load_from_memory(&bytes1).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (spec.width, spec.height)
        );

        if spec.with_alpha {
            let rgba = decoded.to_rgba8();
            assert!(
                rgba.pixels().any(|p| p[3] < 255),
                "expected transparent/translucent pixels in {:?}",
                spec.rel_path
            );
        }
    }
}

#[test]
fn plan_label_seeds_is_deterministic_and_covers_kinds_sources_confidences_and_decisions() {
    let specs = plan_demo_images(48, DEFAULT_SEED);
    let plan1 = plan_label_seeds(&specs, DEFAULT_SEED);
    let plan2 = plan_label_seeds(&specs, DEFAULT_SEED);
    assert_eq!(plan1, plan2);
    assert!(!plan1.is_empty());

    assert!(plan1.iter().any(|s| s.kind == LabelKind::Category));
    assert!(plan1.iter().any(|s| s.kind == LabelKind::Tag));
    assert!(plan1.iter().any(|s| s.kind == LabelKind::Entity));

    assert!(plan1.iter().any(|s| s.source == Source::Ai));
    assert!(plan1.iter().any(|s| s.source == Source::Ocr));
    assert!(plan1.iter().any(|s| s.source == Source::Rules));

    assert!(plan1.iter().any(|s| s.decision == SeedDecision::Suggested));
    assert!(plan1.iter().any(|s| matches!(
        s.decision,
        SeedDecision::Accepted | SeedDecision::AcceptedUser
    )));
    assert!(plan1.iter().any(|s| s.decision == SeedDecision::Rejected));

    let min_c = plan1
        .iter()
        .filter(|s| s.decision == SeedDecision::Suggested)
        .map(|s| s.confidence)
        .fold(f64::INFINITY, f64::min);
    let max_c = plan1
        .iter()
        .filter(|s| s.decision == SeedDecision::Suggested)
        .map(|s| s.confidence)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        min_c <= 0.30,
        "expected low confidence <= 0.30, got {min_c}"
    );
    assert!(
        max_c >= 0.90,
        "expected high confidence >= 0.90, got {max_c}"
    );
}

#[test]
fn prepare_demo_indexes_runs_rules_seeds_labels_and_detects_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("demo_run");

    let setup = prepare_demo(
        Some(&base),
        DemoConfig {
            count: 36,
            seed: DEFAULT_SEED,
        },
        Some(Theme::Light),
    )
    .unwrap();

    assert_eq!(setup.file_count, 36);
    assert!(setup.duplicate_groups >= 2);
    assert!(
        setup.rules_report.errors.is_empty(),
        "{:?}",
        setup.rules_report.errors
    );
    assert!(setup.rules_report.analyzed > 0);
    assert!(setup.rules_report.suggestions > 0);
    assert!(setup.seeded_assignments > 0);

    // Settings register ONLY the synthetic demo library and record requested theme.
    let loaded = settings::load(&setup.data_dir);
    assert!(loaded.warning.is_none());
    assert_eq!(loaded.settings.theme, Theme::Light);
    assert_eq!(loaded.settings.library_roots.len(), 1);
    assert_eq!(
        fs::canonicalize(&loaded.settings.library_roots[0]).unwrap(),
        fs::canonicalize(&setup.library_dir).unwrap()
    );

    // Open catalog and verify counts, duplicates, rules suggestions, and seeded states.
    let roots = loaded.settings.valid_roots();
    let db = settings::db_dest_path(&setup.data_dir, &roots).unwrap();
    let cat = Catalog::open(&db).unwrap();

    let counts = cat.counts().unwrap();
    assert_eq!(counts.locations, 36);
    assert!(
        counts.assets < counts.locations,
        "duplicates should share asset_id"
    );
    assert_eq!(counts.missing, 0);

    let dupes = cat.duplicates().unwrap();
    assert!(
        dupes.len() >= 2,
        "expected >= 2 duplicate groups, got {}",
        dupes.len()
    );

    // Verify rules crate produced its expected suggestions on the demo library.
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    assert_eq!(entries.len(), 36);

    let mut saw_rules_screenshot = false;
    let mut saw_rules_whatsapp = false;
    let mut saw_rules_gif = false;
    let mut saw_rules_tall = false;
    let mut saw_accepted = false;
    let mut saw_rejected = false;
    let mut saw_suggested = false;
    let mut saw_entity = false;

    for e in &entries {
        for a in cat.labels_of(e.asset).unwrap() {
            if a.source == Source::Rules && a.name == "Capturas de pantalla" {
                saw_rules_screenshot = true;
            }
            if a.source == Source::Rules && a.name == "WhatsApp" {
                saw_rules_whatsapp = true;
            }
            if a.source == Source::Rules && a.name == "GIF" {
                saw_rules_gif = true;
            }
            if a.source == Source::Rules && a.name == "formato vertical largo" {
                saw_rules_tall = true;
            }
            match a.status {
                Status::Accepted => saw_accepted = true,
                Status::Rejected => saw_rejected = true,
                Status::Suggested => saw_suggested = true,
            }
            if a.kind == LabelKind::Entity {
                saw_entity = true;
            }
        }
    }

    assert!(
        saw_rules_screenshot,
        "expected rules suggestion Capturas de pantalla"
    );
    assert!(saw_rules_whatsapp, "expected rules suggestion WhatsApp");
    assert!(saw_rules_gif, "expected rules suggestion GIF");
    assert!(
        saw_rules_tall,
        "expected rules suggestion formato vertical largo"
    );
    assert!(saw_accepted, "expected accepted assignments");
    assert!(saw_rejected, "expected rejected assignments");
    assert!(saw_suggested, "expected suggested assignments");
    assert!(saw_entity, "expected entity assignments");

    // First entry in PathAsc order has rich labels for the inspector screenshot.
    let first_labels = cat.labels_of(entries[0].asset).unwrap();
    assert!(
        first_labels.iter().any(|a| a.status == Status::Accepted),
        "first entry should have an accepted label"
    );
    assert!(
        first_labels.iter().any(|a| a.status == Status::Suggested),
        "first entry should have a suggested label"
    );
    assert!(
        first_labels.iter().any(|a| a.status == Status::Rejected),
        "first entry should have a rejected label"
    );
}

#[test]
fn prepare_demo_leaves_no_files_outside_its_base_dir() {
    let parent = tempfile::tempdir().unwrap();
    let base = parent.path().join("sandbox");

    let setup = prepare_demo(
        Some(&base),
        DemoConfig {
            count: 24,
            seed: DEFAULT_SEED,
        },
        None,
    )
    .unwrap();

    // Every single entry inside `parent` must be `base` itself or strictly inside `base`.
    let canonical_base = fs::canonicalize(&base).unwrap();
    for entry in WalkDir::new(parent.path()).into_iter().skip(1) {
        let entry = entry.unwrap();
        let canonical = fs::canonicalize(entry.path()).unwrap();
        assert!(
            canonical.starts_with(&canonical_base),
            "found file outside demo base dir: {}",
            canonical.display()
        );
    }

    // Also verify the default fresh temp-dir mode cleans up completely when dropped.
    let temp_setup = prepare_demo(
        None,
        DemoConfig {
            count: 12,
            seed: DEFAULT_SEED,
        },
        None,
    )
    .unwrap();
    let temp_base = temp_setup.base_dir.clone();
    // The other paths come out of the path guard fully resolved. The temp folder may not be: on
    // some machines `TEMP` is a short 8.3 name or goes through a link.
    let resolved_base = dunce::canonicalize(&temp_base).unwrap();
    assert!(temp_setup.library_dir.starts_with(&resolved_base));
    assert!(temp_setup.data_dir.starts_with(&resolved_base));
    assert!(fs::metadata(&temp_base).is_ok());
    drop(temp_setup);
    assert!(
        !temp_base.exists(),
        "temp demo dir should be cleaned up on drop: {}",
        temp_base.display()
    );
    drop(setup);
}

#[test]
fn demo_export_dir_is_inside_base_dir_and_outside_library_root() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("demo_export_test");
    let setup = prepare_demo(
        Some(&base),
        DemoConfig {
            count: 12,
            seed: DEFAULT_SEED,
        },
        None,
    )
    .unwrap();

    let loaded = settings::load(&setup.data_dir);
    let roots = loaded.settings.valid_roots();
    assert_eq!(roots.len(), 1);

    let dest = demo_export_dir(&setup.base_dir, &roots).unwrap();
    let canonical_base = fs::canonicalize(&setup.base_dir).unwrap();
    let canonical_lib = fs::canonicalize(&setup.library_dir).unwrap();
    let canonical_parent = fs::canonicalize(dest.as_path().parent().unwrap()).unwrap();
    assert_eq!(
        canonical_parent,
        canonical_base,
        "export dir {} must be directly inside demo base {}",
        dest.as_path().display(),
        canonical_base.display()
    );
    assert!(
        !canonical_parent.starts_with(&canonical_lib),
        "export dir {} must stay outside library {}",
        dest.as_path().display(),
        canonical_lib.display()
    );
    assert_eq!(
        dest.as_path().file_name().and_then(|n| n.to_str()),
        Some(DEMO_EXPORT_SUBDIR)
    );
    // Rejects a base_dir inside the library root.
    assert!(demo_export_dir(&setup.library_dir, &roots).is_err());
}
