use crate::model::{ChipMode, FilterState, format_chip_label, format_chip_tooltip};
use crate::ocr_worker::format_eta;
use crate::review_engine::{EnginePhase, EngineSnapshot};
use crate::theme::{self, Palette};
use crate::views::context_menu::attach_chip_context_menu;
use crate::views::label_actions::{LabelAction, draw_label_menu_items, flatten_category_tree};
use catalog::{
    LabelId, LabelKind, LabelNode, MetadataFacets, Orientation, ReviewedCounts, ReviewedFilter,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Id, Layout, Margin, RichText, ScrollArea, Stroke,
    TextureHandle, Vec2,
};

pub const SEARCH_INPUT_ID: &str = "photosort_search_input";
const LONG_FILTER_GROUP_THRESHOLD: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    Gallery,
    Suggestions,
    Duplicates,
    Works,
    Persons,
    Tags,
    Settings,
    Errors,
}

#[derive(Default)]
pub struct SidebarState {
    pub cat_filter: String,
    pub tag_filter: String,
    pub ent_filter: String,
}

#[derive(Default)]
pub struct TopBarOutcome {
    pub search_changed: bool,
    pub toggle_pause: bool,
    pub retry_failed: bool,
    pub open_errors: bool,
    pub open_settings: bool,
    pub dismiss_error: bool,
    pub dismiss_notice: bool,
}

#[derive(Default)]
pub struct SidebarOutcome {
    pub switch_view: Option<ActiveView>,
    pub set_reviewed_filter: Option<ReviewedFilter>,
    pub filter_changed: bool,
    pub label_action: Option<LabelAction>,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_top_bar(
    ui: &mut egui::Ui,
    palette: &Palette,
    search: &mut String,
    focus_search_next_frame: &mut bool,
    engine_snapshot: &EngineSnapshot,
    error_count: usize,
    banner_error: Option<&str>,
    status_notice: Option<&str>,
) -> TopBarOutcome {
    let mut outcome = TopBarOutcome::default();

    egui::Panel::top("top_bar")
        .exact_size(theme::TOP_BAR_HEIGHT)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::symmetric(
                    theme::SPACE_LG as i8,
                    theme::SPACE_SM as i8,
                ))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                let search_id = Id::unique(SEARCH_INPUT_ID);
                let search_w = (ui.available_width() - 360.0).clamp(160.0, 280.0);
                let search_resp = ui
                    .add(
                        egui::TextEdit::singleline(search)
                            .id(search_id)
                            .desired_width(search_w)
                            .hint_text(theme::hint_text(palette, "Buscar en texto o ruta (/)")),
                    )
                    .on_hover_text("Busca en el texto leído y en la ruta (/ o Ctrl+F)");
                if *focus_search_next_frame {
                    search_resp.request_focus();
                    *focus_search_next_frame = false;
                }
                if search_resp.changed() {
                    outcome.search_changed = true;
                }
                if !search.is_empty() && ui.button("Limpiar").clicked() {
                    search.clear();
                    outcome.search_changed = true;
                }

