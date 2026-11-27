use crate::labels::unexpected;
use crate::ocr::{OCR_STAGE, OCR_VERSION, OcrLine, SCREENSHOT_CATEGORY, ocr_search_key};
use crate::{AssetId, Catalog, CatalogError, Entry, LabelId, LabelKind, Source, Status};
use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;

pub const PROMO_CATEGORY: &str = "Página promocional";
pub const CREDITS_CATEGORY: &str = "Créditos";
pub const RECRUITMENT_CATEGORY: &str = "Reclutamiento";
pub const AD_CATEGORY: &str = "Anuncio";
pub const CHAPTER_PAGE_CATEGORY: &str = "Página de capítulo";

/// Scanlation page-type categories in strict descending priority order:
/// Reclutamiento > Anuncio > Créditos > Índice de proyectos (Página promocional) > Portada de manga > Página de capítulo.
pub const SCANLATION_PAGE_CATEGORIES: &[&str] = &[
    RECRUITMENT_CATEGORY,
    AD_CATEGORY,
    CREDITS_CATEGORY,
    PROMO_CATEGORY,
    crate::vision::MANGA_COVER_CATEGORY,
    CHAPTER_PAGE_CATEGORY,
];

const FILENAME_WORK_CONFIDENCE: f64 = 0.85;

