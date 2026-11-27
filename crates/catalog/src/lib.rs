mod analysis;
mod browse;
mod dupes;
mod labels;
mod ocr;
mod vision;
mod works;

pub use analysis::{Pending, ReviewedCounts, StageErrorRecord, sanitize_stage_error_reason};
pub use browse::{Entry, LabelCount, Sort};
pub use dupes::DuplicateGroup;
pub use labels::{
    AssetId, Assignment, Filter, LabelId, LabelKind, ReviewItem, ReviewedFilter, Source, StageSpec,
    Status,
};
pub use ocr::{
    OCR_STAGE, OCR_VERSION, OcrLine, OcrRecognizer, SCREENSHOT_CATEGORY, is_top_strip,
    ocr_search_key,
};
pub use vision::{
    CAT_ANIMALS, CAT_ANIME, CAT_COMPUTERS_HARDWARE, CAT_MANHWA, CAT_NATURE_LANDSCAPES,
    CAT_TECH_ELECTRONICS, CAT_VIDEOGAMES, CAT_WEBTOON, CompiledRule, CompiledVisionRules,
    DEFAULT_CATEGORY_THRESHOLD, DEFAULT_CUSTOM_THRESHOLD, DEFAULT_PERSON_THRESHOLD,
    DEFAULT_TYPE_THRESHOLD, DetectedFace, EMBEDDING_BYTES, EMBEDDING_DIM, MANGA_COVER_CATEGORY,
    RankedAsset, RuleId, RuleMode, SIGLIP2_MODEL_ID, TAG_PERSON, TYPE_DOCUMENT, TYPE_ILLUSTRATION,
    TYPE_MANGA, TYPE_MEME, TYPE_PHOTO, VISION_STAGE, VISION_VERSION, VisionRule, VisionSuggestion,
    YUNET_FACE_SCORE_THRESHOLD, decode_embedding_array, encode_embedding,
    independent_ordering_confidence,
};
pub use works::{
    AssetWork, AssetWorkMatches, CREDITS_CATEGORY, PROMO_CATEGORY, ParsedWorkFilename,
    RECRUITMENT_CATEGORY, WorkAlias, WorkEntry, WorkMatch, WorkMatcher, WorkProposal, WorkSummary,
    extract_work_title_from_stem, parse_work_from_stem, work_match_key,
};

use fsguard::{DestPath, LibraryRoot};
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt;
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

const SCHEMA: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA cache_size = -32000;
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
CREATE INDEX IF NOT EXISTS location_missing_asset ON location(missing, asset_id);
CREATE INDEX IF NOT EXISTS location_asset_missing ON location(asset_id, missing);
CREATE INDEX IF NOT EXISTS location_root_missing ON location(root, missing, asset_id);
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
CREATE INDEX IF NOT EXISTS asset_label_active_label
    ON asset_label(label_id, asset_id) WHERE status <> 'rejected';
CREATE UNIQUE INDEX IF NOT EXISTS asset_label_active_asset
    ON asset_label(asset_id, label_id) WHERE status <> 'rejected';
CREATE INDEX IF NOT EXISTS asset_label_review
    ON asset_label(confidence, asset_id, label_id) WHERE status = 'suggested';
CREATE TABLE IF NOT EXISTS analysis (
    asset_id INTEGER NOT NULL REFERENCES asset(id),
    stage    TEXT NOT NULL,
    version  INTEGER NOT NULL,
    PRIMARY KEY (asset_id, stage)
);
CREATE TABLE IF NOT EXISTS ocr_line (
    -- Explicit so that VACUUM keeps it: the FTS index refers to lines by this number.
    id         INTEGER PRIMARY KEY,
    asset_id   INTEGER NOT NULL REFERENCES asset(id),
    line       INTEGER NOT NULL,
    text       TEXT NOT NULL,
    search_key TEXT NOT NULL,
    score      REAL NOT NULL CHECK (score BETWEEN 0 AND 1),
    recognizer TEXT NOT NULL CHECK (recognizer IN ('v6', 'korean')),
    x0         REAL NOT NULL,
    y0         REAL NOT NULL,
    x1         REAL NOT NULL,
    y1         REAL NOT NULL,
    top_strip  INTEGER NOT NULL CHECK (top_strip IN (0, 1)),
    UNIQUE (asset_id, line)
);
CREATE VIRTUAL TABLE IF NOT EXISTS ocr_line_fts USING fts5(
    search_key,
    content='ocr_line',
    content_rowid='id',
    tokenize='trigram'
);
CREATE TRIGGER IF NOT EXISTS ocr_line_ai AFTER INSERT ON ocr_line BEGIN
    INSERT INTO ocr_line_fts(rowid, search_key) VALUES (new.id, new.search_key);
