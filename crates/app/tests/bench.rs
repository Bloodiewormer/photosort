// Synthetic benchmark tests operate exclusively inside temp directories.
#![allow(clippy::disallowed_methods)]

use app::bench::{
    BENCH_QUERY_RUNS, BENCH_THUMB_COUNT, BenchConfig, DEFAULT_BENCH_COUNT, MAX_BENCH_COUNT,
    TargetStatus, build_unique_jpeg, build_unique_png, evaluate_target, format_bytes,
    format_duration, format_report, format_throughput, mean_ms, parse_bench_args, percentile_ms,
    run_bench_with_hook, validate_bench_count,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

type Snapshot = BTreeMap<
    PathBuf,
    (
        bool,
        u64,
        Option<SystemTime>,
        Option<SystemTime>,
        Option<String>,
    ),
>;

fn snapshot(root: &Path) -> Snapshot {
    WalkDir::new(root)
        .into_iter()
        .map(|e| {
            let e = e.unwrap();
            let md = e.metadata().unwrap();
            let hash = md
                .is_file()
                .then(|| scanner::sha256_file(e.path()).unwrap());
            (
                e.path().to_path_buf(),
                (
                    md.is_dir(),
                    md.len(),
                    md.modified().ok(),
                    md.created().ok(),
                    hash,
                ),
            )
        })
        .collect()
}

#[test]
fn parse_bench_args_handles_defaults_counts_and_errors() {
    let empty: [&str; 0] = [];
    assert_eq!(parse_bench_args(empty).unwrap(), None);
    assert_eq!(
        parse_bench_args(["--demo", "--theme", "dark"]).unwrap(),
        None
    );

    let default_cfg = parse_bench_args(["--bench"]).unwrap().expect("bench mode");
    assert_eq!(default_cfg.count, DEFAULT_BENCH_COUNT);
    assert_eq!(default_cfg.query_runs, BENCH_QUERY_RUNS);
    assert_eq!(default_cfg.thumb_count, BENCH_THUMB_COUNT);

    let custom = parse_bench_args(["--bench", "--bench-count", "5000"])
        .unwrap()
        .expect("bench mode");
    assert_eq!(custom.count, 5000);

    let max_ok = parse_bench_args(["--bench-count", &MAX_BENCH_COUNT.to_string(), "--bench"])
        .unwrap()
        .expect("bench mode");
    assert_eq!(max_ok.count, MAX_BENCH_COUNT);

    assert!(parse_bench_args(["--bench-count", "500"]).is_err());
    assert!(parse_bench_args(["--bench", "--bench-count"]).is_err());
    assert!(parse_bench_args(["--bench", "--bench-count", "0"]).is_err());
    assert!(
        parse_bench_args([
            "--bench",
            "--bench-count",
            &(MAX_BENCH_COUNT + 1).to_string()
        ])
        .is_err()
    );
    assert!(parse_bench_args(["--bench", "--bench-count", "abc"]).is_err());
    assert!(parse_bench_args(["--bench", "--unknown"]).is_err());

    assert_eq!(validate_bench_count(1).unwrap(), 1);
    assert_eq!(
        validate_bench_count(MAX_BENCH_COUNT).unwrap(),
        MAX_BENCH_COUNT
    );
    assert!(validate_bench_count(0).is_err());
    assert!(validate_bench_count(MAX_BENCH_COUNT + 1).is_err());
}

#[test]
fn percentile_mean_and_target_evaluation_are_accurate() {
    assert_eq!(mean_ms(&[]), 0.0);
    assert_eq!(percentile_ms(&[], 95.0), 0.0);

    let single = [Duration::from_millis(42)];
    assert!((mean_ms(&single) - 42.0).abs() < 1e-6);
    assert!((percentile_ms(&single, 95.0) - 42.0).abs() < 1e-6);

    // Unsorted 20 samples: 1..=20 ms -> mean = 10.5 ms, p50 = 10 ms, p95 = 19 ms, p100 = 20 ms.
    let mut samples: Vec<Duration> = (1..=20).map(Duration::from_millis).collect();
    samples.reverse();
    assert!((mean_ms(&samples) - 10.5).abs() < 1e-6);
    assert!((percentile_ms(&samples, 50.0) - 10.0).abs() < 1e-6);
    assert!((percentile_ms(&samples, 95.0) - 19.0).abs() < 1e-6);
    assert!((percentile_ms(&samples, 100.0) - 20.0).abs() < 1e-6);

    assert_eq!(evaluate_target(149.9, 150.0), TargetStatus::Pass);
    assert_eq!(evaluate_target(150.0, 150.0), TargetStatus::Warn);
    assert_eq!(evaluate_target(250.1, 250.0), TargetStatus::Warn);
}

#[test]
fn formatting_helpers_produce_readable_spanish_units() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(2048), "2.0 KB");
    assert_eq!(format_bytes(5 * 1024 * 1024), "5.00 MB");
    assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.00 GB");

    assert_eq!(format_duration(Duration::from_micros(500)), "0.50 ms");
    assert_eq!(format_duration(Duration::from_millis(42)), "42.00 ms");
    assert_eq!(format_duration(Duration::from_millis(2500)), "2.50 s");

    assert_eq!(
        format_throughput(1000, Duration::from_secs(2), "arch/s"),
        "500 arch/s"
    );
}

