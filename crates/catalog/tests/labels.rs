// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use catalog::{
    AssetId, Assignment, Catalog, Filter, LabelKind, RenameOutcome, RuleMode, Source, Status,
};
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

/// A library with three different images, already indexed.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    write(&library, "memes/funny.jpg", b"one");
    write(&library, "shots/chat.png", b"two");
    write(&library, "manga/page.webp", b"three");
    let root = LibraryRoot::new(&library).unwrap();
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
        library,
        cat,
    }
}

impl Setup {
    fn asset(&self, rel: &str) -> AssetId {
        let path = self.library.join(rel);
        let path = dunce::canonicalize(path).unwrap();
        self.cat.asset_of(&path).unwrap().expect("indexed")
    }

    fn rel(&self, paths: Vec<PathBuf>) -> Vec<String> {
        let base = dunce::canonicalize(&self.library).unwrap();
        let mut v: Vec<String> = paths
            .iter()
            .map(|p| {
                p.strip_prefix(&base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        v.sort();
        v
    }
}

fn only(list: Vec<Assignment>) -> Assignment {
    assert_eq!(list.len(), 1, "{list:?}");
    list.into_iter().next().unwrap()
}

#[test]
fn a_label_is_created_once_per_kind_and_name_ignoring_case() {
    let s = setup();
    let a = s.cat.label(LabelKind::Tag, "Meme").unwrap();
    assert_eq!(a, s.cat.label(LabelKind::Tag, "meme").unwrap());
    assert_ne!(a, s.cat.label(LabelKind::Category, "meme").unwrap());
}

#[test]
fn an_image_can_hold_many_labels_at_once() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    for (kind, name) in [
        (LabelKind::Tag, "screenshot"),
        (LabelKind::Tag, "meme"),
        (LabelKind::Category, "Anime"),
    ] {
        let l = s.cat.label(kind, name).unwrap();
        s.cat.suggest(img, l, Source::Ai, 0.8).unwrap();
    }
    assert_eq!(s.cat.labels_of(img).unwrap().len(), 3);
}

#[test]
fn an_assignment_records_where_it_came_from() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.suggest(img, l, Source::Ocr, 0.6).unwrap();
    let a = only(s.cat.labels_of(img).unwrap());
    assert_eq!(
        (a.kind, a.name.as_str(), a.source, a.confidence, a.status),
        (LabelKind::Tag, "meme", Source::Ocr, 0.6, Status::Suggested)
    );
}

#[test]
fn a_new_suggestion_replaces_the_previous_suggestion() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.4).unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.9).unwrap();
    assert_eq!(only(s.cat.labels_of(img).unwrap()).confidence, 0.9);
}

#[test]
fn an_accepted_label_is_not_changed_by_a_later_analysis() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.4).unwrap();
    s.cat.accept(img, l).unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.1).unwrap();
    let a = only(s.cat.labels_of(img).unwrap());
    assert_eq!(
        (a.status, a.confidence, a.source),
        (Status::Accepted, 0.4, Source::Ai)
    );
}

#[test]
fn accepting_a_label_nobody_suggested_adds_it_as_the_users() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.accept(img, l).unwrap();
    let a = only(s.cat.labels_of(img).unwrap());
    assert_eq!(
        (a.status, a.confidence, a.source),
        (Status::Accepted, 1.0, Source::User)
    );
}

#[test]
fn a_rejected_label_does_not_come_back_with_a_later_analysis() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.7).unwrap();
    s.cat.reject(img, l).unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.99).unwrap();
    assert_eq!(only(s.cat.labels_of(img).unwrap()).status, Status::Rejected);
}

#[test]
fn rejecting_a_label_that_was_never_suggested_also_sticks() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat.reject(img, l).unwrap();
    s.cat.suggest(img, l, Source::Ai, 0.99).unwrap();
    assert_eq!(only(s.cat.labels_of(img).unwrap()).status, Status::Rejected);
}

#[test]
fn a_confidence_outside_zero_to_one_is_refused() {
    let s = setup();
    let img = s.asset("memes/funny.jpg");
    let l = s.cat.label(LabelKind::Tag, "meme").unwrap();
    assert!(s.cat.suggest(img, l, Source::Ai, 1.5).is_err());
    assert!(s.cat.suggest(img, l, Source::Ai, -0.1).is_err());
}

#[test]
fn an_unknown_path_has_no_asset() {
    let s = setup();
    assert_eq!(s.cat.asset_of(Path::new("nowhere.jpg")).unwrap(), None);
}

