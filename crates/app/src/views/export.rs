use crate::export_flow::{
    ALL_LAYOUTS, ExportPhase, ExportSummary, ExportWizard, PreviewState, layout_description,
    layout_label, move_confirmation_text, open_in_explorer, remember_export_dir,
};
use crate::model::{count_phrase, format_bytes, plural_word, truncate_middle};
use crate::settings::{self, Settings};
use crate::theme::{self, Palette};
use crate::worker::Notify;
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, RichText, ScrollArea, Stroke,
};
use export::{Operation, Preview};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const EXPORT_WINDOW_WIDTH: f32 = 520.0;

/// Draws the «Copiar o mover» window when open, polling its background workers each frame.
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
    let mut window = egui::Window::new("Copiar o mover")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_width(EXPORT_WINDOW_WIDTH)
        .max_width(EXPORT_WINDOW_WIDTH)
        .default_width(EXPORT_WINDOW_WIDTH);
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
                draw_preview_ready(
                    ui,
                    ctx,
                    palette,
                    wizard,
                    &preview,
                    data_dir,
                    settings,
                    banner_error,
                    &mut close_requested,
                );
            }
            ExportPhase::ConfirmMove(preview) => {
                draw_confirm_move(
                    ui,
                    ctx,
                    palette,
                    wizard,
                    &preview,
                    data_dir,
                    settings,
                    banner_error,
                );
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
    let (verb, promise) = match wizard.operation() {
        Operation::Copy => (
            plural_word(count, "copiará", "copiarán"),
            "Solo se crean copias verificadas con SHA-256. Los originales no se modifican ni se mueven.",
        ),
        Operation::Move => (
            plural_word(count, "moverá", "moverán"),
            "Nada se borra, se sobrescribe ni se vuelve a codificar. Cada movimiento se puede deshacer.",
        ),
    };
    ui.label(
        RichText::new(format!(
            "Se {verb} {} ({origin}).",
            count_phrase(count, "imagen", "imágenes")
        ))
        .size(theme::FONT_BODY)
        .strong(),
    );
    ui.label(
        RichText::new(promise)
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
    ui.horizontal(|ui| {
        for (operation, label) in [
            (Operation::Copy, "Copiar"),
            (Operation::Move, "Mover (cortar)"),
        ] {
            if ui.radio(wizard.operation() == operation, label).clicked() {
                wizard.set_operation(operation);
            }
        }
    });
    ui.add_space(theme::SPACE_MD);
    let moving = wizard.operation() == Operation::Move;
    ui.label(RichText::new("Carpeta de destino").strong());
    ui.add_space(theme::SPACE_XS);

    let mut path_text = wizard.dest_input().to_string_lossy().into_owned();
    let mut picked_folder: Option<PathBuf> = None;
    ui.horizontal(|ui| {
        let edit_w = (ui.available_width() - 148.0).max(180.0);
        let edit = theme::named(
            ui.add(
                egui::TextEdit::singleline(&mut path_text)
                    .desired_width(edit_w)
                    .hint_text(theme::hint_text(
                        palette,
                        if moving {
                            "Elige la carpeta de salida…"
                        } else {
                            "Elige una carpeta fuera de tus bibliotecas…"
                        },
                    )),
            ),
            "Carpeta de destino",
        );
        if wizard.take_focus_dest() {
            edit.request_focus();
        }
        if edit.changed() {
            wizard.set_dest(PathBuf::from(path_text.trim()));
        }
        if ui.button("Elegir carpeta…").clicked() {
            let mut dialog = rfd::FileDialog::new().set_title("Elegir carpeta de destino");
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
    let export_layout_combo_response = egui::ComboBox::from_id_salt("export_layout_combo")
        .selected_text(layout_label(chosen_layout))
        .width(220.0)
        .show_ui(ui, |ui| {
            for option in ALL_LAYOUTS {
                ui.selectable_value(&mut chosen_layout, option, layout_label(option))
                    .on_hover_text(layout_description(option));
            }
        })
        .response;
    theme::named(export_layout_combo_response, "Estructura de carpetas");
    if chosen_layout != wizard.layout() {
        wizard.set_layout(chosen_layout);
    }
    ui.label(
        RichText::new(layout_description(wizard.layout()))
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    );

    ui.add_space(theme::SPACE_MD);
    ui.label(RichText::new("Metadatos y etiquetas").strong());
    ui.add_space(theme::SPACE_XS);

    let mut write_xmp = wizard.write_xmp();
    if ui
        .checkbox(
            &mut write_xmp,
            "Guardar etiquetas en un archivo XMP junto a cada archivo del destino",
        )
        .changed()
    {
        wizard.set_write_xmp(write_xmp);
    }

    let mut embed_tags = wizard.embed_jpeg_tags();
    if ui
        .add_enabled(
            !moving,
            egui::Checkbox::new(
                &mut embed_tags,
                "Incrustar etiquetas en las copias JPEG (cambia sus bytes; el original no se toca)",
            ),
        )
        .changed()
    {
        wizard.set_embed_jpeg_tags(embed_tags);
    }
    if moving {
        ui.label(
            RichText::new("Al mover no se incrustan etiquetas: cambiaría el único ejemplar")
                .size(theme::FONT_SMALL)
                .color(palette.text_secondary.color32()),
        );
    }

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
            let clicked = ui
                .add_enabled(can_preview, egui::Button::new("Calcular vista previa"))
                .clicked();
            if clicked || (can_preview && enter_pressed(ctx)) {
                let roots = settings.valid_roots();
                let notify = repaint_notify(ctx);
                if let Ok(dest) = wizard.request_preview(&roots, data_dir, Some(notify))
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

#[allow(clippy::too_many_arguments)]
fn draw_preview_ready(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    palette: &Palette,
    wizard: &mut ExportWizard,
    preview: &Preview,
    data_dir: &Path,
    settings: &Settings,
    banner_error: &mut Option<String>,
    close_requested: &mut bool,
) {
    let moving = wizard.operation() == Operation::Move;
    ui.label(RichText::new("Vista previa").strong());
    ui.add_space(theme::SPACE_XS);

    card_frame(palette).show(ui, |ui| {
        let dest_str = wizard.dest_input().display().to_string();
        info_row_with_tooltip(
            ui,
            palette,
            "Destino",
            &truncate_middle(&dest_str, 42),
            &dest_str,
        );
        info_row(ui, palette, "Estructura", layout_label(wizard.layout()));
        if wizard.write_xmp() {
            info_row(ui, palette, "Archivo XMP", "Junto a cada archivo");
        }
        if wizard.embed_jpeg_tags() {
            info_row(ui, palette, "Etiquetas JPEG", "Incrustadas");
        }
        ui.separator();
        info_row(
            ui,
            palette,
            if moving {
                "Archivos a mover"
            } else {
                "Archivos a copiar"
            },
            &count_phrase(preview.files, "archivo", "archivos"),
        );
        info_row(ui, palette, "Tamaño total", &format_bytes(preview.bytes));
        if moving {
            ui.label(format!(
                "{} en el mismo disco (instantáneo) · {} a otro disco (copia y Papelera)",
                preview.same_volume, preview.other_volume
            ));
        }
        let free_text = preview
            .free_bytes
            .map(format_bytes)
            .unwrap_or_else(|| "Desconocido".into());
        info_row(ui, palette, "Espacio libre en destino", &free_text);
        info_row(
            ui,
            palette,
            plural_word(
                preview.renamed,
                "Se renombrará por conflicto",
                "Se renombrarán por conflicto",
            ),
            &count_phrase(preview.renamed, "archivo", "archivos"),
        );
        info_row(
            ui,
            palette,
            match (moving, preview.already_present) {
                (false, 1) => "Ya presente (se omitirá)",
                (false, _) => "Ya presentes (se omitirán)",
                (true, 1) => "Ya presente (se conserva el original)",
                (true, _) => "Ya presentes (se conservan los originales)",
            },
            &count_phrase(preview.already_present, "archivo", "archivos"),
        );
        info_row(
            ui,
            palette,
            plural_word(preview.unreadable, "Origen ilegible", "Orígenes ilegibles"),
            &count_phrase(preview.unreadable, "archivo", "archivos"),
        );
        info_row(
            ui,
            palette,
            plural_word(
                preview.blocked,
                "Bloqueado por un enlace",
                "Bloqueados por un enlace",
            ),
            &count_phrase(preview.blocked, "archivo", "archivos"),
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
                format_bytes(if moving {
                    preview.bytes_other_volume
                } else {
                    preview.bytes
                }),
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
            let (label, ready) = if moving {
                ("Mover…", wizard.can_confirm_move())
            } else {
                ("Copiar", wizard.can_start_export())
            };
            let clicked = ui.add_enabled(ready, egui::Button::new(label)).clicked();
            if clicked || (ready && enter_pressed(ctx)) {
                if moving {
                    let _ = wizard.request_move_confirmation();
                } else {
                    start(ctx, wizard, data_dir, settings, banner_error);
                }
            }
        });
    });
}

/// The second question before moving. Enter does not answer it: only the button does.
#[allow(clippy::too_many_arguments)]
fn draw_confirm_move(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    palette: &Palette,
    wizard: &mut ExportWizard,
    preview: &Preview,
    data_dir: &Path,
    settings: &Settings,
    banner_error: &mut Option<String>,
) {
    ui.label(
        RichText::new(move_confirmation_text(preview.files, preview.other_volume))
            .color(palette.text_primary.color32()),
    );
    ui.add_space(theme::SPACE_LG);
    ui.separator();
    ui.add_space(theme::SPACE_SM);
    ui.horizontal(|ui| {
        if ui.button("Volver").clicked() {
            wizard.back_to_preview();
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .add_enabled(wizard.can_start_export(), egui::Button::new("Mover"))
                .clicked()
            {
                start(ctx, wizard, data_dir, settings, banner_error);
            }
        });
    });
}

fn start(
    ctx: &egui::Context,
    wizard: &mut ExportWizard,
    data_dir: &Path,
    settings: &Settings,
    banner_error: &mut Option<String>,
) {
    let roots = settings.valid_roots();
    let started = settings::db_dest_path(data_dir, &roots)
        .map_err(|e| e.to_string())
        .and_then(|db| {
            let notify = repaint_notify(ctx);
            wizard.start_export(Some(notify), db, roots, data_dir.to_path_buf())
        });
    if let Err(e) = started {
        *banner_error = Some(e);
    }
}

fn draw_running_step(
    ui: &mut egui::Ui,
    palette: &Palette,
    wizard: &mut ExportWizard,
    done: usize,
    total: usize,
) {
    let status = match (wizard.operation(), wizard.is_cancel_requested()) {
        (Operation::Copy, true) => {
            "Cancelando la copia (se conservarán las copias ya verificadas y el manifiesto)…"
        }
        (Operation::Copy, false) => "Copiando y verificando cada archivo con SHA-256…",
        (Operation::Move, true) => "Cancelando (lo ya movido se puede deshacer)…",
        (Operation::Move, false) => "Moviendo; a otro disco, cada copia se verifica con SHA-256…",
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
    let moving = summary.operation == Operation::Move;
    let title = match (moving, summary.stopped) {
        (false, true) => "Copia detenida",
        (false, false) => "Copia completada",
        (true, true) => "Movimiento detenido",
        (true, false) => "Movimiento completado",
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
        let done = if moving {
            summary.moved
        } else {
            summary.copied
        };
        info_row(
            ui,
            palette,
            match (moving, done) {
                (false, 1) => "Copiado",
                (false, _) => "Copiados",
                (true, 1) => "Movido",
                (true, _) => "Movidos",
            },
            &count_phrase(done, "archivo", "archivos"),
        );
        if moving {
            info_row(
                ui,
                palette,
                "Copiados, original conservado",
                &count_phrase(summary.kept_original.len(), "archivo", "archivos"),
            );
        }
        info_row(
            ui,
            palette,
            match (moving, summary.already_present) {
                (false, 1) => "Ya presente",
                (false, _) => "Ya presentes",
                (true, _) => "Ya en el destino, original conservado",
            },
            &count_phrase(summary.already_present, "archivo", "archivos"),
        );
        info_row(
            ui,
            palette,
            plural_word(
                summary.blocked.len(),
                "Bloqueado por un enlace",
                "Bloqueados por un enlace",
            ),
            &count_phrase(summary.blocked.len(), "archivo", "archivos"),
        );
        info_row(
            ui,
            palette,
            plural_word(summary.failed.len(), "Fallido", "Fallidos"),
            &count_phrase(summary.failed.len(), "archivo", "archivos"),
        );
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

    draw_not_exported(
        ui,
        palette,
        "Copiados, pero el original sigue en su sitio:",
        "export_kept_list",
        &summary.kept_original,
    );
    draw_not_exported(
        ui,
        palette,
        "Archivos bloqueados por un enlace:",
        "export_blocked_list",
        &summary.blocked,
    );
    draw_not_exported(
        ui,
        palette,
        "Archivos fallidos:",
        "export_failed_list",
        &summary.failed,
    );

    if !summary.warnings.is_empty() {
        ui.add_space(theme::SPACE_SM);
        ui.label(
            RichText::new("Avisos:")
                .strong()
                .color(palette.status_suggested_fg.color32()),
        );
        ui.add_space(theme::SPACE_XS);
        ScrollArea::vertical()
            .id_salt("export_warning_list")
            .max_height(100.0)
            .show(ui, |ui| {
                for (path, warning) in &summary.warnings {
                    let name = path.file_name().unwrap_or(path.as_os_str());
                    ui.label(
                        RichText::new(format!("{} — {warning}", name.to_string_lossy()))
                            .size(theme::FONT_SMALL)
                            .color(palette.status_suggested_fg.color32()),
                    )
                    .on_hover_text(path.display().to_string());
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

fn draw_not_exported(
    ui: &mut egui::Ui,
    palette: &Palette,
    title: &str,
    id: &str,
    files: &[(PathBuf, String)],
) {
    if files.is_empty() {
        return;
    }
    ui.add_space(theme::SPACE_SM);
    ui.label(
        RichText::new(title)
            .strong()
            .color(palette.status_rejected_fg.color32()),
    );
    ui.add_space(theme::SPACE_XS);
    ScrollArea::vertical()
        .id_salt(id)
        .max_height(100.0)
        .show(ui, |ui| {
            for (path, reason) in files {
                ui.label(
                    RichText::new(format!("{} — {reason}", path.display()))
                        .size(theme::FONT_SMALL)
                        .color(palette.status_rejected_fg.color32()),
                );
            }
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
            ui.add(egui::Label::new(RichText::new(value).strong()).truncate());
        });
    });
}

fn info_row_with_tooltip(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    value: &str,
    tooltip: &str,
) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(palette.text_secondary.color32()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(RichText::new(value).strong()).truncate())
                .on_hover_text(tooltip);
        });
    });
}

/// Enter as the default action: only with no control focused, so that Enter on «Cerrar» or
/// «Volver» never starts an export. A held Enter repeats, and its repeats could chain the steps
/// into an export, so only a fresh press counts.
fn enter_pressed(ctx: &egui::Context) -> bool {
    !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| {
            let fresh = i.events.iter().any(is_fresh_enter);
            i.consume_key(egui::Modifiers::NONE, Key::Enter) && fresh
        })
}

fn is_fresh_enter(event: &egui::Event) -> bool {
    matches!(
        event,
        egui::Event::Key { key: Key::Enter, pressed: true, repeat: false, modifiers, .. }
            if modifiers.is_none()
    )
}

fn repaint_notify(ctx: &egui::Context) -> Notify {
    let ctx_clone = ctx.clone();
    Arc::new(move || ctx_clone.request_repaint())
}
