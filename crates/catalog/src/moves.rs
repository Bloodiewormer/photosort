//! The log of moved originals: written before a file is touched, so that every move can be finished
//! after a crash and undone.

use crate::{Catalog, CatalogError, file_is_gone};
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveMethod {
    Rename,
    CopyRecycle,
}

impl MoveMethod {
    fn as_str(self) -> &'static str {
        match self {
            Self::Rename => "rename",
            Self::CopyRecycle => "copy_recycle",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveState {
    Pending,
    Done,
    KeptOriginal,
    Undone,
}

impl MoveState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::KeptOriginal => "kept_original",
            Self::Undone => "undone",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NewMove<'a> {
    pub batch: i64,
    pub src_path: &'a Path,
    pub src_root: &'a Path,
    pub dst_path: &'a Path,
    /// The library that holds the destination, if any.
    pub dst_root: Option<&'a Path>,
    pub sha256: &'a str,
    pub size: u64,
    pub mtime_ns: Option<i64>,
    pub method: MoveMethod,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveRow {
    pub id: i64,
    pub batch: i64,
    pub src_path: PathBuf,
    pub src_root: PathBuf,
    pub dst_path: PathBuf,
    pub dst_root: Option<PathBuf>,
    pub sha256: String,
    pub size: u64,
    pub mtime_ns: Option<i64>,
    pub method: MoveMethod,
}

/// Held while originals are being moved or brought back. The system releases it if the app dies.
pub struct MoveLock(#[allow(dead_code)] Connection);

const ROW: &str = "SELECT id, batch, src_path, src_root, dst_path, sha256, size, mtime_ns, method,
            dst_root
     FROM move_log";

fn row(r: &rusqlite::Row) -> rusqlite::Result<MoveRow> {
    let path = |i: usize| r.get::<_, String>(i).map(PathBuf::from);
    Ok(MoveRow {
        id: r.get(0)?,
        batch: r.get(1)?,
        src_path: path(2)?,
        src_root: path(3)?,
        dst_path: path(4)?,
        dst_root: r.get::<_, Option<String>>(9)?.map(PathBuf::from),
        sha256: r.get(5)?,
        size: r.get::<_, i64>(6)?.max(0) as u64,
        mtime_ns: r.get(7)?,
        method: if r.get::<_, String>(8)? == "rename" {
            MoveMethod::Rename
        } else {
            MoveMethod::CopyRecycle
        },
    })
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Points the gallery at where a moved photo is now: inside the library `dst_root`, or nowhere. The
/// asset and its labels stay either way.
fn relocate(
    tx: &Transaction,
    src: &str,
    dst: &str,
    dst_root: Option<&Path>,
) -> rusqlite::Result<()> {
    let location = |path: &str| -> rusqlite::Result<Option<i64>> {
        tx.query_row(
            "SELECT asset_id FROM location WHERE path = ?1",
            [path],
            |r| r.get(0),
        )
        .optional()
    };
    let Some(asset) = location(src)? else {
        return Ok(());
    };
    let delete = "DELETE FROM location WHERE path = ?1";
    let Some(root) = dst_root else {
        tx.execute(delete, [src])?;
        return Ok(());
    };
    if location(dst)? == Some(asset) {
        // The folder review already saw the file in its new place.
        tx.execute(delete, [src])?;
        tx.execute(
            "UPDATE location SET root = ?2, missing = 0 WHERE path = ?1",
            params![dst, text(root)],
        )?;
    } else {
        // A row of other bytes at that path is left over from a file that is no longer there.
        tx.execute(delete, [dst])?;
        tx.execute(
            "UPDATE location SET path = ?2, root = ?3, missing = 0,
                    seen_run = (SELECT MAX(seen_run) FROM location)
             WHERE path = ?1",
            params![src, dst, text(root)],
        )?;
    }
    Ok(())
}

fn finish(
    tx: &Transaction,
    id: i64,
    state: MoveState,
    dst_root: Option<&Path>,
) -> rusqlite::Result<()> {
    let (src, dst): (String, String) = tx.query_row(
        "SELECT src_path, dst_path FROM move_log WHERE id = ?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    tx.execute(
        "UPDATE move_log SET state = ?2 WHERE id = ?1",
        params![id, state.as_str()],
    )?;
    if state == MoveState::Done {
        relocate(tx, &src, &dst, dst_root)?;
    }
    Ok(())
}

impl Catalog {
    /// Fails when another move or undo is running, in this process or any other.
    pub fn lock_moves(&self) -> Result<MoveLock, CatalogError> {
        // The folder review may hold the catalog while it hashes a batch of new files: waiting
        // for it beats failing photo after photo.
        self.conn.busy_timeout(Duration::from_secs(60))?;
        self.try_lock_moves()?.ok_or_else(|| {
            CatalogError::InvalidOperation("ya hay un movimiento de fotos en curso".into())
        })
    }

    /// The lock is an exclusive transaction on a small database of its own beside the catalog, so it
    /// never holds up the catalog itself.
    fn try_lock_moves(&self) -> Result<Option<MoveLock>, CatalogError> {
        let path = self.conn.path().ok_or_else(|| {
            CatalogError::InvalidOperation("el catálogo no está en un archivo".into())
        })?;
        let lock = Connection::open(format!("{path}.movelock"))?;
        lock.busy_timeout(Duration::ZERO)?;
        match lock.execute_batch("BEGIN EXCLUSIVE") {
            Ok(()) => Ok(Some(MoveLock(lock))),
            Err(e) if e.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn next_move_batch(&self) -> Result<i64, CatalogError> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(batch), 0) + 1 FROM move_log",
            [],
            |r| r.get(0),
        )?)
    }

    /// Logs a move as `pending`, before the file is touched.
    pub fn begin_move(&self, m: &NewMove) -> Result<i64, CatalogError> {
        let at_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as i64);
        self.conn.execute(
            "INSERT INTO move_log
                 (batch, src_path, src_root, dst_path, sha256, size, mtime_ns, method, state, at_ns,
                  dst_root)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'pending', ?9, ?10)",
            params![
                m.batch,
                text(m.src_path),
                text(m.src_root),
                text(m.dst_path),
                m.sha256,
                m.size as i64,
                m.mtime_ns,
                m.method.as_str(),
                at_ns,
                m.dst_root.map(text)
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Closes a logged move and, when it is [`MoveState::Done`], moves the photo in the gallery too:
    /// to `dst_root`, the library that holds the destination, or out of sight when it is in none.
    pub fn finish_move(
        &self,
        id: i64,
        state: MoveState,
        dst_root: Option<&Path>,
    ) -> Result<(), CatalogError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        finish(&tx, id, state, dst_root)?;
        Ok(tx.commit()?)
    }

    /// The newest batch with moves still done, newest move first: the order to undo them in.
    pub fn last_undoable_batch(&self) -> Result<Option<Vec<MoveRow>>, CatalogError> {
        let mut stmt = self.conn.prepare(&format!(
            "{ROW} WHERE state = 'done'
                 AND batch = (SELECT MAX(batch) FROM move_log WHERE state = 'done')
             ORDER BY id DESC"
        ))?;
        let rows: Vec<MoveRow> = stmt.query_map([], row)?.collect::<Result<_, _>>()?;
        Ok((!rows.is_empty()).then_some(rows))
    }

    /// Notes how a pending move is going to happen, once renaming turns out not to be enough.
    pub fn set_move_method(&self, id: i64, method: MoveMethod) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE move_log SET method = ?2 WHERE id = ?1",
            params![id, method.as_str()],
        )?;
        Ok(())
    }