                // Right side: background review status + Pause/Resume
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match &engine_snapshot.phase {
                        EnginePhase::Off => {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Configurar")
                                            .size(theme::FONT_SMALL)
                                            .color(palette.accent.color32()),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                outcome.open_settings = true;
                            }
                            ui.label(
                                RichText::new("Revisión desactivada")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                        EnginePhase::Paused => {
                            if ui.button("Reanudar").clicked() {
                                outcome.toggle_pause = true;
                            }
                            ui.label(
                                RichText::new("Revisión en pausa")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                        EnginePhase::Yielded => {
                            ui.label(
                                RichText::new("Revisión en espera por otra tarea")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                        EnginePhase::Indexing {
                            root_index,
                            root_count,
                            done,
                            total,
                        } => {
                            if ui.button("Pausar").clicked() {
                                outcome.toggle_pause = true;
                            }
                            let text = if *root_count > 1 {
                                format!(
                                    "Buscando cambios ({}/{}): {done} de {total}",
                                    root_index + 1,
                                    root_count
                                )
                            } else {
                                format!("Buscando cambios: {done} de {total}")
                            };
                            ui.label(
                                RichText::new(text)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                        EnginePhase::LoadingModel(stage) => {
                            if ui.button("Pausar").clicked() {
                                outcome.toggle_pause = true;
                            }
                            ui.label(
                                RichText::new(format!("Cargando modelo ({})", stage.label_es()))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                        }
                        EnginePhase::Running {
                            done,
                            total,
                            elapsed_secs,
                            ..
                        } => {
                            if ui.button("Pausar").clicked() {
                                outcome.toggle_pause = true;
                            }
                            let eta = format_eta(*done, *total, *elapsed_secs)
                                .map(|s| format!(" · {s}"))
                                .unwrap_or_default();
                            ui.label(
                                RichText::new(format!("Revisando {done} de {total}{eta}"))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_primary.color32()),
                            );
                        }
                        EnginePhase::Idle => {
                            ui.label(
                                RichText::new("Todo revisado")
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_accepted_fg.color32()),
                            );
                        }
                        EnginePhase::Blocked(reason) => {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new("Configurar")
                                            .size(theme::FONT_SMALL)
                                            .color(palette.accent.color32()),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                outcome.open_settings = true;
                            }
                            ui.label(
                                RichText::new(reason)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                    }

                    let shown_errors = error_count.max(engine_snapshot.failed_count);
                    if shown_errors > 0 {
                        if ui.button("Reintentar").clicked() {
                            outcome.retry_failed = true;
                        }
                        let err_txt = if shown_errors == 1 {
                            "1 error".to_string()
                        } else {
                            format!("{shown_errors} errores")
                        };
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(err_txt)
                                        .size(theme::FONT_SMALL)
                                        .color(palette.status_rejected_fg.color32()),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Ver lista de fotos con errores")
                            .clicked()
                        {
                            outcome.open_errors = true;
                        }
                    } else if !engine_snapshot.paused_stages.is_empty()
                        && !matches!(engine_snapshot.phase, EnginePhase::Blocked(_))
                    {
                        let stage_names: Vec<&str> = engine_snapshot
                            .paused_stages
                            .iter()
                            .map(|p| p.stage.label_es())
                            .collect();
                        let tip = engine_snapshot
                            .paused_stages
                            .iter()
                            .map(|p| format!("{}: {}", p.stage.label_es(), p.reason))
                            .collect::<Vec<_>>()
                            .join("\n");
                        ui.label(
                            RichText::new(format!("En pausa: {}", stage_names.join(", ")))
                                .size(theme::FONT_SMALL)
                                .color(palette.accent.color32()),
                        )
                        .on_hover_text(tip);
                    }
                });
            });
        });

    if let Some(err) = banner_error {
        egui::Panel::top("error_banner")
            .frame(
                Frame::new()
                    .fill(palette.chip_exclude_bg.color32())
                    .inner_margin(Margin::symmetric(
                        theme::SPACE_LG as i8,
                        theme::SPACE_SM as i8,
                    )),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(err).color(palette.chip_exclude_fg.color32()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Cerrar").clicked() {
                            outcome.dismiss_error = true;
                        }
                    });
                });
            });
    } else if let Some(notice) = status_notice {
        egui::Panel::top("status_banner")
            .frame(
                Frame::new()
                    .fill(palette.bg_card.color32())
                    .inner_margin(Margin::symmetric(
                        theme::SPACE_LG as i8,
                        theme::SPACE_XS as i8,
                    ))
                    .stroke(Stroke::new(1.0, palette.border.color32())),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(notice).color(palette.text_secondary.color32()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.button("Ocultar").clicked() {
                            outcome.dismiss_notice = true;
                        }
                    });
                });
            });
    }

    outcome
}

