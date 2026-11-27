use crate::model::ChipMode;
use crate::theme::{self, Palette};
use eframe::egui::{self, CornerRadius, Frame, Id, Key, Margin, Modifiers, Pos2, RichText, Stroke};

/// Action chosen from the photo context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhotoMenuAction {
    OpenViewer,
    RevealInExplorer,
    CopyPath,
    ReviewSelected,
    AddTag,
    CopyOcrText,
    FindSimilar,
    Export,
    ClearSelection,
}

#[derive(Debug, Clone)]
pub struct PhotoMenuState {
    pub pos: Pos2,
    pub focus_idx: usize,
    /// False until the first frame it is drawn in. The click that opens the menu happens in that
    /// frame, and the menu may be moved away from the pointer to fit the window.
    shown: bool,
    /// What had the keyboard focus when the menu opened; Escape gives it back.
    return_focus: Option<Id>,
}

impl PhotoMenuState {
    pub fn at(pos: Pos2) -> Self {
        Self {
            pos,
            focus_idx: 0,
            shown: false,
            return_focus: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PhotoMenuContext {
    pub selected_count: usize,
    pub all_reviewed: bool,
    pub has_ocr_text: bool,
    pub has_embedding: bool,
}

#[derive(Debug, Clone)]
struct MenuItem {
    action: PhotoMenuAction,
    label: String,
    group: u8,
}

fn build_photo_menu_items(ctx: PhotoMenuContext) -> Vec<MenuItem> {
    let mut items = Vec::with_capacity(9);

    // Group 0: open & path
    items.push(MenuItem {
        action: PhotoMenuAction::OpenViewer,
        label: "Abrir en el visor".into(),
        group: 0,
    });
    items.push(MenuItem {
        action: PhotoMenuAction::RevealInExplorer,
        label: "Mostrar en el Explorador".into(),
        group: 0,
    });
    items.push(MenuItem {
        action: PhotoMenuAction::CopyPath,
        label: "Copiar ruta".into(),
        group: 0,
    });

    // Group 1: catalog & review actions
    let review_label = match (ctx.all_reviewed, ctx.selected_count) {
        (true, n) if n > 1 => format!("Volver a revisar ({n})"),
        (true, _) => "Volver a revisar".into(),
        (false, n) if n > 1 => format!("Revisar ({n})"),
        (false, _) => "Revisar".into(),
    };
    items.push(MenuItem {
        action: PhotoMenuAction::ReviewSelected,
        label: review_label,
        group: 1,
    });
    items.push(MenuItem {
        action: PhotoMenuAction::AddTag,
        label: "Añadir etiqueta…".into(),
        group: 1,
    });
    if ctx.has_ocr_text {
        items.push(MenuItem {
            action: PhotoMenuAction::CopyOcrText,
            label: "Copiar texto leído".into(),
            group: 1,
        });
    }
    if ctx.has_embedding {
        items.push(MenuItem {
            action: PhotoMenuAction::FindSimilar,
            label: "Buscar parecidas".into(),
            group: 1,
        });
    }

    // Group 2: export & selection
    items.push(MenuItem {
        action: PhotoMenuAction::Export,
        label: "Exportar…".into(),
        group: 2,
    });
    items.push(MenuItem {
        action: PhotoMenuAction::ClearSelection,
        label: "Quitar selección".into(),
        group: 2,
    });

    items
}

/// Consumes the keyboard shortcuts that open a context menu: the menu key and Shift+F10.
pub fn menu_key_pressed(ctx: &egui::Context) -> bool {
    ctx.input_mut(|i| {
        i.consume_key(Modifiers::NONE, Key::Menu) || i.consume_key(Modifiers::SHIFT, Key::F10)
    })
}

/// Renders the photo context menu (opened by right-click, the menu key, Shift+F10, or
/// `--view menu`). Supports Up/Down arrow and Tab navigation, Enter/Space activation, Escape and
/// click-outside dismissal.
pub fn draw_photo_menu(
    ctx: &egui::Context,
    palette: &Palette,
    state: &mut Option<PhotoMenuState>,
    menu_ctx: PhotoMenuContext,
) -> Option<PhotoMenuAction> {
    let current = state.as_mut()?;

    let items = build_photo_menu_items(menu_ctx);
    if items.is_empty() {
        *state = None;
        return None;
    }
    current.focus_idx = current.focus_idx.min(items.len() - 1);
    if !current.shown {
        current.return_focus = ctx.memory(|m| m.focused());
    }
    let return_focus = current.return_focus;

    let mut chosen: Option<PhotoMenuAction> = None;
    let mut close = false;

    let mut escaped = false;
    ctx.input_mut(|i| {
        // Shift+Tab first: `consume_key` with no modifiers also matches Shift.
        if i.consume_key(Modifiers::NONE, Key::Escape) {
            close = true;
            escaped = true;
        } else if i.consume_key(Modifiers::SHIFT, Key::Tab) {
            current.focus_idx = (current.focus_idx + items.len() - 1) % items.len();
        } else if i.consume_key(Modifiers::NONE, Key::Tab)
            || i.consume_key(Modifiers::NONE, Key::ArrowDown)
        {
            current.focus_idx = (current.focus_idx + 1) % items.len();
        } else if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            current.focus_idx = if current.focus_idx == 0 {
                items.len() - 1
            } else {
                current.focus_idx - 1
            };
        } else if i.consume_key(Modifiers::NONE, Key::Enter)
            || i.consume_key(Modifiers::NONE, Key::Space)
        {
            chosen = Some(items[current.focus_idx].action);
            close = true;
        }
    });

    if close {
        *state = None;
        if escaped && let Some(id) = return_focus {
            ctx.memory_mut(|m| m.request_focus(id));
        }
        return chosen;
    }

    let screen = ctx.content_rect();
    let est_w = 228.0_f32;
    let est_h = (items.len() as f32 * 32.0 + 32.0).min(screen.height() - 16.0);
    let clamped_pos = Pos2::new(
        current
            .pos
            .x
            .clamp(screen.left() + 8.0, (screen.right() - est_w - 8.0).max(8.0)),
        current
            .pos
            .y
            .clamp(screen.top() + 8.0, (screen.bottom() - est_h - 8.0).max(8.0)),
    );

    let area_resp = egui::Area::new(Id::unique("photosort_photo_context_menu"))
        .order(egui::Order::Foreground)
        // A menu that fades in is unreadable for a moment, over whatever photo is behind it.
        .fade_in(false)
        .fixed_pos(clamped_pos)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.bg_panel.color32())
                .stroke(Stroke::new(1.0, palette.border.color32()))
                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                .inner_margin(Margin::same(theme::SPACE_XS as i8))
                .show(ui, |ui| {
                    ui.set_min_width(est_w);
                    ui.set_max_width(est_w);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 2.0);

                    let mut last_group = items[0].group;
                    for (idx, item) in items.iter().enumerate() {
                        if item.group != last_group {
                            ui.add_space(2.0);
                            ui.separator();
                            ui.add_space(2.0);
                            last_group = item.group;
                        }
                        let is_focused = idx == current.focus_idx;
                        let bg = if is_focused {
                            palette.accent_bg.color32()
                        } else {
                            egui::Color32::TRANSPARENT
                        };
                        let stroke = if is_focused {
                            Stroke::new(1.0, palette.accent.color32())
                        } else {
                            Stroke::NONE
                        };
                        let btn = egui::Button::new(
                            RichText::new(&item.label)
                                .size(theme::FONT_BODY)
                                .color(palette.text_primary.color32()),
                        )
                        .fill(bg)
                        .stroke(stroke)
                        .min_size(egui::vec2(est_w - 8.0, 32.0));
                        let resp = ui.add(btn);
                        // Real focus on the highlighted item, so a screen reader announces it.
                        if is_focused {
                            resp.request_focus();
                        }
                        if resp.hovered() {
                            current.focus_idx = idx;
                        }
                        if resp.clicked() {
                            chosen = Some(item.action);
                            close = true;
                        }
                    }
                });
        });

    let clicked_outside = ctx.input(|i| {
        (i.pointer.primary_clicked() || i.pointer.secondary_clicked())
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !area_resp.response.rect.contains(p))
    });
    if clicked_outside && current.shown {
        close = true;
    }
    current.shown = true;

    if close {
        *state = None;
    }

    chosen
}

