//! Speed of the same binary, images and parameters on the CPU and on DirectML. Each
//! configuration runs in its own process, several times, in alternating order.

use crate::Args;
use crate::probe::{GpuSampler, gpu_now, loaded_module, peak_working_set_bytes};
use ocr::engine::{Engine, LIMIT_SIDE_LEN, Line, Options, Provider, Shape, Timings};
use ocr::eval::{ImageRead, score};
use ocr::manifest::verify_models;
use ocr::metrics::percentile;
use ocr::select::Strategy;
use ocr::synth::{Fonts, Regime, Sample, bench_set};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};
use std::process::Command;
use std::time::{Duration, Instant};

/// Both recognizers on every line: the same work whatever the provider reads.
const EVERY_LINE: fn(&ocr::select::Reading) -> bool = |_| true;
const MARK: &str = "RESULT ";

fn regime_of(text: &str) -> Result<Regime, String> {
    match text {
        "fixed" => Ok(Regime::Fixed),
        "variable" => Ok(Regime::Variable),
        _ => Err(format!("--regime {text}: use fixed or variable")),
    }
}

struct Run {
    ms: f64,
    timings: Timings,
    lines: Vec<Line>,
}

fn run(engine: &Engine, sample: &Sample, shapes: &mut Vec<Shape>) -> Result<Run, String> {
    let start = Instant::now();
    let (lines, timings) = engine.read(&sample.image, EVERY_LINE, shapes)?;
    Ok(Run {
        ms: start.elapsed().as_secs_f64() * 1000.0,
        timings,
        lines,
    })
}

/// `[x, y, v6, korean]` per line, top to bottom, to compare providers line by line.
fn texts(lines: &[Line]) -> Value {
    let mut rows: Vec<(i64, i64, &str, &str)> = lines
        .iter()
        .map(|l| {
            (
                l.rect[1].round() as i64,
                l.rect[0].round() as i64,
                l.v6.text.as_str(),
                l.korean.as_ref().map_or("", |k| k.text.as_str()),
            )
        })
        .collect();
    rows.sort();
    json!(rows)
}