#[allow(clippy::too_many_arguments)]
pub fn draw_sidebar(
    ui: &mut egui::Ui,
    palette: &Palette,
    logo: Option<&TextureHandle>,
    active_view: ActiveView,
    reviewed_counts: ReviewedCounts,
    suggestions_count: usize,
    duplicates_count: usize,
    works_count: usize,
    persons_count: usize,
    rules_count: usize,
    all_labels: &[LabelNode],
    all_persons: &[catalog::Person],
    metadata_facets: &MetadataFacets,
    filter: &mut FilterState,
    sidebar_state: &mut SidebarState,
) -> SidebarOutcome {
    let mut outcome = SidebarOutcome::default();

    egui::Panel::left("sidebar")
        .default_size(theme::SIDEBAR_WIDTH)
        .size_range(224.0..=340.0)
        .resizable(true)
        .frame(
            Frame::new()
                .fill(palette.bg_panel.color32())
                .inner_margin(Margin::same(theme::SPACE_LG as i8))
                .stroke(Stroke::new(1.0, palette.border.color32())),
        )
        .show(ui, |ui| {
            // Brand header: logo + wordmark
            ui.horizontal(|ui| {
                if let Some(tex) = logo {
                    ui.image((tex.id(), Vec2::splat(24.0)));
                    ui.add_space(theme::SPACE_XS);
                }
                ui.label(
                    RichText::new("photosort")
                        .size(theme::FONT_HEADING)
                        .strong()
                        .color(palette.text_primary.color32()),
                );
            });
            ui.add_space(theme::SPACE_LG);

            // Reserve bottom space for pinned "Configuración" row.
            let footer_h = 56.0;
            let scroll_h = (ui.available_height() - footer_h).max(120.0);

            ScrollArea::vertical()
                .max_height(scroll_h)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Navigation section: Biblioteca
                    ui.label(
                        RichText::new("Biblioteca")
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                    );
                    ui.add_space(theme::SPACE_XS);

                    for (rev_filter, label, count) in [
                        (ReviewedFilter::All, "Todas", reviewed_counts.all),
                        (ReviewedFilter::Pending, "Sin revisar", reviewed_counts.pending),
                        (ReviewedFilter::Reviewed, "Revisadas", reviewed_counts.reviewed),
                    ] {
                        let is_active =
                            active_view == ActiveView::Gallery && filter.reviewed == rev_filter;
                        if nav_row(ui, palette, label, Some(count), is_active) {
                            outcome.switch_view = Some(ActiveView::Gallery);
                            if filter.reviewed != rev_filter {
                                outcome.set_reviewed_filter = Some(rev_filter);
                            }
                        }
                    }

                    ui.add_space(theme::SPACE_MD);

                    // Secondary sections
                    for (view, label, count) in [
                        (
                            ActiveView::Suggestions,
                            "Sugerencias",
                            (suggestions_count > 0).then_some(suggestions_count),
                        ),
                        (
                            ActiveView::Duplicates,
                            "Duplicados",
                            (duplicates_count > 0).then_some(duplicates_count),
                        ),
                        (
                            ActiveView::Works,
                            "Obras",
                            (works_count > 0).then_some(works_count),
                        ),
                        (
                            ActiveView::Persons,
                            "Personas",
                            (persons_count > 0).then_some(persons_count),
                        ),
                        (
                            ActiveView::Tags,
                            "Etiquetas",
                            (rules_count > 0).then_some(rules_count),
                        ),
                    ] {
                        if nav_row(ui, palette, label, count, active_view == view) {
                            outcome.switch_view = Some(view);
                        }
                    }

                    ui.add_space(theme::SPACE_LG);
                    ui.separator();
                    ui.add_space(theme::SPACE_MD);

                    // Collapsible filter groups
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Filtros")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                        ui.label(
                            RichText::new("?")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_hint.color32()),
                        )
                        .on_hover_text(
                            "Clic para alternar: exigir (Y), incluir alguna (O), excluir (NO) o quitar. Clic derecho para elegir directamente",
                        );
                        if filter.has_active_filters() {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("Limpiar")
                                                .size(theme::FONT_SMALL)
                                                .color(palette.accent.color32()),
                                        )
                                        .frame(false),
                                    )
                                    .clicked()
                                {
                                    filter.clear_filters();
                                    outcome.filter_changed = true;
                                }
                            });
                        }
                    });
                    ui.add_space(theme::SPACE_SM);

                    let visible_cats = flatten_category_tree(all_labels, false);
                    let visible_tags: Vec<&LabelNode> = all_labels
                        .iter()
                        .filter(|l| l.kind == LabelKind::Tag && l.images > 0)
                        .collect();
                    let visible_ents: Vec<&LabelNode> = all_labels
                        .iter()
                        .filter(|l| l.kind == LabelKind::Entity && l.images > 0)
                        .collect();

                    if visible_cats.is_empty()
                        && visible_tags.is_empty()
                        && visible_ents.is_empty()
                        && metadata_facets.media_types.is_empty()
                        && metadata_facets.formats.is_empty()
                        && metadata_facets.orientations.is_empty()
                        && metadata_facets.years.is_empty()
                    {
                        ui.label(
                            RichText::new("Sin etiquetas todavía")
                                .size(theme::FONT_SMALL)
                                .color(palette.text_hint.color32()),
                        );
                    } else {
                        draw_media_type_filter_group(
                            ui,
                            palette,
                            metadata_facets,
                            filter,
                            &mut outcome,
                        );
                        draw_category_tree_group(
                            ui,
                            palette,
                            &visible_cats,
                            filter,
                            &mut sidebar_state.cat_filter,
                            &mut outcome,
                        );
                        draw_flat_filter_group(
                            ui,
                            palette,
                            "Etiquetas",
                            LabelKind::Tag,
                            &visible_tags,
                            filter,
                            &mut sidebar_state.tag_filter,
                            true,
                            &mut outcome,
                        );
                        draw_flat_filter_group(
                            ui,
                            palette,
                            "Entidades",
                            LabelKind::Entity,
                            &visible_ents,
                            filter,
                            &mut sidebar_state.ent_filter,
                            false,
                            &mut outcome,
                        );
                        draw_format_filter_group(
                            ui,
                            palette,
                            metadata_facets,
                            filter,
                            &mut outcome,
                        );
                        draw_orientation_filter_group(
                            ui,
                            palette,
                            metadata_facets,
                            filter,
                            &mut outcome,
                        );
                        draw_year_filter_group(ui, palette, metadata_facets, filter, &mut outcome);
                        draw_persons_filter_group(ui, palette, all_persons, filter, &mut outcome);
                    }
                });

            // Bottom pinned Configuración
            ui.add_space(theme::SPACE_SM);
            ui.separator();
            ui.add_space(theme::SPACE_XS);
            if nav_row(
                ui,
                palette,
                "Configuración",
                None,
                active_view == ActiveView::Settings,
            ) {
                outcome.switch_view = Some(ActiveView::Settings);
            }
        });

    outcome
}

