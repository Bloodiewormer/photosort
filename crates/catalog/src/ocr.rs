use crate::labels::unexpected;
use crate::{AssetId, Catalog, CatalogError};
use rusqlite::params;
use unicode_normalization::UnicodeNormalization;

pub const OCR_STAGE: &str = "ocr";
pub const OCR_VERSION: u32 = 1;
pub const SCREENSHOT_CATEGORY: &str = "Capturas de pantalla";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OcrRecognizer {
    V6,
    Korean,
}

impl OcrRecognizer {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::V6 => "v6",
            Self::Korean => "korean",
        }
    }

    pub(crate) fn parse(s: &str) -> Option<Self> {
        match s {
            "v6" => Some(Self::V6),
            "korean" => Some(Self::Korean),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub score: f32,
    pub recognizer: OcrRecognizer,
    /// Axis-aligned bounding box `[x0, y0, x1, y1]` in image pixels.
    pub rect: [f32; 4],
    /// True when the line sits in the top status-bar strip of a tall phone-aspect image.
    pub top_strip: bool,
}

/// Normalizes OCR text and search queries identically:
/// NFKD, drop only combining diacritical marks (`U+0300..=U+036F`, keeping Japanese dakuten
/// and Hangul intact), NFC, lowercase, and remove all whitespace (OCR often loses spaces).
pub fn ocr_search_key(text: &str) -> String {
    let stripped: String = text
        .nfkd()
        .filter(|&c| !('\u{0300}'..='\u{036F}').contains(&c))
        .collect();
    let composed: String = stripped.nfc().collect();
    composed
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}

/// Returns `true` when the image has a tall phone-like aspect ratio (`height >= 1.7 * width`)
/// and the line's bottom edge (`rect[3]`) falls within the top 5% of the image height.
pub fn is_top_strip(width: u32, height: u32, rect: [f32; 4]) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    let tall = u64::from(height) * 10 >= u64::from(width) * 17;
    tall && f64::from(rect[3]) <= f64::from(height) * 0.05
}

impl Catalog {
    /// Replaces all OCR lines of `asset` and marks stage `"ocr"` version `1` done in one
    /// transaction. An empty slice still marks the asset done (e.g. an image with no text).
    pub fn save_ocr_lines(
        &mut self,
        asset: AssetId,
        lines: &[OcrLine],
    ) -> Result<(), CatalogError> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM ocr_line WHERE asset_id = ?1", [asset.0])?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO ocr_line
                     (asset_id, line, text, search_key, score, recognizer, x0, y0, x1, y1, top_strip)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for (idx, line) in lines.iter().enumerate() {
                let score = f64::from(line.score.clamp(0.0, 1.0));
                let key = ocr_search_key(&line.text);
                stmt.execute(params![
                    asset.0,
                    idx as i64,
                    &line.text,
                    &key,
                    score,
                    line.recognizer.as_str(),
                    f64::from(line.rect[0]),
                    f64::from(line.rect[1]),
                    f64::from(line.rect[2]),
                    f64::from(line.rect[3]),
                    i64::from(line.top_strip),
                ])?;
            }
        }
        tx.execute(
            "INSERT INTO analysis (asset_id, stage, version)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(asset_id, stage) DO UPDATE SET version = excluded.version
             WHERE analysis.version < excluded.version",
            params![asset.0, OCR_STAGE, i64::from(OCR_VERSION)],
        )?;
        tx.execute(
            "DELETE FROM stage_error WHERE asset_id = ?1 AND stage = ?2",
            params![asset.0, OCR_STAGE],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Returns all stored OCR lines for `asset`, in their original line order.
    pub fn ocr_lines(&self, asset: AssetId) -> Result<Vec<OcrLine>, CatalogError> {
        let mut stmt = self.conn.prepare(
            "SELECT text, score, recognizer, x0, y0, x1, y1, top_strip
             FROM ocr_line
             WHERE asset_id = ?1
             ORDER BY line ASC",
        )?;
        let rows = stmt.query_map([asset.0], |r| {
            let rec: String = r.get(2)?;
            let top: i64 = r.get(7)?;
            Ok(OcrLine {
                text: r.get(0)?,
                score: r.get::<_, f64>(1)? as f32,
                recognizer: OcrRecognizer::parse(&rec).ok_or_else(|| unexpected(2, &rec))?,
                rect: [
                    r.get::<_, f64>(3)? as f32,
                    r.get::<_, f64>(4)? as f32,
                    r.get::<_, f64>(5)? as f32,
                    r.get::<_, f64>(6)? as f32,
                ],
                top_strip: top != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns `true` if `asset` has completed OCR at the current version.
    pub fn has_ocr_done(&self, asset: AssetId) -> Result<bool, CatalogError> {
        let done: i64 = self.conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM analysis
                 WHERE asset_id = ?1 AND stage = ?2 AND version >= ?3
             )",
            params![asset.0, OCR_STAGE, i64::from(OCR_VERSION)],
            |r| r.get(0),
        )?;
        Ok(done != 0)
    }
}
