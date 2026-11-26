use crate::{Catalog, CatalogError};
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
    fn as_str(self) -> &'static str {
        match self {
            Self::Ai => "ai",
            Self::Ocr => "ocr",
            Self::Rules => "rules",
            Self::User => "user",
        }
    }

    fn parse(s: &str) -> Option<Self> {
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
    fn as_str(self) -> &'static str {
        match self {
            Self::Suggested => "suggested",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
        }
    }

    fn parse(s: &str) -> Option<Self> {
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

/// Matches the images that carry the label, ignoring rejected ones. The caller appends the comparison.
const HAS: &str = "EXISTS (SELECT 1 FROM asset_label al WHERE al.asset_id = l.asset_id \
                   AND al.status <> 'rejected' AND al.label_id";

/// Appends the filter's WHERE predicates against table alias `l` (location).
pub(crate) fn append_filter(sql: &mut String, args: &mut Vec<Box<dyn ToSql>>, filter: &Filter) {
    for id in &filter.require {
        sql.push_str(&format!(" AND {HAS} = ?)"));
        args.push(Box::new(id.0));
    }
    if !filter.any.is_empty() {
        let marks = vec!["?"; filter.any.len()].join(",");
        sql.push_str(&format!(" AND {HAS} IN ({marks}))"));
        args.extend(filter.any.iter().map(|id| Box::new(id.0) as Box<dyn ToSql>));
    }
    for id in &filter.exclude {
        sql.push_str(&format!(" AND NOT {HAS} = ?)"));
        args.push(Box::new(id.0));
    }
    if let Some(text) = &filter.path_contains {
        sql.push_str(" AND instr(lower(l.path), lower(?)) > 0");
        args.push(Box::new(text.clone()));
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
        let mut sql = String::from("SELECT l.path FROM location l WHERE l.missing = 0");
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        append_filter(&mut sql, &mut args, filter);
        sql.push_str(" ORDER BY l.path");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| r.get::<_, String>(0))?;
        Ok(rows
            .map(|p| p.map(PathBuf::from))
            .collect::<Result<_, _>>()?)
    }

    /// Suggestions the user has not decided on and whose confidence is below `below`, lowest first.
    pub fn review_queue(&self, below: f64) -> Result<Vec<ReviewItem>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT a.asset_id, a.label_id, l.name, a.confidence,
                    (SELECT MIN(path) FROM location WHERE asset_id = a.asset_id AND missing = 0)
             FROM asset_label a JOIN label l ON l.id = a.label_id
             WHERE a.status = 'suggested' AND a.confidence < ?1
               AND EXISTS (SELECT 1 FROM location WHERE asset_id = a.asset_id AND missing = 0)
             ORDER BY a.confidence, a.asset_id, a.label_id",
        )?;
        let rows = stmt.query_map([below], |r| {
            Ok(ReviewItem {
                asset: AssetId(r.get(0)?),
                label: LabelId(r.get(1)?),
                name: r.get(2)?,
                confidence: r.get(3)?,
                path: PathBuf::from(r.get::<_, String>(4)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
