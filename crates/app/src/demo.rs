use crate::settings::{self, Settings, Theme};
use catalog::{Catalog, LabelKind, OcrLine, OcrRecognizer, Source};
use fsguard::{DestPath, GuardError, LibraryRoot, ensure_export_dir, ensure_writable};
use image::codecs::gif::GifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::{ColorType, Frame, ImageEncoder, Rgb, RgbImage, Rgba, RgbaImage};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_DEMO_COUNT: usize = 160;
pub const MAX_DEMO_COUNT: usize = 2000;
pub const DEFAULT_SEED: u64 = 0x5048_4F54_4F53_4F52;
pub const DEMO_EXPORT_SUBDIR: &str = "export-out";

/// Returns a validated export destination inside `base_dir` and outside every registered library
/// root (used by `--demo --view export`).
pub fn demo_export_dir(base_dir: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    ensure_export_dir(&base_dir.join(DEMO_EXPORT_SUBDIR), roots)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoFormat {
    Png,
    Jpeg,
    Gif,
}

impl DemoFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Gradient,
    Stripes,
    Checker,
    ColorBlocks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSpec {
    pub index: usize,
    pub rel_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub format: DemoFormat,
    pub with_alpha: bool,
    pub pattern: PatternKind,
    pub duplicate_of: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedDecision {
    Suggested,
    Accepted,
    AcceptedUser,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SeedEntry {
    pub spec_index: usize,
    pub kind: LabelKind,
    pub name: &'static str,
    pub source: Source,
    pub confidence: f64,
    pub decision: SeedDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DemoConfig {
    pub count: usize,
    pub seed: u64,
}

impl Default for DemoConfig {
    fn default() -> Self {
        Self {
            count: DEFAULT_DEMO_COUNT,
            seed: DEFAULT_SEED,
        }
    }
}

#[derive(Debug)]
pub struct DemoSetup {
    pub base_dir: PathBuf,
    pub library_dir: PathBuf,
    pub data_dir: PathBuf,
    pub file_count: usize,
    pub duplicate_groups: usize,
    pub rules_report: rules::RunReport,
    pub seeded_assignments: usize,
    _temp_dir: Option<tempfile::TempDir>,
}

/// Validates `--demo-count` within `1..=MAX_DEMO_COUNT`.
pub fn validate_count(count: usize) -> Result<usize, String> {
    if count == 0 || count > MAX_DEMO_COUNT {
        return Err(format!(
            "--demo-count debe estar entre 1 y {MAX_DEMO_COUNT} (recibido: {count})"
        ));
    }
    Ok(count)
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next_u64() as usize) % upper
        }
    }
}

const FOLDERS: &[&str] = &["capturas", "documentos", "manga", "memes", "personas"];

const PATTERNS: &[PatternKind] = &[
    PatternKind::Gradient,
    PatternKind::Stripes,
    PatternKind::Checker,
    PatternKind::ColorBlocks,
];

/// Builds a deterministic plan of synthetic demo images covering varied aspect ratios, formats,
/// alpha channels, visual patterns, rule-triggering filenames, subfolders, and exact duplicates.
pub fn plan_demo_images(count: usize, seed: u64) -> Vec<ImageSpec> {
    let mut rng = Rng::new(seed);
    let mut specs: Vec<ImageSpec> = Vec::with_capacity(count);

    // Curated starter set so even small counts exercise every rule, aspect ratio, format,
    // alpha transparency, duplicate group, and give the first screen rich visual variety.
    let starters: &[(&str, &str, u32, u32, DemoFormat, bool, PatternKind)] = &[
        (
            "capturas",
            "Screenshot_2024-01-02.png",
            320,
            180,
            DemoFormat::Png,
            false,
            PatternKind::Gradient,
        ),
        (
            "personas",
            "IMG-20240101-WA0001.jpg",
            240,
            240,
            DemoFormat::Jpeg,
            false,
            PatternKind::ColorBlocks,
        ),
        (
            "memes",
            "reaccion_0002.gif",
            200,
            200,
            DemoFormat::Gif,
            false,
            PatternKind::Checker,
        ),
        (
            "manga",
            "tira_webtoon_0003.png",
            120,
            360,
            DemoFormat::Png,
            false,
            PatternKind::Stripes,
        ),
        (
            "manga",
            "Lluvia de Estrellas - Capítulo 4 - 01.png",
            220,
            300,
            DemoFormat::Png,
            false,
            PatternKind::Gradient,
        ),
        (
            "capturas",
            "alpha_marco_0004.png",
            220,
            220,
            DemoFormat::Png,
            true,
            PatternKind::ColorBlocks,
        ),
        (
            "capturas",
            "movil_0005.png",
            180,
            320,
            DemoFormat::Png,
            false,
            PatternKind::Stripes,
        ),
        (
            "capturas",
            "icono_mini_0006.png",
            48,
            48,
            DemoFormat::Png,
            false,
            PatternKind::Checker,
        ),
        (
            "capturas",
            "ventana_hd_0007.png",
            1280,
            720,
            DemoFormat::Png,
            false,
            PatternKind::Gradient,
        ),
        (
            "documentos",
            "apunte_0008.png",
            220,
            300,
            DemoFormat::Png,
            false,
            PatternKind::ColorBlocks,
        ),
        (
            "capturas",
            "bucle_demo_0009.gif",
            180,
            180,
            DemoFormat::Gif,
            false,
            PatternKind::Stripes,
        ),
    ];

    for (i, &(folder, name, width, height, format, with_alpha, pattern)) in
        starters.iter().enumerate().take(count)
    {
        specs.push(ImageSpec {
            index: i,
            rel_path: PathBuf::from(folder).join(name),
            width,
            height,
            format,
            with_alpha,
            pattern,
            duplicate_of: None,
        });
    }

    // Reserve two early exact duplicates right after the starter set when count allows.
    let dup_a = starters.len();
    let dup_b = starters.len() + 1;
    for i in specs.len()..count {
        if i == dup_a && specs.len() > 1 {
            let src = &specs[1];
            specs.push(ImageSpec {
                index: i,
                rel_path: PathBuf::from("memes").join(format!("copia_chat_{i:04}.jpg")),
                width: src.width,
                height: src.height,
                format: src.format,
                with_alpha: src.with_alpha,
                pattern: src.pattern,
                duplicate_of: Some(src.index),
            });
            continue;
        }
        if i == dup_b && !specs.is_empty() {
            let src = &specs[0];
            specs.push(ImageSpec {
                index: i,
                rel_path: PathBuf::from("documentos").join(format!("copia_captura_{i:04}.png")),
                width: src.width,
                height: src.height,
                format: src.format,
                with_alpha: src.with_alpha,
                pattern: src.pattern,
                duplicate_of: Some(src.index),
            });
            continue;
        }

        // Roughly 4% additional exact duplicates across subfolders.
        if i > 18 && i % 23 == 0 {
            let orig_idx = i % 10;
            let src = &specs[orig_idx];
            let folder = FOLDERS[(i / 3 + rng.range(FOLDERS.len())) % FOLDERS.len()];
            let ext = src.format.extension();
            specs.push(ImageSpec {
                index: i,
                rel_path: PathBuf::from(folder).join(format!("duplicado_{i:04}.{ext}")),
                width: src.width,
                height: src.height,
                format: src.format,
                with_alpha: src.with_alpha,
                pattern: src.pattern,
                duplicate_of: Some(src.index),
            });
            continue;
        }

        let folder = FOLDERS[i % FOLDERS.len()];
        let pattern = PATTERNS[(i + rng.range(PATTERNS.len())) % PATTERNS.len()];

        // Archetype rotation for varied aspect ratios, formats, and rules coverage.
        let (width, height, format, with_alpha, file_name) = match i % 12 {
            0 => (
                320,
                180,
                DemoFormat::Png,
                false,
                format!("Screenshot_2024-01-{:02}_{i:04}.png", (i % 28) + 1),
            ),
            1 => (
                240,
                240,
                DemoFormat::Jpeg,
                false,
                format!("IMG-202401{:02}-WA{i:04}.jpg", (i % 28) + 1),
            ),
            2 => (
                120,
                360,
                DemoFormat::Png,
                false,
                format!("tira_webtoon_{i:04}.png"),
            ),
            3 => (
                180,
                180,
                DemoFormat::Gif,
                false,
                format!("animacion_{i:04}.gif"),
            ),
            4 => (
                220,
                220,
                DemoFormat::Png,
                true,
                format!("sticker_alpha_{i:04}.png"),
            ),
            5 => (
                180,
                320,
                DemoFormat::Png,
                false,
                format!("vertical_movil_{i:04}.png"),
            ),
            6 => (
                56,
                44,
                DemoFormat::Png,
                false,
                format!("pequena_{i:04}.png"),
            ),
            7 => (
                240,
                240,
                DemoFormat::Jpeg,
                false,
                format!("cuadrada_{i:04}.jpg"),
            ),
            8 => (
                320,
                180,
                DemoFormat::Jpeg,
                false,
                format!("panoramica_{i:04}.jpg"),
            ),
            9 => {
                // Occasional exact phone portrait screen size to exercise rules' phone heuristic.
                if i < 36 {
                    (
                        720,
                        1440,
                        DemoFormat::Png,
                        false,
                        format!("pantalla_movil_{i:04}.png"),
                    )
                } else {
                    (
                        180,
                        320,
                        DemoFormat::Jpeg,
                        false,
                        format!("retrato_{i:04}.jpg"),
                    )
                }
            }
            10 => (
                260,
                200,
                DemoFormat::Png,
                false,
                format!("pagina_{i:04}.png"),
            ),
            _ => (
                280,
                210,
                DemoFormat::Jpeg,
                false,
                format!("escena_{i:04}.jpg"),
            ),
        };

        specs.push(ImageSpec {
            index: i,
            rel_path: PathBuf::from(folder).join(file_name),
            width,
            height,
            format,
            with_alpha,
            pattern,
            duplicate_of: None,
        });
    }

    specs
}

// Crisp 5x7 bitmap font for digits 0..=9 (each byte's lower 5 bits encode one row, MSB on left).
const DIGITS_5X7: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E], // 0
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E], // 1
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F], // 2
    [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E], // 3
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02], // 4
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E], // 5
    [0x0E, 0x10, 0x1E, 0x11, 0x11, 0x11, 0x0E], // 6
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08], // 7
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E], // 8
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C], // 9
];

