use crate::model::count_phrase;
use crate::theme::{self, Palette};
use catalog::{AssetId, Catalog, FaceDetail, Person, PersonId};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, Pos2, Rect, RichText, ScrollArea,
    Stroke, TextureHandle, Vec2,
};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct PersonsViewState {
    pub filter: String,
    pub show_hidden: bool,
    pub renaming: Option<(PersonId, String)>,
    pub merging: Option<(PersonId, Option<PersonId>)>,
    pub naming_input: String,
    pub merge_conflict: Option<(PersonId, PersonId, String)>,
    pub postponed_persons: HashSet<PersonId>,
}

#[derive(Default)]
pub struct PersonsOutcome {
    pub open_in_gallery: Option<PersonId>,
    pub filter_changed: bool,
    pub renamed: Option<(PersonId, String)>,
    pub merged: Option<(PersonId, PersonId)>,
    pub toggled_hidden: Option<(PersonId, bool)>,
    pub ignored: Option<PersonId>,
    pub postponed: Option<PersonId>,
    pub removed_asset: Option<(PersonId, AssetId)>,
}

/// Draws the non-intrusive Naming Card for an unnamed person appearing in >= 3 photos.
#[allow(clippy::too_many_arguments)]
pub fn draw_naming_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    person: &Person,
    sample_faces: &[FaceDetail],
    state: &mut PersonsViewState,
    cat: &Catalog,
    thumb_textures: &HashMap<String, TextureHandle>,
    asset_paths: &HashMap<AssetId, String>,
) -> PersonsOutcome {
    let mut outcome = PersonsOutcome::default();

    Frame::new()
        .fill(palette.bg_card.color32())
        .stroke(Stroke::new(1.5, palette.accent.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .inner_margin(Margin::same(theme::SPACE_MD as i8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                // Display sample face crops (up to 4)
                for face in sample_faces.iter().take(4) {
                    draw_face_chip(ui, palette, face, thumb_textures, asset_paths, 44.0);
                }

                ui.add_space(theme::SPACE_SM);

                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Esta persona aparece en {} fotos. ¿Cómo se llama?",
                            person.photo_count
                        ))
                        .strong()
                        .color(palette.text_primary.color32()),
                    );

                    ui.add_space(theme::SPACE_XS);

                    // Conflict resolution if user entered an existing name
                    if let Some((src_id, target_id, name)) = state.merge_conflict.clone()
                        && src_id == person.id
                    {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "Ya existe una persona llamada «{name}». ¿Deseas unirlas en una sola?"
                                ))
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                            );
                            if ui
                                .add(theme::primary_button(palette, "Unir grupos"))
                                .clicked()
                            {
                                outcome.merged = Some((src_id, target_id));
                                state.merge_conflict = None;
                                state.naming_input.clear();
                            }
                            if ui.button("Cancelar").clicked() {
                                state.merge_conflict = None;
                            }
                        });
                        return;
                    }

                    ui.horizontal(|ui| {
                        let text_edit = ui.add(
                            egui::TextEdit::singleline(&mut state.naming_input)
                                .hint_text(theme::hint_text(palette, "Nombre de la persona"))
                                .desired_width(200.0),
                        );

                        let enter_pressed = text_edit.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));

                        let save_clicked = ui
                            .add_enabled(
                                !state.naming_input.trim().is_empty(),
                                theme::primary_button(palette, "Guardar"),
                            )
                            .clicked()
                            || (enter_pressed && !state.naming_input.trim().is_empty());

                        if save_clicked {
                            let trimmed = state.naming_input.trim().to_string();
                            if let Ok(Some(existing)) = cat.find_person_by_name(&trimmed) {
                                if existing.id != person.id {
                                    state.merge_conflict =
                                        Some((person.id, existing.id, trimmed));
                                } else {
                                    outcome.renamed = Some((person.id, trimmed));
                                    state.naming_input.clear();
                                }
                            } else {
                                outcome.renamed = Some((person.id, trimmed));
                                state.naming_input.clear();
                            }
                        }

                        if ui.button("Más tarde").on_hover_text("Vuelve a preguntar en la próxima sesión").clicked() {
                            state.postponed_persons.insert(person.id);
                            outcome.postponed = Some(person.id);
                        }

                        if ui.button("No preguntar").on_hover_text("Ignorar esta persona y no volver a proponerla").clicked() {
                            outcome.ignored = Some(person.id);
                        }
                    });
                });
            });
        });

    outcome
}

