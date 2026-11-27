use crate::settings::Theme;
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, Sense, Shadow, Stroke, TextStyle,
    Vec2, Visuals,
};

// Spacing scale (px) — strict 4 px grid
pub const SPACE_XS: f32 = 4.0;
pub const SPACE_SM: f32 = 8.0;
pub const SPACE_MD: f32 = 12.0;
pub const SPACE_LG: f32 = 16.0;
pub const SPACE_XL: f32 = 24.0;
pub const SPACE_2XL: f32 = 32.0;

// Typography scale (px). Minimum readable font size is 13 px.
pub const FONT_SMALL: f32 = 13.0;
pub const FONT_BODY: f32 = 14.0;
pub const FONT_HEADING: f32 = 16.0;
pub const FONT_TITLE: f32 = 20.0;

// Layout dimensions (px) — multiples of 4
pub const TOP_BAR_HEIGHT: f32 = 48.0;
pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const INSPECTOR_WIDTH: f32 = 328.0;
pub const CELL_WIDTH: f32 = 176.0;
pub const CELL_HEIGHT: f32 = 208.0;
pub const CELL_THUMB_HEIGHT: f32 = 148.0;
pub const CELL_SPACING: f32 = 12.0;
pub const PREVIEW_MAX_EDGE: u32 = 1200;

// Radii & borders
pub const RADIUS_SM: u8 = 4;
pub const RADIUS_MD: u8 = 8;
pub const RADIUS_LG: u8 = 12;
pub const FOCUS_STROKE_WIDTH: f32 = 2.0;

const ICON_PNG_BYTES: &[u8] = include_bytes!("../../../assets/icon.png");

/// Decodes the embedded 256 px application icon into RGBA8 pixels + `(width, height)`.
pub fn load_app_icon_rgba() -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(ICON_PNG_BYTES).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    Some((img.into_raw(), w, h))
}