const PALETTE: &[[u8; 3]] = &[
    [224, 92, 84],   // coral
    [66, 153, 225],  // sky blue
    [72, 187, 120],  // emerald
    [237, 173, 64],  // amber
    [159, 122, 234], // violet
    [56, 178, 172],  // teal
    [237, 100, 166], // rose
    [102, 126, 234], // indigo
    [245, 130, 48],  // tangerine
    [99, 179, 237],  // cyan
    [129, 199, 132], // sage
    [214, 158, 46],  // ochre
];

fn lerp_u8(a: u8, b: u8, num: u32, den: u32) -> u8 {
    let d = den.max(1);
    ((u32::from(a) * (d - num) + u32::from(b) * num) / d) as u8
}

fn draw_digit_badge(img: &mut RgbaImage, number: usize) {
    let (w, h) = img.dimensions();
    let label = format!("{number:03}");
    let digits: Vec<usize> = label
        .bytes()
        .filter_map(|b| (b.is_ascii_digit()).then_some((b - b'0') as usize))
        .collect();
    if digits.is_empty() {
        return;
    }

    let min_dim = w.min(h);
    let scale = (min_dim / 42).clamp(1, 4);
    let glyph_w = 5 * scale;
    let glyph_h = 7 * scale;
    let gap = scale;
    let text_w = digits.len() as u32 * glyph_w + (digits.len() as u32 - 1) * gap;
    let pad_x = (3 * scale).min(w / 6);
    let pad_y = (3 * scale).min(h / 6);
    let box_w = text_w + pad_x * 2;
    let box_h = glyph_h + pad_y * 2;
    if box_w >= w || box_h >= h {
        return;
    }

    let x0 = (w - box_w) / 2;
    let y0 = (h - box_h) / 2;

    for y in y0..(y0 + box_h) {
        for x in x0..(x0 + box_w) {
            let on_border = x == x0 || x + 1 == x0 + box_w || y == y0 || y + 1 == y0 + box_h;
            let color = if on_border {
                Rgba([245, 247, 250, 235])
            } else {
                Rgba([18, 22, 30, 215])
            };
            img.put_pixel(x, y, color);
        }
    }

    let text_x = x0 + pad_x;
    let text_y = y0 + pad_y;
    for (idx, &d) in digits.iter().enumerate() {
        let gx = text_x + idx as u32 * (glyph_w + gap);
        let bitmap = DIGITS_5X7[d];
        for (row, &bits) in bitmap.iter().enumerate() {
            for col in 0..5u32 {
                if (bits >> (4 - col)) & 1 == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = gx + col * scale + sx;
                            let py = text_y + row as u32 * scale + sy;
                            if px < w && py < h {
                                img.put_pixel(px, py, Rgba([250, 252, 255, 255]));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Renders a deterministic, visually distinct synthetic image into encoded PNG, JPEG, or GIF bytes.
/// Duplicates use their original index so their encoded bytes match bit-for-bit.
pub fn render_demo_image(spec: &ImageSpec, seed: u64) -> Result<Vec<u8>, String> {
    let canonical_idx = spec.duplicate_of.unwrap_or(spec.index);
    let mut rng = Rng::new(seed ^ ((canonical_idx as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)));
    let c1 = PALETTE[rng.range(PALETTE.len())];
    let c2 = PALETTE[(rng.range(PALETTE.len() - 1) + 1 + canonical_idx) % PALETTE.len()];
    let c3 = PALETTE[(canonical_idx * 5 + 3) % PALETTE.len()];
    let orient = rng.range(3);

    let (w, h) = (spec.width, spec.height);
    let mut rgba = RgbaImage::new(w, h);
    let stripe_step = (w.min(h) / 10).clamp(6, 28);
    let tile_step = (w.min(h) / 8).clamp(8, 32);

    for y in 0..h {
        for x in 0..w {
            let rgb = match spec.pattern {
                PatternKind::Gradient => {
                    let num = match orient {
                        0 => x,
                        1 => y,
                        _ => (x + y) / 2,
                    };
                    let den = match orient {
                        0 => w.saturating_sub(1),
                        1 => h.saturating_sub(1),
                        _ => (w + h).saturating_sub(2) / 2,
                    };
                    [
                        lerp_u8(c1[0], c2[0], num, den),
                        lerp_u8(c1[1], c2[1], num, den),
                        lerp_u8(c1[2], c2[2], num, den),
                    ]
                }
                PatternKind::Stripes => {
                    let coord = match orient {
                        0 => x,
                        1 => y,
                        _ => x + y,
                    };
                    match (coord / stripe_step) % 3 {
                        0 => c1,
                        1 => c2,
                        _ => c3,
                    }
                }
                PatternKind::Checker => {
                    let odd = ((x / tile_step) + (y / tile_step)) % 2 == 0;
                    if odd { c1 } else { c2 }
                }
                PatternKind::ColorBlocks => {
                    let split_x = (w * 3) / 5;
                    let split_y = (h * 2) / 5;
                    let border = 2.min(w / 12).max(1);
                    if x.abs_diff(split_x) < border || y.abs_diff(split_y) < border {
                        [24, 28, 36]
                    } else if x < split_x && y < split_y {
                        c1
                    } else if x >= split_x && y < split_y {
                        c2
                    } else if x < split_x {
                        c3
                    } else {
                        [
                            lerp_u8(c1[0], 245, 1, 3),
                            lerp_u8(c2[1], 245, 1, 3),
                            lerp_u8(c3[2], 245, 1, 3),
                        ]
                    }
                }
            };

            let alpha = if spec.with_alpha && spec.format == DemoFormat::Png {
                let corner = (w.min(h) / 5).max(8);
                let in_corner = (x < corner || x + corner >= w) && (y < corner || y + corner >= h);
                if in_corner {
                    0
                } else if x < corner / 2 || x + corner / 2 >= w {
                    155
                } else {
                    255
                }
            } else {
                255
            };

            rgba.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], alpha]));
        }
    }

    draw_digit_badge(&mut rgba, canonical_idx + 1);

    let mut out = Vec::new();
    match spec.format {
        DemoFormat::Png => {
            let encoder = PngEncoder::new_with_quality(
                &mut out,
                CompressionType::Fast,
                PngFilterType::NoFilter,
            );
            encoder
                .write_image(rgba.as_raw(), w, h, ColorType::Rgba8.into())
                .map_err(|e| e.to_string())?;
        }
        DemoFormat::Jpeg => {
            let rgb = RgbImage::from_fn(w, h, |x, y| {
                let p = rgba.get_pixel(x, y);
                Rgb([p[0], p[1], p[2]])
            });
            JpegEncoder::new_with_quality(&mut out, 80)
                .encode(rgb.as_raw(), w, h, ColorType::Rgb8.into())
                .map_err(|e| e.to_string())?;
        }
        DemoFormat::Gif => {
            let mut encoder = GifEncoder::new_with_speed(&mut out, 30);
            encoder
                .encode_frame(Frame::new(rgba))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(out)
}

const DEMO_TAGS: &[&str] = &[
    "favorito",
    "portada",
    "recorte",
    "referencia",
    "reacción",
    "promocional",
    "boceto",
    "texto",
];

const DEMO_ENTITIES: &[&str] = &[
    "Crónicas del Relojero",
    "Academia Nebulosa",
    "El Jardín de Papel",
    "Estudio Cometa",
    "Isla de Cristal",
];

const DEMO_CONFIDENCES: &[f64] = &[0.22, 0.29, 0.38, 0.49, 0.58, 0.67, 0.76, 0.84, 0.91, 0.95];

fn folder_category(rel_path: &Path) -> &'static str {
    let first = rel_path
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match first.as_str() {
        "capturas" => "Capturas de pantalla",
        "documentos" => "Documentos",
        "manga" => "Manga",
        "memes" => "Memes",
        "personas" => "Personas",
        _ => "Anime",
    }
}

/// Builds a deterministic label seeding plan covering categories, tags, fictional entities,
/// AI/OCR/Rules sources, confidences in `[0.2, 0.95]`, and accepted/suggested/rejected states.
pub fn plan_label_seeds(specs: &[ImageSpec], seed: u64) -> Vec<SeedEntry> {
    if specs.is_empty() {
        return Vec::new();
    }

    let mut rng = Rng::new(seed ^ 0xA5A5_5A5A_1234_5678);
    let mut plan: Vec<SeedEntry> = Vec::new();

    // Identify the lexicographically first path so the initial inspector selection showcases
    // every label kind, source, and user decision state.
    let first_sorted_idx = specs
        .iter()
        .min_by(|a, b| {
            a.rel_path
                .to_string_lossy()
                .cmp(&b.rel_path.to_string_lossy())
        })
        .map(|s| s.index)
        .unwrap_or(0);

    plan.push(SeedEntry {
        spec_index: first_sorted_idx,
        kind: LabelKind::Category,
        name: "Capturas de pantalla",
        source: Source::Rules,
        confidence: 0.90,
        decision: SeedDecision::Accepted,
    });
    plan.push(SeedEntry {
        spec_index: first_sorted_idx,
        kind: LabelKind::Tag,
        name: "referencia",
        source: Source::User,
        confidence: 1.0,
        decision: SeedDecision::AcceptedUser,
    });
    plan.push(SeedEntry {
        spec_index: first_sorted_idx,
        kind: LabelKind::Tag,
        name: "recorte",
        source: Source::Ai,
        confidence: 0.74,
        decision: SeedDecision::Suggested,
    });
    plan.push(SeedEntry {
        spec_index: first_sorted_idx,
        kind: LabelKind::Entity,
        name: DEMO_ENTITIES[0],
        source: Source::Ocr,
        confidence: 0.38,
        decision: SeedDecision::Suggested,
    });
    plan.push(SeedEntry {
        spec_index: first_sorted_idx,
        kind: LabelKind::Tag,
        name: "borroso",
        source: Source::Ai,
        confidence: 0.22,
        decision: SeedDecision::Rejected,
    });

    for spec in specs {
        if spec.duplicate_of.is_some() || spec.index == first_sorted_idx {
            continue;
        }
        let idx = spec.index;
        let cat = if idx % 7 == 0 {
            "Anime"
        } else {
            folder_category(&spec.rel_path)
        };
        let cat_conf = DEMO_CONFIDENCES[(idx + 5) % DEMO_CONFIDENCES.len()];
        let cat_decision = match idx % 5 {
            0..=2 => SeedDecision::Accepted,
            3 => SeedDecision::Suggested,
            _ => SeedDecision::AcceptedUser,
        };
        let cat_source = match idx % 3 {
            0 => Source::Ai,
            1 => Source::Rules,
            _ => Source::Ocr,
        };
        plan.push(SeedEntry {
            spec_index: idx,
            kind: LabelKind::Category,
            name: cat,
            source: cat_source,
            confidence: cat_conf,
            decision: cat_decision,
        });

        let tag_name = DEMO_TAGS[(idx + rng.range(DEMO_TAGS.len())) % DEMO_TAGS.len()];
        let tag_conf = DEMO_CONFIDENCES[idx % DEMO_CONFIDENCES.len()];
        let tag_source = if idx % 2 == 0 {
            Source::Ai
        } else {
            Source::Ocr
        };
        let tag_decision = match idx % 4 {
            0 => SeedDecision::Suggested,
            1 => SeedDecision::Accepted,
            2 => SeedDecision::Suggested,
            _ => SeedDecision::Rejected,
        };
        plan.push(SeedEntry {
            spec_index: idx,
            kind: LabelKind::Tag,
            name: tag_name,
            source: tag_source,
            confidence: tag_conf,
            decision: tag_decision,
        });

        if idx % 3 == 0 {
            let entity = DEMO_ENTITIES[(idx / 3) % DEMO_ENTITIES.len()];
            let ent_conf = DEMO_CONFIDENCES[(idx * 3) % DEMO_CONFIDENCES.len()];
            let ent_decision = match idx % 9 {
                0 => SeedDecision::Accepted,
                3 => SeedDecision::Suggested,
                _ => SeedDecision::Rejected,
            };
            plan.push(SeedEntry {
                spec_index: idx,
                kind: LabelKind::Entity,
                name: entity,
                source: if idx % 6 == 0 {
                    Source::Ocr
                } else {
                    Source::Ai
                },
                confidence: ent_conf,
                decision: ent_decision,
            });
        }
    }

    plan
}

#[allow(clippy::disallowed_methods)] // `dir` is a fsguard DestPath inside the demo workspace
fn create_dir_dest(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

#[allow(clippy::disallowed_methods)] // `dest` is a fsguard DestPath inside the demo workspace
fn write_bytes_dest(dest: &DestPath, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(dest.as_path())?;
    file.write_all(bytes)
}

fn write_demo_file(library_dest: &DestPath, rel_path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut current = library_dest.clone();
    if let Some(parent) = rel_path.parent() {
        for comp in parent.components() {
            current = current
                .join_name(comp.as_os_str())
                .ok_or_else(|| format!("nombre de subcarpeta inválido: {}", rel_path.display()))?;
        }
        create_dir_dest(&current).map_err(|e| e.to_string())?;
    }
    let name = rel_path
        .file_name()
        .ok_or_else(|| format!("falta el nombre de archivo: {}", rel_path.display()))?;
    let file_dest = current
        .join_name(name)
        .ok_or_else(|| format!("nombre de archivo inválido: {}", rel_path.display()))?;
    write_bytes_dest(&file_dest, bytes).map_err(|e| e.to_string())
}

/// Generates a complete synthetic demo workspace (`library/` + `data/`), registers the library,
/// indexes all images, runs `rules::run`, and seeds catalog labels for every UI state.
pub fn prepare_demo(
    base_dir: Option<&Path>,
    config: DemoConfig,
    theme: Option<Theme>,
) -> Result<DemoSetup, String> {
    let count = validate_count(config.count)?;

    let (temp_dir, resolved_base) = match base_dir {
        Some(dir) => (None, dir.to_path_buf()),
        None => {
            let td = tempfile::tempdir().map_err(|e| e.to_string())?;
            let path = td.path().to_path_buf();
            (Some(td), path)
        }
    };

    let base_dest = ensure_writable(&resolved_base, &[]).map_err(|e| e.to_string())?;
    create_dir_dest(&base_dest).map_err(|e| e.to_string())?;

    let library_dest = base_dest
        .join_name(OsStr::new("library"))
        .ok_or_else(|| "nombre inválido para library".to_string())?;
    let data_dest = base_dest
        .join_name(OsStr::new("data"))
        .ok_or_else(|| "nombre inválido para data".to_string())?;
    create_dir_dest(&library_dest).map_err(|e| e.to_string())?;
    create_dir_dest(&data_dest).map_err(|e| e.to_string())?;

    let library_dir = library_dest.as_path().to_path_buf();
    let data_dir = data_dest.as_path().to_path_buf();

    let specs = plan_demo_images(count, config.seed);
    let mut rendered_cache: HashMap<usize, Vec<u8>> = HashMap::new();
    for spec in &specs {
        let bytes = if let Some(orig) = spec.duplicate_of
            && let Some(cached) = rendered_cache.get(&orig)
        {
            cached.clone()
        } else {
            let data = render_demo_image(spec, config.seed)?;
            if spec.duplicate_of.is_none() {
                rendered_cache.insert(spec.index, data.clone());
            }
            data
        };
        write_demo_file(&library_dest, &spec.rel_path, &bytes)?;
    }

    let mut cfg = Settings::default();
    if let Some(t) = theme {
        cfg.theme = t;
    }
    cfg.add_library_root(&library_dir, &data_dir)
        .map_err(|e| e.to_string())?;
    settings::save(&data_dir, &cfg).map_err(|e| e.to_string())?;

    let roots = cfg.valid_roots();
    let root = roots
        .first()
        .ok_or_else(|| "no se pudo resolver la raíz de biblioteca de demo".to_string())?;
    let db = settings::db_dest_path(&data_dir, &roots).map_err(|e| e.to_string())?;
    let mut cat = Catalog::open(&db).map_err(|e| e.to_string())?;
    cat.index(root).map_err(|e| e.to_string())?;

    let rules_report = rules::run(&cat).map_err(|e| e.to_string())?;

    let canonical_root = root.as_path();
    let seed_plan = plan_label_seeds(&specs, config.seed);
    let mut seeded_assignments = 0usize;
    for item in &seed_plan {
        let Some(spec) = specs.get(item.spec_index) else {
            continue;
        };
        let full_path = canonical_root.join(&spec.rel_path);
        let Some(asset) = cat.asset_of(&full_path).map_err(|e| e.to_string())? else {
            continue;
        };
        let label = cat.label(item.kind, item.name).map_err(|e| e.to_string())?;
        match item.decision {
            SeedDecision::Suggested => {
                cat.suggest(asset, label, item.source, item.confidence)
                    .map_err(|e| e.to_string())?;
            }
            SeedDecision::Accepted => {
                cat.suggest(asset, label, item.source, item.confidence)
                    .map_err(|e| e.to_string())?;
                cat.accept(asset, label).map_err(|e| e.to_string())?;
            }
            SeedDecision::AcceptedUser => {
                cat.accept(asset, label).map_err(|e| e.to_string())?;
            }
            SeedDecision::Rejected => {
                cat.suggest(asset, label, item.source, item.confidence)
                    .map_err(|e| e.to_string())?;
                cat.reject(asset, label).map_err(|e| e.to_string())?;
            }
        }
        seeded_assignments += 1;
    }

    let duplicate_groups = cat.duplicates().map_err(|e| e.to_string())?.len();

    // Seed synthetic OCR lines on the first sorted image (shown in the gallery inspector)
    // and a small subset of other demo images, leaving most images pending for the OCR window.
    // Demo mode never loads ONNX models.
    seed_demo_ocr(&mut cat, canonical_root, &specs)?;
    seed_demo_works(&mut cat)?;

    Ok(DemoSetup {
        base_dir: resolved_base,
        library_dir,
        data_dir,
        file_count: specs.len(),
        duplicate_groups,
        rules_report,
        seeded_assignments,
        _temp_dir: temp_dir,
    })
}

fn seed_demo_ocr(
    cat: &mut Catalog,
    canonical_root: &Path,
    specs: &[ImageSpec],
) -> Result<(), String> {
    if specs.is_empty() {
        return Ok(());
    }
    let first_sorted_idx = specs
        .iter()
        .min_by(|a, b| {
            a.rel_path
                .to_string_lossy()
                .cmp(&b.rel_path.to_string_lossy())
        })
        .map(|s| s.index)
        .unwrap_or(0);

    if let Some(first) = specs.get(first_sorted_idx)
        && let Some(asset) = cat
            .asset_of(&canonical_root.join(&first.rel_path))
            .map_err(|e| e.to_string())?
    {
        cat.save_ocr_lines(
            asset,
            &[
                OcrLine {
                    text: "14:32  5G  84%".to_string(),
                    score: 0.78,
                    recognizer: OcrRecognizer::V6,
                    rect: [12.0, 4.0, 160.0, 18.0],
                    top_strip: true,
                },
                OcrLine {
                    text: "Crónicas del Relojero — Capítulo 12".to_string(),
                    score: 0.96,
                    recognizer: OcrRecognizer::V6,
                    rect: [20.0, 60.0, 290.0, 88.0],
                    top_strip: false,
                },
                OcrLine {
                    text: "Estudio Cometa · Edición especial".to_string(),
                    score: 0.52,
                    recognizer: OcrRecognizer::V6,
                    rect: [20.0, 100.0, 260.0, 124.0],
                    top_strip: false,
                },
            ],
        )
        .map_err(|e| e.to_string())?;
    }

    let sample_texts = [
        ("Academia Nebulosa — Portada", 0.94, OcrRecognizer::V6),
        ("El Jardín de Papel · Notas", 0.89, OcrRecognizer::V6),
        ("바람 공주 · 3화", 0.91, OcrRecognizer::Korean),
        ("Isla de Cristal (boceto)", 0.58, OcrRecognizer::V6),
    ];

    for (i, spec) in specs.iter().enumerate().take(18) {
        if spec.index == first_sorted_idx || spec.duplicate_of.is_some() {
            continue;
        }
        if let Some(asset) = cat
            .asset_of(&canonical_root.join(&spec.rel_path))
            .map_err(|e| e.to_string())?
        {
            if i == 3 {
                // Synthetic promo page mentioning 3 works + header phrase.
                cat.save_ocr_lines(
                    asset,
                    &[
                        OcrLine {
                            text: "PROYECTOS ACTIVOS DE:".to_string(),
                            score: 0.95,
                            recognizer: OcrRecognizer::V6,
                            rect: [20.0, 20.0, 280.0, 46.0],
                            top_strip: false,
                        },
                        OcrLine {
                            text: "Academia Nebulosa".to_string(),
                            score: 0.92,
                            recognizer: OcrRecognizer::V6,
                            rect: [20.0, 120.0, 180.0, 142.0],
                            top_strip: false,
                        },
                        OcrLine {
                            text: "El Jardín".to_string(),
                            score: 0.90,
                            recognizer: OcrRecognizer::V6,
                            rect: [210.0, 120.0, 310.0, 142.0],
                            top_strip: false,
                        },
                        OcrLine {
                            text: "de Papel".to_string(),
                            score: 0.88,
                            recognizer: OcrRecognizer::V6,
                            rect: [215.0, 150.0, 305.0, 172.0],
                            top_strip: false,
                        },
                        OcrLine {
                            text: "Isla de Cristal".to_string(),
                            score: 0.91,
                            recognizer: OcrRecognizer::V6,
                            rect: [20.0, 220.0, 160.0, 242.0],
                            top_strip: false,
                        },
                    ],
                )
                .map_err(|e| e.to_string())?;
                continue;
            }
            let (text, score, rec) = sample_texts[i % sample_texts.len()];
            cat.save_ocr_lines(
                asset,
                &[OcrLine {
                    text: text.to_string(),
                    score,
                    recognizer: rec,
                    rect: [16.0, 40.0, 200.0, 64.0],
                    top_strip: false,
                }],
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn seed_demo_works(cat: &mut Catalog) -> Result<(), String> {
    let demo_aliases: &[(&str, &[&str])] = &[
        (
            "Crónicas del Relojero",
            &["Clockmaker Chronicles", "Relojero"],
        ),
        ("Academia Nebulosa", &["Nebula Academy"]),
        ("El Jardín de Papel", &["Paper Garden"]),
        ("Estudio Cometa", &["Cometa Scans"]),
        ("Isla de Cristal", &["Crystal Island"]),
    ];

    for &(work_name, aliases) in demo_aliases {
        let id = cat.create_work(work_name).map_err(|e| e.to_string())?;
        for &alias in aliases {
            cat.add_work_alias(id, alias).map_err(|e| e.to_string())?;
        }
    }

    let matcher = cat.load_work_matcher().map_err(|e| e.to_string())?;
    let assets = cat.assets_with_ocr().map_err(|e| e.to_string())?;
    for asset in assets {
        cat.apply_work_matches(asset, &matcher)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
