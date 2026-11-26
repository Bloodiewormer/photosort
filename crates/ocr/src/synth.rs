//! Seeded synthetic test images with exact ground truth. Every title and group name is made up.
//! Fonts are read from the Windows font folder and are never copied into the repository.

mod render;
mod words;

use crate::metrics::Rect;
use ab_glyph::{Font, FontVec};
use image::{Rgb, RgbImage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {
    Latin,
    Japanese,
    Chinese,
    Korean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Latin,
    Japanese,
    Chinese,
    Korean,
    Stylized,
    Small,
    Rotated,
    Vertical,
    Promo,
    NoText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Body,
    Header,
    Title,
    Badge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TruthLine {
    pub text: String,
    /// Bounds of the ink, outline included.
    pub rect: Rect,
    pub lang: Lang,
    pub kind: Kind,
    /// Lines of the same promo-page title share this number.
    pub title: Option<usize>,
}

pub struct Sample {
    pub category: Category,
    pub image: RgbImage,
    pub lines: Vec<TruthLine>,
}

/// Two sets with different seeds and different strings: thresholds are chosen on `Tune`
/// and reported on `HeldOut`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Split {
    Tune,
    HeldOut,
}

/// Windows fonts per language, as `(file, face index inside a .ttc collection)`.
const FONT_FILES: [(Lang, &[(&str, u32)]); 4] = [
    (
        Lang::Latin,
        &[
            ("arial.ttf", 0),
            ("segoeui.ttf", 0),
            ("comic.ttf", 0),
            ("impact.ttf", 0),
        ],
    ),
    (
        Lang::Japanese,
        &[("msgothic.ttc", 0), ("YuGothM.ttc", 0), ("YuGothB.ttc", 0)],
    ),
    (
        Lang::Chinese,
        &[("msyh.ttc", 0), ("msyhbd.ttc", 0), ("simsun.ttc", 0)],
    ),
    (Lang::Korean, &[("malgun.ttf", 0), ("malgunbd.ttf", 0)]),
];

pub struct Fonts(Vec<(Lang, Vec<FontVec>)>);

impl Fonts {
    /// Reads the fonts from the Windows font folder.
    pub fn load() -> Result<Self, String> {
        let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let folder = std::path::Path::new(&windows).join("Fonts");
        let load = |&(file, index): &(&str, u32)| {
            let bytes = std::fs::read(folder.join(file)).map_err(|e| format!("{file}: {e}"))?;
            FontVec::try_from_vec_and_index(bytes, index).map_err(|e| format!("{file}: {e}"))
        };
        let mut all = Vec::new();
        for (lang, files) in FONT_FILES {
            let fonts: Result<Vec<_>, String> = files.iter().map(load).collect();
            all.push((lang, fonts?));
        }
        Ok(Self(all))
    }

    fn of(&self, lang: Lang) -> &[FontVec] {
        self.0
            .iter()
            .find(|(l, _)| *l == lang)
            .map_or(&[], |(_, fonts)| fonts)
    }

    /// The first character of `text` that the first font of `lang` cannot draw.
    pub fn missing_glyph(&self, lang: Lang, text: &str) -> Option<char> {
        self.of(lang).first().and_then(|font| missing(font, text))
    }

    /// A random font of `lang` that has every glyph of `text`: a missing one would be drawn as
    /// nothing or as a box, and the ground truth would be wrong.
    fn pick(&self, lang: Lang, text: &str, rng: &mut Rng) -> &FontVec {
        let fonts = self.of(lang);
        let start = rng.below(fonts.len());
        (0..fonts.len())
            .map(|i| &fonts[(start + i) % fonts.len()])
            .find(|font| missing(font, text).is_none())
            .unwrap_or_else(|| panic!("no {lang:?} font has every glyph of {text:?}"))
    }
}

fn missing(font: &FontVec, text: &str) -> Option<char> {
    text.chars().filter(|c| !c.is_whitespace()).find(|&c| {
        let id = font.glyph_id(c);
        id.0 == 0 || font.outline(id).is_none()
    })
}

/// SplitMix64: small, and the same on every machine.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }

    /// Inclusive on both ends.
    fn between(&mut self, low: i64, high: i64) -> i64 {
        low + self.below((high - low + 1).max(1) as usize) as i64
    }

    fn color(&mut self, low: u8, high: u8) -> Rgb<u8> {
        Rgb([(); 3].map(|()| self.between(low.into(), high.into()) as u8))
    }
}

