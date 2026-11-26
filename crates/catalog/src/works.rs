use crate::ocr::{OCR_STAGE, OCR_VERSION, OcrLine, SCREENSHOT_CATEGORY, ocr_search_key};
use crate::{AssetId, Catalog, CatalogError, LabelId, LabelKind};
use rusqlite::{OptionalExtension, params};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use unicode_normalization::UnicodeNormalization;

pub const PROMO_CATEGORY: &str = "Página promocional";

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
    pub images: usize,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkProposal {
    pub title: String,
    pub files: usize,
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

    /// Matches all works and promo-page signals against an asset's OCR lines.
    pub fn match_asset(&self, lines: &[OcrLine], is_screenshot: bool) -> AssetWorkMatches {
        let candidates = build_candidates(lines, is_screenshot);
        if candidates.is_empty() {
            return AssetWorkMatches {
                works: Vec::new(),
                promo_confidence: None,
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

        AssetWorkMatches {
            works,
            promo_confidence,
        }
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

/// Extracts a candidate work title from a file stem containing an explicit chapter/episode marker
/// (`Capítulo`, `Chapter`, `Cap.`, `Ch.`, `Episodio`, `Episode`, `Ep.` followed by a number,
/// accent- and case-insensitive). Returns `None` when no marker is present.
pub fn extract_work_title_from_stem(stem: &str) -> Option<String> {
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

            if is_marker_word && is_followed_by_number(&chars, j) {
                let raw_title = &stem[..start_byte];
                let cleaned = clean_extracted_title(raw_title);
                if work_match_key(&cleaned).chars().count() >= 2 {
                    return Some(cleaned);
                }
                return None;
            }

            i = j;
            continue;
        }
        i += 1;
    }
    None
}

fn fold_ascii_word(word: &str) -> String {
    word.nfkd()
        .filter(|&c| !('\u{0300}'..='\u{036F}').contains(&c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_followed_by_number(chars: &[(usize, char)], mut idx: usize) -> bool {
    let n = chars.len();
    if idx >= n {
        return false;
    }
    // Optional dot after abbreviated or full marker (`Cap. 12`, `Ch.12`).
    let had_dot = if chars[idx].1 == '.' {
        idx += 1;
        true
    } else {
        false
    };
    let mut had_sep = had_dot;
    while idx < n {
        let c = chars[idx].1;
        if c.is_whitespace()
            || matches!(c, '_' | '-' | '–' | '—' | '#' | ':' | 'n' | 'N' | '°' | 'º')
        {
            // Allow `Cap. Nº 12` / `Cap #12` / `Cap 12` / `Cap_12`, but not arbitrary letters.
            if matches!(c, 'n' | 'N') {
                if idx + 1 < n && matches!(chars[idx + 1].1, '°' | 'º' | '.') {
                    idx += 2;
                    had_sep = true;
                    continue;
                }
                return false;
            }
            idx += 1;
            had_sep = true;
        } else {
            break;
        }
    }
    had_sep && idx < n && chars[idx].1.is_ascii_digit()
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

impl Catalog {
    /// Creates or retrieves a work (`label` with `kind = 'entity'`).
    pub fn create_work(&self, name: &str) -> Result<LabelId, CatalogError> {
        let trimmed = name.trim();
        self.label(LabelKind::Entity, trimmed)
    }

    /// Removes a work the user no longer wants: its aliases and every image assignment, including
    /// accepted and rejected ones. Only catalog rows change. Returns `false` if `work` is not a
    /// work. Never called automatically.
    pub fn delete_work(&mut self, work: LabelId) -> Result<bool, CatalogError> {
        const IS_WORK: &str = "SELECT id FROM label WHERE id = ?1 AND kind = 'entity'";
        let tx = self.conn.transaction()?;
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

    /// Lists all works (`kind = 'entity'`) with their alias count and non-rejected present image count.
    pub fn list_works(&self) -> Result<Vec<WorkSummary>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT lb.id, lb.name,
                    (SELECT COUNT(*) FROM work_alias wa WHERE wa.label_id = lb.id) AS aliases,
                    (SELECT COUNT(*) FROM asset_label al
                     WHERE al.label_id = lb.id
                       AND al.status <> 'rejected'
                       AND EXISTS (SELECT 1 FROM location l
                                   WHERE l.asset_id = al.asset_id AND l.missing = 0)) AS images
             FROM label lb
             WHERE lb.kind = 'entity'
             ORDER BY lb.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            let aliases: i64 = r.get(2)?;
            let images: i64 = r.get(3)?;
            Ok(WorkSummary {
                label: LabelId(r.get(0)?),
                name: r.get(1)?,
                aliases: aliases.max(0) as usize,
                images: images.max(0) as usize,
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

        // 3. Remove previous OCR-suggested promo category row for this asset.
        let promo_label_id: Option<i64> = tx
            .query_row(
                "SELECT id FROM label WHERE kind = 'category' AND name = ?1",
                [PROMO_CATEGORY],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(promo_id) = promo_label_id {
            tx.execute(
                "DELETE FROM asset_label
                 WHERE asset_id = ?1
                   AND label_id = ?2
                   AND source = 'ocr'
                   AND status = 'suggested'",
                params![asset.0, promo_id],
            )?;
        }

        // 4. Suggest promo category if applicable.
        if let Some(conf) = result.promo_confidence {
            let promo_id = match promo_label_id {
                Some(id) => id,
                None => {
                    tx.execute(
                        "INSERT OR IGNORE INTO label (kind, name) VALUES ('category', ?1)",
                        [PROMO_CATEGORY],
                    )?;
                    tx.query_row(
                        "SELECT id FROM label WHERE kind = 'category' AND name = ?1",
                        [PROMO_CATEGORY],
                        |r| r.get(0),
                    )?
                }
            };
            tx.execute(
                "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
                 VALUES (?1, ?2, 'ocr', ?3, 'suggested')
                 ON CONFLICT(asset_id, label_id) DO UPDATE SET source = 'ocr', confidence = ?3
                 WHERE status = 'suggested'",
                params![asset.0, promo_id, conf.clamp(0.0, 1.0)],
            )?;
        }

        tx.commit()?;
        Ok(result)
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
}
