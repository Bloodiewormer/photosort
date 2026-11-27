use crate::model::{count_phrase, format_timestamp_utc};
use crate::review_engine::StageKind;
use crate::theme::{self, Palette};
use crate::views::gallery::fit_rect;
use crate::worker::{ThumbPool, ThumbRequest};
use catalog::{AssetId, StageErrorRecord};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, ScrollArea, Sense,
    Stroke, TextureHandle, Vec2,
};
use std::collections::{HashMap, HashSet};

/// Every row has the same height, so only the ones on screen are laid out.
const ROW_CONTENT_HEIGHT: f32 = 64.0;
const ROW_HEIGHT: f32 = ROW_CONTENT_HEIGHT + 2.0 * theme::SPACE_MD + 2.0;

#[derive(Default)]
pub struct ErrorsViewOutcome {
    pub back: bool,
    pub retry_all: bool,
    pub retry_asset: Option<AssetId>,
    pub thumbs_ready: bool,
}

fn stage_label(stage: &str) -> &str {
    StageKind::from_stage_str(stage)
        .map(StageKind::label_es)
        .unwrap_or(stage)
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    errors: &[StageErrorRecord],
    thumb_pool: &ThumbPool,
    thumb_textures: &HashMap<String, TextureHandle>,
    thumb_failed: &HashSet<String>,
) -> ErrorsViewOutcome {
    let mut outcome = ErrorsViewOutcome::default();
    let mut visible_hashes: HashSet<String> = HashSet::new();
    let mut to_request: Vec<ThumbRequest> = Vec::new();

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(palette.bg_canvas.color32())
                .inner_margin(Margin::symmetric(
                    theme::SPACE_XL as i8,
                    theme::SPACE_LG as i8,
                )),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Errores de revisión ({})", errors.len()))
                        .size(theme::FONT_TITLE)
                        .color(palette.text_primary.color32()),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add_enabled(
                            !errors.is_empty(),
                            theme::primary_button(palette, "Reintentar todos"),
                        )
                        .clicked()
                    {
                        outcome.retry_all = true;
                    }
                    if ui.button("Volver").clicked() {
                        outcome.back = true;
                    }
                });
            });
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(
                    "Los fallos se omiten entre sesiones para no repetirse en bucle hasta que decidas reintentarlos",
                )
                .size(theme::FONT_SMALL)
                .color(palette.text_secondary.color32()),
            );
            ui.add_space(theme::SPACE_LG);

            if errors.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(theme::SPACE_2XL * 2.0);
                    ui.label(
                        RichText::new("No hay fotos con errores pendientes")
                            .color(palette.text_secondary.color32()),
                    );
                });
                return;
            }

            ui.spacing_mut().item_spacing.y = theme::SPACE_SM;
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show_rows(ui, ROW_HEIGHT, errors.len(), |ui, rows| {
                    for rec in &errors[rows] {
                        visible_hashes.insert(rec.sha256.clone());
                        if !thumb_textures.contains_key(&rec.sha256)
                            && !thumb_failed.contains(&rec.sha256)
                        {
                            to_request.push(ThumbRequest {
                                source: rec.path.clone(),
                                sha256: rec.sha256.clone(),
                            });
                        }

                        let filename = rec
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| rec.path.display().to_string());
                        let when = format_timestamp_utc(rec.last_at_secs.saturating_mul(1_000_000_000));
                        let attempts_note = if rec.attempts > 1 {
                            format!(" · {}", count_phrase(rec.attempts as usize, "intento", "intentos"))
                        } else {
                            String::new()
                        };

                        Frame::new()
                            .fill(palette.bg_card.color32())
                            .stroke(Stroke::new(1.0, palette.border.color32()))
                            .corner_radius(CornerRadius::same(theme::RADIUS_MD))
                            .inner_margin(Margin::same(theme::SPACE_MD as i8))
                            .show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.set_height(ROW_CONTENT_HEIGHT);
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal(|ui| {
                                    // Thumbnail (48x48)
                                    let (thumb_rect, _) =
                                        ui.allocate_exact_size(Vec2::splat(48.0), Sense::hover());
                                    ui.painter().rect(
                                        thumb_rect,
                                        CornerRadius::same(theme::RADIUS_SM),
                                        palette.bg_canvas.color32(),
                                        Stroke::new(1.0, palette.border.color32()),
                                        egui::StrokeKind::Inside,
                                    );
                                    if let Some(tex) = thumb_textures.get(&rec.sha256) {
                                        let fit = fit_rect(thumb_rect.shrink(2.0), tex.size_vec2());
                                        ui.painter().image(
                                            tex.id(),
                                            fit,
                                            Rect::from_min_max(
                                                egui::pos2(0.0, 0.0),
                                                egui::pos2(1.0, 1.0),
                                            ),
                                            Color32::WHITE,
                                        );
                                    } else {
                                        ui.painter().text(
                                            thumb_rect.center(),
                                            egui::Align2::CENTER_CENTER,
                                            "—",
                                            egui::FontId::proportional(theme::FONT_SMALL),
                                            palette.text_hint.color32(),
                                        );
                                    }

                                    ui.add_space(theme::SPACE_SM);

                                    // Right action button first so left column gets remaining width
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Reintentar").clicked() {
                                            outcome.retry_asset = Some(rec.asset);
                                        }
                                        ui.add_space(theme::SPACE_SM);
                                        ui.with_layout(Layout::top_down(Align::LEFT), |ui| {
                                            ui.add(
                                                egui::Label::new(
                                                    RichText::new(&filename)
                                                        .strong()
                                                        .color(palette.text_primary.color32()),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(rec.path.display().to_string());
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    RichText::new(stage_label(&rec.stage))
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.accent.color32()),
                                                );
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(format!("· {}", rec.reason))
                                                            .size(theme::FONT_SMALL)
                                                            .color(
                                                                palette.status_rejected_fg.color32(),
                                                            ),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(&rec.reason);
                                            });
                                            ui.label(
                                                RichText::new(format!("{when}{attempts_note}"))
                                                    .size(theme::FONT_SMALL)
                                                    .color(palette.text_hint.color32()),
                                            );
                                        });
                                    });
                                });
                            });
                    }
                });
        });

    outcome.thumbs_ready = visible_hashes.is_empty() || to_request.is_empty();
    thumb_pool.set_visible(&visible_hashes);
    for req in to_request {
        thumb_pool.request(req);
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::ThumbnailProvider;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Default)]
    struct Recording(Mutex<HashSet<String>>);

    impl ThumbnailProvider for Recording {
        fn get(&self, _: &Path, sha256: &str) -> io::Result<PathBuf> {
            self.0.lock().unwrap().insert(sha256.to_string());
            Err(io::Error::other("no thumbnail"))
        }
    }

    /// A disconnected GPU can leave thousands of failures; only the rows on screen may cost work.
    #[test]
    fn a_long_list_only_asks_for_the_thumbnails_on_screen() {
        let records: Vec<StageErrorRecord> = (0..2000)
            .map(|i| StageErrorRecord {
                asset: AssetId(i),
                path: PathBuf::from(format!("photo{i}.jpg")),
                sha256: format!("{i:064}"),
                stage: catalog::OCR_STAGE.into(),
                reason: "No se pudo leer el texto".into(),
                last_at_secs: 0,
                attempts: 2,
            })
            .collect();
        let provider = Arc::new(Recording::default());
        let pool = ThumbPool::new(2, Arc::clone(&provider) as _, None);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            draw(
                ui,
                &theme::DARK_PALETTE,
                &records,
                &pool,
                &HashMap::new(),
                &HashSet::new(),
            );
        });
        output.textures_delta.clear();
        std::thread::sleep(Duration::from_millis(500));
        let asked = provider.0.lock().unwrap().len();
        assert!((1..=30).contains(&asked), "asked for {asked} thumbnails");
    }
}