pub fn one(args: &Args) -> Result<(), String> {
    if cfg!(debug_assertions) {
        return Err("benchmarks need --release".into());
    }
    let models = args.models()?;
    let provider = args.parsed("--provider", Provider::Cpu)?;
    let regime_name = args.value("--regime").unwrap_or("fixed");
    let regime = regime_of(regime_name)?;
    let threads = match args.value("--threads") {
        None | Some("default") => None,
        Some(n) => Some(n.parse().map_err(|e| format!("--threads {n}: {e}"))?),
    };
    let rec_batch = args.parsed("--rec-batch", 6)?;
    let passes: usize = args.parsed("--passes", 2)?;
    let set = bench_set(regime, &Fonts::load()?);

    let gpu_before = gpu_now();
    let sampler = GpuSampler::start();
    let start = Instant::now();
    verify_models(&models).map_err(|e| e.join("; "))?;
    let verify_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let engine = Engine::load(
        &models,
        &Options {
            provider,
            intra_threads: threads,
            rec_batch,
            limit_side_len: LIMIT_SIDE_LEN,
            log: None,
        },
    )?;
    // `load` verifies the hashes again, now from the file cache.
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;

    let mut shapes = Vec::new();
    let first_pass: Vec<Run> = set
        .iter()
        .map(|s| run(&engine, s, &mut shapes))
        .collect::<Result<_, _>>()?;
    let distinct_rec: BTreeSet<Shape> = shapes.iter().copied().collect();

    let steady_from = sampler.as_ref().map_or(0, GpuSampler::taken);
    let mut steady = Vec::new();
    let mut unstable = 0;
    let start = Instant::now();
    for _ in 0..passes {
        for (sample, first) in set.iter().zip(&first_pass) {
            let again = run(&engine, sample, &mut Vec::new())?;
            unstable += usize::from(texts(&again.lines) != texts(&first.lines));
            steady.push(again);
        }
    }
    let steady_seconds = start.elapsed().as_secs_f64();
    let samples = sampler.map(GpuSampler::finish).unwrap_or_default();
    let during = &samples[steady_from.min(samples.len())..];

    let times = |runs: &[Run]| -> Vec<f64> { runs.iter().map(|r| r.ms).collect() };
    // The first image is the cold start; it is reported alone and left out of the first pass.
    let new_shapes = times(&first_pass[1..]);
    let warm = times(&steady);
    let share = |part: fn(&Timings) -> Duration| {
        let total: f64 = steady.iter().map(|r| r.ms).sum();
        let part: f64 = steady
            .iter()
            .map(|r| part(&r.timings).as_secs_f64() * 1000.0)
            .sum();
        (part / steady.len() as f64, 100.0 * part / total)
    };
    let reads: Vec<ImageRead> = set
        .iter()
        .zip(&first_pass)
        .map(|(sample, r)| ImageRead {
            category: sample.category,
            truth: &sample.lines,
            lines: r.lines.clone(),
        })
        .collect();
    let accuracy = score(&reads, Strategy::Both(ocr::select::RECOMMENDED));
    let exact: usize = accuracy.lang.values().map(|t| t.exact).sum();
    let truth_lines: usize = accuracy.lang.values().map(|t| t.lines).sum();
    let det_sizes: HashSet<_> = set.iter().map(|s| s.image.dimensions()).collect();

    let result = json!({
        "provider": provider.to_string(),
        "threads": threads.map_or("default".to_string(), |t| t.to_string()),
        "rec_batch": rec_batch,
        "regime": regime_name,
        "images": set.len(),
        "lines": first_pass.iter().map(|r| r.lines.len()).sum::<usize>(),
        "steady_images": steady.len(),
        "verify_ms": verify_ms,
        "load_ms": load_ms,
        "first_ms": first_pass[0].ms,
        "new_median": percentile(&new_shapes, 50.0),
        "new_p95": percentile(&new_shapes, 95.0),
        "new_rate": 1000.0 * new_shapes.len() as f64 / new_shapes.iter().sum::<f64>(),
        "median": percentile(&warm, 50.0),
        "p95": percentile(&warm, 95.0),
        "rate": steady.len() as f64 / steady_seconds,
        "det": share(|t| t.detect),
        "crop": share(|t| t.crop),
        "v6": share(|t| t.v6),
        "korean": share(|t| t.korean),
        "rss_mb": peak_working_set_bytes() as f64 / 1_048_576.0,
        "gpu_before_mib": gpu_before.map(|g| g.0),
        "gpu_peak_mib": samples.iter().map(|s| s.0).max(),
        "gpu_util_mean": during.iter().map(|s| f64::from(s.1)).sum::<f64>()
            / during.len().max(1) as f64,
        "gpu_util_max": during.iter().map(|s| s.1).max(),
        "mean_cer": accuracy.mean_cer(),
        "exact": exact,
        "truth_lines": truth_lines,
        "det_sizes": det_sizes.len(),
        "rec_shapes": distinct_rec.len(),
        "rec_shape_list": if distinct_rec.len() <= 8 { json!(distinct_rec) } else { json!([]) },
        "unstable": unstable,
        "directml": loaded_module("DirectML.dll"),
        "onnxruntime": loaded_module("onnxruntime.dll"),
        "texts": first_pass.iter().map(|r| texts(&r.lines)).collect::<Vec<_>>(),
    });
    println!("{MARK}{result}");
    Ok(())
}

#[derive(Clone, Copy)]
struct Config {
    provider: &'static str,
    threads: &'static str,
    rec_batch: &'static str,
    regime: &'static str,
}

fn configs() -> Vec<Config> {
    let mut all = Vec::new();
    for regime in ["fixed", "variable"] {
        for rec_batch in ["1", "6"] {
            for (provider, threads) in [
                ("cpu", "1"),
                ("cpu", "6"),
                ("cpu", "default"),
                ("dml", "default"),
            ] {
                all.push(Config {
                    provider,
                    threads,
                    rec_batch,
                    regime,
                });
            }
        }
    }
    all
}

fn number(result: &Value, key: &str) -> f64 {
    match &result[key] {
        // `(ms per image, % of the image time)` pairs.
        Value::Array(pair) => pair.first().and_then(Value::as_f64).unwrap_or(0.0),
        value => value.as_f64().unwrap_or(0.0),
    }
}

/// Median over the repetitions, with the smallest and largest.
fn spread(results: &[Value], key: &str, digits: usize) -> String {
    let values: Vec<f64> = results.iter().map(|r| number(r, key)).collect();
    format!(
        "{:.*} [{:.*}–{:.*}]",
        digits,
        percentile(&values, 50.0),
        digits,
        percentile(&values, 0.0),
        digits,
        percentile(&values, 100.0)
    )
}

