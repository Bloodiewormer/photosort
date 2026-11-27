use crate::model::count_phrase;
use crate::theme::{self, Palette};
use catalog::{Catalog, LabelId, LabelKind, LabelNode, RenameOutcome};
use eframe::egui::{
    self, Align, CornerRadius, Frame, Key, Layout, Margin, RichText, ScrollArea, Stroke,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelAction {
    CreateCategory { parent: Option<LabelId> },
    CreateTag,
    Rename(LabelId),
    MoveCategory(LabelId),
    Merge(LabelId),
    Delete(LabelId),
}

/// Flattens categories into pre-order tree rows `(node, depth)` sorted alphabetically per parent.
/// When `include_empty_roots` is false, top-level categories with 0 images and no visible children
/// are omitted (useful for the sidebar filter list while still showing every user subcategory).
pub fn flatten_category_tree(
    labels: &[LabelNode],
    include_empty_roots: bool,
) -> Vec<(&LabelNode, usize)> {
    let cats: Vec<&LabelNode> = labels
        .iter()
        .filter(|l| l.kind == LabelKind::Category)
        .collect();
    let by_id: HashMap<LabelId, &LabelNode> = cats.iter().map(|c| (c.label, *c)).collect();
    let mut children: HashMap<Option<LabelId>, Vec<&LabelNode>> = HashMap::new();
    for c in &cats {
        let parent_key = c.parent.filter(|p| by_id.contains_key(p));
        children.entry(parent_key).or_default().push(*c);
    }
    for list in children.values_mut() {
        list.sort_by_key(|a| a.name.to_lowercase());
    }

    let mut visible: HashSet<LabelId> = HashSet::new();
    if include_empty_roots {
        for c in &cats {
            visible.insert(c.label);
        }
    } else {
        for c in &cats {
            if c.images > 0 || c.parent.is_some() {
                let mut cur = Some(c.label);
                let mut guard = 0;
                while let Some(id) = cur
                    && guard < 64
                {
                    visible.insert(id);
                    cur = by_id.get(&id).and_then(|n| n.parent);
                    guard += 1;
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut visited = HashSet::new();
    fn visit<'a>(
        parent: Option<LabelId>,
        depth: usize,
        children: &HashMap<Option<LabelId>, Vec<&'a LabelNode>>,
        visible: &HashSet<LabelId>,
        visited: &mut HashSet<LabelId>,
        out: &mut Vec<(&'a LabelNode, usize)>,
    ) {
        let Some(list) = children.get(&parent) else {
            return;
        };
        for node in list {
            if !visible.contains(&node.label) || !visited.insert(node.label) {
                continue;
            }
            out.push((*node, depth));
            visit(Some(node.label), depth + 1, children, visible, visited, out);
        }
    }
    visit(None, 0, &children, &visible, &mut visited, &mut out);
    out
}

/// Returns all strict descendants of `root` in the category tree.
pub fn descendants_of(labels: &[LabelNode], root: LabelId) -> HashSet<LabelId> {
    let mut children: HashMap<LabelId, Vec<LabelId>> = HashMap::new();
    for l in labels {
        if l.kind == LabelKind::Category
            && let Some(p) = l.parent
        {
            children.entry(p).or_default().push(l.label);
        }
    }
    let mut out = HashSet::new();
    let mut stack = vec![root];
    while let Some(cur) = stack.pop() {
        if let Some(kids) = children.get(&cur) {
            for &k in kids {
                if out.insert(k) {
                    stack.push(k);
                }
            }
        }
    }
    out
}

/// Draws the management action menu items for a category or tag inside an existing popup/context menu.
pub fn draw_label_menu_items(
    ui: &mut egui::Ui,
    kind: LabelKind,
    id: LabelId,
) -> Option<LabelAction> {
    if kind == LabelKind::Entity {
        return None;
    }
    let mut chosen = None;
    if ui.button("Renombrar").clicked() {
        chosen = Some(LabelAction::Rename(id));
        ui.close();
    }
    if kind == LabelKind::Category {
        if ui.button("Crear subcategoría").clicked() {
            chosen = Some(LabelAction::CreateCategory { parent: Some(id) });
            ui.close();
        }
        if ui.button("Mover a…").clicked() {
            chosen = Some(LabelAction::MoveCategory(id));
            ui.close();
        }
    }
    if ui.button("Fusionar con…").clicked() {
        chosen = Some(LabelAction::Merge(id));
        ui.close();
    }
    if ui.button("Eliminar…").clicked() {
        chosen = Some(LabelAction::Delete(id));
        ui.close();
    }
    chosen
}

#[derive(Debug, Clone)]
enum ModalKind {
    CreateCategory {
        parent: Option<LabelId>,
        name: String,
    },
    CreateTag {
        name: String,
    },
    Rename {
        id: LabelId,
        old_name: String,
        draft: String,
        conflict: Option<(LabelId, String)>,
    },
    MoveCategory {
        id: LabelId,
        selected_parent: Option<LabelId>,
    },
    Merge {
        source: LabelId,
        target: Option<LabelId>,
        query: String,
    },
    Delete {
        id: LabelId,
    },
}

#[derive(Default)]
pub struct LabelActionModal {
    kind: Option<ModalKind>,
    focus_input: bool,
    error: Option<String>,
}

#[derive(Default)]
pub struct LabelModalOutcome {
    pub catalog_mutated: bool,
    pub removed_label: Option<LabelId>,
    pub status_notice: Option<String>,
}

impl LabelActionModal {
    pub fn is_open(&self) -> bool {
        self.kind.is_some()
    }

    pub fn open(&mut self, action: LabelAction, labels: &[LabelNode]) {
        self.error = None;
        self.focus_input = true;
        self.kind = match action {
            LabelAction::CreateCategory { parent } => Some(ModalKind::CreateCategory {
                parent,
                name: String::new(),
            }),
            LabelAction::CreateTag => Some(ModalKind::CreateTag {
                name: String::new(),
            }),
            LabelAction::Rename(id) => {
                labels
                    .iter()
                    .find(|l| l.label == id)
                    .map(|l| ModalKind::Rename {
                        id,
                        old_name: l.name.clone(),
                        draft: l.name.clone(),
                        conflict: None,
                    })
            }
            LabelAction::MoveCategory(id) => labels
                .iter()
                .find(|l| l.label == id && l.kind == LabelKind::Category)
                .map(|l| ModalKind::MoveCategory {
                    id,
                    selected_parent: l.parent,
                }),
            LabelAction::Merge(source) => {
                labels
                    .iter()
                    .find(|l| l.label == source)
                    .map(|_| ModalKind::Merge {
                        source,
                        target: None,
                        query: String::new(),
                    })
            }
            LabelAction::Delete(id) => labels
                .iter()
                .find(|l| l.label == id)
                .map(|_| ModalKind::Delete { id }),
        };
    }

    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        palette: &Palette,
        cat: Option<&mut Catalog>,
        labels: &[LabelNode],
    ) -> LabelModalOutcome {
        let mut outcome = LabelModalOutcome::default();
        if self.kind.is_none() {
            return outcome;
        }

        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            self.kind = None;
            self.error = None;
            return outcome;
        }

        let title = match self.kind.as_ref().expect("checked") {
            ModalKind::CreateCategory {
                parent: Some(_), ..
            } => "Nueva subcategoría",
            ModalKind::CreateCategory { parent: None, .. } => "Nueva categoría",
            ModalKind::CreateTag { .. } => "Nueva etiqueta",
            ModalKind::Rename { .. } => "Renombrar",
            ModalKind::MoveCategory { .. } => "Mover categoría",
            ModalKind::Merge { .. } => "Fusionar con…",
            ModalKind::Delete { .. } => "Eliminar del catálogo",
        };

        let mut close = false;
        let mut submit = false;
        let mut confirm_merge_from_rename: Option<(LabelId, LabelId, String)> = None;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size(egui::vec2(420.0, 0.0))
            .frame(
                Frame::new()
                    .fill(palette.bg_panel.color32())
                    .stroke(Stroke::new(1.0, palette.border.color32()))
                    .corner_radius(CornerRadius::same(theme::RADIUS_LG))
                    .inner_margin(Margin::same(theme::SPACE_LG as i8)),
            )
            .show(ctx, |ui| {
                match self.kind.as_mut().expect("checked") {
                    ModalKind::CreateCategory { parent, name } => {
                        if let Some(pid) = *parent
                            && let Some(p) = labels.iter().find(|l| l.label == pid)
                        {
                            ui.label(
                                RichText::new(format!("Dentro de «{}»", p.name))
                                    .size(theme::FONT_SMALL)
                                    .color(palette.text_secondary.color32()),
                            );
                            ui.add_space(theme::SPACE_SM);
                        }
                        let resp = theme::named(
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .desired_width(ui.available_width())
                                    .hint_text(theme::hint_text(palette, "Nombre de la categoría")),
                            ),
                            "Nombre de la categoría",
                        );
                        if self.focus_input {
                            resp.request_focus();
                            self.focus_input = false;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            submit = true;
                        }
                        if let Some(err) = &self.error {
                            ui.add_space(theme::SPACE_SM);
                            ui.label(
                                RichText::new(err)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                        ui.add_space(theme::SPACE_MD);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let can = !name.trim().is_empty();
                            if ui
                                .add_enabled(can, theme::primary_button(palette, "Crear"))
                                .clicked()
                            {
                                submit = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                close = true;
                            }
                        });
                    }
                    ModalKind::CreateTag { name } => {
                        let resp = theme::named(
                            ui.add(
                                egui::TextEdit::singleline(name)
                                    .desired_width(ui.available_width())
                                    .hint_text(theme::hint_text(palette, "Nombre de la etiqueta")),
                            ),
                            "Nombre de la etiqueta",
                        );
                        if self.focus_input {
                            resp.request_focus();
                            self.focus_input = false;
                        }
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            submit = true;
                        }
                        if let Some(err) = &self.error {
                            ui.add_space(theme::SPACE_SM);
                            ui.label(
                                RichText::new(err)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                        ui.add_space(theme::SPACE_MD);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let can = !name.trim().is_empty();
                            if ui
                                .add_enabled(can, theme::primary_button(palette, "Crear"))
                                .clicked()
                            {
                                submit = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                close = true;
                            }
                        });
                    }
                    ModalKind::Rename {
                        id,
                        old_name,
                        draft,
                        conflict,
                    } => {
                        ui.label(
                            RichText::new(format!("Nombre actual: «{old_name}»"))
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                        ui.add_space(theme::SPACE_SM);
                        let resp = theme::named(
                            ui.add(
                                egui::TextEdit::singleline(draft)
                                    .desired_width(ui.available_width()),
                            ),
                            "Nuevo nombre",
                        );
                        if self.focus_input {
                            resp.request_focus();
                            self.focus_input = false;
                        }
                        if resp.changed() {
                            *conflict = None;
                            self.error = None;
                        }
                        if resp.lost_focus()
                            && ui.input(|i| i.key_pressed(Key::Enter))
                            && conflict.is_none()
                        {
                            submit = true;
                        }

                        if let Some((existing_id, existing_name)) = conflict.clone() {
                            ui.add_space(theme::SPACE_SM);
                            Frame::new()
                                .fill(palette.chip_any_bg.color32())
                                .stroke(Stroke::new(1.0, palette.accent.color32()))
                                .corner_radius(CornerRadius::same(theme::RADIUS_SM))
                                .inner_margin(Margin::same(theme::SPACE_SM as i8))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "Ya existe «{existing_name}». ¿Quieres fusionar «{old_name}» en «{existing_name}»? Se conservarán tus decisiones y ningún archivo cambiará."
                                        ))
                                        .size(theme::FONT_SMALL)
                                        .color(palette.chip_any_fg.color32()),
                                    );
                                    ui.add_space(theme::SPACE_SM);
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add(theme::primary_button(palette, "Fusionar"))
                                            .clicked()
                                        {
                                            confirm_merge_from_rename =
                                                Some((*id, existing_id, existing_name));
                                        }
                                        if ui.button("Cambiar nombre").clicked() {
                                            *conflict = None;
                                        }
                                    });
                                });
                        } else {
                            if let Some(err) = &self.error {
                                ui.add_space(theme::SPACE_SM);
                                ui.label(
                                    RichText::new(err)
                                        .size(theme::FONT_SMALL)
                                        .color(palette.status_rejected_fg.color32()),
                                );
                            }
                            ui.add_space(theme::SPACE_MD);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let can = !draft.trim().is_empty();
                                if ui
                                    .add_enabled(can, theme::primary_button(palette, "Guardar"))
                                    .clicked()
                                {
                                    submit = true;
                                }
                                if ui.button("Cancelar").clicked() {
                                    close = true;
                                }
                            });
                        }
                    }
                    ModalKind::MoveCategory {
                        id,
                        selected_parent,
                    } => {
                        let name = labels
                            .iter()
                            .find(|l| l.label == *id)
                            .map(|l| l.name.as_str())
                            .unwrap_or("Categoría");
                        ui.label(
                            RichText::new(format!("Elige el nuevo padre para «{name}»"))
                                .size(theme::FONT_SMALL)
                                .color(palette.text_secondary.color32()),
                        );
                        ui.add_space(theme::SPACE_SM);
                        let forbidden = descendants_of(labels, *id);
                        let tree = flatten_category_tree(labels, true);

                        ScrollArea::vertical()
                            .max_height(240.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                if ui
                                    .selectable_label(
                                        selected_parent.is_none(),
                                        "Nivel raíz (sin categoría padre)",
                                    )
                                    .clicked()
                                {
                                    *selected_parent = None;
                                }
                                for (node, depth) in tree {
                                    if node.label == *id || forbidden.contains(&node.label) {
                                        continue;
                                    }
                                    ui.horizontal(|ui| {
                                        if depth > 0 {
                                            ui.add_space(depth as f32 * 14.0);
                                        }
                                        let prefix = if depth > 0 { "└ " } else { "" };
                                        let is_sel = *selected_parent == Some(node.label);
                                        if ui
                                            .selectable_label(
                                                is_sel,
                                                format!("{prefix}{}", node.name),
                                            )
                                            .clicked()
                                        {
                                            *selected_parent = Some(node.label);
                                        }
                                    });
                                }
                            });

                        if let Some(err) = &self.error {
                            ui.add_space(theme::SPACE_SM);
                            ui.label(
                                RichText::new(err)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                        ui.add_space(theme::SPACE_MD);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add(theme::primary_button(palette, "Mover")).clicked() {
                                submit = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                close = true;
                            }
                        });
                    }
                    ModalKind::Merge {
                        source,
                        target,
                        query,
                    } => {
                        let src_node = labels.iter().find(|l| l.label == *source);
                        let src_name = src_node.map(|l| l.name.as_str()).unwrap_or("");
                        let src_kind = src_node.map(|l| l.kind).unwrap_or(LabelKind::Tag);

                        ui.label(
                            RichText::new(format!(
                                "Las fotos de «{src_name}» pasarán al destino elegido conservando tus decisiones (aceptada > rechazada > sugerida). Ningún archivo se modifica."
                            ))
                            .size(theme::FONT_SMALL)
                            .color(palette.text_secondary.color32()),
                        );
                        ui.add_space(theme::SPACE_SM);
                        theme::named(
                            ui.add(
                                egui::TextEdit::singleline(query)
                                    .desired_width(ui.available_width())
                                    .hint_text(theme::hint_text(palette, "Filtrar destino…")),
                            ),
                            "Filtrar destino",
                        );
                        ui.add_space(theme::SPACE_XS);
                        let q = query.trim().to_lowercase();

                        ScrollArea::vertical()
                            .max_height(220.0)
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                if src_kind == LabelKind::Category {
                                    for (node, depth) in flatten_category_tree(labels, true) {
                                        if node.label == *source {
                                            continue;
                                        }
                                        if !q.is_empty() && !node.name.to_lowercase().contains(&q) {
                                            continue;
                                        }
                                        ui.horizontal(|ui| {
                                            if depth > 0 {
                                                ui.add_space(depth as f32 * 14.0);
                                            }
                                            let prefix = if depth > 0 { "└ " } else { "" };
                                            let label_txt = format!(
                                                "{prefix}{} ({})",
                                                node.name,
                                                node.images.max(0)
                                            );
                                            if ui
                                                .selectable_label(
                                                    *target == Some(node.label),
                                                    label_txt,
                                                )
                                                .clicked()
                                            {
                                                *target = Some(node.label);
                                            }
                                        });
                                    }
                                } else {
                                    for node in labels.iter().filter(|l| {
                                        l.kind == src_kind && l.label != *source
                                    }) {
                                        if !q.is_empty() && !node.name.to_lowercase().contains(&q) {
                                            continue;
                                        }
                                        let label_txt =
                                            format!("{} ({})", node.name, node.images.max(0));
                                        if ui
                                            .selectable_label(
                                                *target == Some(node.label),
                                                label_txt,
                                            )
                                            .clicked()
                                        {
                                            *target = Some(node.label);
                                        }
                                    }
                                }
                            });

                        if let Some(err) = &self.error {
                            ui.add_space(theme::SPACE_SM);
                            ui.label(
                                RichText::new(err)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                        ui.add_space(theme::SPACE_MD);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add_enabled(
                                    target.is_some(),
                                    theme::primary_button(palette, "Fusionar"),
                                )
                                .clicked()
                            {
                                submit = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                close = true;
                            }
                        });
                    }
                    ModalKind::Delete { id } => {
                        let node = labels.iter().find(|l| l.label == *id);
                        let name = node.map(|l| l.name.as_str()).unwrap_or("");
                        let images = node.map(|l| l.images.max(0) as usize).unwrap_or(0);
                        let has_children = labels.iter().any(|l| l.parent == Some(*id));
                        let msg = if has_children {
                            format!(
                                "¿Eliminar «{name}» del catálogo? Sus subcategorías subirán un nivel y se quitarán sus asignaciones ({}). Ningún archivo se borra ni se modifica.",
                                count_phrase(images, "foto", "fotos")
                            )
                        } else {
                            format!(
                                "¿Eliminar «{name}» del catálogo? Solo se quita del catálogo ({}); ningún archivo se borra ni se modifica.",
                                count_phrase(images, "foto", "fotos")
                            )
                        };
                        ui.label(
                            RichText::new(msg)
                                .size(theme::FONT_BODY)
                                .color(palette.text_primary.color32()),
                        );
                        if let Some(err) = &self.error {
                            ui.add_space(theme::SPACE_SM);
                            ui.label(
                                RichText::new(err)
                                    .size(theme::FONT_SMALL)
                                    .color(palette.status_rejected_fg.color32()),
                            );
                        }
                        ui.add_space(theme::SPACE_MD);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button("Eliminar").clicked() {
                                submit = true;
                            }
                            if ui.button("Cancelar").clicked() {
                                close = true;
                            }
                        });
                    }
                }
            });

        if let Some((src, tgt, tgt_name)) = confirm_merge_from_rename
            && let Some(c) = cat
        {
            match c.merge_labels(src, tgt) {
                Ok(()) => {
                    outcome.catalog_mutated = true;
                    outcome.removed_label = Some(src);
                    outcome.status_notice = Some(format!("Fusionado en «{tgt_name}»"));
                    self.kind = None;
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                }
            }
            return outcome;
        }

        if close {
            self.kind = None;
            self.error = None;
            return outcome;
        }

        if submit && let Some(c) = cat {
            let result = match self.kind.as_mut().expect("checked") {
                ModalKind::CreateCategory { parent, name } => {
                    let n = name.trim().to_string();
                    c.create_category(&n, *parent).map(|_| {
                        outcome.status_notice = Some(format!("Categoría «{n}» creada"));
                    })
                }
                ModalKind::CreateTag { name } => {
                    let n = name.trim().to_string();
                    c.create_tag(&n).map(|_| {
                        outcome.status_notice = Some(format!("Etiqueta «{n}» creada"));
                    })
                }
                ModalKind::Rename {
                    id,
                    draft,
                    conflict,
                    ..
                } => {
                    let n = draft.trim().to_string();
                    match c.rename_label(*id, &n) {
                        Ok(RenameOutcome::Renamed) => {
                            outcome.status_notice = Some(format!("Renombrado a «{n}»"));
                            Ok(())
                        }
                        Ok(RenameOutcome::Conflict {
                            existing,
                            existing_name,
                        }) => {
                            *conflict = Some((existing, existing_name));
                            return outcome;
                        }
                        Err(e) => Err(e),
                    }
                }
                ModalKind::MoveCategory {
                    id,
                    selected_parent,
                } => c.move_category(*id, *selected_parent).map(|()| {
                    outcome.status_notice = Some("Categoría movida".into());
                }),
                ModalKind::Merge { source, target, .. } => {
                    let Some(tgt) = *target else {
                        return outcome;
                    };
                    let tgt_name = labels
                        .iter()
                        .find(|l| l.label == tgt)
                        .map(|l| l.name.clone())
                        .unwrap_or_default();
                    let src = *source;
                    c.merge_labels(src, tgt).map(|()| {
                        outcome.removed_label = Some(src);
                        outcome.status_notice = Some(format!("Fusionado en «{tgt_name}»"));
                    })
                }
                ModalKind::Delete { id } => {
                    let del = *id;
                    c.delete_label(del).map(|_| {
                        outcome.removed_label = Some(del);
                        outcome.status_notice = Some("Eliminado del catálogo".into());
                    })
                }
            };
            match result {
                Ok(()) => {
                    outcome.catalog_mutated = true;
                    self.kind = None;
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e.to_string());
                }
            }
        }

        outcome
    }
}