#[test]
fn find_combines_all_any_none_and_a_name() {
    let s = setup();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let shot = s.cat.label(LabelKind::Tag, "screenshot").unwrap();
    let anime = s.cat.label(LabelKind::Category, "anime").unwrap();
    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    let page = s.asset("manga/page.webp");
    s.cat.suggest(funny, meme, Source::Ai, 0.9).unwrap();
    s.cat.suggest(funny, shot, Source::Ai, 0.9).unwrap();
    s.cat.suggest(funny, anime, Source::Ai, 0.9).unwrap();
    s.cat.suggest(chat, shot, Source::Ai, 0.9).unwrap();
    s.cat.suggest(page, anime, Source::Ai, 0.9).unwrap();

    let found = |f: Filter| s.rel(s.cat.find(&f).unwrap());
    assert_eq!(
        found(Filter {
            require: vec![meme, shot, anime],
            ..Default::default()
        }),
        ["memes/funny.jpg"]
    );
    assert_eq!(
        found(Filter {
            any: vec![shot, anime],
            ..Default::default()
        }),
        ["manga/page.webp", "memes/funny.jpg", "shots/chat.png"]
    );
    assert_eq!(
        found(Filter {
            require: vec![shot],
            exclude: vec![meme],
            ..Default::default()
        }),
        ["shots/chat.png"]
    );
    assert_eq!(
        found(Filter {
            any: vec![shot],
            path_contains: Some("CHAT".into()),
            ..Default::default()
        }),
        ["shots/chat.png"]
    );
    assert_eq!(found(Filter::default()).len(), 3);
}

