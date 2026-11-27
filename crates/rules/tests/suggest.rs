use catalog::LabelKind;
use rules::{Facts, Suggestion, suggest};

fn plain() -> Facts {
    Facts {
        file_name: "photo.jpg".into(),
        extension: "jpg".into(),
        format: Some("JPEG".into()),
        width: 800,
        height: 600,
        has_camera_exif: false,
        captured_at_ns: None,
        captured_year: None,
        media_type: catalog::MediaType::Image,
        duration_ms: None,
    }
}

fn find(list: &[Suggestion], kind: LabelKind, name: &str) -> Vec<f64> {
    list.iter()
        .filter(|s| s.kind == kind && s.name == name)
        .map(|s| s.confidence)
        .collect()
}

#[test]
fn plain_image_produces_no_suggestions() {
    assert!(suggest(&plain()).is_empty());
}

#[test]
fn rule_1_detects_screenshot_keywords_case_insensitively() {
    for name in [
        "Screenshot_20240101.png",
        "Screen Shot 2024-01-01 at 10.00.png",
        "CAPTURA de pantalla 1.png",
        "scr_0042.jpg",
        "my_SCR_1.png",
    ] {
        let facts = Facts {
            file_name: name.into(),
            ..plain()
        };
        assert_eq!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            ),
            vec![0.90],
            "failed for {name}"
        );
    }

    for near_miss in [
        "screen.png",
        "cap_01.png",
        "script.png",
        "scr.png",
        "shots.png",
    ] {
        let facts = Facts {
            file_name: near_miss.into(),
            ..plain()
        };
        assert!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            )
            .is_empty(),
            "unexpectedly fired for {near_miss}"
        );
    }
}

#[test]
fn rule_2_detects_pc_screen_sizes_and_portrait_swaps_without_exif() {
    let sizes = [
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
        (1366, 768),
        (1536, 864),
        (1440, 900),
        (1600, 900),
        (1280, 720),
        (2560, 1080),
        (3440, 1440),
    ];
    for (w, h) in sizes {
        for (width, height) in [(w, h), (h, w)] {
            let facts = Facts {
                width,
                height,
                ..plain()
            };
            assert_eq!(
                find(
                    &suggest(&facts),
                    LabelKind::Category,
                    "Capturas de pantalla"
                ),
                vec![0.60],
                "failed for {width}x{height}"
            );
        }
    }

    for (width, height) in [(1920, 1081), (1919, 1080), (1280, 721), (1000, 800), (0, 0)] {
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            )
            .is_empty(),
            "unexpectedly fired for {width}x{height}"
        );
    }
}

#[test]
fn rule_2_detects_phone_portrait_dimensions_without_exif() {
    let widths = [720, 828, 1080, 1125, 1170, 1179, 1242, 1284, 1290, 1440];
    for width in widths {
        let height = width * 2; // ratio 2.0, inside [1.9, 2.3]
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert_eq!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            ),
            vec![0.60],
            "failed for {width}x{height}"
        );
    }

    // Exact inclusive bounds: 720 * 1.9 = 1368 and 720 * 2.3 = 1656
    for (width, height) in [(720, 1368), (720, 1656), (1080, 2340), (1170, 2532)] {
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert_eq!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            ),
            vec![0.60],
            "failed for {width}x{height}"
        );
    }

    // Near misses: ratio outside [1.9, 2.3], unlisted width, landscape orientation, zero width
    for (width, height) in [
        (720, 1367),
        (720, 1657),
        (1000, 2000),
        (2340, 1080),
        (0, 1600),
    ] {
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert!(
            find(
                &suggest(&facts),
                LabelKind::Category,
                "Capturas de pantalla"
            )
            .is_empty(),
            "unexpectedly fired for {width}x{height}"
        );
    }
}

#[test]
fn camera_exif_suppresses_rule_2_and_triggers_rule_3() {
    for (width, height) in [(1920, 1080), (1080, 1920), (1080, 2340)] {
        let facts = Facts {
            width,
            height,
            has_camera_exif: true,
            ..plain()
        };
        let out = suggest(&facts);
        assert!(
            find(&out, LabelKind::Category, "Capturas de pantalla").is_empty(),
            "rule 2 should be suppressed when camera EXIF is present ({width}x{height})"
        );
        assert_eq!(find(&out, LabelKind::Tag, "foto de cámara"), vec![0.90]);
    }

    assert!(find(&suggest(&plain()), LabelKind::Tag, "foto de cámara").is_empty());
}

#[test]
fn screenshot_name_and_screen_size_emit_once_with_higher_confidence() {
    let facts = Facts {
        file_name: "Screenshot_2024.png".into(),
        extension: "png".into(),
        format: Some("PNG".into()),
        width: 1920,
        height: 1080,
        has_camera_exif: false,
        captured_at_ns: None,
        captured_year: None,
        media_type: catalog::MediaType::Image,
        duration_ms: None,
    };
    let out = suggest(&facts);
    assert_eq!(
        find(&out, LabelKind::Category, "Capturas de pantalla"),
        vec![0.90]
    );
}

