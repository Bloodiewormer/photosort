use crate::labels::unexpected;
use crate::ocr::SCREENSHOT_CATEGORY;
use crate::{AssetId, Catalog, CatalogError, Entry, LabelId, LabelKind};
use rusqlite::{OptionalExtension, params};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

pub const VISION_STAGE: &str = "vision";
pub const VISION_VERSION: u32 = 1;
pub const SIGLIP2_MODEL_ID: &str = "siglip2-base-patch16-256";
pub const EMBEDDING_DIM: usize = 768;
pub const EMBEDDING_BYTES: usize = EMBEDDING_DIM * 4;

/// Minimum YuNet face score that can trigger the `"persona"` tag when the winning type is
/// Foto, Meme or Capturas de pantalla.
pub const YUNET_FACE_SCORE_THRESHOLD: f32 = 0.8;

/// Threshold for built-in exclusive TYPE rules, calibrated on 335 real images (see
/// docs/vision-evaluacion.md). Users can tune each rule's threshold from the UI.
pub const DEFAULT_TYPE_THRESHOLD: f32 = 0.06;

/// UNCALIBRATED placeholder threshold for the built-in `"persona"` independent tag rule.
pub const DEFAULT_PERSON_THRESHOLD: f32 = 0.10;

/// UNCALIBRATED placeholder threshold for built-in independent content category and page-type
/// vision rules. Users should calibrate each rule using «Probar» on their own images.
pub const DEFAULT_CATEGORY_THRESHOLD: f32 = 0.10;

/// Calibrated on 335 real images: anime art scores lower than the generic category default.
const ANIME_THRESHOLD: f32 = 0.085;

/// Calibrated on 335 real images: below this, lists of covers and promo pages match too.
const MANGA_COVER_THRESHOLD: f32 = 0.16;

/// No threshold separated these from the rest on 335 real images; the user can turn them on.
const DISABLED_BY_DEFAULT: &[&str] = &[
    BUILTIN_KEY_MANHWA,
    BUILTIN_KEY_WEBTOON,
    BUILTIN_KEY_VIDEOGAMES,
];

/// Marks catalogs whose untouched built-in rules already got the calibrated values.
const META_BUILTINS_CALIBRATED: &str = "vision_builtins_calibrated:2026-11-27";

/// UNCALIBRATED placeholder threshold for newly created custom independent tag rules.
pub const DEFAULT_CUSTOM_THRESHOLD: f32 = 0.10;

pub const TYPE_MANGA: &str = "Manga";
pub const TYPE_ILLUSTRATION: &str = "Ilustración";
pub const TYPE_PHOTO: &str = "Foto";
pub const TYPE_MEME: &str = "Meme";
pub const TYPE_DOCUMENT: &str = "Documento";
pub const TAG_PERSON: &str = "persona";

pub const CAT_ANIME: &str = "Anime";
pub const CAT_MANHWA: &str = "Manhwa";
pub const CAT_WEBTOON: &str = "Webtoon";
pub const CAT_ANIMALS: &str = "Animales";
pub const CAT_COMPUTERS_HARDWARE: &str = "Computadoras y hardware";
pub const CAT_TECH_ELECTRONICS: &str = "Tecnología y electrónica";
pub const CAT_NATURE_LANDSCAPES: &str = "Naturaleza y paisajes";
pub const CAT_VIDEOGAMES: &str = "Videojuegos";
pub const MANGA_COVER_CATEGORY: &str = "Portada de manga";

const BUILTIN_KEY_MANGA: &str = "type_manga";
const BUILTIN_KEY_ILLUSTRATION: &str = "type_illustration";
const BUILTIN_KEY_PHOTO: &str = "type_photo";
const BUILTIN_KEY_SCREENSHOT: &str = "type_screenshot";
const BUILTIN_KEY_MEME: &str = "type_meme";
const BUILTIN_KEY_DOCUMENT: &str = "type_document";
const BUILTIN_KEY_PERSON: &str = "tag_person";

const BUILTIN_KEY_ANIME: &str = "cat_anime";
const BUILTIN_KEY_MANHWA: &str = "cat_manhwa";
const BUILTIN_KEY_WEBTOON: &str = "cat_webtoon";
const BUILTIN_KEY_ANIMALS: &str = "cat_animals";
const BUILTIN_KEY_COMPUTERS_HARDWARE: &str = "cat_computers_hardware";
const BUILTIN_KEY_TECH_ELECTRONICS: &str = "cat_tech_electronics";
const BUILTIN_KEY_NATURE_LANDSCAPES: &str = "cat_nature_landscapes";
const BUILTIN_KEY_VIDEOGAMES: &str = "cat_videogames";
const BUILTIN_KEY_MANGA_COVER: &str = "page_manga_cover";

/// Keys seeded in the initial schema version under the legacy `vision_builtins_seeded = '1'` flag.
const LEGACY_V1_BUILTIN_KEYS: &[&str] = &[
    BUILTIN_KEY_MANGA,
    BUILTIN_KEY_ILLUSTRATION,
    BUILTIN_KEY_PHOTO,
    BUILTIN_KEY_SCREENSHOT,
    BUILTIN_KEY_MEME,
    BUILTIN_KEY_DOCUMENT,
    BUILTIN_KEY_PERSON,
];

const META_BUILTINS_SEEDED: &str = "vision_builtins_seeded";
const META_BUILTIN_SEEDED_PREFIX: &str = "vision_builtin_seeded:";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RuleId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RuleMode {
    /// Exclusive image type: at most one enabled Type rule is suggested per image.
    Type,
    /// Independent attribute tag: evaluated on its own regardless of other rules.
    Independent,
}

