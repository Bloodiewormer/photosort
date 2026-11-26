mod common;

use ocr::engine::{Engine, Options};
use ocr::manifest::{DICT_KOREAN, DICT_V6};
use ocr::metrics::{Tally, assign_to_lines};
use ocr::select::{Reading, is_hangul};
use ocr::synth::{Category, Fonts, Lang, Split, generate};
use std::collections::HashSet;

/// A character the recognizer cannot output would count as an error no model could avoid.
#[test]
fn every_synthetic_character_is_in_the_dictionary_of_its_recognizer() {
    let Some(models) = common::models_dir() else {
        return;
    };
    let dictionary = |name: &str| -> HashSet<char> {
        let text = std::fs::read_to_string(models.join(name)).unwrap();
        text.lines().flat_map(str::chars).collect()
    };
    let (v6, korean) = (dictionary(DICT_V6), dictionary(DICT_KOREAN));
    assert!(!v6.iter().any(|&c| is_hangul(c)));
    let fonts = Fonts::load().unwrap();
    let mut absent = HashSet::new();
    for split in [Split::Tune, Split::HeldOut] {
        for line in generate(split, &fonts).iter().flat_map(|s| &s.lines) {
            let dict = if line.lang == Lang::Korean {
                &korean
            } else {
                &v6
            };
            absent.extend(
                line.text
                    .chars()
                    .filter(|c| !c.is_whitespace() && !dict.contains(c))
                    .map(|c| (line.lang, c)),
            );
        }
    }
    // Known gap, kept in the ground truth: PP-OCRv6 cannot output the inverted exclamation mark.
    assert_eq!(absent, HashSet::from([(Lang::Latin, '¡')]));
}

#[test]
fn clean_latin_and_korean_lines_are_read_by_the_matching_recognizer() {
    let Some(models) = common::models_dir() else {
        return;
    };
    let engine = Engine::load(&models, &Options::default()).unwrap();
    let samples = generate(Split::Tune, &Fonts::load().unwrap());
    for category in [Category::Latin, Category::Korean] {
        let sample = samples.iter().find(|s| s.category == category).unwrap();
        let (lines, _) = engine
            .read(&sample.image, |_: &Reading| true, &mut Vec::new())
            .unwrap();
        let truth: Vec<_> = sample.lines.iter().map(|l| l.rect).collect();
        let found: Vec<_> = lines.iter().map(|l| l.rect).collect();
        let (assigned, _) = assign_to_lines(&truth, &found);
        let mut tally = Tally::default();
        for (expected, parts) in sample.lines.iter().zip(assigned) {
            let text: Vec<&str> = parts
                .iter()
                .map(|&p| match category {
                    Category::Korean => lines[p].korean.as_ref().unwrap().text.as_str(),
                    _ => lines[p].v6.text.as_str(),
                })
                .collect();
            tally.add(&expected.text, &text.join(" "), false);
        }
        assert!(tally.cer() < 0.15, "{category:?}: {tally:?}");
    }
}
