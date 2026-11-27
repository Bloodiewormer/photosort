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

/// A click area for a whole row, laid under the row's own buttons. Registered after them it would
/// take their clicks, so it reuses the row's box from the previous frame, which `draw_row`
/// remembers. Pointer only: it is not a Tab stop.
pub fn row_with_click_area<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    draw_row: impl FnOnce(&mut egui::Ui) -> egui::InnerResponse<R>,
) -> (R, Option<egui::Response>) {
    let area = ui
        .ctx()
        .data(|d| d.get_temp::<egui::Rect>(id))
        .map(|rect| ui.interact(rect, id, egui::Sense::CLICK));
    let row = draw_row(ui);
    ui.ctx().data_mut(|d| d.insert_temp(id, row.response.rect));
    (row.inner, area)
}

/// Width of `text` on a single line.
fn text_width(ui: &egui::Ui, text: impl Into<egui::WidgetText>, style: egui::TextStyle) -> f32 {
    text.into()
        .into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, style)
        .size()
        .x
}

/// In a wrapped row, pushes the buttons added next, in this order, to the end of the row, or of
/// a new row when they do not fit in what is left of it, so they never paint over the text
/// before them. Unlike a right-to-left layout, Tab visits them in reading order.
fn push_buttons_to_row_end(ui: &mut egui::Ui, labels: &[&str]) {
    let pad = 2.0 * ui.spacing().button_padding.x + ui.spacing().item_spacing.x;
    let needed: f32 = labels
        .iter()
        .map(|label| text_width(ui, *label, egui::TextStyle::Button) + pad)
        .sum();
    if ui.available_size_before_wrap().x < needed {
        ui.end_row();
    }
    ui.add_space((ui.available_size_before_wrap().x - needed).max(0.0));
}

/// In a horizontal `ui`, keeps room at the end for what `end` adds, so `start` shrinks instead
/// of pushing it out. Unlike a right-to-left layout, both parts are created, and so visited
/// by Tab, in reading order. `row` tells this row apart from the others in its list.
fn row_with_end<S, E>(
    ui: &mut egui::Ui,
    row: impl egui::AsIdSalt,
    start_layout: egui::Layout,
    start: impl FnOnce(&mut egui::Ui) -> S,
    end: impl FnOnce(&mut egui::Ui) -> E,
) -> (S, E) {
    // The end's size is known only once laid out: use the last one, and redo the frame when
    // it changes. Its height keeps both parts centred on the same line. Sibling rows share
    // their scope id, so the row's own key keeps each one's size apart.
    let key = ui.scope_id().with(row).with("row_end_size");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_pushed_to_the_row_end_keep_reading_order() {
        for width in [220.0, 700.0] {
            let order = tab_order(width, false, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Una cabecera con un título bastante largo");
                    push_buttons_to_row_end(ui, &["Aceptar", "Rechazar"]);
                    let _ = ui.button("Aceptar");
                    let _ = ui.button("Rechazar");
                });
            });
            assert_reading_order(&order);
            assert_eq!(order.len(), 2);
            assert!(
                order[1].max.x > width - 16.0 && order[0].min.y == order[1].min.y,
                "a {width} px los botones no van juntos al final: {order:?}"
            );
        }
    }
}