const WHITE: Rgb<u8> = Rgb([255, 255, 255]);
const BLACK: Rgb<u8> = Rgb([0, 0, 0]);
const CANVAS: (u32, u32) = (960, 720);

fn gradient((w, h): (u32, u32), from: Rgb<u8>, to: Rgb<u8>) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| {
        let t = (x + y) as f32 / (w + h) as f32;
        Rgb(std::array::from_fn(|i| {
            (f32::from(from.0[i]) * (1.0 - t) + f32::from(to.0[i]) * t) as u8
        }))
    })
}

/// Random half-transparent rectangles and discs inside `area` = `(x, y, width, height)`.
fn shapes(image: &mut RgbImage, rng: &mut Rng, area: (i64, i64, i64, i64), count: usize) {
    let (left, top, width, height) = area;
    for _ in 0..count {
        let color = rng.color(0, 255);
        let disc = rng.below(2) == 0;
        let w = rng.between(8, (width / 3).max(9));
        let h = if disc {
            w
        } else {
            rng.between(8, (height / 3).max(9))
        };
        let x0 = left + rng.between(0, (width - w).max(0));
        let y0 = top + rng.between(0, (height - h).max(0));
        for y in y0..(y0 + h).min(top + height) {
            for x in x0..(x0 + w).min(left + width) {
                let dx = 2 * (x - x0) - w;
                let dy = 2 * (y - y0) - h;
                if disc && dx * dx + dy * dy > w * w {
                    continue;
                }
                if let Some(pixel) = image.get_pixel_mut_checked(x as u32, y as u32) {
                    for (channel, new) in pixel.0.iter_mut().zip(color.0) {
                        *channel = ((u16::from(*channel) + 2 * u16::from(new)) / 3) as u8;
                    }
                }
            }
        }
    }
}

/// How the lines of one single-column category are drawn.
#[derive(Clone, Copy)]
struct Style {
    cap: (i64, i64),
    per_image: usize,
    outlined: bool,
    rotated: bool,
}

const CLEAN: Style = Style {
    cap: (18, 32),
    per_image: 6,
    outlined: false,
    rotated: false,
};

fn body(text: String, rect: Rect, lang: Lang) -> TruthLine {
    TruthLine {
        text,
        rect,
        lang,
        kind: Kind::Body,
        title: None,
    }
}

/// Stacks the given lines, `per_image` to a 960x720 image.
fn line_images(
    out: &mut Vec<Sample>,
    category: Category,
    style: Style,
    texts: &[(Lang, String)],
    fonts: &Fonts,
    rng: &mut Rng,
) {
    let (width, height) = (i64::from(CANVAS.0), i64::from(CANVAS.1));
    for page in texts.chunks(style.per_image) {
        let dark = rng.below(3) == 0;
        let mut image = if style.outlined {
            let mut image = gradient(CANVAS, rng.color(40, 230), rng.color(40, 230));
            shapes(&mut image, rng, (0, 0, width, height), 60);
            image
        } else if dark {
            gradient(CANVAS, rng.color(10, 45), rng.color(10, 45))
        } else {
            gradient(CANVAS, rng.color(215, 255), rng.color(215, 255))
        };
        let row = height / style.per_image as i64;
        let mut lines = Vec::new();
        for (i, (lang, text)) in page.iter().enumerate() {
            let font = fonts.pick(*lang, text, rng);
            let mut cap = rng.between(style.cap.0, style.cap.1) as f32;
            let angle = if style.rotated {
                let degrees = rng.between(3, 8) as f32;
                if rng.below(2) == 0 { degrees } else { -degrees }
            } else {
                0.0
            };
            // Shrinks long lines until they fit their row.
            let (mask, radius) = loop {
                let mut mask = render::run(font, text, render::scale_for(font, cap)).cropped();
                if style.rotated {
                    mask = mask.rotated(angle).cropped();
                }
                let radius = if style.outlined {
                    (cap * 0.12).round().max(2.0) as usize
                } else {
                    0
                };
                let grow = 2 * radius as i64;
                if mask.w as i64 + grow <= width - 40 && mask.h as i64 + grow <= row - 12 {
                    break (mask, radius);
                }
                cap *= 0.93;
            };
            let grow = 2 * radius as i64;
            let x = rng.between(20, width - 20 - mask.w as i64 - grow);
            let y = i as i64 * row + rng.between(6, row - 6 - mask.h as i64 - grow);
            let (fill, outline) = if style.outlined {
                (WHITE, Some((BLACK, radius)))
            } else if dark {
                (rng.color(215, 255), None)
            } else {
                (rng.color(0, 60), None)
            };
            let rect = render::stamp(&mut image, &mask, (x, y), fill, outline);
            lines.push(body(text.clone(), rect, *lang));
        }
        out.push(Sample {
            category,
            image,
            lines,
        });
    }
}

