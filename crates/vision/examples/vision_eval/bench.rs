//! Release benchmark harness: separate process per configuration, alternating order, >= 3 reps.
//! Separates PNG decode + preprocess time from ONNX inference time.

use crate::Args;
use crate::probe::{GpuSampler, gpu_now, loaded_module, peak_working_set_bytes};
use image::{ImageFormat, RgbImage};
use serde_json::{Value, json};
use std::io::Cursor;
use std::process::Command;
use std::time::Instant;
use vision::manifest::{verify_siglip2, verify_yunet};
use vision::siglip2::{MAX_TEXT_TOKENS, preprocess_batch};
use vision::synth::{bench_images, verification_set};
use vision::yunet::preprocess_image as yunet_preprocess;
use vision::{Provider, SessionOptions, SigLip2Text, SigLip2Vision, YuNet, YuNetConfig};

const MARK: &str = "RESULT ";

fn median_f64(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    let mid = v.len() / 2;
    if v.len() % 2 == 1 {
        v[mid]
    } else {
        0.5 * (v[mid - 1] + v[mid])
    }
}

fn encode_pngs(images: &[RgbImage]) -> Vec<Vec<u8>> {
    images
        .iter()
        .map(|img| {
            let mut buf = Vec::new();
            img.write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .expect("in-memory PNG encode");
            buf
        })
        .collect()
}

pub fn one(args: &Args) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("benchmarks need --release".into());
    }
    let models = args.models()?;
    let task = args.value("--task").unwrap_or("vision");
    let provider = args.parsed("--provider", Provider::Cpu)?;
    let threads = match args.value("--threads") {
        None | Some("default") => None,
        Some(n) => Some(n.parse().map_err(|e| format!("--threads {n}: {e}"))?),
    };
    let batch: usize = args.parsed("--batch", 1)?;
    let passes: usize = args.parsed("--passes", 3)?;
    let opts = SessionOptions {
        provider,
        intra_threads: threads,
        log: None,
    };

    let gpu_before = gpu_now();
    let sampler = GpuSampler::start();

    let result = match task {
        "vision" => bench_vision(&models, &opts, batch, passes, gpu_before, sampler)?,
        "text" => bench_text(&models, &opts, batch, passes, gpu_before, sampler)?,
        "yunet" => bench_yunet(&models, &opts, passes, gpu_before, sampler)?,
        other => {
            return Err(format!(
                "unknown --task {other}: use vision, text, or yunet"
            ));
        }
    };
    println!("{MARK}{result}");
    Ok(())
}

fn bench_vision(
    models: &std::path::Path,
    opts: &SessionOptions,
    batch: usize,
    passes: usize,
    gpu_before: Option<(u32, u32)>,
    sampler: Option<GpuSampler>,
) -> Result<Value, String> {
    // Use 32 images so batch sizes 1, 8 and 16 divide evenly.
    let images = bench_images(32);
    let pngs = encode_pngs(&images);

    // Measure PNG decode + resize/normalize separately from inference.
    let dec_start = Instant::now();
    let decoded: Vec<RgbImage> = pngs
        .iter()
        .map(|b| {
            image::load_from_memory_with_format(b, ImageFormat::Png)
                .unwrap()
                .to_rgb8()
        })
        .collect();
    let decode_ms_per_img = dec_start.elapsed().as_secs_f64() * 1000.0 / decoded.len() as f64;

    let pre_start = Instant::now();
    let img_refs: Vec<&RgbImage> = decoded.iter().collect();
    let batches: Vec<Vec<f32>> = img_refs.chunks(batch).map(preprocess_batch).collect();
    let preprocess_ms_per_img = pre_start.elapsed().as_secs_f64() * 1000.0 / decoded.len() as f64;

    let t0 = Instant::now();
    verify_siglip2(models).map_err(|e| e.join("; "))?;
    let verify_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let mut model = SigLip2Vision::load(models, opts)?;
    let load_ms = t1.elapsed().as_secs_f64() * 1000.0;

    // First inference (cold start / shader compilation on DirectML).
    let t2 = Instant::now();
    let _ = model.embed_preprocessed(batch, &batches[0])?;
    let first_infer_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let steady_from = sampler.as_ref().map_or(0, GpuSampler::taken);
    let mut batch_times_ms = Vec::new();
    let steady_start = Instant::now();
    let mut total_items = 0usize;
    for _ in 0..passes {
        for buf in &batches {
            let bs = Instant::now();
            let _ = model.embed_preprocessed(batch, buf)?;
            batch_times_ms.push(bs.elapsed().as_secs_f64() * 1000.0);
            total_items += batch;
        }
    }
    let steady_secs = steady_start.elapsed().as_secs_f64();
    let samples = sampler.map(GpuSampler::finish).unwrap_or_default();
    let during = &samples[steady_from.min(samples.len())..];

    let med_batch_ms = median_f64(batch_times_ms);
    Ok(json!({
        "task": "vision",
        "provider": opts.provider.to_string(),
        "threads": opts.intra_threads.map_or("default".to_string(), |t| t.to_string()),
        "batch": batch,
        "items": total_items,
        "verify_ms": verify_ms,
        "load_ms": load_ms,
        "first_infer_ms": first_infer_ms,
        "decode_ms_per_item": decode_ms_per_img,
        "preprocess_ms_per_item": preprocess_ms_per_img,
        "infer_ms_per_item": med_batch_ms / batch as f64,
        "infer_rate": total_items as f64 / steady_secs,
        "rss_mb": peak_working_set_bytes() as f64 / 1_048_576.0,
        "gpu_before_mib": gpu_before.map(|g| g.0),
        "gpu_peak_mib": samples.iter().map(|s| s.0).max(),
        "gpu_util_mean": (!during.is_empty()).then(|| {
            during.iter().map(|s| f64::from(s.1)).sum::<f64>() / during.len() as f64
        }),
        "gpu_util_max": during.iter().map(|s| s.1).max(),
        "directml": loaded_module("DirectML.dll"),
    }))
}