/// Builds `egui::IconData` for the native window titlebar / taskbar icon.
pub fn window_icon() -> Option<egui::IconData> {
    let (rgba, width, height) = load_app_icon_rgba()?;
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

/// Loads the embedded logo as an egui `ColorImage` for the sidebar and welcome screen.
pub fn logo_color_image() -> Option<egui::ColorImage> {
    let (rgba, width, height) = load_app_icon_rgba()?;
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width as usize, height as usize],
        &rgba,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// Exact palette from `docs/diseno.md`: warm graphite neutrals + grease-pencil ochre accent.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_canvas: Rgb,
    pub bg_panel: Rgb,
    pub bg_card: Rgb,
    pub bg_elevated: Rgb,
    pub border: Rgb,
    pub text_primary: Rgb,
    pub text_secondary: Rgb,
    pub text_hint: Rgb,
    pub accent: Rgb,
    pub accent_bg: Rgb,
    pub primary_btn_bg: Rgb,
    pub primary_btn_fg: Rgb,
    pub mark_ochre: Rgb,
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

pub const OCHRE_MARK: Rgb = Rgb(0xd9, 0xa4, 0x16);
pub const OCHRE_INK: Rgb = Rgb(0x1c, 0x16, 0x08);

pub const DARK_PALETTE: Palette = Palette {
    bg_canvas: Rgb(0x15, 0x15, 0x14),
    bg_panel: Rgb(0x1c, 0x1c, 0x1a),
    bg_card: Rgb(0x24, 0x24, 0x22),
    bg_elevated: Rgb(0x2f, 0x2f, 0x2b),
    border: Rgb(0x3d, 0x3c, 0x37),
    text_primary: Rgb(0xec, 0xeb, 0xe6),
    text_secondary: Rgb(0xb6, 0xb4, 0xaa),
    text_hint: Rgb(0xa0, 0x9e, 0x93),
    accent: Rgb(0xd9, 0xa4, 0x16),
    accent_bg: Rgb(0x3a, 0x30, 0x18),
    primary_btn_bg: OCHRE_MARK,
    primary_btn_fg: OCHRE_INK,
    mark_ochre: OCHRE_MARK,
    chip_require_bg: Rgb(0x2d, 0x35, 0x20),
    chip_require_fg: Rgb(0xd6, 0xe2, 0xae),
    chip_any_bg: Rgb(0x3a, 0x30, 0x18),
    chip_any_fg: Rgb(0xf0, 0xd3, 0x8a),
    chip_exclude_bg: Rgb(0x3f, 0x24, 0x1d),
    chip_exclude_fg: Rgb(0xf2, 0xc3, 0xb4),
    review_badge_bg: Rgb(0x3a, 0x30, 0x18),
    review_badge_fg: Rgb(0xf0, 0xd3, 0x8a),
    status_accepted_fg: Rgb(0xa3, 0xb8, 0x70),
    status_rejected_fg: Rgb(0xe8, 0x95, 0x7d),
    status_suggested_fg: Rgb(0xd9, 0xa4, 0x16),
};

pub const LIGHT_PALETTE: Palette = Palette {
    bg_canvas: Rgb(0xf2, 0xf1, 0xec),
    bg_panel: Rgb(0xfa, 0xf9, 0xf5),
    bg_card: Rgb(0xff, 0xff, 0xff),
    bg_elevated: Rgb(0xe9, 0xe7, 0xdf),
    border: Rgb(0xcd, 0xc9, 0xbb),
    text_primary: Rgb(0x1d, 0x1c, 0x19),
    text_secondary: Rgb(0x4c, 0x4a, 0x42),
    text_hint: Rgb(0x5c, 0x59, 0x4f),
    accent: Rgb(0x76, 0x55, 0x00),
    accent_bg: Rgb(0xf4, 0xe7, 0xbd),
    primary_btn_bg: OCHRE_MARK,
    primary_btn_fg: OCHRE_INK,
    mark_ochre: OCHRE_MARK,
    chip_require_bg: Rgb(0xe3, 0xeb, 0xc9),
    chip_require_fg: Rgb(0x34, 0x46, 0x0f),
    chip_any_bg: Rgb(0xf4, 0xe7, 0xbd),
    chip_any_fg: Rgb(0x5c, 0x42, 0x00),
    chip_exclude_bg: Rgb(0xf6, 0xd9, 0xcf),
    chip_exclude_fg: Rgb(0x6f, 0x24, 0x13),
    review_badge_bg: Rgb(0xf4, 0xe7, 0xbd),
    review_badge_fg: Rgb(0x5c, 0x42, 0x00),
    status_accepted_fg: Rgb(0x4a, 0x5e, 0x1e),
    status_rejected_fg: Rgb(0x97, 0x38, 0x1f),
    status_suggested_fg: Rgb(0x76, 0x55, 0x00),
};

pub fn palette_for(ctx: &egui::Context) -> &'static Palette {
    match ctx.theme() {
        egui::Theme::Dark => &DARK_PALETTE,
        egui::Theme::Light => &LIGHT_PALETTE,
    }
}

/// Builds accessible placeholder text styled with the active palette's `text_hint` colour.
pub fn hint_text(palette: &Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text).color(palette.text_hint.color32())
}

/// Gives a control the name a screen reader says, for those whose text on screen does not name them.
pub fn named(response: egui::Response, name: &str) -> egui::Response {
    response
        .ctx
        .accesskit_node_builder(response.id, |node| node.set_label(name));
    response
}

/// Draws the keyboard focus ring around any focused control. Buttons with their own stroke or
/// no frame hide egui's focus stroke, so it is painted here, once, outside the control.
pub fn paint_focus_ring(ctx: &egui::Context) {
    let Some(focused) = ctx
        .memory(|m| m.focused())
        .and_then(|id| ctx.read_response(id))
    else {
        return;
    };
    if focused.interact_rect.is_positive() {
        ctx.layer_painter(focused.layer_id).rect_stroke(
            focused.interact_rect.expand(2.0),
            CornerRadius::same(RADIUS_MD),
            Stroke::new(FOCUS_STROKE_WIDTH, palette_for(ctx).accent.color32()),
            egui::StrokeKind::Outside,
        );
    }
}

