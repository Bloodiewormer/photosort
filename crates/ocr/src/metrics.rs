//! Pure accuracy metrics. Boxes are axis-aligned `[x0, y0, x1, y1]` in image pixels.

use unicode_normalization::UnicodeNormalization;

pub type Rect = [f32; 4];

/// NFKC, runs of whitespace collapsed to one space, trimmed.
pub fn normalize(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    nfkc.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = (diagonal + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diagonal = std::mem::replace(&mut row[j + 1], cost);
        }
    }
    row[b.len()]
}

/// `1 - distance / longer length`; two empty strings are identical.
pub fn similarity(a: &str, b: &str) -> f64 {
    let longer = a.chars().count().max(b.chars().count());
    if longer == 0 {
        1.0
    } else {
        1.0 - edit_distance(a, b) as f64 / longer as f64
    }
}

pub fn iou(a: Rect, b: Rect) -> f32 {
    let shared = intersection(a, b);
    let union = area(a) + area(b) - shared;
    if union > 0.0 { shared / union } else { 0.0 }
}

fn area(r: Rect) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn intersection(a: Rect, b: Rect) -> f32 {
    area([
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ])
}

/// One-to-one `(truth, prediction)` pairs, best overlap first, with IoU of at least `min_iou`.
pub fn match_boxes(truth: &[Rect], predicted: &[Rect], min_iou: f32) -> Vec<(usize, usize)> {
    let mut candidates = Vec::new();
    for (t, a) in truth.iter().enumerate() {
        for (p, b) in predicted.iter().enumerate() {
            let overlap = iou(*a, *b);
            if overlap >= min_iou {
                candidates.push((overlap, t, p));
            }
        }
    }
    candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (mut used_t, mut used_p) = (vec![false; truth.len()], vec![false; predicted.len()]);
    let mut pairs = Vec::new();
    for (_, t, p) in candidates {
        if !used_t[t] && !used_p[p] {
            used_t[t] = true;
            used_p[p] = true;
            pairs.push((t, p));
        }
    }
    pairs
}

/// For each truth line, the predictions whose centre falls inside it, in reading order (left to
/// right, or top to bottom for a tall line). A detector may split one line into several boxes.
/// The second value lists the predictions that belong to no line.
pub fn assign_to_lines(truth: &[Rect], predicted: &[Rect]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let mut lines = vec![Vec::new(); truth.len()];
    let mut stray = Vec::new();
    for (p, b) in predicted.iter().enumerate() {
        let (x, y) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
        // When captions touch, the line sharing the most area with the box wins.
        let owner = truth
            .iter()
            .enumerate()
            .filter(|(_, t)| x >= t[0] && x <= t[2] && y >= t[1] && y <= t[3])
            .max_by(|a, c| intersection(*a.1, *b).total_cmp(&intersection(*c.1, *b)));
        match owner {
            Some((t, _)) => lines[t].push(p),
            None => stray.push(p),
        }
    }
    for (t, line) in truth.iter().zip(&mut lines) {
        let axis = usize::from(t[3] - t[1] > t[2] - t[0]);
        line.sort_by(|&a, &b| predicted[a][axis].total_cmp(&predicted[b][axis]));
    }
    (lines, stray)
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Tally {
    pub edits: usize,
    pub chars: usize,
    pub exact: usize,
    pub lines: usize,
}

impl Tally {
    /// Adds one line. Both texts are normalised here; `fold_case` ignores letter case.
    pub fn add(&mut self, truth: &str, read: &str, fold_case: bool) {
        let (mut truth, mut read) = (normalize(truth), normalize(read));
        if fold_case {
            truth = truth.to_lowercase();
            read = read.to_lowercase();
        }
        self.edits += edit_distance(&truth, &read);
        self.chars += truth.chars().count();
        self.exact += usize::from(truth == read);
        self.lines += 1;
    }

    /// Character error rate; above 1 when much more is read than is there.
    pub fn cer(&self) -> f64 {
        ratio(self.edits, self.chars)
    }

    pub fn exact_rate(&self) -> f64 {
        ratio(self.exact, self.lines)
    }
}

fn ratio(part: usize, whole: usize) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 / whole as f64
    }
}

/// Nearest-rank percentile, `p` from 0 to 100; 0 for no values.
pub fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or_default()
}

/// A promo-page title is found when its lines, joined, are at least this similar to what was read.
pub const TITLE_SIMILARITY: f64 = 0.85;