/// Japanese columns, right to left.
fn vertical_images(out: &mut Vec<Sample>, split: Split, fonts: &Fonts, rng: &mut Rng) {
    // The long-vowel mark is drawn rotated in real vertical text, which these columns do not do.
    let texts: Vec<String> = words::phrases(split, Lang::Japanese)
        .into_iter()
        .filter(|t| !t.contains('ー') && t.chars().count() <= 14)
        .take(24)
        .collect();
    for page in texts.chunks(6) {
        let mut image = gradient(CANVAS, rng.color(215, 255), rng.color(215, 255));
        let mut lines = Vec::new();
        for (i, text) in page.iter().enumerate() {
            let font = fonts.pick(Lang::Japanese, text, rng);
            let mut cap = rng.between(20, 30) as f32;
            let mask = loop {
                let mask = render::column(font, text, render::scale_for(font, cap)).cropped();
                if mask.h <= 660 && mask.w <= 120 {
                    break mask;
                }
                cap *= 0.93;
            };
            let x = i64::from(CANVAS.0) - 160 * (i as i64 + 1) + (160 - mask.w as i64) / 2;
            let y = rng.between(20, 700 - mask.h as i64);
            let rect = render::stamp(&mut image, &mask, (x, y), rng.color(0, 60), None);
            lines.push(body(text.clone(), rect, Lang::Japanese));
        }
        out.push(Sample {
            category: Category::Vertical,
            image,
            lines,
        });
    }
}

const PAGE: (u32, u32) = (1340, 1280);
const COLUMN: i64 = 268;
const COVER_WIDTH: i64 = 220;
const OUTLINE: usize = 2;

struct Page<'a> {
    image: RgbImage,
    lines: Vec<TruthLine>,
    fonts: &'a Fonts,
}

/// Where an outlined line goes: `x` is either its left edge or its centre.
enum Anchor {
    Left(i64),
    Centre(i64),
    Right(i64),
}

impl Page<'_> {
    /// A white line with a black outline, shrunk until it fits `max_width`.
    fn text(
        &mut self,
        rng: &mut Rng,
        mut line: TruthLine,
        mut cap: f32,
        max_width: i64,
        (anchor, y): (Anchor, i64),
    ) {
        let font = self.fonts.pick(line.lang, &line.text, rng);
        let grow = 2 * OUTLINE as i64;
        let mask = loop {
            let mask = render::run(font, &line.text, render::scale_for(font, cap)).cropped();
            if mask.w as i64 + grow <= max_width {
                break mask;
            }
            cap *= 0.93;
        };
        let width = mask.w as i64 + grow;
        let x = match anchor {
            Anchor::Left(x) => x,
            Anchor::Centre(x) => x - width / 2,
            Anchor::Right(x) => x - width,
        };
        let outline = Some((BLACK, OUTLINE));
        line.rect = render::stamp(&mut self.image, &mask, (x, y), WHITE, outline);
        self.lines.push(line);
    }

    fn fill(&mut self, (x, y, w, h): (i64, i64, i64, i64), color: Rgb<u8>) {
        for py in y..y + h {
            for px in x..x + w {
                self.image.put_pixel(px as u32, py as u32, color);
            }
        }
    }

    /// A fake cover: flat colour and shapes, no text.
    fn cover(&mut self, rng: &mut Rng, x: i64, y: i64, height: i64) {
        self.fill((x, y, COVER_WIDTH, height), rng.color(30, 220));
        shapes(&mut self.image, rng, (x, y, COVER_WIDTH, height), 14);
    }

    fn badge(&mut self, rng: &mut Rng, label: &str, x: i64, y: i64) {
        let font = self.fonts.pick(Lang::Latin, label, rng);
        let mask = render::run(font, label, render::scale_for(font, 15.0)).cropped();
        let color = match label {
            "MANGA" => Rgb([190, 30, 45]),
            "MANHUA" => Rgb([25, 110, 60]),
            "MANHWA" => Rgb([30, 70, 170]),
            _ => Rgb([120, 40, 140]),
        };
        self.fill((x, y, mask.w as i64 + 16, mask.h as i64 + 12), color);
        let rect = render::stamp(&mut self.image, &mask, (x + 8, y + 6), WHITE, None);
        self.lines.push(TruthLine {
            kind: Kind::Badge,
            ..body(label.to_string(), rect, Lang::Latin)
        });
    }
}

