use crate::{Catalog, CatalogError, MediaType};
use rusqlite::types::Type;
use rusqlite::{OptionalExtension, ToSql, params, params_from_iter};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(pub i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LabelId(pub i64);

/// General categories, specific tags and recognised entities (works, groups, people) are different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LabelKind {
    Category,
    Tag,
    Entity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Ai,
    Ocr,
    Rules,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Suggested,
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assignment {
    pub label: LabelId,
    pub kind: LabelKind,
    pub name: String,
    pub source: Source,
    pub confidence: f64,
    pub status: Status,
}

/// Identifies an analysis stage and its current version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StageSpec {
    pub stage: String,
    pub version: u32,
}

impl StageSpec {
    pub fn new(stage: impl Into<String>, version: u32) -> Self {
        Self {
            stage: stage.into(),
            version,
        }
    }
}

/// Filter over whether an image has completed every enabled analysis stage.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum ReviewedFilter {
    #[default]
    All,
    Pending,
    Reviewed,
}

/// Image orientation derived from stored pixel dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Orientation {
    Landscape,
    Portrait,
    Square,
}

/// Every condition must hold. Rejected labels never match.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Filter {
    /// The image has all of these labels.
    pub require: Vec<LabelId>,
    /// The image has at least one of these labels (ignored when empty).
    pub any: Vec<LabelId>,
    /// The image has none of these labels.
    pub exclude: Vec<LabelId>,
    /// The path contains this text, ignoring case.
    pub path_contains: Option<String>,
    /// The path contains this text (ignoring case) OR some searchable OCR line's normalized key
    /// contains the normalized query.
    pub search: Option<String>,
    /// When `Some`, only locations whose `root` is in this list match (empty list matches nothing).
    pub roots: Option<Vec<PathBuf>>,
    /// When `Some((mode, stages))`, filters by whether every stage in `stages` is done.
    /// With `stages` empty, nothing counts as `Reviewed` and everything counts as `Pending`.
    pub reviewed: Option<(ReviewedFilter, Vec<StageSpec>)>,
    /// When non-empty, only assets matching one of these media types match.
    pub media_types: Vec<MediaType>,
    /// When non-empty, only assets whose normalized format (e.g. `"JPEG"`, `"PNG"`) is in this list match.
    pub formats: Vec<String>,
    /// When non-empty, only assets matching one of these orientations match.
    pub orientations: Vec<Orientation>,
    /// When non-empty, only assets whose EXIF capture year is in this list match.
    pub capture_years: Vec<i32>,
    /// When non-empty, only assets that contain a face belonging to one of these persons match.
    pub persons: Vec<crate::PersonId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelNode {
    pub label: LabelId,
    pub kind: LabelKind,
    pub name: String,
    pub parent: Option<LabelId>,
    /// Active (non-rejected) present images matching this label (including subcategories).
    pub images: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameOutcome {
    Renamed,
    /// Another label of the same kind already has this name (case-insensitive).
    Conflict {
        existing: LabelId,
        existing_name: String,
    },
}

#[derive(Debug, PartialEq)]
pub struct ReviewItem {
    pub asset: AssetId,
    pub label: LabelId,
    pub name: String,
    pub confidence: f64,
    pub path: PathBuf,
}

impl LabelKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Category => "category",
            Self::Tag => "tag",
            Self::Entity => "entity",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "category" => Self::Category,
            "tag" => Self::Tag,
            "entity" => Self::Entity,
            _ => return None,
        })
    }
}

impl Source {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ai => "ai",
            Self::Ocr => "ocr",
            Self::Rules => "rules",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "ai" => Self::Ai,
            "ocr" => Self::Ocr,
            "rules" => Self::Rules,
            "user" => Self::User,
            _ => return None,
        })
    }
}

impl Status {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Suggested => "suggested",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "suggested" => Self::Suggested,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            _ => return None,
        })
    }
}

/// The database has CHECK constraints, so an unknown value means a damaged file: report it.
pub(crate) fn unexpected(column: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        column,
        Type::Text,
        format!("unexpected value {value:?}").into(),
    )
}

