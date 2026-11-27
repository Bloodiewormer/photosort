use catalog::{AssetId, Catalog, Filter, LabelId, LabelKind, Sort, Source};
use fsguard::{DestPath, LibraryRoot, ensure_writable};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
use image::{ColorType, ImageEncoder, Rgb, RgbImage};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thumbs::Thumbs;

pub const DEFAULT_BENCH_COUNT: usize = 35_000;
pub const MAX_BENCH_COUNT: usize = 100_000;
pub const BENCH_QUERY_RUNS: usize = 20;
pub const BENCH_THUMB_COUNT: usize = 500;
pub const BENCH_THUMB_MAX_PX: u32 = 256;
pub const BENCH_REVIEW_THRESHOLD: f64 = 0.60;

pub const TARGET_QUERY_P95_MS: f64 = 150.0;
pub const TARGET_THREE_LABEL_P95_MS: f64 = 250.0;
pub const TARGET_REINDEX_SECS: f64 = 5.0;

const DEFAULT_SEED: u64 = 0x4245_4E43_4833_354B;
const SUBFOLDER_COUNT: usize = 200;

const FOLDER_PREFIXES: &[&str] = &["capturas", "manga", "memes", "fotos", "escaneos"];

const SEED_LABELS: &[(LabelKind, &str)] = &[
    (LabelKind::Category, "Anime"),
    (LabelKind::Category, "Capturas de pantalla"),
    (LabelKind::Category, "Documentos"),
    (LabelKind::Category, "Fotografía"),
    (LabelKind::Category, "Manga"),
    (LabelKind::Category, "Memes"),
    (LabelKind::Tag, "favorito"),
    (LabelKind::Tag, "portada"),
    (LabelKind::Tag, "recorte"),
    (LabelKind::Tag, "referencia"),
    (LabelKind::Tag, "reacción"),
    (LabelKind::Tag, "promocional"),
    (LabelKind::Tag, "boceto"),
    (LabelKind::Tag, "texto"),
    (LabelKind::Tag, "monocromo"),
    (LabelKind::Tag, "color"),
    (LabelKind::Tag, "créditos"),
    (LabelKind::Tag, "vertical"),
    (LabelKind::Entity, "Crónicas del Relojero"),
    (LabelKind::Entity, "Academia Nebulosa"),
    (LabelKind::Entity, "El Jardín de Papel"),
    (LabelKind::Entity, "Estudio Cometa"),
    (LabelKind::Entity, "Isla de Cristal"),
    (LabelKind::Entity, "Los Otakus Fansub"),
    (LabelKind::Entity, "Torre del Viento"),
];

const CONFIDENCES: &[f64] = &[0.22, 0.31, 0.42, 0.51, 0.58, 0.66, 0.74, 0.83, 0.91, 0.96];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BenchConfig {
    pub count: usize,
    pub query_runs: usize,
    pub thumb_count: usize,
    pub seed: u64,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            count: DEFAULT_BENCH_COUNT,
            query_runs: BENCH_QUERY_RUNS,
            thumb_count: BENCH_THUMB_COUNT,
            seed: DEFAULT_SEED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetStatus {
    Pass,
    Warn,
}

impl TargetStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
        }
    }
}

#[derive(Debug, Clone)]
pub struct IndexTiming {
    pub elapsed: Duration,
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub hashed: usize,
    pub errors: usize,
}

#[derive(Debug, Clone)]
pub struct QueryTiming {
    pub name: String,
    pub runs: usize,
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub rows: usize,
    pub target_p95_ms: f64,
    pub status: TargetStatus,
    pub explain_plan: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct BenchReport {
    pub count: usize,
    pub subfolders: usize,
    pub generation_elapsed: Duration,
    pub library_bytes: u64,
    pub scan_files: usize,
    pub scan_elapsed: Duration,
    pub first_index: IndexTiming,
    pub second_index: IndexTiming,
    pub second_index_status: TargetStatus,
    pub rules_analyzed: usize,
    pub rules_suggestions: usize,
    pub rules_errors: usize,
    pub rules_elapsed: Duration,
    pub seed_assignments: usize,
    pub seed_elapsed: Duration,
    pub queries: Vec<QueryTiming>,
    pub duplicate_groups: usize,
    pub thumbs_generated: usize,
    pub thumbs_errors: usize,
    pub thumbs_elapsed: Duration,
    pub db_bytes: u64,
    pub peak_rss_bytes: Option<u64>,
}

impl BenchReport {
    pub fn has_warnings(&self) -> bool {
        self.second_index_status == TargetStatus::Warn
            || self.queries.iter().any(|q| q.status == TargetStatus::Warn)
    }
}

/// Validates `--bench-count` within `1..=MAX_BENCH_COUNT`.
pub fn validate_bench_count(count: usize) -> Result<usize, String> {
    if count == 0 || count > MAX_BENCH_COUNT {
        return Err(format!(
            "--bench-count debe estar entre 1 y {MAX_BENCH_COUNT} (recibido: {count})"
        ));
    }
    Ok(count)
}

/// Parses `--bench` and optional `--bench-count <N>`. Returns `Ok(None)` when neither flag is
/// present so regular CLI parsing continues untouched.
pub fn parse_bench_args<I, S>(args: I) -> Result<Option<BenchConfig>, String>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    let items: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let has_bench_flag = items.iter().any(|a| a == "--bench" || a == "--bench-count");
    if !has_bench_flag {
        return Ok(None);
    }