/// Scrolls a control that the keyboard focused out of view back into it, which egui does not do
/// by itself. Call it before drawing. It asks the way a screen reader does.
pub fn scroll_focus_into_view(ctx: &egui::Context) {
    let focused = ctx.memory(|m| m.focused());
    let key = egui::Id::unique("photosort_last_focus");
    let previous = ctx.data(|d| d.get_temp::<Option<egui::Id>>(key)).flatten();
    ctx.data_mut(|d| d.insert_temp(key, focused));
    if focused == previous || ctx.input(|i| i.pointer.any_down() || i.pointer.any_released()) {
        return;
    }
    let Some(response) = focused.and_then(|id| ctx.read_response(id)) else {
        return;
    };
    // Only a clipped control: with no scroll area around it, the request would reach another one.
    if !response
        .interact_rect
        .expand(1.0)
        .contains_rect(response.rect)
    {
        let request = egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::ScrollIntoView,
            target_tree: egui::accesskit::TreeId::ROOT,
            target_node: response.id.accesskit_id(),
            data: None,
        };
        ctx.input_mut(|i| i.events.push(egui::Event::AccessKitActionRequest(request)));
        ctx.request_repaint();
    }
}

/// Builds the primary ochre button (`#d9a416` fill with `#1c1608` text in both themes).
pub fn primary_button<'a>(palette: &Palette, text: impl Into<String>) -> egui::Button<'a> {
    egui::Button::new(
        egui::RichText::new(text.into())
            .color(palette.primary_btn_fg.color32())
            .size(FONT_BODY),
    )
    .fill(palette.primary_btn_bg.color32())
    .stroke(Stroke::new(1.0, palette.primary_btn_bg.color32()))
    .corner_radius(CornerRadius::same(RADIUS_MD))
}

/// Accessible on/off switch painted in the warm graphite + ochre palette.
pub fn toggle_switch(
    ui: &mut egui::Ui,
    palette: &Palette,
    on: &mut bool,
    accessible_name: &str,
) -> egui::Response {
    let desired = Vec2::new(40.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(desired, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), *on, accessible_name)
    });

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(resp.id, *on);
        let radius = rect.height() * 0.5;
        let bg = if *on {
            palette.primary_btn_bg.color32()
        } else if resp.hovered() {
            palette.bg_elevated.color32()
        } else {
            palette.bg_card.color32()
        };
        let border = if resp.has_focus() {
            Stroke::new(FOCUS_STROKE_WIDTH, palette.accent.color32())
        } else if *on {
            Stroke::new(1.0, palette.primary_btn_bg.color32())
        } else {
            Stroke::new(1.0, palette.border.color32())
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(radius.round() as u8),
            bg,
            border,
            egui::StrokeKind::Inside,
        );
        let knob_r = radius - 3.0;
        let cx = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let knob_color = if *on {
            palette.primary_btn_fg.color32()
        } else {
            palette.text_secondary.color32()
        };
        ui.painter()
            .circle_filled(Pos2::new(cx, rect.center().y), knob_r, knob_color);
    }
    resp
}

/// Paints the signature grease-pencil uneven loop in ochre (`#d9a416`), inspired by `assets/icon.svg`,
/// with a dark contrasting outline underneath so it remains legible on any photo.
pub fn paint_reviewed_mark(painter: &egui::Painter, rect: Rect, palette: &Palette) {
    // Normalized points along an uneven hand-drawn loop whose ends slightly cross (like icon.svg).
    const NORM_PTS: &[(f32, f32)] = &[
        (0.84, 0.36),
        (0.74, 0.16),
        (0.50, 0.10),
        (0.24, 0.17),
        (0.09, 0.36),
        (0.11, 0.62),
        (0.26, 0.82),
        (0.52, 0.89),
        (0.77, 0.82),
        (0.91, 0.62),
        (0.90, 0.38),
        (0.76, 0.18),
        (0.46, 0.12),
    ];
    let pts: Vec<Pos2> = NORM_PTS
        .iter()
        .map(|&(nx, ny)| {
            Pos2::new(
                rect.left() + nx * rect.width(),
                rect.top() + ny * rect.height(),
            )
        })
        .collect();

    let outline_w = (rect.width() * 0.22).clamp(3.6, 5.2);
    let stroke_w = (rect.width() * 0.12).clamp(2.0, 3.0);

    // Contrasting dark graphite halo so the ochre stroke reads clearly over light or busy photos.
    painter.add(egui::Shape::line(
        pts.clone(),
        Stroke::new(outline_w, Color32::from_rgba_unmultiplied(21, 21, 20, 210)),
    ));
    painter.add(egui::Shape::line(
        pts,
        Stroke::new(stroke_w, palette.mark_ochre.color32()),
    ));
}

