// Synthetic fixtures in temp dirs. The Recycle Bin is always a stand-in: the real one is never called.
#![allow(clippy::disallowed_methods)]

use catalog::{Catalog, Filter, LabelKind, Sort, Source};
use export::{
    Control, ExportItem, ExportOptions, ExportReport, MoveContext, Operation, Outcome,
    move_with_options, preview_with_options, undo_last_move,
};
use fsguard::{DestPath, LibraryRoot, MoveSource, ensure_move_dir, ensure_writable};
use std::cell::Cell;
use std::fs::{self, FileTimes};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

struct Setup {
    _tmp: TempDir,
    library: PathBuf,
    outside: PathBuf,
    bin: PathBuf,
    app_data: PathBuf,
    roots: Vec<LibraryRoot>,
    cat: Catalog,
    retired: Cell<usize>,
}

fn write(path: &Path, bytes: &[u8]) -> PathBuf {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    path.to_path_buf()
}

fn year_2001(offset: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(980_000_000 + offset)
}

fn set_dates(path: &Path) {
    let times = FileTimes::new().set_modified(year_2001(10));
    #[cfg(windows)]
    let times = std::os::windows::fs::FileTimesExt::set_created(times, year_2001(30));
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

/// A library with three photos (two share a name), indexed and tagged, and a stand-in Recycle Bin.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(tmp.path()).unwrap();
    let library = base.join("library");
    let outside = base.join("outside");
    let bin = base.join("papelera");
    let app_data = base.join("appdata");
    for dir in [&library, &outside, &bin, &app_data] {
        fs::create_dir_all(dir).unwrap();
    }
    write(&library.join("a.jpg"), b"photo a");
    write(&library.join("uno").join("igual.jpg"), b"photo one");
    write(&library.join("dos").join("igual.jpg"), b"photo number two");
    set_dates(&library.join("a.jpg"));
    let roots = vec![LibraryRoot::new(&library).unwrap()];
    let db = ensure_writable(&app_data.join("catalog.sqlite"), &roots).unwrap();
    let mut cat = Catalog::open(&db).unwrap();
    cat.index(&roots[0]).unwrap();
    let tag = cat.label(LabelKind::Tag, "viaje").unwrap();
    let asset = cat.asset_of(&library.join("a.jpg")).unwrap().unwrap();
    cat.suggest(asset, tag, Source::User, 1.0).unwrap();
    cat.accept(asset, tag).unwrap();
    Setup {
        _tmp: tmp,
        library,
        outside,
        bin,
        app_data,
        roots,
        cat,
        retired: Cell::new(0),
    }
}

impl Setup {
    /// The stand-in for the Recycle Bin: it moves the file into a temp folder.
    fn to_bin(&self) -> impl Fn(&MoveSource) -> io::Result<bool> + '_ {
        |file| {
            let n = self.retired.get();
            self.retired.set(n + 1);
            let name = file.as_path().file_name().unwrap().to_string_lossy();
            fs::rename(file.as_path(), self.bin.join(format!("{n}-{name}")))?;
            Ok(true)
        }
    }

    fn context<'a>(
        &'a self,
        retire: &'a dyn Fn(&MoveSource) -> io::Result<bool>,
        force_cross_volume: bool,
    ) -> MoveContext<'a> {
        MoveContext {
            catalog: &self.cat,
            roots: &self.roots,
            retire,
            force_cross_volume,
            before_retire: None,
        }
    }

    fn dest(&self, path: &Path) -> DestPath {
        ensure_move_dir(path, &self.app_data).unwrap()
    }

    fn items(&self, names: &[&str]) -> Vec<ExportItem> {
        names
            .iter()
            // Component by component: the catalog knows each path with the system's separator.
            .map(|name| {
                ExportItem::new(name.split('/').fold(self.library.clone(), |p, c| p.join(c)))
            })
            .collect()
    }

    fn gallery(&self) -> Vec<PathBuf> {
        self.cat
            .entries(&Filter::default(), Sort::PathAsc)
            .unwrap()
            .into_iter()
            .map(|e| e.path)
            .collect()
    }

    fn labels_of(&self, path: &Path) -> Vec<String> {
        let asset = self.cat.asset_of(path).unwrap().expect("in the catalog");
        let labels = self.cat.labels_of(asset).unwrap();
        labels.into_iter().map(|l| l.name).collect()
    }

    fn bin_hashes(&self) -> Vec<String> {
        fs::read_dir(&self.bin)
            .unwrap()
            .map(|e| sha(&e.unwrap().path()))
            .collect()
    }
}

