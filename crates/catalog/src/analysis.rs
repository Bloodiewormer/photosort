use crate::{AssetId, Catalog, CatalogError, StageSpec};
use rusqlite::{ToSql, params, params_from_iter};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const ASSET_CHUNK_SIZE: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub asset: AssetId,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewedCounts {
    pub all: usize,
    pub pending: usize,
    pub reviewed: usize,
}

/// Persistent record of a failed analysis stage for one asset.
/// `reason` is always a short curated Spanish category and never contains OCR text, file paths or file names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageErrorRecord {
    pub asset: AssetId,
    pub path: PathBuf,
    pub sha256: String,
    pub stage: String,
    pub reason: String,
    pub last_at_secs: i64,
    pub attempts: u32,
}

/// Maps any raw error message to a short, fixed Spanish reason string so persisted stage errors
/// can NEVER contain OCR text, file paths, or file names.
pub fn sanitize_stage_error_reason(stage: &str, raw: &str) -> String {
    let lower = raw.to_ascii_lowercase();
    let msg = if lower.contains("not found")
        || lower.contains("no such file")
        || lower.contains("os error 2")
        || lower.contains("os error 3")
        || lower.contains("cannot find")
        || lower.contains("no existe")
    {
        "Archivo no encontrado"
    } else if lower.contains("permission")
        || lower.contains("access is denied")
        || lower.contains("os error 5")
        || lower.contains("os error 13")
        || lower.contains("permiso")
    {
        "Sin permiso para leer el archivo"
    } else if lower.contains("database")
        || lower.contains("sqlite")
        || lower.contains("locked")
        || lower.contains("busy")
        || lower.contains("disk")
        || lower.contains("full")
        || lower.contains("catálogo")
        || lower.contains("catalogo")
    {
        "Error al guardar en el catálogo"
    } else if lower.contains("códec") || lower.contains("codec") {
        "Códec de vídeo no disponible"
    } else if lower.contains("corrupt")
        || lower.contains("invalid")
        || lower.contains("decode")
        || lower.contains("decodificar")
        || lower.contains("unsupported")
        || lower.contains("format")
        || lower.contains("header")
        || lower.contains("eof")
        || lower.contains("truncated")
        || lower.contains("image error")
        || lower.contains("no se pudo leer la imagen")
    {
        "No se pudo leer la imagen"
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "Tiempo de espera agotado"
    } else if lower.contains("interno") || lower.contains("panic") {
        "Error interno al analizar"
    } else {
        match stage {
            "rules" => "No se pudieron leer los metadatos",
            "vision" => "Error en el reconocimiento visual",
            "ocr" => "Error al leer el texto",
            _ => "Error al analizar la foto",
        }
    };
    msg.to_string()
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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
        self.pending_among(stage, version, assets, None, limit)
    }

    /// Like [`pending_in_assets`](Self::pending_in_assets), and the path returned is always inside
    /// one of `roots`: a copy left in a folder the user removed is never read. No roots, no assets.
    pub fn pending_in_assets_in_roots(
        &self,
        stage: &str,
        version: u32,
        assets: &[AssetId],
        roots: &[PathBuf],
    ) -> Result<Vec<Pending>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        self.pending_among(stage, version, assets, Some(roots), usize::MAX)
    }

    fn pending_among(
        &self,
        stage: &str,
        version: u32,
        assets: &[AssetId],
        roots: Option<&[PathBuf]>,
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
            let in_roots = roots.map_or(String::new(), |r| {
                format!("AND l.root IN ({})", vec!["?"; r.len()].join(","))
            });
            let sql = format!(
                "SELECT l.asset_id, MIN(l.path)
                 FROM location l
                 LEFT JOIN analysis a ON a.asset_id = l.asset_id AND a.stage = ?
                 WHERE l.missing = 0
                   AND l.asset_id IN ({marks})
                   AND (a.version IS NULL OR a.version < ?)
                   {in_roots}
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
            for root in roots.unwrap_or_default() {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
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
    /// Also clears any persisted error record for `(asset, stage)`.
    pub fn mark_done(&self, asset: AssetId, stage: &str, version: u32) -> Result<(), CatalogError> {
        self.conn.execute(
            "INSERT INTO analysis (asset_id, stage, version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id, stage) DO UPDATE SET version = excluded.version
             WHERE analysis.version < excluded.version",
            params![asset.0, stage, i64::from(version)],
        )?;
        self.conn.execute(
            "DELETE FROM stage_error WHERE asset_id = ?1 AND stage = ?2",
            params![asset.0, stage],
        )?;
        Ok(())
    }

    /// Removes `analysis` and `stage_error` rows for the given `assets` and `stages` so they are
    /// re-processed. Never touches `asset_label`, `ocr_line`, `asset_embedding`, or `asset_face`.
    pub fn reset_stages(
        &mut self,
        assets: &[AssetId],
        stages: &[&str],
    ) -> Result<usize, CatalogError> {
        if assets.is_empty() || stages.is_empty() {
            return Ok(0);
        }
        let unique: Vec<AssetId> = assets
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let stage_marks = vec!["?"; stages.len()].join(",");
        let tx = self.conn.transaction()?;
        let mut deleted = 0usize;
        for chunk in unique.chunks(ASSET_CHUNK_SIZE) {
            let asset_marks = vec!["?"; chunk.len()].join(",");
            let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(chunk.len() + stages.len());
            for id in chunk {
                args.push(Box::new(id.0));
            }
            for &stage in stages {
                args.push(Box::new(stage.to_string()));
            }
            let sql = format!(
                "DELETE FROM analysis
                 WHERE asset_id IN ({asset_marks})
                   AND stage IN ({stage_marks})"
            );
            deleted += tx.execute(&sql, params_from_iter(args.iter()))?;
            let err_sql = format!(
                "DELETE FROM stage_error
                 WHERE asset_id IN ({asset_marks})
                   AND stage IN ({stage_marks})"
            );
            tx.execute(&err_sql, params_from_iter(args.iter()))?;
        }
        tx.commit()?;
        Ok(deleted)
    }

    /// Persists a stage failure for `(asset, stage)` with a sanitized short reason, current
    /// timestamp, and incremented attempt count.
    pub fn record_stage_error(
        &self,
        asset: AssetId,
        stage: &str,
        raw_reason: &str,
    ) -> Result<(), CatalogError> {
        let reason = sanitize_stage_error_reason(stage, raw_reason);
        let now = now_unix_secs();
        self.conn.execute(
            "INSERT INTO stage_error (asset_id, stage, reason, last_at_secs, attempts)
             VALUES (?1, ?2, ?3, ?4, 1)
             ON CONFLICT(asset_id, stage) DO UPDATE SET
                 reason = excluded.reason,
                 last_at_secs = excluded.last_at_secs,
                 attempts = stage_error.attempts + 1",
            params![asset.0, stage, reason, now],
        )?;
        Ok(())
    }

    /// Clears any persisted error record for `(asset, stage)`.
    pub fn clear_stage_error(&self, asset: AssetId, stage: &str) -> Result<bool, CatalogError> {
        let changed = self.conn.execute(
            "DELETE FROM stage_error WHERE asset_id = ?1 AND stage = ?2",
            params![asset.0, stage],
        )?;
        Ok(changed > 0)
    }

    /// Clears all persisted error records for `asset` across every stage.
    pub fn clear_stage_errors_for_asset(&self, asset: AssetId) -> Result<usize, CatalogError> {
        let changed = self
            .conn
            .execute("DELETE FROM stage_error WHERE asset_id = ?1", [asset.0])?;
        Ok(changed)
    }

    /// Clears persisted error records for all present assets under `roots`.
    pub fn clear_all_stage_errors_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<usize, CatalogError> {
        if roots.is_empty() {
            return Ok(0);
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "DELETE FROM stage_error
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = stage_error.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))"
        );
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();
        let changed = self.conn.execute(&sql, params_from_iter(args.iter()))?;
        Ok(changed)
    }

    /// Lists persisted stage errors for present assets under `roots`, newest first.
    /// Photos in removed folders are automatically excluded.
    pub fn list_stage_errors_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<StageErrorRecord>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT e.asset_id, MIN(l.path), a.sha256, e.stage, e.reason, e.last_at_secs, e.attempts
             FROM stage_error e
             JOIN asset a ON a.id = e.asset_id
             JOIN location l ON l.asset_id = e.asset_id
             WHERE l.missing = 0 AND l.root IN ({marks})
             GROUP BY e.asset_id, e.stage
             ORDER BY e.last_at_secs DESC, e.asset_id ASC, e.stage ASC"
        );
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            let attempts: i64 = r.get(6)?;
            Ok(StageErrorRecord {
                asset: AssetId(r.get(0)?),
                path: PathBuf::from(r.get::<_, String>(1)?),
                sha256: r.get(2)?,
                stage: r.get(3)?,
                reason: r.get(4)?,
                last_at_secs: r.get(5)?,
                attempts: attempts.max(1) as u32,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Count of distinct present assets under `roots` that have at least one persisted stage error.
    pub fn count_stage_error_assets_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<usize, CatalogError> {
        if roots.is_empty() {
            return Ok(0);
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT COUNT(DISTINCT e.asset_id)
             FROM stage_error e
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))"
        );
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();
        let count: i64 = self
            .conn
            .query_row(&sql, params_from_iter(args.iter()), |r| r.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// Returns all persisted `(asset_id, stage)` error pairs for present assets under `roots`.
    pub fn failed_pairs_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<HashSet<(AssetId, String)>, CatalogError> {
        if roots.is_empty() {
            return Ok(HashSet::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT e.asset_id, e.stage
             FROM stage_error e
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))"
        );
        let args: Vec<Box<dyn ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn ToSql>)
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok((AssetId(r.get(0)?), r.get::<_, String>(1)?))
        })?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Counts present locations (matching `entries()`) that are total, pending, or reviewed
    /// across `stages`, optionally restricted to `roots`.
    /// With `stages` empty, `reviewed == 0` and `pending == all`.
    pub fn reviewed_counts(
        &self,
        stages: &[StageSpec],
        roots: Option<&[PathBuf]>,
    ) -> Result<ReviewedCounts, CatalogError> {
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok(ReviewedCounts::default());
        }
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        let reviewed_expr = if stages.is_empty() {
            String::from("0")
        } else {
            let mut parts = Vec::with_capacity(stages.len());
            for s in stages {
                parts.push(
                    "EXISTS (SELECT 1 FROM analysis an \
                             WHERE an.asset_id = l.asset_id \
                               AND an.stage = ? \
                               AND an.version >= ?)",
                );
                args.push(Box::new(s.stage.clone()));
                args.push(Box::new(i64::from(s.version)));
            }
            format!("CASE WHEN {} THEN 1 ELSE 0 END", parts.join(" AND "))
        };
        let mut sql = format!(
            "SELECT COUNT(*), COALESCE(SUM({reviewed_expr}), 0)
             FROM location l
             WHERE l.missing = 0"
        );
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
        let (all, reviewed): (i64, i64) =
            self.conn
                .query_row(&sql, params_from_iter(args.iter()), |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
        let all = all.max(0) as usize;
        let reviewed = (reviewed.max(0) as usize).min(all);
        Ok(ReviewedCounts {
            all,
            pending: all.saturating_sub(reviewed),
            reviewed,
        })
    }

    /// Returns the set of present `AssetId`s that have completed every stage in `stages`,
    /// optionally restricted to `roots`. Empty if `stages` is empty.
    pub fn reviewed_assets(
        &self,
        stages: &[StageSpec],
        roots: Option<&[PathBuf]>,
    ) -> Result<HashSet<AssetId>, CatalogError> {
        if stages.is_empty() {
            return Ok(HashSet::new());
        }
        if let Some(r) = roots
            && r.is_empty()
        {
            return Ok(HashSet::new());
        }
        let mut sql =
            String::from("SELECT DISTINCT l.asset_id FROM location l WHERE l.missing = 0");
        let mut args: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(r) = roots {
            let marks = vec!["?"; r.len()].join(",");
            sql.push_str(&format!(" AND l.root IN ({marks})"));
            for root in r {
                args.push(Box::new(root.to_string_lossy().into_owned()));
            }
        }
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
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| Ok(AssetId(r.get(0)?)))?;
        let mut out = HashSet::new();
        for row in rows {
            out.insert(row?);
        }
        Ok(out)
    }

    /// Returns `true` iff `stages` is non-empty and `asset` has completed every stage at its version.
    pub fn is_asset_reviewed(
        &self,
        asset: AssetId,
        stages: &[StageSpec],
    ) -> Result<bool, CatalogError> {
        if stages.is_empty() {
            return Ok(false);
        }
        let mut sql = String::from("SELECT 1");
        let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(stages.len() * 3);
        sql.push_str(" WHERE ");
        for (idx, s) in stages.iter().enumerate() {
            if idx > 0 {
                sql.push_str(" AND ");
            }
            sql.push_str(
                "EXISTS (SELECT 1 FROM analysis an \
                         WHERE an.asset_id = ? AND an.stage = ? AND an.version >= ?)",
            );
            args.push(Box::new(asset.0));
            args.push(Box::new(s.stage.clone()));
            args.push(Box::new(i64::from(s.version)));
        }
        let found: Option<i64> = rusqlite::OptionalExtension::optional(self.conn.query_row(
            &sql,
            params_from_iter(args.iter()),
            |r| r.get(0),
        ))?;
        Ok(found.is_some())
    }

    /// Distinct present assets under `roots` that still need at least one stage in `stages`,
    /// ordered by `asset_id`, up to `limit`.
    pub fn pending_any_stage_in_roots(
        &self,
        stages: &[StageSpec],
        roots: &[PathBuf],
        limit: usize,
    ) -> Result<Vec<Pending>, CatalogError> {
        if stages.is_empty() || roots.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let mut sql = format!(
            "SELECT l.asset_id, MIN(l.path)
             FROM location l
             WHERE l.missing = 0
               AND l.root IN ({marks})
               AND ("
        );
        let mut args: Vec<Box<dyn ToSql>> = Vec::with_capacity(roots.len() + stages.len() * 2 + 1);
        for root in roots {
            args.push(Box::new(root.to_string_lossy().into_owned()));
        }
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
        sql.push_str(") GROUP BY l.asset_id ORDER BY l.asset_id LIMIT ?");
        args.push(Box::new(i64::try_from(limit).unwrap_or(i64::MAX)));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok(Pending {
                asset: AssetId(r.get(0)?),
                path: PathBuf::from(r.get::<_, String>(1)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
