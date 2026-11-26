//! Text rasterisation into coverage masks, so outlines and rotation work on any font.

use crate::metrics::Rect;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use image::{Rgb, RgbImage};

pub(super) struct Mask {
    pub w: usize,
    pub h: usize,
    alpha: Vec<f32>,
}

impl Mask {
    pub fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            alpha: vec![0.0; w * h],
        }
    }

    fn at(&self, x: i64, y: i64) -> f32 {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            0.0
        } else {
            self.alpha[y as usize * self.w + x as usize]
        }
    }

    fn raise(&mut self, x: i64, y: i64, value: f32) {
        if x >= 0 && y >= 0 && x < self.w as i64 && y < self.h as i64 {
            let cell = &mut self.alpha[y as usize * self.w + x as usize];
            *cell = cell.max(value.min(1.0));
        }
    }

    pub fn paste(&mut self, other: &Mask, x: i64, y: i64) {
        for row in 0..other.h {
            for col in 0..other.w {
                self.raise(
                    x + col as i64,
                    y + row as i64,
                    other.at(col as i64, row as i64),
                );
            }
        }
    }

    /// `(x0, y0, x1, y1)` of the pixels that are at least half covered.
    fn ink(&self) -> Option<(usize, usize, usize, usize)> {
        let mut bounds: Option<(usize, usize, usize, usize)> = None;
        for (i, _) in self.alpha.iter().enumerate().filter(|(_, a)| **a >= 0.5) {
            let (x, y) = (i % self.w, i / self.w);
            let (x0, y0, x1, y1) = bounds.unwrap_or((x, y, x + 1, y + 1));
            bounds = Some((x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)));
        }
        bounds
    }

    pub fn cropped(&self) -> Mask {
        let Some((x0, y0, x1, y1)) = self.ink() else {
            return Mask::new(0, 0);
        };
        // One spare pixel keeps the anti-aliased edge.
        let (x0, y0) = (x0.saturating_sub(1), y0.saturating_sub(1));
        let (x1, y1) = ((x1 + 1).min(self.w), (y1 + 1).min(self.h));
        let mut out = Mask::new(x1 - x0, y1 - y0);
        out.paste(self, -(x0 as i64), -(y0 as i64));
        out
    }

    /// Rotated about its centre, on a canvas large enough for all of it.
    pub fn rotated(&self, degrees: f32) -> Mask {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let (w, h) = (self.w as f32, self.h as f32);
        let out_w = (w * cos.abs() + h * sin.abs()).ceil() as usize;
        let out_h = (w * sin.abs() + h * cos.abs()).ceil() as usize;
        let mut out = Mask::new(out_w, out_h);
        for y in 0..out_h {
            for x in 0..out_w {
                let dx = x as f32 + 0.5 - out_w as f32 / 2.0;
                let dy = y as f32 + 0.5 - out_h as f32 / 2.0;
                let sx = cos * dx + sin * dy + w / 2.0 - 0.5;
                let sy = -sin * dx + cos * dy + h / 2.0 - 0.5;
                let (left, top) = (sx.floor(), sy.floor());
                let (fx, fy) = (sx - left, sy - top);
                let (left, top) = (left as i64, top as i64);
                let upper = self.at(left, top) * (1.0 - fx) + self.at(left + 1, top) * fx;
                let lower = self.at(left, top + 1) * (1.0 - fx) + self.at(left + 1, top + 1) * fx;
                out.raise(x as i64, y as i64, upper * (1.0 - fy) + lower * fy);
            }
        }
        out
    }

    /// Grown by a disc of `radius`, with `radius` pixels of padding on every side.
    fn dilated(&self, radius: usize) -> Mask {
        let r = radius as i64;
        let mut out = Mask::new(self.w + 2 * radius, self.h + 2 * radius);
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r + r {
                    out.paste(self, r + dx, r + dy);
                }
            }
        }
        out
    }
}