fn nav_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    count: Option<usize>,
    active: bool,
) -> bool {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 32.0), egui::Sense::click());
    let bg = if active {
        palette.accent_bg.color32()
    } else if resp.hovered() {
        palette.bg_elevated.color32()
    } else {
        Color32::TRANSPARENT
    };
    let border = if active {
        Stroke::new(1.0, palette.accent.color32())
    } else if resp.has_focus() {
        Stroke::new(theme::FOCUS_STROKE_WIDTH, palette.accent.color32())
    } else {
        Stroke::NONE
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(theme::RADIUS_MD),
        bg,
        border,
        egui::StrokeKind::Inside,
    );

    let text_color = if active {
        palette.text_primary.color32()
    } else {
        palette.text_secondary.color32()
    };
    ui.painter().text(
        egui::pos2(rect.left() + theme::SPACE_MD, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(theme::FONT_BODY),
        text_color,
    );
    if let Some(n) = count {
        ui.painter().text(
            egui::pos2(rect.right() - theme::SPACE_MD, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            n.to_string(),
            egui::FontId::monospace(theme::FONT_SMALL),
            palette.text_hint.color32(),
        );
    }

    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Button, true, active, label.to_string())
    });

    ui.add_space(2.0);
    resp.clicked()
}

fn chip_colors(palette: &Palette, mode: Option<ChipMode>) -> (Color32, Color32, Stroke) {
    match mode {
        None => (
            palette.bg_card.color32(),
            palette.text_primary.color32(),
            Stroke::new(1.0, palette.border.color32()),
        ),
        Some(ChipMode::Require) => {
            let c = palette.chip_require_fg.color32();
            (palette.chip_require_bg.color32(), c, Stroke::new(1.5, c))
        }
        Some(ChipMode::Any) => {
            let c = palette.chip_any_fg.color32();
            (palette.chip_any_bg.color32(), c, Stroke::new(1.5, c))
        }
        Some(ChipMode::Exclude) => {
            let c = palette.chip_exclude_fg.color32();
            (palette.chip_exclude_bg.color32(), c, Stroke::new(1.5, c))
        }
    }
}

