//! Synthetic retrieval verification (English and Spanish), YuNet check on synthetic images,
//! and CPU-vs-DirectML embedding agreement in separate processes.

use crate::Args;
use serde_json::{Value, json};
use std::process::Command;
use vision::synth::{Group, SynthItem, verification_set};
use vision::{
    Provider, SessionOptions, SigLip2, SigLip2Text, SigLip2Vision, YuNet, YuNetConfig,
    cosine_similarity,
};

const MARK: &str = "EMBED ";

pub fn retrieval(args: &Args) -> Result<(), String> {
    let models = args.models()?;
    let provider = args.parsed("--provider", Provider::Cpu)?;
    let opts = SessionOptions {
        provider,
        ..SessionOptions::default()
    };

    let items = verification_set();
    let mut siglip = SigLip2::load(&models, &opts)?;
    let img_refs: Vec<_> = items.iter().map(|i| &i.image).collect();
    let img_embs = siglip.embed_images(&img_refs)?;

    let prompts_en: Vec<&str> = items.iter().map(|i| i.prompt_en).collect();
    let prompts_es: Vec<&str> = items.iter().map(|i| i.prompt_es).collect();
    let txt_en = siglip.embed_texts(&prompts_en)?;
    let txt_es = siglip.embed_texts(&prompts_es)?;

    println!("=== Synthetic retrieval verification ({provider}) ===");
    println!(
        "Note: synthetic images verify pipeline correctness ONLY; they say NOTHING about real photos or manga."
    );
    report_lang("English", &items, &img_embs, &txt_en);
    report_lang("Spanish", &items, &img_embs, &txt_es);

    // YuNet on the same 20 synthetic images.
    let mut yunet = YuNet::load(&models, &opts)?;
    println!(
        "\n=== YuNet on {} synthetic images ({provider}) ===",
        items.len()
    );
    for thresh in [0.6f32, 0.3f32] {
        let cfg = YuNetConfig {
            score_threshold: thresh,
            ..YuNetConfig::default()
        };
        let mut total_faces = 0usize;
        let mut with_faces = Vec::new();
        for item in &items {
            let faces = yunet.detect(&item.image, cfg)?;
            if !faces.is_empty() {
                total_faces += faces.len();
                with_faces.push(format!(
                    "{} ({} faces, max score {:.3})",
                    item.id,
                    faces.len(),
                    faces[0].score
                ));
            }
        }
        println!(
            "  threshold {thresh:.1}: {total_faces} detections across {}/{} images{}",
            with_faces.len(),
            items.len(),
            if with_faces.is_empty() {
                String::new()
            } else {
                format!(" -> {}", with_faces.join(", "))
            }
        );
    }
    Ok(())
}

fn report_lang(lang: &str, items: &[SynthItem], img_embs: &[Vec<f32>], txt_embs: &[Vec<f32>]) {
    println!("\n--- {lang} ---");
    for (label, subset) in [
        ("All (20)", None),
        ("ShapeColor (12)", Some(Group::ShapeColor)),
        ("SolidColor (4)", Some(Group::SolidColor)),
        ("SceneMock (4)", Some(Group::SceneMock)),
    ] {
        let idxs: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| subset.is_none_or(|g| item.group == g))
            .map(|(i, _)| i)
            .collect();
        let (t2i_1, t2i_3, t2i_fails) =
            eval_direction(&idxs, txt_embs, img_embs, items, lang, "T->I");
        let (i2t_1, i2t_3, i2t_fails) =
            eval_direction(&idxs, img_embs, txt_embs, items, lang, "I->T");
        let n = idxs.len();
        println!(
            "  {label:<16} T->I top-1: {t2i_1:>2}/{n} ({:>5.1}%), top-3: {t2i_3:>2}/{n} | I->T top-1: {i2t_1:>2}/{n} ({:>5.1}%), top-3: {i2t_3:>2}/{n}",
            100.0 * t2i_1 as f64 / n as f64,
            100.0 * i2t_1 as f64 / n as f64,
        );
        if subset.is_none() {
            for f in t2i_fails {
                println!("    FAIL {f}");
            }
            for f in i2t_fails {
                println!("    FAIL {f}");
            }
        }
    }
}

