use catalog::{Catalog, CatalogError, LabelKind, Source};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

pub const STAGE: &str = "rules";
pub const VERSION: u32 = 1;

const BATCH_SIZE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub file_name: String,
    pub extension: String,
    pub width: u32,
    pub height: u32,
    pub has_camera_exif: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub kind: LabelKind,
    pub name: &'static str,
    pub confidence: f64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub analyzed: usize,
    pub suggestions: usize,
    pub errors: Vec<(PathBuf, String)>,
}

/// Reads ONLY header/metadata: image dimensions via `image::image_dimensions` (no full decode)
/// and camera Make/Model presence via `exif`. Read-only.
pub fn read_facts(path: &Path) -> io::Result<Facts> {
    let (width, height) = image::image_dimensions(path).map_err(|e| match e {
        image::ImageError::IoError(io_err) => io_err,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    })?;
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let has_camera_exif = exif::Reader::new()
        .read_from_container(&mut reader)
        .map(|data| {
            has_non_empty_tag(&data, exif::Tag::Make) || has_non_empty_tag(&data, exif::Tag::Model)
        })
        .unwrap_or(false);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Ok(Facts {
        file_name,
        extension,
        width,
        height,
        has_camera_exif,
    })
}

fn has_non_empty_tag(data: &exif::Exif, tag: exif::Tag) -> bool {
    data.get_field(tag, exif::In::PRIMARY)
        .is_some_and(|field| match &field.value {
            exif::Value::Ascii(values) => values
                .iter()
                .any(|s| s.iter().any(|&b| b != 0 && !b.is_ascii_whitespace())),
            _ => !field.display_value().to_string().trim().is_empty(),
        })
}

/// Processes `catalog.pending(STAGE, VERSION, ..)` in batches until none is left: `read_facts`,
/// `suggest`, then `catalog.label(kind, name)` + `catalog.suggest(asset, label, Source::Rules, confidence)`
/// for each, then `mark_done`. An unreadable file is reported in `errors` and NOT marked done
/// (it is retried next run). Never touches user decisions. Safe to run repeatedly.
pub fn run(catalog: &Catalog) -> Result<RunReport, CatalogError> {
    let mut report = RunReport::default();
    let mut failed: HashSet<i64> = HashSet::new();

    loop {
        let batch = catalog.pending(STAGE, VERSION, BATCH_SIZE.saturating_add(failed.len()))?;
        let mut progressed = false;
        for item in batch {
            if failed.contains(&item.asset.0) {
                continue;
            }
            progressed = true;
            let facts = match read_facts(&item.path) {
                Ok(facts) => facts,
                Err(e) => {
                    report.errors.push((item.path, e.to_string()));
                    failed.insert(item.asset.0);
                    continue;
                }
            };
            for s in suggest(&facts) {
                let label = catalog.label(s.kind, s.name)?;
                catalog.suggest(item.asset, label, Source::Rules, s.confidence)?;
                report.suggestions += 1;
            }
            catalog.mark_done(item.asset, STAGE, VERSION)?;
            report.analyzed += 1;
        }
        if !progressed {
            break;
        }
    }

    Ok(report)
}

struct Rule {
    kind: LabelKind,
    name: &'static str,
    confidence: f64,
    matches: fn(&Facts) -> bool,
}

const SCREENSHOT_NAME_PATTERNS: &[&str] = &["screenshot", "screen shot", "captura", "scr_"];

const PC_SCREEN_SIZES: &[(u32, u32)] = &[
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

const PHONE_PORTRAIT_WIDTHS: &[u32] = &[720, 828, 1080, 1125, 1170, 1179, 1242, 1284, 1290, 1440];

const WHATSAPP_SUBSTRINGS: &[&str] = &["whatsapp image"];

const RULES: &[Rule] = &[
    Rule {
        kind: LabelKind::Category,
        name: "Capturas de pantalla",
        confidence: 0.90,
        matches: has_screenshot_name,
    },
    Rule {
        kind: LabelKind::Category,
        name: "Capturas de pantalla",
        confidence: 0.60,
        matches: has_screen_dimensions_without_exif,
    },
    Rule {
        kind: LabelKind::Tag,
        name: "foto de cámara",
        confidence: 0.90,
        matches: |f| f.has_camera_exif,
    },
    Rule {
        kind: LabelKind::Tag,
        name: "WhatsApp",
        confidence: 0.95,
        matches: has_whatsapp_name,
    },
    Rule {
        kind: LabelKind::Tag,
        name: "GIF",
        confidence: 0.95,
        matches: |f| f.extension.eq_ignore_ascii_case("gif"),
    },
    Rule {
        kind: LabelKind::Tag,
        name: "formato vertical largo",
        confidence: 0.50,
        matches: is_very_tall,
    },
];

pub fn suggest(facts: &Facts) -> Vec<Suggestion> {
    let mut out: Vec<Suggestion> = Vec::new();
    for rule in RULES {
        if !(rule.matches)(facts) {
            continue;
        }
        if let Some(existing) = out
            .iter_mut()
            .find(|s| s.kind == rule.kind && s.name == rule.name)
        {
            if rule.confidence > existing.confidence {
                existing.confidence = rule.confidence;
            }
        } else {
            out.push(Suggestion {
                kind: rule.kind,
                name: rule.name,
                confidence: rule.confidence,
            });
        }
    }
    out
}

fn has_screenshot_name(facts: &Facts) -> bool {
    let lower = facts.file_name.to_lowercase();
    SCREENSHOT_NAME_PATTERNS.iter().any(|p| lower.contains(p))
}

fn has_screen_dimensions_without_exif(facts: &Facts) -> bool {
    if facts.has_camera_exif || facts.width == 0 || facts.height == 0 {
        return false;
    }
    let is_pc = PC_SCREEN_SIZES.iter().any(|&(w, h)| {
        (facts.width == w && facts.height == h) || (facts.width == h && facts.height == w)
    });
    if is_pc {
        return true;
    }
    // Integer cross-multiplication checks 1.9 <= height / width <= 2.3 without float rounding error.
    let w = u64::from(facts.width);
    let h = u64::from(facts.height);
    PHONE_PORTRAIT_WIDTHS.contains(&facts.width) && 10 * h >= 19 * w && 10 * h <= 23 * w
}

fn has_whatsapp_name(facts: &Facts) -> bool {
    let lower = facts.file_name.to_ascii_lowercase();
    if WHATSAPP_SUBSTRINGS.iter().any(|p| lower.contains(p)) {
        return true;
    }
    matches_wa_tagged(&lower, "img-", Some(8), 4) || matches_wa_tagged(&lower, "vid-", None, 1)
}

fn matches_wa_tagged(
    s: &str,
    prefix: &str,
    exact_date_digits: Option<usize>,
    min_seq: usize,
) -> bool {
    let bytes = s.as_bytes();
    for (idx, _) in s.match_indices(prefix) {
        if idx > 0 && bytes[idx - 1].is_ascii_alphanumeric() {
            continue;
        }
        let rest = &s[idx + prefix.len()..];
        let date_digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let date_ok = match exact_date_digits {
            Some(n) => date_digits == n,
            None => date_digits >= 1,
        };
        if !date_ok {
            continue;
        }
        let Some(after_wa) = rest[date_digits..].strip_prefix("-wa") else {
            continue;
        };
        let seq_digits = after_wa.bytes().take_while(u8::is_ascii_digit).count();
        if seq_digits >= min_seq {
            return true;
        }
    }
    false
}

fn is_very_tall(facts: &Facts) -> bool {
    facts.width > 0 && u64::from(facts.height) * 2 >= u64::from(facts.width) * 5
}