fn attach_label_chip_context_menu(
    resp: &egui::Response,
    kind: LabelKind,
    id: LabelId,
    current_mode: Option<ChipMode>,
    set_chip: &mut Option<(LabelId, Option<ChipMode>)>,
    label_action: &mut Option<LabelAction>,
) {
    if kind == LabelKind::Entity {
        if let Some(chosen_mode) = attach_chip_context_menu(resp, current_mode) {
            *set_chip = Some((id, chosen_mode));
        }
        return;
    }
    resp.context_menu(|ui| {
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Require),
                egui::Button::new("Exigir"),
            )
            .clicked()
        {
            *set_chip = Some((id, Some(ChipMode::Require)));
            ui.close();
        }
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Any),
                egui::Button::new("Incluir alguna"),
            )
            .clicked()
        {
            *set_chip = Some((id, Some(ChipMode::Any)));
            ui.close();
        }
        if ui
            .add_enabled(
                current_mode != Some(ChipMode::Exclude),
                egui::Button::new("Excluir"),
            )
            .clicked()
        {
            *set_chip = Some((id, Some(ChipMode::Exclude)));
            ui.close();
        }
        if ui
            .add_enabled(current_mode.is_some(), egui::Button::new("Quitar filtro"))
            .clicked()
        {
            *set_chip = Some((id, None));
            ui.close();
        }
        ui.separator();
        if let Some(act) = draw_label_menu_items(ui, kind, id) {
            *label_action = Some(act);
        }
    });
}

fn draw_category_tree_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    rows: &[(&LabelNode, usize)],
    filter: &mut FilterState,
    group_query: &mut String,
    outcome: &mut SidebarOutcome,
) {
    if rows.is_empty() {
        return;
    }

    let header_title = format!("Categorías ({})", rows.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt("filter_group_Categorías")
    .default_open(true)
    .show(ui, |ui| {
        if rows.len() > LONG_FILTER_GROUP_THRESHOLD {
            ui.add(
                egui::TextEdit::singleline(group_query)
                    .desired_width(ui.available_width())
                    .hint_text(theme::hint_text(palette, "Filtrar…")),
            );
            ui.add_space(theme::SPACE_XS);
        } else if !group_query.is_empty() {
            group_query.clear();
        }

        let q = group_query.trim().to_lowercase();
        let mut set_chip: Option<(LabelId, Option<ChipMode>)> = None;

        for &(node, depth) in rows {
            if !q.is_empty() && !node.name.to_lowercase().contains(&q) {
                continue;
            }
            let mode = filter.chip_mode(node.label);
            let count = node.images.max(0) as usize;
            let (bg, fg, stroke) = chip_colors(palette, mode);

            ui.horizontal(|ui| {
                if depth > 0 {
                    ui.add_space((depth.min(5) as f32) * 14.0);
                    ui.label(
                        RichText::new("└")
                            .size(theme::FONT_SMALL)
                            .color(palette.text_hint.color32()),
                    );
                }
                let text = format_chip_label(mode, &node.name, count);
                let mut rt = RichText::new(text).size(theme::FONT_SMALL).color(fg);
                if mode.is_some() {
                    rt = rt.strong();
                }
                let btn = egui::Button::new(rt)
                    .fill(bg)
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .truncate();
                let resp = ui
                    .add(btn)
                    .on_hover_text(format_chip_tooltip(mode, &node.name, count));
                if resp.clicked() {
                    filter.cycle_chip(node.label);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
                attach_label_chip_context_menu(
                    &resp,
                    LabelKind::Category,
                    node.label,
                    mode,
                    &mut set_chip,
                    &mut outcome.label_action,
                );
            });
            ui.add_space(2.0);
        }

        if let Some((id, mode)) = set_chip {
            filter.set_chip(id, mode);
            outcome.filter_changed = true;
            outcome.switch_view = Some(ActiveView::Gallery);
        }
    });
    ui.add_space(theme::SPACE_XS);
}

#[allow(clippy::too_many_arguments)]
fn draw_flat_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    heading: &str,
    kind: LabelKind,
    group: &[&LabelNode],
    filter: &mut FilterState,
    group_query: &mut String,
    default_open: bool,
    outcome: &mut SidebarOutcome,
) {
    if group.is_empty() {
        return;
    }

    let header_title = format!("{heading} ({})", group.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt(format!("filter_group_{heading}"))
    .default_open(default_open)
    .show(ui, |ui| {
        if group.len() > LONG_FILTER_GROUP_THRESHOLD {
            ui.add(
                egui::TextEdit::singleline(group_query)
                    .desired_width(ui.available_width())
                    .hint_text(theme::hint_text(palette, "Filtrar…")),
            );
            ui.add_space(theme::SPACE_XS);
        } else if !group_query.is_empty() {
            group_query.clear();
        }

        let q = group_query.trim().to_lowercase();
        let mut set_chip: Option<(LabelId, Option<ChipMode>)> = None;

        ui.horizontal_wrapped(|ui| {
            for lc in group {
                if !q.is_empty() && !lc.name.to_lowercase().contains(&q) {
                    continue;
                }
                let mode = filter.chip_mode(lc.label);
                let count = lc.images.max(0) as usize;
                let (bg, fg, stroke) = chip_colors(palette, mode);
                let text = format_chip_label(mode, &lc.name, count);
                let mut rt = RichText::new(text).size(theme::FONT_SMALL).color(fg);
                if mode.is_some() {
                    rt = rt.strong();
                }
                let btn = egui::Button::new(rt)
                    .fill(bg)
                    .stroke(stroke)
                    .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                    .truncate();
                let resp = ui
                    .add(btn)
                    .on_hover_text(format_chip_tooltip(mode, &lc.name, count));
                if resp.clicked() {
                    filter.cycle_chip(lc.label);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
                attach_label_chip_context_menu(
                    &resp,
                    kind,
                    lc.label,
                    mode,
                    &mut set_chip,
                    &mut outcome.label_action,
                );
            }
        });

        if let Some((id, mode)) = set_chip {
            filter.set_chip(id, mode);
            outcome.filter_changed = true;
            outcome.switch_view = Some(ActiveView::Gallery);
        }
    });
    ui.add_space(theme::SPACE_XS);
}

fn draw_toggle_chip(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    count: usize,
    active: bool,
) -> egui::Response {
    let (bg, fg, stroke) = if active {
        let c = palette.chip_require_fg.color32();
        (palette.chip_require_bg.color32(), c, Stroke::new(1.5, c))
    } else {
        (
            palette.bg_card.color32(),
            palette.text_primary.color32(),
            Stroke::new(1.0, palette.border.color32()),
        )
    };
    let mut rt = RichText::new(format!("{label} ({count})"))
        .size(theme::FONT_SMALL)
        .color(fg);
    if active {
        rt = rt.strong();
    }
    let btn = egui::Button::new(rt)
        .fill(bg)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(theme::RADIUS_SM));
    ui.add(btn)
}

fn draw_media_type_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    facets: &MetadataFacets,
    filter: &mut FilterState,
    outcome: &mut SidebarOutcome,
) {
    if facets.media_types.is_empty() {
        return;
    }
    let header_title = format!("Tipo de medio ({})", facets.media_types.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt("filter_group_media_type")
    .default_open(true)
    .show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for mtc in &facets.media_types {
                let active = filter.media_types.contains(&mtc.media_type);
                let count = mtc.count.max(0) as usize;
                let label = match mtc.media_type {
                    catalog::MediaType::Image => "Fotos",
                    catalog::MediaType::Video => "Vídeos",
                };
                if draw_toggle_chip(ui, palette, label, count, active).clicked() {
                    filter.toggle_media_type(mtc.media_type);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
            }
        });
    });
    ui.add_space(theme::SPACE_XS);
}

