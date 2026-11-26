use crate::labels::{append_filter, unexpected};
use crate::{AssetId, Catalog, CatalogError, Filter, LabelId, LabelKind};
use rusqlite::{OptionalExtension, Row, ToSql, params_from_iter};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub asset: AssetId,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub modified_ns: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    PathAsc,
    PathDesc,
    SizeAsc,
    SizeDesc,
    ModifiedAsc,
    ModifiedDesc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelCount {
    pub label: LabelId,
    pub kind: LabelKind,
    pub name: String,
    pub images: i64,
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
        }
    }
}

fn entry_from_row(r: &Row<'_>) -> rusqlite::Result<Entry> {
    let size: i64 = r.get(3)?;
    Ok(Entry {
        asset: AssetId(r.get(0)?),
        path: PathBuf::from(r.get::<_, String>(1)?),
        sha256: r.get(2)?,
        size: size.max(0) as u64,
        modified_ns: r.get(4)?,
    })
}

impl Catalog {
    pub fn entries(&self, filter: &Filter, sort: Sort) -> Result<Vec<Entry>, CatalogError> {
        let mut sql = String::from(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns
             FROM location l JOIN asset a ON a.id = l.asset_id
             WHERE l.missing = 0",
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        append_filter(&mut sql, &mut args, filter);
        sql.push(' ');
        sql.push_str(sort.order_clause());
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), entry_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn labels_in_use(&self) -> Result<Vec<LabelCount>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT lb.id, lb.kind, lb.name, COUNT(DISTINCT al.asset_id)
             FROM label lb
             JOIN asset_label al ON al.label_id = lb.id
             WHERE al.status <> 'rejected'
               AND EXISTS (SELECT 1 FROM location l WHERE l.asset_id = al.asset_id AND l.missing = 0)
             GROUP BY lb.id, lb.kind, lb.name
             ORDER BY lb.kind, lb.name",
        )?;
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

    pub fn entry_for(&self, asset: AssetId) -> Result<Option<Entry>, CatalogError> {
        Ok(self
            .conn
            .query_row(
                "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns
                 FROM location l JOIN asset a ON a.id = l.asset_id
                 WHERE l.asset_id = ?1 AND l.missing = 0
                 ORDER BY l.path ASC
                 LIMIT 1",
                [asset.0],
                entry_from_row,
            )
            .optional()?)
    }
}
