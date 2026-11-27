// Tests create temporary library roots outside any real user folder.
#![allow(clippy::disallowed_methods)]

use app::demo::{DEFAULT_DEMO_COUNT, MAX_DEMO_COUNT};
use app::screenshot::{
    CaptureAction, CaptureState, CaptureView, CliArgs, DEFAULT_SCREENSHOT_DELAY_FRAMES,
    DEFAULT_WINDOW_SIZE, FrameSignals, guard_screenshot_path, parse_args, parse_theme, parse_view,
    parse_window_size, usage_text, write_screenshot_png,
};
use app::settings::Theme;
use eframe::egui::{Color32, ColorImage};
use fsguard::{GuardError, LibraryRoot};
use std::fs;
use std::path::PathBuf;

#[test]
fn parse_args_defaults_and_all_flags() {
    let empty: [&str; 0] = [];
    let defaults = parse_args(empty).unwrap();
    assert_eq!(defaults, CliArgs::default());
    assert_eq!(defaults.demo_count, DEFAULT_DEMO_COUNT);
    assert_eq!(defaults.window_size, DEFAULT_WINDOW_SIZE);
    assert_eq!(
        defaults.screenshot_delay_frames,
        DEFAULT_SCREENSHOT_DELAY_FRAMES
    );
    assert_eq!(defaults.view, CaptureView::Gallery);

    let full = parse_args([
        "--demo",
        "--demo-count",
        "42",
        "--data-dir",
        "demo_dir",
        "--screenshot",
        "out/shot.png",
        "--screenshot-delay-frames",
        "5",
        "--window-size",
        "1024x768",
        "--theme",
        "light",
        "--view",
        "export",
    ])
    .unwrap();
    assert!(full.demo);
    assert_eq!(full.demo_count, 42);
    assert_eq!(full.data_dir, Some(PathBuf::from("demo_dir")));
    assert_eq!(full.screenshot, Some(PathBuf::from("out/shot.png")));
    assert_eq!(full.screenshot_delay_frames, 5);
    assert_eq!(full.window_size, [1024, 768]);
    assert_eq!(full.theme, Some(Theme::Light));
    assert_eq!(full.view, CaptureView::Export);
    assert!(!full.self_test);
    assert!(!full.help);

    let help = parse_args(["--help"]).unwrap();
    assert!(help.help);
    assert!(usage_text().contains("--demo"));
    assert!(usage_text().contains("--screenshot"));
    assert!(usage_text().contains("--view"));
}

#[test]
fn parse_args_rejects_invalid_values_and_unknown_flags() {
    assert!(parse_args(["--unknown"]).is_err());
    assert!(parse_args(["--demo-count"]).is_err());
    assert!(parse_args(["--demo-count", "0"]).is_err());
    assert!(parse_args(["--demo-count", &(MAX_DEMO_COUNT + 1).to_string()]).is_err());
    assert!(parse_args(["--demo-count", "abc"]).is_err());
    assert!(parse_args(["--screenshot"]).is_err());
    assert!(parse_args(["--screenshot-delay-frames", "-1"]).is_err());
    assert!(parse_args(["--window-size", "1280"]).is_err());
    assert!(parse_args(["--window-size", "0x800"]).is_err());
    assert!(parse_args(["--window-size", "100x100"]).is_err());
    assert!(parse_args(["--theme", "sepia"]).is_err());
    assert!(parse_args(["--data-dir"]).is_err());
    assert!(parse_args(["--view"]).is_err());

    let bad_view = parse_args(["--view", "ajustes"]).unwrap_err();
    assert!(
        bad_view.contains("ajustes") && bad_view.contains("--view"),
        "expected Spanish error mentioning invalid view and flag, got: {bad_view}"
    );

    assert_eq!(parse_window_size("1280X820").unwrap(), [1280, 820]);
    assert_eq!(parse_theme("DARK").unwrap(), Theme::Dark);
    assert_eq!(parse_theme("Light").unwrap(), Theme::Light);

    assert_eq!(parse_view("gallery").unwrap(), CaptureView::Gallery);
    assert_eq!(parse_view("settings").unwrap(), CaptureView::Settings);
    assert_eq!(parse_view("welcome").unwrap(), CaptureView::Welcome);
    assert_eq!(parse_view("suggestions").unwrap(), CaptureView::Suggestions);
    assert_eq!(parse_view("REVIEW").unwrap(), CaptureView::Suggestions);
    assert_eq!(parse_view("Duplicates").unwrap(), CaptureView::Duplicates);
    assert_eq!(parse_view("works").unwrap(), CaptureView::Works);
    assert_eq!(parse_view("personas").unwrap(), CaptureView::Persons);
    assert_eq!(parse_view("tags").unwrap(), CaptureView::Tags);
    assert_eq!(parse_view("export").unwrap(), CaptureView::Export);
    assert_eq!(parse_view("menu").unwrap(), CaptureView::Menu);
    let err = parse_view("mapa").unwrap_err();
    assert!(err.contains("Vista") && err.contains("mapa"), "{err}");
}