impl RuleMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Independent => "independent",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "type" => Some(Self::Type),
            "independent" => Some(Self::Independent),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DetectedFace {
    /// `[x1, y1, x2, y2]` in pixel coordinates of the decoded image.
    pub bbox: [f32; 4],
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisionRule {
    pub id: RuleId,
    pub label: LabelId,
    pub kind: LabelKind,
    pub name: String,
    pub mode: RuleMode,
    pub threshold: f32,
    pub enabled: bool,
    pub positives: Vec<String>,
    pub negatives: Vec<String>,
    pub builtin_key: Option<String>,
    /// Number of present assets where the user accepted an `source='ai'` suggestion for this label.
    pub accepted_ai: usize,
    /// Number of present assets where the user rejected an `source='ai'` suggestion for this label.
    pub rejected_ai: usize,
    /// Number of present assets with a currently pending `source='ai'` suggestion for this label.
    pub suggested_ai: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisionSuggestion {
    pub label: LabelId,
    /// Ordering heuristic in `[0.0, 1.0]` for the review queue — NOT a calibrated probability.
    pub confidence: f64,
    /// Raw maximum cosine similarity over the rule's positive descriptions (or face score).
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedAsset {
    pub entry: Entry,
    /// Maximum cosine similarity against the query embedding(s). Not a probability.
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub id: RuleId,
    pub label: LabelId,
    pub name: String,
    pub mode: RuleMode,
    pub threshold: f32,
    pub builtin_key: Option<String>,
    pub positives: Vec<[f32; EMBEDDING_DIM]>,
    pub negatives: Vec<[f32; EMBEDDING_DIM]>,
}

/// Precompiled set of enabled vision rules with their L2-normalized text embeddings.
#[derive(Debug, Clone, Default)]
pub struct CompiledVisionRules {
    pub rules: Vec<CompiledRule>,
}

impl CompiledVisionRules {
    /// Evaluates all enabled rules against one L2-normalized image embedding and its detected faces.
    ///
    /// Note: the split SigLIP 2 ONNX files do not contain `logit_scale` or `logit_bias`, so
    /// neither raw scores nor `confidence` are probabilities:
    /// - TYPE group: only the highest-cosine enabled type is suggested (when `score >= threshold`),
    ///   with `confidence = softmax(100 * cosines)[winner]` as an ordering heuristic so ambiguous
    ///   images appear first in review.
    /// - INDEPENDENT rules: suggested when `pos_score >= threshold` and (`negatives` is empty or
    ///   `pos_score > neg_score`), with a monotonic ordering heuristic `0.5 + 0.49 * m / (m + 0.05)`
    ///   in `[0.5, 0.99]`.
    /// - `"persona"` is also suggested when YuNet detected a face with `score >= 0.8` AND the
    ///   winning type is `Foto`, `Meme`, or `Capturas de pantalla` (drawn faces on manga or
    ///   illustrations must not trigger `"persona"`).
    pub fn evaluate(
        &self,
        image_embedding: &[f32; EMBEDDING_DIM],
        faces: &[DetectedFace],
    ) -> Vec<VisionSuggestion> {
        let mut out = Vec::new();

        // 1. Exclusive TYPE group.
        let mut type_scores: Vec<(&CompiledRule, f32)> = Vec::new();
        for rule in &self.rules {
            if rule.mode != RuleMode::Type || rule.positives.is_empty() {
                continue;
            }
            let score = max_dot(image_embedding, &rule.positives);
            type_scores.push((rule, score));
        }

        let mut winning_type_allows_face_person = false;
        let mut winning_type_allows_manga_cover = false;
        if !type_scores.is_empty() {
            let (win_idx, &(winner, win_score)) = type_scores
                .iter()
                .enumerate()
                .max_by(|(_, (ra, sa)), (_, (rb, sb))| {
                    sa.total_cmp(sb).then_with(|| rb.id.cmp(&ra.id))
                })
                .expect("non-empty type_scores");

            winning_type_allows_face_person = allows_face_person(winner);

            if win_score >= winner.threshold {
                winning_type_allows_manga_cover = allows_manga_cover(winner);
                let conf = type_ordering_confidence(&type_scores, win_idx);
                out.push(VisionSuggestion {
                    label: winner.label,
                    confidence: conf,
                    score: win_score,
                });
            }
        }

        let max_strong_face: Option<f32> = faces
            .iter()
            .map(|f| f.score)
            .filter(|&s| s >= YUNET_FACE_SCORE_THRESHOLD)
            .fold(None, |acc, s| Some(acc.map_or(s, |a| a.max(s))));

        // 2. Independent rules (content categories, manga cover, persona, custom tags).
        for rule in &self.rules {
            if rule.mode != RuleMode::Independent {
                continue;
            }
            if is_manga_cover_rule(rule) && !winning_type_allows_manga_cover {
                continue;
            }
            let pos_score =
                (!rule.positives.is_empty()).then(|| max_dot(image_embedding, &rule.positives));
            let neg_score =
                (!rule.negatives.is_empty()).then(|| max_dot(image_embedding, &rule.negatives));

            let text_conf = pos_score.and_then(|pos| {
                let passes_thresh = pos >= rule.threshold;
                let passes_neg = neg_score.is_none_or(|neg| pos > neg);
                if passes_thresh && passes_neg {
                    let mut margin = f64::from((pos - rule.threshold).max(0.0));
                    if let Some(neg) = neg_score {
                        margin = margin.min(f64::from((pos - neg).max(0.0)));
                    }
                    Some(independent_ordering_confidence(margin))
                } else {
                    None
                }
            });

            let face_conf = if is_person_rule(rule) && winning_type_allows_face_person {
                max_strong_face.map(|s| f64::from(s.clamp(0.0, 1.0)))
            } else {
                None
            };

            let final_conf = match (text_conf, face_conf) {
                (Some(tc), Some(fc)) => Some(tc.max(fc)),
                (Some(tc), None) => Some(tc),
                (None, Some(fc)) => Some(fc),
                (None, None) => None,
            };

            if let Some(conf) = final_conf {
                out.push(VisionSuggestion {
                    label: rule.label,
                    confidence: conf.clamp(0.0, 1.0),
                    score: pos_score
                        .or(max_strong_face.filter(|_| face_conf.is_some()))
                        .unwrap_or(0.0),
                });
            }
        }

        out
    }
}

fn allows_face_person(rule: &CompiledRule) -> bool {
    if let Some(key) = rule.builtin_key.as_deref() {
        return matches!(
            key,
            BUILTIN_KEY_PHOTO | BUILTIN_KEY_MEME | BUILTIN_KEY_SCREENSHOT
        );
    }
    rule.name.eq_ignore_ascii_case(TYPE_PHOTO)
        || rule.name.eq_ignore_ascii_case(TYPE_MEME)
        || rule.name.eq_ignore_ascii_case(SCREENSHOT_CATEGORY)
}

fn is_person_rule(rule: &CompiledRule) -> bool {
    rule.builtin_key.as_deref() == Some(BUILTIN_KEY_PERSON)
        || rule.name.eq_ignore_ascii_case(TAG_PERSON)
}

fn allows_manga_cover(rule: &CompiledRule) -> bool {
    if let Some(key) = rule.builtin_key.as_deref() {
        return matches!(key, BUILTIN_KEY_MANGA | BUILTIN_KEY_ILLUSTRATION);
    }
    rule.name.eq_ignore_ascii_case(TYPE_MANGA) || rule.name.eq_ignore_ascii_case(TYPE_ILLUSTRATION)
}

fn is_manga_cover_rule(rule: &CompiledRule) -> bool {
    rule.builtin_key.as_deref() == Some(BUILTIN_KEY_MANGA_COVER)
        || rule.name.eq_ignore_ascii_case(MANGA_COVER_CATEGORY)
}

/// Softmax over `100.0 * cosine` for the enabled TYPE rules, returning the winner's entry.
/// Used strictly as an ordering heuristic in `[0.0, 1.0]`, never as a probability.
fn type_ordering_confidence(scores: &[(&CompiledRule, f32)], winner_idx: usize) -> f64 {
    let max_c = scores
        .iter()
        .map(|(_, s)| f64::from(*s))
        .fold(f64::NEG_INFINITY, f64::max);
    let mut denom = 0.0f64;
    let mut win_num = 0.0f64;
    for (i, (_, s)) in scores.iter().enumerate() {
        let e = (100.0 * (f64::from(*s) - max_c)).exp();
        denom += e;
        if i == winner_idx {
            win_num = e;
        }
    }
    if denom <= 0.0 || !denom.is_finite() {
        0.5
    } else {
        (win_num / denom).clamp(0.0, 1.0)
    }
}

/// Strictly monotonic ordering heuristic in `[0.50, 0.99]` for independent rules.
/// At `margin == 0` (right on the threshold), returns `0.50` so borderline items surface in review.
/// Not a probability.
pub fn independent_ordering_confidence(margin: f64) -> f64 {
    let m = margin.max(0.0);
    (0.5 + 0.49 * (m / (m + 0.05))).clamp(0.0, 1.0)
}

/// Encodes a 768-dimensional f32 vector into a 3072-byte little-endian BLOB.
pub fn encode_embedding(vec: &[f32]) -> Result<Vec<u8>, CatalogError> {
    if vec.len() != EMBEDDING_DIM {
        return Err(CatalogError::Db(rusqlite::Error::InvalidParameterName(
            format!("expected {EMBEDDING_DIM}-dim embedding, got {}", vec.len()),
        )));
    }
    let mut out = Vec::with_capacity(EMBEDDING_BYTES);
    for &x in vec {
        out.extend_from_slice(&x.to_le_bytes());
    }
    Ok(out)
}

/// Decodes a 3072-byte little-endian BLOB into a fixed `[f32; 768]` array.
pub fn decode_embedding_array(blob: &[u8]) -> Result<[f32; EMBEDDING_DIM], CatalogError> {
    if blob.len() != EMBEDDING_BYTES {
        return Err(CatalogError::Db(rusqlite::Error::InvalidParameterName(
            format!(
                "expected {EMBEDDING_BYTES}-byte embedding blob, got {}",
                blob.len()
            ),
        )));
    }
    let mut out = [0.0f32; EMBEDDING_DIM];
    for (dst, chunk) in out.iter_mut().zip(blob.as_chunks::<4>().0) {
        *dst = f32::from_le_bytes(*chunk);
    }
    Ok(out)
}

#[inline]
fn dot_array(a: &[f32; EMBEDDING_DIM], b: &[f32; EMBEDDING_DIM]) -> f32 {
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let mut i = 0;
    while i < EMBEDDING_DIM {
        s0 += a[i] * b[i];
        s1 += a[i + 1] * b[i + 1];
        s2 += a[i + 2] * b[i + 2];
        s3 += a[i + 3] * b[i + 3];
        i += 4;
    }
    (s0 + s1) + (s2 + s3)
}

/// Fast zero-allocation dot product directly over a 3072-byte little-endian BLOB.
#[inline]
fn dot_blob_le(blob: &[u8], query: &[f32; EMBEDDING_DIM]) -> f32 {
    debug_assert_eq!(blob.len(), EMBEDDING_BYTES);
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let mut i = 0;
    while i < EMBEDDING_DIM {
        let o = i * 4;
        let a0 = f32::from_le_bytes([blob[o], blob[o + 1], blob[o + 2], blob[o + 3]]);
        let a1 = f32::from_le_bytes([blob[o + 4], blob[o + 5], blob[o + 6], blob[o + 7]]);
        let a2 = f32::from_le_bytes([blob[o + 8], blob[o + 9], blob[o + 10], blob[o + 11]]);
        let a3 = f32::from_le_bytes([blob[o + 12], blob[o + 13], blob[o + 14], blob[o + 15]]);
        s0 += a0 * query[i];
        s1 += a1 * query[i + 1];
        s2 += a2 * query[i + 2];
        s3 += a3 * query[i + 3];
        i += 4;
    }
    (s0 + s1) + (s2 + s3)
}

#[inline]
fn max_dot(img: &[f32; EMBEDDING_DIM], queries: &[[f32; EMBEDDING_DIM]]) -> f32 {
    queries
        .iter()
        .map(|q| dot_array(img, q))
        .fold(f32::NEG_INFINITY, f32::max)
}

fn join_descriptions(lines: &[String]) -> String {
    lines
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn split_descriptions(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

struct BuiltinSpec {
    key: &'static str,
    kind: LabelKind,
    name: &'static str,
    mode: RuleMode,
    threshold: f32,
    positives: &'static [&'static str],
    negatives: &'static [&'static str],
}

/// Built-in vision rules. Written in English for SigLIP 2 (users may edit them or add Spanish
/// descriptions). Thresholds are UNCALIBRATED placeholders from synthetic tests.
const BUILTIN_RULES: &[BuiltinSpec] = &[
    BuiltinSpec {
        key: BUILTIN_KEY_MANGA,
        kind: LabelKind::Category,
        name: TYPE_MANGA,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "a black and white manga comic page with panels and speech bubbles",
            "a full color manhwa or webtoon comic strip page with speech bubbles",
            "a scanned comic book page with dialogue bubbles and panel borders",
            "a manga scanlation page with translated text in speech bubbles",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_ILLUSTRATION,
        kind: LabelKind::Category,
        name: TYPE_ILLUSTRATION,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "an anime style digital illustration artwork, single scene without comic panels",
            "a digital painting or fanart drawing of a character",
            "a colorful anime wallpaper or cover illustration",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_PHOTO,
        kind: LabelKind::Category,
        name: TYPE_PHOTO,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "a real life camera photograph taken outdoors or indoors",
            "a candid smartphone photograph of people, places, or objects",
            "a realistic landscape or portrait photograph",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_SCREENSHOT,
        kind: LabelKind::Category,
        name: SCREENSHOT_CATEGORY,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "a smartphone screenshot of a chat app, social media, or mobile interface",
            "a computer desktop screenshot showing windows, menus, or a web browser",
            "a screen capture of a mobile phone or application user interface",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_MEME,
        kind: LabelKind::Category,
        name: TYPE_MEME,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "an internet meme image with bold caption text",
            "a funny reaction image or humorous social media meme",
            "a captioned comedy macro image",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_DOCUMENT,
        kind: LabelKind::Category,
        name: TYPE_DOCUMENT,
        mode: RuleMode::Type,
        threshold: DEFAULT_TYPE_THRESHOLD,
        positives: &[
            "a white page of printed text document or receipt",
            "a scanned paper document, form, or page of typed text",
            "a document page with paragraphs of printed words",
        ],
        negatives: &[],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_PERSON,
        kind: LabelKind::Tag,
        name: TAG_PERSON,
        mode: RuleMode::Independent,
        threshold: DEFAULT_PERSON_THRESHOLD,
        positives: &[
            "a real photograph of a human person",
            "a camera photo showing a real person or people",
            "a candid portrait photo of a real man or woman",
        ],
        negatives: &[
            "an anime drawing, cartoon, or manga character",
            "a digital illustration or comic book character",
            "a text document, screenshot, or empty landscape without people",
        ],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_ANIME,
        kind: LabelKind::Category,
        name: CAT_ANIME,
        mode: RuleMode::Independent,
        threshold: ANIME_THRESHOLD,
        positives: &[
            "an anime screenshot or anime style artwork with Japanese animation characters",
            "a Japanese anime television series frame or anime movie still",
            "colorful anime character illustration with cel shading",
        ],
        negatives: &[
            "a real life camera photograph of real people or scenery",
            "a plain text document or chat screenshot without anime art",
        ],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_MANHWA,
        kind: LabelKind::Category,
        name: CAT_MANHWA,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a Korean manhwa comic page with full color digital art and speech bubbles",
            "manhwa style comic panels with Korean webcomic art and dialogue",
            "colorful Korean comic book chapter page or manhwa illustration",
        ],
        negatives: &[
            "a black and white Japanese manga page",
            "a real life camera photograph",
        ],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_WEBTOON,
        kind: LabelKind::Category,
        name: CAT_WEBTOON,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a vertical scrolling webtoon comic strip in full color",
            "long vertical webtoon panel layout with speech bubbles and digital coloring",
            "mobile vertical comic strip webtoon episode page",
        ],
        negatives: &[
            "a real life camera photograph",
            "a printed paper text document or receipt",
        ],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_ANIMALS,
        kind: LabelKind::Category,
        name: CAT_ANIMALS,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a photograph or image of a real animal, pet, dog, cat, or bird",
            "wildlife, domestic pets, or animals outdoors or indoors",
            "a close up picture of a cat, dog, bird, or other animal",
        ],
        negatives: &["a text document, chat screenshot, or webpage without animals"],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_COMPUTERS_HARDWARE,
        kind: LabelKind::Category,
        name: CAT_COMPUTERS_HARDWARE,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a desktop computer, PC case, graphics card, motherboard, or computer hardware",
            "a PC desk setup with monitor, mechanical keyboard, mouse, and computer tower",
            "computer components, CPU, RAM, SSD, laptop, or PC build",
        ],
        negatives: &["a natural outdoor landscape or portrait without computers"],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_TECH_ELECTRONICS,
        kind: LabelKind::Category,
        name: CAT_TECH_ELECTRONICS,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "modern electronics, smartphone, tablet, headphones, gadgets, or circuit board",
            "electronic devices, cables, game consoles, microcontrollers, or tech hardware",
            "consumer technology products, screens, audio gear, or electronic equipment",
        ],
        negatives: &["natural outdoor scenery or hand-drawn comic without electronics"],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_NATURE_LANDSCAPES,
        kind: LabelKind::Category,
        name: CAT_NATURE_LANDSCAPES,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a natural landscape photograph of mountains, forest, beach, ocean, or sky",
            "outdoor nature scenery with trees, sunset, lake, rivers, or flowers",
            "scenic view of countryside, hills, clouds, or natural environment",
        ],
        negatives: &[
            "a smartphone chat screenshot or printed text document",
            "an indoor computer desk or comic page with speech bubbles",
        ],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_VIDEOGAMES,
        kind: LabelKind::Category,
        name: CAT_VIDEOGAMES,
        mode: RuleMode::Independent,
        threshold: DEFAULT_CATEGORY_THRESHOLD,
        positives: &[
            "a video game screenshot showing 3D or 2D gameplay, HUD, health bar, or game menu",
            "PC or console gaming screenshot with in-game characters, map, or interface",
            "video game cover art, gameplay scene, or retro pixel art game screen",
        ],
        negatives: &["a plain text document, receipt, or chat messaging app screenshot"],
    },
    BuiltinSpec {
        key: BUILTIN_KEY_MANGA_COVER,
        kind: LabelKind::Category,
        name: MANGA_COVER_CATEGORY,
        mode: RuleMode::Independent,
        threshold: MANGA_COVER_THRESHOLD,
        positives: &[
            "a colorful manga volume cover or chapter cover page with title logo and character art",
            "a Japanese manga book cover illustration with series title text",
            "a full-page manga color spread or front cover artwork without dialogue bubbles",
        ],
        negatives: &[
            "a black and white manga chapter page with multiple panels and speech bubbles",
            "a credits page or list of multiple small comic covers",
            "a real life camera photograph or smartphone screenshot",
        ],
    },
];

