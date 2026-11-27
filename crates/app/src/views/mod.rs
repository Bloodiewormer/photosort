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
