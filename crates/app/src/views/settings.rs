use crate::model::{count_phrase, library_display_name};
use crate::model_dirs;
use crate::review_engine::{EngineSnapshot, StageKind};
use crate::settings::{OcrDevice, Settings, Theme};
use crate::theme::{self, Palette};
use crate::views::duplicates::reveal_in_explorer;
use eframe::egui::{
    self, Align, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Vec2,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const MAX_COLUMN_WIDTH: f32 = 720.0;

#[derive(Default)]
pub struct SettingsViewState {
    pub confirm_remove_root: Option<PathBuf>,
    pub confirm_clear_face_data: bool,
    pub vision_dir_error: Option<String>,
    pub ocr_dir_error: Option<String>,
}

#[derive(Default)]
pub struct SettingsOutcome {
    pub add_folder: bool,
    pub remove_folder: Option<PathBuf>,
    pub scan_changes_now: bool,
    pub open_errors: bool,
    pub settings_changed: bool,
    pub theme_changed: bool,
    pub clear_face_grouping: bool,
    pub start_face_grouping: bool,
    pub error: Option<String>,
}

#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    settings: &mut Settings,
    state: &mut SettingsViewState,
    root_counts: &HashMap<PathBuf, usize>,
    engine_snapshot: &EngineSnapshot,
    error_count: usize,
    data_dir: &Path,
    is_demo: bool,
) -> SettingsOutcome {
    let mut outcome = SettingsOutcome::default();

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
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let avail = ui.available_width();
                    let col_w = avail.min(MAX_COLUMN_WIDTH);
                    let side_pad = ((avail - col_w) * 0.5).max(0.0);

                    ui.horizontal(|ui| {
                        if side_pad > 0.0 {
                            ui.add_space(side_pad);
                        }
                        ui.vertical(|ui| {
                            ui.set_width(col_w);

                            ui.label(
                                RichText::new("Configuración")
                                    .size(theme::FONT_TITLE)
                                    .color(palette.text_primary.color32()),
                            );
                            ui.add_space(theme::SPACE_XL);

                            // 1. Carpetas con fotos
                            section_heading(ui, palette, "Carpetas con fotos");
                            section_card(ui, palette, |ui| {
                                if settings.library_roots.is_empty() {
                                    ui.label(
                                        RichText::new("No hay carpetas añadidas todavía")
                                            .color(palette.text_secondary.color32()),
                                    );
                                } else {
                                    let roots_clone = settings.library_roots.clone();
                                    for root in &roots_clone {
                                        let name = library_display_name(root);
                                        let count = root_counts.get(root).copied().unwrap_or(0);
                                        let full_path = root.display().to_string();

                                        let row_resp = Frame::new()
                                            .fill(palette.bg_card.color32())
                                            .stroke(Stroke::new(1.0, palette.border.color32()))
                                            .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                            .inner_margin(Margin::symmetric(
                                                theme::SPACE_MD as i8,
                                                theme::SPACE_SM as i8,
                                            ))
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        RichText::new(&name).strong().color(
                                                            palette.text_primary.color32(),
                                                        ),
                                                    );
                                                    ui.with_layout(
                                                        Layout::right_to_left(Align::Center),
                                                        |ui| {
                                                            if ui.button("Quitar").clicked() {
                                                                state.confirm_remove_root =
                                                                    Some(root.clone());
                                                            }
                                                            ui.label(
                                                                RichText::new(count_phrase(
                                                                    count, "foto", "fotos",
                                                                ))
                                                                .size(theme::FONT_SMALL)
                                                                .monospace()
                                                                .color(palette.text_hint.color32()),
                                                            );
                                                        },
                                                    );
                                                });
                                                ui.add(
                                                    egui::Label::new(
                                                        RichText::new(&full_path)
                                                            .size(theme::FONT_SMALL)
                                                            .color(
                                                                palette.text_secondary.color32(),
                                                            ),
                                                    )
                                                    .truncate(),
                                                )
                                                .on_hover_text(&full_path);
                                            })
                                            .response
                                            // Right click only; «Quitar» is the keyboard path.
                                            .interact(egui::Sense::CLICK);

                                        row_resp.context_menu(|ui| {
                                            if ui.button("Mostrar en el Explorador").clicked() {
                                                if let Err(e) = reveal_in_explorer(root) {
                                                    outcome.error = Some(format!(
                                                        "No se pudo abrir el Explorador: {e}"
                                                    ));
                                                }
                                                ui.close();
                                            }
                                            if ui.button("Quitar de la app").clicked() {
                                                state.confirm_remove_root = Some(root.clone());
                                                ui.close();
                                            }
                                        });

                                        if state.confirm_remove_root.as_ref() == Some(root) {
                                            ui.add_space(theme::SPACE_XS);
                                            Frame::new()
                                                .fill(palette.chip_exclude_bg.color32())
                                                .stroke(Stroke::new(
                                                    1.0,
                                                    palette.status_rejected_fg.color32(),
                                                ))
                                                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                                .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        RichText::new(format!(
                                                            "¿Quitar «{name}» de photosort? Solo deja de mostrarse en la aplicación; ningún archivo se borra ni se modifica. Si la vuelves a añadir, tus etiquetas y decisiones se conservarán."
                                                        ))
                                                        .size(theme::FONT_SMALL)
                                                        .color(palette.chip_exclude_fg.color32()),
                                                    );
                                                    ui.add_space(theme::SPACE_XS);
                                                    ui.horizontal(|ui| {
                                                        if ui.button("Quitar de la app").clicked() {
                                                            outcome.remove_folder =
                                                                Some(root.clone());
                                                            state.confirm_remove_root = None;
                                                        }
                                                        if ui.button("Cancelar").clicked() {
                                                            state.confirm_remove_root = None;
                                                        }
                                                    });
                                                });
                                        }
                                        ui.add_space(theme::SPACE_SM);
                                    }
                                }

                                ui.add_space(theme::SPACE_XS);
                                ui.horizontal(|ui| {
                                    if ui
                                        .add(theme::primary_button(palette, "Añadir carpeta…"))
                                        .clicked()
                                    {
                                        outcome.add_folder = true;
                                    }
                                    let can_scan = !settings.library_roots.is_empty() && !is_demo;
                                    if ui
                                        .add_enabled(
                                            can_scan,
                                            egui::Button::new("Buscar cambios ahora"),
                                        )
                                        .clicked()
                                    {
                                        outcome.scan_changes_now = true;
                                    }
                                });
                            });

                            ui.add_space(theme::SPACE_XL);

                            // 2. Revisión en segundo plano
                            section_heading(ui, palette, "Revisión en segundo plano");
                            section_card(ui, palette, |ui| {
                                if switch_row(
                                    ui,
                                    palette,
                                    "Revisar fotos en segundo plano",
                                    "Busca cambios al iniciar y revisa las fotos pendientes por lotes",
                                    &mut settings.background_review,
                                ) {
                                    outcome.settings_changed = true;
                                }

                                ui.add_space(theme::SPACE_SM);
                                ui.separator();
                                ui.add_space(theme::SPACE_SM);

                                if switch_row(
                                    ui,
                                    palette,
                                    "Reglas rápidas",
                                    "Detecta capturas, WhatsApp, GIF y metadatos de cámara sin modelos",
                                    &mut settings.stage_rules,
                                ) {
                                    outcome.settings_changed = true;
                                }
                                ui.add_space(theme::SPACE_SM);

                                let vision_note = if settings.vision_models_dir.is_none() {
                                    "Clasifica tipo de imagen y rostros · al activar se pedirá la carpeta de modelos"
                                } else {
                                    "Clasifica tipo de imagen y rostros con modelos locales"
                                };
                                let prev_vision = settings.stage_vision;
                                if switch_row(
                                    ui,
                                    palette,
                                    "Reconocer imágenes",
                                    vision_note,
                                    &mut settings.stage_vision,
                                ) {
                                    if settings.stage_vision
                                        && !prev_vision
                                        && settings.vision_models_dir.is_none()
                                    {
                                        if let Some(dir) = pick_model_folder(
                                            "Elegir carpeta de modelos de visión",
                                            None,
                                        ) {
                                            apply_vision_pick(
                                                settings,
                                                state,
                                                &dir,
                                                &mut outcome.settings_changed,
                                            );
                                        } else {
                                            settings.stage_vision = false;
                                        }
                                    } else {
                                        outcome.settings_changed = true;
                                    }
                                }
                                ui.add_space(theme::SPACE_SM);

                                let ocr_note = if settings.ocr_models_dir.is_none() {
                                    "Lee texto y busca obras conocidas · al activar se pedirá la carpeta de modelos OCR"
                                } else {
                                    "Lee texto y busca obras conocidas · es la etapa más lenta"
                                };
                                let prev_ocr = settings.stage_ocr;
                                if switch_row(
                                    ui,
                                    palette,
                                    "Leer texto",
                                    ocr_note,
                                    &mut settings.stage_ocr,
                                ) {
                                    if settings.stage_ocr
                                        && !prev_ocr
                                        && settings.ocr_models_dir.is_none()
                                    {
                                        if let Some(dir) =
                                            pick_model_folder("Elegir carpeta de modelos OCR", None)
                                        {
                                            apply_ocr_pick(
                                                settings,
                                                state,
                                                &dir,
                                                &mut outcome.settings_changed,
                                            );
                                        } else {
                                            settings.stage_ocr = false;
                                        }
                                    } else {
                                        outcome.settings_changed = true;
                                    }
                                }
                                ui.add_space(theme::SPACE_SM);

                                let prev_grouping = settings.stage_face_grouping;
                                let grouping_note = "Agrupa rostros similares con SFace local · al activar se calcula para rostros ya detectados";
                                if switch_row(
                                    ui,
                                    palette,
                                    "Agrupar rostros",
                                    grouping_note,
                                    &mut settings.stage_face_grouping,
                                ) {
                                    outcome.settings_changed = true;
                                    if settings.stage_face_grouping && !prev_grouping {
                                        outcome.start_face_grouping = true;
                                    }
                                }
                                ui.add_space(theme::SPACE_MD);

                                ui.horizontal(|ui| {
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("Borrar datos de rostros")
                                                    .size(theme::FONT_SMALL)
                                                    .color(palette.status_rejected_fg.color32()),
                                            )
                                            .stroke(Stroke::new(
                                                1.0,
                                                palette.status_rejected_fg.color32(),
                                            ))
                                            .corner_radius(CornerRadius::same(theme::RADIUS_SM)),
                                        )
                                        .on_hover_text("Borra grupos, nombres y embeddings de rostros calculados")
                                        .clicked()
                                    {
                                        state.confirm_clear_face_data = true;
                                    }
                                });

                                if state.confirm_clear_face_data {
                                    ui.add_space(theme::SPACE_XS);
                                    Frame::new()
                                        .fill(palette.chip_exclude_bg.color32())
                                        .stroke(Stroke::new(
                                            1.0,
                                            palette.status_rejected_fg.color32(),
                                        ))
                                        .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                        .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                        .show(ui, |ui| {
                                            ui.label(
                                                RichText::new(
                                                    "¿Borrar todos los datos de rostros? Se eliminarán los grupos, nombres y embeddings de rostros calculados. Las etiquetas y los archivos originales no se verán afectados. Nada sale de tu PC."
                                                )
                                                .size(theme::FONT_SMALL)
                                                .color(palette.chip_exclude_fg.color32()),
                                            );
                                            ui.add_space(theme::SPACE_XS);
                                            ui.horizontal(|ui| {
                                                if ui.button("Sí, borrar datos de rostros").clicked() {
                                                    outcome.clear_face_grouping = true;
                                                    state.confirm_clear_face_data = false;
                                                }
                                                if ui.button("Cancelar").clicked() {
                                                    state.confirm_clear_face_data = false;
                                                }
                                            });
                                        });
                                }

                                for paused in &engine_snapshot.paused_stages {
                                    ui.add_space(theme::SPACE_SM);
                                    ui.label(
                                        RichText::new(format!(
                                            "{} en pausa: {}",
                                            paused.stage.label_es(),
                                            paused.reason
                                        ))
                                        .size(theme::FONT_SMALL)
                                        .color(palette.status_rejected_fg.color32()),
                                    );
                                }

                                ui.add_space(theme::SPACE_SM);
                                ui.separator();
                                ui.add_space(theme::SPACE_SM);

                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new("Errores de revisión")
                                                .color(palette.text_primary.color32()),
                                        );
                                        let err_desc = if error_count == 0 {
                                            "Sin fotos con errores pendientes".to_string()
                                        } else {
                                            format!(
                                                "{} con fallos guardados",
                                                count_phrase(error_count, "foto", "fotos")
                                            )
                                        };
                                        ui.label(
                                            RichText::new(err_desc)
                                                .size(theme::FONT_SMALL)
                                                .color(if error_count > 0 {
                                                    palette.status_rejected_fg.color32()
                                                } else {
                                                    palette.text_secondary.color32()
                                                }),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Ver errores").clicked() {
                                            outcome.open_errors = true;
                                        }
                                    });
                                });

                                ui.add_space(theme::SPACE_MD);
                                ui.label(
                                    RichText::new(
                                        "Revisar lee cada archivo en modo solo lectura; Windows puede actualizar su fecha de último acceso",
                                    )
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_hint.color32()),
                                );
                            });

                            ui.add_space(theme::SPACE_XL);

                            // 3. Modelos y dispositivo
                            section_heading(ui, palette, "Modelos y dispositivo");
                            section_card(ui, palette, |ui| {
                                match model_folder_row(
                                    ui,
                                    palette,
                                    "Modelos de visión (SigLIP 2 y YuNet)",
                                    settings.vision_models_dir.as_deref(),
                                    state.vision_dir_error.as_deref(),
                                    "Elegir carpeta de modelos de visión",
                                ) {
                                    ModelFolderAction::Picked(dir) => {
                                        apply_vision_pick(
                                            settings,
                                            state,
                                            &dir,
                                            &mut outcome.settings_changed,
                                        );
                                    }
                                    ModelFolderAction::Cleared => {
                                        settings.vision_models_dir = None;
                                        settings.stage_vision = false;
                                        state.vision_dir_error = None;
                                        outcome.settings_changed = true;
                                    }
                                    ModelFolderAction::None => {}
                                }
                                ui.add_space(theme::SPACE_MD);
                                match model_folder_row(
                                    ui,
                                    palette,
                                    "Modelos OCR",
                                    settings.ocr_models_dir.as_deref(),
                                    state.ocr_dir_error.as_deref(),
                                    "Elegir carpeta de modelos OCR",
                                ) {
                                    ModelFolderAction::Picked(dir) => {
                                        apply_ocr_pick(
                                            settings,
                                            state,
                                            &dir,
                                            &mut outcome.settings_changed,
                                        );
                                    }
                                    ModelFolderAction::Cleared => {
                                        settings.ocr_models_dir = None;
                                        settings.stage_ocr = false;
                                        state.ocr_dir_error = None;
                                        outcome.settings_changed = true;
                                    }
                                    ModelFolderAction::None => {}
                                }

                                ui.add_space(theme::SPACE_MD);
                                ui.separator();
                                ui.add_space(theme::SPACE_SM);

                                ui.label(RichText::new("Dispositivo").strong());
                                ui.add_space(theme::SPACE_XS);
                                let prev_dev = settings.ocr_device;
                                ui.horizontal(|ui| {
                                    if option_pill(
                                        ui,
                                        palette,
                                        "GPU (DirectML)",
                                        settings.ocr_device == OcrDevice::DirectMl,
                                    ) {
                                        settings.ocr_device = OcrDevice::DirectMl;
                                    }
                                    if option_pill(
                                        ui,
                                        palette,
                                        "CPU",
                                        settings.ocr_device == OcrDevice::Cpu,
                                    ) {
                                        settings.ocr_device = OcrDevice::Cpu;
                                    }
                                });
                                if settings.ocr_device != prev_dev {
                                    outcome.settings_changed = true;
                                }
                                ui.add_space(theme::SPACE_XS);
                                ui.label(
                                    RichText::new(
                                        "Solo se usa un proveedor por sesión; cambiar después de usar un modelo requiere reiniciar. Nunca cambia a CPU en silencio",
                                    )
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_hint.color32()),
                                );
                            });

                            ui.add_space(theme::SPACE_XL);

                            // 4. Apariencia
                            section_heading(ui, palette, "Apariencia");
                            section_card(ui, palette, |ui| {
                                let prev_theme = settings.theme;
                                ui.horizontal(|ui| {
                                    for (t, label) in [
                                        (Theme::System, "Sistema"),
                                        (Theme::Dark, "Oscuro"),
                                        (Theme::Light, "Claro"),
                                    ] {
                                        if option_pill(ui, palette, label, settings.theme == t) {
                                            settings.theme = t;
                                        }
                                    }
                                });
                                if settings.theme != prev_theme {
                                    outcome.theme_changed = true;
                                    outcome.settings_changed = true;
                                }
                            });

                            ui.add_space(theme::SPACE_XL);

                            // 5. Acerca de
                            section_heading(ui, palette, "Acerca de");
                            section_card(ui, palette, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("photosort").strong());
                                    ui.label(
                                        RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                            .size(theme::FONT_SMALL)
                                            .monospace()
                                            .color(palette.text_secondary.color32()),
                                    );
                                    ui.label(
                                        RichText::new("· Licencia MIT")
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_secondary.color32()),
                                    );
                                });
                                ui.add_space(theme::SPACE_SM);
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        RichText::new("Carpeta de datos:")
                                            .size(theme::FONT_SMALL)
                                            .color(palette.text_secondary.color32()),
                                    );
                                    ui.label(
                                        RichText::new(data_dir.display().to_string())
                                            .size(theme::FONT_SMALL)
                                            .monospace(),
                                    );
                                    if ui.button("Mostrar en el Explorador").clicked()
                                        && let Err(e) = reveal_in_explorer(data_dir)
                                    {
                                        outcome.error =
                                            Some(format!("No se pudo abrir el Explorador: {e}"));
                                    }
                                });
                                ui.add_space(theme::SPACE_SM);
                                ui.label(
                                    RichText::new(
                                        "Todo el análisis se ejecuta en tu equipo; tus fotos nunca se suben ni se modifican",
                                    )
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_hint.color32()),
                                );
                            });

                            ui.add_space(theme::SPACE_XL);
                        });
                    });
                });
        });

    let _ = StageKind::Rules; // keep import clean
    outcome
}

