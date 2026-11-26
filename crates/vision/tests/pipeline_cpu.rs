mod common;

use vision::siglip2::{EOS_TOKEN_ID, MAX_TEXT_TOKENS, PAD_TOKEN_ID};
use vision::synth::{Group, verification_set};
use vision::{SessionOptions, SigLip2, YuNet, YuNetConfig, cosine_similarity};

#[test]
fn tokenizer_lowercases_appends_eos_pads_and_truncates() {
    let Some(models) = common::vision_models_dir() else {
        return;
    };
    let text = vision::SigLip2Text::load(&models, &SessionOptions::default()).unwrap();

    let lower = text.tokenize("a red circle on a white background").unwrap();
    let upper = text.tokenize("A RED CIRCLE ON A WHITE BACKGROUND").unwrap();
    assert_eq!(
        lower, upper,
        "do_lower_case must fold case before tokenization"
    );

    // Must contain EOS followed exclusively by PAD tokens.
    let eos_pos = lower
        .iter()
        .position(|&id| id == EOS_TOKEN_ID)
        .expect("EOS token missing");
    assert!(eos_pos > 0 && eos_pos < MAX_TEXT_TOKENS);
    assert!(lower[eos_pos + 1..].iter().all(|&id| id == PAD_TOKEN_ID));

    // Long text (> 64 tokens) must truncate to 63 tokens + EOS at index 63.
    let long = "red blue green yellow circle square triangle ".repeat(30);
    let truncated = text.tokenize(&long).unwrap();
    assert_eq!(truncated.len(), MAX_TEXT_TOKENS);
    assert_eq!(truncated[MAX_TEXT_TOKENS - 1], EOS_TOKEN_ID);
    assert!(
        truncated[..MAX_TEXT_TOKENS - 1]
            .iter()
            .all(|&id| id != PAD_TOKEN_ID && id != EOS_TOKEN_ID)
    );
}

#[test]
fn siglip2_synthetic_shape_color_retrieval_and_yunet_smoke() {
    let Some(models) = common::vision_models_dir() else {
        return;
    };
    let opts = SessionOptions::default();
    let mut siglip = SigLip2::load(&models, &opts).unwrap();
    let items = verification_set();
    let shape_items: Vec<_> = items
        .iter()
        .filter(|i| i.group == Group::ShapeColor)
        .collect();
    let imgs: Vec<_> = shape_items.iter().map(|i| &i.image).collect();
    let prompts_en: Vec<_> = shape_items.iter().map(|i| i.prompt_en).collect();

    let img_embs = siglip.embed_images(&imgs).unwrap();
    let txt_embs = siglip.embed_texts(&prompts_en).unwrap();
    assert_eq!(img_embs.len(), 12);
    assert_eq!(txt_embs.len(), 12);
    for emb in img_embs.iter().chain(txt_embs.iter()) {
        assert_eq!(emb.len(), vision::siglip2::EMBEDDING_DIM);
        let norm: f32 = emb.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "expected unit norm, got {norm}");
    }

    // Text -> Image top-1 on the 12 shape x color grid (chance is 1/12 = 8.3%).
    let mut hits = 0usize;
    for (ti, te) in txt_embs.iter().enumerate() {
        let best = img_embs
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| cosine_similarity(te, a).total_cmp(&cosine_similarity(te, b)))
            .unwrap()
            .0;
        if best == ti {
            hits += 1;
        }
    }
    assert!(
        hits >= 8,
        "shape x color text->image top-1 too low ({hits}/12): preprocessing or tokenization is likely wrong"
    );

    // YuNet smoke on a synthetic image (must run cleanly and return valid coordinates if any).
    let mut yunet = YuNet::load(&models, &opts).unwrap();
    let faces = yunet
        .detect(&items[0].image, YuNetConfig::default())
        .unwrap();
    for f in faces {
        assert!(f.bbox[0] < f.bbox[2] && f.bbox[1] < f.bbox[3]);
        assert!((0.0..=1.0).contains(&f.score));
    }
}
