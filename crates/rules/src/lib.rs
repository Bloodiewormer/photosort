use catalog::LabelKind;

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