#[test]
fn rule_4_detects_whatsapp_patterns_and_ignores_near_misses() {
    for name in [
        "IMG-20230514-WA0001.jpg",
        "img-20230514-wa0042.jpeg",
        "IMG-20230514-WA12345 (1).jpg",
        "WhatsApp Image 2024-02-10 at 18.20.11.jpeg",
        "whatsapp image 1.png",
        "VID-20230514-WA0007.jpg",
        "vid-20240101-wa0001.png",
    ] {
        let facts = Facts {
            file_name: name.into(),
            ..plain()
        };
        assert_eq!(
            find(&suggest(&facts), LabelKind::Tag, "WhatsApp"),
            vec![0.95],
            "failed for {name}"
        );
    }

    for near_miss in [
        "IMG-2023051-WA0001.jpg",
        "IMG-20230514-WA01.jpg",
        "IMG-20230514-0001.jpg",
        "IMG-abcdefgh-WA0001.jpg",
        "VID-20230514-0001.jpg",
        "VID--WA0001.jpg",
        "VID-20230514-WA.jpg",
        "COVID-19-WA0001.jpg",
        "whats_up.jpg",
    ] {
        let facts = Facts {
            file_name: near_miss.into(),
            ..plain()
        };
        assert!(
            find(&suggest(&facts), LabelKind::Tag, "WhatsApp").is_empty(),
            "unexpectedly fired for {near_miss}"
        );
    }
}

#[test]
fn rule_5_detects_gif_extension_case_insensitively() {
    for ext in ["gif", "GIF", "Gif"] {
        let facts = Facts {
            file_name: format!("anim.{ext}"),
            extension: ext.into(),
            ..plain()
        };
        assert_eq!(find(&suggest(&facts), LabelKind::Tag, "GIF"), vec![0.95]);
    }

    for (name, ext) in [
        ("gif_party.jpg", "jpg"),
        ("a.png", "png"),
        ("a.gif2", "gif2"),
        ("gif", ""),
    ] {
        let facts = Facts {
            file_name: name.into(),
            extension: ext.into(),
            ..plain()
        };
        assert!(
            find(&suggest(&facts), LabelKind::Tag, "GIF").is_empty(),
            "unexpectedly fired for {name} / {ext}"
        );
    }
}

#[test]
fn rule_6_detects_very_tall_vertical_images() {
    for (width, height) in [(1000, 2500), (800, 3200), (720, 5000)] {
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert_eq!(
            find(&suggest(&facts), LabelKind::Tag, "formato vertical largo"),
            vec![0.50],
            "failed for {width}x{height}"
        );
    }

    for (width, height) in [(1000, 2499), (2500, 1000), (1080, 2340), (0, 5000)] {
        let facts = Facts {
            width,
            height,
            ..plain()
        };
        assert!(
            find(&suggest(&facts), LabelKind::Tag, "formato vertical largo").is_empty(),
            "unexpectedly fired for {width}x{height}"
        );
    }
}

#[test]
fn multiple_rules_can_fire_together() {
    let facts = Facts {
        file_name: "Screenshot_IMG-20240101-WA0001.gif".into(),
        extension: "gif".into(),
        format: Some("GIF".into()),
        width: 800,
        height: 2400,
        has_camera_exif: false,
        captured_at_ns: None,
        captured_year: None,
        media_type: catalog::MediaType::Image,
        duration_ms: None,
    };
    let out = suggest(&facts);
    assert_eq!(
        find(&out, LabelKind::Category, "Capturas de pantalla"),
        vec![0.90]
    );
    assert_eq!(find(&out, LabelKind::Tag, "WhatsApp"), vec![0.95]);
    assert_eq!(find(&out, LabelKind::Tag, "GIF"), vec![0.95]);
    assert_eq!(
        find(&out, LabelKind::Tag, "formato vertical largo"),
        vec![0.50]
    );
    assert_eq!(out.len(), 4);
}

#[test]
fn video_at_screen_resolution_is_not_classified_as_screenshot() {
    let facts = Facts {
        file_name: "clip.mp4".into(),
        extension: "mp4".into(),
        format: Some("MP4".into()),
        width: 1920,
        height: 1080,
        has_camera_exif: false,
        captured_at_ns: None,
        captured_year: None,
        media_type: catalog::MediaType::Video,
        duration_ms: Some(12_000),
    };
    let out = suggest(&facts);
    assert!(
        find(&out, LabelKind::Category, "Capturas de pantalla").is_empty(),
        "video 1920x1080 should not be suggested as screenshot: {out:?}"
    );
}