#[test]
fn rejected_labels_never_match_and_missing_files_are_not_returned() {
    let mut s = setup();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    s.cat.suggest(funny, meme, Source::Ai, 0.9).unwrap();
    s.cat.suggest(chat, meme, Source::Ai, 0.9).unwrap();
    s.cat.reject(chat, meme).unwrap();
    fs::remove_file(s.library.join("memes/funny.jpg")).unwrap();
    let root = LibraryRoot::new(&s.library).unwrap();
    s.cat.index(&root).unwrap();
    let hits = s
        .cat
        .find(&Filter {
            require: vec![meme],
            ..Default::default()
        })
        .unwrap();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn labels_belong_to_the_content_so_duplicates_share_them() {
    let mut s = setup();
    write(&s.library, "memes/copy.jpg", b"one");
    let root = LibraryRoot::new(&s.library).unwrap();
    s.cat.index(&root).unwrap();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    s.cat
        .suggest(s.asset("memes/funny.jpg"), meme, Source::Ai, 0.9)
        .unwrap();
    let hits = s
        .cat
        .find(&Filter {
            require: vec![meme],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(s.rel(hits), ["memes/copy.jpg", "memes/funny.jpg"]);
}

#[test]
fn the_review_queue_lists_undecided_low_confidence_suggestions_lowest_first() {
    let s = setup();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    let page = s.asset("manga/page.webp");
    s.cat.suggest(funny, meme, Source::Ai, 0.5).unwrap();
    s.cat.suggest(chat, meme, Source::Ai, 0.2).unwrap();
    s.cat.suggest(page, meme, Source::Ai, 0.95).unwrap(); // confident enough
    let queue = s.cat.review_queue(0.6).unwrap();
    assert_eq!(
        queue.iter().map(|i| i.asset).collect::<Vec<_>>(),
        [chat, funny]
    );
    assert_eq!(queue[0].name, "meme");
    assert!(queue[0].path.ends_with("chat.png"));

    s.cat.accept(chat, meme).unwrap(); // decided: leaves the queue
    assert_eq!(s.cat.review_queue(0.6).unwrap().len(), 1);
}

#[test]
fn restore_status_undoes_accept_and_reject_back_to_suggested() {
    let s = setup();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let funny = s.asset("memes/funny.jpg");
    let chat = s.asset("shots/chat.png");
    s.cat.suggest(funny, meme, Source::Rules, 0.5).unwrap();
    s.cat.suggest(chat, meme, Source::Ai, 0.4).unwrap();

    s.cat.accept(funny, meme).unwrap();
    s.cat.reject(chat, meme).unwrap();
    assert!(s.cat.review_queue(0.6).unwrap().is_empty());

    assert!(
        s.cat
            .restore_status(
                funny,
                meme,
                Source::Rules,
                0.5,
                Status::Accepted,
                Status::Suggested
            )
            .unwrap()
    );
    assert!(
        s.cat
            .restore_status(
                chat,
                meme,
                Source::Ai,
                0.4,
                Status::Rejected,
                Status::Suggested
            )
            .unwrap()
    );

    let a_funny = only(s.cat.labels_of(funny).unwrap());
    assert_eq!(
        (a_funny.status, a_funny.source, a_funny.confidence),
        (Status::Suggested, Source::Rules, 0.5)
    );
    let a_chat = only(s.cat.labels_of(chat).unwrap());
    assert_eq!(
        (a_chat.status, a_chat.source, a_chat.confidence),
        (Status::Suggested, Source::Ai, 0.4)
    );
    assert_eq!(s.cat.review_queue(0.6).unwrap().len(), 2);
}

#[test]
fn restore_status_never_resurrects_a_suggestion_replaced_by_analysis_or_newer_decision() {
    let s = setup();
    let meme = s.cat.label(LabelKind::Tag, "meme").unwrap();
    let funny = s.asset("memes/funny.jpg");
    s.cat.suggest(funny, meme, Source::Rules, 0.5).unwrap();
    s.cat.accept(funny, meme).unwrap();

    // If the user changed their decision afterward (e.g. rejected in inspector), undoing the stale
    // accept must not overwrite the newer rejection.
    s.cat.reject(funny, meme).unwrap();
    assert!(
        !s.cat
            .restore_status(
                funny,
                meme,
                Source::Rules,
                0.5,
                Status::Accepted,
                Status::Suggested
            )
            .unwrap()
    );
    assert_eq!(
        only(s.cat.labels_of(funny).unwrap()).status,
        Status::Rejected
    );

    // Restore back to Suggested, then let a newer analysis replace the suggestion with higher
    // confidence/source. A stale undo targeting the old suggestion must do nothing.
    assert!(
        s.cat
            .restore_status(
                funny,
                meme,
                Source::Rules,
                0.5,
                Status::Rejected,
                Status::Suggested
            )
            .unwrap()
    );
    s.cat.suggest(funny, meme, Source::Ai, 0.92).unwrap();

    assert!(
        !s.cat
            .restore_status(
                funny,
                meme,
                Source::Rules,
                0.5,
                Status::Suggested,
                Status::Accepted
            )
            .unwrap()
    );
    let current = only(s.cat.labels_of(funny).unwrap());
    assert_eq!(
        (current.status, current.source, current.confidence),
        (Status::Suggested, Source::Ai, 0.92)
    );
}

#[test]
fn merge_labels_respects_user_decisions_reparents_subcategories_and_merges_vision_rules() {
    let mut s = setup();
    let img_a = s.asset("memes/funny.jpg");
    let img_b = s.asset("shots/chat.png");
    let img_c = s.asset("manga/page.webp");

    let cat_a = s.cat.create_category("Cómic", None).unwrap();
    let cat_b = s.cat.create_category("Historieta", None).unwrap();
    let child_a = s.cat.create_category("Tira corta", Some(cat_a)).unwrap();

    // img_a: A accepted (0.5), B rejected (0.9) -> accepted > rejected -> A wins
    s.cat.suggest(img_a, cat_a, Source::Ai, 0.5).unwrap();
    s.cat.accept(img_a, cat_a).unwrap();
    s.cat.suggest(img_a, cat_b, Source::Ai, 0.9).unwrap();
    s.cat.reject(img_a, cat_b).unwrap();

    // img_b: A rejected (0.4), B suggested (0.95) -> rejected > suggested -> A wins
    s.cat.suggest(img_b, cat_a, Source::Ai, 0.4).unwrap();
    s.cat.reject(img_b, cat_a).unwrap();
    s.cat.suggest(img_b, cat_b, Source::Ai, 0.95).unwrap();

    // img_c: both suggested, A has higher confidence (0.85 > 0.60) -> A wins
    s.cat.suggest(img_c, cat_a, Source::Ocr, 0.85).unwrap();
    s.cat.suggest(img_c, cat_b, Source::Ai, 0.60).unwrap();

    // Vision rule on A only: must transfer to B
    s.cat
        .create_vision_rule(
            "Cómic",
            LabelKind::Category,
            RuleMode::Independent,
            0.12,
            &["comic strip".into()],
            &[],
        )
        .unwrap();

    // Renaming A to B's name offers a merge conflict
    assert_eq!(
        s.cat.rename_label(cat_a, "historieta").unwrap(),
        RenameOutcome::Conflict {
            existing: cat_b,
            existing_name: "Historieta".into(),
        }
    );

    s.cat.merge_labels(cat_a, cat_b).unwrap();

    let la = only(s.cat.labels_of(img_a).unwrap());
    assert_eq!((la.label, la.status), (cat_b, Status::Accepted));
    let lb = only(s.cat.labels_of(img_b).unwrap());
    assert_eq!((lb.label, lb.status), (cat_b, Status::Rejected));
    let lc = only(s.cat.labels_of(img_c).unwrap());
    assert_eq!(
        (lc.label, lc.status, lc.source, lc.confidence),
        (cat_b, Status::Suggested, Source::Ocr, 0.85)
    );

    // Subcategory moved under B
    let nodes = s.cat.list_labels().unwrap();
    assert!(!nodes.iter().any(|n| n.label == cat_a));
    let child_node = nodes.iter().find(|n| n.label == child_a).unwrap();
    assert_eq!(child_node.parent, Some(cat_b));

    // Vision rule moved to B
    let rules = s.cat.list_vision_rules().unwrap();
    assert!(
        rules
            .iter()
            .any(|r| r.label == cat_b && r.positives == vec!["comic strip"])
    );

    // Merging when BOTH have vision rules keeps target's rule and drops source's
    let cat_c = s.cat.create_category("Viñetas", None).unwrap();
    s.cat
        .create_vision_rule(
            "Viñetas",
            LabelKind::Category,
            RuleMode::Independent,
            0.20,
            &["panels".into()],
            &[],
        )
        .unwrap();
    s.cat.merge_labels(cat_c, cat_b).unwrap();
    let rules_after = s.cat.list_vision_rules().unwrap();
    let kept = rules_after.iter().find(|r| r.label == cat_b).unwrap();
    assert_eq!(kept.positives, vec!["comic strip"]);
    assert!(!rules_after.iter().any(|r| r.label == cat_c));
}

#[test]
fn move_category_rejects_self_and_descendant_cycles() {
    let s = setup();
    let root = s.cat.create_category("Raíz", None).unwrap();
    let mid = s.cat.create_category("Nivel 2", Some(root)).unwrap();
    let leaf = s.cat.create_category("Nivel 3", Some(mid)).unwrap();

    assert!(s.cat.move_category(root, Some(root)).is_err());
    assert!(s.cat.move_category(root, Some(mid)).is_err());
    assert!(s.cat.move_category(root, Some(leaf)).is_err());
    assert!(s.cat.move_category(mid, Some(leaf)).is_err());

    // Valid moves: leaf to root level, and back under root
    s.cat.move_category(leaf, None).unwrap();
    s.cat.move_category(leaf, Some(root)).unwrap();
    let leaf_node = s
        .cat
        .list_labels()
        .unwrap()
        .into_iter()
        .find(|n| n.label == leaf)
        .unwrap();
    assert_eq!(leaf_node.parent, Some(root));
}

#[test]
fn filter_by_category_includes_subcategories_recursively() {
    let s = setup();
    let manga = s.cat.label(LabelKind::Category, "Manga").unwrap();
    let scan = s.cat.create_category("Scanlation", Some(manga)).unwrap();
    let promo = s
        .cat
        .create_category("Página promocional", Some(scan))
        .unwrap();

    let img_root = s.asset("memes/funny.jpg");
    let img_leaf = s.asset("manga/page.webp");
    s.cat.accept(img_root, manga).unwrap();
    s.cat.accept(img_leaf, promo).unwrap();

    // Require parent matches both parent and grandchild
    let hits = s.rel(
        s.cat
            .find(&Filter {
                require: vec![manga],
                ..Default::default()
            })
            .unwrap(),
    );
    assert_eq!(hits, ["manga/page.webp", "memes/funny.jpg"]);

    // Require intermediate matches grandchild only
    let mid_hits = s.rel(
        s.cat
            .find(&Filter {
                require: vec![scan],
                ..Default::default()
            })
            .unwrap(),
    );
    assert_eq!(mid_hits, ["manga/page.webp"]);

    // Exclude parent excludes grandchild too
    let excl_hits = s.rel(
        s.cat
            .find(&Filter {
                exclude: vec![manga],
                ..Default::default()
            })
            .unwrap(),
    );
    assert_eq!(excl_hits, ["shots/chat.png"]);

    // Subtree image count on parent includes grandchild without double-counting
    let nodes = s.cat.list_labels().unwrap();
    let manga_node = nodes.iter().find(|n| n.label == manga).unwrap();
    assert_eq!(manga_node.images, 2);
}
