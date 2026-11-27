//! People and incremental face grouping.
//!
//! Faces detected by YuNet are incrementally assigned to persons using SFace's 128-dimensional
//! normalized embeddings and cosine similarity (threshold 0.363). Unassigned faces create new
//! unnamed persons. User-assigned faces are permanent and never overridden by analysis.

use crate::{AssetId, Catalog, CatalogError};
use rusqlite::{OptionalExtension, params};

pub const SFACE_SIMILARITY_THRESHOLD: f32 = 0.363;
/// Maximum number of highest-score face embeddings averaged to build a person's centroid.
const MAX_CENTROID_FACES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PersonId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub id: PersonId,
    pub name: Option<String>,
    pub hidden: bool,
    pub ignored: bool,
    /// Number of distinct images that contain this person.
    pub photo_count: usize,
    /// ID of a representative face for showing a thumbnail/crop.
    pub cover_face_id: Option<i64>,
    pub cover_asset_id: Option<AssetId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FaceDetail {
    pub id: i64,
    pub asset_id: AssetId,
    pub face_idx: usize,
    pub bbox: [f32; 4],
    pub score: f32,
    pub landmarks: [[f32; 2]; 5],
    pub person_id: Option<PersonId>,
    pub person_name: Option<String>,
    pub user_assigned: bool,
    pub similarity: Option<f32>,
    pub has_embedding: bool,
}

/// Encodes a 128-dimensional f32 embedding into a 512-byte Little-Endian blob.
pub fn encode_face_embedding(emb: &[f32; 128]) -> [u8; 512] {
    let mut bytes = [0u8; 512];
    for (i, &val) in emb.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }
    bytes
}