fn sha(path: &Path) -> String {
    scanner::sha256_file(path).unwrap()
}

fn never(_: &MoveSource) -> io::Result<bool> {
    panic!("nothing may be sent to the Recycle Bin here")
}

fn options() -> ExportOptions {
    ExportOptions {
        operation: Operation::Move,
        ..Default::default()
    }
}

fn run(items: &[ExportItem], dest: &DestPath, ctx: &MoveContext) -> ExportReport {
    move_with_options(items, dest, options(), ctx, &mut |_| Control::Continue).unwrap()
}

fn dates(path: &Path) -> (SystemTime, Option<SystemTime>) {
    let md = fs::metadata(path).unwrap();
    (md.modified().unwrap(), md.created().ok())
}

#[test]
fn moving_on_the_same_disk_keeps_bytes_dates_and_labels() {
    let s = setup();
    let src = s.library.join("a.jpg");
    let (hash, before) = (sha(&src), dates(&src));
    // Inside the same library: sorting folders.
    let dest = s.dest(&s.library.join("ordenadas"));
    let ctx = s.context(&never, false);

    let report = run(&s.items(&["a.jpg"]), &dest, &ctx);

    let moved = s.library.join("ordenadas").join("a.jpg");
    assert_eq!(report.outcomes[0].1, Outcome::Moved(moved.clone()));
    assert!(!src.exists());
    assert_eq!((sha(&moved), dates(&moved)), (hash, before));
    assert_eq!(s.labels_of(&moved), ["viaje"]);
    assert!(s.gallery().contains(&moved));
    assert!(!s.gallery().contains(&src));
}

#[test]
fn moving_to_another_disk_retires_the_original_only_after_a_verified_copy() {
    let s = setup();
    let src = s.library.join("a.jpg");
    let (hash, before) = (sha(&src), dates(&src));
    let copy = s.outside.join("a.jpg");
    let to_bin = s.to_bin();
    let retire = |file: &MoveSource| {
        assert_eq!(sha(&copy), hash, "the verified copy comes first");
        to_bin(file)
    };
    let ctx = s.context(&retire, true);

    let report = run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);

    assert_eq!(report.outcomes[0].1, Outcome::Moved(copy.clone()));
    assert!(!src.exists());
    assert_eq!((sha(&copy), dates(&copy)), (hash.clone(), before));
    assert_eq!(s.bin_hashes(), [hash]);
    assert!(!s.outside.join("a.jpg.part").exists());
    assert!(!s.gallery().contains(&src));
}

#[test]
fn a_declined_recycle_keeps_the_original() {
    let s = setup();
    let src = s.library.join("a.jpg");
    let declined = |_: &MoveSource| Ok(false);
    let ctx = s.context(&declined, true);

    let report = run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);

    let copy = s.outside.join("a.jpg");
    assert!(
        matches!(&report.outcomes[0].1, Outcome::KeptOriginal { dest, .. } if *dest == copy),
        "{:?}",
        report.outcomes
    );
    assert_eq!(sha(&src), sha(&copy));
    assert!(s.gallery().contains(&src));
    assert!(s.cat.last_undoable_batch().unwrap().is_none());
}