END;
CREATE TRIGGER IF NOT EXISTS ocr_line_ad AFTER DELETE ON ocr_line BEGIN
    INSERT INTO ocr_line_fts(ocr_line_fts, rowid, search_key)
    VALUES ('delete', old.id, old.search_key);
END;
CREATE TRIGGER IF NOT EXISTS ocr_line_au AFTER UPDATE ON ocr_line BEGIN
    INSERT INTO ocr_line_fts(ocr_line_fts, rowid, search_key)
    VALUES ('delete', old.id, old.search_key);
    INSERT INTO ocr_line_fts(rowid, search_key) VALUES (new.id, new.search_key);
END;
CREATE TABLE IF NOT EXISTS work_alias (
    label_id INTEGER NOT NULL REFERENCES label(id),
    alias    TEXT NOT NULL,
    key      TEXT NOT NULL,
    UNIQUE (label_id, key)
);
CREATE INDEX IF NOT EXISTS work_alias_label ON work_alias(label_id);
CREATE TABLE IF NOT EXISTS catalog_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS asset_embedding (
    asset_id INTEGER PRIMARY KEY REFERENCES asset(id),
    model    TEXT NOT NULL,
    vector   BLOB NOT NULL CHECK (length(vector) = 3072)
);
CREATE INDEX IF NOT EXISTS asset_embedding_model ON asset_embedding(model, asset_id);
CREATE TABLE IF NOT EXISTS asset_face (
    id       INTEGER PRIMARY KEY,
    asset_id INTEGER NOT NULL REFERENCES asset(id),
    face_idx INTEGER NOT NULL,
    x1       REAL NOT NULL,
    y1       REAL NOT NULL,
    x2       REAL NOT NULL,
    y2       REAL NOT NULL,
    score    REAL NOT NULL CHECK (score BETWEEN 0 AND 1),
    UNIQUE (asset_id, face_idx)
);
CREATE INDEX IF NOT EXISTS asset_face_asset ON asset_face(asset_id);
CREATE TABLE IF NOT EXISTS vision_rule (
    id          INTEGER PRIMARY KEY,
    label_id    INTEGER NOT NULL UNIQUE REFERENCES label(id),
    mode        TEXT NOT NULL CHECK (mode IN ('type', 'independent')),
    threshold   REAL NOT NULL,
    enabled     INTEGER NOT NULL CHECK (enabled IN (0, 1)) DEFAULT 1,
    positives   TEXT NOT NULL,
    negatives   TEXT NOT NULL DEFAULT '',
    builtin_key TEXT UNIQUE
);
CREATE TABLE IF NOT EXISTS vision_managed_label (
    label_id INTEGER PRIMARY KEY REFERENCES label(id)
);
CREATE TABLE IF NOT EXISTS text_embedding_cache (
    model  TEXT NOT NULL,
    text   TEXT NOT NULL,
    vector BLOB NOT NULL CHECK (length(vector) = 3072),
    PRIMARY KEY (model, text)
);
CREATE TABLE IF NOT EXISTS asset_work (
    asset_id   INTEGER NOT NULL REFERENCES asset(id),
    work_id    INTEGER NOT NULL REFERENCES label(id),
    chapter    TEXT NOT NULL DEFAULT '',
    page       INTEGER,
    source     TEXT NOT NULL CHECK (source IN ('ai', 'ocr', 'rules', 'user')),
    confidence REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    status     TEXT NOT NULL CHECK (status IN ('suggested', 'accepted', 'rejected')),
    PRIMARY KEY (asset_id, work_id)
);
CREATE INDEX IF NOT EXISTS asset_work_work ON asset_work(work_id, status);
CREATE TABLE IF NOT EXISTS stage_error (
    asset_id     INTEGER NOT NULL REFERENCES asset(id),
    stage        TEXT NOT NULL,
    reason       TEXT NOT NULL,
    last_at_secs INTEGER NOT NULL,
    attempts     INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (asset_id, stage)
);
CREATE INDEX IF NOT EXISTS stage_error_stage ON stage_error(stage, asset_id);
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
    pub stopped: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Continue,
    Stop,
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
        // wait briefly when another connection (e.g. the indexing thread) is committing in WAL mode
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        conn.execute_batch(SCHEMA)?;
        let mut cat = Self { conn };
        cat.seed_builtin_vision_rules()?;
        Ok(cat)
    }

    /// Adds new and changed images and flags the ones that disappeared. A file is only read again when
    /// its size or modified time differs from what the catalog remembers. Progress is committed in
    /// batches (and immediately on [`Control::Stop`]), so an interrupted run keeps its work and the
    /// next one resumes where it stopped.
    // shortcut: a file edited in place keeping the same size and modified time is not noticed; upgrade
    // to an explicit "rehash everything" command if that ever matters.
    pub fn index(&mut self, root: &LibraryRoot) -> Result<IndexReport, CatalogError> {
        self.index_with(root, &mut |_| Control::Continue)
    }

    /// Like [`index`](Self::index), reporting progress after each file and stopping early when the
    /// callback returns [`Control::Stop`]. Work already committed is kept and nothing is flagged missing
    /// when stopped early.
    pub fn index_with(
        &mut self,
        root: &LibraryRoot,
        progress: &mut dyn FnMut(&Progress) -> Control,
    ) -> Result<IndexReport, CatalogError> {
        // Batch commits avoid tens of thousands of individual WAL transactions on 35k+ libraries
        // while keeping resumable granularity and short write locks for concurrent readers.
        const INDEX_BATCH_SIZE: usize = 512;

        let root_key = root.as_path().to_string_lossy().into_owned();
        let scanned = scanner::scan(root.as_path())?;
        let total = scanned.files.len();
        let run: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(seen_run), 0) + 1 FROM location",
            [],
            |r| r.get(0),
        )?;
        let mut report = IndexReport {
            errors: scanned.errors,
            ..Default::default()
        };

        if progress(&Progress { done: 0, total }) == Control::Stop {
            report.stopped = true;
            return Ok(report);
        }

        let mut files = scanned.files.into_iter().enumerate().peekable();
        while files.peek().is_some() {
            let tx = self.conn.transaction()?;
            let mut batch_count = 0usize;
            let mut stopped = false;

            while batch_count < INDEX_BATCH_SIZE {
                let Some((idx, file)) = files.next() else {
                    break;
                };
                batch_count += 1;

                let path = file.path.to_string_lossy().into_owned();
                let mtime = file
                    .modified
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_nanos() as i64);
                let known: Option<(i64, Option<i64>)> = {
                    let mut stmt =
                        tx.prepare_cached("SELECT size, mtime_ns FROM location WHERE path = ?1")?;
                    stmt.query_row([&path], |r| Ok((r.get(0)?, r.get(1)?)))
                        .optional()?
                };

                if let Some((size, remembered)) = known
                    && size == file.size as i64
                    && remembered.is_some()
                    && remembered == mtime
                {
                    let mut stmt = tx.prepare_cached(
                        "UPDATE location SET seen_run = ?1, missing = 0 WHERE path = ?2",
                    )?;
                    stmt.execute(params![run, path])?;
                    report.unchanged += 1;
                } else {
                    match scanner::sha256_file(&file.path) {
                        Ok(sha) => {
                            report.hashed += 1;
                            {
                                let mut stmt = tx.prepare_cached(
                                    "INSERT OR IGNORE INTO asset (sha256, size) VALUES (?1, ?2)",
                                )?;
                                stmt.execute(params![sha, file.size as i64])?;
                            }
                            let asset_id: i64 = {
                                let mut stmt =
                                    tx.prepare_cached("SELECT id FROM asset WHERE sha256 = ?1")?;
                                stmt.query_row([&sha], |r| r.get(0))?
                            };
                            {
                                let mut stmt = tx.prepare_cached(
                                    "INSERT INTO location (asset_id, root, path, size, mtime_ns, seen_run, missing)
                                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
                                     ON CONFLICT(path) DO UPDATE SET asset_id = ?1, root = ?2, size = ?4,
                                         mtime_ns = ?5, seen_run = ?6, missing = 0",
                                )?;
                                stmt.execute(params![
                                    asset_id,
                                    root_key,
                                    path,
                                    file.size as i64,
                                    mtime,
                                    run
                                ])?;
                            }
                            if known.is_some() {
                                report.changed += 1;
                            } else {
                                report.new += 1;
                            }
                        }
                        Err(e) => {
                            // still on disk, so it must not be flagged missing
                            let mut stmt = tx.prepare_cached(
                                "UPDATE location SET seen_run = ?1 WHERE path = ?2",
                            )?;
                            stmt.execute(params![run, path])?;
                            report.errors.push((file.path, e.to_string()));
                        }
                    }
                }

                if progress(&Progress {
                    done: idx + 1,
                    total,
                }) == Control::Stop
                {
                    stopped = true;
                    break;
                }
            }

            tx.commit()?;
            if stopped {
                report.stopped = true;
                return Ok(report);
            }
        }

        // only after the whole walk: an interrupted run must not flag the files it never reached
        report.missing = self.conn.execute(
            "UPDATE location SET missing = 1 WHERE root = ?1 AND seen_run <> ?2 AND missing = 0",
            params![root_key, run],
        )?;
        Ok(report)
    }

    pub(crate) fn explain_sql<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Vec<String>, CatalogError> {
        let mut stmt = self.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
        let rows = stmt.query_map(params, |r| r.get::<_, String>(3))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn explain_counts(&self) -> Result<Vec<String>, CatalogError> {
        self.explain_sql(Self::COUNTS_SQL, [])
    }

    const COUNTS_SQL: &str = "SELECT (SELECT COUNT(DISTINCT asset_id) FROM location),
            (SELECT COUNT(*) FROM location),
            (SELECT COUNT(*) FROM location WHERE missing = 1)";

    pub fn counts(&self) -> Result<Counts, CatalogError> {
        Ok(self.conn.query_row(Self::COUNTS_SQL, [], |r| {
            Ok(Counts {
                assets: r.get(0)?,
                locations: r.get(1)?,
                missing: r.get(2)?,
            })
        })?)
    }

    /// Counts restricted to locations under the given registered `roots`.
    pub fn counts_in_roots(&self, roots: &[PathBuf]) -> Result<Counts, CatalogError> {
        if roots.is_empty() {
            return Ok(Counts {
                assets: 0,
                locations: 0,
                missing: 0,
            });
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT
                (SELECT COUNT(DISTINCT asset_id) FROM location WHERE missing = 0 AND root IN ({marks})),
                (SELECT COUNT(*) FROM location WHERE missing = 0 AND root IN ({marks})),
                (SELECT COUNT(*) FROM location WHERE missing = 1 AND root IN ({marks}))"
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() * 3);
        for _ in 0..3 {
            for r in roots {
                args.push(Box::new(r.to_string_lossy().into_owned()));
            }
        }
        Ok(self
            .conn
            .query_row(&sql, rusqlite::params_from_iter(args.iter()), |r| {
                Ok(Counts {
                    assets: r.get(0)?,
                    locations: r.get(1)?,
                    missing: r.get(2)?,
                })
            })?)
    }

    /// Count of present image locations indexed under `root`.
    pub fn count_images_in_root(&self, root: &std::path::Path) -> Result<usize, CatalogError> {
        let root_key = root.to_string_lossy().into_owned();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM location WHERE missing = 0 AND root = ?1",
            [root_key],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as usize)
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
