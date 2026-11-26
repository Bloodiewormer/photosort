use catalog::{Entry, Filter, LabelId, Sort};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveDir {
    Left,
    Right,
    Up,
    Down,
}

/// Multi-selection over an ordered list of catalog entries (click, ctrl-click, shift-click,
/// keyboard arrows, select all, clear, invert).
#[derive(Debug, Default, Clone)]
pub struct Selection {
    selected: BTreeSet<usize>,
    anchor: Option<usize>,
    cursor: Option<usize>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    pub fn len(&self) -> usize {
        self.selected.len()
    }

    pub fn contains(&self, idx: usize) -> bool {
        self.selected.contains(&idx)
    }

    pub fn selected_indices(&self) -> Vec<usize> {
        self.selected.iter().copied().collect()
    }

    /// Primary focused entry for the inspector: cursor if still selected, otherwise first selected.
    pub fn primary(&self) -> Option<usize> {
        self.cursor
            .filter(|i| self.selected.contains(i))
            .or_else(|| self.selected.iter().next().copied())
    }

    pub fn click(&mut self, idx: usize, len: usize) {
        if idx >= len {
            return;
        }
        self.selected.clear();
        self.selected.insert(idx);
        self.anchor = Some(idx);
        self.cursor = Some(idx);
    }

    pub fn ctrl_click(&mut self, idx: usize, len: usize) {
        if idx >= len {
            return;
        }
        if !self.selected.remove(&idx) {
            self.selected.insert(idx);
        }
        self.anchor = Some(idx);
        self.cursor = Some(idx);
    }

    pub fn shift_click(&mut self, idx: usize, len: usize) {
        if idx >= len {
            return;
        }
        let Some(anchor) = self.anchor.filter(|&a| a < len) else {
            self.click(idx, len);
            return;
        };
        let (lo, hi) = if anchor <= idx {
            (anchor, idx)
        } else {
            (idx, anchor)
        };
        self.selected.clear();
        self.selected.extend(lo..=hi);
        self.cursor = Some(idx);
    }

    pub fn select_all(&mut self, len: usize) {
        self.selected.clear();
        self.selected.extend(0..len);
        if len > 0 {
            self.anchor.get_or_insert(0);
            self.cursor = self.cursor.filter(|&c| c < len).or(Some(0));
        } else {
            self.anchor = None;
            self.cursor = None;
        }
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.cursor = None;
    }

    pub fn invert(&mut self, len: usize) {
        let mut next = BTreeSet::new();
        for i in 0..len {
            if !self.selected.contains(&i) {
                next.insert(i);
            }
        }
        self.selected = next;
        if let Some(c) = self.cursor
            && !self.selected.contains(&c)
        {
            self.cursor = self.selected.iter().next().copied();
        }
        if let Some(a) = self.anchor
            && a >= len
        {
            self.anchor = self.cursor;
        }
    }

    pub fn move_cursor(&mut self, dir: MoveDir, columns: usize, len: usize, extend: bool) {
        if len == 0 {
            self.clear();
            return;
        }
        let cols = columns.max(1);
        let Some(cur) = self.cursor.or_else(|| self.primary()).filter(|&i| i < len) else {
            self.click(0, len);
            return;
        };
        let next = match dir {
            MoveDir::Left => cur.saturating_sub(1),
            MoveDir::Right => (cur + 1).min(len - 1),
            MoveDir::Up => cur.saturating_sub(cols),
            MoveDir::Down => (cur + cols).min(len - 1),
        };
        if extend {
            self.shift_click(next, len);
        } else {
            self.click(next, len);
        }
    }

    /// Re-maps selected indices by file path after the entries list is refreshed or re-sorted.
    pub fn reconcile(&mut self, old: &[Entry], new: &[Entry]) {
        let new_pos: HashMap<&PathBuf, usize> =
            new.iter().enumerate().map(|(i, e)| (&e.path, i)).collect();
        let map_idx = |i: usize| old.get(i).and_then(|e| new_pos.get(&e.path).copied());

        self.selected = self.selected.iter().copied().filter_map(map_idx).collect();
        self.anchor = self.anchor.and_then(map_idx);
        self.cursor = self.cursor.and_then(map_idx);
    }
}

/// Pure grid geometry for virtualized thumbnail rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridLayout {
    pub columns: usize,
    pub cell_width: f32,
    pub cell_height: f32,
    pub spacing: f32,
    pub item_count: usize,
    pub row_count: usize,
}

impl GridLayout {
    pub fn compute(
        available_width: f32,
        cell_width: f32,
        cell_height: f32,
        spacing: f32,
        item_count: usize,
    ) -> Self {
        let stride_x = (cell_width + spacing).max(1.0);
        let columns = (((available_width.max(0.0) + spacing) / stride_x).floor() as usize).max(1);
        let row_count = item_count.div_ceil(columns);
        Self {
            columns,
            cell_width,
            cell_height,
            spacing,
            item_count,
            row_count,
        }
    }