    let mut bench = false;
    let mut cfg = BenchConfig::default();
    let mut iter = items.into_iter();
    while let Some(arg) = iter.next() {
        let Some(flag) = arg.to_str() else {
            return Err(format!("Argumento desconocido: {}", arg.to_string_lossy()));
        };
        match flag {
            "--bench" => bench = true,
            "--bench-count" => {
                let val = iter
                    .next()
                    .ok_or_else(|| "Falta el número después de --bench-count".to_string())?;
                let text = val.to_string_lossy();
                let count: usize = text
                    .parse()
                    .map_err(|_| format!("Número inválido para --bench-count: '{text}'"))?;
                cfg.count = validate_bench_count(count)?;
            }
            other => {
                return Err(format!("Argumento no compatible con --bench: {other}"));
            }
        }
    }

    if !bench {
        return Err("--bench-count requiere --bench".to_string());
    }
    Ok(Some(cfg))
}

pub fn mean_ms(samples: &[Duration]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let total_ms: f64 = samples.iter().map(|d| d.as_secs_f64() * 1_000.0).sum();
    total_ms / samples.len() as f64
}

pub fn percentile_ms(samples: &[Duration], pct: f64) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut ms: Vec<f64> = samples.iter().map(|d| d.as_secs_f64() * 1_000.0).collect();
    ms.sort_by(f64::total_cmp);
    let clamped = pct.clamp(0.0, 100.0);
    let rank = ((clamped / 100.0) * ms.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(ms.len() - 1);
    ms[idx]
}

pub fn evaluate_target(measured: f64, limit: f64) -> TargetStatus {
    if measured < limit {
        TargetStatus::Pass
    } else {
        TargetStatus::Warn
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs >= 1.0 {
        format!("{secs:.2} s")
    } else {
        format!("{:.2} ms", secs * 1_000.0)
    }
}

pub fn format_throughput(count: usize, elapsed: Duration, unit: &str) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return format!("0 {unit}");
    }
    format!("{:.0} {unit}", count as f64 / secs)
}

fn format_mb_per_sec(bytes: u64, elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs <= 0.0 {
        return "0.0 MB/s".to_string();
    }
    let mb = bytes as f64 / (1024.0 * 1024.0);
    format!("{:.1} MB/s", mb / secs)
}