#[derive(Debug, Clone, Copy)]
struct HeapItem {
    asset_id: i64,
    score: f32,
}

impl PartialEq for HeapItem {
    fn eq(&self, other: &Self) -> bool {
        self.asset_id == other.asset_id && self.score.to_bits() == other.score.to_bits()
    }
}

impl Eq for HeapItem {}

/// Reverse ordering so `BinaryHeap` acts as a min-heap keeping the top-N highest scores.
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| self.asset_id.cmp(&other.asset_id))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Catalog {
    /// Seeds each built-in vision rule at most once per catalog database, tracked individually by
    /// `builtin_key`. Existing catalogs that already have the legacy `vision_builtins_seeded = '1'`
    /// flag receive only the newer built-ins once, never recreating or overwriting edited/deleted
    /// rules, and reusing any existing label of the same kind and name (any case) without replacing
    /// an existing rule.
    pub(crate) fn seed_builtin_vision_rules(&mut self) -> Result<(), CatalogError> {
        // An up-to-date catalog must open without writing: other connections may hold the lock.
        let mut flag = self
            .conn
            .prepare("SELECT 1 FROM catalog_meta WHERE key = ?1 AND value = '1'")?;
        let mut missing = false;
        for spec in BUILTIN_RULES {
            missing |= !flag.exists([format!("{META_BUILTIN_SEEDED_PREFIX}{}", spec.key)])?;
        }
        drop(flag);
        if !missing {
            return Ok(());
        }

        // Immediate, so two connections upgrading at once wait for each other instead of failing.
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let legacy_seeded = tx
            .query_row(
                "SELECT value FROM catalog_meta WHERE key = ?1",
                [META_BUILTINS_SEEDED],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .as_deref()
            == Some("1");

        for spec in BUILTIN_RULES {
            let meta_key = format!("{META_BUILTIN_SEEDED_PREFIX}{}", spec.key);
            let already = tx
                .query_row(
                    "SELECT value FROM catalog_meta WHERE key = ?1",
                    [&meta_key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?
                .as_deref()
                == Some("1");
            if already {
                continue;
            }

            if legacy_seeded && LEGACY_V1_BUILTIN_KEYS.contains(&spec.key) {
                tx.execute(
                    "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, '1')",
                    [&meta_key],
                )?;
                continue;
            }

            let kind_str = match spec.kind {
                LabelKind::Category => "category",
                LabelKind::Tag => "tag",
                LabelKind::Entity => "entity",
            };
            tx.execute(
                "INSERT OR IGNORE INTO label (kind, name) VALUES (?1, ?2)",
                params![kind_str, spec.name],
            )?;
            let label_id: i64 = tx.query_row(
                "SELECT id FROM label WHERE kind = ?1 AND name = ?2",
                params![kind_str, spec.name],
                |r| r.get(0),
            )?;
            let pos = spec.positives.join("\n");
            let neg = spec.negatives.join("\n");
            tx.execute(
                "INSERT OR IGNORE INTO vision_rule
                     (label_id, mode, threshold, enabled, positives, negatives, builtin_key)
                 VALUES (?1, ?2, ?3, ?7, ?4, ?5, ?6)",
                params![
                    label_id,
                    spec.mode.as_str(),
                    f64::from(spec.threshold),
                    pos,
                    neg,
                    spec.key,
                    !DISABLED_BY_DEFAULT.contains(&spec.key),
                ],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO vision_managed_label (label_id) VALUES (?1)",
                [label_id],
            )?;
            tx.execute(
                "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, '1')",
                [&meta_key],
            )?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, '1')",
            [META_BUILTINS_SEEDED],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Gives built-in rules the user never edited the values calibrated on real images. Rules the
    /// user changed in any way keep their values. Runs once per catalog.
    pub(crate) fn calibrate_builtin_vision_rules(&mut self) -> Result<(), CatalogError> {
        let done = |c: &rusqlite::Connection| -> Result<bool, CatalogError> {
            Ok(c.prepare("SELECT 1 FROM catalog_meta WHERE key = ?1")?
                .exists([META_BUILTINS_CALIBRATED])?)
        };
        if done(&self.conn)? {
            return Ok(());
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if done(&tx)? {
            return Ok(());
        }
        for spec in BUILTIN_RULES {
            let old = if spec.mode == RuleMode::Type {
                0.08
            } else {
                DEFAULT_CATEGORY_THRESHOLD
            };
            tx.execute(
                "UPDATE vision_rule SET threshold = ?2, enabled = ?3
                 WHERE builtin_key = ?1 AND enabled = 1 AND abs(threshold - ?4) < 1e-6
                   AND positives = ?5 AND negatives = ?6",
                params![
                    spec.key,
                    f64::from(spec.threshold),
                    !DISABLED_BY_DEFAULT.contains(&spec.key),
                    old,
                    spec.positives.join("\n"),
                    spec.negatives.join("\n"),
                ],
            )?;
        }
        tx.execute(
            "INSERT INTO catalog_meta (key, value) VALUES (?1, '1')",
            [META_BUILTINS_CALIBRATED],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Saves the SigLIP 2 image embedding and detected faces for `asset` and marks analysis stage
    /// `"vision"` version `1` done in ONE transaction.
    pub fn save_vision_result(
        &mut self,
        asset: AssetId,
        model: &str,
        embedding: &[f32],
        faces: &[DetectedFace],
    ) -> Result<(), CatalogError> {
        let blob = encode_embedding(embedding)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO asset_embedding (asset_id, model, vector)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id) DO UPDATE SET model = excluded.model, vector = excluded.vector",
            params![asset.0, model, blob],
        )?;
        tx.execute("DELETE FROM asset_face WHERE asset_id = ?1", [asset.0])?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO asset_face (asset_id, face_idx, x1, y1, x2, y2, score)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for (idx, face) in faces.iter().enumerate() {
                stmt.execute(params![
                    asset.0,
                    idx as i64,
                    f64::from(face.bbox[0]),
                    f64::from(face.bbox[1]),
                    f64::from(face.bbox[2]),
                    f64::from(face.bbox[3]),
                    f64::from(face.score.clamp(0.0, 1.0)),
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO analysis (asset_id, stage, version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id, stage) DO UPDATE SET version = excluded.version
             WHERE analysis.version < excluded.version",
            params![asset.0, VISION_STAGE, i64::from(VISION_VERSION)],
        )?;
        tx.execute(
            "DELETE FROM stage_error WHERE asset_id = ?1 AND stage = ?2",
            params![asset.0, VISION_STAGE],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Returns the stored `(model, embedding)` for `asset`, if any.
    pub fn asset_embedding(
        &self,
        asset: AssetId,
    ) -> Result<Option<(String, Vec<f32>)>, CatalogError> {
        let row: Option<(String, Vec<u8>)> = self
            .conn
            .query_row(
                "SELECT model, vector FROM asset_embedding WHERE asset_id = ?1",
                [asset.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((model, blob)) => {
                let arr = decode_embedding_array(&blob)?;
                Ok(Some((model, arr.to_vec())))
            }
            None => Ok(None),
        }
    }

    /// Returns all detected faces stored for `asset`, ordered by `face_idx`.
    pub fn asset_faces(&self, asset: AssetId) -> Result<Vec<DetectedFace>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT x1, y1, x2, y2, score
             FROM asset_face
             WHERE asset_id = ?1
             ORDER BY face_idx ASC",
        )?;
        let rows = stmt.query_map([asset.0], |r| {
            Ok(DetectedFace {
                bbox: [
                    r.get::<_, f64>(0)? as f32,
                    r.get::<_, f64>(1)? as f32,
                    r.get::<_, f64>(2)? as f32,
                    r.get::<_, f64>(3)? as f32,
                ],
                score: r.get::<_, f64>(4)? as f32,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns `true` if `asset` has completed vision analysis at the current version.
    pub fn has_vision_done(&self, asset: AssetId) -> Result<bool, CatalogError> {
        let done: i64 = self.conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM analysis
                 WHERE asset_id = ?1 AND stage = ?2 AND version >= ?3
             )",
            params![asset.0, VISION_STAGE, i64::from(VISION_VERSION)],
            |r| r.get(0),
        )?;
        Ok(done != 0)
    }

    /// Count of present assets under `roots` that have a stored vision embedding.
    pub fn count_assets_with_vision_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<usize, CatalogError> {
        if roots.is_empty() {
            return Ok(0);
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT COUNT(*)
             FROM asset_embedding e
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))"
        );
        let args: Vec<Box<dyn rusqlite::ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn rusqlite::ToSql>)
            .collect();
        let count: i64 =
            self.conn
                .query_row(&sql, rusqlite::params_from_iter(args.iter()), |r| r.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Lists all configured vision rules with per-rule counts restricted to present assets under `roots`.
    pub fn list_vision_rules_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<VisionRule>, CatalogError> {
        if roots.is_empty() {
            let mut stmt = self.conn.prepare(
                "SELECT vr.id, vr.label_id, lb.kind, lb.name, vr.mode, vr.threshold, vr.enabled,
                        vr.positives, vr.negatives, vr.builtin_key, 0, 0, 0
                 FROM vision_rule vr
                 JOIN label lb ON lb.id = vr.label_id
                 ORDER BY CASE vr.mode WHEN 'type' THEN 0 ELSE 1 END, vr.id ASC",
            )?;
            let rows = stmt.query_map([], map_vision_rule_row)?;
            return Ok(rows.collect::<Result<_, _>>()?);
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT vr.id, vr.label_id, lb.kind, lb.name, vr.mode, vr.threshold, vr.enabled,
                    vr.positives, vr.negatives, vr.builtin_key,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'accepted'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0 AND l.root IN ({marks}))) AS acc,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'rejected'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0 AND l.root IN ({marks}))) AS rej,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'suggested'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0 AND l.root IN ({marks}))) AS sug
             FROM vision_rule vr
             JOIN label lb ON lb.id = vr.label_id
             ORDER BY CASE vr.mode WHEN 'type' THEN 0 ELSE 1 END, vr.id ASC"
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() * 3);
        for _ in 0..3 {
            for r in roots {
                args.push(Box::new(r.to_string_lossy().into_owned()));
            }
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), map_vision_rule_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Count of present assets that have a stored vision embedding.
    pub fn count_assets_with_vision(&self) -> Result<usize, CatalogError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*)
             FROM asset_embedding e
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id AND l.missing = 0)",
            [],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Returns all present assets that have a stored vision embedding, ordered by `asset_id`.
    pub fn assets_with_vision(&self) -> Result<Vec<AssetId>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT e.asset_id
             FROM asset_embedding e
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id AND l.missing = 0)
             ORDER BY e.asset_id",
        )?;
        let rows = stmt.query_map([], |r| Ok(AssetId(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Lists all configured vision rules (TYPE group first, then INDEPENDENT), with per-rule
    /// accepted/rejected/suggested counts of `source='ai'` rows on present assets.
    pub fn list_vision_rules(&self) -> Result<Vec<VisionRule>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT vr.id, vr.label_id, lb.kind, lb.name, vr.mode, vr.threshold, vr.enabled,
                    vr.positives, vr.negatives, vr.builtin_key,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'accepted'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0)) AS acc,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'rejected'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0)) AS rej,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = vr.label_id AND al.source = 'ai' AND al.status = 'suggested'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0)) AS sug
             FROM vision_rule vr
             JOIN label lb ON lb.id = vr.label_id
             ORDER BY CASE vr.mode WHEN 'type' THEN 0 ELSE 1 END, vr.id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind_s: String = r.get(2)?;
            let mode_s: String = r.get(4)?;
            let pos_s: String = r.get(7)?;
            let neg_s: String = r.get(8)?;
            Ok(VisionRule {
                id: RuleId(r.get(0)?),
                label: LabelId(r.get(1)?),
                kind: LabelKind::parse(&kind_s).ok_or_else(|| unexpected(2, &kind_s))?,
                name: r.get(3)?,
                mode: RuleMode::parse(&mode_s).ok_or_else(|| unexpected(4, &mode_s))?,
                threshold: r.get::<_, f64>(5)? as f32,
                enabled: r.get::<_, i64>(6)? != 0,
                positives: split_descriptions(&pos_s),
                negatives: split_descriptions(&neg_s),
                builtin_key: r.get(9)?,
                accepted_ai: r.get::<_, i64>(10)?.max(0) as usize,
                rejected_ai: r.get::<_, i64>(11)?.max(0) as usize,
                suggested_ai: r.get::<_, i64>(12)?.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Creates a new vision rule. Fails if that label already has one: a built-in or a rule the
    /// user tuned is never replaced by creating a tag with the same name.
    pub fn create_vision_rule(
        &mut self,
        name: &str,
        kind: LabelKind,
        mode: RuleMode,
        threshold: f32,
        positives: &[String],
        negatives: &[String],
    ) -> Result<RuleId, CatalogError> {
        let trimmed = name.trim();
        let label = self.label(kind, trimmed)?;
        let pos = join_descriptions(positives);
        let neg = join_descriptions(negatives);
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO vision_rule (label_id, mode, threshold, enabled, positives, negatives)
             VALUES (?1, ?2, ?3, 1, ?4, ?5)",
            params![label.0, mode.as_str(), f64::from(threshold), pos, neg],
        )?;
        tx.execute(
            "INSERT OR IGNORE INTO vision_managed_label (label_id) VALUES (?1)",
            [label.0],
        )?;
        let id: i64 = tx.query_row(
            "SELECT id FROM vision_rule WHERE label_id = ?1",
            [label.0],
            |r| r.get(0),
        )?;
        tx.commit()?;
        Ok(RuleId(id))
    }

    /// Updates an existing vision rule's enabled flag, threshold, and positive/negative descriptions.
    pub fn update_vision_rule(
        &self,
        id: RuleId,
        enabled: bool,
        threshold: f32,
        positives: &[String],
        negatives: &[String],
    ) -> Result<bool, CatalogError> {
        let pos = join_descriptions(positives);
        let neg = join_descriptions(negatives);
        let changed = self.conn.execute(
            "UPDATE vision_rule
             SET enabled = ?2, threshold = ?3, positives = ?4, negatives = ?5
             WHERE id = ?1",
            params![id.0, i64::from(enabled), f64::from(threshold), pos, neg],
        )?;
        Ok(changed > 0)
    }

    /// Deletes a vision rule while keeping its `label` and all user decisions (`accepted`/`rejected`).
    /// Its `label_id` stays recorded in `vision_managed_label` so the next apply removes any
    /// remaining `source='ai', status='suggested'` rows that the deleted rule created.
    pub fn delete_vision_rule(&self, id: RuleId) -> Result<bool, CatalogError> {
        let changed = self
            .conn
            .execute("DELETE FROM vision_rule WHERE id = ?1", [id.0])?;
        Ok(changed > 0)
    }

    /// Looks up a cached text embedding for `(model, text)`.
    pub fn cached_text_embedding(
        &self,
        model: &str,
        text: &str,
    ) -> Result<Option<Vec<f32>>, CatalogError> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT vector FROM text_embedding_cache WHERE model = ?1 AND text = ?2",
                params![model, text],
                |r| r.get(0),
            )
            .optional()?;
        match blob {
            Some(b) => Ok(Some(decode_embedding_array(&b)?.to_vec())),
            None => Ok(None),
        }
    }

    /// Stores one or more `(text, embedding)` pairs in `text_embedding_cache`.
    pub fn save_text_embeddings(
        &mut self,
        model: &str,
        pairs: &[(&str, &[f32])],
    ) -> Result<(), CatalogError> {
        if pairs.is_empty() {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO text_embedding_cache (model, text, vector)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(model, text) DO UPDATE SET vector = excluded.vector",
            )?;
            for &(text, vec) in pairs {
                let blob = encode_embedding(vec)?;
                stmt.execute(params![model, text, blob])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Compiles all enabled vision rules using cached text embeddings for `model`.
    /// Returns `(compiled, missing_texts)` where `missing_texts` lists every unique description
    /// across enabled rules that is not yet in `text_embedding_cache`.
    pub fn compile_vision_rules(
        &self,
        model: &str,
    ) -> Result<(CompiledVisionRules, Vec<String>), CatalogError> {
        let rules = self.list_vision_rules()?;
        let mut cache_stmt = self
            .conn
            .prepare("SELECT vector FROM text_embedding_cache WHERE model = ?1 AND text = ?2")?;
        let mut missing = Vec::new();
        let mut seen_missing = std::collections::HashSet::new();
        let mut compiled = Vec::new();

        for rule in rules {
            if !rule.enabled {
                continue;
            }
            let mut pos_vecs = Vec::with_capacity(rule.positives.len());
            for desc in &rule.positives {
                let blob: Option<Vec<u8>> = cache_stmt
                    .query_row(params![model, desc], |r| r.get(0))
                    .optional()?;
                match blob {
                    Some(b) => pos_vecs.push(decode_embedding_array(&b)?),
                    None => {
                        if seen_missing.insert(desc.clone()) {
                            missing.push(desc.clone());
                        }
                    }
                }
            }
            let mut neg_vecs = Vec::with_capacity(rule.negatives.len());
            for desc in &rule.negatives {
                let blob: Option<Vec<u8>> = cache_stmt
                    .query_row(params![model, desc], |r| r.get(0))
                    .optional()?;
                match blob {
                    Some(b) => neg_vecs.push(decode_embedding_array(&b)?),
                    None => {
                        if seen_missing.insert(desc.clone()) {
                            missing.push(desc.clone());
                        }
                    }
                }
            }
            compiled.push(CompiledRule {
                id: rule.id,
                label: rule.label,
                name: rule.name,
                mode: rule.mode,
                threshold: rule.threshold,
                builtin_key: rule.builtin_key,
                positives: pos_vecs,
                negatives: neg_vecs,
            });
        }

        Ok((CompiledVisionRules { rules: compiled }, missing))
    }

    /// Applies `rules` to one asset using its stored embedding and faces, idempotently and
    /// transactionally. Replaces only `source='ai', status='suggested'` rows belonging to vision
    /// rules; never touches accepted/rejected rows or suggestions from other sources.
    pub fn apply_vision_rules(
        &mut self,
        asset: AssetId,
        rules: &CompiledVisionRules,
    ) -> Result<Vec<VisionSuggestion>, CatalogError> {
        let Some((_, vec)) = self.asset_embedding(asset)? else {
            return Ok(Vec::new());
        };
        let arr: [f32; EMBEDDING_DIM] = vec
            .try_into()
            .map_err(|_| CatalogError::Db(rusqlite::Error::InvalidQuery))?;
        let faces = self.asset_faces(asset)?;
        let suggestions = rules.evaluate(&arr, &faces);

        let tx = self.conn.transaction()?;
        apply_suggestions_in_tx(&tx, asset, &suggestions)?;
        tx.commit()?;
        Ok(suggestions)
    }

    /// Applies `rules` to a slice of assets in a single transaction for high throughput (used by
    /// bulk re-apply and the 45k benchmark). Preserves the exact per-asset idempotence and safety
    /// guarantees. Returns `(matched_assets, total_suggestions)`.
    pub fn apply_vision_rules_batch(
        &mut self,
        assets: &[AssetId],
        rules: &CompiledVisionRules,
    ) -> Result<(usize, usize), CatalogError> {
        if assets.is_empty() {
            return Ok((0, 0));
        }

        let tx = self.conn.transaction()?;
        let mut matched_assets = 0usize;
        let mut total_suggestions = 0usize;

        {
            let mut emb_stmt =
                tx.prepare_cached("SELECT vector FROM asset_embedding WHERE asset_id = ?1")?;
            let mut face_stmt = tx.prepare_cached(
                "SELECT x1, y1, x2, y2, score FROM asset_face WHERE asset_id = ?1 ORDER BY face_idx ASC",
            )?;

            for &asset in assets {
                let blob: Option<Vec<u8>> =
                    emb_stmt.query_row([asset.0], |r| r.get(0)).optional()?;
                let Some(blob) = blob else {
                    continue;
                };
                let arr = decode_embedding_array(&blob)?;
                let faces: Vec<DetectedFace> = face_stmt
                    .query_map([asset.0], |r| {
                        Ok(DetectedFace {
                            bbox: [
                                r.get::<_, f64>(0)? as f32,
                                r.get::<_, f64>(1)? as f32,
                                r.get::<_, f64>(2)? as f32,
                                r.get::<_, f64>(3)? as f32,
                            ],
                            score: r.get::<_, f64>(4)? as f32,
                        })
                    })?
                    .collect::<Result<_, _>>()?;

                let suggestions = rules.evaluate(&arr, &faces);
                apply_suggestions_in_tx(&tx, asset, &suggestions)?;
                if !suggestions.is_empty() {
                    matched_assets += 1;
                    total_suggestions += suggestions.len();
                }
            }
        }

        tx.commit()?;
        Ok((matched_assets, total_suggestions))
    }

    /// Ranks present assets with stored `model` embeddings by maximum cosine similarity against
    /// `queries`, returning up to `limit` assets sorted by descending cosine similarity.
    pub fn rank_by_embeddings(
        &self,
        model: &str,
        queries: &[&[f32]],
        limit: usize,
    ) -> Result<Vec<RankedAsset>, CatalogError> {
        if queries.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let mut q_arrays = Vec::with_capacity(queries.len());
        for q in queries {
            if q.len() != EMBEDDING_DIM {
                return Err(CatalogError::Db(rusqlite::Error::InvalidParameterName(
                    format!("expected {EMBEDDING_DIM}-dim query, got {}", q.len()),
                )));
            }
            let mut arr = [0.0f32; EMBEDDING_DIM];
            arr.copy_from_slice(q);
            q_arrays.push(arr);
        }

        // Single streaming scan over present assets' embeddings with zero per-row BLOB allocation.
        let mut stmt = self.conn.prepare(
            "SELECT e.asset_id, e.vector
             FROM asset_embedding e
             WHERE e.model = ?1
               AND EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id AND l.missing = 0)",
        )?;

        let mut heap: BinaryHeap<HeapItem> = BinaryHeap::with_capacity(limit.min(4096) + 1);
        let mut rows = stmt.query([model])?;
        while let Some(row) = rows.next()? {
            let asset_id: i64 = row.get(0)?;
            let blob = row.get_ref(1)?.as_blob().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            if blob.len() != EMBEDDING_BYTES {
                continue;
            }
            let score = q_arrays
                .iter()
                .map(|q| dot_blob_le(blob, q))
                .fold(f32::NEG_INFINITY, f32::max);
            let item = HeapItem { asset_id, score };
            if heap.len() < limit {
                heap.push(item);
            } else if let Some(&worst) = heap.peek()
                && item < worst
            {
                // Remember HeapItem reverses Ord: `item < worst` means `item` has a HIGHER score.
                heap.pop();
                heap.push(item);
            }
        }
        drop(rows);
        drop(stmt);

        let mut top: Vec<HeapItem> = heap.into_vec();
        top.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.asset_id.cmp(&b.asset_id))
        });

        if top.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate Entry metadata for the top-N winners only.
        let mut entries_by_id: HashMap<i64, Entry> = HashMap::with_capacity(top.len());
        let mut entry_stmt = self.conn.prepare(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns
             FROM location l JOIN asset a ON a.id = l.asset_id
             WHERE l.asset_id = ?1 AND l.missing = 0
             ORDER BY l.path ASC
             LIMIT 1",
        )?;
        for item in &top {
            if let Some(entry) = entry_stmt
                .query_row([item.asset_id], crate::browse::entry_from_row)
                .optional()?
            {
                entries_by_id.insert(item.asset_id, entry);
            }
        }

        let mut out = Vec::with_capacity(top.len());
        for item in top {
            if let Some(entry) = entries_by_id.remove(&item.asset_id) {
                out.push(RankedAsset {
                    entry,
                    score: item.score,
                });
            }
        }
        Ok(out)
    }

    /// Like [`rank_by_embeddings`](Self::rank_by_embeddings), restricted to assets under `roots`.
    pub fn rank_by_embeddings_in_roots(
        &self,
        model: &str,
        queries: &[&[f32]],
        roots: &[PathBuf],
        limit: usize,
    ) -> Result<Vec<RankedAsset>, CatalogError> {
        if queries.is_empty() || limit == 0 || roots.is_empty() {
            return Ok(Vec::new());
        }
        let mut q_arrays = Vec::with_capacity(queries.len());
        for q in queries {
            if q.len() != EMBEDDING_DIM {
                return Err(CatalogError::Db(rusqlite::Error::InvalidParameterName(
                    format!("expected {EMBEDDING_DIM}-dim query, got {}", q.len()),
                )));
            }
            let mut arr = [0.0f32; EMBEDDING_DIM];
            arr.copy_from_slice(q);
            q_arrays.push(arr);
        }

        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT e.asset_id, e.vector
             FROM asset_embedding e
             WHERE e.model = ?
               AND EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))"
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() + 1);
        args.push(Box::new(model.to_string()));
        for r in roots {
            args.push(Box::new(r.to_string_lossy().into_owned()));
        }
        let mut stmt = self.conn.prepare(&sql)?;

        let mut heap: BinaryHeap<HeapItem> = BinaryHeap::with_capacity(limit.min(4096) + 1);
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        while let Some(row) = rows.next()? {
            let asset_id: i64 = row.get(0)?;
            let blob = row.get_ref(1)?.as_blob().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            if blob.len() != EMBEDDING_BYTES {
                continue;
            }
            let score = q_arrays
                .iter()
                .map(|q| dot_blob_le(blob, q))
                .fold(f32::NEG_INFINITY, f32::max);
            let item = HeapItem { asset_id, score };
            if heap.len() < limit {
                heap.push(item);
            } else if let Some(&worst) = heap.peek()
                && item < worst
            {
                heap.pop();
                heap.push(item);
            }
        }
        drop(rows);
        drop(stmt);

        let mut top: Vec<HeapItem> = heap.into_vec();
        top.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.asset_id.cmp(&b.asset_id))
        });

        if top.is_empty() {
            return Ok(Vec::new());
        }

        let mut entries_by_id: HashMap<i64, Entry> = HashMap::with_capacity(top.len());
        let entry_sql = format!(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns
             FROM location l JOIN asset a ON a.id = l.asset_id
             WHERE l.asset_id = ? AND l.missing = 0 AND l.root IN ({marks})
             ORDER BY l.path ASC
             LIMIT 1"
        );
        let mut entry_stmt = self.conn.prepare(&entry_sql)?;
        for item in &top {
            let mut e_args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() + 1);
            e_args.push(Box::new(item.asset_id));
            for r in roots {
                e_args.push(Box::new(r.to_string_lossy().into_owned()));
            }
            if let Some(entry) = entry_stmt
                .query_row(
                    rusqlite::params_from_iter(e_args.iter()),
                    crate::browse::entry_from_row,
                )
                .optional()?
            {
                entries_by_id.insert(item.asset_id, entry);
            }
        }

        let mut out = Vec::with_capacity(top.len());
        for item in top {
            if let Some(entry) = entries_by_id.remove(&item.asset_id) {
                out.push(RankedAsset {
                    entry,
                    score: item.score,
                });
            }
        }
        Ok(out)
    }
}