/// Matches the images that carry a label or any of its descendant categories, ignoring rejected ones.
const HAS_SUBTREE_SINGLE: &str = "EXISTS (WITH RECURSIVE sub(id) AS (\
                                  SELECT ? \
                                  UNION ALL \
                                  SELECT lb.id FROM label lb JOIN sub ON lb.parent_id = sub.id\
                                  ) \
                                  SELECT 1 FROM asset_label al \
                                  WHERE al.asset_id = l.asset_id \
                                    AND al.status <> 'rejected' \
                                    AND al.label_id IN (SELECT id FROM sub))";

/// Appends the filter's WHERE predicates against table alias `l` (location).
pub(crate) fn append_filter(sql: &mut String, args: &mut Vec<Box<dyn ToSql>>, filter: &Filter) {
    for id in &filter.require {
        sql.push_str(&format!(" AND {HAS_SUBTREE_SINGLE}"));
        args.push(Box::new(id.0));
    }
    if !filter.any.is_empty() {
        let marks = vec!["?"; filter.any.len()].join(",");
        sql.push_str(&format!(
            " AND EXISTS (WITH RECURSIVE sub(id) AS (\
               SELECT id FROM label WHERE id IN ({marks}) \
               UNION ALL \
               SELECT lb.id FROM label lb JOIN sub ON lb.parent_id = sub.id\
             ) \
             SELECT 1 FROM asset_label al \
             WHERE al.asset_id = l.asset_id \
               AND al.status <> 'rejected' \
               AND al.label_id IN (SELECT id FROM sub))"
        ));
        args.extend(filter.any.iter().map(|id| Box::new(id.0) as Box<dyn ToSql>));
    }
    for id in &filter.exclude {
        sql.push_str(&format!(" AND NOT {HAS_SUBTREE_SINGLE}"));
        args.push(Box::new(id.0));
    }
    if let Some(text) = &filter.path_contains {
        sql.push_str(" AND instr(lower(l.path), lower(?)) > 0");
        args.push(Box::new(text.clone()));
    }
    if let Some(text) = &filter.search {
        let key = crate::ocr::ocr_search_key(text);
        if key.is_empty() {
            sql.push_str(" AND instr(lower(l.path), lower(?)) > 0");
            args.push(Box::new(text.clone()));
        } else if key.chars().count() >= 3 {
            let fts_query = format!("\"{}\"", key.replace('"', "\"\""));
            sql.push_str(
                " AND (instr(lower(l.path), lower(?)) > 0 \
                   OR l.asset_id IN (SELECT o.asset_id \
                                     FROM ocr_line_fts f \
                                     JOIN ocr_line o ON o.id = f.rowid \
                                     WHERE f.search_key MATCH ? \
                                       AND instr(o.search_key, ?) > 0 \
                                       AND (o.top_strip = 0 \
                                            OR NOT EXISTS (SELECT 1 FROM asset_label al \
                                                           JOIN label lb ON lb.id = al.label_id \
                                                           WHERE al.asset_id = o.asset_id \
                                                             AND al.status <> 'rejected' \
                                                             AND lb.kind = 'category' \
                                                             AND lb.name = ?))))",
            );
            args.push(Box::new(text.clone()));
            args.push(Box::new(fts_query));
            args.push(Box::new(key));
            args.push(Box::new(crate::ocr::SCREENSHOT_CATEGORY.to_string()));
        } else {
            // shortcut: 1-2 character queries cannot use the FTS5 trigram index and fall back to
            // a plain instr scan over ocr_line.search_key.
            sql.push_str(
                " AND (instr(lower(l.path), lower(?)) > 0 \
                   OR EXISTS (SELECT 1 FROM ocr_line o \
                              WHERE o.asset_id = l.asset_id \
                                AND instr(o.search_key, ?) > 0 \
                                AND (o.top_strip = 0 \
                                     OR NOT EXISTS (SELECT 1 FROM asset_label al \
                                                    JOIN label lb ON lb.id = al.label_id \
                                                    WHERE al.asset_id = l.asset_id \
                                                      AND al.status <> 'rejected' \
                                                      AND lb.kind = 'category' \
                                                      AND lb.name = ?))))",
            );
            args.push(Box::new(text.clone()));
            args.push(Box::new(key));
            args.push(Box::new(crate::ocr::SCREENSHOT_CATEGORY.to_string()));
        }
    }
    if let Some(roots) = &filter.roots {
        if roots.is_empty() {
            sql.push_str(" AND 0");
        } else {
            let marks = vec!["?"; roots.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in roots {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
    }
    if let Some((mode, stages)) = &filter.reviewed {
        match mode {
            ReviewedFilter::All => {}
            ReviewedFilter::Reviewed => {
                if stages.is_empty() {
                    sql.push_str(" AND 0");
                } else {
                    for s in stages {
                        sql.push_str(
                            " AND EXISTS (SELECT 1 FROM analysis an \
                                          WHERE an.asset_id = l.asset_id \
                                            AND an.stage = ? \
                                            AND an.version >= ?)",
                        );
                        args.push(Box::new(s.stage.clone()));
                        args.push(Box::new(i64::from(s.version)));
                    }
                }
            }
            ReviewedFilter::Pending => {
                if !stages.is_empty() {
                    sql.push_str(" AND (");
                    for (idx, s) in stages.iter().enumerate() {
                        if idx > 0 {
                            sql.push_str(" OR ");
                        }
                        sql.push_str(
                            "NOT EXISTS (SELECT 1 FROM analysis an \
                                         WHERE an.asset_id = l.asset_id \
                                           AND an.stage = ? \
                                           AND an.version >= ?)",
                        );
                        args.push(Box::new(s.stage.clone()));
                        args.push(Box::new(i64::from(s.version)));
                    }
                    sql.push(')');
                }
            }
        }
    }
    if !filter.media_types.is_empty() {
        let marks = vec!["?"; filter.media_types.len()].join(",");
        sql.push_str(&format!(
            " AND COALESCE(a.media_type, 'image') IN ({marks})"
        ));
        for mt in &filter.media_types {
            args.push(Box::new(mt.as_str().to_string()));
        }
    }
    if !filter.persons.is_empty() {
        let marks = vec!["?"; filter.persons.len()].join(",");
        sql.push_str(&format!(
            " AND a.id IN (SELECT DISTINCT af.asset_id FROM asset_face af WHERE af.person_id IN ({marks}))"
        ));
        for p in &filter.persons {
            args.push(Box::new(p.0));
        }
    }
    if !filter.formats.is_empty() {
        let marks = vec!["?"; filter.formats.len()].join(",");
        sql.push_str(&format!(" AND a.format IN ({marks})"));
        for fmt in &filter.formats {
            args.push(Box::new(fmt.clone()));
        }
    }
    if !filter.orientations.is_empty() {
        sql.push_str(" AND (");
        for (idx, orient) in filter.orientations.iter().enumerate() {
            if idx > 0 {
                sql.push_str(" OR ");
            }
            match orient {
                Orientation::Landscape => {
                    sql.push_str("(a.width > a.height AND a.width > 0 AND a.height > 0)");
                }
                Orientation::Portrait => {
                    sql.push_str("(a.height > a.width AND a.width > 0 AND a.height > 0)");
                }
                Orientation::Square => {
                    sql.push_str("(a.width = a.height AND a.width > 0)");
                }
            }
        }
        sql.push(')');
    }
    if !filter.capture_years.is_empty() {
        let marks = vec!["?"; filter.capture_years.len()].join(",");
        sql.push_str(&format!(" AND a.captured_year IN ({marks})"));
        for &year in &filter.capture_years {
            args.push(Box::new(i64::from(year)));
        }
    }
}

impl Catalog {
    /// Returns the label, creating it the first time. Names ignore case within a kind.
    pub fn label(&self, kind: LabelKind, name: &str) -> Result<LabelId, CatalogError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO label (kind, name) VALUES (?1, ?2)",
            params![kind.as_str(), name],
        )?;
        let id = self.conn.query_row(
            "SELECT id FROM label WHERE kind = ?1 AND name = ?2",
            params![kind.as_str(), name],
            |r| r.get(0),
        )?;
        Ok(LabelId(id))
    }

    pub fn asset_of(&self, path: &Path) -> Result<Option<AssetId>, CatalogError> {
        Ok(self
            .conn
            .query_row(
                "SELECT asset_id FROM location WHERE path = ?1",
                [path.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?
            .map(AssetId))
    }

    /// Records what an analysis found. A decision made by the user is never touched.
    // shortcut: a new suggestion simply replaces the previous suggested one, whatever its source;
    // keep the best of several sources only if reviewing shows that matters.
    pub fn suggest(
        &self,
        asset: AssetId,
        label: LabelId,
        source: Source,
        confidence: f64,
    ) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
             VALUES (?1, ?2, ?3, ?4, 'suggested')
             ON CONFLICT(asset_id, label_id) DO UPDATE SET source = ?3, confidence = ?4
             WHERE status = 'suggested'",
            params![asset.0, label.0, source.as_str(), confidence],
        )?;
        Ok(())
    }

    pub fn accept(&self, asset: AssetId, label: LabelId) -> Result<(), CatalogError> {
        self.decide(asset, label, "accepted")
    }

    /// A rejection also sticks when the label was never suggested, so a later analysis cannot bring it back.
    pub fn reject(&self, asset: AssetId, label: LabelId) -> Result<(), CatalogError> {
        self.decide(asset, label, "rejected")
    }

    /// Restores a label's status from `from` back to `to` only when the row still holds the exact
    /// `(from, expected_source, expected_confidence)` left by the action being undone. Returns
    /// `true` if a row was updated, or `false` if it was missing or replaced in the meantime.
    // shortcut: guards against concurrent analysis replacements by matching (status, source,
    // confidence) rather than a monotonic revision counter; if a newer analysis can ever write
    // identical source and confidence after a reset, upgrade by adding a revision column.
    pub fn restore_status(
        &self,
        asset: AssetId,
        label: LabelId,
        expected_source: Source,
        expected_confidence: f64,
        from: Status,
        to: Status,
    ) -> Result<bool, CatalogError> {
        let changed = self.conn.execute(
            "UPDATE asset_label SET status = ?6
             WHERE asset_id = ?1 AND label_id = ?2
               AND source = ?3 AND confidence = ?4 AND status = ?5",
            params![
                asset.0,
                label.0,
                expected_source.as_str(),
                expected_confidence,
                from.as_str(),
                to.as_str(),
            ],
        )?;
        Ok(changed > 0)
    }

    fn decide(&self, asset: AssetId, label: LabelId, status: &str) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
             VALUES (?1, ?2, 'user', 1.0, ?3)
             ON CONFLICT(asset_id, label_id) DO UPDATE SET status = ?3",
            params![asset.0, label.0, status],
        )?;
        if status == "accepted" && crate::works::is_page_type_label(&self.conn, label)? {
            crate::works::reconcile_page_types_in_tx(&self.conn, asset)?;
        }
        Ok(())
    }

    pub fn labels_of(&self, asset: AssetId) -> Result<Vec<Assignment>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT l.id, l.kind, l.name, a.source, a.confidence, a.status
             FROM asset_label a JOIN label l ON l.id = a.label_id
             WHERE a.asset_id = ?1 ORDER BY l.kind, l.name",
        )?;
        let rows = stmt.query_map([asset.0], |r| {
            let kind: String = r.get(1)?;
            let source: String = r.get(3)?;
            let status: String = r.get(5)?;
            Ok(Assignment {
                label: LabelId(r.get(0)?),
                kind: LabelKind::parse(&kind).ok_or_else(|| unexpected(1, &kind))?,
                name: r.get(2)?,
                source: Source::parse(&source).ok_or_else(|| unexpected(3, &source))?,
                confidence: r.get(4)?,
                status: Status::parse(&status).ok_or_else(|| unexpected(5, &status))?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn find(&self, filter: &Filter) -> Result<Vec<PathBuf>, CatalogError> {
        let mut sql = String::from(
            "SELECT l.path FROM location l JOIN asset a ON a.id = l.asset_id WHERE l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        append_filter(&mut sql, &mut args, filter);
        sql.push_str(" ORDER BY l.path");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
        Ok(rows
            .map(|p| p.map(PathBuf::from))
            .collect::<Result<_, _>>()?)
    }

    // `MIN(path)` is NULL iff the asset has no present location, avoiding a duplicate EXISTS probe.
    const REVIEW_QUEUE_SQL: &str = "SELECT a.asset_id, a.label_id, l.name, a.confidence,
                (SELECT MIN(path) FROM location WHERE asset_id = a.asset_id AND missing = 0)
         FROM asset_label a JOIN label l ON l.id = a.label_id
         WHERE a.status = 'suggested' AND a.confidence < ?1
         ORDER BY a.confidence, a.asset_id, a.label_id";

    pub fn explain_review_queue(&self, below: f64) -> Result<Vec<String>, CatalogError> {
        self.explain_sql(Self::REVIEW_QUEUE_SQL, [below])
    }

    /// Suggestions the user has not decided on and whose confidence is below `below`, lowest first.
    pub fn review_queue(&self, below: f64) -> Result<Vec<ReviewItem>, CatalogError> {
        let mut stmt = self.conn.prepare(Self::REVIEW_QUEUE_SQL)?;
        let rows = stmt.query_map([below], |r| {
            let Some(path) = r.get::<_, Option<String>>(4)? else {
                return Ok(None);
            };
            Ok(Some(ReviewItem {
                asset: AssetId(r.get(0)?),
                label: LabelId(r.get(1)?),
                name: r.get(2)?,
                confidence: r.get(3)?,
                path: PathBuf::from(path),
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(item) = row? {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Suggestions restricted to assets with at least one present location under `roots`.
    pub fn review_queue_in_roots(
        &self,
        below: f64,
        roots: &[PathBuf],
    ) -> Result<Vec<ReviewItem>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT a.asset_id, a.label_id, l.name, a.confidence,
                    (SELECT MIN(path) FROM location
                     WHERE asset_id = a.asset_id AND missing = 0 AND root IN ({marks}))
             FROM asset_label a JOIN label l ON l.id = a.label_id
             WHERE a.status = 'suggested' AND a.confidence < ?
             ORDER BY a.confidence, a.asset_id, a.label_id"
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(roots.len() + 1);
        for root in roots {
            args.push(Box::new(root.to_string_lossy().into_owned()));
        }
        args.push(Box::new(below));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let Some(path) = r.get::<_, Option<String>>(4)? else {
                return Ok(None);
            };
            Ok(Some(ReviewItem {
                asset: AssetId(r.get(0)?),
                label: LabelId(r.get(1)?),
                name: r.get(2)?,
                confidence: r.get(3)?,
                path: PathBuf::from(path),
            }))
        })?;
        let mut out = Vec::new();
        for row in rows {
            if let Some(item) = row? {
                out.push(item);
            }
        }
        Ok(out)
    }

    /// Lists all labels with their parent link and active present image counts (subcategories included).
    pub fn list_labels(&self) -> Result<Vec<LabelNode>, CatalogError> {
        self.list_labels_scoped(None)
    }

    /// Lists all labels with active present image counts restricted to `roots` (subcategories included).
    pub fn list_labels_in_roots(&self, roots: &[PathBuf]) -> Result<Vec<LabelNode>, CatalogError> {
        self.list_labels_scoped(Some(roots))
    }

    fn list_labels_scoped(
        &self,
        roots: Option<&[PathBuf]>,
    ) -> Result<Vec<LabelNode>, CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, kind, name, parent_id
                 FROM label
                 ORDER BY kind, name COLLATE NOCASE",
            )?;
            let rows = stmt.query_map([], |row| {
                let kind: String = row.get(1)?;
                Ok(LabelNode {
                    label: LabelId(row.get(0)?),
                    kind: LabelKind::parse(&kind).ok_or_else(|| unexpected(1, &kind))?,
                    name: row.get(2)?,
                    parent: row.get::<_, Option<i64>>(3)?.map(LabelId),
                    images: 0,
                })
            })?;
            return Ok(rows.collect::<Result<_, _>>()?);
        }

        let (root_clause, args): (String, Vec<Box<dyn ToSql>>) = match roots {
            None => (String::new(), Vec::new()),
            Some(r) => {
                let marks = vec!["?"; r.len()].join(",");
                let a = r
                    .iter()
                    .map(|p| Box::new(p.to_string_lossy().into_owned()) as Box<dyn ToSql>)
                    .collect();
                (format!(" AND l.root IN ({marks})"), a)
            }
        };

        let sql = format!(
            "WITH RECURSIVE anc(descendant_id, ancestor_id) AS (
                 SELECT id, id FROM label
                 UNION ALL
                 SELECT a.descendant_id, l.parent_id
                 FROM anc a
                 JOIN label l ON l.id = a.ancestor_id
                 WHERE l.parent_id IS NOT NULL
             )
             SELECT lb.id, lb.kind, lb.name, lb.parent_id, COALESCE(c.images, 0)
             FROM label lb
             LEFT JOIN (
                 SELECT anc.ancestor_id AS label_id, COUNT(DISTINCT al.asset_id) AS images
                 FROM anc
                 JOIN asset_label al ON al.label_id = anc.descendant_id
                 WHERE al.status <> 'rejected'
                   AND EXISTS (SELECT 1 FROM location l
                               WHERE l.asset_id = al.asset_id
                                 AND l.missing = 0{root_clause})
                 GROUP BY anc.ancestor_id
             ) c ON c.label_id = lb.id
             ORDER BY lb.kind, lb.name COLLATE NOCASE"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| {
            let kind: String = row.get(1)?;
            Ok(LabelNode {
                label: LabelId(row.get(0)?),
                kind: LabelKind::parse(&kind).ok_or_else(|| unexpected(1, &kind))?,
                name: row.get(2)?,
                parent: row.get::<_, Option<i64>>(3)?.map(LabelId),
                images: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Creates a new category (optionally under `parent`). Fails if the name is empty, already
    /// exists (case-insensitive), or `parent` is not an existing category.
    pub fn create_category(
        &self,
        name: &str,
        parent: Option<LabelId>,
    ) -> Result<LabelId, CatalogError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(CatalogError::InvalidOperation(
                "El nombre de la categoría no puede estar vacío".into(),
            ));
        }
        if let Some(p) = parent {
            let parent_kind: Option<String> = self
                .conn
                .query_row("SELECT kind FROM label WHERE id = ?1", [p.0], |r| r.get(0))
                .optional()?;
            if parent_kind.as_deref() != Some("category") {
                return Err(CatalogError::InvalidOperation(
                    "La categoría padre no existe".into(),
                ));
            }
        }
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM label WHERE kind = 'category' AND name = ?1",
                [trimmed],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(CatalogError::InvalidOperation(format!(
                "Ya existe una categoría llamada «{trimmed}»"
            )));
        }
        self.conn.execute(
            "INSERT INTO label (kind, name, parent_id) VALUES ('category', ?1, ?2)",
            params![trimmed, parent.map(|p| p.0)],
        )?;
        Ok(LabelId(self.conn.last_insert_rowid()))
    }

    /// Creates a new flat tag. Fails if empty or already exists (case-insensitive).
    pub fn create_tag(&self, name: &str) -> Result<LabelId, CatalogError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(CatalogError::InvalidOperation(
                "El nombre de la etiqueta no puede estar vacío".into(),
            ));
        }
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM label WHERE kind = 'tag' AND name = ?1",
                [trimmed],
                |r| r.get(0),
            )
            .optional()?;
        if existing.is_some() {
            return Err(CatalogError::InvalidOperation(format!(
                "Ya existe una etiqueta llamada «{trimmed}»"
            )));
        }
        self.conn.execute(
            "INSERT INTO label (kind, name, parent_id) VALUES ('tag', ?1, NULL)",
            [trimmed],
        )?;
        Ok(LabelId(self.conn.last_insert_rowid()))
    }

    /// Renames a category or tag. If another label of the same kind already has `new_name`
    /// (case-insensitive), returns `RenameOutcome::Conflict` so the caller can offer a merge.
    pub fn rename_label(&self, id: LabelId, new_name: &str) -> Result<RenameOutcome, CatalogError> {
        let trimmed = new_name.trim();
        if trimmed.is_empty() {
            return Err(CatalogError::InvalidOperation(
                "El nombre no puede estar vacío".into(),
            ));
        }
        let kind: String = self
            .conn
            .query_row("SELECT kind FROM label WHERE id = ?1", [id.0], |r| r.get(0))
            .optional()?
            .ok_or_else(|| CatalogError::InvalidOperation("La etiqueta no existe".into()))?;
        if kind == "entity" {
            return Err(CatalogError::InvalidOperation(
                "Las obras se gestionan desde la pestaña Obras".into(),
            ));
        }
        let conflict: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, name FROM label WHERE kind = ?1 AND name = ?2 AND id <> ?3",
                params![kind, trimmed, id.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((existing_id, existing_name)) = conflict {
            return Ok(RenameOutcome::Conflict {
                existing: LabelId(existing_id),
                existing_name,
            });
        }
        self.conn.execute(
            "UPDATE label SET name = ?2 WHERE id = ?1",
            params![id.0, trimmed],
        )?;
        Ok(RenameOutcome::Renamed)
    }

    /// Looks up the name of a label by its id.
    pub fn label_name(&self, id: LabelId) -> Result<Option<String>, CatalogError> {
        Ok(self
            .conn
            .query_row("SELECT name FROM label WHERE id = ?1", [id.0], |r| r.get(0))
            .optional()?)
    }

    /// Moves a category under `new_parent` (or to the root when `None`). Rejects cycles.
    pub fn move_category(
        &self,
        id: LabelId,
        new_parent: Option<LabelId>,
    ) -> Result<(), CatalogError> {
        let kind: Option<String> = self
            .conn
            .query_row("SELECT kind FROM label WHERE id = ?1", [id.0], |r| r.get(0))
            .optional()?;
        if kind.as_deref() != Some("category") {
            return Err(CatalogError::InvalidOperation(
                "Solo las categorías pueden moverse dentro de otra categoría".into(),
            ));
        }
        if let Some(p) = new_parent {
            if p == id {
                return Err(CatalogError::InvalidOperation(
                    "No se puede mover una categoría dentro de sí misma".into(),
                ));
            }
            let p_kind: Option<String> = self
                .conn
                .query_row("SELECT kind FROM label WHERE id = ?1", [p.0], |r| r.get(0))
                .optional()?;
            if p_kind.as_deref() != Some("category") {
                return Err(CatalogError::InvalidOperation(
                    "La categoría de destino no existe".into(),
                ));
            }
            let creates_cycle: bool = self
                .conn
                .query_row(
                    "WITH RECURSIVE anc(id) AS (
                         SELECT parent_id FROM label WHERE id = ?1
                         UNION ALL
                         SELECT l.parent_id FROM label l JOIN anc ON l.id = anc.id
                         WHERE l.parent_id IS NOT NULL
                     )
                     SELECT 1 FROM anc WHERE id = ?2 LIMIT 1",
                    params![p.0, id.0],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if creates_cycle {
                return Err(CatalogError::InvalidOperation(
                    "No se puede mover una categoría dentro de una de sus subcategorías".into(),
                ));
            }
        }
        self.conn.execute(
            "UPDATE label SET parent_id = ?2 WHERE id = ?1",
            params![id.0, new_parent.map(|p| p.0)],
        )?;
        Ok(())
    }

    /// Merges `source` into `target` (same kind, category or tag).
    /// - Assignments move to `target`; conflicts keep higher precedence:
    ///   accepted > rejected > suggested, then higher confidence.
    /// - Subcategories of `source` move to `target` without creating cycles.
    /// - If `source` has a vision rule and `target` does not, the rule moves to `target`;
    ///   if both have one, `target`'s rule is kept and `source`'s is deleted.
    /// - `source` is removed from the catalog; no files are touched.
    pub fn merge_labels(&mut self, source: LabelId, target: LabelId) -> Result<(), CatalogError> {
        if source == target {
            return Err(CatalogError::InvalidOperation(
                "No se puede fusionar una etiqueta consigo misma".into(),
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;

        let (src_kind, src_parent): (String, Option<i64>) = tx
            .query_row(
                "SELECT kind, parent_id FROM label WHERE id = ?1",
                [source.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| {
                CatalogError::InvalidOperation("La etiqueta de origen no existe".into())
            })?;
        let tgt_kind: String = tx
            .query_row("SELECT kind FROM label WHERE id = ?1", [target.0], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| {
                CatalogError::InvalidOperation("La etiqueta de destino no existe".into())
            })?;

        if src_kind == "entity" || tgt_kind == "entity" {
            return Err(CatalogError::InvalidOperation(
                "Las obras se gestionan desde la pestaña Obras".into(),
            ));
        }
        if src_kind != tgt_kind {
            return Err(CatalogError::InvalidOperation(
                "Solo se pueden fusionar elementos del mismo tipo".into(),
            ));
        }

        // 1. Reparent subcategories safely without cycles.
        if src_kind == "category" {
            let target_is_descendant: bool = tx
                .query_row(
                    "WITH RECURSIVE anc(id) AS (
                         SELECT parent_id FROM label WHERE id = ?1
                         UNION ALL
                         SELECT l.parent_id FROM label l JOIN anc ON l.id = anc.id
                         WHERE l.parent_id IS NOT NULL
                     )
                     SELECT 1 FROM anc WHERE id = ?2 LIMIT 1",
                    params![target.0, source.0],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if target_is_descendant {
                tx.execute(
                    "UPDATE label SET parent_id = ?2 WHERE id = ?1",
                    params![target.0, src_parent],
                )?;
            }
            tx.execute(
                "UPDATE label SET parent_id = ?2 WHERE parent_id = ?1",
                params![source.0, target.0],
            )?;
        }

        // 2. Merge asset_label rows respecting user decisions: accepted > rejected > suggested,
        //    then higher confidence.
        let src_rows: Vec<(i64, String, f64, String)> = {
            let mut stmt = tx.prepare(
                "SELECT asset_id, source, confidence, status
                 FROM asset_label
                 WHERE label_id = ?1",
            )?;
            let rows = stmt.query_map([source.0], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            rows.collect::<Result<_, _>>()?
        };

        for (asset_id, src_source, src_conf, src_status) in src_rows {
            let tgt_row: Option<(f64, String)> = tx
                .query_row(
                    "SELECT confidence, status FROM asset_label WHERE asset_id = ?1 AND label_id = ?2",
                    params![asset_id, target.0],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            match tgt_row {
                None => {
                    tx.execute(
                        "INSERT INTO asset_label (asset_id, label_id, source, confidence, status)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![asset_id, target.0, src_source, src_conf, src_status],
                    )?;
                }
                Some((tgt_conf, tgt_status)) => {
                    if status_precedence(&src_status) > status_precedence(&tgt_status)
                        || (status_precedence(&src_status) == status_precedence(&tgt_status)
                            && src_conf > tgt_conf)
                    {
                        tx.execute(
                            "UPDATE asset_label SET source = ?3, confidence = ?4, status = ?5
                             WHERE asset_id = ?1 AND label_id = ?2",
                            params![asset_id, target.0, src_source, src_conf, src_status],
                        )?;
                    }
                }
            }
        }
        tx.execute("DELETE FROM asset_label WHERE label_id = ?1", [source.0])?;

        // 3. Merge vision_rule and vision_managed_label.
        let src_has_rule = tx
            .query_row(
                "SELECT 1 FROM vision_rule WHERE label_id = ?1",
                [source.0],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let tgt_has_rule = tx
            .query_row(
                "SELECT 1 FROM vision_rule WHERE label_id = ?1",
                [target.0],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let src_managed = tx
            .query_row(
                "SELECT 1 FROM vision_managed_label WHERE label_id = ?1",
                [source.0],
                |_| Ok(()),
            )
            .optional()?
            .is_some();

        if src_has_rule {
            if tgt_has_rule {
                tx.execute("DELETE FROM vision_rule WHERE label_id = ?1", [source.0])?;
            } else {
                tx.execute(
                    "UPDATE vision_rule SET label_id = ?2 WHERE label_id = ?1",
                    params![source.0, target.0],
                )?;
                if src_managed {
                    tx.execute(
                        "INSERT OR IGNORE INTO vision_managed_label (label_id) VALUES (?1)",
                        [target.0],
                    )?;
                }
            }
        }
        tx.execute(
            "DELETE FROM vision_managed_label WHERE label_id = ?1",
            [source.0],
        )?;

        // 4. Delete source label.
        tx.execute("DELETE FROM label WHERE id = ?1", [source.0])?;
        tx.commit()?;
        Ok(())
    }

    /// Removes a category or tag from the catalog. Direct subcategories are promoted to its parent.
    /// Never touches files on disk.
    pub fn delete_label(&mut self, id: LabelId) -> Result<bool, CatalogError> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let Some((kind, parent_id)): Option<(String, Option<i64>)> = tx
            .query_row(
                "SELECT kind, parent_id FROM label WHERE id = ?1",
                [id.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
        else {
            return Ok(false);
        };
        if kind == "entity" {
            return Err(CatalogError::InvalidOperation(
                "Las obras se eliminan desde la pestaña Obras".into(),
            ));
        }
        if kind == "category" {
            tx.execute(
                "UPDATE label SET parent_id = ?2 WHERE parent_id = ?1",
                params![id.0, parent_id],
            )?;
        }
        tx.execute("DELETE FROM asset_label WHERE label_id = ?1", [id.0])?;
        tx.execute("DELETE FROM vision_rule WHERE label_id = ?1", [id.0])?;
        tx.execute(
            "DELETE FROM vision_managed_label WHERE label_id = ?1",
            [id.0],
        )?;
        tx.execute("DELETE FROM label WHERE id = ?1", [id.0])?;
        tx.commit()?;
        Ok(true)
    }
}

fn status_precedence(status: &str) -> u8 {
    match status {
        "accepted" => 3,
        "rejected" => 2,
        "suggested" => 1,
        _ => 0,
    }
}
