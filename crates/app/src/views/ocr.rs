use crate::model::{count_phrase, library_display_name};
use crate::ocr_worker::{OcrEvent, OcrScope, OcrWorker, format_eta};
use crate::settings::{self, OcrDevice, Settings};
use crate::theme::{self, Palette};
use crate::worker::Notify;
use catalog::{AssetId, Catalog, OCR_STAGE, OCR_VERSION};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, ProgressBar, RichText, ScrollArea,
    Stroke,
};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const OCR_WINDOW_WIDTH: f32 = 540.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrTargetMode {
    Selection,
    Folders,
}

#[derive(Debug, Clone)]
pub struct FolderPending {
    pub root: PathBuf,
    pub pending: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OcrRunState {
    Idle,
    LoadingModel,
    Running {
        done: usize,
        total: usize,
        errors: usize,
        elapsed_secs: f64,
    },
    Done {
        stopped: bool,
        processed: usize,
        errors: usize,
    },
    Failed(String),
}

pub struct OcrDialog {
    pub open: bool,
    pub mode: OcrTargetMode,
    pub selected_assets: Vec<AssetId>,
    pub selection_pending: usize,
    pub folders: Vec<FolderPending>,
    /// Checked folder roots (none checked by default).
    pub checked_roots: BTreeSet<PathBuf>,
    pub models_input: String,
    pub run_state: OcrRunState,
}

impl OcrDialog {
    pub fn open_fresh(
        cat: Option<&Catalog>,
        settings: &Settings,
        selected_assets: Vec<AssetId>,
    ) -> Self {
        let mut dialog = Self {
            open: true,
            mode: if selected_assets.is_empty() {
                OcrTargetMode::Folders
            } else {
                OcrTargetMode::Selection
            },
            selected_assets,
            selection_pending: 0,
            folders: Vec::new(),
            checked_roots: BTreeSet::new(), // None checked by default
            models_input: settings
                .ocr_models_dir
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            run_state: OcrRunState::Idle,
        };
        dialog.refresh_counts(cat, settings);
        dialog
    }

    pub fn refresh_counts(&mut self, cat: Option<&Catalog>, settings: &Settings) {
        self.selection_pending = cat
            .and_then(|c| {
                c.count_pending_in_assets(OCR_STAGE, OCR_VERSION, &self.selected_assets)
                    .ok()
            })
            .unwrap_or(0);

        self.folders = settings
            .library_roots
            .iter()
            .map(|root| {
                let pending = cat
                    .and_then(|c| c.count_pending_in_root(OCR_STAGE, OCR_VERSION, root).ok())
                    .unwrap_or(0);
                FolderPending {
                    root: root.clone(),
                    pending,
                }
            })
            .collect();

        let valid: BTreeSet<PathBuf> = settings.library_roots.iter().cloned().collect();
        self.checked_roots.retain(|r| valid.contains(r));
    }

    pub fn is_running(&self) -> bool {
        matches!(
            self.run_state,
            OcrRunState::LoadingModel | OcrRunState::Running { .. }
        )
    }

    pub fn apply_event(&mut self, ev: &OcrEvent) {
        match ev {
            OcrEvent::LoadingModel => {
                self.run_state = OcrRunState::LoadingModel;
            }
            OcrEvent::Progress {
                done,
                total,
                errors,
                elapsed_secs,
                ..
            } => {
                self.run_state = OcrRunState::Running {
                    done: *done,
                    total: *total,
                    errors: *errors,
                    elapsed_secs: *elapsed_secs,
                };
            }
            OcrEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                self.run_state = OcrRunState::Done {
                    stopped: *stopped,
                    processed: *processed,
                    errors: *errors,
                };
            }
            OcrEvent::Failed(msg) => {
                self.run_state = OcrRunState::Failed(msg.clone());
            }
        }
    }

    fn selected_scope(&self) -> Option<(OcrScope, usize)> {
        match self.mode {
            OcrTargetMode::Selection => {
                if self.selected_assets.is_empty() || self.selection_pending == 0 {
                    None
                } else {
                    Some((
                        OcrScope::Assets(self.selected_assets.clone()),
                        self.selection_pending,
                    ))
                }
            }
            OcrTargetMode::Folders => {
                let roots: Vec<PathBuf> = self
                    .folders
                    .iter()
                    .filter(|f| self.checked_roots.contains(&f.root))
                    .map(|f| f.root.clone())
                    .collect();
                let pending: usize = self
                    .folders
                    .iter()
                    .filter(|f| self.checked_roots.contains(&f.root))
                    .map(|f| f.pending)
                    .sum();
                if roots.is_empty() || pending == 0 {
                    None
                } else {
                    Some((OcrScope::Roots(roots), pending))
                }
            }
        }
    }
}