#[test]
fn an_original_changed_during_the_copy_is_never_retired() {
    let s = setup();
    let src = s.library.join("a.jpg");
    // Same size: only the content tells.
    let edit = |original: &Path| fs::write(original, b"PHOTO A").unwrap();
    let mut ctx = s.context(&never, true);
    ctx.before_retire = Some(&edit);

    let report = run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);

    assert!(matches!(report.outcomes[0].1, Outcome::KeptOriginal { .. }));
    assert_eq!(fs::read(&src).unwrap(), b"PHOTO A");
    assert_eq!(fs::read(s.outside.join("a.jpg")).unwrap(), b"photo a");
}

#[test]
fn a_copy_that_is_no_longer_whole_never_retires_the_original() {
    let s = setup();
    let copy = s.outside.join("a.jpg");
    let shorten = |_: &Path| fs::write(&copy, b"pho").unwrap();
    let mut ctx = s.context(&never, true);
    ctx.before_retire = Some(&shorten);

    let report = run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);

    assert!(
        matches!(report.outcomes[0].1, Outcome::KeptOriginal { .. }),
        "{:?}",
        report.outcomes
    );
    assert_eq!(fs::read(s.library.join("a.jpg")).unwrap(), b"photo a");
}

/// A temporary file left by a power cut is neither reused nor deleted.
#[test]
fn a_leftover_temporary_file_does_not_block_the_move_and_is_left_alone() {
    let s = setup();
    let first = write(&s.outside.join("a.jpg.part"), b"half a copy");
    let second = write(&s.outside.join("a.jpg.1.part"), b"another half");
    let to_bin = s.to_bin();

    let report = run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.outside),
        &s.context(&to_bin, true),
    );

    assert_eq!(
        report.outcomes[0].1,
        Outcome::Moved(s.outside.join("a.jpg"))
    );
    assert_eq!(fs::read(first).unwrap(), b"half a copy");
    assert_eq!(fs::read(second).unwrap(), b"another half");
    assert!(!s.outside.join("a.jpg.2.part").exists());
}

#[test]
fn an_original_that_disappears_before_moving_is_never_retired() {
    let s = setup();
    let items = s.items(&["a.jpg"]);
    fs::remove_file(s.library.join("a.jpg")).unwrap();

    let report = run(&items, &s.dest(&s.outside), &s.context(&never, true));

    assert!(matches!(report.outcomes[0].1, Outcome::Failed(_)));
    assert!(!s.outside.join("a.jpg").exists());
}

#[test]
fn a_file_the_catalog_does_not_know_is_not_moved() {
    let s = setup();
    let stranger = write(&s.outside.join("ajena.jpg"), b"not indexed");

    let report = run(
        &[ExportItem::new(stranger.clone())],
        &s.dest(&s.library),
        &s.context(&never, false),
    );

    assert!(matches!(report.outcomes[0].1, Outcome::Failed(_)));
    assert!(stranger.exists() && !s.library.join("ajena.jpg").exists());
}

#[test]
fn moving_never_overwrites_and_an_identical_file_keeps_the_original() {
    for cross_volume in [false, true] {
        let s = setup();
        let taken = write(&s.outside.join("a.jpg"), b"another photo");
        let identical = write(&s.outside.join("igual.jpg"), b"photo one");
        let to_bin = s.to_bin();

        let report = run(
            &s.items(&["a.jpg", "uno/igual.jpg"]),
            &s.dest(&s.outside),
            &s.context(&to_bin, cross_volume),
        );

        let numbered = s.outside.join("a (1).jpg");
        assert_eq!(report.outcomes[0].1, Outcome::Moved(numbered.clone()));
        assert_eq!(fs::read(&taken).unwrap(), b"another photo");
        assert_eq!(fs::read(&numbered).unwrap(), b"photo a");
        assert_eq!(report.outcomes[1].1, Outcome::AlreadyPresent(identical));
        assert!(s.library.join("uno").join("igual.jpg").exists());
        assert_eq!(s.retired.get(), usize::from(cross_volume));
    }
}

