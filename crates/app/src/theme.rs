use crate::settings::Theme;
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Visuals};

// Spacing scale (px)
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 12.0;
pub const SPACE_LG: f32 = 16.0;
pub const SPACE_XL: f32 = 24.0;

// Typography scale (px). Minimum readable font size is 13 px.
pub const FONT_SMALL: f32 = 13.0;
pub const FONT_BODY: f32 = 14.0;
pub const FONT_HEADING: f32 = 16.0;
pub const FONT_TITLE: f32 = 20.0;

// Layout dimensions (px)
pub const TOP_BAR_HEIGHT: f32 = 44.0;
pub const SIDEBAR_WIDTH: f32 = 268.0;
pub const INSPECTOR_WIDTH: f32 = 348.0;
pub const CELL_WIDTH: f32 = 172.0;
pub const CELL_HEIGHT: f32 = 206.0;
pub const CELL_THUMB_HEIGHT: f32 = 148.0;
pub const CELL_SPACING: f32 = 10.0;
pub const PREVIEW_MAX_EDGE: u32 = 1200;

// Radii & borders
pub const RADIUS_SM: u8 = 4;
pub const RADIUS_MD: u8 = 6;
pub const FOCUS_STROKE_WIDTH: f32 = 2.0;

#[derive(Debug, Clone, Copy)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn color32(self) -> Color32 {
        Color32::from_rgb(self.0, self.1, self.2)
    }

    /// Relative luminance per WCAG 2.1.
    pub fn relative_luminance(self) -> f64 {
        fn channel(v: u8) -> f64 {
            let s = f64::from(v) / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(self.0) + 0.7152 * channel(self.1) + 0.0722 * channel(self.2)
    }

    /// Contrast ratio between two opaque colors per WCAG 2.1 (1.0 to 21.0).
    pub fn contrast_ratio(self, other: Self) -> f64 {
        let a = self.relative_luminance();
        let b = other.relative_luminance();
        let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_canvas: Rgb,
    pub bg_panel: Rgb,
    pub bg_card: Rgb,
    pub bg_elevated: Rgb,
    pub border: Rgb,
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub accent: Rgb,
    pub accent_bg: Rgb,
    pub chip_require_bg: Rgb,
    pub chip_require_fg: Rgb,
    pub chip_any_bg: Rgb,
    pub chip_any_fg: Rgb,
    pub chip_exclude_bg: Rgb,
    pub chip_exclude_fg: Rgb,
    pub review_badge_bg: Rgb,
    pub review_badge_fg: Rgb,
    pub status_accepted_fg: Rgb,
    pub status_rejected_fg: Rgb,
    pub status_suggested_fg: Rgb,
}

pub const DARK_PALETTE: Palette = Palette {
    bg_canvas: Rgb(18, 20, 23),
    bg_panel: Rgb(25, 28, 33),
    bg_card: Rgb(34, 38, 45),
    bg_elevated: Rgb(43, 48, 58),
    border: Rgb(58, 65, 78),
    text_primary: Rgb(236, 239, 244),
    text_secondary: Rgb(168, 178, 194),
    accent: Rgb(91, 155, 213),
    accent_bg: Rgb(31, 58, 86),
    chip_require_bg: Rgb(29, 64, 50),
    chip_require_fg: Rgb(184, 242, 214),
    chip_any_bg: Rgb(29, 55, 82),
    chip_any_fg: Rgb(197, 226, 255),
    chip_exclude_bg: Rgb(76, 34, 40),
    chip_exclude_fg: Rgb(255, 208, 214),
    review_badge_bg: Rgb(89, 64, 16),
    review_badge_fg: Rgb(255, 231, 168),
    status_accepted_fg: Rgb(142, 230, 184),
    status_rejected_fg: Rgb(255, 155, 166),
    status_suggested_fg: Rgb(255, 210, 125),
};

pub const LIGHT_PALETTE: Palette = Palette {
    bg_canvas: Rgb(241, 243, 246),
    bg_panel: Rgb(248, 249, 251),
    bg_card: Rgb(255, 255, 255),
    bg_elevated: Rgb(229, 233, 240),
    border: Rgb(192, 201, 214),
    text_primary: Rgb(26, 30, 36),
    text_secondary: Rgb(72, 82, 98),
    accent: Rgb(31, 95, 168),
    accent_bg: Rgb(220, 235, 250),
    chip_require_bg: Rgb(216, 243, 229),
    chip_require_fg: Rgb(15, 76, 48),
    chip_any_bg: Rgb(217, 233, 250),
    chip_any_fg: Rgb(18, 60, 110),
    chip_exclude_bg: Rgb(251, 220, 224),
    chip_exclude_fg: Rgb(122, 22, 34),
    review_badge_bg: Rgb(253, 239, 199),
    review_badge_fg: Rgb(92, 61, 0),
    status_accepted_fg: Rgb(15, 92, 56),
    status_rejected_fg: Rgb(154, 28, 42),
    status_suggested_fg: Rgb(122, 78, 0),
};

pub fn palette_for(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        egui::Theme::Dark => &DARK_PALETTE,
        egui::Theme::Light => &LIGHT_PALETTE,
    }
}