/// Decodes a 512-byte Little-Endian blob into a 128-dimensional f32 embedding.
pub fn decode_face_embedding(blob: &[u8]) -> Option<[f32; 128]> {
    if blob.len() != 512 {
        return None;
    }
    let mut out = [0.0f32; 128];
    for (i, item) in out.iter_mut().enumerate() {
        let chunk = &blob[i * 4..i * 4 + 4];
        *item = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    Some(out)
}

/// Cosine similarity between two L2-normalized 128-d vectors.
fn l2_normalize_128(v: &mut [f32; 128]) {
    let norm_sq: f32 = v.iter().map(|x| x * x).sum();
    if norm_sq > 0.0 {
        let inv = 1.0 / norm_sq.sqrt();
        for x in v.iter_mut() {
            *x *= inv;
        }
    }
}

pub fn cosine_similarity_128(a: &[f32; 128], b: &[f32; 128]) -> f32 {
    let mut dot = 0.0f32;
    for i in 0..128 {
        dot += a[i] * b[i];
    }
    dot.clamp(-1.0, 1.0)
}

/// Recomputes and stores the L2-normalized centroid for a single person using up to
/// [`MAX_CENTROID_FACES`] highest-score embeddings, keeping grouping O(persons) instead of O(faces).
fn recompute_person_centroid(
    conn: &rusqlite::Connection,
    person_id: i64,
) -> Result<(), CatalogError> {
    let mut stmt = conn.prepare_cached(
        "SELECT embedding FROM asset_face
         WHERE person_id = ?1 AND embedding IS NOT NULL
         ORDER BY score DESC, id ASC
         LIMIT ?2",
    )?;
    let mut centroid = [0.0f32; 128];
    let mut count = 0usize;
    let rows = stmt.query_map(params![person_id, MAX_CENTROID_FACES as i64], |r| {
        r.get::<_, Vec<u8>>(0)
    })?;
    for row in rows {
        if let Some(arr) = decode_face_embedding(&row?) {
            for (c, &v) in centroid.iter_mut().zip(&arr) {
                *c += v;
            }
            count += 1;
        }
    }
    drop(stmt);
    if count == 0 {
        conn.execute(
            "UPDATE person SET centroid = NULL WHERE id = ?1",
            [person_id],
        )?;
    } else {
        l2_normalize_128(&mut centroid);
        let blob = encode_face_embedding(&centroid);
        conn.execute(
            "UPDATE person SET centroid = ?1 WHERE id = ?2",
            params![blob.as_slice(), person_id],
        )?;
    }
    Ok(())
}

impl Catalog {
    /// Lists all persons in the catalog, ordered by photo count descending, then name/id.
    pub fn list_persons(&self, include_hidden: bool) -> Result<Vec<Person>, CatalogError> {
        let sql = if include_hidden {
            "SELECT p.id, p.name, p.hidden, p.ignored,
                    COUNT(DISTINCT af.asset_id) as photo_count,
                    (SELECT af2.id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_face_id,
                    (SELECT af2.asset_id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_asset_id
             FROM person p
             LEFT JOIN asset_face af ON af.person_id = p.id
             GROUP BY p.id
             ORDER BY photo_count DESC, p.name ASC, p.id ASC"
        } else {
            "SELECT p.id, p.name, p.hidden, p.ignored,
                    COUNT(DISTINCT af.asset_id) as photo_count,
                    (SELECT af2.id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_face_id,
                    (SELECT af2.asset_id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_asset_id
             FROM person p
             LEFT JOIN asset_face af ON af.person_id = p.id
             WHERE p.hidden = 0
             GROUP BY p.id
             ORDER BY photo_count DESC, p.name ASC, p.id ASC"
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map([], |r| {
            Ok(Person {
                id: PersonId(r.get(0)?),
                name: r.get(1)?,
                hidden: r.get::<_, i64>(2)? == 1,
                ignored: r.get::<_, i64>(3)? == 1,
                photo_count: r.get::<_, i64>(4)? as usize,
                cover_face_id: r.get(5)?,
                cover_asset_id: r.get::<_, Option<i64>>(6)?.map(AssetId),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns a single person by ID.
    pub fn get_person(&self, id: PersonId) -> Result<Option<Person>, CatalogError> {
        let row = self
            .conn
            .query_row(
                "SELECT p.id, p.name, p.hidden, p.ignored,
                        COUNT(DISTINCT af.asset_id) as photo_count,
                        (SELECT af2.id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_face_id,
                        (SELECT af2.asset_id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_asset_id
                 FROM person p
                 LEFT JOIN asset_face af ON af.person_id = p.id
                 WHERE p.id = ?1
                 GROUP BY p.id",
                [id.0],
                |r| {
                    Ok(Person {
                        id: PersonId(r.get(0)?),
                        name: r.get(1)?,
                        hidden: r.get::<_, i64>(2)? == 1,
                        ignored: r.get::<_, i64>(3)? == 1,
                        photo_count: r.get::<_, i64>(4)? as usize,
                        cover_face_id: r.get(5)?,
                        cover_asset_id: r.get::<_, Option<i64>>(6)?.map(AssetId),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Finds an existing person with the exact name (case-insensitive).
    pub fn find_person_by_name(&self, name: &str) -> Result<Option<Person>, CatalogError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        let row = self
            .conn
            .query_row(
                "SELECT p.id, p.name, p.hidden, p.ignored,
                        COUNT(DISTINCT af.asset_id) as photo_count,
                        (SELECT af2.id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_face_id,
                        (SELECT af2.asset_id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_asset_id
                 FROM person p
                 LEFT JOIN asset_face af ON af.person_id = p.id
                 WHERE lower(trim(p.name)) = lower(?1)
                 GROUP BY p.id",
                [trimmed],
                |r| {
                    Ok(Person {
                        id: PersonId(r.get(0)?),
                        name: r.get(1)?,
                        hidden: r.get::<_, i64>(2)? == 1,
                        ignored: r.get::<_, i64>(3)? == 1,
                        photo_count: r.get::<_, i64>(4)? as usize,
                        cover_face_id: r.get(5)?,
                        cover_asset_id: r.get::<_, Option<i64>>(6)?.map(AssetId),
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    /// Creates a new person. Returns the new PersonId.
    pub fn create_person(&mut self, name: Option<&str>) -> Result<PersonId, CatalogError> {
        let trimmed = name.map(|s| s.trim()).filter(|s| !s.is_empty());
        self.conn.execute(
            "INSERT INTO person (name, hidden, ignored) VALUES (?1, 0, 0)",
            params![trimmed],
        )?;
        Ok(PersonId(self.conn.last_insert_rowid()))
    }

    /// Renames a person. If `new_name` is empty or None, clears the name.
    pub fn rename_person(
        &mut self,
        id: PersonId,
        new_name: Option<&str>,
    ) -> Result<(), CatalogError> {
        let trimmed = new_name.map(|s| s.trim()).filter(|s| !s.is_empty());
        self.conn.execute(
            "UPDATE person SET name = ?1 WHERE id = ?2",
            params![trimmed, id.0],
        )?;
        Ok(())
    }

    /// Marks a person as hidden (or unhidden).
    pub fn set_person_hidden(&mut self, id: PersonId, hidden: bool) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE person SET hidden = ?1 WHERE id = ?2",
            params![if hidden { 1 } else { 0 }, id.0],
        )?;
        Ok(())
    }

    /// Marks a person as ignored (or unignored).
    pub fn set_person_ignored(&mut self, id: PersonId, ignored: bool) -> Result<(), CatalogError> {
        self.conn.execute(
            "UPDATE person SET ignored = ?1 WHERE id = ?2",
            params![if ignored { 1 } else { 0 }, id.0],
        )?;
        Ok(())
    }

    /// Merges `source` person into `target` person.
    ///
    /// Reassigns all faces of `source` to `target`. If `target` was unnamed and `source` had a name,
    /// preserves `source`'s name on `target`. Then deletes `source`.
    pub fn merge_persons(
        &mut self,
        source: PersonId,
        target: PersonId,
    ) -> Result<(), CatalogError> {
        if source == target {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        let source_name: Option<String> = tx
            .query_row("SELECT name FROM person WHERE id = ?1", [source.0], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();
        let target_name: Option<String> = tx
            .query_row("SELECT name FROM person WHERE id = ?1", [target.0], |r| {
                r.get::<_, Option<String>>(0)
            })
            .optional()?
            .flatten();

        if target_name.as_deref().unwrap_or("").trim().is_empty()
            && let Some(sname) = source_name
            && !sname.trim().is_empty()
        {
            tx.execute(
                "UPDATE person SET name = ?1 WHERE id = ?2",
                params![sname.trim(), target.0],
            )?;
        }

        tx.execute(
            "UPDATE asset_face SET person_id = ?1, user_assigned = 1 WHERE person_id = ?2",
            params![target.0, source.0],
        )?;
        tx.execute("DELETE FROM person WHERE id = ?1", [source.0])?;
        recompute_person_centroid(&tx, target.0)?;
        tx.commit()?;
        Ok(())
    }

    /// Removes an asset from a person: clears `person_id` on all faces of that asset that belong to
    /// `person`, and marks them `user_assigned = 1` so future grouping does not re-add them.
    pub fn remove_asset_from_person(
        &mut self,
        asset: AssetId,
        person: PersonId,
    ) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE asset_face SET person_id = NULL, user_assigned = 1, similarity = NULL
             WHERE asset_id = ?1 AND person_id = ?2",
            params![asset.0, person.0],
        )?;
        recompute_person_centroid(&tx, person.0)?;
        tx.commit()?;
        Ok(())
    }

    /// Assigns a specific face to a person. If `user_assigned` is true, future analysis will not
    /// reassign it.
    pub fn assign_face_to_person(
        &mut self,
        face_id: i64,
        person: PersonId,
        user_assigned: bool,
    ) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        let prev_pid: Option<i64> = tx
            .query_row(
                "SELECT person_id FROM asset_face WHERE id = ?1",
                [face_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        tx.execute(
            "UPDATE asset_face SET person_id = ?1, user_assigned = ?2 WHERE id = ?3",
            params![person.0, if user_assigned { 1 } else { 0 }, face_id],
        )?;
        if let Some(old_pid) = prev_pid
            && old_pid != person.0
        {
            recompute_person_centroid(&tx, old_pid)?;
        }
        recompute_person_centroid(&tx, person.0)?;
        tx.commit()?;
        Ok(())
    }

    /// Returns unnamed, non-ignored, non-hidden persons that appear in `>= min_photos` distinct photos.
    pub fn unnamed_persons_with_min_photos(
        &self,
        min_photos: usize,
    ) -> Result<Vec<Person>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, p.name, p.hidden, p.ignored, COUNT(DISTINCT af.asset_id) as photo_count,
                    (SELECT af2.id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_face_id,
                    (SELECT af2.asset_id FROM asset_face af2 WHERE af2.person_id = p.id ORDER BY af2.score DESC LIMIT 1) as cover_asset_id
             FROM person p
             JOIN asset_face af ON af.person_id = p.id
             WHERE (p.name IS NULL OR trim(p.name) = '')
               AND p.ignored = 0
               AND p.hidden = 0
             GROUP BY p.id
             HAVING photo_count >= ?1
             ORDER BY photo_count DESC, p.id ASC",
        )?;
        let rows = stmt.query_map([min_photos as i64], |r| {
            Ok(Person {
                id: PersonId(r.get(0)?),
                name: r.get(1)?,
                hidden: r.get::<_, i64>(2)? == 1,
                ignored: r.get::<_, i64>(3)? == 1,
                photo_count: r.get::<_, i64>(4)? as usize,
                cover_face_id: r.get(5)?,
                cover_asset_id: r.get::<_, Option<i64>>(6)?.map(AssetId),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns face details for all faces in `asset`, with person name and user_assigned status.
    pub fn asset_face_details(&self, asset: AssetId) -> Result<Vec<FaceDetail>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT af.id, af.asset_id, af.face_idx, af.x1, af.y1, af.x2, af.y2, af.score,
                    af.landmarks, af.person_id, p.name, af.user_assigned, af.similarity,
                    af.embedding IS NOT NULL
             FROM asset_face af
             LEFT JOIN person p ON p.id = af.person_id
             WHERE af.asset_id = ?1
             ORDER BY af.face_idx ASC",
        )?;
        let rows = stmt.query_map([asset.0], |r| {
            let lm_str: Option<String> = r.get(8)?;
            let landmarks = lm_str
                .as_deref()
                .and_then(crate::vision::decode_landmarks)
                .unwrap_or([[0.0, 0.0]; 5]);
            let person_id: Option<i64> = r.get(9)?;
            Ok(FaceDetail {
                id: r.get(0)?,
                asset_id: AssetId(r.get(1)?),
                face_idx: r.get::<_, i64>(2)? as usize,
                bbox: [
                    r.get::<_, f64>(3)? as f32,
                    r.get::<_, f64>(4)? as f32,
                    r.get::<_, f64>(5)? as f32,
                    r.get::<_, f64>(6)? as f32,
                ],
                score: r.get::<_, f64>(7)? as f32,
                landmarks,
                person_id: person_id.map(PersonId),
                person_name: r.get(10)?,
                user_assigned: r.get::<_, i64>(11)? == 1,
                similarity: r.get::<_, Option<f64>>(12)?.map(|s| s as f32),
                has_embedding: r.get::<_, i64>(13)? == 1,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns face details for all faces belonging to `person`.
    pub fn person_faces(&self, person: PersonId) -> Result<Vec<FaceDetail>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT af.id, af.asset_id, af.face_idx, af.x1, af.y1, af.x2, af.y2, af.score,
                    af.landmarks, af.person_id, p.name, af.user_assigned, af.similarity,
                    af.embedding IS NOT NULL
             FROM asset_face af
             JOIN person p ON p.id = af.person_id
             WHERE af.person_id = ?1
             ORDER BY af.score DESC, af.id ASC",
        )?;
        let rows = stmt.query_map([person.0], |r| {
            let lm_str: Option<String> = r.get(8)?;
            let landmarks = lm_str
                .as_deref()
                .and_then(crate::vision::decode_landmarks)
                .unwrap_or([[0.0, 0.0]; 5]);
            let person_id: Option<i64> = r.get(9)?;
            Ok(FaceDetail {
                id: r.get(0)?,
                asset_id: AssetId(r.get(1)?),
                face_idx: r.get::<_, i64>(2)? as usize,
                bbox: [
                    r.get::<_, f64>(3)? as f32,
                    r.get::<_, f64>(4)? as f32,
                    r.get::<_, f64>(5)? as f32,
                    r.get::<_, f64>(6)? as f32,
                ],
                score: r.get::<_, f64>(7)? as f32,
                landmarks,
                person_id: person_id.map(PersonId),
                person_name: r.get(10)?,
                user_assigned: r.get::<_, i64>(11)? == 1,
                similarity: r.get::<_, Option<f64>>(12)?.map(|s| s as f32),
                has_embedding: r.get::<_, i64>(13)? == 1,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Incrementally groups `face_id` using its 128-dimensional L2-normalized embedding.
    ///
    /// - If `face_id` is user-assigned (`user_assigned == 1`), its person assignment (including an
    ///   explicit removal where `person_id` is `None`) is never changed.
    /// - Otherwise, compares against all existing persons' stored centroids (O(persons), not O(faces)).
    /// - If maximum similarity >= `threshold`: assigns the face to that person and updates its centroid.
    /// - If below threshold (or no existing persons): creates a new unnamed person with this embedding
    ///   as its centroid and assigns the face.
    ///
    /// Saves the embedding on `asset_face`. Returns the assigned `PersonId` (or `None` if the face
    /// was explicitly removed by the user).
    pub fn group_face(
        &mut self,
        face_id: i64,
        embedding: &[f32; 128],
        threshold: f32,
    ) -> Result<Option<PersonId>, CatalogError> {
        let blob = encode_face_embedding(embedding);
        let tx = self.conn.transaction()?;

        // 1. Check if user assigned (either pinned to a person or explicitly removed)
        let current: Option<(Option<i64>, i64)> = tx
            .query_row(
                "SELECT person_id, user_assigned FROM asset_face WHERE id = ?1",
                [face_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let Some((existing_pid, user_assigned)) = current else {
            return Err(CatalogError::InvalidOperation(format!(
                "face {face_id} not found"
            )));
        };

        if user_assigned == 1 {
            tx.execute(
                "UPDATE asset_face SET embedding = ?1 WHERE id = ?2",
                params![blob.as_slice(), face_id],
            )?;
            if let Some(pid) = existing_pid {
                recompute_person_centroid(&tx, pid)?;
            }
            tx.commit()?;
            return Ok(existing_pid.map(PersonId));
        }

        // 2. Compare against stored person centroids (O(persons))
        let mut best_person: Option<(i64, f32)> = None;
        {
            let mut stmt =
                tx.prepare_cached("SELECT id, centroid FROM person WHERE centroid IS NOT NULL")?;
            let rows = stmt.query_map([], |r| {
                let pid: i64 = r.get(0)?;
                let b: Vec<u8> = r.get(1)?;
                Ok((pid, b))
            })?;
            for row in rows {
                let (pid, bytes) = row?;
                if let Some(centroid) = decode_face_embedding(&bytes) {
                    let sim = cosine_similarity_128(embedding, &centroid);
                    if sim >= threshold {
                        match best_person {
                            Some((_, best_sim)) if sim > best_sim => {
                                best_person = Some((pid, sim));
                            }
                            None => {
                                best_person = Some((pid, sim));
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        // 3. Assign to best person or create a new unnamed person
        let (assigned_pid, sim_val, created_new) = match best_person {
            Some((pid, sim)) => (pid, sim, false),
            None => {
                tx.execute(
                    "INSERT INTO person (name, hidden, ignored, centroid) VALUES (NULL, 0, 0, ?1)",
                    [blob.as_slice()],
                )?;
                let new_pid = tx.last_insert_rowid();
                (new_pid, 1.0f32, true)
            }
        };

        tx.execute(
            "UPDATE asset_face
             SET person_id = ?1, similarity = ?2, user_assigned = 0, embedding = ?3
             WHERE id = ?4",
            params![assigned_pid, f64::from(sim_val), blob.as_slice(), face_id],
        )?;

        if !created_new {
            recompute_person_centroid(&tx, assigned_pid)?;
        }
        if let Some(old_pid) = existing_pid
            && old_pid != assigned_pid
        {
            recompute_person_centroid(&tx, old_pid)?;
        }

        tx.commit()?;
        Ok(Some(PersonId(assigned_pid)))
    }

    /// Clears all computed face grouping data: deletes all persons and clears person_id,
    /// similarity, user_assigned, and embedding on all asset_face rows.
    ///
    /// Does NOT delete face bounding boxes or touch any labels/tags or other catalog data.
    pub fn clear_face_grouping_data(&mut self) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM person", [])?;
        tx.execute(
            "UPDATE asset_face
             SET person_id = NULL, user_assigned = 0, similarity = NULL, embedding = NULL",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Returns all asset IDs that have detected faces missing face embeddings.
    pub fn assets_with_faces_needing_embeddings(&self) -> Result<Vec<AssetId>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT asset_id
             FROM asset_face
             WHERE embedding IS NULL
             ORDER BY asset_id ASC",
        )?;
        let rows = stmt.query_map([], |r| Ok(AssetId(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}