/// Moving into the photo's own folder finds the photo itself: nothing may happen to it.
#[test]
fn moving_a_photo_onto_itself_does_nothing() {
    let s = setup();

    let report = run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.library),
        &s.context(&never, true),
    );

    assert!(matches!(report.outcomes[0].1, Outcome::AlreadyPresent(_)));
    assert_eq!(fs::read(s.library.join("a.jpg")).unwrap(), b"photo a");
}

#[test]
fn two_sources_with_the_same_name_get_numbered_and_none_is_lost() {
    for cross_volume in [false, true] {
        let s = setup();
        let to_bin = s.to_bin();
        // The first one is listed twice.
        let items = s.items(&["uno/igual.jpg", "dos/igual.jpg", "uno/igual.jpg"]);

        let report = run(
            &items,
            &s.dest(&s.outside),
            &s.context(&to_bin, cross_volume),
        );

        assert_eq!(
            report.outcomes[0].1,
            Outcome::Moved(s.outside.join("igual.jpg"))
        );
        assert_eq!(
            report.outcomes[1].1,
            Outcome::Moved(s.outside.join("igual (1).jpg"))
        );
        assert!(matches!(report.outcomes[2].1, Outcome::Failed(_)));
        assert_eq!(fs::read(s.outside.join("igual.jpg")).unwrap(), b"photo one");
        assert_eq!(
            fs::read(s.outside.join("igual (1).jpg")).unwrap(),
            b"photo number two"
        );
        assert_eq!(s.retired.get(), if cross_volume { 2 } else { 0 });
    }
}

#[test]
fn undo_restores_identical_files_and_sets_aside_what_it_cannot() {
    for cross_volume in [false, true] {
        let s = setup();
        let a = s.library.join("a.jpg");
        let one = s.library.join("uno").join("igual.jpg");
        let two = s.library.join("dos").join("igual.jpg");
        let (hash, before) = (sha(&a), dates(&a));
        let to_bin = s.to_bin();
        let ctx = s.context(&to_bin, cross_volume);
        run(
            &s.items(&["a.jpg", "uno/igual.jpg", "dos/igual.jpg"]),
            &s.dest(&s.outside),
            &ctx,
        );
        // The original folder is gone, one original path is taken and one moved copy was edited.
        fs::remove_dir(one.parent().unwrap()).unwrap();
        write(&two, b"a newer file");
        let edited = s.outside.join("igual.jpg");
        fs::write(&edited, b"edited after the move").unwrap();

        let report = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();

        assert_eq!(report.restored, std::slice::from_ref(&a));
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        let lost: Vec<&PathBuf> = report.lost.iter().map(|(p, _)| p).collect();
        assert_eq!(lost, [&two, &one]);
        // The report says where each one was left.
        let numbered = s.outside.join("igual (1).jpg");
        assert!(report.lost[0].1.contains(&*numbered.to_string_lossy()));
        assert!(report.lost[1].1.contains(&*edited.to_string_lossy()));
        assert_eq!((sha(&a), dates(&a)), (hash, before));
        assert!(!s.outside.join("a.jpg").exists());
        assert_eq!(s.labels_of(&a), ["viaje"]);
        assert_eq!(fs::read(&two).unwrap(), b"a newer file");
        assert_eq!(fs::read(&numbered).unwrap(), b"photo number two");
        assert_eq!(fs::read(&edited).unwrap(), b"edited after the move");
        assert!(!one.exists());
        assert!(undo_last_move(&ctx, &s.app_data).unwrap().is_none());
    }
}

#[test]
fn a_file_that_cannot_come_back_does_not_hold_back_the_batch_before_it() {
    let s = setup();
    let ctx = s.context(&never, false);
    let dest = s.dest(&s.outside);
    run(&s.items(&["a.jpg"]), &dest, &ctx);
    run(&s.items(&["uno/igual.jpg"]), &dest, &ctx);
    let gone = s.outside.join("igual.jpg");
    fs::remove_file(&gone).unwrap();

    let first = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();
    assert_eq!(first.lost.len(), 1, "{first:?}");
    let second = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();

    assert_eq!(second.restored, [s.library.join("a.jpg")]);
    assert!(undo_last_move(&ctx, &s.app_data).unwrap().is_none());
}

