use catalog::{AssetId, Assignment, Catalog, CatalogError, LabelId, LabelKind, Source, Status};
use std::collections::HashMap;
use std::path::PathBuf;

pub const THRESHOLD_MIN: f64 = 0.30;
pub const THRESHOLD_MAX: f64 = 0.95;
pub const THRESHOLD_DEFAULT: f64 = 0.60;

// shortcut: bounded in-memory undo stack per session; persist or enlarge if long review sessions
// ever need more than 100 steps.
const MAX_UNDO_STEPS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Accept,
    Reject,
}

impl ReviewDecision {
    fn target_status(self) -> Status {
        match self {
            Self::Accept => Status::Accepted,
            Self::Reject => Status::Rejected,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewRow {
    pub asset: AssetId,
    pub label: LabelId,
    pub label_name: String,
    pub label_kind: LabelKind,
    pub confidence: f64,
    pub source: Source,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReviewGroup {
    pub label: LabelId,
    pub name: String,
    pub kind: LabelKind,
    pub items: Vec<ReviewRow>,
}

impl ReviewGroup {
    pub fn count(&self) -> usize {
        self.items.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGroupAction {
    pub label: LabelId,
    pub group_name: String,
    pub count: usize,
    pub decision: ReviewDecision,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UndoItem {
    pub asset: AssetId,
    pub label: LabelId,
    pub source: Source,
    pub confidence: f64,
    pub applied_status: Status,
    pub previous_status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UndoAction {
    pub description: String,
    pub items: Vec<UndoItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UndoOutcome {
    pub restored: usize,
    pub skipped: usize,
}

/// Pure, UI-independent state machine for the review queue: threshold filtering, grouping by label,
/// cursor navigation, confirmed group actions, and safe undo.
#[derive(Debug, Clone)]
pub struct ReviewFlow {
    threshold: f64,
    groups: Vec<ReviewGroup>,
    active_group: Option<LabelId>,
    cursor: usize,
    pending_group_action: Option<PendingGroupAction>,
    undo_stack: Vec<UndoAction>,
}

impl Default for ReviewFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReviewFlow {
    pub fn new() -> Self {
        Self {
            threshold: THRESHOLD_DEFAULT,
            groups: Vec::new(),
            active_group: None,
            cursor: 0,
            pending_group_action: None,
            undo_stack: Vec::new(),
        }
    }

    pub fn threshold(&self) -> f64 {
        self.threshold
    }

    /// Updates the review confidence threshold clamped to `[THRESHOLD_MIN, THRESHOLD_MAX]`.
    /// Returns `true` when the value changed.
    pub fn set_threshold(&mut self, value: f64) -> bool {
        if value.is_nan() {
            return false;
        }
        let clamped = value.clamp(THRESHOLD_MIN, THRESHOLD_MAX);
        if (clamped - self.threshold).abs() < 1e-9 {
            return false;
        }
        self.threshold = clamped;
        self.pending_group_action = None;
        true
    }

    pub fn groups(&self) -> &[ReviewGroup] {
        &self.groups
    }

    pub fn total_items(&self) -> usize {
        self.groups.iter().map(ReviewGroup::count).sum()
    }

    pub fn active_group_label(&self) -> Option<LabelId> {
        self.active_group
    }

    pub fn active_group(&self) -> Option<&ReviewGroup> {
        let label = self.active_group?;
        self.groups.iter().find(|g| g.label == label)
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn current_row(&self) -> Option<&ReviewRow> {
        self.active_group()?.items.get(self.cursor)
    }

    pub fn pending_group_action(&self) -> Option<&PendingGroupAction> {
        self.pending_group_action.as_ref()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.undo_stack.len()
    }

    pub fn last_undo_description(&self) -> Option<&str> {
        self.undo_stack.last().map(|u| u.description.as_str())
    }

    /// Reloads pending suggestions below `threshold` from `catalog` and regroups them by label.
    pub fn reload(&mut self, catalog: &Catalog) -> Result<(), CatalogError> {
        let prev_group_idx = self
            .active_group
            .and_then(|id| self.groups.iter().position(|g| g.label == id));

        let raw = catalog.review_queue(self.threshold)?;
        let mut entry_cache: HashMap<AssetId, Option<String>> = HashMap::new();
        let mut labels_cache: HashMap<AssetId, Vec<Assignment>> = HashMap::new();
        let mut by_label: HashMap<LabelId, ReviewGroup> = HashMap::new();

        for item in raw {
            let sha256 = if let Some(cached) = entry_cache.get(&item.asset) {
                cached.clone()
            } else {
                let sha = catalog.entry_for(item.asset)?.map(|e| e.sha256);
                entry_cache.insert(item.asset, sha.clone());
                sha
            };
            let Some(sha256) = sha256 else {
                continue;
            };

            let assignments = if let Some(cached) = labels_cache.get(&item.asset) {
                cached
            } else {
                let list = catalog.labels_of(item.asset)?;
                labels_cache.entry(item.asset).or_insert(list)
            };
            let Some(assign) = assignments.iter().find(|a| a.label == item.label) else {
                continue;
            };

            let row = ReviewRow {
                asset: item.asset,
                label: item.label,
                label_name: item.name.clone(),
                label_kind: assign.kind,
                confidence: item.confidence,
                source: assign.source,
                path: item.path,
                sha256,
            };

            by_label
                .entry(item.label)
                .or_insert_with(|| ReviewGroup {
                    label: item.label,
                    name: item.name,
                    kind: assign.kind,
                    items: Vec::new(),
                })
                .items
                .push(row);
        }

        let mut groups: Vec<ReviewGroup> = by_label.into_values().collect();
        groups.sort_by(|a, b| {
            b.count()
                .cmp(&a.count())
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.label.cmp(&b.label))
        });

        self.groups = groups;
        self.reconcile_selection(prev_group_idx);
        Ok(())
    }

    fn reconcile_selection(&mut self, prev_group_idx: Option<usize>) {
        if let Some(active) = self.active_group
            && let Some(group) = self.groups.iter().find(|g| g.label == active)
        {
            self.cursor = self.cursor.min(group.items.len().saturating_sub(1));
        } else if !self.groups.is_empty() {
            let idx = prev_group_idx.unwrap_or(0).min(self.groups.len() - 1);
            self.active_group = Some(self.groups[idx].label);
            self.cursor = 0;
        } else {
            self.active_group = None;
            self.cursor = 0;
        }

        if let Some(pending) = &self.pending_group_action {
            let still_matches = self
                .groups
                .iter()
                .find(|g| g.label == pending.label)
                .is_some_and(|g| g.count() == pending.count);
            if !still_matches {
                self.pending_group_action = None;
            }
        }
    }

    pub fn select_group(&mut self, label: LabelId) {
        if self.groups.iter().any(|g| g.label == label) {
            self.active_group = Some(label);
            self.cursor = 0;
            self.pending_group_action = None;
        }
    }

    pub fn select_row(&mut self, idx: usize) {
        if let Some(group) = self.active_group()
            && !group.items.is_empty()
        {
            self.cursor = idx.min(group.items.len() - 1);
        }
    }

    pub fn move_prev(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_next(&mut self) {
        if let Some(group) = self.active_group()
            && !group.items.is_empty()
        {
            self.cursor = (self.cursor + 1).min(group.items.len() - 1);
        }
    }

    pub fn move_group_prev(&mut self) {
        if let Some(active) = self.active_group
            && let Some(pos) = self.groups.iter().position(|g| g.label == active)
            && pos > 0
        {
            self.active_group = Some(self.groups[pos - 1].label);
            self.cursor = 0;
            self.pending_group_action = None;
        }
    }

    pub fn move_group_next(&mut self) {
        if let Some(active) = self.active_group
            && let Some(pos) = self.groups.iter().position(|g| g.label == active)
            && pos + 1 < self.groups.len()
        {
            self.active_group = Some(self.groups[pos + 1].label);
            self.cursor = 0;
            self.pending_group_action = None;
        }
    }

    pub fn decide_current(
        &mut self,
        catalog: &Catalog,
        decision: ReviewDecision,
    ) -> Result<bool, CatalogError> {
        self.decide_at(catalog, self.cursor, decision)
    }

    /// Applies `decision` to the row at `row_idx` in the active group, records an undo entry,
    /// and refreshes the queue so the cursor naturally advances to the next item.
    pub fn decide_at(
        &mut self,
        catalog: &Catalog,
        row_idx: usize,
        decision: ReviewDecision,
    ) -> Result<bool, CatalogError> {
        let Some(row) = self
            .active_group()
            .and_then(|g| g.items.get(row_idx))
            .cloned()
        else {
            return Ok(false);
        };

        apply_decision(catalog, row.asset, row.label, decision)?;

        let verb = match decision {
            ReviewDecision::Accept => "Aceptar",
            ReviewDecision::Reject => "Rechazar",
        };
        self.push_undo(UndoAction {
            description: format!("{verb} \"{}\"", row.label_name),
            items: vec![UndoItem {
                asset: row.asset,
                label: row.label,
                source: row.source,
                confidence: row.confidence,
                applied_status: decision.target_status(),
                previous_status: Status::Suggested,
            }],
        });

        self.cursor = row_idx;
        self.pending_group_action = None;
        self.reload(catalog)?;
        Ok(true)
    }

    /// Stages a bulk accept/reject on the active group so the UI can ask for explicit confirmation.
    pub fn request_group_action(&mut self, decision: ReviewDecision) -> bool {
        let Some(group) = self.active_group() else {
            return false;
        };
        if group.items.is_empty() {
            return false;
        }
        self.pending_group_action = Some(PendingGroupAction {
            label: group.label,
            group_name: group.name.clone(),
            count: group.count(),
            decision,
        });
        true
    }

    pub fn cancel_group_action(&mut self) {
        self.pending_group_action = None;
    }

    /// Executes the pending group action across every item currently in that group and records a
    /// single grouped undo step.
    pub fn confirm_group_action(&mut self, catalog: &Catalog) -> Result<usize, CatalogError> {
        let Some(pending) = self.pending_group_action.take() else {
            return Ok(0);
        };
        let items: Vec<ReviewRow> = self
            .groups
            .iter()
            .find(|g| g.label == pending.label)
            .map(|g| g.items.clone())
            .unwrap_or_default();
        if items.is_empty() {
            return Ok(0);
        }

        let mut undo_items = Vec::with_capacity(items.len());
        for row in &items {
            apply_decision(catalog, row.asset, row.label, pending.decision)?;
            undo_items.push(UndoItem {
                asset: row.asset,
                label: row.label,
                source: row.source,
                confidence: row.confidence,
                applied_status: pending.decision.target_status(),
                previous_status: Status::Suggested,
            });
        }

        let verb = match pending.decision {
            ReviewDecision::Accept => "Aceptar grupo",
            ReviewDecision::Reject => "Rechazar grupo",
        };
        let count = undo_items.len();
        self.push_undo(UndoAction {
            description: format!("{verb} \"{}\" ({count})", pending.group_name),
            items: undo_items,
        });

        self.reload(catalog)?;
        Ok(count)
    }

    /// Undoes the most recent single or group decision, restoring each item only if the catalog
    /// row was not replaced or changed in the meantime.
    pub fn undo(&mut self, catalog: &Catalog) -> Result<Option<UndoOutcome>, CatalogError> {
        let Some(action) = self.undo_stack.pop() else {
            return Ok(None);
        };
        self.pending_group_action = None;

        let mut restored = 0usize;
        let mut skipped = 0usize;
        let mut focus_target: Option<(LabelId, AssetId)> = None;

        for item in &action.items {
            let changed = catalog.restore_status(
                item.asset,
                item.label,
                item.source,
                item.confidence,
                item.applied_status,
                item.previous_status,
            )?;
            if changed {
                restored += 1;
                focus_target.get_or_insert((item.label, item.asset));
            } else {
                skipped += 1;
            }
        }

        self.reload(catalog)?;
        if let Some((label, asset)) = focus_target
            && let Some(group) = self.groups.iter().find(|g| g.label == label)
        {
            self.active_group = Some(label);
            if let Some(pos) = group.items.iter().position(|r| r.asset == asset) {
                self.cursor = pos;
            }
        }

        Ok(Some(UndoOutcome { restored, skipped }))
    }

    fn push_undo(&mut self, action: UndoAction) {
        if self.undo_stack.len() >= MAX_UNDO_STEPS {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(action);
    }
}

fn apply_decision(
    catalog: &Catalog,
    asset: AssetId,
    label: LabelId,
    decision: ReviewDecision,
) -> Result<(), CatalogError> {
    match decision {
        ReviewDecision::Accept => catalog.accept(asset, label),
        ReviewDecision::Reject => catalog.reject(asset, label),
    }
}