    /// Records that the file of a done move is back, and puts the photo back at its original path.
    pub fn undo_done(&self, id: i64) -> Result<(), CatalogError> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let m = tx.query_row(&format!("{ROW} WHERE id = ?1"), [id], row)?;
        tx.execute(
            "UPDATE move_log SET state = ?2 WHERE id = ?1",
            params![id, MoveState::Undone.as_str()],
        )?;
        let asset: Option<i64> = tx
            .query_row("SELECT id FROM asset WHERE sha256 = ?1", [&m.sha256], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(asset) = asset {
            let (src, dst, root) = (text(&m.src_path), text(&m.dst_path), text(&m.src_root));
            tx.execute("DELETE FROM location WHERE path = ?1", [&src])?;
            let updated = tx.execute(
                "UPDATE location SET path = ?2, root = ?3, asset_id = ?4, missing = 0
                 WHERE path = ?1",
                params![dst, src, root, asset],
            )?;
            if updated == 0 {
                tx.execute(
                    "INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                     VALUES (?1, ?2, ?3, ?4, ?5,
                             (SELECT COALESCE(MAX(seen_run), 0) FROM location), 0)",
                    params![asset, root, src, m.size as i64, m.mtime_ns],
                )?;
            }
        }
        Ok(tx.commit()?)
    }

    /// Forgets a pending move that failed before anything on disk changed.
    pub fn discard_move(&self, id: i64) -> Result<(), CatalogError> {
        self.conn.execute(
            "DELETE FROM move_log WHERE id = ?1 AND state = 'pending'",
            [id],
        )?;
        Ok(())
    }

    /// Moves cut short (a crash, a power cut) are settled by looking at the disk. Nothing on disk is
    /// touched, and whatever cannot be told for sure stays pending.
    pub(crate) fn resolve_pending_moves(&self) -> Result<(), CatalogError> {
        const PENDING: &str = "state = 'pending'";
        let waiting: bool = self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM move_log WHERE {PENDING})"),
            [],
            |r| r.get(0),
        )?;
        if !waiting {
            return Ok(());
        }
        // The rows of a move that is running right now are its own business.
        let Some(_lock) = self.try_lock_moves()? else {
            return Ok(());
        };
        let rows: Vec<MoveRow> = {
            let mut stmt = self
                .conn
                .prepare(&format!("{ROW} WHERE {PENDING} ORDER BY id"))?;
            stmt.query_map([], row)?.collect::<Result<_, _>>()?
        };
        for m in rows {
            // An unplugged drive makes everything on it look gone.
            if !m.src_root.is_dir() || !m.dst_path.parent().is_some_and(Path::is_dir) {
                continue;
            }
            match (file_is_gone(&m.src_path), file_is_gone(&m.dst_path)) {
                (true, false) => {
                    // Only the very bytes that were being moved prove that it happened.
                    if scanner::sha256_file(&m.dst_path).is_ok_and(|sha| sha == m.sha256) {
                        self.finish_move(m.id, MoveState::Done, m.dst_root.as_deref())?;
                    }
                }
                (false, true) => self.discard_move(m.id)?,
                (false, false) => self.finish_move(m.id, MoveState::KeptOriginal, None)?,
                (true, true) => {}
            }
        }
        Ok(())
    }
}