/// A file that cannot be read right now is not lost.
#[cfg(windows)]
#[test]
fn a_locked_file_can_be_undone_later() {
    use std::os::windows::fs::OpenOptionsExt;
    let s = setup();
    let ctx = s.context(&never, false);
    run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);
    {
        let _held = fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(s.outside.join("a.jpg"))
            .unwrap();
        let report = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();
        assert_eq!(
            (report.skipped.len(), report.lost.len()),
            (1, 0),
            "{report:?}"
        );
    }

    let report = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();
    assert_eq!(report.restored, [s.library.join("a.jpg")]);
}

/// An undo cut after the file was back, but before the log said so.
#[test]
fn undoing_a_file_that_is_already_back_only_settles_the_catalog() {
    for copy_left in [false, true] {
        let s = setup();
        let a = s.library.join("a.jpg");
        let original_gallery = s.gallery();
        let ctx = s.context(&never, false);
        run(&s.items(&["a.jpg"]), &s.dest(&s.outside), &ctx);
        let moved = s.outside.join("a.jpg");
        if copy_left {
            fs::copy(&moved, &a).unwrap();
        } else {
            fs::rename(&moved, &a).unwrap();
        }

        let report = undo_last_move(&ctx, &s.app_data).unwrap().unwrap();

        assert_eq!(report.restored, std::slice::from_ref(&a));
        assert_eq!(report.warnings.len(), usize::from(copy_left));
        assert_eq!(moved.exists(), copy_left);
        assert_eq!(s.gallery(), original_gallery);
        assert_eq!(s.labels_of(&a), ["viaje"]);
        assert!(undo_last_move(&ctx, &s.app_data).unwrap().is_none());
    }
}

/// What was moved by renaming comes back by copying when the two places are no longer one disk.
#[test]
fn undoing_a_rename_across_disks_copies_back_and_recycles_the_copy() {
    let s = setup();
    let a = s.library.join("a.jpg");
    let (hash, before) = (sha(&a), dates(&a));
    run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.outside),
        &s.context(&never, false),
    );
    let to_bin = s.to_bin();

    let report = undo_last_move(&s.context(&to_bin, true), &s.app_data)
        .unwrap()
        .unwrap();

    assert_eq!(report.restored, std::slice::from_ref(&a));
    assert_eq!((sha(&a), dates(&a)), (hash.clone(), before));
    assert_eq!(s.bin_hashes(), [hash]);
    assert!(!s.outside.join("a.jpg").exists());
}

#[test]
fn undo_with_a_declined_recycle_restores_and_says_the_copy_stayed() {
    let s = setup();
    let to_bin = s.to_bin();
    run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.outside),
        &s.context(&to_bin, true),
    );
    let declined = |_: &MoveSource| Ok(false);

    let report = undo_last_move(&s.context(&declined, true), &s.app_data)
        .unwrap()
        .unwrap();

    assert_eq!(report.restored, [s.library.join("a.jpg")]);
    assert_eq!(report.warnings.len(), 1, "{:?}", report.warnings);
    assert!(s.library.join("a.jpg").exists() && s.outside.join("a.jpg").exists());
    assert!(s.cat.last_undoable_batch().unwrap().is_none());
}

/// Nothing changed on disk, so nothing is left in the log to settle or undo.
#[cfg(windows)]
#[test]
fn a_move_windows_refuses_leaves_no_row_in_the_log() {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let s = setup();
    let src = s.library.join("a.jpg");
    // Another program has it open: it can be read, but not renamed.
    let _held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&src)
        .unwrap();

    let report = run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.outside),
        &s.context(&never, false),
    );

    assert!(
        matches!(report.outcomes[0].1, Outcome::Failed(_)),
        "{:?}",
        report.outcomes
    );
    assert!(src.exists() && !s.outside.join("a.jpg").exists());
    assert_eq!(s.cat.next_move_batch().unwrap(), 1);
}

