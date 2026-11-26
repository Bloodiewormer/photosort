use crate::{AssetId, Catalog, CatalogError, Entry};
use std::cmp::Reverse;
use std::path::PathBuf;

/// Exact duplicates sharing identical bytes. Shown for review only: nothing is ever deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub sha256: String,
    pub size: u64,
    pub entries: Vec<Entry>,
}

impl DuplicateGroup {
    fn wasted_bytes_u128(&self) -> u128 {
        u128::from(self.size) * u128::from(self.entries.len().saturating_sub(1) as u64)
    }
}

impl Catalog {
    /// Non-missing locations sharing the same asset (2+ locations per group), ordered by wasted
    /// bytes (`size * (n - 1)`) descending then `sha256` ascending, with entries ordered by path.
    pub fn duplicate_groups(&self) -> Result<Vec<DuplicateGroup>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT l.asset_id, l.path, a.sha256, a.size, l.size, l.mtime_ns
             FROM location l
             JOIN asset a ON a.id = l.asset_id
             WHERE l.missing = 0 AND l.asset_id IN (
                 SELECT asset_id FROM location WHERE missing = 0
                 GROUP BY asset_id HAVING COUNT(*) > 1
             )
             ORDER BY l.asset_id ASC, l.path ASC",
        )?;

        let rows = stmt.query_map([], |r| {
            let asset_id: i64 = r.get(0)?;
            let path = PathBuf::from(r.get::<_, String>(1)?);
            let sha256: String = r.get(2)?;
            let asset_size = r.get::<_, i64>(3)?.max(0) as u64;
            let loc_size = r.get::<_, i64>(4)?.max(0) as u64;
            let modified_ns: Option<i64> = r.get(5)?;
            Ok((
                asset_id,
                asset_size,
                Entry {
                    asset: AssetId(asset_id),
                    path,
                    sha256,
                    size: loc_size,
                    modified_ns,
                },
            ))
        })?;

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
}