fn draw_format_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    facets: &MetadataFacets,
    filter: &mut FilterState,
    outcome: &mut SidebarOutcome,
) {
    if facets.formats.is_empty() {
        return;
    }
    let header_title = format!("Formato ({})", facets.formats.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt("filter_group_format")
    .default_open(false)
    .show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for fc in &facets.formats {
                let active = filter.formats.contains(&fc.format);
                let count = fc.images.max(0) as usize;
                if draw_toggle_chip(ui, palette, &fc.format, count, active).clicked() {
                    filter.toggle_format(&fc.format);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
            }
        });
    });
    ui.add_space(theme::SPACE_XS);
}

fn orientation_label(orient: Orientation) -> &'static str {
    match orient {
        Orientation::Landscape => "Horizontal",
        Orientation::Portrait => "Vertical",
        Orientation::Square => "Cuadrada",
    }
}

fn draw_orientation_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    facets: &MetadataFacets,
    filter: &mut FilterState,
    outcome: &mut SidebarOutcome,
) {
    if facets.orientations.is_empty() {
        return;
    }
    let header_title = format!("Orientación ({})", facets.orientations.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt("filter_group_orientation")
    .default_open(false)
    .show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for oc in &facets.orientations {
                let active = filter.orientations.contains(&oc.orientation);
                let count = oc.images.max(0) as usize;
                if draw_toggle_chip(
                    ui,
                    palette,
                    orientation_label(oc.orientation),
                    count,
                    active,
                )
                .clicked()
                {
                    filter.toggle_orientation(oc.orientation);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
            }
        });
    });
    ui.add_space(theme::SPACE_XS);
}

