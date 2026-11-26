//! Tunes the selection thresholds on one set and reports on the held-out one, as Markdown.

use ocr::engine::{Engine, Options, Provider};
use ocr::eval::{ImageRead, Score, chosen, read_text, score, tune};
use ocr::metrics::{Tally, assign_to_lines, normalize};
use ocr::select::{RECOMMENDED, Source, Strategy, Thresholds, choose, is_hangul, needs_korean};
use ocr::synth::{Category, Fonts, Lang, Sample, Split, generate};
use std::path::Path;
use std::time::Duration;

const SHARES: [f32; 4] = [0.2, 0.5, 0.8, 1.0];
const MARGINS: [f32; 7] = [-0.1, 0.0, 0.05, 0.1, 0.2, 0.4, 1.0];
const CASCADE: [f32; 8] = [0.6, 0.7, 0.8, 0.85, 0.9, 0.95, 0.98, 1.01];

struct Readings<'a> {
    images: Vec<ImageRead<'a>>,
    v6: Duration,
    korean: Duration,
    lines: usize,
}

fn read<'a>(engine: &Engine, samples: &'a [Sample]) -> Result<Readings<'a>, String> {
    let mut out = Readings {
        images: Vec::new(),
        v6: Duration::ZERO,
        korean: Duration::ZERO,
        lines: 0,
    };
    for sample in samples {
        let (lines, timings) = engine.read(&sample.image, |_| true, &mut Vec::new())?;
        out.v6 += timings.v6;
        out.korean += timings.korean;
        out.lines += lines.len();
        out.images.push(ImageRead {
            category: sample.category,
            truth: &sample.lines,
            lines,
        });
    }
    Ok(out)
}

/// The values each threshold takes among the combinations that tie.
fn print_ranges(tied: &[Strategy]) {
    let of = |pick: fn(&Thresholds) -> f32| -> Vec<f32> {
        let mut values: Vec<f32> = tied
            .iter()
            .filter_map(|s| match s {
                Strategy::Both(t) | Strategy::Cascade(t) => Some(pick(t)),
                Strategy::V6Only => None,
            })
            .collect();
        values.sort_by(f32::total_cmp);
        values.dedup();
        values
    };
    println!(
        "  Valores entre los empates: H {:?}, M {:?}, T {:?}.",
        of(|t| t.hangul_share),
        of(|t| t.margin),
        of(|t| t.cascade_below)
    );
}

fn percent(part: usize, whole: usize) -> String {
    format!("{:.1} %", 100.0 * part as f64 / whole.max(1) as f64)
}

fn cell(tally: Option<&Tally>) -> String {
    tally.map_or("—".into(), |t| {
        format!("{:.2} % / {:.1} %", 100.0 * t.cer(), 100.0 * t.exact_rate())
    })
}

fn table(header: &[&str], rows: Vec<Vec<String>>) {
    println!("\n| {} |", header.join(" | "));
    println!("|{}", "---|".repeat(header.len()));
    for row in rows {
        println!("| {} |", row.join(" | "));
    }
}

fn name(strategy: Strategy) -> String {
    match strategy {
        Strategy::V6Only => "S0".into(),
        Strategy::Both(t) => format!("S1 (H={}, M={})", t.hangul_share, t.margin),
        Strategy::Cascade(t) => format!(
            "S2 (H={}, M={}, T={})",
            t.hangul_share, t.margin, t.cascade_below
        ),
    }
}

