//! Scores what the engine read against the synthetic ground truth, for any selection strategy.
//! Both recognizers read every line once; strategies are then compared offline.

use crate::engine::Line;
use crate::metrics::{Tally, assign_to_lines, match_boxes, normalize, similarity, title_found};
use crate::select::{Reading, Source, Strategy, choose, needs_korean};
use crate::synth::{Category, Kind, Lang, TruthLine};
use std::collections::{BTreeMap, BTreeSet};

/// A detection counts for the recall and precision figures from this overlap on.
pub const MIN_IOU: f32 = 0.5;

pub struct ImageRead<'a> {
    pub category: Category,
    pub truth: &'a [TruthLine],
    pub lines: Vec<Line>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Boxes {
    pub matched: usize,
    /// Truth lines with at least one box centred on them.
    pub found: usize,
    pub truth: usize,
    pub predicted: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Found {
    pub found: usize,
    pub total: usize,
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Invented {
    pub images: usize,
    pub images_with_text: usize,
    pub readings: usize,
    pub chars: usize,
}

#[derive(Debug, Default)]
pub struct Score {
    /// Case-sensitive.
    pub lang: BTreeMap<Lang, Tally>,
    pub latin_folded: Tally,
    pub category: BTreeMap<Category, Tally>,
    pub category_lang: BTreeMap<(Category, Lang), Tally>,
    pub detection: BTreeMap<Category, Boxes>,
    pub titles: BTreeMap<Lang, Found>,
    pub badges: Found,
    pub no_text: Invented,
    /// Non-empty readings that belong to no truth line, in images that do have text.
    pub stray: usize,
    pub lines_detected: usize,
    pub korean_runs: usize,
}

impl Score {
    /// Unweighted mean over the languages, so that no language dominates by line count.
    pub fn mean_cer(&self) -> f64 {
        let sum: f64 = self.lang.values().map(Tally::cer).sum();
        sum / self.lang.len().max(1) as f64
    }
}

/// The reading the strategy keeps for a detected line.
pub fn chosen(strategy: Strategy, line: &Line) -> &Reading {
    match (
        choose(strategy, &line.v6, line.korean.as_ref()),
        &line.korean,
    ) {
        (Source::Korean, Some(korean)) => korean,
        _ => &line.v6,
    }
}

/// What was read for one truth line. A line split by the detector is joined back, without a
/// space in Japanese and Chinese, which do not use them.
pub fn read_text(strategy: Strategy, lang: Lang, parts: &[usize], lines: &[Line]) -> String {
    let texts: Vec<&str> = parts
        .iter()
        .map(|&p| chosen(strategy, &lines[p]).text.trim())
        .filter(|t| !t.is_empty())
        .collect();
    let space = match lang {
        Lang::Japanese | Lang::Chinese => "",
        Lang::Latin | Lang::Korean => " ",
    };
    texts.join(space)
}

pub fn score(set: &[ImageRead], strategy: Strategy) -> Score {
    let mut score = Score::default();
    for image in set {
        score.lines_detected += image.lines.len();
        score.korean_runs += image
            .lines
            .iter()
            .filter(|l| needs_korean(strategy, &l.v6))
            .count();
        let said = |p: usize| !chosen(strategy, &image.lines[p]).text.trim().is_empty();
        if image.category == Category::NoText {
            let invented: Vec<usize> = (0..image.lines.len()).filter(|&p| said(p)).collect();
            score.no_text.images += 1;
            score.no_text.images_with_text += usize::from(!invented.is_empty());
            score.no_text.readings += invented.len();
            score.no_text.chars += invented
                .iter()
                .map(|&p| chosen(strategy, &image.lines[p]).text.chars().count())
                .sum::<usize>();
            continue;
        }
        let truth: Vec<_> = image.truth.iter().map(|l| l.rect).collect();
        let found: Vec<_> = image.lines.iter().map(|l| l.rect).collect();
        let boxes = score.detection.entry(image.category).or_default();
        boxes.matched += match_boxes(&truth, &found, MIN_IOU).len();
        boxes.truth += truth.len();
        boxes.predicted += found.len();

        let (assigned, stray) = assign_to_lines(&truth, &found);
        boxes.found += assigned.iter().filter(|parts| !parts.is_empty()).count();
        score.stray += stray.into_iter().filter(|&p| said(p)).count();
        let read: Vec<String> = image
            .truth
            .iter()
            .zip(&assigned)
            .map(|(line, parts)| read_text(strategy, line.lang, parts, &image.lines))
            .collect();
        for (line, read) in image.truth.iter().zip(&read) {
            for tally in [
                score.lang.entry(line.lang).or_default(),
                score.category.entry(image.category).or_default(),
                score
                    .category_lang
                    .entry((image.category, line.lang))
                    .or_default(),
            ] {
                tally.add(&line.text, read, false);
            }
            if line.lang == Lang::Latin {
                score.latin_folded.add(&line.text, read, true);
            }
            if line.kind == Kind::Badge {
                let squeeze = |t: &str| normalize(t).to_lowercase().replace(' ', "");
                let hit = similarity(&squeeze(&line.text), &squeeze(read))
                    >= crate::metrics::TITLE_SIMILARITY;
                score.badges.found += usize::from(hit);
                score.badges.total += 1;
            }
        }
        let titles: BTreeSet<usize> = image.truth.iter().filter_map(|l| l.title).collect();
        for title in titles {
            let own: Vec<usize> = (0..image.truth.len())
                .filter(|&i| image.truth[i].title == Some(title))
                .collect();
            let expected: Vec<&str> = own.iter().map(|&i| image.truth[i].text.as_str()).collect();
            let got: Vec<&str> = own.iter().map(|&i| read[i].as_str()).collect();
            let lang = score.titles.entry(image.truth[own[0]].lang).or_default();
            lang.found += usize::from(title_found(&expected, &got));
            lang.total += 1;
        }
    }
    score
}

pub struct Tuned {
    pub best: Strategy,
    pub cer: f64,
    /// Every candidate that reaches that CER, in the order given.
    pub tied: Vec<Strategy>,
}

/// The lowest mean CER; among equals, those that run the Korean model least, and of those the
/// middle one, so that a threshold does not sit on the edge of the range that works.
pub fn tune(set: &[ImageRead], candidates: &[Strategy]) -> Option<Tuned> {
    let scored: Vec<(Strategy, f64, usize)> = candidates
        .iter()
        .map(|&strategy| {
            let score = score(set, strategy);
            (strategy, score.mean_cer(), score.korean_runs)
        })
        .collect();
    let cer = scored.iter().map(|s| s.1).min_by(f64::total_cmp)?;
    let tied: Vec<_> = scored.iter().filter(|s| s.1 - cer < 1e-12).collect();
    let fewest = tied.iter().map(|s| s.2).min()?;
    let cheapest: Vec<_> = tied.iter().filter(|s| s.2 == fewest).collect();
    Some(Tuned {
        best: cheapest[cheapest.len() / 2].0,
        cer,
        tied: tied.iter().map(|s| s.0).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::Thresholds;

    const T: Thresholds = Thresholds {
        hangul_share: 0.5,
        margin: 0.1,
        cascade_below: 0.8,
    };

    fn truth(text: &str, y: f32, lang: Lang, kind: Kind, title: Option<usize>) -> TruthLine {
        TruthLine {
            text: text.into(),
            rect: [0.0, y, 100.0, y + 10.0],
            lang,
            kind,
            title,
        }
    }

    fn line(rect: [f32; 4], v6: (&str, f32), korean: (&str, f32)) -> Line {
        let reading = |(text, score): (&str, f32)| Reading {
            text: text.into(),
            score,
        };
        Line {
            rect,
            v6: reading(v6),
            korean: Some(reading(korean)),
        }
    }

    fn page() -> Vec<TruthLine> {
        vec![
            truth("한글 책", 0.0, Lang::Korean, Kind::Title, Some(0)),
            truth("星空猫", 20.0, Lang::Japanese, Kind::Body, None),
            truth("MANGA", 40.0, Lang::Latin, Kind::Badge, None),
        ]
    }

    fn reads(truth: &[TruthLine]) -> Vec<ImageRead<'_>> {
        let nothing = ("", 0.0);
        vec![
            ImageRead {
                category: Category::Promo,
                truth,
                lines: vec![
                    // v6 invents Chinese for Korean, with less confidence than the Korean model.
                    line([0.0, 0.0, 100.0, 10.0], ("吾吾", 0.6), ("한글 책", 0.95)),
                    // One line detected as two boxes, listed right to left.
                    line([52.0, 20.0, 100.0, 30.0], ("猫", 0.9), nothing),
                    line([0.0, 20.0, 48.0, 30.0], ("星空", 0.9), nothing),
                    line([0.0, 40.0, 100.0, 50.0], ("manga", 0.99), nothing),
                    line([300.0, 300.0, 340.0, 310.0], ("x", 0.5), nothing),
                ],
            },
            ImageRead {
                category: Category::NoText,
                truth: &[],
                lines: vec![
                    line([0.0, 0.0, 9.0, 9.0], ("III", 0.4), nothing),
                    line([20.0, 0.0, 29.0, 9.0], ("", 0.0), nothing),
                ],
            },
        ]
    }

    #[test]
    fn scoring_with_v6_only_charges_the_korean_line() {
        let truth = page();
        let score = score(&reads(&truth), Strategy::V6Only);
        assert_eq!(score.lang[&Lang::Korean].cer(), 1.0);
        assert_eq!(score.lang[&Lang::Japanese].exact, 1);
        assert_eq!(score.titles[&Lang::Korean], Found { found: 0, total: 1 });
        assert_eq!(score.korean_runs, 0);
    }

    #[test]
    fn scoring_with_both_reads_it_and_counts_boxes_badges_and_inventions() {
        let truth = page();
        let score = score(&reads(&truth), Strategy::Both(T));
        assert_eq!(score.lang[&Lang::Korean].cer(), 0.0);
        assert_eq!(score.titles[&Lang::Korean], Found { found: 1, total: 1 });
        // Badges ignore case and spacing; CER on Latin does not, unless folded.
        assert_eq!(score.badges, Found { found: 1, total: 1 });
        assert_eq!(score.lang[&Lang::Latin].exact, 0);
        assert_eq!(score.latin_folded.exact, 1);
        let boxes = score.detection[&Category::Promo];
        assert_eq!((boxes.matched, boxes.truth, boxes.predicted), (2, 3, 5));
        // A split line misses the overlap threshold but still has boxes on it.
        assert_eq!(boxes.found, 3);
        assert_eq!(score.stray, 1);
        let invented = Invented {
            images: 1,
            images_with_text: 1,
            readings: 1,
            chars: 3,
        };
        assert_eq!(score.no_text, invented);
        assert_eq!((score.lines_detected, score.korean_runs), (7, 7));
    }

    #[test]
    fn tuning_prefers_accuracy_then_fewer_korean_runs() {
        let truth = page();
        let set = reads(&truth);
        let candidates = [Strategy::V6Only, Strategy::Both(T), Strategy::Cascade(T)];
        let tuned = tune(&set, &candidates).unwrap();
        assert_eq!(tuned.best, Strategy::Cascade(T));
        assert_eq!(tuned.tied, [Strategy::Both(T), Strategy::Cascade(T)]);
        assert!(tuned.cer < score(&set, Strategy::V6Only).mean_cer());
    }

    #[test]
    fn among_equal_candidates_tuning_takes_the_middle_one_not_an_edge() {
        let truth = page();
        let set = reads(&truth);
        let with_margin = |margin| Strategy::Both(Thresholds { margin, ..T });
        let candidates = [0.05, 0.1, 0.2].map(with_margin);
        assert_eq!(tune(&set, &candidates).unwrap().best, with_margin(0.1));
    }
}