pub fn format_report(report: &BenchReport) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "=== Benchmark de rendimiento de photosort ({} imágenes, {} subcarpetas) ===",
        report.count, report.subfolders
    );
    let _ = writeln!(
        out,
        "Preparación (no evaluada): generación en {}, sembrado de {} etiquetas en {}",
        format_duration(report.generation_elapsed),
        report.seed_assignments,
        format_duration(report.seed_elapsed),
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        "Paso", "Tiempo", "Rendimiento / detalle", "Objetivo", "Estado"
    );
    let _ = writeln!(out, "{:-<102}", "");

    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        "Escaneo (scanner::scan)",
        format_duration(report.scan_elapsed),
        format_throughput(report.scan_files, report.scan_elapsed, "arch/s"),
        "-",
        "INFO"
    );

    let first_detail = format!(
        "{}, {}",
        format_throughput(
            report.first_index.hashed,
            report.first_index.elapsed,
            "arch/s"
        ),
        format_mb_per_sec(report.library_bytes, report.first_index.elapsed)
    );
    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        "Primera indexación (con SHA-256)",
        format_duration(report.first_index.elapsed),
        first_detail,
        "-",
        "INFO"
    );

    let second_detail = format!(
        "{} sin cambios, {} hash",
        report.second_index.unchanged, report.second_index.hashed
    );
    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        "Segunda indexación (sin cambios)",
        format_duration(report.second_index.elapsed),
        second_detail,
        format!("< {TARGET_REINDEX_SECS:.0} s"),
        report.second_index_status.as_str()
    );

    let rules_detail = format!(
        "{} ({} sug.)",
        format_throughput(report.rules_analyzed, report.rules_elapsed, "arch/s"),
        report.rules_suggestions
    );
    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        "Cola de análisis (rules::run)",
        format_duration(report.rules_elapsed),
        rules_detail,
        "-",
        "INFO"
    );

    let thumbs_label = format!(
        "Miniaturas ({} reales, {} px)",
        report.thumbs_generated, BENCH_THUMB_MAX_PX
    );
    let _ = writeln!(
        out,
        "{:<38} | {:>10} | {:>24} | {:>12} | {:>6}",
        thumbs_label,
        format_duration(report.thumbs_elapsed),
        format_throughput(report.thumbs_generated, report.thumbs_elapsed, "min/s"),
        "-",
        "INFO"
    );

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<38} | {:>9} | {:>9} | {:>8} | {:>12} | {:>6}",
        "Consulta interactiva (20 ejec.)", "Media", "p95", "Filas", "Objetivo p95", "Estado"
    );
    let _ = writeln!(out, "{:-<98}", "");

    for q in &report.queries {
        let _ = writeln!(
            out,
            "{:<38} | {:>6.2} ms | {:>6.2} ms | {:>8} | {:>9.0} ms | {:>6}",
            q.name,
            q.mean_ms,
            q.p95_ms,
            q.rows,
            q.target_p95_ms,
            q.status.as_str()
        );
    }

    let warned_plans: Vec<_> = report
        .queries
        .iter()
        .filter_map(|q| q.explain_plan.as_ref().map(|p| (&q.name, p)))
        .collect();
    if !warned_plans.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "Planes de consulta (EXPLAIN QUERY PLAN) para WARN:");
        for (name, lines) in warned_plans {
            let _ = writeln!(out, "  [{name}]");
            for line in lines {
                let _ = writeln!(out, "    {line}");
            }
        }
    }

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "Tamaño de la biblioteca: {}",
        format_bytes(report.library_bytes)
    );
    let _ = writeln!(
        out,
        "Tamaño de la base de datos: {}",
        format_bytes(report.db_bytes)
    );
    let _ = writeln!(
        out,
        "Grupos de duplicados exactos: {}",
        report.duplicate_groups
    );
    match report.peak_rss_bytes {
        Some(rss) => {
            let _ = writeln!(out, "Memoria residente pico (RSS): {}", format_bytes(rss));
        }
        None => {
            let _ = writeln!(
                out,
                "Memoria residente pico (RSS): no disponible en esta plataforma"
            );
        }
    }

    out
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper <= 1 {
            0
        } else {
            (self.next_u64() as usize) % upper
        }
    }
}