#[test]
fn a_failed_copy_to_another_disk_leaves_no_row_in_the_log() {
    let mut s = setup();
    // A valid name that has no room left for the temporary copy's suffix.
    let name = format!("{}.jpg", "n".repeat(248));
    let src = write(&s.library.join(&name), b"long name");
    s.cat.index(&s.roots[0]).unwrap();

    let report = run(
        &[ExportItem::new(src.clone())],
        &s.dest(&s.outside),
        &s.context(&never, true),
    );

    assert!(
        matches!(report.outcomes[0].1, Outcome::Failed(_)),
        "{:?}",
        report.outcomes
    );
    assert!(src.exists() && !s.outside.join(&name).exists());
    assert_eq!(s.cat.next_move_batch().unwrap(), 1);
}

#[cfg(windows)]
#[test]
fn moving_to_another_disk_keeps_the_read_only_attribute() {
    let s = setup();
    let src = s.library.join("a.jpg");
    let mut permissions = fs::metadata(&src).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&src, permissions).unwrap();
    let to_bin = s.to_bin();

    run(
        &s.items(&["a.jpg"]),
        &s.dest(&s.outside),
        &s.context(&to_bin, true),
    );

    let moved = s.outside.join("a.jpg");
    assert!(fs::metadata(&moved).unwrap().permissions().readonly());
}

#[test]
fn the_manifest_says_whether_it_copied_or_moved() {
    let s = setup();
    let items = s.items(&["a.jpg"]);
    let copies = ensure_writable(&s.outside.join("copias"), &s.roots).unwrap();
    let copied = export::export(&items, &copies).unwrap();
    let moved = run(
        &items,
        &s.dest(&s.outside.join("movidas")),
        &s.context(&never, false),
    );

    for (report, operation, status) in [(copied, "copiar", "copied"), (moved, "mover", "moved")] {
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&report.manifest_json).unwrap()).unwrap();
        assert_eq!(json["files"][0]["operacion"], operation);
        assert_eq!(json["files"][0]["status"], status);
        let csv = fs::read_to_string(&report.manifest_csv).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].ends_with(",operacion"), "{}", lines[0]);
        assert!(lines[1].ends_with(&format!(",{operation}")), "{}", lines[1]);
    }
}

#[test]
fn a_move_never_embeds_tags_in_the_only_copy() {
    let s = setup();
    let jpeg = [0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x02, 0x11, 0x22, 0xFF, 0xD9];
    let src = write(&s.library.join("real.jpg"), &jpeg);
    let mut items = vec![ExportItem::new(src)];
    items[0].persons = vec!["Alguien".into()];
    let db = ensure_writable(&s.app_data.join("catalog.sqlite"), &s.roots).unwrap();
    Catalog::open(&db).unwrap().index(&s.roots[0]).unwrap();
    let to_bin = s.to_bin();

    let report = move_with_options(
        &items,
        &s.dest(&s.outside),
        ExportOptions {
            embed_jpeg_tags: true,
            ..options()
        },
        &s.context(&to_bin, true),
        &mut |_| Control::Continue,
    )
    .unwrap();

    assert!(matches!(report.outcomes[0].1, Outcome::Moved(_)));
    assert_eq!(fs::read(s.outside.join("real.jpg")).unwrap(), jpeg);
}

#[test]
fn the_preview_counts_what_stays_on_the_same_disk() {
    let s = setup();
    let preview = preview_with_options(
        &s.items(&["a.jpg", "uno/igual.jpg"]),
        &s.dest(&s.outside),
        options(),
    )
    .unwrap();
    assert_eq!((preview.same_volume, preview.other_volume), (2, 0));
    assert_eq!(preview.bytes_other_volume, 0);
    assert!(preview.fits());
    assert!(
        !s.outside.join("a.jpg").exists(),
        "a preview writes nothing"
    );
}