fn map_vision_rule_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<VisionRule> {
    let kind_s: String = r.get(2)?;
    let mode_s: String = r.get(4)?;
    let pos_s: String = r.get(7)?;
    let neg_s: String = r.get(8)?;
    Ok(VisionRule {
        id: RuleId(r.get(0)?),
        label: LabelId(r.get(1)?),
        kind: LabelKind::parse(&kind_s).ok_or_else(|| unexpected(2, &kind_s))?,
        name: r.get(3)?,
        mode: RuleMode::parse(&mode_s).ok_or_else(|| unexpected(4, &mode_s))?,
        threshold: r.get::<_, f64>(5)? as f32,
        enabled: r.get::<_, i64>(6)? != 0,
        positives: split_descriptions(&pos_s),
        negatives: split_descriptions(&neg_s),
        builtin_key: r.get(9)?,
        accepted_ai: r.get::<_, i64>(10)?.max(0) as usize,
        rejected_ai: r.get::<_, i64>(11)?.max(0) as usize,
        suggested_ai: r.get::<_, i64>(12)?.max(0) as usize,
    })
}

fn apply_suggestions_in_tx(
    tx: &rusqlite::Transaction<'_>,
    asset: AssetId,
    suggestions: &[VisionSuggestion],
) -> Result<(), CatalogError> {
    // 1. Remove only previous AI suggestions belonging to vision-managed labels (including
    //    rules that were subsequently disabled or deleted). Never touch accepted/rejected rows
    //    or rows from other sources (`rules`, `ocr`, `user`).
    {
        let mut del = tx.prepare_cached(
            "DELETE FROM asset_label
             WHERE asset_id = ?1
               AND source = 'ai'
               AND status = 'suggested'
               AND label_id IN (SELECT label_id FROM vision_managed_label)",
        )?;
        del.execute([asset.0])?;
    }

    // 2. Insert new AI suggestions; if a row already exists (accepted/rejected, or suggested by
    //    another source like `rules` or `ocr`), NEVER overwrite it.
    {
        let mut ins = tx.prepare_cached(
            "INSERT OR IGNORE INTO asset_label (asset_id, label_id, source, confidence, status)
             VALUES (?1, ?2, 'ai', ?3, 'suggested')",
        )?;
        for s in suggestions {
            ins.execute(params![asset.0, s.label.0, s.confidence.clamp(0.0, 1.0)])?;
        }
    }

    crate::works::reconcile_page_types_in_tx(tx, asset)?;

    Ok(())
}