fn crc32_png(chunk_type: &[u8; 4], data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &b in chunk_type.iter().chain(data.iter()) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn encode_png_template(width: u32, height: u32, seed: u64) -> Result<Vec<u8>, String> {
    let mut img = RgbImage::new(width, height);
    let base_r = (seed & 0xFF) as u8;
    let base_g = ((seed >> 8) & 0xFF) as u8;
    let base_b = ((seed >> 16) & 0xFF) as u8;
    for y in 0..height {
        for x in 0..width {
            let r = base_r.wrapping_add((x * 7) as u8);
            let g = base_g.wrapping_add((y * 9) as u8);
            let b = base_b.wrapping_add(((x ^ y) * 5) as u8);
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    let mut out = Vec::new();
    let encoder =
        PngEncoder::new_with_quality(&mut out, CompressionType::Fast, PngFilterType::NoFilter);
    encoder
        .write_image(img.as_raw(), width, height, ColorType::Rgb8.into())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn encode_jpeg_template(width: u32, height: u32, seed: u64) -> Result<Vec<u8>, String> {
    let mut img = RgbImage::new(width, height);
    let base_r = ((seed >> 4) & 0xFF) as u8;
    let base_g = ((seed >> 12) & 0xFF) as u8;
    let base_b = ((seed >> 20) & 0xFF) as u8;
    for y in 0..height {
        for x in 0..width {
            let r = base_r.wrapping_add((x * 5) as u8);
            let g = base_g.wrapping_add((y * 11) as u8);
            let b = base_b.wrapping_add(((x + y) * 3) as u8);
            img.put_pixel(x, y, Rgb([r, g, b]));
        }
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 75)
        .encode(img.as_raw(), width, height, ColorType::Rgb8.into())
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn splice_png_text_chunk(template: &[u8], unique_id: usize) -> Result<Vec<u8>, String> {
    const IEND_LEN: usize = 12;
    if template.len() < 8 + IEND_LEN {
        return Err("plantilla PNG inválida".to_string());
    }
    let pad_len = (unique_id % 96) + 8;
    let mut data = Vec::with_capacity(16 + pad_len);
    data.extend_from_slice(b"id\0");
    let _ = write!(&mut data, "bench-{unique_id:08}-");
    let pad_byte = b'a' + (unique_id % 26) as u8;
    data.resize(data.len() + pad_len, pad_byte);

    let chunk_type = *b"tEXt";
    let crc = crc32_png(&chunk_type, &data);
    let len_bytes = (data.len() as u32).to_be_bytes();

    let split = template.len() - IEND_LEN;
    let mut out = Vec::with_capacity(template.len() + 12 + data.len());
    out.extend_from_slice(&template[..split]);
    out.extend_from_slice(&len_bytes);
    out.extend_from_slice(&chunk_type);
    out.extend_from_slice(&data);
    out.extend_from_slice(&crc.to_be_bytes());
    out.extend_from_slice(&template[split..]);
    Ok(out)
}

fn splice_jpeg_comment(template: &[u8], unique_id: usize) -> Result<Vec<u8>, String> {
    if template.len() < 4 || template[0] != 0xFF || template[1] != 0xD8 {
        return Err("plantilla JPEG inválida".to_string());
    }
    let pad_len = (unique_id % 96) + 8;
    let mut payload = Vec::with_capacity(16 + pad_len);
    let _ = write!(&mut payload, "bench-{unique_id:08}-");
    let pad_byte = b'a' + (unique_id % 26) as u8;
    payload.resize(payload.len() + pad_len, pad_byte);

    let seg_len = (payload.len() + 2) as u16;
    let mut out = Vec::with_capacity(template.len() + 4 + payload.len());
    out.extend_from_slice(&template[..2]);
    out.push(0xFF);
    out.push(0xFE);
    out.extend_from_slice(&seg_len.to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(&template[2..]);
    Ok(out)
}

/// Builds a valid synthetic PNG with a unique `tEXt` chunk so its SHA-256 and size vary cleanly.
pub fn build_unique_png(unique_id: usize, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let tpl = encode_png_template(width, height, DEFAULT_SEED ^ (unique_id as u64))?;
    splice_png_text_chunk(&tpl, unique_id)
}

/// Builds a valid synthetic JPEG with a unique `COM` marker so its SHA-256 and size vary cleanly.
pub fn build_unique_jpeg(unique_id: usize, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let tpl = encode_jpeg_template(width, height, DEFAULT_SEED ^ (unique_id as u64))?;
    splice_jpeg_comment(&tpl, unique_id)
}

#[derive(Clone)]
struct ImageTemplate {
    is_png: bool,
    bytes: Vec<u8>,
}

fn build_templates(seed: u64) -> Result<Vec<ImageTemplate>, String> {
    let png_dims: &[(u32, u32)] = &[
        (16, 16),
        (24, 32),
        (32, 24),
        (32, 48),
        (48, 32),
        (48, 48),
        (64, 48),
        (16, 48),
    ];
    let jpg_dims: &[(u32, u32)] = &[
        (16, 24),
        (24, 24),
        (32, 32),
        (40, 32),
        (48, 40),
        (48, 64),
        (64, 48),
        (64, 64),
    ];
    let mut templates = Vec::with_capacity(png_dims.len() + jpg_dims.len());
    for (idx, &(w, h)) in png_dims.iter().enumerate() {
        templates.push(ImageTemplate {
            is_png: true,
            bytes: encode_png_template(w, h, seed.wrapping_add(idx as u64 * 101))?,
        });
    }
    for (idx, &(w, h)) in jpg_dims.iter().enumerate() {
        templates.push(ImageTemplate {
            is_png: false,
            bytes: encode_jpeg_template(w, h, seed.wrapping_add((idx + 32) as u64 * 131))?,
        });
    }
    Ok(templates)
}

#[allow(clippy::disallowed_methods)] // `dir` is a fsguard DestPath inside the temporary bench dir
fn create_dir_dest(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

#[allow(clippy::disallowed_methods)] // `dest` is a fsguard DestPath inside the temporary bench dir
fn write_bytes_dest(dest: &DestPath, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(dest.as_path())?;
    file.write_all(bytes)
}

fn generate_synthetic_library(
    library_dest: &DestPath,
    count: usize,
    seed: u64,
) -> Result<(usize, u64), String> {
    let templates = build_templates(seed)?;
    let subfolders = SUBFOLDER_COUNT.min(count).max(1);
    let mut folder_dests: Vec<DestPath> = Vec::with_capacity(subfolders);
    for f in 0..subfolders {
        let name = format!("{}_{f:03}", FOLDER_PREFIXES[f % FOLDER_PREFIXES.len()]);
        let dest = library_dest
            .join_name(OsStr::new(&name))
            .map_err(|e| e.to_string())?;
        create_dir_dest(&dest).map_err(|e| e.to_string())?;
        folder_dests.push(dest);
    }

    // Keep ~1.5% of unique files in memory to produce ~3% exact duplicates across folders.
    let dup_pool_cap = (count / 66).max(1);
    let mut dup_pool: Vec<(bool, Vec<u8>)> = Vec::with_capacity(dup_pool_cap);
    let mut total_bytes: u64 = 0;

    for i in 0..count {
        let folder_idx = i % subfolders;
        let is_dup = i >= 33 && i % 33 == 0 && !dup_pool.is_empty();

        let (is_png, bytes): (bool, Vec<u8>) = if is_dup {
            let src_idx = ((i / 33) - 1) % dup_pool.len();
            let (png, ref data) = dup_pool[src_idx];
            (png, data.clone())
        } else {
            let tpl = &templates[i % templates.len()];
            let data = if tpl.is_png {
                splice_png_text_chunk(&tpl.bytes, i)?
            } else {
                splice_jpeg_comment(&tpl.bytes, i)?
            };
            if dup_pool.len() < dup_pool_cap {
                dup_pool.push((tpl.is_png, data.clone()));
            }
            (tpl.is_png, data)
        };

        let ext = if is_png { "png" } else { "jpg" };
        let day = (i % 28) + 1;
        let file_name = if is_dup {
            format!("dup_{i:06}.{ext}")
        } else {
            match i % 15 {
                0 => format!("Screenshot_2024-01-{day:02}_{i:06}.{ext}"),
                1 => format!("IMG-202401{day:02}-WA{i:06}.{ext}"),
                2 | 3 => format!("pagina_{i:06}.{ext}"),
                _ => format!("foto_{i:06}.{ext}"),
            }
        };

        let file_dest = folder_dests[folder_idx]
            .join_name(OsStr::new(&file_name))
            .map_err(|e| e.to_string())?;
        write_bytes_dest(&file_dest, &bytes).map_err(|e| e.to_string())?;
        total_bytes += bytes.len() as u64;
    }

    Ok((subfolders, total_bytes))
}

fn seed_realistic_labels(
    cat: &Catalog,
    assets: &[AssetId],
    seed: u64,
) -> Result<(Vec<LabelId>, usize), String> {
    let mut label_ids = Vec::with_capacity(SEED_LABELS.len());
    for &(kind, name) in SEED_LABELS {
        let id = cat.label(kind, name).map_err(|e| e.to_string())?;
        label_ids.push(id);
    }

    let mut rng = Rng::new(seed ^ 0x5EED_CAFE_1234_5678);
    let mut assignments = 0usize;
    let mut chosen: Vec<usize> = Vec::with_capacity(6);

    for (idx, &asset) in assets.iter().enumerate() {
        let k = rng.range(7); // 0..=6 labels per asset
        chosen.clear();
        for _ in 0..k {
            // Bias slightly toward earlier labels so filter queries match realistic slices.
            let candidate = if rng.range(3) == 0 {
                rng.range(6)
            } else {
                rng.range(label_ids.len())
            };
            if chosen.contains(&candidate) {
                continue;
            }
            chosen.push(candidate);
            let label = label_ids[candidate];
            let roll = (idx + candidate + rng.range(100)) % 100;
            if roll < 45 {
                cat.accept(asset, label).map_err(|e| e.to_string())?;
            } else if roll < 90 {
                let source = match roll % 3 {
                    0 => Source::Ai,
                    1 => Source::Ocr,
                    _ => Source::Rules,
                };
                let conf = CONFIDENCES[(idx + candidate) % CONFIDENCES.len()];
                cat.suggest(asset, label, source, conf)
                    .map_err(|e| e.to_string())?;
            } else {
                cat.reject(asset, label).map_err(|e| e.to_string())?;
            }
            assignments += 1;
        }
    }

    Ok((label_ids, assignments))
}

fn time_query<F, E>(
    name: &str,
    runs: usize,
    target_p95_ms: f64,
    mut f: F,
    mut explain: E,
) -> Result<QueryTiming, String>
where
    F: FnMut() -> Result<usize, String>,
    E: FnMut() -> Result<Vec<String>, String>,
{
    let runs = runs.max(1);
    let mut samples = Vec::with_capacity(runs);
    let mut last_rows = 0usize;
    for _ in 0..runs {
        let start = Instant::now();
        last_rows = f()?;
        samples.push(start.elapsed());
    }
    let mean = mean_ms(&samples);
    let p95 = percentile_ms(&samples, 95.0);
    let status = evaluate_target(p95, target_p95_ms);
    let explain_plan = if status == TargetStatus::Warn {
        Some(explain()?)
    } else {
        None
    };
    Ok(QueryTiming {
        name: name.to_string(),
        runs,
        mean_ms: mean,
        p95_ms: p95,
        rows: last_rows,
        target_p95_ms,
        status,
        explain_plan,
    })
}

fn sqlite_total_bytes(db_path: &Path) -> u64 {
    let mut total = fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    for suffix in ["-wal", "-shm"] {
        let mut side = db_path.as_os_str().to_os_string();
        side.push(suffix);
        let p = PathBuf::from(side);
        total += fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    }
    total
}

#[cfg(windows)]
fn peak_rss_bytes() -> Option<u64> {
    use std::mem::{MaybeUninit, size_of};
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = MaybeUninit::<PROCESS_MEMORY_COUNTERS>::zeroed();
    let cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // SAFETY: `GetCurrentProcess` returns the current process pseudo-handle and `counters` is
    // initialized to the exact size of `PROCESS_MEMORY_COUNTERS`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), counters.as_mut_ptr(), cb) };
    if ok != 0 {
        // SAFETY: `K32GetProcessMemoryInfo` succeeded and populated `counters`.
        let initialized = unsafe { counters.assume_init() };
        Some(initialized.PeakWorkingSetSize as u64)
    } else {
        None
    }
}

#[cfg(not(windows))]
fn peak_rss_bytes() -> Option<u64> {
    None
}

/// Runs the full synthetic benchmark in an isolated temporary directory that is deleted before
/// returning.
pub fn run_bench(config: BenchConfig) -> Result<BenchReport, String> {
    run_bench_with_hook(config, None, |_, _| {})
}

/// Runs the benchmark, invoking `on_step(step_name, library_dir)` after generation and after every
/// read-only step so tests can verify the library tree never changes.
pub fn run_bench_with_hook<F>(
    config: BenchConfig,
    parent_temp_dir: Option<&Path>,
    mut on_step: F,
) -> Result<BenchReport, String>
where
    F: FnMut(&str, &Path),
{
    let count = validate_bench_count(config.count)?;

    let temp_dir = match parent_temp_dir {
        Some(parent) => tempfile::Builder::new()
            .prefix("photosort-bench-")
            .tempdir_in(parent)
            .map_err(|e| e.to_string())?,
        None => tempfile::Builder::new()
            .prefix("photosort-bench-")
            .tempdir()
            .map_err(|e| e.to_string())?,
    };

    let base_dest = ensure_writable(temp_dir.path(), &[]).map_err(|e| e.to_string())?;
    let library_dest = base_dest
        .join_name(OsStr::new("library"))
        .map_err(|e| e.to_string())?;
    let data_dest = base_dest
        .join_name(OsStr::new("data"))
        .map_err(|e| e.to_string())?;
    create_dir_dest(&library_dest).map_err(|e| e.to_string())?;
    create_dir_dest(&data_dest).map_err(|e| e.to_string())?;

    // 1. Generación de la biblioteca sintética (preparación, no evaluada como objetivo).
    let gen_start = Instant::now();
    let (subfolders, library_bytes) =
        generate_synthetic_library(&library_dest, count, config.seed)?;
    let generation_elapsed = gen_start.elapsed();

    let library_dir = library_dest.as_path().to_path_buf();
    on_step("generacion", &library_dir);

    // Desde aquí la biblioteca es estrictamente de solo lectura mediante LibraryRoot.
    let root = LibraryRoot::new(&library_dir).map_err(|e| e.to_string())?;
    let roots = [root];
    let db_dest = ensure_writable(&data_dest.as_path().join("catalog.sqlite"), &roots)
        .map_err(|e| e.to_string())?;
    let thumbs_dest =
        ensure_writable(&data_dest.as_path().join("thumbs"), &roots).map_err(|e| e.to_string())?;

    // 2. Escaneo puro.
    let scan_start = Instant::now();
    let scanned = scanner::scan(roots[0].as_path()).map_err(|e| e.to_string())?;
    let scan_elapsed = scan_start.elapsed();
    let scan_files = scanned.files.len();
    on_step("escaneo", &library_dir);

    // 3. Primera indexación completa (con cálculo SHA-256).
    let mut cat = Catalog::open(&db_dest).map_err(|e| e.to_string())?;
    let idx1_start = Instant::now();
    let idx1 = cat.index(&roots[0]).map_err(|e| e.to_string())?;
    let idx1_elapsed = idx1_start.elapsed();
    let first_index = IndexTiming {
        elapsed: idx1_elapsed,
        new: idx1.new,
        changed: idx1.changed,
        unchanged: idx1.unchanged,
        hashed: idx1.hashed,
        errors: idx1.errors.len(),
    };
    on_step("primera_indexacion", &library_dir);

    // 4. Segunda indexación sin cambios (no debe calcular ningún hash).
    let idx2_start = Instant::now();
    let idx2 = cat.index(&roots[0]).map_err(|e| e.to_string())?;
    let idx2_elapsed = idx2_start.elapsed();
    if idx2.hashed != 0 {
        return Err(format!(
            "la segunda indexación calculó {} hashes inesperados",
            idx2.hashed
        ));
    }
    let second_index = IndexTiming {
        elapsed: idx2_elapsed,
        new: idx2.new,
        changed: idx2.changed,
        unchanged: idx2.unchanged,
        hashed: idx2.hashed,
        errors: idx2.errors.len(),
    };
    let second_index_status = evaluate_target(idx2_elapsed.as_secs_f64(), TARGET_REINDEX_SECS);
    on_step("segunda_indexacion", &library_dir);

    // 5. Cola de análisis de reglas.
    let rules_start = Instant::now();
    let rules_report = rules::run(&cat).map_err(|e| e.to_string())?;
    let rules_elapsed = rules_start.elapsed();
    on_step("reglas", &library_dir);

    // 6. Sembrado de etiquetas realistas (~25 etiquetas, 0-6 por imagen) vía API del catálogo.
    let all_entries = cat
        .entries(&Filter::default(), Sort::PathAsc)
        .map_err(|e| e.to_string())?;
    let mut seen_assets = HashSet::with_capacity(all_entries.len());
    let mut unique_assets: Vec<AssetId> = Vec::with_capacity(all_entries.len());
    let mut thumb_targets: Vec<(PathBuf, String)> = Vec::new();
    let thumb_limit = config.thumb_count.min(count);
    for entry in &all_entries {
        if seen_assets.insert(entry.asset) {
            unique_assets.push(entry.asset);
            if thumb_targets.len() < thumb_limit {
                thumb_targets.push((entry.path.clone(), entry.sha256.clone()));
            }
        }
    }

    let seed_start = Instant::now();
    let (label_ids, seed_assignments) = seed_realistic_labels(&cat, &unique_assets, config.seed)?;
    let seed_elapsed = seed_start.elapsed();

    // 7. Consultas interactivas (20 ejecuciones cada una).
    let runs = config.query_runs;
    let empty_filter = Filter::default();
    let mut queries = Vec::with_capacity(14);

    let sorts = [
        ("entries (vacío, PathAsc)", Sort::PathAsc),
        ("entries (vacío, PathDesc)", Sort::PathDesc),
        ("entries (vacío, SizeAsc)", Sort::SizeAsc),
        ("entries (vacío, SizeDesc)", Sort::SizeDesc),
        ("entries (vacío, ModifiedAsc)", Sort::ModifiedAsc),
        ("entries (vacío, ModifiedDesc)", Sort::ModifiedDesc),
    ];
    for (name, sort) in sorts {
        queries.push(time_query(
            name,
            runs,
            TARGET_QUERY_P95_MS,
            || {
                Ok(cat
                    .entries(&empty_filter, sort)
                    .map_err(|e| e.to_string())?
                    .len())
            },
            || {
                cat.explain_entries(&empty_filter, sort)
                    .map_err(|e| e.to_string())
            },
        )?);
    }

    let filter_1 = Filter {
        require: vec![label_ids[0]],
        ..Default::default()
    };
    queries.push(time_query(
        "entries (1 etiqueta: require)",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            Ok(cat
                .entries(&filter_1, Sort::PathAsc)
                .map_err(|e| e.to_string())?
                .len())
        },
        || {
            cat.explain_entries(&filter_1, Sort::PathAsc)
                .map_err(|e| e.to_string())
        },
    )?);

    let filter_2 = Filter {
        require: vec![label_ids[0]],
        any: vec![label_ids[1], label_ids[6]],
        ..Default::default()
    };
    queries.push(time_query(
        "entries (2 etiquetas: req + any)",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            Ok(cat
                .entries(&filter_2, Sort::PathAsc)
                .map_err(|e| e.to_string())?
                .len())
        },
        || {
            cat.explain_entries(&filter_2, Sort::PathAsc)
                .map_err(|e| e.to_string())
        },
    )?);

    let filter_3 = Filter {
        require: vec![label_ids[0]],
        any: vec![label_ids[1], label_ids[6]],
        exclude: vec![label_ids[2]],
        ..Default::default()
    };
    queries.push(time_query(
        "entries (3 etiquetas: req+any+excl)",
        runs,
        TARGET_THREE_LABEL_P95_MS,
        || {
            Ok(cat
                .entries(&filter_3, Sort::PathAsc)
                .map_err(|e| e.to_string())?
                .len())
        },
        || {
            cat.explain_entries(&filter_3, Sort::PathAsc)
                .map_err(|e| e.to_string())
        },
    )?);

    let filter_path = Filter {
        path_contains: Some("manga".to_string()),
        ..Default::default()
    };
    queries.push(time_query(
        "entries (filtro de texto en ruta)",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            Ok(cat
                .entries(&filter_path, Sort::PathAsc)
                .map_err(|e| e.to_string())?
                .len())
        },
        || {
            cat.explain_entries(&filter_path, Sort::PathAsc)
                .map_err(|e| e.to_string())
        },
    )?);

    queries.push(time_query(
        "labels_in_use",
        runs,
        TARGET_QUERY_P95_MS,
        || Ok(cat.labels_in_use().map_err(|e| e.to_string())?.len()),
        || cat.explain_labels_in_use().map_err(|e| e.to_string()),
    )?);

    let mut duplicate_groups = 0usize;
    queries.push(time_query(
        "duplicate_groups",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            duplicate_groups = cat.duplicate_groups().map_err(|e| e.to_string())?.len();
            Ok(duplicate_groups)
        },
        || cat.explain_duplicate_groups().map_err(|e| e.to_string()),
    )?);

    queries.push(time_query(
        "review_queue",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            Ok(cat
                .review_queue(BENCH_REVIEW_THRESHOLD)
                .map_err(|e| e.to_string())?
                .len())
        },
        || {
            cat.explain_review_queue(BENCH_REVIEW_THRESHOLD)
                .map_err(|e| e.to_string())
        },
    )?);

    queries.push(time_query(
        "counts",
        runs,
        TARGET_QUERY_P95_MS,
        || {
            let c = cat.counts().map_err(|e| e.to_string())?;
            Ok(c.locations.max(0) as usize)
        },
        || cat.explain_counts().map_err(|e| e.to_string()),
    )?);

    on_step("consultas", &library_dir);

    // 8. Generación de miniaturas reales (hasta 500, 256 px).
    let thumbs = Thumbs::new(&thumbs_dest, BENCH_THUMB_MAX_PX).map_err(|e| e.to_string())?;
    let thumbs_start = Instant::now();
    let mut thumbs_generated = 0usize;
    let mut thumbs_errors = 0usize;
    for (src_path, sha256) in &thumb_targets {
        match thumbs.get(src_path, sha256) {
            Ok(_) => thumbs_generated += 1,
            Err(_) => thumbs_errors += 1,
        }
    }
    let thumbs_elapsed = thumbs_start.elapsed();
    on_step("miniaturas", &library_dir);

    let db_bytes = sqlite_total_bytes(db_dest.as_path());
    let peak_rss_bytes = peak_rss_bytes();

    // Cerrar el catálogo antes de eliminar el directorio temporal en Windows.
    drop(cat);
    temp_dir.close().map_err(|e| e.to_string())?;

    Ok(BenchReport {
        count,
        subfolders,
        generation_elapsed,
        library_bytes,
        scan_files,
        scan_elapsed,
        first_index,
        second_index,
        second_index_status,
        rules_analyzed: rules_report.analyzed,
        rules_suggestions: rules_report.suggestions,
        rules_errors: rules_report.errors.len(),
        rules_elapsed,
        seed_assignments,
        seed_elapsed,
        queries,
        duplicate_groups,
        thumbs_generated,
        thumbs_errors,
        thumbs_elapsed,
        db_bytes,
        peak_rss_bytes,
    })
}