fn draw_face_chip(
    ui: &mut egui::Ui,
    palette: &Palette,
    face: &FaceDetail,
    thumb_textures: &HashMap<String, TextureHandle>,
    asset_paths: &HashMap<AssetId, String>,
    size_px: f32,
) {
    let sha256 = asset_paths.get(&face.asset_id);
    let texture = sha256.and_then(|sha| thumb_textures.get(sha));

    let (rect, _response) = ui.allocate_exact_size(Vec2::splat(size_px), egui::Sense::hover());

    if let Some(tex) = texture {
        // Crop face from texture using UV coordinates with ~15% margin
        let [x1, y1, x2, y2] = face.bbox;
        let fw = (x2 - x1).max(1.0);
        let fh = (y2 - y1).max(1.0);
        let margin_x = fw * 0.15;
        let margin_y = fh * 0.15;

        // Image coordinates in YuNet are in decoded px (longest edge <= 640px)
        let tex_size = tex.size_vec2();
        let longest = tex_size.x.max(tex_size.y).max(1.0);
        let scale = 640.0f32 / longest;
        let dec_w = (tex_size.x * scale).max(1.0);
        let dec_h = (tex_size.y * scale).max(1.0);
        let u0 = ((x1 - margin_x) / dec_w).clamp(0.0, 1.0);
        let v0 = ((y1 - margin_y) / dec_h).clamp(0.0, 1.0);
        let u1 = ((x2 + margin_x) / dec_w).clamp(0.0, 1.0);
        let v1 = ((y2 + margin_y) / dec_h).clamp(0.0, 1.0);

        let uv = Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1));
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(theme::RADIUS_SM),
            palette.bg_panel.color32(),
        );
        ui.painter().image(tex.id(), rect, uv, Color32::WHITE);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(theme::RADIUS_SM),
            Stroke::new(1.0, palette.border.color32()),
            egui::StrokeKind::Inside,
        );
    } else {
        // Placeholder avatar with user icon
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(theme::RADIUS_SM),
            palette.bg_panel.color32(),
        );
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(theme::RADIUS_SM),
            Stroke::new(1.0, palette.border.color32()),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "👤",
            egui::FontId::proportional(size_px * 0.5),
            palette.text_secondary.color32(),
        );
    }
}