const PAGES: usize = 6;
const GROUPS: [&str; 2] = ["LUNA TORCIDA FANSUB", "FAROL DE PAPEL SCANS"];

/// A scanlation group's project list: ten active works, three finished ones.
fn promo_page(split: Split, number: usize, fonts: &Fonts, rng: &mut Rng) -> Sample {
    let mut background = gradient(PAGE, rng.color(20, 90), rng.color(60, 160));
    let whole = (0, 0, i64::from(PAGE.0), i64::from(PAGE.1));
    shapes(&mut background, rng, whole, 25);
    let mut page = Page {
        image: background,
        lines: Vec::new(),
        fonts,
    };
    let header = |text: &str| TruthLine {
        kind: Kind::Header,
        ..body(text.to_string(), [0.0; 4], Lang::Latin)
    };
    let active = header("PROYECTOS ACTIVOS DE:");
    page.text(rng, active, 30.0, 760, (Anchor::Left(24), 28));
    let group = header(GROUPS[split as usize]);
    let right = Anchor::Right(i64::from(PAGE.0) - 24);
    page.text(rng, group, 28.0, 520, (right, 30));
    let finished = header("PROYECTOS TERMINADOS:");
    page.text(rng, finished, 28.0, 760, (Anchor::Left(24), 906));

    // Seven Latin, three Japanese and three Korean works, numbered on from the previous page.
    let mut used = [0; 3];
    for slot in 0..13 {
        let (lang, kind) = match slot {
            1 | 5 | 9 => (Lang::Japanese, 1),
            2 | 7 | 11 => (Lang::Korean, 2),
            _ => (Lang::Latin, 0),
        };
        let per_page = [7, 3, 3][kind];
        let caption = words::title(split, lang, number * per_page + used[kind]);
        used[kind] += 1;
        let (column, top, cover_height) = if slot < 10 {
            let (column, row) = (slot as i64 % 5, slot as i64 / 5);
            (column, 90 + row * 405, 300)
        } else {
            (slot as i64 - 9, 955, 220)
        };
        let x = column * COLUMN + (COLUMN - COVER_WIDTH) / 2;
        page.cover(rng, x, top, cover_height);
        let label = match (slot, lang) {
            (10 | 11, _) => "ONE SHOT",
            (_, Lang::Korean) => "MANHWA",
            (4 | 8, _) => "MANHUA",
            _ => "MANGA",
        };
        page.badge(rng, label, x + 6, top + 6);
        let cap = rng.between(14, 17) as f32;
        let centre = column * COLUMN + COLUMN / 2;
        for (i, text) in caption.iter().enumerate() {
            let y = top + cover_height + 10 + i as i64 * 32;
            let line = TruthLine {
                kind: Kind::Title,
                title: Some(slot),
                ..body(text.clone(), [0.0; 4], lang)
            };
            page.text(rng, line, cap, COLUMN - 12, (Anchor::Centre(centre), y));
        }
    }
    Sample {
        category: Category::Promo,
        image: page.image,
        lines: page.lines,
    }
}

