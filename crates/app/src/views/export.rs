use crate::export_flow::{
    ALL_LAYOUTS, ExportPhase, ExportSummary, ExportWizard, PreviewState, layout_description,
    layout_label, open_in_explorer, remember_export_dir,
};
use crate::model::format_bytes;
use crate::settings::Settings;
use crate::theme::{self, Palette};
use crate::worker::Notify;
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, RichText, ScrollArea, Stroke,
};
use export::Preview;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Draws the export wizard window when open, polling background preview/copy workers each frame.
pub fn draw_window(
    ctx: &egui::Context,
    palette: &Palette,
    slot: &mut Option<ExportWizard>,
    data_dir: &Path,
    settings: &mut Settings,
    banner_error: &mut Option<String>,
) {
    let Some(wizard) = slot.as_mut() else {
        return;
    };

    wizard.poll();

    let mut close_requested = false;
    if !wizard.is_running()
        && !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
    {
        close_requested = true;
    }

    let mut open_flag = true;
    let mut window = egui::Window::new("Exportar copias")
        .collapsible(false)
        .resizable(true)
        .min_width(480.0)
        .default_width(540.0);
    if !wizard.is_running() {
        window = window.open(&mut open_flag);
    }

    window.show(ctx, |ui| {
        draw_header(ui, palette, wizard);
        ui.add_space(theme::SPACE_SM);
        ui.separator();
        ui.add_space(theme::SPACE_SM);

        match wizard.phase().clone() {
            ExportPhase::Choose => {
                draw_choose_step(
                    ui,
                    ctx,
                    palette,
                    wizard,
                    data_dir,
                    settings,
                    banner_error,
                    &mut close_requested,
                );
            }
            ExportPhase::Preview(PreviewState::Computing) => {
                draw_preview_computing(ui, palette, wizard, &mut close_requested);
            }
            ExportPhase::Preview(PreviewState::Ready(preview)) => {
                draw_preview_ready(ui, ctx, palette, wizard, &preview, &mut close_requested);
            }
            ExportPhase::Running { done, total } => {
                draw_running_step(ui, palette, wizard, done, total);
            }
            ExportPhase::Done(summary) => {
                draw_done_step(ui, palette, &summary, banner_error, &mut close_requested);
            }
            ExportPhase::Error(message) => {
                draw_error_step(ui, palette, wizard, &message, &mut close_requested);
            }
        }
    });

    if (!open_flag && !wizard.is_running()) || close_requested {
        *slot = None;
    }
}

fn draw_header(ui: &mut egui::Ui, palette: &Palette, wizard: &ExportWizard) {
    let origin = if wizard.from_selection() {
        "de la selección actual"
    } else {
        "de los resultados filtrados actuales"
    };
    let count = wizard.item_count();
    let plural = if count == 1 { "imagen" } else { "imágenes" };
    ui.label(
        RichText::new(format!("Se exportarán {count} {plural} ({origin})."))
            .size(theme::FONT_BODY)
            .strong(),
    );
    ui.label(
        RichText::new(
            "Solo se crean copias verificadas con SHA-256. Los originales nunca se modifican ni se mueven.",
        )
        .size(theme::FONT_SMALL)
        .color(palette.text_secondary.color32()),
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_choose_step(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    palette: &Palette,
    wizard: &mut ExportWizard,
    data_dir: &Path,
    settings: &mut Settings,
    banner_error: &mut Option<String>,
    close_requested: &mut bool,
) {
    ui.label(RichText::new("Carpeta de destino").strong());
    ui.add_space(theme::SPACE_XS);

    let mut path_text = wizard.dest_input().to_string_lossy().into_owned();
    let mut picked_folder: Option<PathBuf> = None;
    ui.horizontal(|ui| {
        let edit_w = (ui.available_width() - 148.0).max(180.0);
        let edit = ui.add(
            egui::TextEdit::singleline(&mut path_text)
                .desired_width(edit_w)
                .hint_text("Elige una carpeta fuera de tus bibliotecas…"),
        );
        if edit.changed() {
            wizard.set_dest(PathBuf::from(path_text.trim()));
        }
        if ui.button("Elegir carpeta…").clicked() {
            let mut dialog =
                rfd::FileDialog::new().set_title("Elegir carpeta de destino para exportar");
            let current = wizard.dest_input();
            if !current.as_os_str().is_empty() && current.is_dir() {
                dialog = dialog.set_directory(current);
            }
            if let Some(folder) = dialog.pick_folder() {
                picked_folder = Some(folder);
            }
        }
    });
    if let Some(folder) = picked_folder {
        wizard.set_dest(folder);
    }

    if let Some(err) = wizard.validation_error() {
        ui.add_space(theme::SPACE_XS);
        draw_alert_box(ui, palette, err);
    }

    ui.add_space(theme::SPACE_MD);
    ui.label(RichText::new("Estructura de carpetas").strong());
    ui.add_space(theme::SPACE_XS);

    let mut chosen_layout = wizard.layout();
    egui::ComboBox::from_id_salt("export_layout_combo")
        .selected_text(layout_label(chosen_layout))
        .width(220.0)
        .show_ui(ui, |ui| {
            for option in ALL_LAYOUTS {
                ui.selectable_value(&mut chosen_layout, option, layout_label(option))
                    .on_hover_text(layout_description(option));
            }
        });
    if chosen_layout != wizard.layout() {
        wizard.set_layout(chosen_layout);
    }
    ui.label(
        RichText::new(layout_description(wizard.layout()))
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    );

    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);

    ui.horizontal(|ui| {
        if ui.button("Cerrar").clicked() {
            *close_requested = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_preview =
                wizard.item_count() > 0 && !wizard.dest_input().as_os_str().is_empty();
            if ui
                .add_enabled(can_preview, egui::Button::new("Calcular vista previa"))
                .clicked()
            {
                let roots = settings.valid_roots();
                let notify = repaint_notify(ctx);
                if let Ok(dest) = wizard.request_preview(&roots, Some(notify))
                    && let Err(e) = remember_export_dir(data_dir, settings, &dest)
                {
                    *banner_error = Some(format!(
                        "No se pudo guardar la última carpeta de exportación: {e}"
                    ));
                }
            }
        });
    });
}