pub fn draw(
    ui: &mut egui::Ui,
    palette: &Palette,
    state: &mut PersonsViewState,
    cat: &Catalog,
    thumb_textures: &HashMap<String, TextureHandle>,
    asset_paths: &HashMap<AssetId, String>,
) -> PersonsOutcome {
    let mut outcome = PersonsOutcome::default();

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
            // Header
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Personas")
                        .size(theme::FONT_TITLE)
                        .color(palette.text_primary.color32()),
                );

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.checkbox(&mut state.show_hidden, "Mostrar ocultas").changed() {
                        outcome.filter_changed = true;
                    }
                    ui.add_space(theme::SPACE_MD);
                    ui.add(
                        egui::TextEdit::singleline(&mut state.filter)
                            .hint_text(theme::hint_text(palette, "Filtrar personas..."))
                            .desired_width(180.0),
                    );
                });
            });

            ui.add_space(theme::SPACE_MD);

            // Naming proposals card (unnamed persons with >= 3 photos)
            if let Ok(unnamed_candidates) = cat.unnamed_persons_with_min_photos(3)
                && let Some(candidate) = unnamed_candidates
                    .iter()
                    .find(|p| !state.postponed_persons.contains(&p.id))
            {
                let faces = cat.person_faces(candidate.id).unwrap_or_default();
                let card_outcome = draw_naming_card(
                    ui,
                    palette,
                    candidate,
                    &faces,
                    state,
                    cat,
                    thumb_textures,
                    asset_paths,
                );
                if card_outcome.renamed.is_some() {
                    outcome.renamed = card_outcome.renamed;
                }
                if card_outcome.merged.is_some() {
                    outcome.merged = card_outcome.merged;
                }
                if card_outcome.ignored.is_some() {
                    outcome.ignored = card_outcome.ignored;
                }
                if card_outcome.postponed.is_some() {
                    outcome.postponed = card_outcome.postponed;
                }
                ui.add_space(theme::SPACE_MD);
            }

            // List / Grid of persons
            let persons = cat.list_persons(state.show_hidden).unwrap_or_default();

            let filter_lower = state.filter.trim().to_lowercase();
            let filtered_persons: Vec<&Person> = persons
                .iter()
                .filter(|p| {
                    if filter_lower.is_empty() {
                        true
                    } else if let Some(name) = &p.name {
                        name.to_lowercase().contains(&filter_lower)
                    } else {
                        "sin nombre".contains(&filter_lower)
                    }
                })
                .collect();

            if filtered_persons.is_empty() {
                ui.add_space(theme::SPACE_XL);
                ui.vertical_centered(|ui| {
                    ui.label(
                        RichText::new(if persons.is_empty() {
                            "No se han detectado o agrupado personas todavía.\nActiva «Agrupar rostros» en Configuración para agrupar rostros automáticamente."
                        } else {
                            "Ninguna persona coincide con el filtro."
                        })
                        .color(palette.text_secondary.color32()),
                    );
                });
                return;
            }

            ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let col_width = 320.0f32;
                    let avail_width = ui.available_width();
                    let cols = ((avail_width / (col_width + theme::SPACE_MD)).floor() as usize).max(1);

                    egui::Grid::new("persons_grid")
                        .spacing(Vec2::new(theme::SPACE_MD, theme::SPACE_MD))
                        .show(ui, |ui| {
                            for (idx, person) in filtered_persons.iter().enumerate() {
                                draw_person_card(
                                    ui,
                                    palette,
                                    person,
                                    &persons,
                                    state,
                                    &mut outcome,
                                    cat,
                                    thumb_textures,
                                    asset_paths,
                                );
                                if (idx + 1) % cols == 0 {
                                    ui.end_row();
                                }
                            }
                        });
                });
        });

    outcome
}

