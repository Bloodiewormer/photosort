use crate::labels::{append_filter, unexpected};
use crate::{AssetId, Catalog, CatalogError, Filter, LabelId, LabelKind, MediaType, Orientation};
use rusqlite::{OptionalExtension, Row, ToSql, params_from_iter};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub asset: AssetId,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub modified_ns: Option<i64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub format: Option<String>,
    pub captured_at_ns: Option<i64>,
    pub media_type: MediaType,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    PathAsc,
    PathDesc,
    SizeAsc,
    SizeDesc,
    ModifiedAsc,
    ModifiedDesc,
    CapturedAsc,
    CapturedDesc,
    PixelsAsc,
    PixelsDesc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelCount {
    pub label: LabelId,
    pub kind: LabelKind,
    pub name: String,
    pub images: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatCount {
    pub format: String,
    pub images: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrientationCount {
    pub orientation: Orientation,
    pub images: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YearCount {
    pub year: i32,
    pub images: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaTypeCount {
    pub media_type: MediaType,
    pub count: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MetadataFacets {
    pub media_types: Vec<MediaTypeCount>,
    pub formats: Vec<FormatCount>,
    pub orientations: Vec<OrientationCount>,
    pub years: Vec<YearCount>,
}

impl MetadataFacets {
    pub fn photos_count(&self) -> i64 {
        self.media_types
            .iter()
            .find(|m| m.media_type == MediaType::Image)
            .map(|m| m.count)
            .unwrap_or(0)
    }

    pub fn videos_count(&self) -> i64 {
        self.media_types
            .iter()
            .find(|m| m.media_type == MediaType::Video)
            .map(|m| m.count)
            .unwrap_or(0)
    }
}

impl Sort {
    fn order_clause(self) -> &'static str {
        match self {
            Self::PathAsc => "ORDER BY l.path ASC",
            Self::PathDesc => "ORDER BY l.path DESC",
            Self::SizeAsc => "ORDER BY l.size ASC, l.path ASC",
            Self::SizeDesc => "ORDER BY l.size DESC, l.path ASC",
            Self::ModifiedAsc => "ORDER BY l.mtime_ns ASC NULLS LAST, l.path ASC",
            Self::ModifiedDesc => "ORDER BY l.mtime_ns DESC NULLS LAST, l.path ASC",
            Self::CapturedAsc => {
                "ORDER BY COALESCE(a.captured_at_ns, l.mtime_ns) ASC NULLS LAST, l.path ASC"
            }
            Self::CapturedDesc => {
                "ORDER BY COALESCE(a.captured_at_ns, l.mtime_ns) DESC NULLS LAST, l.path ASC"
            }
            Self::PixelsAsc => {
                "ORDER BY (CASE WHEN a.width IS NOT NULL AND a.height IS NOT NULL THEN a.width * a.height END) ASC NULLS LAST, l.path ASC"
            }
            Self::PixelsDesc => {
                "ORDER BY (CASE WHEN a.width IS NOT NULL AND a.height IS NOT NULL THEN a.width * a.height END) DESC NULLS LAST, l.path ASC"
            }
        }
    }
}

pub(crate) fn entry_from_row(r: &Row<'_>) -> rusqlite::Result<Entry> {
    let size: i64 = r.get(3)?;
    let width: Option<i64> = r.get(5)?;
    let height: Option<i64> = r.get(6)?;
    let mtype_str: Option<String> = r.get(9)?;
    let media_type = mtype_str
        .as_deref()
        .and_then(MediaType::from_str)
        .unwrap_or(MediaType::Image);
    let duration_ms: Option<i64> = r.get(10)?;
    Ok(Entry {
        asset: AssetId(r.get(0)?),
        path: PathBuf::from(r.get::<_, String>(1)?),
        sha256: r.get(2)?,
        size: size.max(0) as u64,
        modified_ns: r.get(4)?,
        width: width.and_then(|v| u32::try_from(v).ok()),
        height: height.and_then(|v| u32::try_from(v).ok()),
        format: r.get(7)?,
        captured_at_ns: r.get(8)?,
        media_type,
        duration_ms: duration_ms.and_then(|v| u64::try_from(v).ok()),
    })
}

// `(asset_id, label_id)` is the primary key of `asset_label`, so each asset appears at most once
// per label without needing `COUNT(DISTINCT ...)`. Grouping by `al.label_id` lets SQLite stream
// `asset_label_active_label` in order without a temporary B-tree for GROUP BY.
const LABELS_IN_USE_SQL: &str = "SELECT lb.id, lb.kind, lb.name, c.images
     FROM (
         SELECT al.label_id, COUNT(*) AS images
         FROM asset_label al
         WHERE al.status <> 'rejected'
           AND EXISTS (SELECT 1 FROM location l WHERE l.asset_id = al.asset_id AND l.missing = 0)
         GROUP BY al.label_id
     ) c
     JOIN label lb ON lb.id = c.label_id
     ORDER BY lb.kind, lb.name";

fn build_entries_sql(filter: &Filter, sort: Sort) -> (String, Vec<Box<dyn ToSql>>) {
    let mut sql = String::from(
        "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                a.width, a.height, a.format, a.captured_at_ns,
                a.media_type, a.duration_ms
         FROM location l JOIN asset a ON a.id = l.asset_id
         WHERE l.missing = 0",
    );
    let mut args: Vec<Box<dyn ToSql>> = Vec::new();
    append_filter(&mut sql, &mut args, filter);
    sql.push(' ');
    sql.push_str(sort.order_clause());
    (sql, args)
}

impl Catalog {
    pub fn explain_entries(
        &self,
        filter: &Filter,
        sort: Sort,
    ) -> Result<Vec<String>, CatalogError> {
        let (sql, args) = build_entries_sql(filter, sort);
        self.explain_sql(&sql, params_from_iter(args.iter()))
    }

    pub fn explain_labels_in_use(&self) -> Result<Vec<String>, CatalogError> {
        self.explain_sql(LABELS_IN_USE_SQL, [])
    }

    pub fn entries(&self, filter: &Filter, sort: Sort) -> Result<Vec<Entry>, CatalogError> {
        let (sql, args) = build_entries_sql(filter, sort);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), entry_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn labels_in_use(&self) -> Result<Vec<LabelCount>, CatalogError> {
        let mut stmt = self.conn.prepare(LABELS_IN_USE_SQL)?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(1)?;
            Ok(LabelCount {
                label: LabelId(r.get(0)?),
                kind: LabelKind::parse(&kind).ok_or_else(|| unexpected(1, &kind))?,
                name: r.get(2)?,
                images: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Active label counts restricted to present assets under `roots`.
    pub fn labels_in_use_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<LabelCount>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT lb.id, lb.kind, lb.name, c.images
             FROM (
                 SELECT al.label_id, COUNT(*) AS images
                 FROM asset_label al
                 WHERE al.status <> 'rejected'
                   AND EXISTS (SELECT 1 FROM location l
                               WHERE l.asset_id = al.asset_id
                                 AND l.missing = 0
                                 AND l.root IN ({marks}))
                 GROUP BY al.label_id
             ) c
             JOIN label lb ON lb.id = c.label_id
             ORDER BY lb.kind, lb.name"
        );
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let kind: String = r.get(1)?;
            Ok(LabelCount {
                label: LabelId(r.get(0)?),
                kind: LabelKind::parse(&kind).ok_or_else(|| unexpected(1, &kind))?,
                name: r.get(2)?,
                images: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn entry_for(&self, asset: AssetId) -> Result<Option<Entry>, CatalogError> {
        Ok(self
            .conn
            .query_row(
                "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                        a.width, a.height, a.format, a.captured_at_ns,
                        a.media_type, a.duration_ms
                 FROM location l JOIN asset a ON a.id = l.asset_id
                 WHERE l.asset_id = ?1 AND l.missing = 0
                 ORDER BY l.path ASC
                 LIMIT 1",
                [asset.0],
                entry_from_row,
            )
            .optional()?)
    }

    /// Returns a present entry for `asset` restricted to `roots`.
    pub fn entry_for_in_roots(
        &self,
        asset: AssetId,
        roots: &[PathBuf],
    ) -> Result<Option<Entry>, CatalogError> {
        if roots.is_empty() {
            return Ok(None);
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns,
                    a.media_type, a.duration_ms
             FROM location l JOIN asset a ON a.id = l.asset_id
             WHERE l.asset_id = ? AND l.missing = 0 AND l.root IN ({marks})
             ORDER BY l.path ASC
             LIMIT 1"
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(roots.len() + 1);
        args.push(Box::new(asset.0));
        for root in roots {
            args.push(Box::new(root.to_string_lossy().into_owned()));
        }
        Ok(self
            .conn
            .query_row(&sql, params_from_iter(args.iter()), entry_from_row)
            .optional()?)
    }

    /// Counts distinct present assets under `roots` grouped by format, orientation, and capture year.
    pub fn metadata_facets_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<MetadataFacets, CatalogError> {
        if roots.is_empty() {
            return Ok(MetadataFacets::default());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();

        let exists_clause = format!(
            "EXISTS (SELECT 1 FROM location l WHERE l.asset_id = a.id AND l.missing = 0 AND l.root IN ({marks}))"
        );

        let media_sql = format!(
            "SELECT COALESCE(a.media_type, 'image') AS mtype, COUNT(*)
             FROM asset a
             WHERE {exists_clause}
             GROUP BY mtype
             ORDER BY mtype ASC"
        );
        let mut stmt = self.conn.prepare(&media_sql)?;
        let media_types = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                let mtype_str: String = r.get(0)?;
                let media_type = MediaType::from_str(&mtype_str).unwrap_or(MediaType::Image);
                Ok(MediaTypeCount {
                    media_type,
                    count: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let fmt_sql = format!(
            "SELECT a.format, COUNT(*)
             FROM asset a
             WHERE a.format IS NOT NULL AND {exists_clause}
             GROUP BY a.format
             ORDER BY COUNT(*) DESC, a.format ASC"
        );
        let mut stmt = self.conn.prepare(&fmt_sql)?;
        let formats = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(FormatCount {
                    format: r.get(0)?,
                    images: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let orient_sql = format!(
            "SELECT
                 CASE
                     WHEN a.width > a.height THEN 'landscape'
                     WHEN a.height > a.width THEN 'portrait'
                     ELSE 'square'
                 END AS orient,
                 COUNT(*)
             FROM asset a
             WHERE a.width > 0 AND a.height > 0 AND {exists_clause}
             GROUP BY orient"
        );
        let mut stmt = self.conn.prepare(&orient_sql)?;
        let mut raw_orients: Vec<OrientationCount> = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                let tag: String = r.get(0)?;
                let orientation = match tag.as_str() {
                    "landscape" => Orientation::Landscape,
                    "portrait" => Orientation::Portrait,
                    _ => Orientation::Square,
                };
                Ok(OrientationCount {
                    orientation,
                    images: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        raw_orients.sort_by_key(|o| o.orientation);

        let year_sql = format!(
            "SELECT a.captured_year, COUNT(*)
             FROM asset a
             WHERE a.captured_year IS NOT NULL AND {exists_clause}
             GROUP BY a.captured_year
             ORDER BY a.captured_year DESC"
        );
        let mut stmt = self.conn.prepare(&year_sql)?;
        let years = stmt
            .query_map(params_from_iter(args.iter()), |r| {
                Ok(YearCount {
                    year: r.get::<_, i64>(0)? as i32,
                    images: r.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(MetadataFacets {
            media_types,
            formats,
            orientations: raw_orients,
            years,
        })
    }
}