fn print_table(header: &[&str], rows: Vec<Vec<String>>) {
    println!("\n| {} |", header.join(" | "));
    println!("|{}", "---|".repeat(header.len()));
    for row in rows {
        println!("| {} |", row.join(" | "));
    }
}

pub fn matrix(args: &Args) -> Result<(), String> {
    let models = args.models()?;
    let reps: usize = args.parsed("--reps", 3)?;
    let passes = args.value("--passes").unwrap_or("2");
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    println!(
        "CPU: AVX2 {}, FMA {}, AVX-512F {}, {} logical processors.",
        std::arch::is_x86_feature_detected!("avx2"),
        std::arch::is_x86_feature_detected!("fma"),
        std::arch::is_x86_feature_detected!("avx512f"),
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );
    println!(
        "GPU memory in use before the matrix (MiB, load %): {:?}",
        gpu_now()
    );

    let configs = configs();
    let mut results: Vec<Vec<Value>> = vec![Vec::new(); configs.len()];
    for rep in 0..reps {
        // Forward, backward, then starting halfway, so that no configuration always follows
        // the same one.
        let mut order: Vec<usize> = (0..configs.len()).collect();
        match rep % 3 {
            1 => order.reverse(),
            2 => order.rotate_left(configs.len() / 2 + 1),
            _ => {}
        }
        for index in order {
            let c = configs[index];
            let output = Command::new(&exe)
                .arg("bench")
                .arg("--models")
                .arg(&models)
                .args(["--provider", c.provider, "--threads", c.threads])
                .args(["--rec-batch", c.rec_batch, "--regime", c.regime])
                .args(["--passes", passes])
                .output()
                .map_err(|e| e.to_string())?;
            let text = String::from_utf8_lossy(&output.stdout);
            let result = text
                .lines()
                .find_map(|l| l.strip_prefix(MARK))
                .and_then(|json| serde_json::from_str::<Value>(json).ok());
            let Some(result) = result.filter(|_| output.status.success()) else {
                return Err(format!(
                    "{} {} batch {} {}: {} {}",
                    c.provider,
                    c.threads,
                    c.rec_batch,
                    c.regime,
                    output.status,
                    String::from_utf8_lossy(&output.stderr)
                ));
            };
            eprintln!(
                "rep {} {} threads={} batch={} {}: median {:.0} ms, new shapes {:.0} ms",
                rep + 1,
                c.provider,
                c.threads,
                c.rec_batch,
                c.regime,
                number(&result, "median"),
                number(&result, "new_median")
            );
            results[index].push(result);
        }
    }

    let label = |c: &Config| {
        vec![
            c.regime.to_string(),
            c.rec_batch.to_string(),
            if c.provider == "cpu" {
                format!("CPU, {} hilos", c.threads)
            } else {
                "DirectML".to_string()
            },
        ]
    };
    let rows = |cells: &dyn Fn(&[Value]) -> Vec<String>| -> Vec<Vec<String>> {
        configs
            .iter()
            .zip(&results)
            .map(|(c, r)| label(c).into_iter().chain(cells(r)).collect())
            .collect()
    };
    println!("\nMediana de {reps} procesos [mín–máx].");
    println!("\n### Régimen estable (formas ya vistas)");
    print_table(
        &[
            "Régimen",
            "Lote",
            "Proveedor",
            "mediana ms",
            "p95 ms",
            "imágenes/s",
        ],
        rows(&|r| {
            vec![
                spread(r, "median", 0),
                spread(r, "p95", 0),
                spread(r, "rate", 2),
            ]
        }),
    );
    println!("\n### Primera pasada (cada imagen por primera vez) y arranque");
    print_table(
        &[
            "Régimen",
            "Lote",
            "Proveedor",
            "carga ms",
            "primera inferencia ms",
            "mediana ms",
            "p95 ms",
            "imágenes/s",
        ],
        rows(&|r| {
            vec![
                spread(r, "load_ms", 0),
                spread(r, "first_ms", 0),
                spread(r, "new_median", 0),
                spread(r, "new_p95", 0),
                spread(r, "new_rate", 2),
            ]
        }),
    );
    println!("\n### Reparto por imagen (régimen estable, ms) y recursos");
    print_table(
        &[
            "Régimen",
            "Lote",
            "Proveedor",
            "detección",
            "recorte",
            "rec v6",
            "rec coreano",
            "RSS pico MiB",
            "GPU antes MiB",
            "GPU pico MiB",
            "GPU uso medio / máx %",
        ],
        rows(&|r| {
            vec![
                spread(r, "det", 0),
                spread(r, "crop", 0),
                spread(r, "v6", 0),
                spread(r, "korean", 0),
                spread(r, "rss_mb", 0),
                spread(r, "gpu_before_mib", 0),
                spread(r, "gpu_peak_mib", 0),
                format!(
                    "{} / {}",
                    spread(r, "gpu_util_mean", 0),
                    spread(r, "gpu_util_max", 0)
                ),
            ]
        }),
    );
    println!("\n### Trabajo realizado y exactitud");
    print_table(
        &[
            "Régimen",
            "Lote",
            "Proveedor",
            "imágenes (estables)",
            "líneas",
            "CER medio %",
            "líneas exactas",
            "tamaños de imagen",
            "formas de rec",
            "textos que cambian al repetir",
        ],
        rows(&|r| {
            let first = &r[0];
            let same_lines = r.iter().all(|v| v["lines"] == first["lines"]);
            let same_texts = r.iter().all(|v| v["texts"] == first["texts"]);
            let shapes = &first["rec_shape_list"];
            let shapes = if shapes.as_array().is_some_and(|s| !s.is_empty()) {
                format!("{} {shapes}", first["rec_shapes"])
            } else {
                first["rec_shapes"].to_string()
            };
            vec![
                format!("{} ({})", first["images"], first["steady_images"]),
                format!(
                    "{}{}",
                    first["lines"],
                    if same_lines { "" } else { " (varía)" }
                ),
                format!("{:.2}", 100.0 * number(first, "mean_cer")),
                format!("{}/{}", first["exact"], first["truth_lines"]),
                first["det_sizes"].to_string(),
                shapes,
                format!(
                    "{}{}",
                    r.iter().map(|v| number(v, "unstable")).sum::<f64>(),
                    if same_texts {
                        ""
                    } else {
                        " (difieren entre procesos)"
                    }
                ),
            ]
        }),
    );

    println!("\n### CPU frente a DirectML, línea a línea");
    for (c, dml) in configs.iter().zip(&results) {
        if c.provider != "dml" {
            continue;
        }
        let cpu = configs
            .iter()
            .position(|o| {
                o.provider == "cpu"
                    && o.threads == "default"
                    && o.rec_batch == c.rec_batch
                    && o.regime == c.regime
            })
            .map(|i| &results[i][0])
            .ok_or("no CPU run to compare with")?;
        compare(c, cpu, &dml[0]);
    }
    if let Some((_, dml)) = configs
        .iter()
        .zip(&results)
        .find(|(c, _)| c.provider == "dml")
    {
        println!(
            "\nDirectML.dll cargada: {}; onnxruntime.dll: {}",
            dml[0]["directml"], dml[0]["onnxruntime"]
        );
    }
    println!("GPU memory in use after the matrix: {:?}", gpu_now());
    Ok(())
}

fn compare(config: &Config, cpu: &Value, dml: &Value) {
    let empty = Vec::new();
    let images = |v: &Value| v["texts"].as_array().unwrap_or(&empty).clone();
    let (a, b) = (images(cpu), images(dml));
    let (mut lines, mut v6, mut korean, mut unmatched_images) = (0, 0, 0, 0);
    for (a, b) in a.iter().zip(&b) {
        let (a, b) = (
            a.as_array().unwrap_or(&empty),
            b.as_array().unwrap_or(&empty),
        );
        if a.len() != b.len() {
            unmatched_images += 1;
            continue;
        }
        for (a, b) in a.iter().zip(b) {
            lines += 1;
            v6 += usize::from(a[2] == b[2]);
            korean += usize::from(a[3] == b[3]);
        }
    }
    println!(
        "- {} lote {}: imágenes {} y {}; líneas {} y {}; imágenes con distinto número de líneas: \
         {unmatched_images}; texto idéntico v6 {v6}/{lines}, coreano {korean}/{lines}",
        config.regime,
        config.rec_batch,
        a.len(),
        b.len(),
        cpu["lines"],
        dml["lines"]
    );
}