fn section_heading(ui: &mut egui::Ui, palette: &Palette, title: &str) {
    ui.label(
        RichText::new(title)
            .size(theme::FONT_HEADING)
            .color(palette.text_primary.color32()),
    );
    ui.add_space(theme::SPACE_SM);
}

fn section_card(ui: &mut egui::Ui, palette: &Palette, body: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(palette.bg_panel.color32())
        .stroke(Stroke::new(1.0, palette.border.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_LG))
        .inner_margin(Margin::same(theme::SPACE_LG as i8))
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            body(ui);
        });
}

fn switch_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    title: &str,
    description: &str,
    value: &mut bool,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(title).color(palette.text_primary.color32()));
            ui.label(
                RichText::new(description)
                    .size(theme::FONT_SMALL)
                    .color(palette.text_secondary.color32()),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if theme::toggle_switch(ui, palette, value, title).changed() {
                changed = true;
            }
        });
    });
    changed
}

enum ModelFolderAction {
    None,
    Picked(PathBuf),
    Cleared,
}

fn pick_model_folder(dialog_title: &str, current: Option<&Path>) -> Option<PathBuf> {
    let mut dlg = rfd::FileDialog::new().set_title(dialog_title);
    if let Some(cur) = current
        && cur.is_dir()
    {
        dlg = dlg.set_directory(cur);
    }
    dlg.pick_folder()
}