/// Busy images with nothing to read, to count invented text.
fn no_text_image(number: usize, rng: &mut Rng) -> Sample {
    let (width, height) = (i64::from(CANVAS.0), i64::from(CANVAS.1));
    let mut image = gradient(CANVAS, rng.color(0, 255), rng.color(0, 255));
    match number % 5 {
        0 => {}
        1 => shapes(&mut image, rng, (0, 0, width, height), 120),
        2 => {
            let size = rng.between(6, 40) as u32;
            let color = rng.color(0, 255);
            let checker = rng.below(2) == 0;
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                let cell = if checker {
                    x / size + y / size
                } else {
                    (x + y) / size
                };
                if cell % 2 == 0 {
                    *pixel = color;
                }
            }
        }
        3 => {
            // Speckle in small blocks, like film grain or foliage.
            let block = rng.between(2, 6) as usize;
            let (w, h) = (CANVAS.0 as usize / block + 1, CANVAS.1 as usize / block + 1);
            let grain: Vec<Rgb<u8>> = (0..w * h).map(|_| rng.color(0, 255)).collect();
            for (x, y, pixel) in image.enumerate_pixels_mut() {
                *pixel = grain[y as usize / block * w + x as usize / block];
            }
        }
        _ => {
            // Clusters of small shapes in rows: the closest thing to text without any.
            for _ in 0..40 {
                let x = rng.between(0, width - 200);
                let y = rng.between(0, height - 40);
                shapes(&mut image, rng, (x, y, 200, 24), 12);
            }
        }
    }
    Sample {
        category: Category::NoText,
        image,
        lines: Vec::new(),
    }
}

/// Input regimes for the speed benchmark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Regime {
    /// One image size and six short lines each, so every tensor has the same shape.
    Fixed,
    /// A different size, line count and line width in every image, as in a real library.
    Variable,
}

pub const BENCH_IMAGES: usize = 24;

pub fn bench_set(regime: Regime, fonts: &Fonts) -> Vec<Sample> {
    let rng = &mut Rng(3003);
    let langs = [Lang::Latin, Lang::Japanese, Lang::Chinese, Lang::Korean];
    let pools = langs.map(|lang| words::phrases(Split::HeldOut, lang));
    (0..BENCH_IMAGES)
        .map(|number| {
            let (size, count) = match regime {
                Regime::Fixed => ((960, 960), 6),
                // The step keeps every width different.
                Regime::Variable => {
                    let width = 480 + 48 * number as u32 + rng.below(40) as u32;
                    let height = rng.between(420, 1400) as u32;
                    let rows = i64::from(height) / 70;
                    ((width, height), rng.between(1, rows.min(14)) as usize)
                }
            };
            let mut image = gradient(size, rng.color(215, 255), rng.color(215, 255));
            let (width, row) = (i64::from(size.0), i64::from(size.1) / count as i64);
            let lines = (0..count)
                .map(|i| {
                    let which = (number + i) % 4;
                    let (lang, pool) = (langs[which], &pools[which]);
                    let (text, cap) = match regime {
                        // The first characters of a phrase: narrow enough for the smallest
                        // recognizer tensor.
                        Regime::Fixed => {
                            let phrase = &pool[rng.below(pool.len())];
                            let short: String = match lang {
                                Lang::Latin => phrase.chars().take(6).collect(),
                                _ => phrase.chars().take(4).collect(),
                            };
                            (short.trim().to_string(), 34.0)
                        }
                        Regime::Variable => {
                            let parts: Vec<&str> = (0..rng.between(1, 3))
                                .map(|_| pool[rng.below(pool.len())].as_str())
                                .collect();
                            (parts.join(" "), rng.between(16, 28) as f32)
                        }
                    };
                    let font = fonts.pick(lang, &text, rng);
                    let mut cap = cap;
                    let mask = loop {
                        let mask = render::run(font, &text, render::scale_for(font, cap)).cropped();
                        if mask.w as i64 <= width - 32 && mask.h as i64 <= row - 12 {
                            break mask;
                        }
                        cap *= 0.93;
                    };
                    let x = rng.between(16, width - 16 - mask.w as i64);
                    let y = i as i64 * row + (row - mask.h as i64) / 2;
                    let rect = render::stamp(&mut image, &mask, (x, y), rng.color(0, 60), None);
                    body(text, rect, lang)
                })
                .collect();
            Sample {
                category: Category::Latin,
                image,
                lines,
            }
        })
        .collect()
}