fn draw_preview_computing(
    ui: &mut egui::Ui,
    palette: &Palette,
    wizard: &mut ExportWizard,
    close_requested: &mut bool,
) {
    ui.add_space(theme::SPACE_MD);
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(
            RichText::new("Calculando vista previa y espacio disponible…")
                .size(theme::FONT_BODY)
                .color(palette.text_secondary.color32()),
        );
    });
    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);
    ui.horizontal(|ui| {
        if ui.button("Volver").clicked() {
            wizard.back_to_choose();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Cerrar").clicked() {
                *close_requested = true;
            }
        });
    });
}

fn draw_preview_ready(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    palette: &Palette,
    wizard: &mut ExportWizard,
    preview: &Preview,
    close_requested: &mut bool,
) {
    ui.label(RichText::new("Vista previa de exportación").strong());
    ui.add_space(theme::SPACE_XS);

    card_frame(palette).show(ui, |ui| {
        info_row(
            ui,
            palette,
            "Destino",
            &wizard.dest_input().display().to_string(),
        );
        info_row(ui, palette, "Estructura", layout_label(wizard.layout()));
        ui.separator();
        info_row(ui, palette, "Archivos a copiar", &preview.files.to_string());
        info_row(ui, palette, "Tamaño total", &format_bytes(preview.bytes));
        let free_text = preview
            .free_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "Desconocido".into());
        info_row(ui, palette, "Espacio libre en destino", &free_text);
        info_row(
            ui,
            palette,
            "Se renombrarán por conflicto",
            &preview.renamed.to_string(),
        );
        info_row(
            ui,
            palette,
            "Ya presentes (se omitirán)",
            &preview.already_present.to_string(),
        );
        info_row(
            ui,
            palette,
            "Orígenes ilegibles",
            &preview.unreadable.to_string(),
        );
    });

    if !preview.fits() {
        ui.add_space(theme::SPACE_SM);
        let free_text = preview
            .free_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "0 B".into());
        draw_alert_box(
            ui,
            palette,
            &format!(
                "No hay espacio suficiente en el volumen de destino: se necesitan {} y solo hay {} disponibles. Libera espacio o elige otra carpeta.",
                format_bytes(preview.bytes),
                free_text
            ),
        );
    }

    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);

    ui.horizontal(|ui| {
        if ui.button("Volver").clicked() {
            wizard.back_to_choose();
        }
        if ui.button("Cerrar").clicked() {
            *close_requested = true;
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let can_export = wizard.can_start_export();
            if ui
                .add_enabled(can_export, egui::Button::new("Exportar"))
                .clicked()
            {
                let notify = repaint_notify(ctx);
                let _ = wizard.start_export(Some(notify));
            }
        });
    });
}