fn bench_text(
    models: &std::path::Path,
    opts: &SessionOptions,
    batch: usize,
    passes: usize,
    gpu_before: Option<(u32, u32)>,
    sampler: Option<GpuSampler>,
) -> Result<Value, String> {
    let items = verification_set();
    // 32 prompts (16 EN + 16 ES) so batch 1 and 16 divide evenly.
    let prompts: Vec<&str> = items
        .iter()
        .take(16)
        .map(|i| i.prompt_en)
        .chain(items.iter().take(16).map(|i| i.prompt_es))
        .collect();

    let t0 = Instant::now();
    verify_siglip2(models).map_err(|e| e.join("; "))?;
    let verify_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let mut model = SigLip2Text::load(models, opts)?;
    let load_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let tok_start = Instant::now();
    let mut batches = Vec::new();
    for chunk in prompts.chunks(batch) {
        let mut buf = Vec::with_capacity(chunk.len() * MAX_TEXT_TOKENS);
        for p in chunk {
            buf.extend_from_slice(&model.tokenize(p)?);
        }
        batches.push((chunk.len(), buf));
    }
    let tokenize_ms_per_item = tok_start.elapsed().as_secs_f64() * 1000.0 / prompts.len() as f64;

    let t2 = Instant::now();
    let _ = model.embed_token_ids(batches[0].0, &batches[0].1)?;
    let first_infer_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let steady_from = sampler.as_ref().map_or(0, GpuSampler::taken);
    let mut batch_times_ms = Vec::new();
    let steady_start = Instant::now();
    let mut total_items = 0usize;
    for _ in 0..passes {
        for (n, buf) in &batches {
            let bs = Instant::now();
            let _ = model.embed_token_ids(*n, buf)?;
            batch_times_ms.push(bs.elapsed().as_secs_f64() * 1000.0);
            total_items += *n;
        }
    }
    let steady_secs = steady_start.elapsed().as_secs_f64();
    let samples = sampler.map(GpuSampler::finish).unwrap_or_default();
    let during = &samples[steady_from.min(samples.len())..];

    let med_batch_ms = median_f64(batch_times_ms);
    Ok(json!({
        "task": "text",
        "provider": opts.provider.to_string(),
        "threads": opts.intra_threads.map_or("default".to_string(), |t| t.to_string()),
        "batch": batch,
        "items": total_items,
        "verify_ms": verify_ms,
        "load_ms": load_ms,
        "first_infer_ms": first_infer_ms,
        "decode_ms_per_item": 0.0,
        "preprocess_ms_per_item": tokenize_ms_per_item,
        "infer_ms_per_item": med_batch_ms / batch as f64,
        "infer_rate": total_items as f64 / steady_secs,
        "rss_mb": peak_working_set_bytes() as f64 / 1_048_576.0,
        "gpu_before_mib": gpu_before.map(|g| g.0),
        "gpu_peak_mib": samples.iter().map(|s| s.0).max(),
        "gpu_util_mean": (!during.is_empty()).then(|| {
            during.iter().map(|s| f64::from(s.1)).sum::<f64>() / during.len() as f64
        }),
        "gpu_util_max": during.iter().map(|s| s.1).max(),
        "directml": loaded_module("DirectML.dll"),
    }))
}

