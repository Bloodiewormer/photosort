//! Moving originals and bringing them back. Every step that takes a file from its place goes through
//! `fsguard`; between disks, only after a verified copy.

use crate::{
    Control, ExportItem, ExportOptions, ExportProgress, ExportReport, Operation, Outcome, part_of,
    read_source, run, write_verified,
};
use catalog::{Catalog, CatalogError, MoveMethod, MoveRow, MoveState, NewMove};
use fsguard::{
    DestPath, LibraryRoot, MoveSource, ensure_move_dir, ensure_move_source, io_reason,
    move_no_replace,
};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub struct MoveContext<'a> {
    pub catalog: &'a Catalog,
    pub roots: &'a [LibraryRoot],
    /// `fsguard::recycle` in the app; a stand-in that moves to a temp folder in tests.
    pub retire: &'a dyn Fn(&MoveSource) -> io::Result<bool>,
    /// Tests only: take the other-disk path on the same disk.
    pub force_cross_volume: bool,
    /// Tests only: runs after the verified copy and before the original is checked again.
    pub before_retire: Option<&'a dyn Fn(&Path)>,
}

#[derive(Debug, Default)]
pub struct UndoReport {
    /// Original paths that hold their file again.
    pub restored: Vec<PathBuf>,
    /// Files left as they are, by original path, with the reason. They can be tried again.
    pub skipped: Vec<(PathBuf, String)>,
    /// Files that can no longer be undone, by original path, with the reason and where they were
    /// left. Nothing on disk was touched.
    pub lost: Vec<(PathBuf, String)>,
    /// Restored files with something else to know, such as a copy that could not be recycled.
    pub warnings: Vec<(PathBuf, String)>,
}

/// Where a move is going: the folder and the free name in it.
pub(crate) struct Place<'a> {
    pub dir: &'a DestPath,
    pub target: &'a DestPath,
}

/// Moves the originals into `dest`, laid out and listed in a manifest like a copy. Nothing is ever
/// overwritten or deleted, every move is logged before the file is touched, and the whole call is
/// one batch to undo. Tags are never embedded: that would change the only copy.
pub fn move_with_options(
    items: &[ExportItem],
    dest: &DestPath,
    options: ExportOptions,
    ctx: &MoveContext,
    progress: &mut dyn FnMut(&ExportProgress) -> Control,
) -> io::Result<ExportReport> {
    let options = ExportOptions {
        embed_jpeg_tags: false,
        operation: Operation::Move,
        ..options
    };
    let _lock = ctx.catalog.lock_moves().map_err(db_error)?;
    let batch = ctx.catalog.next_move_batch().map_err(db_error)?;
    run(items, dest, options, Some((ctx, batch)), progress)
}

fn db_error(e: CatalogError) -> io::Error {
    io::Error::other(e.to_string())
}

/// Moves one original to a free name. Besides the outcome, returns a warning when the file moved but
/// the log could not be closed. An error means the original is where it was and there is no copy.
pub(crate) fn move_one(
    source: &Path,
    place: Place,
    sha: &str,
    md: &fs::Metadata,
    ctx: &MoveContext,
    batch: i64,
) -> io::Result<(Outcome, Option<String>)> {
    let original = ensure_move_source(source).map_err(io::Error::other)?;
    let src_root = ctx
        .catalog
        .root_of_path(source)
        .map_err(db_error)?
        .ok_or_else(|| io::Error::other("el catálogo no conoce este archivo; no se mueve"))?;
    // The folder with the capitals it has on disk, which is how the folder review will list it.
    let dir = dunce::canonicalize(place.dir.as_path())?;
    let dst = dir.join(place.target.as_path().file_name().unwrap());
    let dst_root = ctx
        .roots
        .iter()
        .filter(|root| root.contains(&dst))
        .max_by_key(|root| root.as_path().components().count())
        .map(LibraryRoot::as_path);
    let id = ctx
        .catalog
        .begin_move(&NewMove {
            batch,
            src_path: source,
            src_root: &src_root,
            dst_path: &dst,
            dst_root,
            sha256: sha,
            size: md.len(),
            mtime_ns: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64),
            method: MoveMethod::Rename,
        })
        .map_err(db_error)?;
    // If this fails the row stays pending, and the next time the catalog opens it settles it.
    let close = |state: MoveState| {
        ctx.catalog
            .finish_move(id, state, dst_root)
            .err()
            .map(|e| format!("el registro del movimiento se completará al volver a abrir: {e}"))
    };

    // A failure that leaves the disk as it was leaves nothing in the log either. If the original is
    // not in sight, or this fails, the row stays pending for the next time the catalog opens.
    let discard = |e: io::Error| {
        if fs::symlink_metadata(source).is_ok() {
            let _ = ctx.catalog.discard_move(id);
        }
        e
    };
    if !ctx.force_cross_volume {
        match move_no_replace(&original, place.target) {
            Ok(()) => return Ok((Outcome::Moved(dst), close(MoveState::Done))),
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
            Err(e) => return Err(discard(e)),
        }
    }
    // The method is written first, so that an undo after a cut knows the way back.
    ctx.catalog
        .set_move_method(id, MoveMethod::CopyRecycle)
        .map_err(db_error)
        .and_then(|()| part_of(place.dir, place.target))
        .and_then(|part| write_verified(source, &part, place.target, sha, md))
        .map_err(discard)?;

    // From here on the copy exists: anything that goes wrong keeps the original.
    let retired = keep_attributes(md, place.target).and_then(|()| {
        if let Some(hook) = ctx.before_retire {
            hook(source);
        }
        check_unchanged(source, sha, md)?;
        check_copy(place.target.as_path(), md.len())?;
        retire(ctx, &original)
    });
    Ok(match retired {
        Ok(()) => (Outcome::Moved(dst), close(MoveState::Done)),
        Err(e) => {
            let reason = io_reason(&e);
            let warning = close(MoveState::KeptOriginal);
            (Outcome::KeptOriginal { dest: dst, reason }, warning)
        }
    })
}