fn draw_running_step(
    ui: &mut egui::Ui,
    palette: &Palette,
    wizard: &mut ExportWizard,
    done: usize,
    total: usize,
) {
    let status = if wizard.is_cancel_requested() {
        "Cancelando exportación (se conservarán las copias ya verificadas y el manifiesto)…"
    } else {
        "Copiando y verificando cada archivo con SHA-256…"
    };
    ui.label(RichText::new(status).color(palette.text_secondary.color32()));
    ui.add_space(theme::SPACE_SM);

    let frac = if total == 0 {
        1.0
    } else {
        (done as f32 / total as f32).clamp(0.0, 1.0)
    };
    ui.add(
        egui::ProgressBar::new(frac)
            .text(format!("{done} / {total}"))
            .desired_width(ui.available_width()),
    );

    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);

    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if ui
            .add_enabled(!wizard.is_cancel_requested(), egui::Button::new("Cancelar"))
            .clicked()
        {
            wizard.cancel_export();
        }
    });
}

fn draw_done_step(
    ui: &mut egui::Ui,
    palette: &Palette,
    summary: &ExportSummary,
    banner_error: &mut Option<String>,
    close_requested: &mut bool,
) {
    let title = if summary.stopped {
        "Exportación detenida"
    } else {
        "Exportación completada"
    };
    let title_color = if summary.stopped {
        palette.status_suggested_fg.color32()
    } else {
        palette.status_accepted_fg.color32()
    };
    ui.label(
        RichText::new(title)
            .size(theme::FONT_HEADING)
            .strong()
            .color(title_color),
    );
    ui.add_space(theme::SPACE_XS);

    card_frame(palette).show(ui, |ui| {
        info_row(ui, palette, "Copiados", &summary.copied.to_string());
        info_row(
            ui,
            palette,
            "Ya presentes",
            &summary.already_present.to_string(),
        );
        info_row(ui, palette, "Fallidos", &summary.failed.len().to_string());
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Manifiestos:").color(palette.text_secondary.color32()));
            ui.label(RichText::new(summary.manifest_json_name()).monospace())
                .on_hover_text(summary.manifest_json.display().to_string());
            ui.label(RichText::new("y").color(palette.text_secondary.color32()));
            ui.label(RichText::new(summary.manifest_csv_name()).monospace())
                .on_hover_text(summary.manifest_csv.display().to_string());
        });
    });

    if !summary.failed.is_empty() {
        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new("Archivos fallidos:")
                .strong()
                .color(palette.status_rejected_fg.color32()),
        );
        ui.add_space(theme::SPACE_XS);
        ScrollArea::vertical()
            .id_salt("export_failed_list")
            .max_height(140.0)
            .show(ui, |ui| {
                for (path, reason) in &summary.failed {
                    ui.label(
                        RichText::new(format!("{} — {reason}", path.display()))
                            .size(theme::FONT_SMALL)
                            .color(palette.status_rejected_fg.color32()),
                    );
                }
            });
    }

    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);

    ui.horizontal(|ui| {
        if ui.button("Abrir carpeta").clicked()
            && let Err(e) = open_in_explorer(&summary.dest)
        {
            *banner_error = Some(format!("No se pudo abrir la carpeta de destino: {e}"));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Cerrar").clicked() {
                *close_requested = true;
            }
        });
    });
}

fn draw_error_step(
    ui: &mut egui::Ui,
    palette: &Palette,
    wizard: &mut ExportWizard,
    message: &str,
    close_requested: &mut bool,
) {
    draw_alert_box(ui, palette, message);
    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);
    ui.horizontal(|ui| {
        if wizard.item_count() > 0 && ui.button("Volver").clicked() {
            wizard.back_to_choose();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("Cerrar").clicked() {
                *close_requested = true;
            }
        });
    });
}

fn card_frame(palette: &Palette) -> Frame {
    Frame::new()
        .fill(palette.bg_card.color32())
        .stroke(Stroke::new(1.0, palette.border.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(Margin::same(theme::SPACE_MD as i8))
}

fn draw_alert_box(ui: &mut egui::Ui, palette: &Palette, message: &str) {
    Frame::new()
        .fill(palette.chip_exclude_bg.color32())
        .stroke(Stroke::new(1.0, palette.chip_exclude_fg.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
        .inner_margin(Margin::same(theme::SPACE_SM as i8))
        .show(ui, |ui| {
            ui.label(
                RichText::new(message)
                    .size(theme::FONT_BODY)
                    .color(palette.chip_exclude_fg.color32()),
            );
        });
}

fn info_row(ui: &mut egui::Ui, palette: &Palette, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(palette.text_secondary.color32()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value).strong());
        });
    });
}

fn repaint_notify(ctx: &egui::Context) -> Notify {
    let ctx_clone = ctx.clone();
    Arc::new(move || ctx_clone.request_repaint())
}