pub(crate) fn build_visuals(dark: bool, p: &Palette) -> Visuals {
    let mut v = if dark {
        Visuals::dark()
    } else {
        Visuals::light()
    };
    v.panel_fill = p.bg_panel.color32();
    v.window_fill = p.bg_panel.color32();
    v.window_stroke = Stroke::new(1.0, p.border.color32());
    v.extreme_bg_color = p.bg_canvas.color32();
    v.text_edit_bg_color = Some(p.bg_canvas.color32());
    v.faint_bg_color = p.bg_card.color32();
    v.code_bg_color = p.bg_elevated.color32();
    v.override_text_color = Some(p.text_primary.color32());
    // egui's TextEdit styles hint_text via visuals.weak_text_color(); setting an opaque
    // palette colour keeps placeholder text at >= 4.5:1 contrast in both themes.
    v.weak_text_color = Some(p.text_hint.color32());
    v.hyperlink_color = p.accent.color32();
    v.warn_fg_color = p.accent.color32();
    v.error_fg_color = p.status_rejected_fg.color32();
    v.selection.bg_fill = p.accent_bg.color32();
    v.selection.stroke = Stroke::new(FOCUS_STROKE_WIDTH, p.accent.color32());
    v.slider_trailing_fill = true;

    // Dark theme uses surface steps + hairlines (no shadows). Light theme uses soft neutral shadows.
    if dark {
        v.window_shadow = Shadow::NONE;
        v.popup_shadow = Shadow::NONE;
    } else {
        v.window_shadow = Shadow {
            offset: [0, 4],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(18),
        };
        v.popup_shadow = Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(16),
        };
    }

    let radius = CornerRadius::same(RADIUS_MD);
    v.window_corner_radius = CornerRadius::same(RADIUS_LG);
    v.menu_corner_radius = CornerRadius::same(RADIUS_MD);

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