/// The scale at which the font's capital H is `cap_height` pixels tall.
pub(super) fn scale_for(font: &FontVec, cap_height: f32) -> PxScale {
    let h = font
        .outline(font.glyph_id('H'))
        .map(|o| o.bounds.height().abs())
        .unwrap_or_else(|| font.height_unscaled() * 0.7);
    PxScale::from(cap_height * font.height_unscaled() / h)
}

/// One horizontal run on a canvas whose height and baseline depend only on font and scale.
pub(super) fn run(font: &FontVec, text: &str, scale: PxScale) -> Mask {
    let scaled = font.as_scaled(scale);
    let pad = (scale.y * 0.5).ceil();
    let mut pen = pad;
    let mut previous = None;
    let mut glyphs = Vec::new();
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(previous) = previous {
            pen += scaled.kern(previous, id);
        }
        glyphs.push(id.with_scale_and_position(scale, point(pen, pad + scaled.ascent())));
        pen += scaled.h_advance(id);
        previous = Some(id);
    }
    let height = scaled.ascent() - scaled.descent() + 2.0 * pad;
    let mut mask = Mask::new((pen + pad).ceil() as usize, height.ceil() as usize);
    for glyph in glyphs {
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|x, y, coverage| {
                mask.raise(
                    bounds.min.x as i64 + i64::from(x),
                    bounds.min.y as i64 + i64::from(y),
                    coverage,
                );
            });
        }
    }
    mask
}

/// A top-to-bottom column, one character per cell.
pub(super) fn column(font: &FontVec, text: &str, scale: PxScale) -> Mask {
    let scaled = font.as_scaled(scale);
    let pitch = scaled.h_advance(scaled.glyph_id('国')) * 1.08;
    let cells: Vec<Mask> = text
        .chars()
        .map(|c| run(font, c.encode_utf8(&mut [0; 4]), scale))
        .collect();
    let w = cells.iter().map(|c| c.w).max().unwrap_or(0);
    let h = cells.iter().map(|c| c.h).max().unwrap_or(0);
    let mut out = Mask::new(w, h + (pitch * cells.len() as f32).ceil() as usize);
    for (i, cell) in cells.iter().enumerate() {
        out.paste(cell, ((w - cell.w) / 2) as i64, (pitch * i as f32) as i64);
    }
    out
}

fn blend(image: &mut RgbImage, mask: &Mask, x: i64, y: i64, color: Rgb<u8>) {
    for row in 0..mask.h {
        for col in 0..mask.w {
            let (px, py) = (x + col as i64, y + row as i64);
            let alpha = mask.at(col as i64, row as i64);
            if alpha > 0.0
                && px >= 0
                && py >= 0
                && let Some(pixel) = image.get_pixel_mut_checked(px as u32, py as u32)
            {
                for (channel, new) in pixel.0.iter_mut().zip(color.0) {
                    *channel = (f32::from(*channel) * (1.0 - alpha) + f32::from(new) * alpha) as u8;
                }
            }
        }
    }
}

/// Draws a cropped mask so that its ink, outline included, starts near `(x, y)`. Returns the
/// bounds of that ink in image pixels.
pub(super) fn stamp(
    image: &mut RgbImage,
    mask: &Mask,
    (x, y): (i64, i64),
    fill: Rgb<u8>,
    outline: Option<(Rgb<u8>, usize)>,
) -> Rect {
    let radius = outline.map_or(0, |(_, r)| r);
    let grown = mask.dilated(radius);
    if let Some((color, _)) = outline {
        blend(image, &grown, x, y, color);
    }
    blend(image, mask, x + radius as i64, y + radius as i64, fill);
    let (x0, y0, x1, y1) = grown.ink().unwrap_or((0, 0, 0, 0));
    [
        (x + x0 as i64) as f32,
        (y + y0 as i64) as f32,
        (x + x1 as i64) as f32,
        (y + y1 as i64) as f32,
    ]
}
