use crate::{AssetId, Catalog, CatalogError};
use rusqlite::{ToSql, params, params_from_iter};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const ASSET_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub asset: AssetId,
    pub path: PathBuf,
}

impl Catalog {
    /// Assets with at least one present location that have not yet been processed by `stage` at `version`.
    pub fn pending(
        &self,
        stage: &str,
        version: u32,
        limit: usize,
    ) -> Result<Vec<Pending>, CatalogError> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut stmt = self.conn.prepare(
            "SELECT l.asset_id, MIN(l.path)
             FROM location l
             LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?1
             WHERE l.missing = 0 AND (a.version IS NULL OR a.version < ?2)
             GROUP BY l.asset_id
             ORDER BY l.asset_id
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![stage, i64::from(version), limit], |r| {
            Ok(Pending {
                asset: AssetId(r.get(0)?),
                path: PathBuf::from(r.get::<_, String>(1)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Pending assets that have at least one present location under one of `roots`.
    pub fn pending_in_roots(
        &self,
        stage: &str,
        version: u32,
        roots: &[PathBuf],
        limit: usize,
    ) -> Result<Vec<Pending>, CatalogError> {
        if roots.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT l.asset_id, MIN(l.path)
             FROM location l
             LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?
             WHERE l.missing = 0
               AND l.root IN ({marks})
               AND (a.version IS NULL OR a.version < ?)
             GROUP BY l.asset_id
             ORDER BY l.asset_id
             LIMIT ?"
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(roots.len() + 3);
        args.push(Box::new(stage.to_string()));
        for root in roots {
            args.push(Box::new(root.to_string_lossy().into_owned()));
        }
        args.push(Box::new(i64::from(version)));
        args.push(Box::new(limit));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok(Pending {
                asset: AssetId(r.get(0)?),
                path: PathBuf::from(r.get::<_, String>(1)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Count of distinct pending assets with a present location under `root`.
    pub fn count_pending_in_root(
        &self,
        stage: &str,
        version: u32,
        root: &Path,
    ) -> Result<usize, CatalogError> {
        let root_key = root.to_string_lossy().into_owned();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT l.asset_id)
             FROM location l
             LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?1
             WHERE l.missing = 0
               AND l.root = ?2
               AND (a.version IS NULL OR a.version < ?3)",
            params![stage, root_key, i64::from(version)],
            |r| r.get(0),
        )?;
        Ok(count.max(0) as usize)
    }

    /// Pending assets restricted to the given `assets` set (deduplicated, ordered by `asset_id`).
    pub fn pending_in_assets(
        &self,
        stage: &str,
        version: u32,
        assets: &[AssetId],
        limit: usize,
    ) -> Result<Vec<Pending>, CatalogError> {
        if assets.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let unique: Vec<AssetId> = assets
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut out = Vec::new();
        for chunk in unique.chunks(ASSET_CHUNK_SIZE) {
            if out.len() >= limit {
                break;
            }
            let remaining = i64::try_from(limit - out.len()).unwrap_or(i64::MAX);
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT l.asset_id, MIN(l.path)
                 FROM location l
                 LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?
                 WHERE l.missing = 0
                   AND l.asset_id IN ({marks})
                   AND (a.version IS NULL OR a.version < ?)
                 GROUP BY l.asset_id
                 ORDER BY l.asset_id
                 LIMIT ?"
            );
            let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + 3);
            args.push(Box::new(stage.to_string()));
            for id in chunk {
                args.push(Box::new(id.0));
            }
            args.push(Box::new(i64::from(version)));
            args.push(Box::new(remaining));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
                Ok(Pending {
                    asset: AssetId(r.get(0)?),
                    path: PathBuf::from(r.get::<_, String>(1)?),
                })
            })?;
            for row in rows {
                out.push(row?);
            }
        }
        Ok(out)
    }

    /// Count of distinct pending assets within `assets`.
    pub fn count_pending_in_assets(
        &self,
        stage: &str,
        version: u32,
        assets: &[AssetId],
    ) -> Result<usize, CatalogError> {
        if assets.is_empty() {
            return Ok(0);
        }
        let unique: Vec<AssetId> = assets
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut total = 0usize;
        for chunk in unique.chunks(ASSET_CHUNK_SIZE) {
            let marks = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT COUNT(DISTINCT l.asset_id)
                 FROM location l
                 LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?
                 WHERE l.missing = 0
                   AND l.asset_id IN ({marks})
                   AND (a.version IS NULL OR a.version < ?)"
            );
            let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + 2);
            args.push(Box::new(stage.to_string()));
            for id in chunk {
                args.push(Box::new(id.0));
            }
            args.push(Box::new(i64::from(version)));
            let count: i64 = self
                .conn
                .query_row(&sql, params_from_iter(args.iter()), |r| r.get(0))?;
            total += count.max(0) as usize;
        }
        Ok(total)
    }

    /// Records that `asset` completed `stage` at `version`. Never lowers a recorded version.
    pub fn mark_done(&self, asset: AssetId, stage: &str, version: u32) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO analysis (asset_id, stage, version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id, stage) DO UPDATE SET version = excluded.version
             WHERE analysis.version < excluded.version",
            params![asset.0, stage, i64::from(version)],
        )?;
        Ok(())
    }
}