/// The whole test set of one split, always the same for the same fonts.
pub fn generate(split: Split, fonts: &Fonts) -> Vec<Sample> {
    let rng = &mut Rng(match split {
        Split::Tune => 1001,
        Split::HeldOut => 2002,
    });
    let pools = [Lang::Latin, Lang::Japanese, Lang::Chinese, Lang::Korean]
        .map(|lang| (lang, words::phrases(split, lang)));
    let mut out = Vec::new();

    let categories = [
        Category::Latin,
        Category::Japanese,
        Category::Chinese,
        Category::Korean,
    ];
    for (category, (lang, pool)) in categories.into_iter().zip(&pools) {
        let texts: Vec<_> = pool.iter().take(48).map(|t| (*lang, t.clone())).collect();
        line_images(&mut out, category, CLEAN, &texts, fonts, rng);
    }

    // The mixed categories take twelve phrases per language, cycling through the languages.
    let mixed = |offset: usize| -> Vec<(Lang, String)> {
        (0..48)
            .map(|i| {
                let (lang, pool) = &pools[i % 4];
                let skip = if *lang == Lang::Latin { 48 } else { 0 };
                (*lang, pool[skip + (offset + i / 4) % 48].clone())
            })
            .collect()
    };
    let stylized = Style {
        cap: (22, 40),
        outlined: true,
        ..CLEAN
    };
    let small = Style {
        cap: (10, 16),
        ..CLEAN
    };
    let rotated = Style {
        cap: (20, 30),
        per_image: 4,
        rotated: true,
        ..CLEAN
    };
    for (category, style, offset) in [
        (Category::Stylized, stylized, 0),
        (Category::Small, small, 12),
        (Category::Rotated, rotated, 24),
    ] {
        line_images(&mut out, category, style, &mixed(offset), fonts, rng);
    }

    vertical_images(&mut out, split, fonts, rng);
    for number in 0..PAGES {
        out.push(promo_page(split, number, fonts, rng));
    }
    for number in 0..20 {
        out.push(no_text_image(number, rng));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::OnceLock;

    fn fresh(split: Split) -> Vec<Sample> {
        generate(split, &Fonts::load().expect("Windows system fonts"))
    }

    /// Drawing a set is slow in debug builds, so the tests share one of each.
    fn set(split: Split) -> &'static [Sample] {
        static SETS: [OnceLock<Vec<Sample>>; 2] = [OnceLock::new(), OnceLock::new()];
        SETS[split as usize].get_or_init(|| fresh(split))
    }

    #[test]
    fn the_same_split_is_identical_every_time() {
        let (a, b) = (set(Split::Tune), fresh(Split::Tune));
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(&b) {
            assert_eq!(a.lines, b.lines);
            assert!(a.image.as_raw() == b.image.as_raw());
        }
    }

    #[test]
    fn tuning_and_held_out_strings_differ() {
        // Section headers and type badges are fixed words by definition.
        let strings = |split| -> HashSet<String> {
            set(split)
                .iter()
                .flat_map(|s| &s.lines)
                .filter(|l| matches!(l.kind, Kind::Body | Kind::Title))
                .map(|l| l.text.clone())
                .collect()
        };
        let (tune, held) = (strings(Split::Tune), strings(Split::HeldOut));
        let shared: Vec<_> = tune.intersection(&held).collect();
        assert!(shared.is_empty(), "{shared:?}");
    }

    #[test]
    fn every_category_and_language_has_enough_lines_inside_the_image() {
        for split in [Split::Tune, Split::HeldOut] {
            let samples = set(split);
            let mut per_category: HashMap<Category, usize> = HashMap::new();
            let mut per_lang: HashMap<Lang, usize> = HashMap::new();
            for sample in samples {
                *per_category.entry(sample.category).or_default() += sample.lines.len();
                for line in &sample.lines {
                    *per_lang.entry(line.lang).or_default() += 1;
                    let [x0, y0, x1, y1] = line.rect;
                    let (w, h) = sample.image.dimensions();
                    assert!(
                        x0 >= 0.0 && y0 >= 0.0 && x1 <= w as f32 && y1 <= h as f32,
                        "{line:?} leaves {w}x{h}"
                    );
                    assert!(x1 - x0 >= 4.0 && y1 - y0 >= 4.0, "{line:?}");
                }
            }
            use Category::*;
            for category in [Latin, Japanese, Chinese, Korean, Stylized, Small, Rotated] {
                assert!(per_category[&category] >= 48, "{category:?}");
            }
            assert!(per_category[&Vertical] >= 24);
            assert_eq!(per_category[&NoText], 0);
            assert!(samples.iter().filter(|s| s.category == NoText).count() >= 20);
            assert!(per_lang.values().all(|&n| n >= 40), "{per_lang:?}");
        }
    }

    #[test]
    fn lines_of_one_image_do_not_overlap() {
        for sample in set(Split::HeldOut) {
            for (i, a) in sample.lines.iter().enumerate() {
                for b in &sample.lines[i + 1..] {
                    assert!(
                        crate::metrics::iou(a.rect, b.rect) < 0.05,
                        "{a:?} overlaps {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_promo_page_lists_many_works_with_badges_and_headers() {
        let pages: Vec<&Sample> = set(Split::HeldOut)
            .iter()
            .filter(|s| s.category == Category::Promo)
            .collect();
        assert!(pages.len() >= 6);
        let mut badges = HashSet::new();
        for page in &pages {
            assert_eq!(page.image.dimensions(), (1340, 1280));
            let titles: HashSet<_> = page.lines.iter().filter_map(|l| l.title).collect();
            assert!(titles.len() >= 12);
            assert!(
                titles
                    .iter()
                    .any(|t| { page.lines.iter().filter(|l| l.title == Some(*t)).count() == 2 })
            );
            for lang in [Lang::Latin, Lang::Japanese, Lang::Korean] {
                assert!(
                    page.lines
                        .iter()
                        .any(|l| l.kind == Kind::Title && l.lang == lang)
                );
            }
            assert!(page.lines.iter().any(|l| l.text == "PROYECTOS ACTIVOS DE:"));
            assert!(page.lines.iter().any(|l| l.text == "PROYECTOS TERMINADOS:"));
            badges.extend(
                page.lines
                    .iter()
                    .filter(|l| l.kind == Kind::Badge)
                    .map(|l| l.text.clone()),
            );
        }
        for badge in ["MANGA", "MANHUA", "MANHWA", "ONE SHOT"] {
            assert!(badges.contains(badge), "{badge}");
        }
    }

    #[test]
    fn benchmark_images_are_either_one_size_or_all_different() {
        let fonts = Fonts::load().expect("Windows system fonts");
        let fixed = bench_set(Regime::Fixed, &fonts);
        assert_eq!(fixed.len(), BENCH_IMAGES);
        for sample in &fixed {
            assert_eq!(sample.image.dimensions(), (960, 960));
            assert_eq!(sample.lines.len(), 6);
            // Detected boxes are looser than the ink: this leaves them under the 320 px tensor.
            for line in &sample.lines {
                let [x0, y0, x1, y1] = line.rect;
                assert!((x1 - x0) / (y1 - y0) <= 4.6, "{line:?}");
            }
        }
        let variable = bench_set(Regime::Variable, &fonts);
        let sizes: HashSet<_> = variable.iter().map(|s| s.image.dimensions()).collect();
        assert_eq!((variable.len(), sizes.len()), (BENCH_IMAGES, BENCH_IMAGES));
        let counts: HashSet<_> = variable.iter().map(|s| s.lines.len()).collect();
        assert!(counts.len() >= 5, "{counts:?}");
        let widest = variable
            .iter()
            .flat_map(|s| &s.lines)
            .map(|l| (l.rect[2] - l.rect[0]) / (l.rect[3] - l.rect[1]))
            .fold(0.0, f32::max);
        assert!(widest > 15.0, "{widest}");
    }

    #[test]
    fn a_font_without_the_glyphs_is_detected() {
        let fonts = Fonts::load().expect("Windows system fonts");
        // Arial has no Hangul: drawing it would silently corrupt the ground truth.
        assert_eq!(fonts.missing_glyph(Lang::Latin, "abc 한"), Some('한'));
        assert_eq!(fonts.missing_glyph(Lang::Latin, "¿Año? ¡Sí!"), None);
        assert_eq!(fonts.missing_glyph(Lang::Korean, "한글 책"), None);
    }
}