/// The file at `path` still has the size, modified time and bytes it had when it was copied.
fn check_unchanged(path: &Path, sha: &str, md: &fs::Metadata) -> io::Result<()> {
    let (now, now_sha) = read_source(path).map_err(io::Error::other)?;
    if now.len() != md.len() || now.modified().ok() != md.modified().ok() || now_sha != sha {
        return Err(io::Error::other("el archivo cambió mientras se copiaba"));
    }
    Ok(())
}

/// The last look before a file is retired: the copy that replaces it is at its final name, whole.
fn check_copy(copy: &Path, size: u64) -> io::Result<()> {
    if fs::symlink_metadata(copy).is_ok_and(|md| md.is_file() && md.len() == size) {
        Ok(())
    } else {
        Err(io::Error::other(
            "la copia ya no está completa en el destino",
        ))
    }
}

fn retire(ctx: &MoveContext, file: &MoveSource) -> io::Result<()> {
    if (ctx.retire)(file)? {
        Ok(())
    } else {
        Err(io::Error::other("no se envió a la Papelera"))
    }
}

/// Gives the copy the read-only, hidden and system attributes of the file it replaces: a move must
/// not lose them.
#[cfg(windows)]
fn keep_attributes(md: &fs::Metadata, copy: &DestPath) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
    };
    let wanted = md.file_attributes()
        & (FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM);
    if wanted == 0 {
        return Ok(());
    }
    copy.verify().map_err(io::Error::other)?;
    let current = fs::symlink_metadata(copy.as_path())?.file_attributes();
    // The verbatim form has no length limit.
    let wide: Vec<u16> = fs::canonicalize(copy.as_path())?
        .as_os_str()
        .encode_wide()
        .chain([0])
        .collect();
    // SAFETY: `wide` is null-terminated UTF-16. It is our own new copy, never an original.
    if unsafe { SetFileAttributesW(wide.as_ptr(), current | wanted) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn keep_attributes(_md: &fs::Metadata, _copy: &DestPath) -> io::Result<()> {
    Ok(())
}

/// Brings back the most recent batch of moves that is still done, newest first. A file is only brought
/// back when it is still the one that was moved and its original path is free; one that no longer
/// can be is set aside, so it does not hold back the batches before it. `None` when there is nothing
/// to undo.
pub fn undo_last_move(ctx: &MoveContext, app_data: &Path) -> io::Result<Option<UndoReport>> {
    let _lock = ctx.catalog.lock_moves().map_err(db_error)?;
    let Some(rows) = ctx.catalog.last_undoable_batch().map_err(db_error)? else {
        return Ok(None);
    };
    let mut report = UndoReport::default();
    for row in rows {
        match restore(&row, ctx, app_data) {
            Ok(Restored::Back(mut warnings)) => {
                if let Err(e) = ctx.catalog.undo_done(row.id) {
                    warnings.push(format!("no se actualizó el catálogo: {e}"));
                }
                if !warnings.is_empty() {
                    let warning = warnings.join("; ");
                    report.warnings.push((row.src_path.clone(), warning));
                }
                report.restored.push(row.src_path);
            }
            Ok(Restored::Lost(why)) => {
                match ctx.catalog.finish_move(row.id, MoveState::Lost, None) {
                    Ok(()) => report.lost.push((row.src_path, why)),
                    Err(e) => report
                        .skipped
                        .push((row.src_path, format!("{why}; no se pudo anotar: {e}"))),
                }
            }
            Err(e) => report.skipped.push((row.src_path, io_reason(&e))),
        }
    }
    Ok(Some(report))
}

/// How far an undo of one file got.
enum Restored {
    /// The file is at its original path, with anything else there is to know.
    Back(Vec<String>),
    /// It can never be brought back: why, and where the moved file was left.
    Lost(String),
}

/// True only when the system says there is nothing at `path`, not even a broken link.
fn is_absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(e) if e.kind() == io::ErrorKind::NotFound)
}