fn build_visuals(dark: bool, p: &Palette) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg_panel.color32();
    v.window_fill = p.bg_panel.color32();
    v.extreme_bg_color = p.bg_canvas.color32();
    v.faint_bg_color = p.bg_card.color32();
    v.override_text_color = Some(p.text_primary.color32());
    v.selection.bg_fill = p.accent_bg.color32();
    v.selection.stroke = Stroke::new(FOCUS_STROKE_WIDTH, p.accent.color32());

    let radius = CornerRadius::same(RADIUS_SM);
    v.widgets.noninteractive.bg_fill = p.bg_panel.color32();
    v.widgets.noninteractive.weak_bg_fill = p.bg_card.color32();
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border.color32());
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_primary.color32());
    v.widgets.noninteractive.corner_radius = radius;

    v.widgets.inactive.bg_fill = p.bg_card.color32();
    v.widgets.inactive.weak_bg_fill = p.bg_card.color32();
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, p.border.color32());
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text_primary.color32());
    v.widgets.inactive.corner_radius = radius;

    v.widgets.hovered.bg_fill = p.bg_elevated.color32();
    v.widgets.hovered.weak_bg_fill = p.bg_elevated.color32();
    v.widgets.hovered.bg_stroke = Stroke::new(1.5, p.accent.color32());
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text_primary.color32());
    v.widgets.hovered.corner_radius = radius;

    v.widgets.active.bg_fill = p.accent_bg.color32();
    v.widgets.active.weak_bg_fill = p.accent_bg.color32();
    v.widgets.active.bg_stroke = Stroke::new(FOCUS_STROKE_WIDTH, p.accent.color32());
    v.widgets.active.fg_stroke = Stroke::new(1.0, p.text_primary.color32());
    v.widgets.active.corner_radius = radius;

    v.widgets.open.bg_fill = p.bg_elevated.color32();
    v.widgets.open.weak_bg_fill = p.bg_elevated.color32();
    v.widgets.open.bg_stroke = Stroke::new(1.5, p.accent.color32());
    v.widgets.open.fg_stroke = Stroke::new(1.0, p.text_primary.color32());
    v.widgets.open.corner_radius = radius;

    v
}

/// Configures typography (>= 13 px), consistent spacing, and accessible Dark/Light visuals.
pub fn configure_styles(ctx: &egui::Context, preference: Theme) {
    let apply_style = |style: &mut egui::Style, dark: bool, p: &Palette| {
        style.visuals = build_visuals(dark, p);
        style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
        style.spacing.button_padding = egui::vec2(SPACE_MD, SPACE_XS + 2.0);
        style.spacing.window_margin = egui::Margin::same(SPACE_MD as i8);
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(FONT_SMALL, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Body,
            FontId::new(FONT_BODY, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(FONT_BODY, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(FONT_SMALL, FontFamily::Monospace),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(FONT_HEADING, FontFamily::Proportional),
        );
    };

    ctx.style_mut_of(egui::Theme::Dark, |s| apply_style(s, true, &DARK_PALETTE));
    ctx.style_mut_of(egui::Theme::Light, |s| {
        apply_style(s, false, &LIGHT_PALETTE)
    });
    apply_theme_preference(ctx, preference);
}

pub fn apply_theme_preference(ctx: &egui::Context, preference: Theme) {
    let pref = match preference {
        Theme::System => egui::ThemePreference::System,
        Theme::Dark => egui::ThemePreference::Dark,
        Theme::Light => egui::ThemePreference::Light,
    };
    ctx.set_theme(pref);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_palette_accessible(name: &str, p: &Palette) {
        let surfaces = [
            ("bg_canvas", p.bg_canvas),
            ("bg_panel", p.bg_panel),
            ("bg_card", p.bg_card),
            ("bg_elevated", p.bg_elevated),
        ];
        for (surf_name, bg) in surfaces {
            let primary = p.text_primary.contrast_ratio(bg);
            assert!(
                primary >= 4.5,
                "{name}: text_primary on {surf_name} contrast {primary:.2} < 4.5"
            );
            let secondary = p.text_secondary.contrast_ratio(bg);
            assert!(
                secondary >= 4.5,
                "{name}: text_secondary on {surf_name} contrast {secondary:.2} < 4.5"
            );
        }

        let pairs = [
            ("text_primary on accent_bg", p.text_primary, p.accent_bg),
            ("chip_require", p.chip_require_fg, p.chip_require_bg),
            ("chip_any", p.chip_any_fg, p.chip_any_bg),
            ("chip_exclude", p.chip_exclude_fg, p.chip_exclude_bg),
            ("review_badge", p.review_badge_fg, p.review_badge_bg),
            ("accepted on card", p.status_accepted_fg, p.bg_card),
            ("rejected on card", p.status_rejected_fg, p.bg_card),
            ("suggested on card", p.status_suggested_fg, p.bg_card),
            ("accepted on panel", p.status_accepted_fg, p.bg_panel),
            ("rejected on panel", p.status_rejected_fg, p.bg_panel),
            ("suggested on panel", p.status_suggested_fg, p.bg_panel),
        ];
        for (label, fg, bg) in pairs {
            let ratio = fg.contrast_ratio(bg);
            assert!(ratio >= 4.5, "{name}: {label} contrast {ratio:.2} < 4.5");
        }
    }

    #[test]
    fn dark_and_light_palettes_meet_wcag_aa_contrast_ratio() {
        assert_palette_accessible("dark", &DARK_PALETTE);
        assert_palette_accessible("light", &LIGHT_PALETTE);
    }

    #[test]
    fn font_sizes_meet_minimum_readability_threshold() {
        const {
            assert!(FONT_SMALL >= 13.0);
            assert!(FONT_BODY >= FONT_SMALL);
            assert!(FONT_HEADING >= FONT_BODY);
            assert!(FONT_TITLE >= FONT_HEADING);
        }
    }
}