fn report(title: &str, readings: &Readings, strategies: [Strategy; 3]) {
    println!("\n## {title}");
    let scores: Vec<Score> = strategies
        .iter()
        .map(|&s| score(&readings.images, s))
        .collect();
    let header = ["Medida (CER / líneas exactas)", "S0", "S1", "S2"];
    let rows = |label: String, of: &dyn Fn(&Score) -> String| -> Vec<String> {
        std::iter::once(label)
            .chain(scores.iter().map(of))
            .collect()
    };

    let langs = [Lang::Latin, Lang::Japanese, Lang::Chinese, Lang::Korean];
    let mut by_lang: Vec<Vec<String>> = langs
        .iter()
        .map(|l| rows(format!("{l:?}"), &|s| cell(s.lang.get(l))))
        .collect();
    by_lang.push(rows("Latin sin distinguir mayúsculas".into(), &|s| {
        cell(Some(&s.latin_folded))
    }));
    by_lang.push(rows("CER medio de los 4 idiomas".into(), &|s| {
        format!("{:.2} %", 100.0 * s.mean_cer())
    }));
    table(&header, by_lang);

    let categories: Vec<Category> = scores[0].category.keys().copied().collect();
    let by_category = categories
        .iter()
        .map(|c| rows(format!("{c:?}"), &|s| cell(s.category.get(c))))
        .collect();
    table(&header, by_category);

    let mixed = scores[0]
        .category_lang
        .keys()
        .copied()
        .filter(|(c, _)| {
            matches!(
                c,
                Category::Stylized | Category::Small | Category::Rotated | Category::Promo
            )
        })
        .map(|key| {
            rows(format!("{:?} · {:?}", key.0, key.1), &|s| {
                cell(s.category_lang.get(&key))
            })
        })
        .collect();
    table(&header, mixed);

    println!("\nDetección (IoU ≥ 0.5; no depende de la estrategia):");
    table(
        &[
            "Categoría",
            "Recall",
            "Precisión",
            "Líneas con alguna caja",
            "Cajas / verdad",
        ],
        scores[0]
            .detection
            .iter()
            .map(|(c, b)| {
                vec![
                    format!("{c:?}"),
                    percent(b.matched, b.truth),
                    percent(b.matched, b.predicted),
                    percent(b.found, b.truth),
                    format!("{} / {}", b.predicted, b.truth),
                ]
            })
            .collect(),
    );

    let mut promo: Vec<Vec<String>> = scores[0]
        .titles
        .keys()
        .map(|l| {
            rows(format!("Títulos {l:?}"), &|s| {
                let t = s.titles[l];
                format!("{}/{} ({})", t.found, t.total, percent(t.found, t.total))
            })
        })
        .collect();
    promo.push(rows("Títulos, total".into(), &|s| {
        let found: usize = s.titles.values().map(|t| t.found).sum();
        let total: usize = s.titles.values().map(|t| t.total).sum();
        format!("{found}/{total} ({})", percent(found, total))
    }));
    promo.push(rows("Insignias".into(), &|s| {
        format!("{}/{}", s.badges.found, s.badges.total)
    }));
    promo.push(rows(
        "Sin texto: imágenes con texto inventado".into(),
        &|s| format!("{}/{}", s.no_text.images_with_text, s.no_text.images),
    ));
    promo.push(rows("Sin texto: lecturas / caracteres".into(), &|s| {
        format!("{} / {}", s.no_text.readings, s.no_text.chars)
    }));
    promo.push(rows(
        "Lecturas sueltas en imágenes con texto".into(),
        &|s| s.stray.to_string(),
    ));
    table(&["Medida", "S0", "S1", "S2"], promo);

    let ms = |d: Duration| d.as_secs_f64() * 1000.0 / readings.lines.max(1) as f64;
    println!(
        "\nCosto (esta ejecución): {} líneas detectadas; v6 {:.1} ms/línea; coreano {:.1} ms/línea.",
        readings.lines,
        ms(readings.v6),
        ms(readings.korean)
    );
    for (strategy, s) in strategies.iter().zip(&scores) {
        println!(
            "- {}: {} reconocimientos coreanos ({}), ≈ +{:.0} % sobre el tiempo de v6",
            name(*strategy),
            s.korean_runs,
            percent(s.korean_runs, s.lines_detected),
            100.0 * ms(readings.korean) * s.korean_runs as f64
                / (ms(readings.v6) * s.lines_detected as f64)
        );
    }
}

