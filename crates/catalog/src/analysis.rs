use crate::{AssetId, Catalog, CatalogError};
use rusqlite::params;
use std::path::PathBuf;

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