fn eval_direction(
    idxs: &[usize],
    queries: &[Vec<f32>],
    targets: &[Vec<f32>],
    items: &[SynthItem],
    _lang: &str,
    dir: &str,
) -> (usize, usize, Vec<String>) {
    let mut top1 = 0usize;
    let mut top3 = 0usize;
    let mut fails = Vec::new();
    for &qi in idxs {
        let mut ranked: Vec<(usize, f32)> = idxs
            .iter()
            .map(|&ti| (ti, cosine_similarity(&queries[qi], &targets[ti])))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        let rank = ranked.iter().position(|&(ti, _)| ti == qi).unwrap();
        if rank == 0 {
            top1 += 1;
        } else {
            let (pred_i, pred_s) = ranked[0];
            let true_s = ranked[rank].1;
            fails.push(format!(
                "{dir} {} -> predicted {} ({pred_s:.4}) instead of {} ({true_s:.4}, rank {})",
                items[qi].id,
                items[pred_i].id,
                items[qi].id,
                rank + 1
            ));
        }
        if rank < 3 {
            top3 += 1;
        }
    }
    (top1, top3, fails)
}

/// Dumps vision and text embeddings on the requested provider as one JSON line (`EMBED {...}`).
pub fn embed_dump(args: &Args) -> Result<(), String> {
    let models = args.models()?;
    let provider = args.parsed("--provider", Provider::Cpu)?;
    let opts = SessionOptions {
        provider,
        ..SessionOptions::default()
    };
    let items = verification_set();
    let img_refs: Vec<_> = items.iter().map(|i| &i.image).collect();
    let prompts: Vec<&str> = items
        .iter()
        .map(|i| i.prompt_en)
        .chain(items.iter().map(|i| i.prompt_es))
        .collect();

    let mut vision = SigLip2Vision::load(&models, &opts)?;
    let img_embs = vision.embed_images(&img_refs)?;

    let text_res = SigLip2Text::load(&models, &opts).and_then(|mut t| t.embed_texts(&prompts));
    let (txt_embs, text_error) = match text_res {
        Ok(e) => (Some(e), None),
        Err(e) => (None, Some(e)),
    };

    println!(
        "{MARK}{}",
        json!({
            "provider": provider.to_string(),
            "images": img_embs,
            "texts": txt_embs,
            "text_error": text_error,
        })
    );
    Ok(())
}

fn run_dump(models: &std::path::Path, provider: &str) -> Result<Value, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = Command::new(exe)
        .args(["embed-dump", "--models"])
        .arg(models)
        .args(["--provider", provider])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "embed-dump {provider} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout
        .lines()
        .find_map(|l| l.strip_prefix(MARK))
        .ok_or_else(|| format!("embed-dump {provider} produced no {MARK} line"))?;
    serde_json::from_str(line).map_err(|e| e.to_string())
}

fn parse_vecs(v: &Value) -> Result<Vec<Vec<f32>>, String> {
    let arr = v.as_array().ok_or("expected JSON array")?;
    arr.iter()
        .map(|row| {
            row.as_array()
                .ok_or("expected inner array")?
                .iter()
                .map(|x| x.as_f64().map(|f| f as f32).ok_or("expected float"))
                .collect::<Result<Vec<f32>, _>>()
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn summarize_agreement(label: &str, a: &[Vec<f32>], b: &[Vec<f32>]) {
    let mut cosines: Vec<f32> = a
        .iter()
        .zip(b)
        .map(|(x, y)| cosine_similarity(x, y))
        .collect();
    let max_abs_diff = a
        .iter()
        .zip(b)
        .flat_map(|(x, y)| x.iter().zip(y).map(|(u, v)| (u - v).abs()))
        .fold(0.0f32, f32::max);
    let any_nan = b.iter().flatten().any(|x| !x.is_finite());
    cosines.sort_by(f32::total_cmp);
    let min = cosines[0];
    let median = cosines[cosines.len() / 2];
    let mean = cosines.iter().copied().sum::<f32>() / cosines.len() as f32;
    println!(
        "  {label:<22} n={:>2} | cosine min={min:.6}, median={median:.6}, mean={mean:.6} | max |diff|={max_abs_diff:.6} | non-finite={any_nan}",
        cosines.len()
    );
}

/// Spawns CPU and DirectML in separate processes and compares vision fp16 and text int8 embeddings.
pub fn agree(args: &Args) -> Result<(), String> {
    let models = args.models()?;
    let cpu = run_dump(&models, "cpu")?;
    let dml = run_dump(&models, "dml:0")?;

    println!("=== CPU vs DirectML (dml:0) embedding agreement ===");
    let cpu_img = parse_vecs(&cpu["images"])?;
    let dml_img = parse_vecs(&dml["images"])?;
    summarize_agreement("SigLIP2 vision fp16", &cpu_img, &dml_img);

    if let Some(err) = dml["text_error"].as_str() {
        println!("  SigLIP2 text int8      DirectML ERROR: {err}");
    } else {
        let cpu_txt = parse_vecs(&cpu["texts"])?;
        let dml_txt = parse_vecs(&dml["texts"])?;
        summarize_agreement("SigLIP2 text int8", &cpu_txt, &dml_txt);
    }
    Ok(())
}