const PROMO_HEADER_KEYS: &[&str] = &[
    "proyectosactivos",
    "proyectosterminados",
    "nuestrosproyectos",
    "otrosproyectos",
    "currentprojects",
    "ongoingprojects",
    "completedprojects",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkAlias {
    pub alias: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkSummary {
    pub label: LabelId,
    pub name: String,
    pub aliases: usize,
    /// Non-rejected present images that MENTION this work (entity labels).
    pub images: usize,
    /// Non-rejected present images that BELONG to this work (`asset_work`).
    pub belongs_to: usize,
    /// Pending (`suggested`) present images that belong to this work.
    pub belongs_to_suggested: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkEntry {
    pub label: LabelId,
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkMatch {
    pub label: LabelId,
    pub confidence: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetWorkMatches {
    pub works: Vec<WorkMatch>,
    pub promo_confidence: Option<f64>,
    pub credits_confidence: Option<f64>,
    pub recruitment_confidence: Option<f64>,
    pub ad_confidence: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkProposal {
    pub title: String,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedWorkGroup {
    pub id: i64,
    pub folder: PathBuf,
    pub work: LabelId,
    pub work_name: String,
    pub chapter: String,
    pub page_count: usize,
    pub sample_entries: Vec<Entry>,
}

/// Work title, chapter (kept verbatim as text, e.g. `"23.00"`), and optional page number parsed
/// from a file stem containing an explicit chapter/episode marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedWorkFilename {
    pub title: String,
    pub chapter: String,
    pub page: Option<u32>,
}

/// Ownership relation (`belongs_to`) stored in `asset_work`: at most one work per asset that is
/// not rejected (rejected ones are kept so they are not suggested again),
/// separate from OCR/entity mentions in `asset_label`.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetWork {
    pub asset: AssetId,
    pub work: LabelId,
    pub work_name: String,
    pub chapter: String,
    pub page: Option<u32>,
    pub source: Source,
    pub confidence: f64,
    pub status: Status,
}

/// Normalizes text for work matching: runs `ocr_search_key` (NFKD, drop combining diacritics
/// `U+0300..=U+036F`, NFC, lowercase, remove whitespace) and keeps only alphanumeric characters.
/// Kana with dakuten, CJK ideographs and Hangul syllables are preserved.
pub fn work_match_key(text: &str) -> String {
    ocr_search_key(text)
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

#[derive(Debug, Clone)]
struct CompiledAlias {
    label: LabelId,
    key: String,
    chars: Vec<char>,
    digits: Vec<char>,
    max_err: usize,
}

#[derive(Debug, Clone, Copy)]
struct PieceRef {
    alias_idx: usize,
    piece_start: usize,
    piece_len: usize,
}

/// Precompiled index of works and aliases for fast matching across tens of thousands of assets.
#[derive(Debug, Clone)]
pub struct WorkMatcher {
    aliases: Vec<CompiledAlias>,
    /// Exact lookup for short aliases (`2..=5` chars).
    short_exact: HashMap<String, Vec<usize>>,
    /// Pigeonhole piece index keyed by the first 3 chars of each piece for aliases of len `>= 6`.
    piece_index: HashMap<[char; 3], Vec<PieceRef>>,
}

impl WorkMatcher {
    pub fn build(entries: &[WorkEntry]) -> Self {
        let mut aliases: Vec<CompiledAlias> = Vec::new();
        let mut seen: HashSet<(LabelId, String)> = HashSet::new();

        for entry in entries {
            let iter = std::iter::once(entry.name.as_str())
                .chain(entry.aliases.iter().map(String::as_str));
            for raw in iter {
                let key = work_match_key(raw);
                let chars: Vec<char> = key.chars().collect();
                let len = chars.len();
                if len < 2 {
                    continue;
                }
                if !seen.insert((entry.label, key.clone())) {
                    continue;
                }
                let max_err = if len >= 10 { len / 10 } else { 0 };
                let digits: Vec<char> = chars.iter().copied().filter(|c| c.is_numeric()).collect();
                aliases.push(CompiledAlias {
                    label: entry.label,
                    key,
                    chars,
                    digits,
                    max_err,
                });
            }
        }

        let mut short_exact: HashMap<String, Vec<usize>> = HashMap::new();
        let mut piece_index: HashMap<[char; 3], Vec<PieceRef>> = HashMap::new();

        for (alias_idx, alias) in aliases.iter().enumerate() {
            let len = alias.chars.len();
            if len <= 5 {
                short_exact
                    .entry(alias.key.clone())
                    .or_default()
                    .push(alias_idx);
            } else {
                // Split into k + 1 disjoint pieces (for len >= 6, each piece has >= 5 chars >= 3).
                // With at most k edit errors, at least one piece must appear verbatim in the match.
                let pieces = alias.max_err + 1;
                for p in 0..pieces {
                    let start = (p * len) / pieces;
                    let end = ((p + 1) * len) / pieces;
                    let piece_len = end - start;
                    debug_assert!(piece_len >= 3);
                    let trigram = [
                        alias.chars[start],
                        alias.chars[start + 1],
                        alias.chars[start + 2],
                    ];
                    piece_index.entry(trigram).or_default().push(PieceRef {
                        alias_idx,
                        piece_start: start,
                        piece_len,
                    });
                }
            }
        }

        Self {
            aliases,
            short_exact,
            piece_index,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.aliases.is_empty()
    }

    /// Matches all works and manga page-type signals (promo, credits, recruitment) against an
    /// asset's OCR lines.
    pub fn match_asset(&self, lines: &[OcrLine], is_screenshot: bool) -> AssetWorkMatches {
        let candidates = build_candidates(lines, is_screenshot);
        if candidates.is_empty() {
            return AssetWorkMatches {
                works: Vec::new(),
                promo_confidence: None,
                credits_confidence: None,
                recruitment_confidence: None,
                ad_confidence: None,
            };
        }

        let has_header = candidates
            .iter()
            .any(|c| PROMO_HEADER_KEYS.iter().any(|h| c.key.contains(h)));

        let mut raw_matches: Vec<RawMatch<'_>> = Vec::new();
        let mut checked_aliases: Vec<u32> = vec![0; self.aliases.len()];
        let mut stamp: u32 = 0;

        for cand in &candidates {
            let score = f64::from(cand.min_score.clamp(0.0, 1.0));

            // 1. Short aliases (2..=5 chars): whole-candidate exact match only.
            if cand.chars.len() <= 5
                && let Some(indices) = self.short_exact.get(&cand.key)
            {
                for &idx in indices {
                    let alias = &self.aliases[idx];
                    raw_matches.push(RawMatch {
                        label: alias.label,
                        key: &alias.key,
                        key_len: alias.chars.len(),
                        confidence: score,
                        lines: cand.lines,
                    });
                }
            }

            // 2. Longer aliases (>= 6 chars): pigeonhole piece prefilter -> exact or approximate.
            if cand.chars.len() < 3 || self.piece_index.is_empty() {
                continue;
            }
            stamp = stamp.wrapping_add(1);
            if stamp == 0 {
                checked_aliases.fill(0);
                stamp = 1;
            }

            for pos in 0..=(cand.chars.len() - 3) {
                let trigram = [cand.chars[pos], cand.chars[pos + 1], cand.chars[pos + 2]];
                let Some(refs) = self.piece_index.get(&trigram) else {
                    continue;
                };
                for &pref in refs {
                    if checked_aliases[pref.alias_idx] == stamp {
                        continue;
                    }
                    let alias = &self.aliases[pref.alias_idx];
                    if pos + pref.piece_len > cand.chars.len() {
                        continue;
                    }
                    // Verify the full pigeonhole piece matches at `pos`.
                    if cand.chars[pos..pos + pref.piece_len]
                        != alias.chars[pref.piece_start..pref.piece_start + pref.piece_len]
                    {
                        continue;
                    }
                    // Length and exact-digit prefilter before running substring or DP checks.
                    if cand.chars.len() + alias.max_err < alias.chars.len() {
                        continue;
                    }
                    if !alias.digits.is_empty()
                        && !contains_subsequence(&cand.digits, &alias.digits)
                    {
                        checked_aliases[pref.alias_idx] = stamp;
                        continue;
                    }

                    // Check exact substring first (fast path).
                    if exact_substring_match(&alias.chars, &cand.chars) {
                        checked_aliases[pref.alias_idx] = stamp;
                        raw_matches.push(RawMatch {
                            label: alias.label,
                            key: &alias.key,
                            key_len: alias.chars.len(),
                            confidence: score,
                            lines: cand.lines,
                        });
                        continue;
                    }

                    // Approximate match for len >= 10 within a bounded window around the piece hit.
                    if alias.max_err > 0 {
                        let k = alias.max_err;
                        let win_start = pos.saturating_sub(pref.piece_start + k);
                        let win_end =
                            (pos.saturating_sub(pref.piece_start) + alias.chars.len() + k)
                                .min(cand.chars.len());
                        let preceded_by_digit =
                            win_start > 0 && cand.chars[win_start - 1].is_numeric();
                        let followed_by_digit =
                            win_end < cand.chars.len() && cand.chars[win_end].is_numeric();
                        if let Some(errs) = min_substring_edit_distance(
                            &alias.chars,
                            &cand.chars[win_start..win_end],
                            k,
                            preceded_by_digit,
                            followed_by_digit,
                        ) {
                            // Keep checking later piece hits only if errs > 0 and we might find a better one?
                            // If errs == 1 and max_err == 1, this is already optimal among non-exact hits.
                            if errs == 1 || errs == 0 {
                                checked_aliases[pref.alias_idx] = stamp;
                            }
                            let len = alias.chars.len() as f64;
                            let conf = (score * (1.0 - errs as f64 / len)).clamp(0.0, 1.0);
                            raw_matches.push(RawMatch {
                                label: alias.label,
                                key: &alias.key,
                                key_len: alias.chars.len(),
                                confidence: conf,
                                lines: cand.lines,
                            });
                        }
                    }
                }
            }
        }

        // Subsumption: if one matched work's key is contained in another matched work's longer key
        // within the same lines, keep only the longer one.
        let mut best_per_work: HashMap<LabelId, (f64, usize)> = HashMap::new();
        for (i, m) in raw_matches.iter().enumerate() {
            let subsumed = raw_matches.iter().enumerate().any(|(j, other)| {
                i != j
                    && other.label != m.label
                    && other.key_len > m.key_len
                    && other.key.contains(m.key)
                    && m.lines.is_subset_of(&other.lines)
            });
            if subsumed {
                continue;
            }
            best_per_work
                .entry(m.label)
                .and_modify(|(best_conf, best_len)| {
                    if m.confidence > *best_conf
                        || ((m.confidence - *best_conf).abs() < f64::EPSILON
                            && m.key_len > *best_len)
                    {
                        *best_conf = m.confidence;
                        *best_len = m.key_len;
                    }
                })
                .or_insert((m.confidence, m.key_len));
        }

        let mut works: Vec<WorkMatch> = best_per_work
            .into_iter()
            .map(|(label, (confidence, _))| WorkMatch { label, confidence })
            .collect();
        works.sort_by(|a, b| {
            b.confidence
                .total_cmp(&a.confidence)
                .then_with(|| a.label.cmp(&b.label))
        });

        let has_many_works = works.len() >= 3;
        let promo_confidence = match (has_many_works, has_header) {
            (true, true) => Some(0.9),
            (true, false) => Some(0.8),
            (false, true) => Some(0.7),
            (false, false) => None,
        };

        let visible_lines: Vec<&str> = lines
            .iter()
            .filter(|l| !(is_screenshot && l.top_strip))
            .map(|l| l.text.as_str())
            .collect();
        let credits_confidence = detect_credits_confidence(&visible_lines);
        let recruitment_confidence = detect_recruitment_confidence(&visible_lines);
        let ad_confidence = if recruitment_confidence.is_some() {
            None
        } else {
            detect_ad_confidence(&visible_lines)
        };

        AssetWorkMatches {
            works,
            promo_confidence,
            credits_confidence,
            recruitment_confidence,
            ad_confidence,
        }
    }
}

/// Normalizes a line into lowercase accent-folded space-separated tokens, keeping colons attached
/// to detect role labels such as `"traduccion:"` or `"tl:"`.
fn normalize_phrase_line(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_space = true;
    for ch in raw.nfkd() {
        if ('\u{0300}'..='\u{036F}').contains(&ch) {
            continue;
        }
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if ch == ':' {
            if !prev_space {
                out.push(':');
            }
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Checks if `phrase` (space-separated tokens) occurs at word boundaries inside `padded`
/// (which has a leading and trailing space, and colons stripped).
fn contains_phrase(padded: &str, phrase: &str) -> bool {
    let needle = format!(" {phrase} ");
    padded.contains(&needle)
}

/// Detects scanlation credits pages from curated Spanish and English role lists.
/// Requires strong enough evidence that ordinary dialogue bubbles are not flagged.
fn detect_credits_confidence(lines: &[&str]) -> Option<f64> {
    if lines.is_empty() {
        return None;
    }

    const ROLE_GROUPS: &[(&[&str], &[&str])] = &[
        // (general role tokens/phrases, scanlation-specific tokens/phrases)
        (
            &[
                "traduccion",
                "traductor",
                "traductora",
                "traductores",
                "translation",
                "translator",
                "translators",
                "translated by",
            ],
            &["tl:"],
        ),
        (
            &["limpieza", "limpiador", "limpiadora", "cleaning"],
            &["cleaner", "cleaners", "cl:"],
        ),
        (
            &["tipografia", "maquetacion", "maquetador", "maquetadora"],
            &["typesetter", "typesetters", "typesetting", "typer", "ts:"],
        ),
        (
            &[],
            &[
                "redibujo",
                "redibujado",
                "redibujador",
                "redibujadora",
                "redraw",
                "redrawer",
                "redrawers",
                "redrawing",
                "rd:",
            ],
        ),
        (
            &["edicion", "editor", "editora", "editores", "edition"],
            &["ed:"],
        ),
        (
            &["correccion", "corrector", "correctora", "correctores"],
            &[
                "proofreader",
                "proofreaders",
                "proofreading",
                "quality check",
                "control de calidad",
                "pr:",
                "qc:",
            ],
        ),
        (&["escaneo", "scanner"], &["raw provider", "raws:"]),
    ];

    const CREDITS_HEADERS: &[&str] = &[
        "creditos",
        "credits",
        "staff del capitulo",
        "chapter staff",
        "scanlation staff",
        "equipo de traduccion",
        "gracias por leer",
        "thanks for reading",
        "thank you for reading",
        "fansub",
        "scanlation",
    ];

    let mut matched_groups: HashSet<usize> = HashSet::new();
    let mut has_scanlation_specific = false;
    let mut has_colon_role = false;
    let mut has_header = false;

    for &raw_line in lines {
        let norm_colon = normalize_phrase_line(raw_line);
        if norm_colon.is_empty() {
            continue;
        }
        let plain = norm_colon.replace(':', " ");
        let plain_collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        let padded = format!(" {plain_collapsed} ");
        let padded_colon = format!(" {norm_colon} ");

        if CREDITS_HEADERS.iter().any(|h| contains_phrase(&padded, h)) {
            has_header = true;
        }

        for (group_idx, &(general, specific)) in ROLE_GROUPS.iter().enumerate() {
            let mut hit = false;
            for &term in specific {
                if term.ends_with(':') {
                    if padded_colon.contains(&format!(" {term}")) {
                        hit = true;
                        has_scanlation_specific = true;
                        has_colon_role = true;
                    }
                } else if contains_phrase(&padded, term) {
                    hit = true;
                    has_scanlation_specific = true;
                    if padded_colon.contains(&format!(" {term}:")) {
                        has_colon_role = true;
                    }
                }
            }
            for &term in general {
                if contains_phrase(&padded, term) {
                    hit = true;
                    if padded_colon.contains(&format!(" {term}:")) {
                        has_colon_role = true;
                    }
                }
            }
            if hit {
                matched_groups.insert(group_idx);
            }
        }
    }

    let roles = matched_groups.len();
    if roles >= 3 {
        Some(0.90)
    } else if roles >= 2 && (has_header || has_colon_role || has_scanlation_specific) {
        Some(if has_header { 0.85 } else { 0.80 })
    } else {
        None
    }
}

/// Detects scanlation recruitment / staff-hiring pages from curated Spanish and English phrases.
/// Generic phrases like `"join us"` or `"únete a nosotros"` require supporting scanlation/role
/// context so ordinary manga dialogue is not flagged.
fn detect_recruitment_confidence(lines: &[&str]) -> Option<f64> {
    if lines.is_empty() {
        return None;
    }

    const STRONG_PHRASES: &[&str] = &[
        "reclutamiento",
        "estamos reclutando",
        "reclutamos",
        "buscamos staff",
        "se busca staff",
        "necesitamos staff",
        "unete al staff",
        "buscamos traductores",
        "buscamos traductor",
        "buscamos cleaners",
        "buscamos cleaner",
        "buscamos typesetters",
        "buscamos typesetter",
        "buscamos typers",
        "buscamos redibujadores",
        "buscamos redibujador",
        "buscamos correctores",
        "se necesitan traductores",
        "se necesitan cleaners",
        "se necesitan typesetters",
        "se necesitan editores",
        "hiring staff",
        "looking for staff",
        "looking for translators",
        "looking for cleaners",
        "looking for typesetters",
        "looking for redrawers",
        "looking for proofreaders",
        "need translators",
        "need cleaners",
        "need typesetters",
        "need redrawers",
        "apply on discord",
        "apply via discord",
    ];

    const GENERAL_PHRASES: &[&str] = &[
        "we are hiring",
        "we re hiring",
        "were hiring",
        "now hiring",
        "recruitment",
        "recruiting",
        "join our team",
        "join our staff",
        "join the team",
        "join us",
        "unete al equipo",
        "unete a nuestro equipo",
        "unete a nosotros",
        "buscamos personal",
        "buscamos editores",
        "vacantes abiertas",
        "sin experiencia necesaria",
        "no necesitas experiencia",
    ];

    const SUPPORTING_SIGNALS: &[&str] = &[
        "traductor",
        "traductora",
        "traductores",
        "traduccion",
        "translator",
        "translators",
        "cleaner",
        "cleaners",
        "limpieza",
        "typesetter",
        "typesetters",
        "typesetting",
        "typer",
        "typers",
        "redrawer",
        "redrawers",
        "redraw",
        "redibujo",
        "redibujador",
        "redibujadores",
        "proofreader",
        "proofreaders",
        "corrector",
        "correctores",
        "quality check",
        "editor",
        "editores",
        "staff",
        "discord",
        "fansub",
        "scanlation",
        "scans",
        "raws",
        "requisitos",
        "requirements",
        "experiencia",
        "experience",
    ];

    let mut has_strong = false;
    let mut has_general = false;
    let mut support_hits = 0usize;

    for &raw_line in lines {
        let norm_colon = normalize_phrase_line(raw_line);
        if norm_colon.is_empty() {
            continue;
        }
        let plain = norm_colon.replace(':', " ");
        let plain_collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        let padded = format!(" {plain_collapsed} ");

        if STRONG_PHRASES.iter().any(|p| contains_phrase(&padded, p)) {
            has_strong = true;
        }
        if GENERAL_PHRASES.iter().any(|p| contains_phrase(&padded, p)) {
            has_general = true;
        }
        for &sig in SUPPORTING_SIGNALS {
            if contains_phrase(&padded, sig) {
                support_hits += 1;
            }
        }
    }

    if has_strong && support_hits >= 1 {
        Some(0.90)
    } else if has_strong {
        Some(0.85)
    } else if has_general && support_hits >= 1 {
        Some(0.80)
    } else {
        None
    }
}

/// Detects scanlation advertisement / support pages from curated Spanish and English phrases.
/// Platforms like Discord, Patreon, or Ko-fi alone without calls to support/visit/follow do not
/// trigger an ad confidence, preventing ordinary dialogue bubbles mentioning Discord from being flagged.
/// Recruitment pages take precedence over ads.
pub(crate) fn detect_ad_confidence(lines: &[&str]) -> Option<f64> {
    if lines.is_empty() {
        return None;
    }

    const STRONG_AD_PHRASES: &[&str] = &[
        // Spanish call-to-actions / support / platforms
        "apoyanos en patreon",
        "apoyanos en ko fi",
        "apoyanos en kofi",
        "apoya nuestro trabajo",
        "apoya al scan",
        "apoya al fansub",
        "apoya el proyecto",
        "visita nuestra pagina",
        "visita nuestra web",
        "visita nuestro sitio",
        "visita nuestro discord",
        "siguenos en nuestras redes",
        "siguenos en redes",
        "siguenos en twitter",
        "siguenos en instagram",
        "siguenos en discord",
        "unete a nuestro discord",
        "unete al discord",
        "descarga nuestra app",
        "descarga la app",
        "descarga la aplicacion",
        "disponible en google play",
        "disponible en app store",
        "lee primero en nuestra web",
        "lee antes en nuestra web",
        "capitulos adelantados en",
        "capitulos anticipados",
        "haz tu donacion",
        "donaciones por paypal",
        "patreon com",
        "ko fi com",
        "discord gg",
        // English call-to-actions / support / platforms
        "support us on patreon",
        "support us on ko fi",
        "support us on kofi",
        "support our work",
        "support the scan",
        "support the scanlation",
        "support this project",
        "visit our website",
        "visit our web",
        "visit our page",
        "visit our site",
        "follow our socials",
        "follow our social media",
        "follow us on twitter",
        "follow us on instagram",
        "follow us on discord",
        "join our discord",
        "join our discord server",
        "download our app",
        "download the app",
        "get it on google play",
        "available on the app store",
        "read ahead on",
        "early access on",
        "read first on our website",
        "become a patron",
        "become a supporter",
        "buy us a coffee",
        "donate on paypal",
        "paypal me",
    ];

    const GENERAL_AD_PHRASES: &[&str] = &[
        // Spanish
        "apoyanos",
        "donaciones",
        "donacion",
        "donar",
        "visitanos",
        "visita el sitio",
        "siguenos",
        "siguenos para mas",
        "capitulos adelantados",
        "capitulos anticipados",
        "lee primero",
        "nuestra pagina",
        "nuestro sitio",
        // English
        "support us",
        "donate",
        "donations",
        "donation",
        "visit us",
        "follow us",
        "follow us for more",
        "early access",
        "read ahead",
        "read first",
        "our webpage",
    ];

    const SUPPORTING_AD_SIGNALS: &[&str] = &[
        "patreon",
        "ko fi",
        "kofi",
        "paypal",
        "discord",
        "fansub",
        "scanlation",
        "scans",
        "socials",
        "twitter",
        "instagram",
        "tiktok",
        "subscription",
        "suscripcion",
        "google play",
        "app store",
    ];

    let mut has_strong = false;
    let mut has_general = false;
    let mut support_hits = 0usize;

    for &raw_line in lines {
        let norm_colon = normalize_phrase_line(raw_line);
        if norm_colon.is_empty() {
            continue;
        }
        let plain = norm_colon.replace(':', " ");
        let plain_collapsed = plain.split_whitespace().collect::<Vec<_>>().join(" ");
        let padded = format!(" {plain_collapsed} ");

        if STRONG_AD_PHRASES
            .iter()
            .any(|p| contains_phrase(&padded, p))
        {
            has_strong = true;
        }
        if GENERAL_AD_PHRASES
            .iter()
            .any(|p| contains_phrase(&padded, p))
        {
            has_general = true;
        }
        for &sig in SUPPORTING_AD_SIGNALS {
            if contains_phrase(&padded, sig) {
                support_hits += 1;
            }
        }
    }

    if has_strong && support_hits >= 1 {
        Some(0.90)
    } else if has_strong {
        Some(0.85)
    } else if has_general && support_hits >= 1 {
        Some(0.80)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LineSet {
    ids: [usize; 3],
    len: u8,
}

impl LineSet {
    fn one(a: usize) -> Self {
        Self {
            ids: [a, 0, 0],
            len: 1,
        }
    }

    fn two(a: usize, b: usize) -> Self {
        Self {
            ids: [a, b, 0],
            len: 2,
        }
    }

    fn three(a: usize, b: usize, c: usize) -> Self {
        Self {
            ids: [a, b, c],
            len: 3,
        }
    }

    fn as_slice(&self) -> &[usize] {
        &self.ids[..usize::from(self.len)]
    }

    fn is_subset_of(&self, other: &Self) -> bool {
        let other_slice = other.as_slice();
        self.as_slice().iter().all(|id| other_slice.contains(id))
    }
}

struct Candidate {
    key: String,
    chars: Vec<char>,
    digits: Vec<char>,
    min_score: f32,
    lines: LineSet,
}

struct RawMatch<'a> {
    label: LabelId,
    key: &'a str,
    key_len: usize,
    confidence: f64,
    lines: LineSet,
}

struct ValidLine {
    key: String,
    score: f32,
    rect: [f32; 4],
}

fn build_candidates(lines: &[OcrLine], is_screenshot: bool) -> Vec<Candidate> {
    let valid: Vec<ValidLine> = lines
        .iter()
        .filter(|l| !(is_screenshot && l.top_strip))
        .filter_map(|l| {
            let key = work_match_key(&l.text);
            if key.is_empty() {
                None
            } else {
                Some(ValidLine {
                    key,
                    score: l.score,
                    rect: l.rect,
                })
            }
        })
        .collect();

    if valid.is_empty() {
        return Vec::new();
    }

    // For each line, find the single line directly below it in the same column, independent of
    // stored line order.
    let mut next_below: Vec<Option<usize>> = vec![None; valid.len()];
    for (i, a) in valid.iter().enumerate() {
        let h_a = (a.rect[3] - a.rect[1]).max(1.0);
        let w_a = (a.rect[2] - a.rect[0]).max(1.0);
        let mut best: Option<(usize, f32, f32)> = None; // (idx, b.y0, overlap)

        for (j, b) in valid.iter().enumerate() {
            if i == j {
                continue;
            }
            let h_b = (b.rect[3] - b.rect[1]).max(1.0);
            let w_b = (b.rect[2] - b.rect[0]).max(1.0);

            // Must be strictly below `a`, never side-by-side on the same row.
            if b.rect[1] <= a.rect[1] + 0.25 * h_a {
                continue;
            }
            let gap = b.rect[1] - a.rect[3];
            if gap < -0.35 * h_a.min(h_b) || gap > 1.2 * h_a.max(h_b) {
                continue;
            }
            let overlap = a.rect[2].min(b.rect[2]) - a.rect[0].max(b.rect[0]);
            if overlap <= 0.0 || overlap < 0.25 * w_a.min(w_b) {
                continue;
            }

            let better = match best {
                None => true,
                Some((_, best_y0, best_ov)) => {
                    b.rect[1] < best_y0 - 0.5
                        || ((b.rect[1] - best_y0).abs() <= 0.5 && overlap > best_ov)
                }
            };
            if better {
                best = Some((j, b.rect[1], overlap));
            }
        }

        next_below[i] = best.map(|(j, _, _)| j);
    }

    let mut out = Vec::with_capacity(valid.len() * 3);
    for (i, a) in valid.iter().enumerate() {
        let chars1: Vec<char> = a.key.chars().collect();
        let digits1: Vec<char> = chars1.iter().copied().filter(|c| c.is_numeric()).collect();
        out.push(Candidate {
            chars: chars1,
            digits: digits1,
            key: a.key.clone(),
            min_score: a.score,
            lines: LineSet::one(i),
        });

        if let Some(j) = next_below[i] {
            let b = &valid[j];
            let mut key2 = String::with_capacity(a.key.len() + b.key.len());
            key2.push_str(&a.key);
            key2.push_str(&b.key);
            let score2 = a.score.min(b.score);
            let chars2: Vec<char> = key2.chars().collect();
            let digits2: Vec<char> = chars2.iter().copied().filter(|c| c.is_numeric()).collect();
            out.push(Candidate {
                chars: chars2,
                digits: digits2,
                key: key2.clone(),
                min_score: score2,
                lines: LineSet::two(i, j),
            });

            if let Some(k) = next_below[j]
                && k != i
            {
                let c = &valid[k];
                let mut key3 = String::with_capacity(key2.len() + c.key.len());
                key3.push_str(&key2);
                key3.push_str(&c.key);
                let score3 = score2.min(c.score);
                let chars3: Vec<char> = key3.chars().collect();
                let digits3: Vec<char> =
                    chars3.iter().copied().filter(|c| c.is_numeric()).collect();
                out.push(Candidate {
                    chars: chars3,
                    digits: digits3,
                    key: key3,
                    min_score: score3,
                    lines: LineSet::three(i, j, k),
                });
            }
        }
    }

    out
}

fn contains_subsequence(haystack: &[char], needle: &[char]) -> bool {
    let m = needle.len();
    if m == 0 {
        return true;
    }
    if m > haystack.len() {
        return false;
    }
    haystack.windows(m).any(|w| w == needle)
}

fn exact_substring_match(pattern: &[char], text: &[char]) -> bool {
    let m = pattern.len();
    let n = text.len();
    if m == 0 || m > n {
        return false;
    }
    let starts_digit = pattern[0].is_numeric();
    let ends_digit = pattern[m - 1].is_numeric();
    for start in 0..=(n - m) {
        if &text[start..start + m] != pattern {
            continue;
        }
        if starts_digit && start > 0 && text[start - 1].is_numeric() {
            continue;
        }
        if ends_digit && start + m < n && text[start + m].is_numeric() {
            continue;
        }
        return true;
    }
    false
}

/// Semi-global Levenshtein distance of `pattern` against any substring of `text`, capped at
/// `max_err`. Digits must match identically (no substituting, deleting, or inserting digits, and
/// no splitting an adjacent digit sequence at the boundaries).
fn min_substring_edit_distance(
    pattern: &[char],
    text: &[char],
    max_err: usize,
    preceded_by_digit: bool,
    followed_by_digit: bool,
) -> Option<usize> {
    let m = pattern.len();
    if m == 0 {
        return Some(0);
    }
    let inf = max_err + 1;
    let starts_digit = pattern[0].is_numeric();
    let ends_digit = pattern[m - 1].is_numeric();

    let mut prev = vec![inf; m + 1];
    let mut curr = vec![inf; m + 1];

    prev[0] = if starts_digit && preceded_by_digit {
        inf
    } else {
        0
    };
    for i in 1..=m {
        if pattern[i - 1].is_numeric() || prev[i - 1] >= inf {
            prev[i] = inf;
        } else {
            prev[i] = (prev[i - 1] + 1).min(inf);
        }
    }

    let mut best = inf;
    for (j, &tc) in text.iter().enumerate() {
        // Start of a new match right after text[j] (for the next column):
        let next_start_blocked = starts_digit && tc.is_numeric();
        curr[0] = if next_start_blocked { inf } else { 0 };

        for i in 1..=m {
            let pc = pattern[i - 1];
            let sub = if pc == tc {
                prev[i - 1]
            } else if pc.is_numeric() || tc.is_numeric() {
                inf
            } else {
                prev[i - 1].saturating_add(1)
            };
            let del = if pc.is_numeric() {
                inf
            } else {
                curr[i - 1].saturating_add(1)
            };
            let ins = if tc.is_numeric() {
                inf
            } else {
                prev[i].saturating_add(1)
            };
            curr[i] = sub.min(del).min(ins).min(inf);
        }

        let end_blocked = ends_digit
            && if j + 1 < text.len() {
                text[j + 1].is_numeric()
            } else {
                followed_by_digit
            };
        if !end_blocked && curr[m] < best {
            best = curr[m];
            if best == 0 {
                return Some(0);
            }
        }

        std::mem::swap(&mut prev, &mut curr);
    }

    (best <= max_err).then_some(best)
}

/// Parses a work title, chapter text (preserving formatting like `"23.00"`), and optional page
/// number from a file stem containing an explicit chapter/episode marker
/// (`Capítulo`, `Chapter`, `Cap.`, `Ch.`, `Episodio`, `Episode`, `Ep.` followed by a number,
/// accent- and case-insensitive). Returns `None` when no marker is present.
pub fn parse_work_from_stem(stem: &str) -> Option<ParsedWorkFilename> {
    let chars: Vec<(usize, char)> = stem.char_indices().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        // Start of a word at a non-alphanumeric boundary.
        let at_boundary = i == 0 || !chars[i - 1].1.is_alphanumeric();
        if at_boundary && chars[i].1.is_alphabetic() {
            let start_byte = chars[i].0;
            let mut j = i;
            while j < n && chars[j].1.is_alphabetic() {
                j += 1;
            }
            let end_byte = if j < n { chars[j].0 } else { stem.len() };
            let word = fold_ascii_word(&stem[start_byte..end_byte]);

            let is_marker_word = matches!(
                word.as_str(),
                "capitulo" | "chapter" | "cap" | "ch" | "episodio" | "episode" | "ep"
            );

            if is_marker_word && let Some(num_start) = find_chapter_number_start(&chars, j) {
                let raw_title = &stem[..start_byte];
                let cleaned = clean_extracted_title(raw_title);
                if work_match_key(&cleaned).chars().count() < 2 {
                    return None;
                }
                let (chapter, after_chapter) = extract_chapter_token(stem, &chars, num_start);
                let page = extract_page_number(stem, &chars, after_chapter);
                return Some(ParsedWorkFilename {
                    title: cleaned,
                    chapter,
                    page,
                });
            }

            i = j;
            continue;
        }
        i += 1;
    }
    None
}

/// Suggests that `asset` belongs to `work`. Rows the user decided on are never changed: nothing is
/// added over a confirmed work, and a rejected one stays rejected.
fn suggest_ownership(
    conn: &rusqlite::Connection,
    asset: AssetId,
    work: LabelId,
    parsed: &ParsedWorkFilename,
) -> Result<bool, CatalogError> {
    conn.prepare_cached(
        "DELETE FROM asset_work
         WHERE asset_id = ?1 AND work_id <> ?2 AND status = 'suggested'",
    )?
    .execute([asset.0, work.0])?;
    let changed = conn
        .prepare_cached(
            "INSERT INTO asset_work (asset_id, work_id, chapter, page, source, confidence, status)
             SELECT ?1, ?2, ?3, ?4, 'rules', ?5, 'suggested'
             WHERE NOT EXISTS (SELECT 1 FROM asset_work
                               WHERE asset_id = ?1 AND work_id <> ?2 AND status = 'accepted')
             ON CONFLICT(asset_id, work_id) DO UPDATE SET
                 chapter = excluded.chapter,
                 page = excluded.page,
                 source = excluded.source,
                 confidence = excluded.confidence
             WHERE asset_work.status = 'suggested'",
        )?
        .execute(params![
            asset.0,
            work.0,
            parsed.chapter,
            parsed.page.map(i64::from),
            FILENAME_WORK_CONFIDENCE
        ])?;
    Ok(changed > 0)
}

/// Returns `true` if `label` is one of the six scanlation page-type categories.
pub(crate) fn is_page_type_label(
    conn: &rusqlite::Connection,
    label: LabelId,
) -> Result<bool, CatalogError> {
    let is_page_type: bool = conn
        .prepare_cached(
            "SELECT EXISTS(
                 SELECT 1 FROM label
                 WHERE id = ?1
                   AND kind = 'category'
                   AND name IN (
                       'Reclutamiento',
                       'Anuncio',
                       'Créditos',
                       'Página promocional',
                       'Portada de manga',
                       'Página de capítulo'
                   )
             )",
        )?
        .query_row([label.0], |r| r.get(0))?;
    Ok(is_page_type)
}

/// Reconciles scanlation page-type suggestions for `asset` according to strict descending priority:
/// Reclutamiento > Anuncio > Créditos > Página promocional > Portada de manga > Página de capítulo.
/// Enforces at most one page-type suggestion per asset when multiple candidates exist.
/// Decisions made by the user (`accepted` or `rejected`) are NEVER overwritten.
pub(crate) fn reconcile_page_types_in_tx(
    conn: &rusqlite::Connection,
    asset: AssetId,
) -> Result<(), CatalogError> {
    let mut stmt = conn.prepare_cached(
        "SELECT al.label_id, l.name, al.status
         FROM asset_label al
         JOIN label l ON l.id = al.label_id
         WHERE al.asset_id = ?1
           AND l.kind = 'category'
           AND l.name IN (
               'Reclutamiento',
               'Anuncio',
               'Créditos',
               'Página promocional',
               'Portada de manga',
               'Página de capítulo'
           )",
    )?;

    struct PageTypeRow {
        label_id: i64,
        name: String,
        status: String,
    }

    let rows: Vec<PageTypeRow> = stmt
        .query_map([asset.0], |r| {
            Ok(PageTypeRow {
                label_id: r.get(0)?,
                name: r.get(1)?,
                status: r.get(2)?,
            })
        })?
        .collect::<Result<_, _>>()?;

    let has_accepted = rows.iter().any(|r| r.status == "accepted");
    if has_accepted {
        conn.prepare_cached(
            "DELETE FROM asset_label
             WHERE asset_id = ?1
               AND status = 'suggested'
               AND label_id IN (
                   SELECT id FROM label
                   WHERE kind = 'category'
                     AND name IN (
                         'Reclutamiento',
                         'Anuncio',
                         'Créditos',
                         'Página promocional',
                         'Portada de manga',
                         'Página de capítulo'
                     )
               )",
        )?
        .execute([asset.0])?;
        return Ok(());
    }

    let suggested: Vec<&PageTypeRow> = rows.iter().filter(|r| r.status == "suggested").collect();
    if suggested.len() <= 1 {
        return Ok(());
    }

    let winner = suggested.into_iter().min_by_key(|r| {
        let lower = r.name.to_lowercase();
        SCANLATION_PAGE_CATEGORIES
            .iter()
            .position(|&c| c.to_lowercase() == lower)
            .unwrap_or(usize::MAX)
    });

    if let Some(win) = winner {
        conn.prepare_cached(
            "DELETE FROM asset_label
             WHERE asset_id = ?1
               AND status = 'suggested'
               AND label_id <> ?2
               AND label_id IN (
                   SELECT id FROM label
                   WHERE kind = 'category'
                     AND name IN (
                         'Reclutamiento',
                         'Anuncio',
                         'Créditos',
                         'Página promocional',
                         'Portada de manga',
                         'Página de capítulo'
                     )
               )",
        )?
        .execute(params![asset.0, win.label_id])?;
    }

    Ok(())
}

/// Suggests `Página de capítulo` from filename when a chapter marker is present and the page
/// is not promotional, credits, recruitment, advertisement, or cover.
pub(crate) fn suggest_chapter_page_in_tx(
    conn: &rusqlite::Connection,
    asset: AssetId,
    has_chapter: bool,
) -> Result<bool, CatalogError> {
    let chapter_label_id: Option<i64> = conn
        .prepare_cached("SELECT id FROM label WHERE kind = 'category' AND name = ?1")?
        .query_row([CHAPTER_PAGE_CATEGORY], |r| r.get(0))
        .optional()?;

    if !has_chapter {
        if let Some(id) = chapter_label_id {
            conn.prepare_cached(
                "DELETE FROM asset_label
                 WHERE asset_id = ?1
                   AND label_id = ?2
                   AND source = 'rules'
                   AND status = 'suggested'",
            )?
            .execute(params![asset.0, id])?;
        }
        return Ok(false);
    }

    if let Some(id) = chapter_label_id {
        let existing_status: Option<String> = conn
            .prepare_cached("SELECT status FROM asset_label WHERE asset_id = ?1 AND label_id = ?2")?
            .query_row(params![asset.0, id], |r| r.get(0))
            .optional()?;
        if matches!(existing_status.as_deref(), Some("accepted" | "rejected")) {
            return Ok(false);
        }
    }

    let has_higher_priority: bool = conn
        .prepare_cached(
            "SELECT EXISTS(
                 SELECT 1 FROM asset_label al
                 JOIN label l ON l.id = al.label_id
                 WHERE al.asset_id = ?1
                   AND l.kind = 'category'
                   AND l.name IN (
                       'Reclutamiento',
                       'Anuncio',
                       'Créditos',
                       'Página promocional',
                       'Portada de manga'
                   )
                   AND al.status IN ('accepted', 'suggested')
             )",
        )?
        .query_row([asset.0], |r| r.get(0))?;

    if has_higher_priority {
        if let Some(id) = chapter_label_id {
            conn.prepare_cached(
                "DELETE FROM asset_label
                 WHERE asset_id = ?1
                   AND label_id = ?2
                   AND source = 'rules'
                   AND status = 'suggested'",
            )?
            .execute(params![asset.0, id])?;
        }
        return Ok(false);
    }

    let id = match chapter_label_id {
        Some(id) => id,
        None => {
            conn.prepare_cached(
                "INSERT OR IGNORE INTO label (kind, name) VALUES ('category', ?1)",
            )?
            .execute([CHAPTER_PAGE_CATEGORY])?;
            conn.prepare_cached("SELECT id FROM label WHERE kind = 'category' AND name = ?1")?
                .query_row([CHAPTER_PAGE_CATEGORY], |r| r.get(0))?
        }
    };

    let changed = conn
        .prepare_cached(
            "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
             VALUES (?1, ?2, 'rules', ?3, 'suggested')
             ON CONFLICT(asset_id, label_id) DO UPDATE SET source = 'rules', confidence = ?3
             WHERE status = 'suggested'",
        )?
        .execute(params![asset.0, id, FILENAME_WORK_CONFIDENCE])?;

    Ok(changed > 0)
}

/// Extracts a candidate work title from a file stem containing an explicit chapter/episode marker
/// (`Capítulo`, `Chapter`, `Cap.`, `Ch.`, `Episodio`, `Episode`, `Ep.` followed by a number,
/// accent- and case-insensitive). Returns `None` when no marker is present.
pub fn extract_work_title_from_stem(stem: &str) -> Option<String> {
    parse_work_from_stem(stem).map(|p| p.title)
}

fn fold_ascii_word(word: &str) -> String {
    word.nfkd()
        .filter(|&c| !('\u{0300}'..='\u{036F}').contains(&c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn find_chapter_number_start(chars: &[(usize, char)], mut idx: usize) -> Option<usize> {
    let n = chars.len();
    if idx >= n {
        return None;
    }
    // Optional dot after abbreviated or full marker (`Cap. 12`, `Ch.12`).
    if chars[idx].1 == '.' {
        idx += 1;
    }
    while idx < n {
        let c = chars[idx].1;
        if c.is_whitespace()
            || matches!(c, '_' | '-' | '–' | '—' | '#' | ':' | 'n' | 'N' | '°' | 'º')
        {
            // Allow `Cap. Nº 12` / `Cap #12` / `Cap 12` / `Cap_12`, but not arbitrary letters.
            if matches!(c, 'n' | 'N') {
                if idx + 1 < n && matches!(chars[idx + 1].1, '°' | 'º' | '.') {
                    idx += 2;
                    continue;
                }
                return None;
            }
            idx += 1;
        } else {
            break;
        }
    }
    (idx < n && chars[idx].1.is_ascii_digit()).then_some(idx)
}

/// Extracts the chapter number text starting at `num_start` (e.g. `"23.00"`, `"4"`, `"04.5"`),
/// returning `(chapter_text, next_char_index)`.
fn extract_chapter_token(stem: &str, chars: &[(usize, char)], num_start: usize) -> (String, usize) {
    let n = chars.len();
    let mut k = num_start;
    while k < n && chars[k].1.is_ascii_digit() {
        k += 1;
    }
    if k + 1 < n && chars[k].1 == '.' && chars[k + 1].1.is_ascii_digit() {
        k += 1;
        while k < n && chars[k].1.is_ascii_digit() {
            k += 1;
        }
    }
    let start_byte = chars[num_start].0;
    let end_byte = if k < n { chars[k].0 } else { stem.len() };
    (stem[start_byte..end_byte].to_string(), k)
}

/// Extracts an optional page number following the chapter token, such as:
/// ` - 7`, ` - 01`, `_p05`, ` pag. 12`, ` Page 3`.
fn extract_page_number(stem: &str, chars: &[(usize, char)], mut idx: usize) -> Option<u32> {
    let n = chars.len();
    let mut had_sep = false;
    while idx < n {
        let c = chars[idx].1;
        if c.is_whitespace()
            || matches!(c, '-' | '–' | '—' | '_' | ':' | ',' | '|' | '/' | '(' | '[')
        {
            idx += 1;
            had_sep = true;
        } else {
            break;
        }
    }
    if !had_sep || idx >= n {
        return None;
    }

    // Optional explicit page marker word (`p`, `pg`, `pag`, `pagina`, `page`).
    if chars[idx].1.is_alphabetic() {
        let word_start = chars[idx].0;
        let mut w_end = idx;
        while w_end < n && chars[w_end].1.is_alphabetic() {
            w_end += 1;
        }
        let word_end_byte = if w_end < n {
            chars[w_end].0
        } else {
            stem.len()
        };
        let word = fold_ascii_word(&stem[word_start..word_end_byte]);
        if !matches!(word.as_str(), "p" | "pg" | "pag" | "pagina" | "page") {
            return None;
        }
        idx = w_end;
        while idx < n
            && (chars[idx].1.is_whitespace() || matches!(chars[idx].1, '.' | '-' | '_' | ':' | '#'))
        {
            idx += 1;
        }
        if idx >= n || !chars[idx].1.is_ascii_digit() {
            return None;
        }
    }

    if !chars[idx].1.is_ascii_digit() {
        return None;
    }
    let d_start = chars[idx].0;
    let mut d_end = idx;
    while d_end < n && chars[d_end].1.is_ascii_digit() {
        d_end += 1;
    }
    // Make sure digits are not glued to letters or decimal point (e.g. `1080p` or `2.5`).
    if d_end < n && (chars[d_end].1.is_alphabetic() || chars[d_end].1 == '.') {
        return None;
    }
    let d_end_byte = if d_end < n {
        chars[d_end].0
    } else {
        stem.len()
    };
    stem[d_start..d_end_byte].parse::<u32>().ok()
}

fn clean_extracted_title(raw: &str) -> String {
    raw.trim()
        .trim_end_matches(|c: char| {
            c.is_whitespace()
                || matches!(c, '-' | '–' | '—' | '_' | ':' | '|' | ',' | '.' | '(' | '[')
        })
        .trim()
        .to_string()
}

/// Extracts a chapter number token from a folder name containing an explicit chapter/episode marker
/// (`Capítulo`, `Chapter`, `Cap.`, `Ch.`, `Episodio`, `Episode`, `Ep.` followed by a number,
/// accent- and case-insensitive). Returns `None` when no marker is present.
pub fn extract_chapter_from_folder_name(folder_name: &str) -> Option<String> {
    let chars: Vec<(usize, char)> = folder_name.char_indices().collect();
    let n = chars.len();
    let mut i = 0;

    while i < n {
        let at_boundary = i == 0 || !chars[i - 1].1.is_alphanumeric();
        if at_boundary && chars[i].1.is_alphabetic() {
            let start_byte = chars[i].0;
            let mut j = i;
            while j < n && chars[j].1.is_alphabetic() {
                j += 1;
            }
            let end_byte = if j < n { chars[j].0 } else { folder_name.len() };
            let word = fold_ascii_word(&folder_name[start_byte..end_byte]);

            let is_marker_word = matches!(
                word.as_str(),
                "capitulo" | "chapter" | "cap" | "ch" | "episodio" | "episode" | "ep"
            );

            if is_marker_word && let Some(num_start) = find_chapter_number_start(&chars, j) {
                let (chapter, _) = extract_chapter_token(folder_name, &chars, num_start);
                return Some(chapter);
            }

            i = j;
            continue;
        }
        i += 1;
    }
    None
}

/// Extracts a page number from a filename stem (e.g. `001.jpg` -> 1, `p_12.png` -> 12, `IMG_2033.png` -> 2033).
pub fn extract_page_from_stem(stem: &str) -> Option<u32> {
    if let Some(parsed) = parse_work_from_stem(stem)
        && let Some(p) = parsed.page
    {
        return Some(p);
    }

    let chars: Vec<(usize, char)> = stem.char_indices().collect();
    let n = chars.len();

    // 1. Check for explicit page markers: p, pg, pag, pagina, page followed by digits.
    let mut i = 0;
    while i < n {
        let at_boundary = i == 0 || !chars[i - 1].1.is_alphanumeric();
        if at_boundary && chars[i].1.is_alphabetic() {
            let start_byte = chars[i].0;
            let mut j = i;
            while j < n && chars[j].1.is_alphabetic() {
                j += 1;
            }
            let end_byte = if j < n { chars[j].0 } else { stem.len() };
            let word = fold_ascii_word(&stem[start_byte..end_byte]);
            if matches!(word.as_str(), "p" | "pg" | "pag" | "pagina" | "page") {
                let mut k = j;
                while k < n
                    && (chars[k].1.is_whitespace()
                        || matches!(chars[k].1, '.' | '-' | '_' | ':' | '#'))
                {
                    k += 1;
                }
                if k < n && chars[k].1.is_ascii_digit() {
                    let d_start = chars[k].0;
                    let mut d_end = k;
                    while d_end < n && chars[d_end].1.is_ascii_digit() {
                        d_end += 1;
                    }
                    let d_end_byte = if d_end < n {
                        chars[d_end].0
                    } else {
                        stem.len()
                    };
                    if let Ok(num) = stem[d_start..d_end_byte].parse::<u32>() {
                        return Some(num);
                    }
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }

    // 2. If the entire trimmed stem is digits (e.g. "001"), parse it.
    let trimmed = stem.trim();
    if !trimmed.is_empty() && trimmed.chars().all(|c| c.is_ascii_digit()) {
        return trimmed.parse::<u32>().ok();
    }

    // 3. Find trailing sequence of digits (e.g. "IMG_2033", "scan_004", "05").
    let mut end = n;
    while end > 0 && !chars[end - 1].1.is_ascii_digit() {
        end -= 1;
    }
    if end > 0 {
        let mut start = end;
        while start > 0 && chars[start - 1].1.is_ascii_digit() {
            start -= 1;
        }
        let start_byte = chars[start].0;
        let end_byte = if end < n { chars[end].0 } else { stem.len() };
        if let Ok(num) = stem[start_byte..end_byte].parse::<u32>() {
            return Some(num);
        }
    }

    None
}

impl Catalog {
    /// Creates or retrieves a work (`label` with `kind = 'entity'`).
    pub fn create_work(&self, name: &str) -> Result<LabelId, CatalogError> {
        let trimmed = name.trim();
        self.label(LabelKind::Entity, trimmed)
    }

    /// Removes a work the user no longer wants: its aliases, its ownership (`asset_work`) rows,
    /// and every image assignment, including accepted and rejected ones. Only catalog rows change.
    /// Returns `false` if `work` is not a work. Never called automatically.
    pub fn delete_work(&mut self, work: LabelId) -> Result<bool, CatalogError> {
        const IS_WORK: &str = "SELECT id FROM label WHERE id = ?1 AND kind = 'entity'";
        let tx = self.conn.transaction()?;
        tx.execute(
            &format!("DELETE FROM asset_work WHERE work_id IN ({IS_WORK})"),
            [work.0],
        )?;
        tx.execute(
            &format!("DELETE FROM proposed_work_group WHERE work_id IN ({IS_WORK})"),
            [work.0],
        )?;
        for table in ["asset_label", "work_alias"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE label_id IN ({IS_WORK})"),
                [work.0],
            )?;
        }
        let removed = tx.execute(
            "DELETE FROM label WHERE id = ?1 AND kind = 'entity'",
            [work.0],
        )?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Adds an alias to a work. Returns `true` if inserted, `false` if empty or already present.
    pub fn add_work_alias(&self, work: LabelId, alias: &str) -> Result<bool, CatalogError> {
        let trimmed = alias.trim();
        let key = work_match_key(trimmed);
        if trimmed.is_empty() || key.chars().count() < 2 {
            return Ok(false);
        }
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO work_alias (label_id, alias, key) VALUES (?1, ?2, ?3)",
            params![work.0, trimmed, key],
        )?;
        Ok(changed > 0)
    }

    /// Removes an alias from a work by its normalized `key` (or exact alias text).
    pub fn remove_work_alias(
        &self,
        work: LabelId,
        alias_or_key: &str,
    ) -> Result<bool, CatalogError> {
        let key = work_match_key(alias_or_key);
        let changed = self.conn.execute(
            "DELETE FROM work_alias WHERE label_id = ?1 AND (key = ?2 OR alias = ?3)",
            params![work.0, key, alias_or_key],
        )?;
        Ok(changed > 0)
    }

    /// Lists explicit aliases stored for a work (its own name is always an implicit alias).
    pub fn work_aliases(&self, work: LabelId) -> Result<Vec<WorkAlias>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT alias, key FROM work_alias WHERE label_id = ?1 ORDER BY alias COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([work.0], |r| {
            Ok(WorkAlias {
                alias: r.get(0)?,
                key: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Lists all works (`kind = 'entity'`) with their alias count, non-rejected mention count
    /// (`images`), and ownership counts (`belongs_to`, `belongs_to_suggested`) restricted to `roots`.
    pub fn list_works_in_roots(
        &self,
        roots: &[std::path::PathBuf],
    ) -> Result<Vec<WorkSummary>, CatalogError> {
        let mut stmt = if roots.is_empty() {
            self.conn.prepare(
                "SELECT lb.id, lb.name,
                        (SELECT COUNT(*) FROM work_alias wa WHERE wa.label_id = lb.id) AS aliases,
                        0 AS images,
                        0 AS belongs_to,
                        0 AS belongs_to_suggested
                 FROM label lb
                 WHERE lb.kind = 'entity'
                 ORDER BY lb.name COLLATE NOCASE",
            )?
        } else {
            let marks = vec!["?"; roots.len()].join(",");
            let sql = format!(
                "SELECT lb.id, lb.name,
                        (SELECT COUNT(*) FROM work_alias wa WHERE wa.label_id = lb.id) AS aliases,
                        (SELECT COUNT(*) FROM asset_label al
                         WHERE al.label_id = lb.id
                           AND al.status <> 'rejected'
                           AND EXISTS (SELECT 1 FROM location l
                                       WHERE l.asset_id = al.asset_id
                                         AND l.missing = 0
                                         AND l.root IN ({marks}))) AS images,
                        (SELECT COUNT(*) FROM asset_work aw
                         WHERE aw.work_id = lb.id
                           AND aw.status <> 'rejected'
                           AND EXISTS (SELECT 1 FROM location l
                                       WHERE l.asset_id = aw.asset_id
                                         AND l.missing = 0
                                         AND l.root IN ({marks}))) AS belongs_to,
                        (SELECT COUNT(*) FROM asset_work aw
                         WHERE aw.work_id = lb.id
                           AND aw.status = 'suggested'
                           AND EXISTS (SELECT 1 FROM location l
                                       WHERE l.asset_id = aw.asset_id
                                         AND l.missing = 0
                                         AND l.root IN ({marks}))) AS belongs_to_suggested
                 FROM label lb
                 WHERE lb.kind = 'entity'
                 ORDER BY lb.name COLLATE NOCASE"
            );
            self.conn.prepare(&sql)?
        };
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() * 3);
        if !roots.is_empty() {
            for _ in 0..3 {
                for r in roots {
                    args.push(Box::new(r.to_string_lossy().into_owned()));
                }
            }
        }
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), |r| {
            let aliases: i64 = r.get(2)?;
            let images: i64 = r.get(3)?;
            let belongs_to: i64 = r.get(4)?;
            let belongs_to_suggested: i64 = r.get(5)?;
            Ok(WorkSummary {
                label: LabelId(r.get(0)?),
                name: r.get(1)?,
                aliases: aliases.max(0) as usize,
                images: images.max(0) as usize,
                belongs_to: belongs_to.max(0) as usize,
                belongs_to_suggested: belongs_to_suggested.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Lists all works (`kind = 'entity'`) with their alias count, mention count, and ownership counts.
    pub fn list_works(&self) -> Result<Vec<WorkSummary>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT lb.id, lb.name,
                    (SELECT COUNT(*) FROM work_alias wa WHERE wa.label_id = lb.id) AS aliases,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = lb.id
                       AND al.status <> 'rejected'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0)) AS images,
                    (SELECT COUNT(*) FROM asset_work aw
                     WHERE aw.work_id = lb.id
                       AND aw.status <> 'rejected'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = aw.asset_id AND l.missing = 0)) AS belongs_to,
                    (SELECT COUNT(*) FROM asset_work aw
                     WHERE aw.work_id = lb.id
                       AND aw.status = 'suggested'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = aw.asset_id AND l.missing = 0)) AS belongs_to_suggested
             FROM label lb
             WHERE lb.kind = 'entity'
             ORDER BY lb.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            let aliases: i64 = r.get(2)?;
            let images: i64 = r.get(3)?;
            let belongs_to: i64 = r.get(4)?;
            let belongs_to_suggested: i64 = r.get(5)?;
            Ok(WorkSummary {
                label: LabelId(r.get(0)?),
                name: r.get(1)?,
                aliases: aliases.max(0) as usize,
                images: images.max(0) as usize,
                belongs_to: belongs_to.max(0) as usize,
                belongs_to_suggested: belongs_to_suggested.max(0) as usize,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Loads all works and their aliases and compiles a [`WorkMatcher`].
    pub fn load_work_matcher(&self) -> Result<WorkMatcher, CatalogError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM label WHERE kind = 'entity' ORDER BY id")?;
        let mut entries: Vec<WorkEntry> = stmt
            .query_map([], |r| {
                Ok(WorkEntry {
                    label: LabelId(r.get(0)?),
                    name: r.get(1)?,
                    aliases: Vec::new(),
                })
            })?
            .collect::<Result<_, _>>()?;

        let index_by_id: HashMap<LabelId, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.label, i))
            .collect();

        let mut alias_stmt = self
            .conn
            .prepare("SELECT label_id, alias FROM work_alias ORDER BY label_id, alias")?;
        let alias_rows =
            alias_stmt.query_map([], |r| Ok((LabelId(r.get(0)?), r.get::<_, String>(1)?)))?;
        for row in alias_rows {
            let (label, alias) = row?;
            if let Some(&idx) = index_by_id.get(&label) {
                entries[idx].aliases.push(alias);
            }
        }

        Ok(WorkMatcher::build(&entries))
    }

    /// Returns all present assets that have completed OCR.
    pub fn assets_with_ocr(&self) -> Result<Vec<AssetId>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT a.asset_id
             FROM analysis a
             WHERE a.stage = ?1
               AND a.version >= ?2
               AND EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = a.asset_id AND l.missing = 0)
             ORDER BY a.asset_id",
        )?;
        let rows = stmt.query_map(params![OCR_STAGE, i64::from(OCR_VERSION)], |r| {
            Ok(AssetId(r.get(0)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Matches works and promo-page status for one asset using its stored OCR lines, and applies
    /// the resulting `source = 'ocr', status = 'suggested'` rows idempotently in a transaction.
    /// Accepted, rejected, and non-OCR rows are never touched.
    pub fn apply_work_matches(
        &mut self,
        asset: AssetId,
        matcher: &WorkMatcher,
    ) -> Result<AssetWorkMatches, CatalogError> {
        let lines = self.ocr_lines(asset)?;
        let is_screenshot: bool = self.conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM asset_label al
                 JOIN label lb ON lb.id = al.label_id
                 WHERE al.asset_id = ?1
                   AND al.status <> 'rejected'
                   AND lb.kind = 'category'
                   AND lb.name = ?2
             )",
            params![asset.0, SCREENSHOT_CATEGORY],
            |r| Ok(r.get::<_, i64>(0)? != 0),
        )?;

        let result = matcher.match_asset(&lines, is_screenshot);

        let tx = self.conn.transaction()?;

        // 1. Remove previous OCR-suggested entity rows for this asset.
        tx.execute(
            "DELETE FROM asset_label
             WHERE asset_id = ?1
               AND source = 'ocr'
               AND status = 'suggested'
               AND label_id IN (SELECT id FROM label WHERE kind = 'entity')",
            [asset.0],
        )?;

        // 2. Insert/update current work suggestions (never touching accepted/rejected).
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
                 VALUES (?1, ?2, 'ocr', ?3, 'suggested')
                 ON CONFLICT(asset_id, label_id) DO UPDATE SET source = 'ocr', confidence = ?3
                 WHERE status = 'suggested'",
            )?;
            for m in &result.works {
                stmt.execute(params![asset.0, m.label.0, m.confidence.clamp(0.0, 1.0)])?;
            }
        }

        // 3. Synchronize OCR-suggested manga page-type categories.
        // Upsert every detected OCR candidate (leaving accepted/rejected rows untouched) and
        // remove stale OCR suggestions for undetected categories; reconcile_page_types_in_tx
        // then keeps only the highest-priority non-rejected suggestion.
        let ocr_candidates: [(&str, Option<f64>); 4] = [
            (RECRUITMENT_CATEGORY, result.recruitment_confidence),
            (AD_CATEGORY, result.ad_confidence),
            (CREDITS_CATEGORY, result.credits_confidence),
            (PROMO_CATEGORY, result.promo_confidence),
        ];

        for (cat_name, conf_opt) in ocr_candidates {
            let label_id: Option<i64> = tx
                .prepare_cached("SELECT id FROM label WHERE kind = 'category' AND name = ?1")?
                .query_row([cat_name], |r| r.get(0))
                .optional()?;

            match conf_opt {
                Some(conf) => {
                    let id = match label_id {
                        Some(id) => id,
                        None => {
                            tx.prepare_cached(
                                "INSERT OR IGNORE INTO label (kind, name) VALUES ('category', ?1)",
                            )?
                            .execute([cat_name])?;
                            tx.prepare_cached(
                                "SELECT id FROM label WHERE kind = 'category' AND name = ?1",
                            )?
                            .query_row([cat_name], |r| r.get(0))?
                        }
                    };
                    tx.prepare_cached(
                        "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
                         VALUES (?1, ?2, 'ocr', ?3, 'suggested')
                         ON CONFLICT(asset_id, label_id) DO UPDATE SET source = 'ocr', confidence = ?3
                         WHERE status = 'suggested'",
                    )?
                    .execute(params![asset.0, id, conf.clamp(0.0, 1.0)])?;
                }
                None => {
                    if let Some(id) = label_id {
                        tx.prepare_cached(
                            "DELETE FROM asset_label
                             WHERE asset_id = ?1
                               AND label_id = ?2
                               AND source = 'ocr'
                               AND status = 'suggested'",
                        )?
                        .execute(params![asset.0, id])?;
                    }
                }
            }
        }

        reconcile_page_types_in_tx(&tx, asset)?;

        tx.commit()?;
        Ok(result)
    }

    /// Builds a lookup map of normalized work keys (canonical names and aliases) to `LabelId`.
    /// Canonical work names take precedence on key collisions.
    pub fn load_work_key_map(&self) -> Result<HashMap<String, LabelId>, CatalogError> {
        let mut map: HashMap<String, LabelId> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT id, name FROM label WHERE kind = 'entity' ORDER BY id")?;
            let rows = stmt.query_map([], |r| Ok((LabelId(r.get(0)?), r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, name) = row?;
                let key = work_match_key(&name);
                if key.chars().count() >= 2 {
                    map.entry(key).or_insert(id);
                }
            }
        }
        {
            let mut stmt = self
                .conn
                .prepare("SELECT label_id, key FROM work_alias ORDER BY label_id, key")?;
            let rows = stmt.query_map([], |r| Ok((LabelId(r.get(0)?), r.get::<_, String>(1)?)))?;
            for row in rows {
                let (id, key) = row?;
                if key.chars().count() >= 2 {
                    map.entry(key).or_insert(id);
                }
            }
        }
        Ok(map)
    }

    /// Suggests work ownership (`belongs_to`) for `asset` solely from its file name when the
    /// parsed title matches an existing work or alias. Never creates works automatically and
    /// never overwrites `accepted` or `rejected` rows.
    pub fn suggest_work_from_path(
        &self,
        asset: AssetId,
        path: &Path,
    ) -> Result<bool, CatalogError> {
        // The rules stage calls this for every photo; most names have no chapter marker.
        if path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(parse_work_from_stem)
            .is_none()
        {
            return Ok(false);
        }
        let keys = self.load_work_key_map()?;
        self.suggest_work_from_path_with_map(asset, path, &keys)
    }

    /// Like [`suggest_work_from_path`](Self::suggest_work_from_path), reusing a preloaded key map.
    pub fn suggest_work_from_path_with_map(
        &self,
        asset: AssetId,
        path: &Path,
        keys: &HashMap<String, LabelId>,
    ) -> Result<bool, CatalogError> {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            return Ok(false);
        };
        let Some(parsed) = parse_work_from_stem(stem) else {
            return Ok(false);
        };
        let key = work_match_key(&parsed.title);
        let Some(&work_id) = keys.get(&key) else {
            return Ok(false);
        };
        suggest_ownership(&self.conn, asset, work_id, &parsed)
    }

    /// Suggests `Página de capítulo` from filename if the stem contains a chapter marker and
    /// the page is not promotional, credits, recruitment, advertisement, or cover.
    pub fn suggest_chapter_page_from_path(
        &self,
        asset: AssetId,
        path: &Path,
    ) -> Result<bool, CatalogError> {
        // The rules stage calls this for every photo; avoid any DB work when there is no marker.
        let has_chapter = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(parse_work_from_stem)
            .is_some();
        if !has_chapter {
            return Ok(false);
        }
        suggest_chapter_page_in_tx(&self.conn, asset, true)
    }

    /// Reconciles scanlation page-type suggestions for `asset` so that at most one suggestion
    /// remains according to priority, preserving any user decisions.
    pub fn reconcile_page_types(&self, asset: AssetId) -> Result<(), CatalogError> {
        reconcile_page_types_in_tx(&self.conn, asset)
    }

    /// Scans present locations (optionally restricted to `roots`) and idempotently suggests
    /// filename-based work ownership and chapter-page status for every asset whose file name
    /// matches. Never creates works and never touches `accepted`/`rejected` rows.
    /// Returns `(scanned_assets, matched_assets)`.
    pub fn apply_filename_works(
        &mut self,
        roots: Option<&[PathBuf]>,
    ) -> Result<(usize, usize), CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok((0, 0));
        }
        let keys = self.load_work_key_map()?;
        let mut sql = String::from(
            "SELECT l.asset_id, l.path
             FROM location l
             WHERE l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        sql.push_str(" ORDER BY l.asset_id, l.path");

        let rows: Vec<(AssetId, PathBuf)> = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params_from_iter(args.iter()), |r| {
                Ok((AssetId(r.get(0)?), PathBuf::from(r.get::<_, String>(1)?)))
            })?;
            mapped.collect::<Result<_, _>>()?
        };

        let mut scanned = 0usize;
        let mut matched = 0usize;
        let tx = self.conn.transaction()?;
        {
            let mut del_stale = tx.prepare_cached(
                "DELETE FROM asset_work
                 WHERE asset_id = ?1 AND source = 'rules' AND status = 'suggested'",
            )?;
            // One asset can have several copies; any of their names may carry the work.
            for copies in rows.chunk_by(|a, b| a.0 == b.0) {
                let asset = copies[0].0;
                scanned += 1;
                let found = copies.iter().find_map(|(_, path)| {
                    let parsed = parse_work_from_stem(path.file_stem()?.to_str()?)?;
                    let work_id = *keys.get(&work_match_key(&parsed.title))?;
                    Some((work_id, parsed))
                });
                if let Some((work_id, parsed)) = found {
                    suggest_ownership(&tx, asset, work_id, &parsed)?;
                    matched += 1;
                } else {
                    del_stale.execute([asset.0])?;
                }

                let has_chapter = copies.iter().any(|(_, path)| {
                    path.file_stem()
                        .and_then(|s| s.to_str())
                        .and_then(parse_work_from_stem)
                        .is_some()
                });
                suggest_chapter_page_in_tx(&tx, asset, has_chapter)?;
            }
        }
        tx.commit()?;
        Ok((scanned, matched))
    }

    /// Returns the work ownership row (`belongs_to`) for `asset`, if any.
    pub fn asset_work(&self, asset: AssetId) -> Result<Option<AssetWork>, CatalogError> {
        Ok(self
            .conn
            .query_row(
                "SELECT aw.asset_id, aw.work_id, lb.name, aw.chapter, aw.page,
                        aw.source, aw.confidence, aw.status
                 FROM asset_work aw
                 JOIN label lb ON lb.id = aw.work_id
                 WHERE aw.asset_id = ?1
                 ORDER BY CASE aw.status WHEN 'accepted' THEN 0 WHEN 'suggested' THEN 1 ELSE 2 END,
                          aw.work_id DESC
                 LIMIT 1",
                [asset.0],
                |r| {
                    let source_s: String = r.get(5)?;
                    let status_s: String = r.get(7)?;
                    let page: Option<i64> = r.get(4)?;
                    Ok(AssetWork {
                        asset: AssetId(r.get(0)?),
                        work: LabelId(r.get(1)?),
                        work_name: r.get(2)?,
                        chapter: r.get(3)?,
                        page: page.and_then(|p| u32::try_from(p).ok()),
                        source: Source::parse(&source_s).ok_or_else(|| unexpected(5, &source_s))?,
                        confidence: r.get(6)?,
                        status: Status::parse(&status_s).ok_or_else(|| unexpected(7, &status_s))?,
                    })
                },
            )
            .optional()?)
    }

    /// Confirms the ownership that [`asset_work`](Self::asset_work) reports for `asset`.
    pub fn accept_asset_work(&self, asset: AssetId) -> Result<bool, CatalogError> {
        self.decide_asset_work(asset, "accepted")
    }

    /// Rejects the ownership that [`asset_work`](Self::asset_work) reports for `asset`. The
    /// rejection is kept per work, so another work can still be suggested later.
    pub fn reject_asset_work(&self, asset: AssetId) -> Result<bool, CatalogError> {
        self.decide_asset_work(asset, "rejected")
    }

    fn decide_asset_work(&self, asset: AssetId, status: &str) -> Result<bool, CatalogError> {
        let Some(shown) = self.asset_work(asset)? else {
            return Ok(false);
        };
        let changed = self.conn.execute(
            "UPDATE asset_work SET status = ?3 WHERE asset_id = ?1 AND work_id = ?2",
            params![asset.0, shown.work.0, status],
        )?;
        Ok(changed > 0)
    }

    /// Confirms (`accepted`) all pending (`suggested`) ownership rows belonging to `work`,
    /// optionally restricted to present assets under `roots`.
    pub fn accept_all_suggested_work_ownership(
        &self,
        work: LabelId,
        roots: Option<&[PathBuf]>,
    ) -> Result<usize, CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok(0);
        }
        let mut sql = String::from(
            "UPDATE asset_work
             SET status = 'accepted'
             WHERE work_id = ?
               AND status = 'suggested'
               AND EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = asset_work.asset_id
                             AND l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        args.push(Box::new(work.0));
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        sql.push(')');
        let changed = self.conn.execute(&sql, params_from_iter(args.iter()))?;
        Ok(changed)
    }

    /// Lists present entries that belong to `work` (excluding `rejected`), ordered naturally by
    /// chapter, then page number, then path.
    pub fn entries_belonging_to_work(
        &self,
        work: LabelId,
        roots: Option<&[PathBuf]>,
    ) -> Result<Vec<Entry>, CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok(Vec::new());
        }
        let mut sql = String::from(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns,
                    a.media_type, a.duration_ms, aw.chapter, aw.page
             FROM asset_work aw
             JOIN asset a ON a.id = aw.asset_id
             JOIN location l ON l.asset_id = aw.asset_id
             WHERE aw.work_id = ?
               AND aw.status <> 'rejected'
               AND l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        args.push(Box::new(work.0));
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        sql.push_str(" ORDER BY l.path ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let entry = crate::browse::entry_from_row(r)?;
            let chapter: String = r.get(11)?;
            let page: Option<i64> = r.get(12)?;
            Ok((entry, chapter, page.and_then(|p| u32::try_from(p).ok())))
        })?;

        let mut items: Vec<(Entry, String, Option<u32>)> = rows.collect::<Result<_, _>>()?;
        items.sort_by(|(ea, ca, pa), (eb, cb, pb)| {
            let ch_a = ca.trim();
            let ch_b = cb.trim();
            let has_a = !ch_a.is_empty();
            let has_b = !ch_b.is_empty();

            match (has_a, has_b) {
                (true, true) => compare_chapters(ch_a, ch_b)
                    .then_with(|| match (pa, pb) {
                        (Some(a), Some(b)) => a.cmp(b),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    })
                    .then_with(|| ea.path.cmp(&eb.path)),
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                (false, false) => {
                    let parent_a = ea.path.parent();
                    let parent_b = eb.path.parent();
                    parent_a
                        .cmp(&parent_b)
                        .then_with(|| match (pa, pb) {
                            (Some(a), Some(b)) => a.cmp(b),
                            (Some(_), None) => std::cmp::Ordering::Less,
                            (None, Some(_)) => std::cmp::Ordering::Greater,
                            (None, None) => std::cmp::Ordering::Equal,
                        })
                        .then_with(|| ea.path.cmp(&eb.path))
                }
            }
        });
        Ok(items.into_iter().map(|(e, _, _)| e).collect())
    }

    /// Scans present file names for explicit chapter markers and proposes distinct work titles
    /// that are not already a work or alias. Never assigns any work to any image.
    pub fn propose_works_from_filenames(&self) -> Result<Vec<WorkProposal>, CatalogError> {
        let mut existing_keys: HashSet<String> = HashSet::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT name FROM label WHERE kind = 'entity'")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for name in rows {
                let k = work_match_key(&name?);
                if !k.is_empty() {
                    existing_keys.insert(k);
                }
            }
        }
        {
            let mut stmt = self.conn.prepare("SELECT key FROM work_alias")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for key in rows {
                existing_keys.insert(key?);
            }
        }

        // Map normalized key -> (preferred display title, file count).
        let mut grouped: HashMap<String, (String, usize)> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM location WHERE missing = 0 ORDER BY path")?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            for row in rows {
                let path_str = row?;
                let Some(stem) = Path::new(&path_str).file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let Some(title) = extract_work_title_from_stem(stem) else {
                    continue;
                };
                let key = work_match_key(&title);
                if existing_keys.contains(&key) {
                    continue;
                }
                grouped
                    .entry(key)
                    .and_modify(|(_, count)| *count += 1)
                    .or_insert((title, 1));
            }
        }

        let mut proposals: Vec<WorkProposal> = grouped
            .into_values()
            .map(|(title, files)| WorkProposal { title, files })
            .collect();
        proposals.sort_by(|a, b| {
            b.files
                .cmp(&a.files)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        Ok(proposals)
    }

    /// Scans library folders and proposes work ownership for images in folders where:
    /// - the majority of images with pertenencia point to the same work, or
    /// - the folder or parent folder name matches a work or alias.
    ///
    /// Never creates works, skips dismissed/accepted groups, and never touches accepted or rejected belongings.
    /// Returns the number of assets newly proposed.
    pub fn apply_folder_work_groups(
        &mut self,
        roots: Option<&[PathBuf]>,
    ) -> Result<usize, CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok(0);
        }
        let keys = self.load_work_key_map()?;
        if keys.is_empty() {
            return Ok(0);
        }

        let mut sql = String::from(
            "SELECT l.asset_id, l.path
             FROM location l
             WHERE l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        sql.push_str(" ORDER BY l.path ASC");

        let locations: Vec<(AssetId, PathBuf)> = {
            let mut stmt = self.conn.prepare(&sql)?;
            let mapped = stmt.query_map(params_from_iter(args.iter()), |r| {
                Ok((AssetId(r.get(0)?), PathBuf::from(r.get::<_, String>(1)?)))
            })?;
            mapped.collect::<Result<_, _>>()?
        };

        // Group by folder
        let mut folders: HashMap<PathBuf, Vec<(AssetId, PathBuf)>> = HashMap::new();
        for (asset_id, path) in locations {
            if let Some(parent) = path.parent() {
                folders
                    .entry(parent.to_path_buf())
                    .or_default()
                    .push((asset_id, path));
            }
        }

        // Load existing proposed_work_group
        let mut group_statuses: HashMap<(String, LabelId), String> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT folder, work_id, status FROM proposed_work_group")?;
            let rows = stmt.query_map([], |r| {
                Ok((
                    (r.get::<_, String>(0)?, LabelId(r.get::<_, i64>(1)?)),
                    r.get::<_, String>(2)?,
                ))
            })?;
            for r in rows {
                let (k, s) = r?;
                group_statuses.insert(k, s);
            }
        }

        let tx = self.conn.transaction()?;
        let mut total_proposed_assets = 0usize;

        for (folder, files) in &folders {
            let folder_str = folder.to_string_lossy().into_owned();

            // Load existing asset_work for assets in this folder
            let mut asset_works: HashMap<AssetId, Vec<(LabelId, String, Status)>> = HashMap::new();
            {
                let asset_ids: Vec<i64> = files.iter().map(|(id, _)| id.0).collect();
                for chunk in asset_ids.chunks(500) {
                    let marks = vec!["?"; chunk.len()].join(",");
                    let q = format!(
                        "SELECT asset_id, work_id, chapter, status FROM asset_work WHERE asset_id IN ({marks})"
                    );
                    let mut stmt = tx.prepare(&q)?;
                    let chunk_args: Vec<&dyn ToSql> =
                        chunk.iter().map(|id| id as &dyn ToSql).collect();
                    let rows = stmt.query_map(params_from_iter(chunk_args.iter()), |r| {
                        let status_s: String = r.get(3)?;
                        let status = Status::parse(&status_s).unwrap_or(Status::Suggested);
                        Ok((
                            AssetId(r.get(0)?),
                            LabelId(r.get(1)?),
                            r.get::<_, String>(2)?,
                            status,
                        ))
                    })?;
                    for row in rows {
                        let (aid, wid, ch, st) = row?;
                        asset_works.entry(aid).or_default().push((wid, ch, st));
                    }
                }
            }

            // Count existing non-rejected pertenencias in this folder per work
            let mut work_counts: HashMap<LabelId, usize> = HashMap::new();
            let mut total_with_belonging = 0usize;

            for (aid, _) in files {
                if let Some(list) = asset_works.get(aid)
                    && let Some((wid, _ch, _)) = list
                        .iter()
                        .find(|(_, _, st)| *st == Status::Accepted || *st == Status::Suggested)
                {
                    *work_counts.entry(*wid).or_default() += 1;
                    total_with_belonging += 1;
                }
            }

            // Determine candidate work:
            // 1. Majority of images with pertenencia
            let mut candidate_work: Option<LabelId> = None;
            if total_with_belonging > 0
                && let Some((&majority_work, &count)) = work_counts.iter().max_by_key(|(_, c)| *c)
                && count * 2 > total_with_belonging
            {
                candidate_work = Some(majority_work);
            }

            // 2. Folder name or parent folder name match
            let folder_name = folder.file_name().and_then(|s| s.to_str()).unwrap_or("");
            let parent_name = folder
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("");

            if candidate_work.is_none() {
                let k = work_match_key(folder_name);
                if let Some(&wid) = keys.get(&k) {
                    candidate_work = Some(wid);
                } else if let Some(title) = extract_work_title_from_stem(folder_name) {
                    let tk = work_match_key(&title);
                    if let Some(&wid) = keys.get(&tk) {
                        candidate_work = Some(wid);
                    }
                }
            }

            if candidate_work.is_none() {
                let pk = work_match_key(parent_name);
                if let Some(&wid) = keys.get(&pk) {
                    candidate_work = Some(wid);
                } else if let Some(title) = extract_work_title_from_stem(parent_name) {
                    let ptk = work_match_key(&title);
                    if let Some(&wid) = keys.get(&ptk) {
                        candidate_work = Some(wid);
                    }
                }
            }

            let Some(work_id) = candidate_work else {
                continue;
            };

            // Check if group is already dismissed or accepted
            if let Some(status) = group_statuses.get(&(folder_str.clone(), work_id))
                && (status == "dismissed" || status == "accepted")
            {
                continue;
            }

            // Extract chapter: from folder name, then parent folder name
            let chapter = extract_chapter_from_folder_name(folder_name)
                .or_else(|| extract_chapter_from_folder_name(parent_name))
                .unwrap_or_default();

            // Find assets without pertenencia or with suggested pertenencia
            let mut proposed_for_group = 0usize;
            for (asset_id, path) in files {
                let existing = asset_works.get(asset_id);
                // "Nunca crees obras por tu cuenta. Las propuestas solo tocan imágenes sin pertenencia o con pertenencia sugerida, nunca las aceptadas o rechazadas."
                let has_accepted =
                    existing.is_some_and(|l| l.iter().any(|(_, _, st)| *st == Status::Accepted));
                if has_accepted {
                    continue;
                }
                let is_rejected_for_work = existing.is_some_and(|l| {
                    l.iter()
                        .any(|(w, _, st)| *w == work_id && *st == Status::Rejected)
                });
                if is_rejected_for_work {
                    continue;
                }

                // Assets that already belong to this work (suggested or accepted) are not touched
                let already_belongs = existing.is_some_and(|l| {
                    l.iter().any(|(w, _, st)| {
                        *w == work_id && (*st == Status::Suggested || *st == Status::Accepted)
                    })
                });
                if already_belongs {
                    continue;
                }

                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                let page = extract_page_from_stem(stem);

                tx.prepare_cached(
                    "DELETE FROM asset_work WHERE asset_id = ?1 AND work_id <> ?2 AND status = 'suggested'",
                )?
                .execute(params![asset_id.0, work_id.0])?;

                tx.prepare_cached(
                    "INSERT INTO asset_work (asset_id, work_id, chapter, page, source, confidence, status)
                     VALUES (?1, ?2, ?3, ?4, 'rules', ?5, 'suggested')
                     ON CONFLICT(asset_id, work_id) DO UPDATE SET
                         chapter = excluded.chapter,
                         page = excluded.page,
                         source = excluded.source,
                         confidence = excluded.confidence
                     WHERE asset_work.status = 'suggested'",
                )?
                .execute(params![
                    asset_id.0,
                    work_id.0,
                    chapter,
                    page.map(i64::from),
                    FILENAME_WORK_CONFIDENCE
                ])?;

                suggest_chapter_page_in_tx(&tx, *asset_id, !chapter.is_empty())?;
                total_proposed_assets += 1;

                proposed_for_group += 1;
            }

            if proposed_for_group > 0 {
                let existing_for_work = files
                    .iter()
                    .filter(|(aid, _)| {
                        asset_works.get(aid).is_some_and(|l| {
                            l.iter().any(|(w, _, st)| {
                                *w == work_id
                                    && (*st == Status::Suggested || *st == Status::Accepted)
                            })
                        })
                    })
                    .count();
                let total_group_pages = existing_for_work + proposed_for_group;

                tx.prepare_cached(
                    "INSERT INTO proposed_work_group (folder, work_id, chapter, page_count, status)
                     VALUES (?1, ?2, ?3, ?4, 'suggested')
                     ON CONFLICT(folder, work_id) DO UPDATE SET
                         chapter = excluded.chapter,
                         page_count = excluded.page_count
                     WHERE status = 'suggested'",
                )?
                .execute(params![
                    folder_str,
                    work_id.0,
                    chapter,
                    total_group_pages as i64
                ])?;
            }
        }

        tx.commit()?;
        Ok(total_proposed_assets)
    }

    /// Lists proposed work groups with status 'suggested', optionally restricted to `roots`.
    pub fn list_proposed_work_groups(
        &self,
        roots: Option<&[PathBuf]>,
    ) -> Result<Vec<ProposedWorkGroup>, CatalogError> {
        let mut sql = String::from(
            "SELECT pwg.id, pwg.folder, pwg.work_id, lb.name, pwg.chapter, pwg.page_count
             FROM proposed_work_group pwg
             JOIN label lb ON lb.id = pwg.work_id
             WHERE pwg.status = 'suggested'",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(r) = roots {
            if r.is_empty() {
                return Ok(Vec::new());
            }
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(
                " AND EXISTS (
                     SELECT 1 FROM location l
                     WHERE l.missing = 0
                       AND l.root IN ({marks})
                       AND (l.path LIKE pwg.folder || '/%' OR l.path LIKE pwg.folder || '\\%')
                 )"
            ));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        sql.push_str(" ORDER BY pwg.folder COLLATE NOCASE ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let id: i64 = r.get(0)?;
            let folder_str: String = r.get(1)?;
            let work_id = LabelId(r.get(2)?);
            let work_name: String = r.get(3)?;
            let chapter: String = r.get(4)?;
            let page_count: i64 = r.get(5)?;
            Ok((
                id,
                folder_str,
                work_id,
                work_name,
                chapter,
                page_count.max(0) as usize,
            ))
        })?;

        let mut groups = Vec::new();
        for item in rows {
            let (id, folder_str, work, work_name, chapter, page_count) = item?;
            let folder = PathBuf::from(&folder_str);

            let sample_entries = {
                let mut sample_stmt = self.conn.prepare(
                    "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                            a.width, a.height, a.format, a.captured_at_ns,
                            a.media_type, a.duration_ms
                     FROM asset_work aw
                     JOIN asset a ON a.id = aw.asset_id
                     JOIN location l ON l.asset_id = aw.asset_id
                     WHERE aw.work_id = ?1
                       AND aw.status = 'suggested'
                       AND l.missing = 0
                     ORDER BY aw.page ASC, l.path ASC",
                )?;
                let sample_rows =
                    sample_stmt.query_map(params![work.0], crate::browse::entry_from_row)?;
                let mut samples = Vec::new();
                for entry_res in sample_rows {
                    let entry = entry_res?;
                    if entry.path.parent() == Some(&folder) {
                        samples.push(entry);
                        if samples.len() >= 6 {
                            break;
                        }
                    }
                }
                samples
            };

            groups.push(ProposedWorkGroup {
                id,
                folder,
                work,
                work_name,
                chapter,
                page_count,
                sample_entries,
            });
        }

        Ok(groups)
    }

    /// Accepts a proposed work group, confirming the ownership rows for all images in that folder.
    pub fn accept_proposed_work_group(
        &self,
        folder: &Path,
        work: LabelId,
    ) -> Result<usize, CatalogError> {
        let folder_str = folder.to_string_lossy().into_owned();
        let tx = self.conn.unchecked_transaction()?;

        tx.prepare_cached(
            "UPDATE proposed_work_group
             SET status = 'accepted'
             WHERE folder = ?1 AND work_id = ?2",
        )?
        .execute(params![folder_str, work.0])?;

        let matching_assets = {
            let mut stmt =
                tx.prepare("SELECT l.asset_id, l.path FROM location l WHERE l.missing = 0")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            mapped
                .filter_map(|r| r.ok())
                .filter(|(_, p)| Path::new(p).parent() == Some(folder))
                .map(|(aid, _)| aid)
                .collect::<Vec<i64>>()
        };

        let mut changed = 0usize;
        for chunk in matching_assets.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "UPDATE asset_work
                 SET status = 'accepted'
                 WHERE work_id = ?1
                   AND status = 'suggested'
                   AND asset_id IN ({marks})"
            );
            let mut stmt = tx.prepare(&sql)?;
            let mut args: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() + 1);
            args.push(&work.0);
            for id in chunk {
                args.push(id);
            }
            changed += stmt.execute(params_from_iter(args.iter()))?;
        }

        tx.commit()?;
        Ok(changed)
    }

    /// Discards a proposed work group, removing suggested belongings for that group and remembering
    /// the dismissal so it will never be proposed again.
    pub fn dismiss_proposed_work_group(
        &self,
        folder: &Path,
        work: LabelId,
    ) -> Result<usize, CatalogError> {
        let folder_str = folder.to_string_lossy().into_owned();
        let tx = self.conn.unchecked_transaction()?;

        tx.prepare_cached(
            "INSERT INTO proposed_work_group (folder, work_id, chapter, page_count, status)
             VALUES (?1, ?2, '', 0, 'dismissed')
             ON CONFLICT(folder, work_id) DO UPDATE SET status = 'dismissed'",
        )?
        .execute(params![folder_str, work.0])?;

        let matching_assets = {
            let mut stmt =
                tx.prepare("SELECT l.asset_id, l.path FROM location l WHERE l.missing = 0")?;
            let mapped =
                stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            mapped
                .filter_map(|r| r.ok())
                .filter(|(_, p)| Path::new(p).parent() == Some(folder))
                .map(|(aid, _)| aid)
                .collect::<Vec<i64>>()
        };

        let mut changed = 0usize;
        for chunk in matching_assets.chunks(500) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "DELETE FROM asset_work
                 WHERE work_id = ?1
                   AND status = 'suggested'
                   AND asset_id IN ({marks})"
            );
            let mut stmt = tx.prepare(&sql)?;
            let mut args: Vec<&dyn ToSql> = Vec::with_capacity(chunk.len() + 1);
            args.push(&work.0);
            for id in chunk {
                args.push(id);
            }
            changed += stmt.execute(params_from_iter(args.iter()))?;
        }

        tx.commit()?;
        Ok(changed)
    }
}

/// Natural numeric ordering for chapter strings (`"2"` < `"10"`, `"23.00"` < `"23.5"`),
/// falling back to lexicographical order for non-numeric or tied values.
fn compare_chapters(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.trim().parse::<f64>().ok(), b.trim().parse::<f64>().ok()) {
        (Some(na), Some(nb)) => na.total_cmp(&nb).then_with(|| a.cmp(b)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.cmp(b),
    }
}