fn draw_year_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    facets: &MetadataFacets,
    filter: &mut FilterState,
    outcome: &mut SidebarOutcome,
) {
    if facets.years.is_empty() {
        return;
    }
    let header_title = format!("Año de captura ({})", facets.years.len());
    egui::CollapsingHeader::new(
        RichText::new(header_title)
            .size(theme::FONT_SMALL)
            .color(palette.text_secondary.color32()),
    )
    .id_salt("filter_group_capture_year")
    .default_open(false)
    .show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            for yc in &facets.years {
                let active = filter.capture_years.contains(&yc.year);
                let count = yc.images.max(0) as usize;
                let year_text = yc.year.to_string();
                if draw_toggle_chip(ui, palette, &year_text, count, active).clicked() {
                    filter.toggle_capture_year(yc.year);
                    outcome.filter_changed = true;
                    outcome.switch_view = Some(ActiveView::Gallery);
                }
            }
        });
    });
    ui.add_space(theme::SPACE_XS);
}

fn draw_persons_filter_group(
    ui: &mut egui::Ui,
    palette: &Palette,
    persons: &[catalog::Person],
    filter: &mut FilterState,
    outcome: &mut SidebarOutcome,
) {
    if persons.is_empty() {
        return;
    }
    let any_active = persons.iter().any(|p| filter.persons.contains(&p.id));
    let title = if any_active {
        format!("Personas ({})*", persons.len())
    } else {
        format!("Personas ({})", persons.len())
    };

    egui::CollapsingHeader::new(RichText::new(title).size(theme::FONT_BODY))
        .id_salt("sidebar_filter_persons")
        .default_open(any_active)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for person in persons.iter().take(16) {
                    let active = filter.persons.contains(&person.id);
                    let name = person.name.as_deref().unwrap_or("Sin nombre");
                    if draw_toggle_chip(ui, palette, name, person.photo_count, active).clicked() {
                        if !filter.persons.remove(&person.id) {
                            filter.persons.insert(person.id);
                        }
                        outcome.filter_changed = true;
                        outcome.switch_view = Some(ActiveView::Gallery);
                    }
                }
            });
        });
    ui.add_space(theme::SPACE_XS);
}

/// First-run welcome screen when no library folders are registered.
/// Returns `true` if the user clicked the primary "Añadir carpeta…" button.
pub fn draw_welcome(ui: &mut egui::Ui, palette: &Palette, logo: Option<&TextureHandle>) -> bool {
    let mut add_clicked = false;

    egui::CentralPanel::default()
        .frame(Frame::new().fill(palette.bg_canvas.color32()))
        .show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.24).max(theme::SPACE_2XL));
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_LG))
                    .inner_margin(Margin::same(theme::SPACE_2XL as i8))
                    .show(ui, |ui| {
                        ui.set_max_width(440.0);
                        ui.vertical_centered(|ui| {
                            if let Some(tex) = logo {
                                ui.image((tex.id(), Vec2::splat(56.0)));
                                ui.add_space(theme::SPACE_LG);
                            }
                            ui.label(
                                RichText::new("Elige las carpetas con tus fotos")
                                    .size(theme::FONT_TITLE)
                                    .color(palette.text_primary.color32()),
                            );
                            ui.add_space(theme::SPACE_LG);
                            if ui
                                .add(
                                    theme::primary_button(palette, "Añadir carpeta…")
                                        .min_size(Vec2::new(180.0, 40.0)),
                                )
                                .clicked()
                            {
                                add_clicked = true;
                            }
                            ui.add_space(theme::SPACE_LG);
                            ui.label(
                                RichText::new(
                                    "photosort solo lee tus carpetas y nunca modifica ni borra los originales",
                                )
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                            );
                            ui.add_space(theme::SPACE_XS);
                            ui.label(
                                RichText::new(
                                    "Todo funciona en tu equipo, sin subir fotos ni enviar datos fuera",
                                )
                                .size(theme::FONT_SMALL)
                                .color(palette.text_hint.color32()),
                            );
                        });
                    });
            });
        });

    add_clicked
}
