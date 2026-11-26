mod labels;

pub use labels::{AssetId, Assignment, Filter, LabelId, LabelKind, ReviewItem, Source, Status};

use fsguard::{DestPath, LibraryRoot};
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS asset (
    id     INTEGER PRIMARY KEY,
    sha256 TEXT NOT NULL UNIQUE,
    size   INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS location (
    id       INTEGER PRIMARY KEY,
    asset_id INTEGER NOT NULL REFERENCES asset(id),
    root     TEXT NOT NULL,
    path     TEXT NOT NULL UNIQUE,
    size     INTEGER NOT NULL,
    mtime_ns INTEGER,
    seen_run INTEGER NOT NULL,
    missing  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS location_asset ON location(asset_id);
CREATE TABLE IF NOT EXISTS label (
    id   INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('category', 'tag', 'entity')),
    name TEXT NOT NULL COLLATE NOCASE,
    UNIQUE (kind, name)
);
CREATE TABLE IF NOT EXISTS asset_label (
    asset_id   INTEGER NOT NULL REFERENCES asset(id),
    label_id   INTEGER NOT NULL REFERENCES label(id),
    source     TEXT NOT NULL CHECK (source IN ('ai', 'ocr', 'rules', 'user')),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    status     TEXT NOT NULL CHECK (status IN ('suggested', 'accepted', 'rejected')),
    PRIMARY KEY (asset_id, label_id)
);
CREATE INDEX IF NOT EXISTS asset_label_label ON asset_label(label_id);
";

#[derive(Debug)]
pub enum CatalogError {
    Db(rusqlite::Error),
    Io(std::io::Error),
}

impl fmt::Display for CatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Db(e) => write!(f, "catalog database error: {e}"),
            Self::Io(e) => write!(f, "could not read the library: {e}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<rusqlite::Error> for CatalogError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Db(e)
    }
}

impl From<std::io::Error> for CatalogError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[derive(Debug, Default)]
pub struct IndexReport {
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    /// Files that were in the catalog and are no longer on disk. They are flagged, never forgotten.
    pub missing: usize,
    /// Files whose bytes were actually read to compute a hash.
    pub hashed: usize,
    pub errors: Vec<(PathBuf, String)>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Counts {
    /// Distinct contents that currently have at least one location.
    pub assets: i64,
    pub locations: i64,
    pub missing: i64,
}

/// The catalog never touches the library: it reads files and writes only to its own database.
pub struct Catalog {
    conn: Connection,
}

impl Catalog {
    /// The database path must be a [`DestPath`], so it can never be created inside a library.
    pub fn open(db: &DestPath) -> Result<Self, CatalogError> {
        let conn = Connection::open(db.as_path())?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Adds new and changed images and flags the ones that disappeared. A file is only read again when
    /// its size or modified time differs from what the catalog remembers. Progress is committed file by
    /// file, so an interrupted run keeps its work and the next one resumes where it stopped.
    // shortcut: a file edited in place keeping the same size and modified time is not noticed; upgrade
    // to an explicit "rehash everything" command if that ever matters.
    pub fn index(&mut self, root: &LibraryRoot) -> Result<IndexReport, CatalogError> {
        let root_key = root.as_path().to_string_lossy().into_owned();
        let scanned = scanner::scan(root.as_path())?;
        let run: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seen_run), 0) + 1 FROM location",
            [],
            |r| r.get(0),
        )?;
        let mut report = IndexReport {
            errors: scanned.errors,
            ..Default::default()
        };

        for file in scanned.files {
            let path = file.path.to_string_lossy().into_owned();
            let mtime = file
                .modified
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i64);
            let known: Option<(i64, Option<i64>)> = self
                .conn
                .query_row(
                    "SELECT size, mtime_ns FROM location WHERE path = ?1",
                    [&path],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;

            if let Some((size, remembered)) = known
                && size == file.size as i64
                && remembered.is_some()
                && remembered == mtime
            {
                self.conn.execute(
                    "UPDATE location SET seen_run = ?1, missing = 0 WHERE path = ?2",
                    params![run, path],
                )?;
                report.unchanged += 1;
                continue;
            }

            let sha = match scanner::sha256_file(&file.path) {
                Ok(sha) => sha,
                Err(e) => {
                    // still on disk, so it must not be flagged missing
                    self.conn.execute(
                        "UPDATE location SET seen_run = ?1 WHERE path = ?2",
                        params![run, path],
                    )?;
                    report.errors.push((file.path, e.to_string()));
                    continue;
                }
            };
            report.hashed += 1;

            let tx = self.conn.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO asset (sha256, size) VALUES (?1, ?2)",
                params![sha, file.size as i64],
            )?;
            let asset_id: i64 =
                tx.query_row("SELECT id FROM asset WHERE sha256 = ?1", [&sha], |r| {
                    r.get(0)
                })?;
            tx.execute(
                "INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
                 ON CONFLICT(path) DO UPDATE SET asset_id = ?1, root = ?2, size = ?4,
                     mtime_ns = ?5, seen_run = ?6, missing = 0",
                params![asset_id, root_key, path, file.size as i64, mtime, run],
            )?;
            tx.commit()?;
            if known.is_some() {
                report.changed += 1;
            } else {
                report.new += 1;
            }
        }

        // only after the whole walk: an interrupted run must not flag the files it never reached
        report.missing = self.conn.execute(
            "UPDATE location SET missing = 1 WHERE root = ?1 AND seen_run <> ?2 AND missing = 0",
            params![root_key, run],
        )?;
        Ok(report)
    }

    pub fn counts(&self) -> Result<Counts, CatalogError> {
        Ok(self.conn.query_row(
            "SELECT (SELECT COUNT(DISTINCT asset_id) FROM location),
                    (SELECT COUNT(*) FROM location),
                    (SELECT COUNT(*) FROM location WHERE missing = 1)",
            [],
            |r| {
                Ok(Counts {
                    assets: r.get(0)?,
                    locations: r.get(1)?,
                    missing: r.get(2)?,
                })
            },
        )?)
    }

    /// Groups of paths that hold exactly the same bytes. Only for review: nothing is ever deleted.
    pub fn duplicates(&self) -> Result<Vec<Vec<PathBuf>>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT asset_id, path FROM location
             WHERE missing = 0 AND asset_id IN
                 (SELECT asset_id FROM location WHERE missing = 0 GROUP BY asset_id HAVING COUNT(*) > 1)
             ORDER BY asset_id, path",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        let mut groups: Vec<(i64, Vec<PathBuf>)> = Vec::new();
        for row in rows {
            let (id, path) = row?;
            match groups.last_mut() {
                Some((last, paths)) if *last == id => paths.push(path.into()),
                _ => groups.push((id, vec![path.into()])),
            }
        }
        Ok(groups.into_iter().map(|(_, paths)| paths).collect())
    }
}