fn bench_yunet(
    models: &std::path::Path,
    opts: &SessionOptions,
    passes: usize,
    gpu_before: Option<(u32, u32)>,
    sampler: Option<GpuSampler>,
) -> Result<Value, String> {
    let images = bench_images(16);
    let pngs = encode_pngs(&images);

    let dec_start = Instant::now();
    let decoded: Vec<RgbImage> = pngs
        .iter()
        .map(|b| {
            image::load_from_memory_with_format(b, ImageFormat::Png)
                .unwrap()
                .to_rgb8()
        })
        .collect();
    let decode_ms_per_img = dec_start.elapsed().as_secs_f64() * 1000.0 / decoded.len() as f64;

    let pre_start = Instant::now();
    let prepped: Vec<_> = decoded.iter().map(yunet_preprocess).collect();
    let preprocess_ms_per_img = pre_start.elapsed().as_secs_f64() * 1000.0 / decoded.len() as f64;

    let t0 = Instant::now();
    verify_yunet(models).map_err(|e| e.join("; "))?;
    let verify_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let t1 = Instant::now();
    let mut model = YuNet::load(models, opts)?;
    let load_ms = t1.elapsed().as_secs_f64() * 1000.0;

    let cfg = YuNetConfig::default();
    let t2 = Instant::now();
    let _ = model.detect_preprocessed(&prepped[0].0, prepped[0].1, cfg)?;
    let first_infer_ms = t2.elapsed().as_secs_f64() * 1000.0;

    let steady_from = sampler.as_ref().map_or(0, GpuSampler::taken);
    let mut times_ms = Vec::new();
    let steady_start = Instant::now();
    let mut total_items = 0usize;
    for _ in 0..passes {
        for (buf, lb) in &prepped {
            let bs = Instant::now();
            let _ = model.detect_preprocessed(buf, *lb, cfg)?;
            times_ms.push(bs.elapsed().as_secs_f64() * 1000.0);
            total_items += 1;
        }
    }
    let steady_secs = steady_start.elapsed().as_secs_f64();
    let samples = sampler.map(GpuSampler::finish).unwrap_or_default();
    let during = &samples[steady_from.min(samples.len())..];

    Ok(json!({
        "task": "yunet",
        "provider": opts.provider.to_string(),
        "threads": opts.intra_threads.map_or("default".to_string(), |t| t.to_string()),
        "batch": 1,
        "items": total_items,
        "verify_ms": verify_ms,
        "load_ms": load_ms,
        "first_infer_ms": first_infer_ms,
        "decode_ms_per_item": decode_ms_per_img,
        "preprocess_ms_per_item": preprocess_ms_per_img,
        "infer_ms_per_item": median_f64(times_ms),
        "infer_rate": total_items as f64 / steady_secs,
        "rss_mb": peak_working_set_bytes() as f64 / 1_048_576.0,
        "gpu_before_mib": gpu_before.map(|g| g.0),
        "gpu_peak_mib": samples.iter().map(|s| s.0).max(),
        "gpu_util_mean": (!during.is_empty()).then(|| {
            during.iter().map(|s| f64::from(s.1)).sum::<f64>() / during.len() as f64
        }),
        "gpu_util_max": during.iter().map(|s| s.1).max(),
        "directml": loaded_module("DirectML.dll"),
    }))
}

#[derive(Clone, Copy)]
struct Config {
    task: &'static str,
    provider: &'static str,
    threads: &'static str,
    batch: &'static str,
}