#[allow(clippy::too_many_arguments)]
fn draw_person_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    person: &Person,
    all_persons: &[Person],
    state: &mut PersonsViewState,
    outcome: &mut PersonsOutcome,
    cat: &Catalog,
    thumb_textures: &HashMap<String, TextureHandle>,
    asset_paths: &HashMap<AssetId, String>,
) {
    Frame::new()
        .fill(palette.bg_panel.color32())
        .stroke(Stroke::new(1.0, palette.border.color32()))
        .corner_radius(CornerRadius::same(theme::RADIUS_MD))
        .inner_margin(Margin::same(theme::SPACE_MD as i8))
        .show(ui, |ui| {
            ui.set_width(320.0);

            ui.horizontal(|ui| {
                // Representative face crop
                let representative_face = person.cover_face_id.and_then(|fid| {
                    if let Some(aid) = person.cover_asset_id {
                        cat.asset_face_details(aid)
                            .ok()
                            .and_then(|faces| faces.into_iter().find(|f| f.id == fid))
                    } else {
                        None
                    }
                });

                if let Some(face) = &representative_face {
                    draw_face_chip(ui, palette, face, thumb_textures, asset_paths, 56.0);
                } else {
                    let rect = ui
                        .allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover())
                        .0;
                    ui.painter().rect_filled(
                        rect,
                        CornerRadius::same(theme::RADIUS_SM),
                        palette.bg_card.color32(),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "👤",
                        egui::FontId::proportional(28.0),
                        palette.text_secondary.color32(),
                    );
                }

                ui.add_space(theme::SPACE_SM);

                ui.vertical(|ui| {
                    // Check if inline renaming
                    let is_renaming = state
                        .renaming
                        .as_ref()
                        .map(|(id, _)| *id == person.id)
                        .unwrap_or(false);
                    if is_renaming {
                        let mut name_buf = state.renaming.as_ref().unwrap().1.clone();
                        let mut cancel = false;
                        let mut save = false;
                        ui.horizontal(|ui| {
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut name_buf)
                                    .hint_text("Nombre...")
                                    .desired_width(120.0),
                            );
                            if (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                                || ui.button("OK").clicked()
                            {
                                save = true;
                            }
                            if ui.button("✕").clicked() {
                                cancel = true;
                            }
                        });
                        if save {
                            outcome.renamed = Some((person.id, name_buf.trim().to_string()));
                            state.renaming = None;
                        } else if cancel {
                            state.renaming = None;
                        } else {
                            state.renaming = Some((person.id, name_buf));
                        }
                    } else {
                        draw_person_info(ui, palette, person);
                    }

                    ui.add_space(theme::SPACE_XS);

                    // Actions
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .button("Ver fotos")
                            .on_hover_text("Mostrar fotos de esta persona en la galería")
                            .clicked()
                        {
                            outcome.open_in_gallery = Some(person.id);
                        }

                        if ui.button("Renombrar").clicked() {
                            state.renaming =
                                Some((person.id, person.name.clone().unwrap_or_default()));
                        }

                        if ui
                            .button("Son la misma...")
                            .on_hover_text("Unir con otra persona existente")
                            .clicked()
                        {
                            state.merging = Some((person.id, None));
                        }

                        let hide_label = if person.hidden { "Mostrar" } else { "Ocultar" };
                        if ui.button(hide_label).clicked() {
                            outcome.toggled_hidden = Some((person.id, !person.hidden));
                        }
                    });

                    // Merge dialog inline if active for this person
                    let is_merging = state
                        .merging
                        .as_ref()
                        .map(|(id, _)| *id == person.id)
                        .unwrap_or(false);
                    if is_merging {
                        let mut chosen_target = state.merging.as_ref().unwrap().1;
                        let mut cancel = false;
                        let mut confirm_target = None;
                        ui.add_space(theme::SPACE_XS);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Unir a:").size(theme::FONT_SMALL));
                            egui::ComboBox::from_id_salt(("merge_target", person.id.0))
                                .selected_text(match chosen_target {
                                    Some(tid) => all_persons
                                        .iter()
                                        .find(|p| p.id == tid)
                                        .and_then(|p| p.name.as_deref())
                                        .unwrap_or("Sin nombre"),
                                    None => "Elegir persona...",
                                })
                                .show_ui(ui, |ui| {
                                    for other in all_persons {
                                        if other.id != person.id {
                                            let other_name =
                                                other.name.as_deref().unwrap_or("Sin nombre");
                                            let label = format!(
                                                "{other_name} ({} fotos)",
                                                other.photo_count
                                            );
                                            ui.selectable_value(
                                                &mut chosen_target,
                                                Some(other.id),
                                                label,
                                            );
                                        }
                                    }
                                });

                            if let Some(target) = chosen_target
                                && ui
                                    .add(theme::primary_button(palette, "Confirmar unión"))
                                    .clicked()
                            {
                                confirm_target = Some(target);
                            }
                            if ui.button("Cancelar").clicked() {
                                cancel = true;
                            }
                        });

                        if let Some(target) = confirm_target {
                            outcome.merged = Some((person.id, target));
                            state.merging = None;
                        } else if cancel {
                            state.merging = None;
                        } else {
                            state.merging = Some((person.id, chosen_target));
                        }
                    }
                });
            });
        });
}

fn draw_person_info(ui: &mut egui::Ui, palette: &Palette, person: &Person) {
    match &person.name {
        Some(name) => {
            ui.label(
                RichText::new(name)
                    .strong()
                    .size(theme::FONT_BODY)
                    .color(palette.text_primary.color32()),
            );
        }
        None => {
            ui.label(
                RichText::new("Sin nombre")
                    .italics()
                    .size(theme::FONT_BODY)
                    .color(palette.text_secondary.color32()),
            );
        }
    }

    ui.label(
        RichText::new(count_phrase(person.photo_count, "foto", "fotos"))
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    );
}
