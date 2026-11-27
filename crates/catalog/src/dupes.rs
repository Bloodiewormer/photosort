use crate::{AssetId, Catalog, CatalogError, EMBEDDING_BYTES, EMBEDDING_DIM, Entry};
use rusqlite::{OptionalExtension, params};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread;

pub const DEFAULT_NEAR_DUPLICATE_THRESHOLD: f32 = 0.95;

const META_NEAR_DUP_THRESHOLD: &str = "near_duplicates_threshold";
const META_NEAR_DUP_MODEL: &str = "near_duplicates_model";
const META_NEAR_DUP_ASSET_COUNT: &str = "near_duplicates_asset_count";

/// Exact duplicates sharing identical bytes. Shown for review only: nothing is ever deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub sha256: String,
    pub size: u64,
    pub entries: Vec<Entry>,
}

/// Visually similar distinct assets grouped by cosine similarity over persisted SigLIP 2 embeddings.
/// Shown for review only: nothing is ever deleted.
#[derive(Debug, Clone, PartialEq)]
pub struct NearDuplicateGroup {
    pub id: i64,
    pub similarity: f32,
    pub entries: Vec<Entry>,
}

/// Summary of the last saved near-duplicate scan in `catalog_meta`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearDuplicateMeta {
    pub threshold: f32,
    pub asset_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NearDuplicateProgress {
    pub done_pairs: u64,
    pub total_pairs: u64,
}

impl DuplicateGroup {
    fn wasted_bytes_u128(&self) -> u128 {
        u128::from(self.size) * u128::from(self.entries.len().saturating_sub(1) as u64)
    }
}

const DUPLICATE_GROUPS_SQL: &str =
    "SELECT l.asset_id, l.path, a.sha256, a.size, l.size, l.mtime_ns,
            a.width, a.height, a.format, a.captured_at_ns
     FROM location l
     JOIN asset a ON a.id = l.asset_id
     WHERE l.missing = 0 AND l.asset_id IN (
         SELECT asset_id FROM location WHERE missing = 0
         GROUP BY asset_id HAVING COUNT(*) > 1
     )
     ORDER BY l.asset_id ASC, l.path ASC";

fn dup_row_from_sql(r: &rusqlite::Row<'_>) -> rusqlite::Result<(i64, u64, Entry)> {
    let asset_id: i64 = r.get(0)?;
    let path = PathBuf::from(r.get::<_, String>(1)?);
    let sha256: String = r.get(2)?;
    let asset_size = r.get::<_, i64>(3)?.max(0) as u64;
    let loc_size = r.get::<_, i64>(4)?.max(0) as u64;
    let modified_ns: Option<i64> = r.get(5)?;
    let width: Option<i64> = r.get(6)?;
    let height: Option<i64> = r.get(7)?;
    let format: Option<String> = r.get(8)?;
    let captured_at_ns: Option<i64> = r.get(9)?;
    Ok((
        asset_id,
        asset_size,
        Entry {
            asset: AssetId(asset_id),
            path,
            sha256,
            size: loc_size,
            modified_ns,
            width: width.and_then(|v| u32::try_from(v).ok()),
            height: height.and_then(|v| u32::try_from(v).ok()),
            format,
            captured_at_ns,
        },
    ))
}

impl Catalog {
    pub fn explain_duplicate_groups(&self) -> Result<Vec<String>, CatalogError> {
        self.explain_sql(DUPLICATE_GROUPS_SQL, [])
    }

    /// Non-missing locations sharing the same asset (2+ locations per group), ordered by wasted
    /// bytes (`size * (n - 1)`) descending then `sha256` ascending, with entries ordered by path.
    pub fn duplicate_groups(&self) -> Result<Vec<DuplicateGroup>, CatalogError> {
        let mut stmt = self.conn.prepare(DUPLICATE_GROUPS_SQL)?;
        let rows = stmt.query_map([], dup_row_from_sql)?;
        group_exact_rows(rows)
    }