    pub fn row_stride(&self) -> f32 {
        (self.cell_height + self.spacing).max(1.0)
    }

    pub fn total_height(&self) -> f32 {
        if self.row_count == 0 {
            0.0
        } else {
            self.row_count as f32 * self.cell_height + (self.row_count - 1) as f32 * self.spacing
        }
    }

    pub fn row_indices(&self, row: usize) -> Range<usize> {
        let start = row.saturating_mul(self.columns).min(self.item_count);
        let end = start.saturating_add(self.columns).min(self.item_count);
        start..end
    }

    pub fn visible_rows(
        &self,
        scroll_y: f32,
        viewport_height: f32,
        overscan_rows: usize,
    ) -> Range<usize> {
        if self.row_count == 0 || viewport_height <= 0.0 {
            return 0..0;
        }
        let stride = self.row_stride();
        let top = scroll_y.max(0.0);
        let first = (top / stride).floor() as usize;
        let last_exclusive = ((top + viewport_height) / stride).ceil() as usize;
        let start = first.saturating_sub(overscan_rows).min(self.row_count);
        let end = last_exclusive
            .saturating_add(overscan_rows)
            .min(self.row_count);
        start..end
    }

    pub fn visible_indices(
        &self,
        scroll_y: f32,
        viewport_height: f32,
        overscan_rows: usize,
    ) -> Range<usize> {
        let rows = self.visible_rows(scroll_y, viewport_height, overscan_rows);
        if rows.is_empty() {
            return 0..0;
        }
        let start = rows.start.saturating_mul(self.columns).min(self.item_count);
        let end = rows.end.saturating_mul(self.columns).min(self.item_count);
        start..end
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipMode {
    Require,
    Any,
    Exclude,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterState {
    pub search: String,
    pub chips: BTreeMap<LabelId, ChipMode>,
    pub sort: Sort,
}

impl Default for FilterState {
    fn default() -> Self {
        Self {
            search: String::new(),
            chips: BTreeMap::new(),
            sort: Sort::PathAsc,
        }
    }
}

impl FilterState {
    pub fn chip_mode(&self, id: LabelId) -> Option<ChipMode> {
        self.chips.get(&id).copied()
    }

    pub fn set_chip(&mut self, id: LabelId, mode: Option<ChipMode>) {
        match mode {
            Some(m) => {
                self.chips.insert(id, m);
            }
            None => {
                self.chips.remove(&id);
            }
        }
    }

    pub fn cycle_chip(&mut self, id: LabelId) {
        let next = match self.chip_mode(id) {
            None => Some(ChipMode::Require),
            Some(ChipMode::Require) => Some(ChipMode::Any),
            Some(ChipMode::Any) => Some(ChipMode::Exclude),
            Some(ChipMode::Exclude) => None,
        };
        self.set_chip(id, next);
    }

    pub fn clear_filters(&mut self) {
        self.search.clear();
        self.chips.clear();
    }

    pub fn has_active_filters(&self) -> bool {
        !self.search.trim().is_empty() || !self.chips.is_empty()
    }

    pub fn to_catalog_filter(&self) -> Filter {
        let mut filter = Filter::default();
        for (&id, &mode) in &self.chips {
            match mode {
                ChipMode::Require => filter.require.push(id),
                ChipMode::Any => filter.any.push(id),
                ChipMode::Exclude => filter.exclude.push(id),
            }
        }
        let trimmed = self.search.trim();
        if !trimmed.is_empty() {
            filter.path_contains = Some(trimmed.to_owned());
        }
        filter
    }
}

/// Human-readable byte size for the inspector (e.g. "512 B", "14.2 KB", "3.45 MB").
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let b = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else if b < GB {
        format!("{:.2} MB", b / MB)
    } else {
        format!("{:.2} GB", b / GB)
    }
}

/// Formats a Unix timestamp in nanoseconds as `YYYY-MM-DD HH:MM UTC` without extra date crates.
pub fn format_timestamp_utc(ns: i64) -> String {
    let secs = ns.div_euclid(1_000_000_000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;

    // Howard Hinnant's civil-from-days algorithm (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    format!("{year:04}-{m:02}-{d:02} {hour:02}:{minute:02} UTC")
}

/// Returns `singular` when `n == 1`, otherwise `plural` (Spanish number agreement).
pub fn plural_word<'a>(n: usize, singular: &'a str, plural: &'a str) -> &'a str {
    if n == 1 { singular } else { plural }
}

/// Formats a Spanish count phrase such as `"1 imagen"` or `"3 imágenes"`.
pub fn count_phrase(n: usize, singular: &str, plural: &str) -> String {
    format!("{n} {}", plural_word(n, singular, plural))
}

/// Formats the selected-image count in Spanish (`"1 seleccionada"`, `"0 seleccionadas"`, `"3 seleccionadas"`).
pub fn selected_phrase(n: usize) -> String {
    count_phrase(n, "seleccionada", "seleccionadas")
}