/// What v6 does with Korean text decides whether a cascade on its confidence is safe.
fn hangul_record(title: &str, readings: &Readings, strategies: [Strategy; 3]) {
    let mut records = Vec::new();
    let mut stolen = [0usize; 2];
    let mut others = 0;
    for image in &readings.images {
        let truth: Vec<_> = image.truth.iter().map(|l| l.rect).collect();
        let found: Vec<_> = image.lines.iter().map(|l| l.rect).collect();
        let (assigned, _) = assign_to_lines(&truth, &found);
        for (line, parts) in image.truth.iter().zip(assigned) {
            for p in parts {
                let read = &image.lines[p];
                if line.lang == Lang::Korean {
                    records.push((line.text.as_str(), image.category, read));
                } else {
                    others += 1;
                    for (count, strategy) in stolen.iter_mut().zip(&strategies[1..]) {
                        let source = choose(*strategy, &read.v6, read.korean.as_ref());
                        *count += usize::from(source == Source::Korean);
                    }
                }
            }
        }
    }
    println!("\n### v6 ante líneas coreanas — {title}");
    let cjk = |text: &str| {
        text.chars()
            .any(|c| c.is_alphabetic() && !c.is_ascii() && !is_hangul(c) && c > '\u{2E7F}')
    };
    let total = records.len();
    let empty = records
        .iter()
        .filter(|r| r.2.v6.text.trim().is_empty())
        .count();
    let invented = records.iter().filter(|r| cjk(&r.2.v6.text)).count();
    let mut scores: Vec<f64> = records.iter().map(|r| f64::from(r.2.v6.score)).collect();
    scores.sort_by(f64::total_cmp);
    let at = |p| ocr::metrics::percentile(&scores, p);
    println!(
        "{total} cajas coreanas; v6 vacío en {empty}; con kana o ideogramas en {invented}. \
         Puntuación de v6: mín {:.2}, mediana {:.2}, p90 {:.2}, p99 {:.2}, máx {:.2}.",
        at(0.0),
        at(50.0),
        at(90.0),
        at(99.0),
        at(100.0)
    );
    for threshold in [0.6, 0.7, 0.8, 0.85, 0.9, 0.95] {
        let confident = records.iter().filter(|r| r.2.v6.score >= threshold);
        let all = confident.clone().count();
        let with_cjk = confident.filter(|r| cjk(&r.2.v6.text)).count();
        println!(
            "- puntuación ≥ {threshold}: {all} cajas ({}), {with_cjk} de ellas con kana/ideogramas",
            percent(all, total)
        );
    }
    println!(
        "Cajas no coreanas en las que gana el coreano: S1 {} y S2 {} de {others}.",
        stolen[0], stolen[1]
    );
    records.sort_by(|a, b| b.2.v6.score.total_cmp(&a.2.v6.score));
    table(
        &[
            "Verdad",
            "Categoría",
            "v6",
            "puntuación v6",
            "coreano",
            "puntuación",
        ],
        records
            .iter()
            .take(8)
            .map(|(truth, category, read)| {
                let korean = read.korean.as_ref();
                vec![
                    truth.to_string(),
                    format!("{category:?}"),
                    read.v6.text.clone(),
                    format!("{:.2}", read.v6.score),
                    korean.map_or(String::new(), |k| k.text.clone()),
                    korean.map_or(String::new(), |k| format!("{:.2}", k.score)),
                ]
            })
            .collect(),
    );
}

fn failures(readings: &Readings, strategy: Strategy) {
    println!(
        "\n## Peores lecturas del conjunto reservado, {}",
        name(strategy)
    );
    let mut worst: Vec<(Category, f64, &str, String)> = Vec::new();
    let mut invented = Vec::new();
    for image in &readings.images {
        if image.category == Category::NoText {
            invented.extend(
                image
                    .lines
                    .iter()
                    .map(|l| chosen(strategy, l))
                    .filter(|r| !r.text.trim().is_empty()),
            );
            continue;
        }
        let truth: Vec<_> = image.truth.iter().map(|l| l.rect).collect();
        let found: Vec<_> = image.lines.iter().map(|l| l.rect).collect();
        let (assigned, _) = assign_to_lines(&truth, &found);
        for (line, parts) in image.truth.iter().zip(assigned) {
            let read = read_text(strategy, line.lang, &parts, &image.lines);
            let mut tally = Tally::default();
            tally.add(&line.text, &read, false);
            if normalize(&line.text) != normalize(&read) {
                worst.push((image.category, tally.cer(), &line.text, read));
            }
        }
    }
    worst.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)));
    let mut rows = Vec::new();
    for category in [
        Category::Latin,
        Category::Japanese,
        Category::Chinese,
        Category::Korean,
        Category::Stylized,
        Category::Small,
        Category::Rotated,
        Category::Vertical,
        Category::Promo,
    ] {
        let of: Vec<_> = worst.iter().filter(|w| w.0 == category).collect();
        println!("- {category:?}: {} líneas no exactas", of.len());
        // The worst two and two typical ones.
        let typical = of.iter().skip(of.len() / 2).take(2);
        rows.extend(of.iter().take(2).chain(typical).map(|w| {
            vec![
                format!("{category:?}"),
                w.2.to_string(),
                w.3.clone(),
                format!("{:.0} %", 100.0 * w.1),
            ]
        }));
    }
    table(&["Categoría", "Verdad", "Leído", "CER"], rows);
    let examples: Vec<String> = invented
        .iter()
        .take(10)
        .map(|r| format!("`{}` ({:.2})", r.text, r.score))
        .collect();
    println!("\nTexto inventado (ejemplos): {}", examples.join(", "));
}