    /// Duplicate groups restricted to non-missing locations under `roots`.
    pub fn duplicate_groups_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<DuplicateGroup>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT l.asset_id, l.path, a.sha256, a.size, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns
             FROM location l
             JOIN asset a ON a.id = l.asset_id
             WHERE l.missing = 0 AND l.root IN ({marks}) AND l.asset_id IN (
                 SELECT asset_id FROM location WHERE missing = 0 AND root IN ({marks})
                 GROUP BY asset_id HAVING COUNT(*) > 1
             )
             ORDER BY l.asset_id ASC, l.path ASC"
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() * 2);
        for _ in 0..2 {
            for r in roots {
                args.push(Box::new(r.to_string_lossy().into_owned()));
            }
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), dup_row_from_sql)?;
        group_exact_rows(rows)
    }

    /// Loads all persisted embeddings for `model` belonging to distinct present assets under `roots`.
    pub fn load_embeddings_in_roots(
        &self,
        model: &str,
        roots: &[PathBuf],
    ) -> Result<Vec<(AssetId, [f32; EMBEDDING_DIM])>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT e.asset_id, e.vector
             FROM asset_embedding e
             WHERE e.model = ?
               AND EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = e.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))
             ORDER BY e.asset_id ASC"
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() + 1);
        args.push(Box::new(model.to_string()));
        for r in roots {
            args.push(Box::new(r.to_string_lossy().into_owned()));
        }
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let asset_id = AssetId(row.get(0)?);
            let blob = row.get_ref(1)?.as_blob().map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Blob,
                    Box::new(e),
                )
            })?;
            if blob.len() != EMBEDDING_BYTES {
                continue;
            }
            let vec = crate::decode_embedding_array(blob)?;
            out.push((asset_id, vec));
        }
        Ok(out)
    }

    /// Replaces the cached near-duplicate groups in SQLite and records the scan parameters.
    pub fn save_near_duplicate_groups(
        &mut self,
        model: &str,
        threshold: f32,
        asset_count: usize,
        groups: &[(f32, Vec<AssetId>)],
    ) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM near_duplicate_member", [])?;
        tx.execute("DELETE FROM near_duplicate_group", [])?;
        {
            let mut ins_group =
                tx.prepare_cached("INSERT INTO near_duplicate_group (similarity) VALUES (?1)")?;
            let mut ins_member = tx.prepare_cached(
                "INSERT OR IGNORE INTO near_duplicate_member (group_id, asset_id) VALUES (?1, ?2)",
            )?;
            for (similarity, members) in groups {
                if members.len() < 2 {
                    continue;
                }
                ins_group.execute([f64::from(*similarity)])?;
                let group_id = tx.last_insert_rowid();
                for &asset in members {
                    ins_member.execute(params![group_id, asset.0])?;
                }
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, ?2)",
            params![META_NEAR_DUP_THRESHOLD, format!("{threshold:.4}")],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, ?2)",
            params![META_NEAR_DUP_MODEL, model],
        )?;
        tx.execute(
            "INSERT OR REPLACE INTO catalog_meta (key, value) VALUES (?1, ?2)",
            params![META_NEAR_DUP_ASSET_COUNT, asset_count.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Metadata of the last completed near-duplicate scan, if any.
    pub fn near_duplicate_meta(&self) -> Result<Option<NearDuplicateMeta>, CatalogError> {
        let threshold_str: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM catalog_meta WHERE key = ?1",
                [META_NEAR_DUP_THRESHOLD],
                |r| r.get(0),
            )
            .optional()?;
        let Some(t_str) = threshold_str else {
            return Ok(None);
        };
        let threshold: f32 = t_str.parse().unwrap_or(DEFAULT_NEAR_DUPLICATE_THRESHOLD);
        let asset_count: usize = self
            .conn
            .query_row(
                "SELECT value FROM catalog_meta WHERE key = ?1",
                [META_NEAR_DUP_ASSET_COUNT],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(Some(NearDuplicateMeta {
            threshold,
            asset_count,
        }))
    }

    /// Loads cached near-duplicate groups restricted to present assets under `roots`.
    /// Groups that shrink below 2 distinct assets after filtering by `roots` are omitted.
    pub fn near_duplicate_groups_in_roots(
        &self,
        roots: &[PathBuf],
    ) -> Result<Vec<NearDuplicateGroup>, CatalogError> {
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        let marks = vec!["?"; roots.len()].join(",");
        let sql = format!(
            "SELECT g.id, g.similarity, m.asset_id
             FROM near_duplicate_group g
             JOIN near_duplicate_member m ON m.group_id = g.id
             WHERE EXISTS (SELECT 1 FROM location l
                           WHERE l.asset_id = m.asset_id
                             AND l.missing = 0
                             AND l.root IN ({marks}))
             ORDER BY g.similarity DESC, g.id ASC, m.asset_id ASC"
        );
        let args: Vec<Box<dyn rusqlite::ToSql>> = roots
            .iter()
            .map(|r| Box::new(r.to_string_lossy().into_owned()) as Box<dyn rusqlite::ToSql>)
            .collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let raw_rows: Vec<(i64, f32, AssetId)> = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)? as f32,
                    AssetId(r.get::<_, i64>(2)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        drop(stmt);

        // Group asset IDs by group_id and keep only groups with >= 2 distinct assets in roots.
        let mut grouped: Vec<(i64, f32, Vec<AssetId>)> = Vec::new();
        for (gid, sim, asset) in raw_rows {
            match grouped.last_mut() {
                Some((last_gid, _, assets)) if *last_gid == gid => assets.push(asset),
                _ => grouped.push((gid, sim, vec![asset])),
            }
        }
        grouped.retain(|(_, _, assets)| assets.len() >= 2);
        if grouped.is_empty() {
            return Ok(Vec::new());
        }

        // Hydrate one representative Entry per asset_id within roots.
        let entry_sql = format!(
            "SELECT l.asset_id, l.path, a.sha256, l.size, l.mtime_ns,
                    a.width, a.height, a.format, a.captured_at_ns
             FROM location l JOIN asset a ON a.id = l.asset_id
             WHERE l.asset_id = ? AND l.missing = 0 AND l.root IN ({marks})
             ORDER BY l.path ASC
             LIMIT 1"
        );
        let mut entry_stmt = self.conn.prepare(&entry_sql)?;
        let mut cache: HashMap<AssetId, Entry> = HashMap::new();
        let mut out = Vec::with_capacity(grouped.len());
        for (gid, sim, assets) in grouped {
            let mut entries = Vec::with_capacity(assets.len());
            for asset in assets {
                if let Some(entry) = cache.get(&asset) {
                    entries.push(entry.clone());
                    continue;
                }
                let mut e_args: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(roots.len() + 1);
                e_args.push(Box::new(asset.0));
                for r in roots {
                    e_args.push(Box::new(r.to_string_lossy().into_owned()));
                }
                if let Some(entry) = entry_stmt
                    .query_row(
                        rusqlite::params_from_iter(e_args.iter()),
                        crate::browse::entry_from_row,
                    )
                    .optional()?
                {
                    cache.insert(asset, entry.clone());
                    entries.push(entry);
                }
            }
            if entries.len() >= 2 {
                entries.sort_by(|a, b| a.path.cmp(&b.path));
                out.push(NearDuplicateGroup {
                    id: gid,
                    similarity: sim,
                    entries,
                });
            }
        }
        Ok(out)
    }

    /// Computes near-duplicate groups over all persisted `model` embeddings in `roots`, saves them
    /// in the catalog, and returns the hydrated groups. Returns `Ok(None)` if cancelled.
    pub fn compute_and_save_near_duplicates_in_roots(
        &mut self,
        model: &str,
        roots: &[PathBuf],
        threshold: f32,
        cancel: &AtomicBool,
        on_progress: &(dyn Fn(NearDuplicateProgress) + Sync),
    ) -> Result<Option<Vec<NearDuplicateGroup>>, CatalogError> {
        let items = self.load_embeddings_in_roots(model, roots)?;
        let asset_count = items.len();
        let Some(groups) = compute_near_duplicate_groups(&items, threshold, cancel, on_progress)
        else {
            return Ok(None);
        };
        self.save_near_duplicate_groups(model, threshold, asset_count, &groups)?;
        let hydrated = self.near_duplicate_groups_in_roots(roots)?;
        Ok(Some(hydrated))
    }
}

fn group_exact_rows(
    rows: impl Iterator<Item = rusqlite::Result<(i64, u64, Entry)>>,
) -> Result<Vec<DuplicateGroup>, CatalogError> {
    let mut grouped: Vec<(i64, DuplicateGroup)> = Vec::new();
    for row in rows {
        let (asset_id, asset_size, entry) = row?;
        match grouped.last_mut() {
            Some((last_id, group)) if *last_id == asset_id => group.entries.push(entry),
            _ => {
                let group = DuplicateGroup {
                    sha256: entry.sha256.clone(),
                    size: asset_size,
                    entries: vec![entry],
                };
                grouped.push((asset_id, group));
            }
        }
    }

    let mut groups: Vec<DuplicateGroup> = grouped.into_iter().map(|(_, g)| g).collect();
    groups.sort_by(|a, b| {
        Reverse(a.wasted_bytes_u128())
            .cmp(&Reverse(b.wasted_bytes_u128()))
            .then_with(|| a.sha256.cmp(&b.sha256))
    });
    Ok(groups)
}

/// Computes connected components of distinct L2-normalized embeddings whose pairwise cosine
/// similarity is `>= threshold`, using `std::thread::scope` with cache-blocked tiling.
/// Returns `None` if `cancel` is triggered before completion.
pub fn compute_near_duplicate_groups(
    items: &[(AssetId, [f32; EMBEDDING_DIM])],
    threshold: f32,
    cancel: &AtomicBool,
    on_progress: &(dyn Fn(NearDuplicateProgress) + Sync),
) -> Option<Vec<(f32, Vec<AssetId>)>> {
    let n = items.len();
    let total_pairs = (n as u64).saturating_mul(n.saturating_sub(1) as u64) / 2;
    on_progress(NearDuplicateProgress {
        done_pairs: 0,
        total_pairs,
    });
    if n < 2 {
        return Some(Vec::new());
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    const BLOCK: usize = 64;
    let num_threads = thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .clamp(1, 16)
        .min(n.div_ceil(BLOCK).max(1));

    let next_row = AtomicUsize::new(0);
    let done_pairs = AtomicU64::new(0);

    let per_thread_edges: Vec<Vec<(u32, u32, f32)>> = thread::scope(|s| {
        let mut handles = Vec::with_capacity(num_threads);
        for _ in 0..num_threads {
            let next_row = &next_row;
            let done_pairs = &done_pairs;
            handles.push(s.spawn(move || {
                let mut edges: Vec<(u32, u32, f32)> = Vec::new();
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let i0 = next_row.fetch_add(BLOCK, Ordering::Relaxed);
                    if i0 >= n {
                        break;
                    }
                    let i_end = (i0 + BLOCK).min(n);
                    let mut block_pairs = 0u64;

                    // 1. Diagonal triangle: i0 <= i < j < i_end
                    for i in i0..i_end {
                        let vi = &items[i].1;
                        for (offset, item_j) in items[(i + 1)..i_end].iter().enumerate() {
                            let sim = dot_f32(vi, &item_j.1);
                            if sim >= threshold {
                                edges.push((i as u32, (i + 1 + offset) as u32, sim));
                            }
                        }
                        block_pairs += (i_end - i - 1) as u64;
                    }

                    // 2. Off-diagonal tiles: i in i0..i_end, j in j0..j_end (j0 >= i_end)
                    let mut j0 = i_end;
                    while j0 < n {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let j_end = (j0 + BLOCK).min(n);

                        // Process 4 rows at a time so each column vector `vj` is loaded once per 4 rows.
                        let mut i = i0;
                        while i + 4 <= i_end {
                            let v0 = &items[i].1;
                            let v1 = &items[i + 1].1;
                            let v2 = &items[i + 2].1;
                            let v3 = &items[i + 3].1;
                            for (offset, item_j) in items[j0..j_end].iter().enumerate() {
                                let j = j0 + offset;
                                let sims = dot4_f32(v0, v1, v2, v3, &item_j.1);
                                if sims[0] >= threshold {
                                    edges.push((i as u32, j as u32, sims[0]));
                                }
                                if sims[1] >= threshold {
                                    edges.push(((i + 1) as u32, j as u32, sims[1]));
                                }
                                if sims[2] >= threshold {
                                    edges.push(((i + 2) as u32, j as u32, sims[2]));
                                }
                                if sims[3] >= threshold {
                                    edges.push(((i + 3) as u32, j as u32, sims[3]));
                                }
                            }
                            i += 4;
                        }
                        while i < i_end {
                            let vi = &items[i].1;
                            for (offset, item_j) in items[j0..j_end].iter().enumerate() {
                                let sim = dot_f32(vi, &item_j.1);
                                if sim >= threshold {
                                    edges.push((i as u32, (j0 + offset) as u32, sim));
                                }
                            }
                            i += 1;
                        }

                        block_pairs += ((i_end - i0) * (j_end - j0)) as u64;
                        j0 = j_end;
                    }

                    let done = done_pairs.fetch_add(block_pairs, Ordering::Relaxed) + block_pairs;
                    on_progress(NearDuplicateProgress {
                        done_pairs: done.min(total_pairs),
                        total_pairs,
                    });
                }
                edges
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    if cancel.load(Ordering::Relaxed) {
        return None;
    }

    // Union-Find connected components over matching edges.
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut rank: Vec<u8> = vec![0; n];
    let mut all_edges_count = 0usize;
    for chunk in &per_thread_edges {
        all_edges_count += chunk.len();
        for &(u, v, _) in chunk {
            uf_union(&mut parent, &mut rank, u, v);
        }
    }
    if all_edges_count == 0 {
        on_progress(NearDuplicateProgress {
            done_pairs: total_pairs,
            total_pairs,
        });
        return Some(Vec::new());
    }

    // Track minimum edge similarity per component and collect members.
    let mut comp_min_sim: HashMap<u32, f32> = HashMap::new();
    for chunk in &per_thread_edges {
        for &(u, _, sim) in chunk {
            let root = uf_find(&mut parent, u);
            comp_min_sim
                .entry(root)
                .and_modify(|s| *s = s.min(sim))
                .or_insert(sim);
        }
    }

    let mut comp_members: HashMap<u32, Vec<AssetId>> = HashMap::with_capacity(comp_min_sim.len());
    for i in 0..n as u32 {
        let root = uf_find(&mut parent, i);
        if comp_min_sim.contains_key(&root) {
            comp_members
                .entry(root)
                .or_default()
                .push(items[i as usize].0);
        }
    }

    let mut groups: Vec<(f32, Vec<AssetId>)> = comp_members
        .into_iter()
        .filter_map(|(root, mut members)| {
            if members.len() < 2 {
                return None;
            }
            members.sort();
            let sim = comp_min_sim
                .get(&root)
                .copied()
                .unwrap_or(threshold)
                .clamp(0.0, 1.0);
            Some((sim, members))
        })
        .collect();

    groups.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.first().cmp(&b.1.first()))
    });

    on_progress(NearDuplicateProgress {
        done_pairs: total_pairs,
        total_pairs,
    });
    Some(groups)
}

fn uf_find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        let p = parent[x as usize];
        parent[x as usize] = parent[p as usize];
        x = p;
    }
    x
}

fn uf_union(parent: &mut [u32], rank: &mut [u8], a: u32, b: u32) {
    let ra = uf_find(parent, a);
    let rb = uf_find(parent, b);
    if ra == rb {
        return;
    }
    match rank[ra as usize].cmp(&rank[rb as usize]) {
        std::cmp::Ordering::Less => parent[ra as usize] = rb,
        std::cmp::Ordering::Greater => parent[rb as usize] = ra,
        std::cmp::Ordering::Equal => {
            parent[rb as usize] = ra;
            rank[ra as usize] = rank[ra as usize].saturating_add(1);
        }
    }
}

#[inline]
fn dot_f32(a: &[f32; EMBEDDING_DIM], b: &[f32; EMBEDDING_DIM]) -> f32 {
    let mut s0 = 0.0f32;
    let mut s1 = 0.0f32;
    let mut s2 = 0.0f32;
    let mut s3 = 0.0f32;
    let mut i = 0;
    while i < EMBEDDING_DIM {
        s0 += a[i] * b[i];
        s1 += a[i + 1] * b[i + 1];
        s2 += a[i + 2] * b[i + 2];
        s3 += a[i + 3] * b[i + 3];
        i += 4;
    }
    (s0 + s1) + (s2 + s3)
}

#[inline]
fn dot4_f32(
    a0: &[f32; EMBEDDING_DIM],
    a1: &[f32; EMBEDDING_DIM],
    a2: &[f32; EMBEDDING_DIM],
    a3: &[f32; EMBEDDING_DIM],
    b: &[f32; EMBEDDING_DIM],
) -> [f32; 4] {
    let mut s0 = [0.0f32; 8];
    let mut s1 = [0.0f32; 8];
    let mut s2 = [0.0f32; 8];
    let mut s3 = [0.0f32; 8];
    let mut i = 0;
    while i < EMBEDDING_DIM {
        for lane in 0..8 {
            let bv = b[i + lane];
            s0[lane] += a0[i + lane] * bv;
            s1[lane] += a1[i + lane] * bv;
            s2[lane] += a2[i + lane] * bv;
            s3[lane] += a3[i + lane] * bv;
        }
        i += 8;
    }
    [
        s0.iter().sum(),
        s1.iter().sum(),
        s2.iter().sum(),
        s3.iter().sum(),
    ]
}