#[test]
fn guard_screenshot_path_refuses_paths_inside_library_and_accepts_outside() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("demo_lib");
    fs::create_dir_all(&lib).unwrap();
    let root = LibraryRoot::new(&lib).unwrap();
    let roots = [root];

    // Direct file inside the library.
    let inside = lib.join("shot.png");
    let err = guard_screenshot_path(&inside, &roots).unwrap_err();
    assert!(matches!(err, GuardError::InsideLibrary { .. }));

    // Nested file in a not-yet-existing subfolder inside the library.
    let nested = lib.join("nested/dir/shot.png");
    let err = guard_screenshot_path(&nested, &roots).unwrap_err();
    assert!(matches!(err, GuardError::InsideLibrary { .. }));

    // Case-insensitive match on Windows NTFS.
    let upper = tmp.path().join("DEMO_LIB").join("shot.png");
    let err = guard_screenshot_path(&upper, &roots).unwrap_err();
    assert!(matches!(err, GuardError::InsideLibrary { .. }));

    // Parent-dir traversal refused.
    let traversal = tmp.path().join("out").join("..").join("shot.png");
    let err = guard_screenshot_path(&traversal, &roots).unwrap_err();
    assert!(matches!(err, GuardError::ParentDirComponent(_)));

    // Sibling path outside the library is accepted.
    let outside = tmp.path().join("shots").join("shot.png");
    let dest = guard_screenshot_path(&outside, &roots).unwrap();
    assert!(dest.as_path().ends_with("shot.png"));
}

#[test]
fn capture_state_selects_first_waits_for_readiness_delays_then_captures() {
    let mut state = CaptureState::new(2, 10, 5);

    // 1. Entries exist, nothing selected yet -> SelectFirst.
    let a = state.step(FrameSignals {
        has_entries: true,
        ..Default::default()
    });
    assert_eq!(a, CaptureAction::SelectFirst);

    // 2. Selected, but thumbs and preview not ready yet -> WaitForWorkers.
    for _ in 0..2 {
        let a = state.step(FrameSignals {
            has_entries: true,
            has_selection: true,
            ..Default::default()
        });
        assert_eq!(a, CaptureAction::WaitForWorkers);
    }

    // 3. Thumbs ready, preview still loading -> WaitForWorkers.
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            has_selection: true,
            thumbs_ready: true,
            ..Default::default()
        }),
        CaptureAction::WaitForWorkers
    );

    // 4. Both ready -> waits `delay_frames` (2) frames before RequestScreenshot.
    let ready = FrameSignals {
        has_entries: true,
        has_selection: true,
        thumbs_ready: true,
        preview_ready: true,
        ..Default::default()
    };
    assert_eq!(state.step(ready), CaptureAction::WaitFrame);
    assert_eq!(state.step(ready), CaptureAction::WaitFrame);
    assert_eq!(state.step(ready), CaptureAction::RequestScreenshot);

    // 5. Waiting for callback image -> WaitFrame, then SaveAndClose once received.
    assert_eq!(state.step(ready), CaptureAction::WaitFrame);
    assert_eq!(
        state.step(FrameSignals {
            image_received: true,
            ..ready
        }),
        CaptureAction::SaveAndClose
    );

    // 6. Terminal Done state.
    assert_eq!(state.step(ready), CaptureAction::Done);
}

#[test]
fn capture_state_review_and_duplicates_wait_for_thumbs_without_selecting_first() {
    for view in [CaptureView::Suggestions, CaptureView::Duplicates] {
        let mut state = CaptureState::for_view(view, 1, 10, 5);

        // Does not emit SelectFirst or OpenExport; waits while thumbs are loading.
        assert_eq!(
            state.step(FrameSignals {
                has_entries: true,
                has_selection: false,
                thumbs_ready: false,
                preview_ready: false,
                ..Default::default()
            }),
            CaptureAction::WaitForWorkers
        );

        // Once thumbs are ready (even without selection/preview), settles then captures.
        let ready = FrameSignals {
            has_entries: true,
            thumbs_ready: true,
            ..Default::default()
        };
        assert_eq!(state.step(ready), CaptureAction::WaitFrame);
        assert_eq!(state.step(ready), CaptureAction::RequestScreenshot);
        assert_eq!(
            state.step(FrameSignals {
                image_received: true,
                ..ready
            }),
            CaptureAction::SaveAndClose
        );
        assert_eq!(state.step(ready), CaptureAction::Done);
    }
}