pub struct OcrWindowOutcome {
    pub start_request: Option<(OcrScope, PathBuf, OcrDevice)>,
    pub cancel_requested: bool,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_window(
    ctx: &egui::Context,
    palette: &Palette,
    dialog: &mut Option<OcrDialog>,
    worker_running: bool,
    indexing: bool,
    data_dir: &Path,
    settings: &mut Settings,
    banner_error: &mut Option<String>,
) -> OcrWindowOutcome {
    let mut outcome = OcrWindowOutcome {
        start_request: None,
        cancel_requested: false,
    };

    let Some(dlg) = dialog.as_mut() else {
        return outcome;
    };

    let running = worker_running || dlg.is_running();

    let mut close_requested = false;
    if !running
        && !ctx.egui_wants_keyboard_input()
        && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape))
    {
        close_requested = true;
    }

    let mut open_flag = true;
    let mut window = egui::Window::new("Leer texto (OCR)")
        .collapsible(false)
        .resizable(false)
        .fade_in(false)
        .frame(Frame::window(&ctx.style_of(ctx.theme())).fill(palette.bg_panel.color32()))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .min_width(OCR_WINDOW_WIDTH)
        .max_width(OCR_WINDOW_WIDTH)
        .default_width(OCR_WINDOW_WIDTH);
    if !running {
        window = window.open(&mut open_flag);
    }

    window.show(ctx, |ui| {
        ui.label(
            RichText::new(
                "Lee el texto de las imágenes localmente y lo guarda en el catálogo para poder buscarlo. Los originales nunca se modifican.",
            )
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
        );
        ui.add_space(theme::SPACE_SM);
        ui.separator();
        ui.add_space(theme::SPACE_SM);

        // 1. Target scope
        ui.label(RichText::new("Qué leer").strong());
        ui.add_space(theme::SPACE_XS);

        ui.add_enabled_ui(!running, |ui| {
            let sel_count = dlg.selected_assets.len();
            let sel_label = if sel_count == 0 {
                "Selección actual (ninguna imagen seleccionada)".to_string()
            } else {
                format!(
                    "Selección actual ({}, {})",
                    sel_count,
                    count_phrase(dlg.selection_pending, "pendiente", "pendientes")
                )
            };
            ui.add_enabled_ui(sel_count > 0, |ui| {
                ui.radio_value(&mut dlg.mode, OcrTargetMode::Selection, sel_label);
            });

            ui.radio_value(
                &mut dlg.mode,
                OcrTargetMode::Folders,
                "Carpetas de la biblioteca",
            );

            if dlg.mode == OcrTargetMode::Folders {
                Frame::new()
                    .fill(palette.bg_card.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        if dlg.folders.is_empty() {
                            ui.label(
                                RichText::new("No hay carpetas de biblioteca registradas.")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        } else {
                            ScrollArea::vertical()
                                .max_height(130.0)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for folder in &dlg.folders {
                                        let mut checked = dlg.checked_roots.contains(&folder.root);
                                        let name = library_display_name(&folder.root);
                                        let label = format!(
                                            "{name} ({})",
                                            count_phrase(
                                                folder.pending,
                                                "pendiente",
                                                "pendientes"
                                            )
                                        );
                                        let resp = ui
                                            .checkbox(&mut checked, label)
                                            .on_hover_text(folder.root.display().to_string());
                                        if resp.changed() {
                                            if checked {
                                                dlg.checked_roots.insert(folder.root.clone());
                                            } else {
                                                dlg.checked_roots.remove(&folder.root);
                                            }
                                        }
                                    }
                                });
                        }
                    });
            }

            ui.add_space(theme::SPACE_MD);

            // 2. Models folder
            ui.label(RichText::new("Carpeta de modelos OCR").strong());
            ui.add_space(theme::SPACE_XS);
            let mut models_changed = false;
            ui.horizontal(|ui| {
                let edit_w = (ui.available_width() - 148.0).max(180.0);
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut dlg.models_input)
                        .desired_width(edit_w)
                        .hint_text(theme::hint_text(
                            palette,
                            "Carpeta con los cinco archivos de modelos…",
                        )),
                );
                if edit.changed() {
                    models_changed = true;
                }
                if ui.button("Elegir carpeta…").clicked() {
                    let mut picker = rfd::FileDialog::new()
                        .set_title("Elegir carpeta de modelos OCR (solo lectura)");
                    let current = PathBuf::from(dlg.models_input.trim());
                    if !current.as_os_str().is_empty() && current.is_dir() {
                        picker = picker.set_directory(&current);
                    }
                    if let Some(folder) = picker.pick_folder() {
                        dlg.models_input = folder.to_string_lossy().into_owned();
                        models_changed = true;
                    }
                }
            });
            if models_changed {
                let trimmed = dlg.models_input.trim();
                settings.ocr_models_dir = if trimmed.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(trimmed))
                };
                if let Err(e) = settings::save(data_dir, settings) {
                    *banner_error = Some(format!("No se pudo guardar la carpeta de modelos: {e}"));
                }
            }

            ui.add_space(theme::SPACE_MD);

            // 3. Device choice
            ui.label(RichText::new("Dispositivo").strong());
            ui.add_space(theme::SPACE_XS);
            let prev_device = settings.ocr_device;
            ui.horizontal(|ui| {
                ui.radio_value(
                    &mut settings.ocr_device,
                    OcrDevice::DirectMl,
                    "GPU (DirectML)",
                );
                ui.radio_value(&mut settings.ocr_device, OcrDevice::Cpu, "CPU");
            });
            if settings.ocr_device != prev_device
                && let Err(e) = settings::save(data_dir, settings)
            {
                *banner_error = Some(format!("No se pudo guardar el dispositivo OCR: {e}"));
            }
            ui.label(
                RichText::new(
                    "No hay cambio automático a CPU si falla la GPU. Cambiar de dispositivo tras ejecutar OCR requiere reiniciar la aplicación.",
                )
                .size(theme::FONT_SMALL)
                .color(palette.text_secondary.color32()),
            );
        });

        ui.add_space(theme::SPACE_MD);

        // 4. Status / Progress / Error
        match &dlg.run_state {
            OcrRunState::Idle => {}
            OcrRunState::LoadingModel => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Cargando modelos de OCR…")
                            .color(palette.text_secondary.color32()),
                    );
                });
            }
            OcrRunState::Running {
                done,
                total,
                errors,
                elapsed_secs,
            } => {
                let frac = if *total == 0 {
                    0.0
                } else {
                    *done as f32 / *total as f32
                };
                ui.add(ProgressBar::new(frac).desired_width(ui.available_width()));
                ui.add_space(theme::SPACE_XS);
                let mut status = format!("{done} de {total}");
                if let Some(eta) = format_eta(*done, *total, *elapsed_secs) {
                    status.push_str(&format!(" · Tiempo restante estimado: {eta}"));
                }
                if *errors > 0 {
                    status.push_str(&format!(
                        " · {}",
                        count_phrase(*errors, "error", "errores")
                    ));
                }
                ui.label(RichText::new(status).strong());
            }
            OcrRunState::Done {
                stopped,
                processed,
                errors,
            } => {
                let prefix = if *stopped {
                    "Lectura detenida"
                } else {
                    "Lectura completada"
                };
                let mut msg = format!(
                    "{prefix}: {}.",
                    count_phrase(*processed, "imagen leída", "imágenes leídas")
                );
                if *errors > 0 {
                    msg.push_str(&format!(
                        " {} (siguen pendientes).",
                        count_phrase(*errors, "error", "errores")
                    ));
                }
                Frame::new()
                    .fill(palette.chip_require_bg.color32())
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        ui.label(RichText::new(msg).color(palette.chip_require_fg.color32()));
                    });
            }
            OcrRunState::Failed(err) => {
                Frame::new()
                    .fill(palette.chip_exclude_bg.color32())
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .inner_margin(Margin::same(theme::SPACE_SM as i8))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(format!("No se pudo ejecutar el OCR: {err}"))
                                .color(palette.chip_exclude_fg.color32()),
                        );
                        ui.add_space(theme::SPACE_XS);
                        ui.label(
                            RichText::new(
                                "Si deseas cambiar entre GPU (DirectML) y CPU, guarda la opción y reinicia la aplicación.",
                            )
                            .size(theme::FONT_SMALL)
                            .color(palette.chip_exclude_fg.color32()),
                        );
                    });
            }
        }

        ui.add_space(theme::SPACE_MD);
        ui.separator();
        ui.add_space(theme::SPACE_SM);

        if indexing {
            ui.label(
                RichText::new("Podrás leer texto cuando termine el indexado.")
                    .color(palette.text_secondary.color32()),
            );
        }

        // 5. Footer buttons
        ui.horizontal(|ui| {
            if running {
                if ui.button("Cancelar").clicked() {
                    outcome.cancel_requested = true;
                }
            } else if ui.button("Cerrar").clicked() {
                close_requested = true;
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !running {
                    let models_path = PathBuf::from(dlg.models_input.trim());
                    let scope = dlg.selected_scope();
                    let can_start =
                        !indexing && !models_path.as_os_str().is_empty() && scope.is_some();
                    let label = match &scope {
                        Some((_, n)) => format!("Leer texto ({n})"),
                        None => "Leer texto".to_string(),
                    };
                    if ui
                        .add_enabled(can_start, egui::Button::new(label))
                        .clicked()
                        && let Some((chosen_scope, _)) = scope
                    {
                        outcome.start_request =
                            Some((chosen_scope, models_path, settings.ocr_device));
                    }
                }
            });
        });
    });

    if (!open_flag && !running) || close_requested {
        *dialog = None;
    }

    outcome
}

pub fn start_ocr_worker(
    ctx: &egui::Context,
    data_dir: &Path,
    settings: &Settings,
    scope: OcrScope,
    models_dir: PathBuf,
    device: OcrDevice,
) -> Result<OcrWorker, String> {
    let roots = settings.valid_roots();
    let db = settings::db_dest_path(data_dir, &roots).map_err(|e| e.to_string())?;
    let ctx_clone = ctx.clone();
    let notify: Notify = Arc::new(move || ctx_clone.request_repaint());
    Ok(OcrWorker::start(
        db,
        scope,
        models_dir,
        device,
        Some(notify),
    ))
}
