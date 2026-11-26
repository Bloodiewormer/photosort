//! Which recognizer's reading to keep for a detected line. PP-OCRv6 has no Hangul in its
//! dictionary; the Korean model has no kana or CJK.

#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub text: String,
    pub score: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    V6,
    Korean,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thresholds {
    /// H: minimum Hangul share, among letters, to accept the Korean reading.
    pub hangul_share: f32,
    /// M: how far below the v6 score the Korean score may be.
    pub margin: f32,
    /// T: in the cascade, v6 scores below this ask the Korean model.
    pub cascade_below: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Strategy {
    /// S0
    V6Only,
    /// S1
    Both(Thresholds),
    /// S2
    Cascade(Thresholds),
}

/// Readings below this score are only highlighted for review, never discarded: it was chosen on
/// synthetic images, and in the real-photo pilot most marked lines below it were correct.
pub const REVIEW_BELOW: f32 = 0.65;

/// The recommended thresholds, for [`Strategy::Both`]: H and M from the middle of the range that
/// ties on the tuning set. T is the cascade's best there; see `docs/ocr-evaluacion.md`.
pub const RECOMMENDED: Thresholds = Thresholds {
    hangul_share: 0.5,
    margin: 0.1,
    cascade_below: 0.98,
};

pub fn is_hangul(c: char) -> bool {
    matches!(c, '\u{AC00}'..='\u{D7A3}' | '\u{1100}'..='\u{11FF}' | '\u{3130}'..='\u{318F}'
        | '\u{A960}'..='\u{A97F}' | '\u{D7B0}'..='\u{D7FF}')
}

/// Share of Hangul among the alphabetic characters; 0 when there are none.
pub fn hangul_share(text: &str) -> f32 {
    let (mut hangul, mut letters) = (0u32, 0u32);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        letters += 1;
        hangul += u32::from(is_hangul(c));
    }
    if letters == 0 {
        0.0
    } else {
        hangul as f32 / letters as f32
    }
}

/// Whether the strategy runs the Korean model on a line that v6 read as `v6`.
pub fn needs_korean(strategy: Strategy, v6: &Reading) -> bool {
    match strategy {
        Strategy::V6Only => false,
        Strategy::Both(_) => true,
        Strategy::Cascade(t) => {
            v6.text.trim().is_empty()
                || v6.score < t.cascade_below
                || !v6.text.chars().any(char::is_alphabetic)
        }
    }
}

/// `korean` is `None` when the model did not run.
pub fn choose(strategy: Strategy, v6: &Reading, korean: Option<&Reading>) -> Source {
    let (Strategy::Both(t) | Strategy::Cascade(t)) = strategy else {
        return Source::V6;
    };
    match korean {
        Some(k)
            if needs_korean(strategy, v6)
                && hangul_share(&k.text) >= t.hangul_share
                && k.score >= v6.score - t.margin =>
        {
            Source::Korean
        }
        _ => Source::V6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Thresholds = Thresholds {
        hangul_share: 0.5,
        margin: 0.1,
        cascade_below: 0.8,
    };

    fn reading(text: &str, score: f32) -> Reading {
        Reading {
            text: text.into(),
            score,
        }
    }

    #[test]
    fn hangul_share_counts_only_letters() {
        assert_eq!(hangul_share("한글 123 !?"), 1.0);
        assert_eq!(hangul_share("한a"), 0.5);
        assert_eq!(hangul_share("かな漢字 abc"), 0.0);
        assert_eq!(hangul_share("123 !"), 0.0);
        assert_eq!(hangul_share(""), 0.0);
        assert!(is_hangul('ㄱ') && is_hangul('힣') && !is_hangul('ア') && !is_hangul('中'));
    }

    #[test]
    fn v6_only_never_asks_for_korean() {
        let v6 = reading("", 0.0);
        assert!(!needs_korean(Strategy::V6Only, &v6));
        let korean = reading("한글", 1.0);
        assert_eq!(choose(Strategy::V6Only, &v6, Some(&korean)), Source::V6);
    }

    #[test]
    fn both_needs_hangul_and_a_close_enough_score() {
        let s = Strategy::Both(T);
        let v6 = reading("abc", 0.9);
        assert!(needs_korean(s, &v6));
        let pick = |text, score| choose(s, &v6, Some(&reading(text, score)));
        assert_eq!(pick("한글", 0.85), Source::Korean);
        assert_eq!(pick("한글", 0.75), Source::V6);
        assert_eq!(pick("abc", 0.99), Source::V6);
        assert_eq!(choose(s, &v6, None), Source::V6);
    }

    #[test]
    fn cascade_asks_only_for_empty_unsure_or_letterless_v6_readings() {
        let s = Strategy::Cascade(T);
        assert!(needs_korean(s, &reading("  ", 0.99)));
        assert!(needs_korean(s, &reading("abc", 0.79)));
        assert!(needs_korean(s, &reading("123!", 0.99)));
        assert!(!needs_korean(s, &reading("abc", 0.8)));
        // A confident v6 reading wins even if a Korean reading is at hand.
        let korean = reading("한글", 1.0);
        assert_eq!(choose(s, &reading("abc", 0.9), Some(&korean)), Source::V6);
        assert_eq!(
            choose(s, &reading("abc", 0.5), Some(&korean)),
            Source::Korean
        );
    }
}