fn configs(filter_task: Option<&str>) -> Vec<Config> {
    let mut all = Vec::new();
    let providers = [
        ("cpu", "1"),
        ("cpu", "4"),
        ("cpu", "6"),
        ("cpu", "default"),
        ("dml:0", "default"),
    ];
    if filter_task.is_none() || filter_task == Some("vision") {
        for batch in ["1", "8", "16"] {
            for (provider, threads) in providers {
                all.push(Config {
                    task: "vision",
                    provider,
                    threads,
                    batch,
                });
            }
        }
    }
    if filter_task.is_none() || filter_task == Some("text") {
        for batch in ["1", "16"] {
            for (provider, threads) in providers {
                all.push(Config {
                    task: "text",
                    provider,
                    threads,
                    batch,
                });
            }
        }
    }
    if filter_task.is_none() || filter_task == Some("yunet") {
        for (provider, threads) in providers {
            all.push(Config {
                task: "yunet",
                provider,
                threads,
                batch: "1",
            });
        }
    }
    all
}

fn format_med(vals: &[f64], decimals: usize) -> String {
    if vals.is_empty() {
        return "-".into();
    }
    let mut s = vals.to_vec();
    s.sort_by(f64::total_cmp);
    let med = median_f64(s.clone());
    let lo = s[0];
    let hi = *s.last().unwrap();
    format!("{med:.decimals$} [{lo:.decimals$}–{hi:.decimals$}]")
}

pub fn matrix(args: &Args) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("matrix benchmark needs --release".into());
    }
    let models = args.models()?;
    let reps: usize = args.parsed("--reps", 3)?;
    let passes: usize = args.parsed("--passes", 2)?;
    let filter_task = args.value("--task");
    let cfgs = configs(filter_task);
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    let mut results: Vec<Vec<Value>> = vec![Vec::new(); cfgs.len()];
    for rep in 0..reps {
        let mut order: Vec<usize> = (0..cfgs.len()).collect();
        if rep % 2 == 1 {
            order.reverse();
        } else if rep == 2 {
            let mid = order.len() / 2;
            order.rotate_left(mid);
        }
        for idx in order {
            let c = cfgs[idx];
            eprintln!(
                "rep {}/{reps}: task={} provider={} threads={} batch={}",
                rep + 1,
                c.task,
                c.provider,
                c.threads,
                c.batch
            );
            let out = Command::new(&exe)
                .args(["bench-one", "--models"])
                .arg(&models)
                .args([
                    "--task",
                    c.task,
                    "--provider",
                    c.provider,
                    "--threads",
                    c.threads,
                    "--batch",
                    c.batch,
                    "--passes",
                    &passes.to_string(),
                ])
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!(
                    "bench-one failed for {} {} t={} b={}: {}",
                    c.task,
                    c.provider,
                    c.threads,
                    c.batch,
                    String::from_utf8_lossy(&out.stderr)
                ));
            }
            let stdout = String::from_utf8_lossy(&out.stdout);
            let line = stdout
                .lines()
                .find_map(|l| l.strip_prefix(MARK))
                .ok_or("missing RESULT line")?;
            let val: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
            results[idx].push(val);
        }
    }

    println!(
        "\n| Tarea | Lote | Proveedor | Hilos | Carga ms | 1ª inf ms | Decod ms/u | Preproc ms/u | Infer ms/u | u/s | RSS pico MiB | GPU antes/pico MiB | GPU % med/máx |"
    );
    println!("|---|---:|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (c, runs) in cfgs.iter().zip(&results) {
        let col = |k: &str| -> Vec<f64> { runs.iter().filter_map(|r| r[k].as_f64()).collect() };
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} / {} | {} / {} |",
            c.task,
            c.batch,
            c.provider,
            c.threads,
            format_med(&col("load_ms"), 0),
            format_med(&col("first_infer_ms"), 1),
            format_med(&col("decode_ms_per_item"), 2),
            format_med(&col("preprocess_ms_per_item"), 2),
            format_med(&col("infer_ms_per_item"), 2),
            format_med(&col("infer_rate"), 1),
            format_med(&col("rss_mb"), 0),
            format_med(&col("gpu_before_mib"), 0),
            format_med(&col("gpu_peak_mib"), 0),
            format_med(&col("gpu_util_mean"), 0),
            format_med(&col("gpu_util_max"), 0),
        );
    }
    Ok(())
}
