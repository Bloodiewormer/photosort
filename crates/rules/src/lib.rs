pub use catalog::Control;
use catalog::{AssetId, Catalog, CatalogError, LabelKind, Pending, Source};
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

pub const STAGE: &str = "rules";
pub const VERSION: u32 = 2;

const BATCH_SIZE: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub file_name: String,
    pub extension: String,
    pub format: Option<String>,
    pub width: u32,
    pub height: u32,
    pub has_camera_exif: bool,
    pub captured_at_ns: Option<i64>,
    pub captured_year: Option<i32>,
    pub media_type: catalog::MediaType,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    pub kind: LabelKind,
    pub name: &'static str,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunProgress {
    pub analyzed: usize,
    pub pending: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RunReport {
    pub analyzed: usize,
    pub suggestions: usize,
    pub errors: Vec<(PathBuf, String)>,
    pub stopped: bool,
}

/// Reads ONLY header/metadata: image format and dimensions via `image::ImageReader::into_dimensions`
/// (no full decode), plus camera Make/Model presence and EXIF capture date (`DateTimeOriginal` with
/// fallback to `DateTime`) via `exif`. Read-only.
pub fn read_facts(path: &Path) -> io::Result<Facts> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    if thumbs::is_video_path(path) {
        let meta = thumbs::read_metadata(path)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        return Ok(Facts {
            file_name,
            extension,
            format: Some(meta.format),
            width: meta.width,
            height: meta.height,
            has_camera_exif: false,
            captured_at_ns: None,
            captured_year: None,
            media_type: catalog::MediaType::Video,
            duration_ms: Some(meta.duration_ms),
        });
    }

    let reader = image::ImageReader::open(path)?.with_guessed_format()?;
    let detected_format = reader.format().map(image_format_name);
    let (width, height) = reader.into_dimensions().map_err(|e| match e {
        image::ImageError::IoError(io_err) => io_err,
        other => io::Error::new(io::ErrorKind::InvalidData, other),
    })?;
    let file = File::open(path)?;
    let mut buf = BufReader::new(file);
    let exif_data = exif::Reader::new().read_from_container(&mut buf).ok();
    let has_camera_exif = exif_data.as_ref().is_some_and(|data| {
        has_non_empty_tag(data, exif::Tag::Make) || has_non_empty_tag(data, exif::Tag::Model)
    });
    let (captured_at_ns, captured_year) = exif_data
        .as_ref()
        .and_then(extract_capture_datetime)
        .map(|(ns, y)| (Some(ns), Some(y)))
        .unwrap_or((None, None));
    let format = detected_format
        .map(ToOwned::to_owned)
        .or_else(|| format_from_extension(&extension).map(ToOwned::to_owned));
    Ok(Facts {
        file_name,
        extension,
        format,
        width,
        height,
        has_camera_exif,
        captured_at_ns,
        captured_year,
        media_type: catalog::MediaType::Image,
        duration_ms: None,
    })
}

fn image_format_name(fmt: image::ImageFormat) -> &'static str {
    match fmt {
        image::ImageFormat::Jpeg => "JPEG",
        image::ImageFormat::Png => "PNG",
        image::ImageFormat::Gif => "GIF",
        image::ImageFormat::WebP => "WEBP",
        image::ImageFormat::Bmp => "BMP",
        image::ImageFormat::Tiff => "TIFF",
        image::ImageFormat::Avif => "AVIF",
        image::ImageFormat::Ico => "ICO",
        _ => "OTRO",
    }
}

fn format_from_extension(ext: &str) -> Option<&'static str> {
    match ext {
        "jpg" | "jpeg" => Some("JPEG"),
        "png" => Some("PNG"),
        "gif" => Some("GIF"),
        "webp" => Some("WEBP"),
        "bmp" => Some("BMP"),
        "tif" | "tiff" => Some("TIFF"),
        "avif" => Some("AVIF"),
        "ico" => Some("ICO"),
        _ => None,
    }
}

fn extract_capture_datetime(data: &exif::Exif) -> Option<(i64, i32)> {
    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTime] {
        let Some(field) = data.get_field(tag, exif::In::PRIMARY) else {
            continue;
        };
        let raw = match &field.value {
            exif::Value::Ascii(values) => values
                .iter()
                .find_map(|bytes| {
                    let s = std::str::from_utf8(bytes).ok()?.trim_matches('\0').trim();
                    (!s.is_empty()).then(|| s.to_string())
                })
                .unwrap_or_default(),
            _ => field.display_value().to_string(),
        };
        if let Some(parsed) = parse_exif_datetime_ns(&raw) {
            return Some(parsed);
        }
    }
    None
}