/// Configures typography (>= 13 px), 4 px spacing grid, and accessible Dark/Light visuals.
pub fn configure_styles(ctx: &egui::Context, preference: Theme) {
    let apply_style = |style: &mut egui::Style, dark: bool, p: &Palette| {
        style.visuals = build_visuals(dark, p);
        style.spacing.item_spacing = egui::vec2(SPACE_SM, SPACE_SM);
        style.spacing.button_padding = egui::vec2(SPACE_MD, SPACE_XS + 2.0);
        style.spacing.window_margin = egui::Margin::same(SPACE_LG as i8);
        style.spacing.menu_margin = egui::Margin::same(SPACE_XS as i8);
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
    crate::fonts::configure_cjk_fallback_fonts(ctx);
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
            for (role, fg) in [
                ("text_primary", p.text_primary),
                ("text_secondary", p.text_secondary),
                ("text_hint", p.text_hint),
                ("accent", p.accent),
                ("status_accepted_fg", p.status_accepted_fg),
                ("status_rejected_fg", p.status_rejected_fg),
                ("status_suggested_fg", p.status_suggested_fg),
            ] {
                let ratio = fg.contrast_ratio(bg);
                assert!(
                    ratio >= 4.5,
                    "{name}: {role} on {surf_name} contrast {ratio:.2} < 4.5"
                );
            }
        }

        let pairs = [
            ("text_primary on accent_bg", p.text_primary, p.accent_bg),
            ("primary_btn", p.primary_btn_fg, p.primary_btn_bg),
            ("chip_require", p.chip_require_fg, p.chip_require_bg),
            ("chip_any", p.chip_any_fg, p.chip_any_bg),
            ("chip_exclude", p.chip_exclude_fg, p.chip_exclude_bg),
            ("review_badge", p.review_badge_fg, p.review_badge_bg),
        ];
        for (label, fg, bg) in pairs {
            let ratio = fg.contrast_ratio(bg);
            assert!(ratio >= 4.5, "{name}: {label} contrast {ratio:.2} < 4.5");
        }
    }

    fn is_blue_or_violet(c: Color32) -> bool {
        // Every palette color in photosort is warm graphite, ochre, olive, or rust (b <= r and b <= g,
        // or neutral grey). Any color where blue noticeably exceeds both red and green is a stray
        // egui default blue/violet.
        let r = i32::from(c.r());
        let g = i32::from(c.g());
        let b = i32::from(c.b());
        b > r + 10 && b > g + 10
    }

    #[test]
    fn dark_and_light_palettes_meet_wcag_aa_contrast_ratio() {
        assert_palette_accessible("dark", &DARK_PALETTE);
        assert_palette_accessible("light", &LIGHT_PALETTE);
    }

    #[test]
    fn visuals_have_no_stray_blue_or_violet_in_either_theme() {
        for (name, dark, palette) in [
            ("dark", true, &DARK_PALETTE),
            ("light", false, &LIGHT_PALETTE),
        ] {
            let v = build_visuals(dark, palette);
            let colors = [
                ("panel_fill", v.panel_fill),
                ("window_fill", v.window_fill),
                ("extreme_bg_color", v.extreme_bg_color),
                ("faint_bg_color", v.faint_bg_color),
                ("code_bg_color", v.code_bg_color),
                ("hyperlink_color", v.hyperlink_color),
                ("warn_fg_color", v.warn_fg_color),
                ("error_fg_color", v.error_fg_color),
                ("selection.bg_fill", v.selection.bg_fill),
                ("selection.stroke", v.selection.stroke.color),
                ("noninteractive.bg", v.widgets.noninteractive.bg_fill),
                ("inactive.bg", v.widgets.inactive.bg_fill),
                ("hovered.bg", v.widgets.hovered.bg_fill),
                ("hovered.stroke", v.widgets.hovered.bg_stroke.color),
                ("active.bg", v.widgets.active.bg_fill),
                ("active.stroke", v.widgets.active.bg_stroke.color),
                ("open.bg", v.widgets.open.bg_fill),
                ("open.stroke", v.widgets.open.bg_stroke.color),
            ];
            for (label, c) in colors {
                assert!(
                    !is_blue_or_violet(c),
                    "{name}: {label} has stray blue/violet color {c:?}"
                );
            }
        }
    }

    #[test]
    fn text_edit_placeholder_visuals_meet_wcag_aa_contrast_in_both_themes() {
        for (name, dark, palette) in [
            ("dark", true, &DARK_PALETTE),
            ("light", false, &LIGHT_PALETTE),
        ] {
            let visuals = build_visuals(dark, palette);
            let hint = visuals.weak_text_color();
            let bg = visuals.text_edit_bg_color();
            assert_eq!(hint, palette.text_hint.color32(), "{name}: weak_text_color");
            assert_eq!(
                bg,
                palette.bg_canvas.color32(),
                "{name}: text_edit_bg_color"
            );
            assert_eq!(hint.a(), 255, "{name}: hint text must be opaque");
            let ratio =
                Rgb(hint.r(), hint.g(), hint.b()).contrast_ratio(Rgb(bg.r(), bg.g(), bg.b()));
            assert!(
                ratio >= 4.5,
                "{name}: placeholder hint on actual TextEdit background contrast {ratio:.2} < 4.5"
            );
        }
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

    #[test]
    fn embedded_icon_decodes_cleanly() {
        let icon = window_icon().expect("embedded icon.png should decode");
        assert_eq!(icon.width, 256);
        assert_eq!(icon.height, 256);
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
    }
}
