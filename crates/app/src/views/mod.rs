pub mod context_menu;
pub mod duplicates;
pub mod errors;
pub mod export;
pub mod gallery;
pub mod inspector;
pub mod label_actions;
pub mod persons;
pub mod review;
pub mod settings;
pub mod shell;
pub mod tags;
pub mod works;

use eframe::egui;

/// Width of `text` on a single line.
fn text_width(ui: &egui::Ui, text: impl Into<egui::WidgetText>, style: egui::TextStyle) -> f32 {
    text.into()
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .x
}

/// In a wrapped row, moves to a new row when these buttons do not fit in what is left of it,
/// so a right-aligned group never paints over the text before it.
fn end_row_unless_buttons_fit(ui: &mut egui::Ui, labels: &[&str]) {
    let pad = 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x;
    let needed: f32 = labels
        .iter()
        .map(|label| text_width(ui, *label, egui::TextStyle::Button) + pad)
        .sum();
    if ui.available_size_before_wrap().x < needed {
        ui.end_row();
    }
}

/// In a horizontal `ui`, keeps room at the end for what `end` adds, so `start` shrinks instead
/// of pushing it out. Unlike a right-to-left layout, both parts are created, and so visited
/// by Tab, in reading order.
fn row_with_end<S, E>(
    ui: &mut egui::Ui,
    start_layout: egui::Layout,
    start: impl FnOnce(&mut egui::Ui) -> S,
    end: impl FnOnce(&mut egui::Ui) -> E,
) -> (S, E) {
    // The end's size is known only once laid out: use the last one, and redo the frame when
    // it changes. Its height keeps both parts centred on the same line.
    let key = ui.scope_id().with("row_end_size");
    let end_size = ui
        .data(|d| d.get_temp::<egui::Vec2>(key))
        .unwrap_or_default();
    let mut rect = ui.available_rect_before_wrap();
    rect.set_width((rect.width() - end_size.x - ui.spacing().item_spacing.x).max(0.0));
    rect.set_height(rect.height().max(end_size.y));
    let start = ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect).layout(start_layout),
        |ui| {
            ui.set_width(rect.width());
            start(ui)
        },
    );
    let end = ui.scope(end);
    let size = end.response.rect.size();
    if (size - end_size).abs().max_elem() > 0.5 {
        ui.data_mut(|d| d.insert_temp(key, size));
        ui.ctx().request_discard("row end width");
    }
    (start.inner, end.inner)
}

/// Rects of the widgets Tab visits in `row`, in visiting order.
#[cfg(test)]
fn tab_order(
    width: f32,
    screen_reader: bool,
    mut row: impl FnMut(&mut egui::Ui),
) -> Vec<egui::Rect> {
    use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
    let ctx = egui::Context::default();
    ctx.options_mut(|o| o.screen_reader = screen_reader);
    let mut step = |events| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(width, 400.0))),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, &mut row).textures_delta.clear();
        ctx.memory(|m| m.focused())
    };
    step(Vec::new());
    let tab = Event::Key {
        key: Key::Tab,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };
    let mut seen = Vec::new();
    while let Some(id) = step(vec![tab.clone()]) {
        if seen.contains(&id) || seen.len() > 20 {
            break;
        }
        seen.push(id);
    }
    seen.iter()
        .map(|id| ctx.read_response(*id).expect("respuesta del foco").rect)
        .collect()
}

/// Tab must read a row like text: left to right, then down.
#[cfg(test)]
fn assert_reading_order(order: &[egui::Rect]) {
    assert!(order.len() >= 2, "Tab solo visita {order:?}");
    for pair in order.windows(2) {
        assert!(
            pair[1].min.x > pair[0].min.x || pair[1].min.y > pair[0].max.y - 1.0,
            "Tab salta de {:?} a {:?}",
            pair[0],
            pair[1]
        );
    }
}
