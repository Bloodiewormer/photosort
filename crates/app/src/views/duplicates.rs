use crate::model::{format_bytes, format_timestamp_utc};
use crate::theme::{self, Palette};
use crate::worker::{ThumbPool, ThumbRequest};
use catalog::DuplicateGroup;
use eframe::egui::{
    self, Color32, CornerRadius, Frame, Margin, RichText, ScrollArea, Sense, Stroke, TextureHandle,
    Vec2,
};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::Command;

const DUP_CARD_WIDTH: f32 = 260.0;
const DUP_THUMB_HEIGHT: f32 = 132.0;

pub const READ_ONLY_NOTE: &str = "Nota: photosort solo muestra duplicados para revisión. Este programa nunca borra, mueve ni renombra ningún archivo.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DuplicatesSummary {
    pub group_count: usize,
    pub total_copies: usize,
    pub redundant_copies: usize,
    pub wasted_bytes: u64,
}

pub fn group_wasted_bytes(group: &DuplicateGroup) -> u64 {
    group
        .size
        .saturating_mul(group.entries.len().saturating_sub(1) as u64)
}

pub fn summarize(groups: &[DuplicateGroup]) -> DuplicatesSummary {
    let mut summary = DuplicatesSummary {
        group_count: groups.len(),
        ..Default::default()
    };
    for g in groups {
        let copies = g.entries.len();
        summary.total_copies = summary.total_copies.saturating_add(copies);
        summary.redundant_copies = summary
            .redundant_copies
            .saturating_add(copies.saturating_sub(1));
        summary.wasted_bytes = summary.wasted_bytes.saturating_add(group_wasted_bytes(g));
    }
    summary
}

pub fn sort_groups(groups: &mut [DuplicateGroup]) {
    for g in groups.iter_mut() {
        g.entries.sort_by(|a, b| a.path.cmp(&b.path));
    }
    groups.sort_by(|a, b| {
        let wa = u128::from(a.size) * u128::from(a.entries.len().saturating_sub(1) as u64);
        let wb = u128::from(b.size) * u128::from(b.entries.len().saturating_sub(1) as u64);
        Reverse(wa)
            .cmp(&Reverse(wb))
            .then_with(|| a.sha256.cmp(&b.sha256))
    });
}

pub fn format_summary_line(summary: &DuplicatesSummary) -> String {
    let groups_word = if summary.group_count == 1 {
        "grupo"
    } else {
        "grupos"
    };
    let copies_word = if summary.redundant_copies == 1 {
        "copia redundante"
    } else {
        "copias redundantes"
    };
    format!(
        "{} {groups_word} · {} {copies_word} · {} de espacio duplicado",
        summary.group_count,
        summary.redundant_copies,
        format_bytes(summary.wasted_bytes),
    )
}

pub fn format_group_heading(group: &DuplicateGroup) -> String {
    format!(
        "{} copias · {} c/u · {} duplicados",
        group.entries.len(),
        format_bytes(group.size),
        format_bytes(group_wasted_bytes(group)),
    )
}

/// Builds the arguments for `explorer.exe /select,<path>` without lossy UTF-8 conversion.
pub fn explorer_select_args(path: &Path) -> [OsString; 2] {
    [OsString::from("/select,"), path.as_os_str().to_os_string()]
}

/// Reveals `path` in Windows File Explorer without modifying any file.
pub fn reveal_in_explorer(path: &Path) -> io::Result<()> {
    let args = explorer_select_args(path);
    Command::new("explorer.exe").args(&args).spawn()?;
    Ok(())
}