/// Puts one file back. An error means nothing at the original path or the destination changed, and
/// that it can be tried again.
#[allow(clippy::disallowed_methods)] // recreates the folder an original was in, checked as a DestPath
fn restore(row: &MoveRow, ctx: &MoveContext, app_data: &Path) -> io::Result<Restored> {
    let at = row.dst_path.display();
    match fs::symlink_metadata(&row.src_path) {
        Ok(md) => {
            // Already back: an undo that was cut short, or the user put it there.
            return Ok(
                if md.is_file() && scanner::sha256_file(&row.src_path)? == row.sha256 {
                    let left = !is_absent(&row.dst_path);
                    Restored::Back(
                        left.then(|| format!("en «{at}» sigue otra copia"))
                            .into_iter()
                            .collect(),
                    )
                } else {
                    Restored::Lost(format!(
                        "otro contenido ocupa la ruta original; el archivo se había movido a «{at}»"
                    ))
                },
            );
        }
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        Err(_) => {}
    }
    match fs::symlink_metadata(&row.dst_path) {
        Ok(md) if md.is_file() => {}
        Ok(_) => {
            return Ok(Restored::Lost(format!(
                "en «{at}» ya no hay un archivo normal; no se toca"
            )));
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            // On a drive that is not there everything looks deleted: that can be tried again.
            if !row.dst_path.ancestors().last().is_some_and(Path::is_dir) {
                return Err(io::Error::other(format!("no se ve la unidad de «{at}»")));
            }
            return Ok(Restored::Lost(format!("ya no está en «{at}»")));
        }
        Err(e) => return Err(e),
    }
    let moved = ensure_move_source(&row.dst_path)
        .map_err(|e| io::Error::other(format!("ya no se puede leer en el destino: {e}")))?;
    let (md, sha) = read_source(moved.as_path()).map_err(io::Error::other)?;
    if sha != row.sha256 {
        return Ok(Restored::Lost(format!(
            "el archivo cambió después de moverlo; se queda en «{at}»"
        )));
    }
    let (Some(parent), Some(name)) = (row.src_path.parent(), row.src_path.file_name()) else {
        return Err(io::Error::other("la ruta original no es válida"));
    };
    let dir = ensure_move_dir(parent, app_data).map_err(io::Error::other)?;
    fs::create_dir_all(dir.as_path())?;
    dir.verify().map_err(io::Error::other)?;
    let target = dir.join_name(name).map_err(io::Error::other)?;

    if row.method == MoveMethod::Rename && !ctx.force_cross_volume {
        match move_no_replace(&moved, &target) {
            Ok(()) => return Ok(Restored::Back(Vec::new())),
            // What was one disk when it moved no longer is.
            Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {}
            Err(e) => return Err(e),
        }
    }
    let part = part_of(&dir, &target)?;
    write_verified(moved.as_path(), &part, &target, &sha, &md)?;
    // The original is back from here on: what is left is tidying up the destination.
    let mut warnings = Vec::new();
    if let Err(e) = keep_attributes(&md, &target) {
        warnings.push(format!(
            "no se restauraron los atributos: {}",
            io_reason(&e)
        ));
    }
    let retired = check_unchanged(moved.as_path(), &sha, &md)
        .and_then(|()| check_copy(target.as_path(), md.len()))
        .and_then(|()| retire(ctx, &moved));
    if let Err(e) = retired {
        warnings.push(format!("la copia sigue en «{at}»: {}", io_reason(&e)));
    }
    Ok(Restored::Back(warnings))
}