pub fn title_found(truth_lines: &[&str], read_lines: &[&str]) -> bool {
    let joined = |lines: &[&str]| normalize(&lines.join(" ")).to_lowercase();
    similarity(&joined(truth_lines), &joined(read_lines)) >= TITLE_SIMILARITY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_applies_nfkc_and_collapses_whitespace() {
        assert_eq!(normalize("  ＡＢＣ　１２３\t\n x "), "ABC 123 x");
        assert_eq!(normalize("ｶﾞｲﾄﾞ"), "ガイド");
    }

    #[test]
    fn edit_distance_works_on_characters_not_bytes() {
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("한글", "한굴"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
        assert_eq!(edit_distance("abc", "abc"), 0);
    }

    #[test]
    fn similarity_is_relative_to_the_longer_text() {
        assert_eq!(similarity("", ""), 1.0);
        assert_eq!(similarity("abcd", "abcx"), 0.75);
        assert_eq!(similarity("ab", ""), 0.0);
    }

    #[test]
    fn iou_of_half_overlapping_squares_is_one_third() {
        let a = [0.0, 0.0, 10.0, 10.0];
        assert!((iou(a, [5.0, 0.0, 15.0, 10.0]) - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(iou(a, a), 1.0);
        assert_eq!(iou(a, [20.0, 20.0, 30.0, 30.0]), 0.0);
    }

    #[test]
    fn match_boxes_is_one_to_one_and_prefers_the_best_overlap() {
        let truth = [[0.0, 0.0, 10.0, 10.0]];
        let predicted = [[2.0, 0.0, 12.0, 10.0], [0.0, 0.0, 10.0, 10.0]];
        assert_eq!(match_boxes(&truth, &predicted, 0.5), vec![(0, 1)]);
        assert!(match_boxes(&truth, &[[9.0, 0.0, 19.0, 10.0]], 0.5).is_empty());
    }

    #[test]
    fn split_detections_are_joined_in_reading_order() {
        let truth = [[0.0, 0.0, 100.0, 20.0], [0.0, 40.0, 20.0, 140.0]];
        let predicted = [
            [60.0, 0.0, 98.0, 20.0],
            [2.0, 1.0, 50.0, 19.0],
            [300.0, 300.0, 320.0, 310.0],
            [1.0, 95.0, 19.0, 138.0],
            [1.0, 42.0, 19.0, 90.0],
        ];
        let (lines, stray) = assign_to_lines(&truth, &predicted);
        assert_eq!(lines, vec![vec![1, 0], vec![4, 3]]);
        assert_eq!(stray, vec![2]);
    }

    #[test]
    fn tally_accumulates_cer_and_exact_lines() {
        let mut sensitive = Tally::default();
        sensitive.add("Hola  mundo", "Hola mundo", false);
        sensitive.add("ABCD", "abcd", false);
        assert_eq!((sensitive.exact, sensitive.lines), (1, 2));
        assert_eq!((sensitive.edits, sensitive.chars), (4, 14));
        assert!((sensitive.cer() - 4.0 / 14.0).abs() < 1e-9);
        assert_eq!(sensitive.exact_rate(), 0.5);
        let mut folded = Tally::default();
        folded.add("ABCD", "abcd", true);
        assert_eq!((folded.edits, folded.exact), (0, 1));
        assert_eq!(Tally::default().cer(), 0.0);
        assert_eq!(Tally::default().exact_rate(), 0.0);
    }

    #[test]
    fn percentile_takes_the_nearest_rank() {
        let values = [4.0, 1.0, 3.0, 2.0, 5.0];
        assert_eq!(percentile(&values, 50.0), 3.0);
        assert_eq!(percentile(&values, 95.0), 5.0);
        assert_eq!(percentile(&values, 0.0), 1.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
    }

    #[test]
    fn a_title_is_found_from_its_joined_lines_ignoring_case() {
        assert!(title_found(
            &["EL JARDIN DE", "NIEBLA AZUL"],
            &["El jardin de", "niebla azul"]
        ));
        assert!(title_found(
            &["ABCDEFGHIJKLMNOPQRST"],
            &["ABCDEFGHIJKLMNOPQXXX"]
        ));
        assert!(!title_found(
            &["ABCDEFGHIJKLMNOPQRST"],
            &["ABCDEFGHIJKLMNOPXXXX"]
        ));
        assert!(!title_found(
            &["EL JARDIN DE", "NIEBLA AZUL"],
            &["EL JARDIN DE"]
        ));
    }
}
