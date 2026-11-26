// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::review_flow::{
    ReviewDecision, ReviewFlow, THRESHOLD_DEFAULT, THRESHOLD_MAX, THRESHOLD_MIN, UndoOutcome,
};
use catalog::{AssetId, Catalog, LabelKind, Source, Status};
use fsguard::{LibraryRoot, ensure_writable};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    cat: Catalog,
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, bytes).unwrap();
}

fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "a.jpg", b"img-a");
    write(&library, "b.jpg", b"img-b");
    write(&library, "c.jpg", b"img-c");
    write(&library, "d.jpg", b"img-d");
    let root = LibraryRoot::new(&library).unwrap();
    let canonical_library = root.as_path().to_path_buf();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    let db = ensure_writable(
        &tmp.path().join("data").join("catalog.sqlite"),
        std::slice::from_ref(&root),
    )
    .unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&root).unwrap();
    Setup {
        _tmp: tmp,
        library: canonical_library,
        cat,
    }
}

impl Setup {
    fn asset(&self, rel: &str) -> AssetId {
        let p = self.library.join(rel);
        self.cat.asset_of(&p).unwrap().unwrap()
    }
}

#[test]
fn default_threshold_and_clamping_work_and_groups_by_label_with_counts() {
    let s = setup();
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let c = s.asset("c.jpg");
    let d = s.asset("d.jpg");

    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let shot = s.cat.label(LabelKind::Category, "Capturas").unwrap();

    s.cat.suggest(a, meme, Source::Rules, 0.35).unwrap();
    s.cat.suggest(b, meme, Source::Ai, 0.55).unwrap();
    s.cat.suggest(c, meme, Source::Ocr, 0.80).unwrap(); // above default 0.60
    s.cat.suggest(d, shot, Source::Rules, 0.50).unwrap();

    let mut flow = ReviewFlow::new();
    assert!((flow.threshold() - THRESHOLD_DEFAULT).abs() < 1e-9);
    assert!((THRESHOLD_DEFAULT - 0.60).abs() < 1e-9);

    flow.reload(&s.cat).unwrap();
    assert_eq!(flow.total_items(), 3);
    assert_eq!(flow.groups().len(), 2);
    // Larger group first: meme (2) before Capturas (1).
    assert_eq!(flow.groups()[0].name, "meme");
    assert_eq!(flow.groups()[0].count(), 2);
    assert_eq!(flow.groups()[1].name, "Capturas");
    assert_eq!(flow.groups()[1].count(), 1);
    assert_eq!(flow.groups()[0].items[0].source, Source::Rules);
    assert_eq!(flow.groups()[0].items[1].source, Source::Ai);
    assert!(!flow.groups()[0].items[0].sha256.is_empty());

    // Threshold clamps to [0.30, 0.95].
    flow.set_threshold(0.10);
    assert!((flow.threshold() - THRESHOLD_MIN).abs() < 1e-9);
    flow.set_threshold(1.50);
    assert!((flow.threshold() - THRESHOLD_MAX).abs() < 1e-9);
    flow.reload(&s.cat).unwrap();
    assert_eq!(flow.total_items(), 4);
    assert_eq!(flow.groups()[0].count(), 3);
}

#[test]
fn cursor_navigation_and_accept_reject_advance_to_next_item_and_group() {
    let s = setup();
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let c = s.asset("c.jpg");

    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let shot = s.cat.label(LabelKind::Category, "Capturas").unwrap();
    s.cat.suggest(a, meme, Source::Rules, 0.40).unwrap();
    s.cat.suggest(b, meme, Source::Rules, 0.50).unwrap();
    s.cat.suggest(c, shot, Source::Rules, 0.45).unwrap();

    let mut flow = ReviewFlow::new();
    flow.reload(&s.cat).unwrap();

    // Active group is meme: [a (0.40), b (0.50)], cursor starts at 0.
    assert_eq!(flow.cursor(), 0);
    assert_eq!(flow.current_row().unwrap().asset, a);

    flow.move_next();
    assert_eq!(flow.cursor(), 1);
    assert_eq!(flow.current_row().unwrap().asset, b);
    flow.move_next(); // clamps at end
    assert_eq!(flow.cursor(), 1);

    flow.move_prev();
    assert_eq!(flow.cursor(), 0);
    assert_eq!(flow.current_row().unwrap().asset, a);

    // Accept `a`: cursor stays at 0, which is now `b` (the next item).
    assert!(flow.decide_current(&s.cat, ReviewDecision::Accept).unwrap());
    assert_eq!(flow.active_group().unwrap().label, meme);
    assert_eq!(flow.cursor(), 0);
    assert_eq!(flow.current_row().unwrap().asset, b);

    // Reject `b`: `meme` group is now empty, so flow automatically moves to `Capturas`.
    assert!(flow.decide_current(&s.cat, ReviewDecision::Reject).unwrap());
    assert_eq!(flow.active_group().unwrap().label, shot);
    assert_eq!(flow.cursor(), 0);
    assert_eq!(flow.current_row().unwrap().asset, c);

    // Verify DB statuses.
    assert_eq!(s.cat.labels_of(a).unwrap()[0].status, Status::Accepted);
    assert_eq!(s.cat.labels_of(b).unwrap()[0].status, Status::Rejected);
}