#[test]
fn synthetic_png_and_jpeg_variants_are_valid_and_distinct() {
    let png_a = build_unique_png(0, 24, 32).unwrap();
    let png_b = build_unique_png(1, 24, 32).unwrap();
    assert_ne!(png_a, png_b);
    let img_a = image::load_from_memory(&png_a).unwrap();
    let img_b = image::load_from_memory(&png_b).unwrap();
    assert_eq!((img_a.width(), img_a.height()), (24, 32));
    assert_eq!((img_b.width(), img_b.height()), (24, 32));

    let jpg_a = build_unique_jpeg(0, 32, 24).unwrap();
    let jpg_b = build_unique_jpeg(1, 32, 24).unwrap();
    assert_ne!(jpg_a, jpg_b);
    let jimg_a = image::load_from_memory(&jpg_a).unwrap();
    let jimg_b = image::load_from_memory(&jpg_b).unwrap();
    assert_eq!((jimg_a.width(), jimg_a.height()), (32, 24));
    assert_eq!((jimg_b.width(), jimg_b.height()), (32, 24));
}

#[test]
fn end_to_end_benchmark_200_runs_all_steps_and_preserves_originals() {
    let outer = tempfile::tempdir().unwrap();

    let mut baseline: Option<Snapshot> = None;
    let mut observed_steps: Vec<String> = Vec::new();
    let mut temp_root_seen: Option<PathBuf> = None;

    let cfg = BenchConfig {
        count: 200,
        query_runs: 5,
        thumb_count: 40,
        ..BenchConfig::default()
    };

    let report = run_bench_with_hook(cfg, Some(outer.path()), |step, lib_dir| {
        observed_steps.push(step.to_string());
        if temp_root_seen.is_none() {
            temp_root_seen = lib_dir.parent().map(Path::to_path_buf);
        }
        if step == "generacion" {
            // Settle NTFS lazy directory mtime once before capturing baseline snapshot.
            let _ = snapshot(lib_dir);
            baseline = Some(snapshot(lib_dir));
        } else {
            let expected = baseline
                .as_ref()
                .expect("baseline captured after generation");
            let current = snapshot(lib_dir);
            assert_eq!(
                expected, &current,
                "la biblioteca cambió tras el paso de solo lectura '{step}'"
            );
        }
    })
    .unwrap();

    // Every read-only phase must have been observed and verified.
    for required in [
        "generacion",
        "escaneo",
        "primera_indexacion",
        "segunda_indexacion",
        "reglas",
        "consultas",
        "miniaturas",
    ] {
        assert!(
            observed_steps.iter().any(|s| s == required),
            "faltó el paso '{required}' en {observed_steps:?}"
        );
    }

    assert_eq!(report.count, 200);
    assert_eq!(report.scan_files, 200);
    assert_eq!(report.first_index.new, 200);
    assert_eq!(report.first_index.hashed, 200);
    assert_eq!(report.second_index.unchanged, 200);
    assert_eq!(report.second_index.hashed, 0);
    assert!(report.rules_analyzed > 0);
    assert_eq!(report.rules_errors, 0);
    assert_eq!(report.queries.len(), 14);
    assert!(report.queries.iter().all(|q| q.runs == 5));
    assert_eq!(report.thumbs_generated, 40);
    assert_eq!(report.thumbs_errors, 0);
    assert!(report.library_bytes > 0);
    assert!(report.db_bytes > 0);
    assert!(report.duplicate_groups > 0);

    #[cfg(windows)]
    assert!(report.peak_rss_bytes.is_some_and(|b| b > 0));

    let rendered = format_report(&report);
    assert!(rendered.contains("Escaneo"));
    assert!(rendered.contains("Primera indexación"));
    assert!(rendered.contains("Segunda indexación"));
    assert!(rendered.contains("Cola de análisis"));
    assert!(rendered.contains("Miniaturas"));
    assert!(rendered.contains("PASS") || rendered.contains("WARN"));

    // Confirm nothing remains in the outer directory after the benchmark finishes and cleans up.
    let remaining: Vec<_> = std::fs::read_dir(outer.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert!(
        remaining.is_empty(),
        "quedaron archivos fuera o sin limpiar en el directorio temporal: {remaining:?}"
    );
}
