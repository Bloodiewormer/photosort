use app::model::{ChipMode, FilterState, GridLayout, MoveDir, Selection};
use catalog::{AssetId, Entry, LabelId, Sort};
use std::path::PathBuf;

fn sample_entries(n: usize) -> Vec<Entry> {
    (0..n)
        .map(|i| Entry {
            asset: AssetId(i as i64 + 1),
            path: PathBuf::from(format!("img_{i:02}.jpg")),
            sha256: format!("{i:064x}"),
            size: (i as u64 + 1) * 100,
            modified_ns: Some(1_700_000_000 + i as i64),
        })
        .collect()
}

#[test]
fn click_ctrl_click_and_shift_click_selection_behave_like_a_desktop_gallery() {
    let mut sel = Selection::default();
    assert!(sel.is_empty());
    assert_eq!(sel.primary(), None);

    // Click selects one item and sets anchor + cursor.
    sel.click(2, 10);
    assert_eq!(sel.selected_indices(), vec![2]);
    assert_eq!(sel.primary(), Some(2));

    // Shift-click extends from anchor (2) to 5.
    sel.shift_click(5, 10);
    assert_eq!(sel.selected_indices(), vec![2, 3, 4, 5]);
    assert_eq!(sel.primary(), Some(5));

    // Another shift-click adjusts range from the SAME anchor (2) backwards to 0.
    sel.shift_click(0, 10);
    assert_eq!(sel.selected_indices(), vec![0, 1, 2]);
    assert_eq!(sel.primary(), Some(0));

    // Ctrl-click toggles individual items.
    sel.ctrl_click(1, 10);
    assert_eq!(sel.selected_indices(), vec![0, 2]);
    sel.ctrl_click(7, 10);
    assert_eq!(sel.selected_indices(), vec![0, 2, 7]);
    assert_eq!(sel.primary(), Some(7));

    // Plain click replaces selection.
    sel.click(4, 10);
    assert_eq!(sel.selected_indices(), vec![4]);
    assert_eq!(sel.primary(), Some(4));
}

#[test]
fn select_all_invert_clear_and_shift_without_anchor() {
    let mut sel = Selection::default();
    // Shift-click with no anchor behaves like click.
    sel.shift_click(3, 5);
    assert_eq!(sel.selected_indices(), vec![3]);

    sel.invert(5);
    assert_eq!(sel.selected_indices(), vec![0, 1, 2, 4]);

    sel.select_all(5);
    assert_eq!(sel.selected_indices(), vec![0, 1, 2, 3, 4]);

    sel.clear();
    assert!(sel.is_empty());
    assert_eq!(sel.primary(), None);
}

#[test]
fn keyboard_navigation_moves_and_extends_across_grid_columns() {
    let mut sel = Selection::default();
    // Empty grid does nothing.
    sel.move_cursor(MoveDir::Right, 3, 0, false);
    assert!(sel.is_empty());

    // First move with no selection picks 0.
    sel.move_cursor(MoveDir::Right, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![0]);

    // Move right and down across a 3-column grid of 7 items.
    sel.move_cursor(MoveDir::Right, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![1]);
    sel.move_cursor(MoveDir::Down, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![4]);
    // Down from 4 would be 7 (past end), clamps to last item (6).
    sel.move_cursor(MoveDir::Down, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![6]);
    // Up from 6 goes to 3.
    sel.move_cursor(MoveDir::Up, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![3]);
    // Shift+Right extends selection from 3 to 4.
    sel.move_cursor(MoveDir::Right, 3, 7, true);
    assert_eq!(sel.selected_indices(), vec![3, 4]);
    // Left without shift moves to 3.
    sel.move_cursor(MoveDir::Left, 3, 7, false);
    assert_eq!(sel.selected_indices(), vec![3]);
}

#[test]
fn reconcile_preserves_selection_by_path_after_reordering_or_filtering() {
    let old = sample_entries(4);
    let mut sel = Selection::default();
    sel.click(1, old.len());
    sel.ctrl_click(3, old.len()); // img_01 and img_03, primary = 3 (img_03)

    // New list reverses and drops img_01.
    let new = vec![old[3].clone(), old[2].clone(), old[0].clone()];
    sel.reconcile(&old, &new);
    assert_eq!(sel.selected_indices(), vec![0]); // img_03 moved to index 0
    assert_eq!(sel.primary(), Some(0));
}