#[test]
fn group_actions_require_confirmation_and_undo_reverts_entire_group() {
    let s = setup();
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");
    let c = s.asset("c.jpg");

    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let shot = s.cat.label(LabelKind::Category, "Capturas").unwrap();
    s.cat.suggest(a, meme, Source::Rules, 0.40).unwrap();
    s.cat.suggest(b, meme, Source::Rules, 0.50).unwrap();
    s.cat.suggest(c, shot, Source::Rules, 0.45).unwrap();

    let mut flow = ReviewFlow::new();
    flow.reload(&s.cat).unwrap();

    // Requesting sets pending confirmation without modifying the catalog yet.
    assert!(flow.request_group_action(ReviewDecision::Accept));
    let pending = flow.pending_group_action().unwrap();
    assert_eq!(pending.label, meme);
    assert_eq!(pending.count, 2);
    assert_eq!(pending.decision, ReviewDecision::Accept);
    assert_eq!(s.cat.labels_of(a).unwrap()[0].status, Status::Suggested);

    // Cancelling clears confirmation without applying anything.
    flow.cancel_group_action();
    assert!(flow.pending_group_action().is_none());
    assert_eq!(flow.confirm_group_action(&s.cat).unwrap(), 0);

    // Request and confirm group rejection.
    assert!(flow.request_group_action(ReviewDecision::Reject));
    let applied = flow.confirm_group_action(&s.cat).unwrap();
    assert_eq!(applied, 2);
    assert!(flow.pending_group_action().is_none());
    assert_eq!(s.cat.labels_of(a).unwrap()[0].status, Status::Rejected);
    assert_eq!(s.cat.labels_of(b).unwrap()[0].status, Status::Rejected);
    assert_eq!(flow.active_group().unwrap().label, shot);

    // A single undo reverts both items in the group.
    let outcome = flow.undo(&s.cat).unwrap().unwrap();
    assert_eq!(
        outcome,
        UndoOutcome {
            restored: 2,
            skipped: 0
        }
    );
    assert_eq!(s.cat.labels_of(a).unwrap()[0].status, Status::Suggested);
    assert_eq!(s.cat.labels_of(b).unwrap()[0].status, Status::Suggested);
    assert_eq!(flow.active_group().unwrap().label, meme);
    assert_eq!(flow.active_group().unwrap().count(), 2);
}

#[test]
fn undo_stack_works_in_lifo_order_and_skips_items_replaced_by_later_changes() {
    let s = setup();
    let a = s.asset("a.jpg");
    let b = s.asset("b.jpg");

    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.suggest(a, meme, Source::Rules, 0.40).unwrap();
    s.cat.suggest(b, meme, Source::Rules, 0.50).unwrap();

    let mut flow = ReviewFlow::new();
    flow.reload(&s.cat).unwrap();

    // Accept `a`, then reject `b`.
    assert!(flow.decide_current(&s.cat, ReviewDecision::Accept).unwrap());
    assert!(flow.decide_current(&s.cat, ReviewDecision::Reject).unwrap());
    assert_eq!(flow.undo_len(), 2);
    assert_eq!(flow.total_items(), 0);

    // First undo restores `b` (LIFO).
    let u1 = flow.undo(&s.cat).unwrap().unwrap();
    assert_eq!(
        u1,
        UndoOutcome {
            restored: 1,
            skipped: 0
        }
    );
    assert_eq!(flow.current_row().unwrap().asset, b);

    // Meanwhile, `a` is reverted and replaced by a newer analysis suggestion with different confidence.
    s.cat
        .restore_status(
            a,
            meme,
            Source::Rules,
            0.40,
            Status::Accepted,
            Status::Suggested,
        )
        .unwrap();
    s.cat.suggest(a, meme, Source::Ai, 0.88).unwrap();

    // Second undo tries to undo the old accept of `a` (Rules, 0.40, Accepted -> Suggested),
    // which must be skipped without touching the newer (Ai, 0.88, Suggested) row.
    let u2 = flow.undo(&s.cat).unwrap().unwrap();
    assert_eq!(
        u2,
        UndoOutcome {
            restored: 0,
            skipped: 1
        }
    );
    let label_a = &s.cat.labels_of(a).unwrap()[0];
    assert_eq!(
        (label_a.status, label_a.source, label_a.confidence),
        (Status::Suggested, Source::Ai, 0.88)
    );

    // Stack is now empty.
    assert!(!flow.can_undo());
    assert_eq!(flow.undo(&s.cat).unwrap(), None);
}