/// Parses an EXIF datetime (`YYYY:MM:DD HH:MM:SS` or `YYYY-MM-DD HH:MM:SS`) into
/// `(epoch_nanos, year)` using proleptic Gregorian wall-clock time.
pub fn parse_exif_datetime_ns(raw: &str) -> Option<(i64, i32)> {
    let trimmed = raw.trim().trim_matches('\0').trim();
    if trimmed.len() < 19 {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let sep1 = bytes[4];
    let sep2 = bytes[7];
    let sep_mid = bytes[10];
    let sep3 = bytes[13];
    let sep4 = bytes[16];
    if !matches!(sep1, b':' | b'-')
        || sep2 != sep1
        || !matches!(sep_mid, b' ' | b'T')
        || sep3 != b':'
        || sep4 != b':'
    {
        return None;
    }
    let year: i32 = trimmed[0..4].parse().ok()?;
    let month: u32 = trimmed[5..7].parse().ok()?;
    let day: u32 = trimmed[8..10].parse().ok()?;
    let hour: u32 = trimmed[11..13].parse().ok()?;
    let minute: u32 = trimmed[14..16].parse().ok()?;
    let second: u32 = trimmed[17..19].parse().ok()?;
    if !(1900..=2200).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // Howard Hinnant's days-from-civil algorithm (inverse of civil-from-days).
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let d = i64::from(day);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second);
    let ns = secs.checked_mul(1_000_000_000)?;
    Some((ns, year))
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

/// Runs rules on a single pending asset: reads header facts, stores image metadata, records rule
/// suggestions, and marks `STAGE` at `VERSION` done. Returns `Ok(Ok(suggestion_count))` on success,
/// `Ok(Err(reason))` if the image could not be read (leaving it pending), or `Err(db_err)` on
/// catalog failure.
pub fn analyze_pending_item(
    catalog: &Catalog,
    item: &Pending,
) -> Result<Result<usize, String>, CatalogError> {
    match read_facts(&item.path) {
        Ok(facts) => {
            catalog.set_asset_metadata(
                item.asset,
                Some(facts.width),
                Some(facts.height),
                facts.format.as_deref(),
                facts.captured_at_ns,
                facts.captured_year,
                facts.duration_ms,
            )?;
            let mut suggestions = 0usize;
            for s in suggest(&facts) {
                let label = catalog.label(s.kind, s.name)?;
                catalog.suggest(item.asset, label, Source::Rules, s.confidence)?;
                suggestions += 1;
            }
            if catalog.suggest_work_from_path(item.asset, &item.path)? {
                suggestions += 1;
            }
            if catalog.suggest_chapter_page_from_path(item.asset, &item.path)? {
                suggestions += 1;
            }
            catalog.mark_done(item.asset, STAGE, VERSION)?;
            Ok(Ok(suggestions))
        }
        Err(e) => Ok(Err(e.to_string())),
    }
}

/// Runs rule-based analysis scoped to the given `assets` (only those still pending for `STAGE`),
/// reporting progress and stopping early on [`Control::Stop`].
pub fn run_for_assets_with(
    catalog: &Catalog,
    assets: &[AssetId],
    progress: &mut dyn FnMut(&RunProgress) -> Control,
) -> Result<RunReport, CatalogError> {
    let mut report = RunReport::default();
    let batch = catalog.pending_in_assets(STAGE, VERSION, assets, usize::MAX)?;
    let total = batch.len();
    if progress(&RunProgress {
        analyzed: 0,
        pending: total,
    }) == Control::Stop
    {
        report.stopped = true;
        return Ok(report);
    }
    let mut failed = 0usize;
    for item in batch {
        match analyze_pending_item(catalog, &item)? {
            Ok(suggestions) => {
                report.suggestions += suggestions;
                report.analyzed += 1;
            }
            Err(reason) => {
                report.errors.push((item.path, reason));
                failed += 1;
            }
        }
        let pending = total.saturating_sub(report.analyzed + failed);
        if progress(&RunProgress {
            analyzed: report.analyzed,
            pending,
        }) == Control::Stop
        {
            report.stopped = true;
            return Ok(report);
        }
    }
    Ok(report)
}

/// Processes `catalog.pending(STAGE, VERSION, ..)` in batches until none is left: `read_facts`,
/// `suggest`, then `catalog.label(kind, name)` + `catalog.suggest(asset, label, Source::Rules, confidence)`
/// for each, then `mark_done`. An unreadable file is reported in `errors` and NOT marked done
/// (it is retried next run). Never touches user decisions. Safe to run repeatedly.
pub fn run(catalog: &Catalog) -> Result<RunReport, CatalogError> {
    run_with(catalog, &mut |_| Control::Continue)
}

/// Like [`run`], reporting monotonic progress after each pending item and stopping early when
/// `progress` returns [`Control::Stop`]. Everything already analyzed stays committed and marked done.
pub fn run_with(
    catalog: &Catalog,
    progress: &mut dyn FnMut(&RunProgress) -> Control,
) -> Result<RunReport, CatalogError> {
    let mut report = RunReport::default();
    let mut failed: HashSet<i64> = HashSet::new();
    // Snapshot initial pending count once so `pending` decreases monotonically without extra SQL per item.
    let initial_pending = catalog.pending(STAGE, VERSION, usize::MAX)?.len();

    if progress(&RunProgress {
        analyzed: 0,
        pending: initial_pending,
    }) == Control::Stop
    {
        report.stopped = true;
        return Ok(report);
    }

    loop {
        let batch = catalog.pending(STAGE, VERSION, BATCH_SIZE.saturating_add(failed.len()))?;
        let mut progressed = false;
        for item in batch {
            if failed.contains(&item.asset.0) {
                continue;
            }
            progressed = true;
            match analyze_pending_item(catalog, &item)? {
                Ok(suggestions) => {
                    report.suggestions += suggestions;
                    report.analyzed += 1;
                }
                Err(reason) => {
                    report.errors.push((item.path, reason));
                    failed.insert(item.asset.0);
                }
            }

            let pending = initial_pending.saturating_sub(report.analyzed + failed.len());
            if progress(&RunProgress {
                analyzed: report.analyzed,
                pending,
            }) == Control::Stop
            {
                report.stopped = true;
                return Ok(report);
            }
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
    if facts.media_type == catalog::MediaType::Video
        || facts.has_camera_exif
        || facts.width == 0
        || facts.height == 0
    {
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