pub fn run(models: &Path, provider: Provider, limit_side_len: u32) -> Result<(), String> {
    let fonts = Fonts::load()?;
    let (tuning, held_out) = (
        generate(Split::Tune, &fonts),
        generate(Split::HeldOut, &fonts),
    );
    let engine = Engine::load(
        models,
        &Options {
            provider,
            limit_side_len,
            ..Options::default()
        },
    )?;
    println!("Proveedor {provider}, limit_side_len {limit_side_len}.");
    let tune_readings = read(&engine, &tuning)?;
    let held_readings = read(&engine, &held_out)?;

    let thresholds = |hangul_share, margin, cascade_below| Thresholds {
        hangul_share,
        margin,
        cascade_below,
    };
    let (mut both, mut cascade) = (Vec::new(), Vec::new());
    for h in SHARES {
        for m in MARGINS {
            both.push(Strategy::Both(thresholds(h, m, 0.0)));
            cascade.extend(CASCADE.map(|t| Strategy::Cascade(thresholds(h, m, t))));
        }
    }
    let mut best = Vec::new();
    for candidates in [&both, &cascade] {
        let tuned = tune(&tune_readings.images, candidates).ok_or("no candidates")?;
        let strategy = tuned.best;
        println!(
            "Ajuste: {} con CER medio {:.3} % ({} de {} combinaciones empatan).",
            name(strategy),
            100.0 * tuned.cer,
            tuned.tied.len(),
            candidates.len()
        );
        print_ranges(&tuned.tied);
        if let Strategy::Cascade(chosen) = strategy {
            println!("\nSensibilidad de S2 a T (mismo H y M), conjunto de ajuste:");
            for t in CASCADE {
                let s = Strategy::Cascade(Thresholds {
                    cascade_below: t,
                    ..chosen
                });
                let result = score(&tune_readings.images, s);
                let asked = tune_readings
                    .images
                    .iter()
                    .flat_map(|i| &i.lines)
                    .filter(|l| needs_korean(s, &l.v6))
                    .count();
                println!(
                    "- T={t}: CER medio {:.3} %, coreano en {asked} de {} líneas",
                    100.0 * result.mean_cer(),
                    tune_readings.lines
                );
            }
        }
        best.push(strategy);
    }
    let strategies = [Strategy::V6Only, best[0], best[1]];
    for s in strategies {
        println!("- {}", name(s));
    }
    report("Conjunto de ajuste", &tune_readings, strategies);
    report("Conjunto reservado", &held_readings, strategies);
    hangul_record("ajuste", &tune_readings, strategies);
    hangul_record("reservado", &held_readings, strategies);
    let recommended = Strategy::Both(RECOMMENDED);
    println!("\n## Estrategia recomendada: {}", name(recommended));
    for (set, readings) in [("ajuste", &tune_readings), ("reservado", &held_readings)] {
        let result = score(&readings.images, recommended);
        let found: usize = result.titles.values().map(|t| t.found).sum();
        let total: usize = result.titles.values().map(|t| t.total).sum();
        println!(
            "- {set}: CER medio {:.3} %, títulos {found}/{total}, insignias {}/{}",
            100.0 * result.mean_cer(),
            result.badges.found,
            result.badges.total
        );
    }
    failures(&held_readings, recommended);
    Ok(())
}