fn apply_vision_pick(
    settings: &mut Settings,
    state: &mut SettingsViewState,
    dir: &Path,
    settings_changed: &mut bool,
) {
    match model_dirs::apply_picked_vision_dir(settings, dir) {
        Ok(inferred_ocr) => {
            state.vision_dir_error = None;
            if inferred_ocr.is_some() {
                state.ocr_dir_error = None;
            }
            *settings_changed = true;
        }
        Err(reason) => {
            if settings.vision_models_dir.is_none() {
                settings.stage_vision = false;
            }
            state.vision_dir_error = Some(reason);
        }
    }
}

fn apply_ocr_pick(
    settings: &mut Settings,
    state: &mut SettingsViewState,
    dir: &Path,
    settings_changed: &mut bool,
) {
    match model_dirs::apply_picked_ocr_dir(settings, dir) {
        Ok(inferred_vision) => {
            state.ocr_dir_error = None;
            if inferred_vision.is_some() {
                state.vision_dir_error = None;
            }
            *settings_changed = true;
        }
        Err(reason) => {
            if settings.ocr_models_dir.is_none() {
                settings.stage_ocr = false;
            }
            state.ocr_dir_error = Some(reason);
        }
    }
}

fn model_folder_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    title: &str,
    current: Option<&Path>,
    error: Option<&str>,
    dialog_title: &str,
) -> ModelFolderAction {
    let mut action = ModelFolderAction::None;
    ui.label(RichText::new(title).strong());
    ui.add_space(theme::SPACE_XS);
    ui.horizontal(|ui| {
        let display = current
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Sin configurar".into());
        let color = if current.is_some() {
            palette.text_primary.color32()
        } else {
            palette.text_hint.color32()
        };
        ui.add(
            egui::Label::new(RichText::new(display).size(theme::FONT_SMALL).color(color))
                .truncate(),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if current.is_some() && ui.button("Quitar").clicked() {
                action = ModelFolderAction::Cleared;
            }
            if ui.button("Elegir carpeta…").clicked()
                && let Some(dir) = pick_model_folder(dialog_title, current)
            {
                action = ModelFolderAction::Picked(dir);
            }
        });
    });
    if let Some(err) = error {
        ui.add_space(theme::SPACE_XS);
        ui.label(
            RichText::new(err)
                .size(theme::FONT_SMALL)
                .color(palette.status_rejected_fg.color32()),
        );
    }
    action
}

fn option_pill(ui: &mut egui::Ui, palette: &Palette, label: &str, selected: bool) -> bool {
    let bg = if selected {
        palette.accent_bg.color32()
    } else {
        palette.bg_card.color32()
    };
    let stroke = if selected {
        Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
    } else {
        Stroke::new(1.0, palette.border.color32())
    };
    let resp = ui.add(
        egui::Button::new(
            RichText::new(label)
                .size(theme::FONT_BODY)
                .color(palette.text_primary.color32()),
        )
        .fill(bg)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .min_size(Vec2::new(88.0, 32.0)),
    );
    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, label));
    resp.clicked()
}