/// Renders the read-only duplicate review tab. Returns an error message if opening Explorer fails.
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    groups: &[DuplicateGroup],
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> Option<String> {
    let summary = summarize(groups);
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();
    let mut open_error: Option<String> = None;

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::same(theme::SPACE_MD as i8)),
        )
        .show(ui, |ui| {
            // Top summary and read-only guarantee notice.
            Frame::new()
                .fill(palette.bg_panel.color32())
                .stroke(Stroke::new(1.0, palette.border.color32()))
                .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                .inner_margin(Margin::same(theme::SPACE_MD as i8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal_wrapped(|ui| {
                        ui.label(
                            RichText::new("Duplicados exactos")
                                .size(theme::FONT_HEADING)
                                .strong(),
                        );
                        ui.separator();
                        ui.label(
                            RichText::new(format_summary_line(&summary))
                                .size(theme::FONT_BODY)
                                .strong(),
                        );
                    });
                    ui.add_space(theme::SPACE_XS);
                    ui.label(
                        RichText::new(READ_ONLY_NOTE)
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                });

            ui.add_space(theme::SPACE_MD);

            if groups.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_XL * 2.0);
                    ui.label(
                        RichText::new("No se encontraron imágenes duplicadas en el catálogo.")
                            .size(theme::FONT_BODY)
                            .color(palette.text_secondary.color32()),
                    );
                });
                return;
            }

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (group_idx, group) in groups.iter().enumerate() {
                        Frame::new()
                            .fill(palette.bg_panel.color32())
                            .stroke(Stroke::new(1.0, palette.border.color32()))
                            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                            .inner_margin(Margin::same(theme::SPACE_MD as i8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "Grupo {} — {}",
                                            group_idx + 1,
                                            format_group_heading(group)
                                        ))
                                        .size(theme::FONT_BODY)
                                        .strong(),
                                    );
                                    let short_sha = if group.sha256.len() > 12 {
                                        format!("SHA-256: {}...", &group.sha256[..12])
                                    } else {
                                        format!("SHA-256: {}", group.sha256)
                                    };
                                    ui.label(
                                        RichText::new(short_sha)
                                            .size(theme::FONT_SMALL)
                                            .monospace()
                                            .color(palette.text_secondary.color32()),
                                    )
                                    .on_hover_text(&group.sha256);
                                });
                                ui.add_space(theme::SPACE_SM);

                                ui.horizontal_wrapped(|ui| {
                                    for entry in &group.entries {
                                        visible_hashes.insert(entry.sha256.clone());
                                        if !thumb_textures.contains_key(&entry.sha256)
                                            && !thumb_failed.contains(&entry.sha256)
                                        {
                                            to_request.push(ThumbRequest {
                                                source: entry.path.clone(),
                                                sha256: entry.sha256.clone(),
                                            });
                                        }

                                        Frame::new()
                                            .fill(palette.bg_card.color32())
                                            .stroke(Stroke::new(1.0, palette.border.color32()))
                                            .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                            .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                            .show(ui, |ui| {
                                                ui.set_width(DUP_CARD_WIDTH);
                                                let (thumb_rect, _) = ui.allocate_exact_size(
                                                    Vec2::new(DUP_CARD_WIDTH, DUP_THUMB_HEIGHT),
                                                    Sense::hover(),
                                                );
                                                draw_thumb_box(
                                                    ui,
                                                    palette,
                                                    thumb_rect,
                                                    thumb_textures.get(&entry.sha256),
                                                    thumb_failed.contains(&entry.sha256),
                                                );
                                                ui.add_space(theme::SPACE_XS);

                                                let filename = entry
                                                    .path
                                                    .file_name()
                                                    .map(|n| n.to_string_lossy().into_owned())
                                                    .unwrap_or_else(|| {
                                                        entry.path.display().to_string()
                                                    });
                                                ui.label(
                                                    RichText::new(filename)
                                                        .size(theme::FONT_BODY)
                                                        .strong(),
                                                );
                                                ui.label(
                                                    RichText::new(entry.path.display().to_string())
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.text_secondary.color32()),
                                                );

                                                let date_str = entry
                                                    .modified_ns
                                                    .map(format_timestamp_utc)
                                                    .unwrap_or_else(|| "Desconocida".into());
                                                ui.label(
                                                    RichText::new(format!(
                                                        "{} · {}",
                                                        format_bytes(entry.size),
                                                        date_str
                                                    ))
                                                    .size(theme::FONT_SMALL)
                                                    .color(palette.text_secondary.color32()),
                                                );
                                                ui.add_space(theme::SPACE_XS);

                                                if ui.button("Mostrar en el Explorador").clicked()
                                                    && let Err(e) = reveal_in_explorer(&entry.path)
                                                {
                                                    open_error = Some(format!(
                                                        "No se pudo abrir el Explorador: {e}"
                                                    ));
                                                }
                                            });
                                    }
                                });
                            });
                        ui.add_space(theme::SPACE_MD);
                    }
                });
        });

    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }

    open_error
}

fn draw_thumb_box(
    ui: &egui::Ui,
    palette: &Palette,
    rect: egui::Rect,
    texture: Option<&TextureHandle>,
    failed: bool,
) {
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(theme::RADIUS_SM),
        palette.bg_canvas.color32(),
    );
    if let Some(tex) = texture {
        let fit = fit_rect(rect, tex.size_vec2());
        painter.image(
            tex.id(),
            fit,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    } else {
        let placeholder = if failed {
            "Sin miniatura"
        } else {
            "Cargando..."
        };
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            placeholder,
            egui::FontId::proportional(theme::FONT_SMALL),
            palette.text_secondary.color32(),
        );
    }
}

fn fit_rect(bounds: egui::Rect, content: Vec2) -> egui::Rect {
    if content.x <= 0.0 || content.y <= 0.0 || bounds.width() <= 0.0 || bounds.height() <= 0.0 {
        return bounds;
    }
    let scale = (bounds.width() / content.x)
        .min(bounds.height() / content.y)
        .min(1.0);
    let size = content * scale;
    egui::Rect::from_center_size(bounds.center(), size)
}