/// Attaches the filter chip context menu (Exigir, Incluir alguna, Excluir, Quitar filtro).
pub fn attach_chip_context_menu(
    resp: &egui::Response,
    current_mode: Option<ChipMode>,
) -> Option<Option<ChipMode>> {
    let mut next: Option<Option<ChipMode>> = None;
    resp.context_menu(|ui| {
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Require),
                egui::Button::new("Exigir"),
            )
            .clicked()
        {
            next = Some(Some(ChipMode::Require));
            ui.close();
        }
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Any),
                egui::Button::new("Incluir alguna"),
            )
            .clicked()
        {
            next = Some(Some(ChipMode::Any));
            ui.close();
        }
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Exclude),
                egui::Button::new("Excluir"),
            )
            .clicked()
        {
            next = Some(Some(ChipMode::Exclude));
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(current_mode.is_some(), egui::Button::new("Quitar filtro"))
            .clicked()
        {
            next = Some(None);
            ui.close();
        }
    });
    next
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real right click, driven through the UI library without a window: the click that opens
    /// the menu must not be taken for a click outside it, wherever the pointer is.
    #[test]
    fn the_right_click_that_opens_the_menu_does_not_close_it() {
        use eframe::egui::{Event, PointerButton, RawInput, Rect};
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1280.0, 820.0));
        let menu_ctx = PhotoMenuContext {
            selected_count: 1,
            all_reviewed: true,
            has_ocr_text: true,
            has_embedding: true,
        };
        for at in [
            Pos2::new(400.0, 300.0),
            Pos2::new(1275.0, 815.0),
            Pos2::new(1275.0, 40.0),
            Pos2::new(5.0, 815.0),
        ] {
            let ctx = egui::Context::default();
            let mut state = None;
            let frame = |events: Vec<Event>, state: &mut Option<PhotoMenuState>| {
                let input = RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                };
                let mut output = ctx.run_ui(input, |ui| {
                    let ctx = ui.ctx().clone();
                    // Like the gallery: the menu opens in the frame the button is released.
                    if ctx.input(|i| i.pointer.secondary_clicked()) {
                        *state = Some(PhotoMenuState::at(at));
                    }
                    draw_photo_menu(&ctx, &theme::DARK_PALETTE, state, menu_ctx);
                });
                // Nothing is painted here.
                output.textures_delta.clear();
            };
            let button = |pressed| Event::PointerButton {
                pos: at,
                button: PointerButton::Secondary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            frame(vec![Event::PointerMoved(at)], &mut state);
            frame(vec![button(true)], &mut state);
            frame(vec![button(false)], &mut state);
            assert!(state.is_some(), "closed as it opened at {at:?}");
            frame(Vec::new(), &mut state);
            assert!(state.is_some(), "closed on the next frame at {at:?}");
            let away = Pos2::new(640.0, 10.0);
            for pressed in [true, false] {
                let click = Event::PointerButton {
                    pos: away,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                };
                frame(vec![Event::PointerMoved(away), click], &mut state);
            }
            assert!(state.is_none(), "a click outside left it open at {at:?}");
        }
    }

    #[test]
    fn photo_menu_has_at_most_three_groups_and_hides_conditional_items() {
        let minimal = build_photo_menu_items(PhotoMenuContext {
            selected_count: 1,
            all_reviewed: false,
            has_ocr_text: false,
            has_embedding: false,
        });
        assert!(
            !minimal
                .iter()
                .any(|i| i.action == PhotoMenuAction::CopyOcrText)
        );
        assert!(
            !minimal
                .iter()
                .any(|i| i.action == PhotoMenuAction::FindSimilar)
        );
        assert!(minimal.iter().all(|i| i.group <= 2));
        assert!(minimal.iter().any(|i| i.label == "Revisar"));

        let full = build_photo_menu_items(PhotoMenuContext {
            selected_count: 3,
            all_reviewed: true,
            has_ocr_text: true,
            has_embedding: true,
        });
        assert!(
            full.iter()
                .any(|i| i.action == PhotoMenuAction::CopyOcrText)
        );
        assert!(
            full.iter()
                .any(|i| i.action == PhotoMenuAction::FindSimilar)
        );
        assert!(full.iter().any(|i| i.label == "Volver a revisar (3)"));
    }
}