#[test]
fn capture_state_export_opens_wizard_and_waits_for_preview_ready() {
    let mut state = CaptureState::for_view(CaptureView::Export, 1, 10, 5);

    // 1. Entries exist, wizard not yet open -> OpenExport (not SelectFirst).
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            export_open: false,
            ..Default::default()
        }),
        CaptureAction::OpenExport
    );

    // 2. Wizard open and thumbs ready, but export preview still computing -> WaitForWorkers.
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            thumbs_ready: true,
            export_open: true,
            export_ready: false,
            ..Default::default()
        }),
        CaptureAction::WaitForWorkers
    );

    // 3. Export preview ready, but gallery thumbs behind modal still loading -> WaitForWorkers.
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            thumbs_ready: false,
            export_open: true,
            export_ready: true,
            ..Default::default()
        }),
        CaptureAction::WaitForWorkers
    );

    // 4. Both ready -> settle 1 frame, then RequestScreenshot -> SaveAndClose.
    let ready = FrameSignals {
        has_entries: true,
        thumbs_ready: true,
        export_open: true,
        export_ready: true,
        ..Default::default()
    };
    assert_eq!(state.step(ready), CaptureAction::WaitFrame);
    assert_eq!(state.step(ready), CaptureAction::RequestScreenshot);
    assert_eq!(
        state.step(FrameSignals {
            image_received: true,
            ..ready
        }),
        CaptureAction::SaveAndClose
    );
    assert_eq!(state.step(ready), CaptureAction::Done);
}

#[test]
fn capture_state_export_fails_fast_when_preview_errors() {
    let mut state = CaptureState::for_view(CaptureView::Export, 1, 10, 5);
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            export_open: false,
            ..Default::default()
        }),
        CaptureAction::OpenExport
    );
    let action = state.step(FrameSignals {
        has_entries: true,
        export_open: true,
        export_failed: true,
        ..Default::default()
    });
    assert!(matches!(action, CaptureAction::FailAndClose(_)));
    assert_eq!(
        state.step(FrameSignals {
            has_entries: true,
            export_open: true,
            export_failed: true,
            ..Default::default()
        }),
        CaptureAction::Done
    );
}

#[test]
fn capture_state_times_out_when_workers_or_capture_stall() {
    let mut stuck_thumbs = CaptureState::new(1, 3, 5);
    let not_ready = FrameSignals {
        has_entries: true,
        has_selection: true,
        ..Default::default()
    };
    assert_eq!(stuck_thumbs.step(not_ready), CaptureAction::WaitForWorkers);
    assert_eq!(stuck_thumbs.step(not_ready), CaptureAction::WaitForWorkers);
    assert!(matches!(
        stuck_thumbs.step(not_ready),
        CaptureAction::FailAndClose(_)
    ));
    assert_eq!(stuck_thumbs.step(not_ready), CaptureAction::Done);

    let mut stuck_capture = CaptureState::new(0, 5, 2);
    let ready = FrameSignals {
        has_entries: true,
        has_selection: true,
        thumbs_ready: true,
        preview_ready: true,
        ..Default::default()
    };
    assert_eq!(stuck_capture.step(ready), CaptureAction::RequestScreenshot);
    assert_eq!(stuck_capture.step(ready), CaptureAction::WaitFrame);
    assert!(matches!(
        stuck_capture.step(ready),
        CaptureAction::FailAndClose(_)
    ));
}

#[test]
fn write_screenshot_png_saves_valid_png_and_normalises_hidpi_scale() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    fs::create_dir_all(&lib).unwrap();
    let roots = [LibraryRoot::new(&lib).unwrap()];

    // Simulate a 2x HiDPI framebuffer (160x100) for a requested 80x50 window.
    let mut pixels = Vec::with_capacity(160 * 100);
    for y in 0..100u32 {
        for x in 0..160u32 {
            pixels.push(Color32::from_rgba_unmultiplied(
                (x % 256) as u8,
                (y * 2 % 256) as u8,
                120,
                255,
            ));
        }
    }
    let color_img = ColorImage::new([160, 100], pixels);

    let out_path = tmp.path().join("nested_out").join("shot.png");
    let dest = guard_screenshot_path(&out_path, &roots).unwrap();
    let saved = write_screenshot_png(&dest, &roots, &color_img, Some([80, 50])).unwrap();

    assert_eq!((saved.width, saved.height), (80, 50));
    assert!(saved.bytes > 0);
    let decoded = image::open(&saved.path).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (80, 50));
}