#[test]
fn grid_layout_computes_columns_rows_and_visible_ranges() {
    // Available 500px, cell 150x180, spacing 10:
    // 3 cells = 3*150 + 2*10 = 470 <= 500; 4 cells = 630 > 500 -> 3 columns.
    let grid = GridLayout::compute(500.0, 150.0, 180.0, 10.0, 10);
    assert_eq!(grid.columns, 3);
    assert_eq!(grid.row_count, 4); // rows: [0..3], [3..6], [6..9], [9..10]
    assert_eq!(grid.row_stride(), 190.0);
    assert!((grid.total_height() - (4.0 * 180.0 + 3.0 * 10.0)).abs() < 0.01);

    assert_eq!(grid.row_indices(0), 0..3);
    assert_eq!(grid.row_indices(3), 9..10);

    // At scroll 0 with viewport 200px: rows 0 and 1 are visible.
    assert_eq!(grid.visible_rows(0.0, 200.0, 0), 0..2);
    // With 1 row overscan: 0..3.
    assert_eq!(grid.visible_rows(0.0, 200.0, 1), 0..3);
    assert_eq!(grid.visible_indices(0.0, 200.0, 0), 0..6);

    // Scrolled to y = 380 (row 2 start), viewport 300 -> rows 2..4.
    assert_eq!(grid.visible_rows(380.0, 300.0, 0), 2..4);
    assert_eq!(grid.visible_indices(380.0, 300.0, 0), 6..10);

    // Narrow viewport still gives at least 1 column.
    let narrow = GridLayout::compute(50.0, 150.0, 180.0, 10.0, 5);
    assert_eq!(narrow.columns, 1);
    assert_eq!(narrow.row_count, 5);

    // Empty gallery has 0 rows and empty visible range.
    let empty = GridLayout::compute(500.0, 150.0, 180.0, 10.0, 0);
    assert_eq!(empty.row_count, 0);
    assert_eq!(empty.total_height(), 0.0);
    assert_eq!(empty.visible_rows(0.0, 400.0, 1), 0..0);
}

#[test]
fn filter_state_cycles_chips_and_builds_catalog_filter() {
    let mut f = FilterState::default();
    assert_eq!(f.sort, Sort::PathAsc);
    assert_eq!(f.to_catalog_filter().path_contains, None);

    let a = LabelId(1);
    let b = LabelId(2);
    let c = LabelId(3);

    // Cycle: None -> Require -> Any -> Exclude -> None.
    assert_eq!(f.chip_mode(a), None);
    f.cycle_chip(a);
    assert_eq!(f.chip_mode(a), Some(ChipMode::Require));
    f.cycle_chip(a);
    assert_eq!(f.chip_mode(a), Some(ChipMode::Any));
    f.cycle_chip(a);
    assert_eq!(f.chip_mode(a), Some(ChipMode::Exclude));
    f.cycle_chip(a);
    assert_eq!(f.chip_mode(a), None);

    f.set_chip(a, Some(ChipMode::Require));
    f.set_chip(b, Some(ChipMode::Any));
    f.set_chip(c, Some(ChipMode::Exclude));
    f.search = "  manga  ".into();
    f.sort = Sort::ModifiedDesc;

    let cf = f.to_catalog_filter();
    assert_eq!(cf.require, vec![a]);
    assert_eq!(cf.any, vec![b]);
    assert_eq!(cf.exclude, vec![c]);
    assert_eq!(cf.path_contains.as_deref(), Some("manga"));

    // Whitespace-only search produces None.
    f.search = "   ".into();
    assert_eq!(f.to_catalog_filter().path_contains, None);

    f.clear_filters();
    assert!(f.chips.is_empty());
    assert!(f.search.is_empty());
    // Clearing filters preserves sort choice.
    assert_eq!(f.sort, Sort::ModifiedDesc);
}

#[test]
fn formatting_helpers_render_sizes_and_utc_dates() {
    use app::model::{format_bytes, format_timestamp_utc};

    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert_eq!(format_bytes(2 * 1024 * 1024), "2.00 MB");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GB");

    assert_eq!(format_timestamp_utc(0), "1970-01-01 00:00 UTC");
    // 2023-11-14 22:13:20 UTC = 1_700_000_000s
    assert_eq!(
        format_timestamp_utc(1_700_000_000_000_000_000),
        "2023-11-14 22:13 UTC"
    );
}
