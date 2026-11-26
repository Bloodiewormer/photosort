use super::*;
use image::RgbImage;
use ocr::synth::{Category, Fonts, Split, generate};
use std::collections::BTreeMap;

fn args(text: &[&str]) -> Vec<String> {
    text.iter().map(|s| s.to_string()).collect()
}

#[test]
fn arguments_are_checked_and_their_values_never_echoed() {
    let base = ["--folder", "a", "--out", "b", "--models", "c"];
    let config = parse(&args(&base)).unwrap();
    assert_eq!(config.count, 150);
    assert_eq!(config.provider, Provider::DirectMl { device: 0 });
    for extra in [["--count", "201"], ["--count", "0"]] {
        assert!(parse(&args(&[&base[..], &extra].concat())).is_err());
    }
    for extra in [["--seed", "secreto"], ["--secreto", "1"]] {
        let error = parse(&args(&[&base[..], &extra].concat())).err().unwrap();
        assert!(!error.contains("secreto"), "{error}");
    }
    assert!(parse(&args(&base[2..])).is_err(), "no folder");
}

#[test]
fn output_inside_or_around_a_folder_is_refused_without_naming_it() {
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("secreto");
    make_folder(&library.join("sub"));
    let folders = vec![library.clone()];
    for out in [
        library.clone(),
        library.join("salida"),
        temp.path().to_path_buf(),
    ] {
        let error = guard_out(&folders, &out).err().unwrap();
        assert!(!error.contains("secreto"), "{error}");
    }
    assert!(guard_out(&folders, &temp.path().join("salida")).is_ok());
}

#[test]
fn the_sample_is_deterministic_and_shared_between_folders() {
    let folder = |name: &str, n: usize| -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("{name}/{i}.png")))
            .collect()
    };
    let all = || vec![folder("a", 50), folder("b", 50), folder("c", 3)];
    let sample = pick(all(), 23, 7);
    assert_eq!(sample, pick(all(), 23, 7));
    assert_ne!(sample, pick(all(), 23, 8));
    let from = |name: &str| sample.iter().filter(|p| p.starts_with(name)).count();
    assert_eq!((from("a"), from("b"), from("c")), (10, 10, 3));
    // Fewer files than asked for, and the same folder given twice: each file once.
    assert_eq!(pick(vec![folder("a", 4), folder("a", 4)], 150, 1).len(), 4);
}

#[test]
fn text_from_an_image_cannot_become_markup() {
    let hostile = r#"</span><img src=x onerror="alert('x')">&"#;
    let entries = [report::Entry {
        name: hostile.into(),
        outcome: Ok(report::Image {
            thumb: "a.jpg".into(),
            width: 100,
            height: 100,
            ms: 1.0,
            lines: vec![report::Line {
                rect: [1.0, 1.0, 9.0, 9.0],
                text: hostile.into(),
                score: 0.9,
                korean: false,
            }],
        }),
    }];
    let summary = report::Summary::of(&entries, "cpu".into());
    let page = report::render(&entries, &summary, REVIEW_BELOW, 1);
    assert!(!page.contains("<img src=x"));
    assert!(!page.contains("alert('x')"));
    assert!(page.contains("&lt;img src=x onerror=&quot;alert(&#39;x&#39;)&quot;&gt;&amp;"));
    assert!(page.contains("default-src 'none'"));
    assert!(!summary.to_string().contains("alert"));
}

#[allow(clippy::disallowed_methods)] // test fixtures in a temporary folder
fn make_folder(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
}

#[allow(clippy::disallowed_methods)] // test fixtures in a temporary folder
fn save(image: &RgbImage, path: &Path) {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    std::fs::write(path, bytes.into_inner()).unwrap();
}

/// Every file and folder below `root`, with what must not change.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Option<(String, u64, SystemTime, SystemTime)>> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .map(|entry| {
            let path = entry.unwrap().into_path();
            let state = path.is_file().then(|| {
                let f = fingerprint(&path).unwrap();
                (f.sha256, f.size, f.modified.unwrap(), f.created.unwrap())
            });
            (path, state)
        })
        .collect()
}

#[test]
fn a_whole_run_reads_synthetic_images_without_touching_them_or_printing_their_text() {
    let Some(models) = std::env::var_os("PHOTOSORT_OCR_MODELS") else {
        eprintln!("skipped: PHOTOSORT_OCR_MODELS is not set");
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let library = temp.path().join("biblioteca-secreta");
    make_folder(&library);
    let samples = generate(Split::Tune, &Fonts::load().unwrap());
    let sample = samples
        .iter()
        .find(|s| s.category == Category::Latin)
        .unwrap();
    save(&sample.image, &library.join("foto-privada.png"));
    save(&RgbImage::new(64, 64), &library.join("vacia-privada.png"));
    make_folder(&library.join("sub"));
    #[allow(clippy::disallowed_methods)] // a corrupt fixture in a temporary folder
    std::fs::write(library.join("sub/rota-privada.jpg"), b"not an image").unwrap();

    snapshot(&library); // NTFS updates directory metadata lazily.
    let before = snapshot(&library);
    let out = temp.path().join("salida");
    let config = Config {
        folders: vec![library.clone()],
        out: out.clone(),
        models: models.into(),
        count: 150,
        seed: 1,
        provider: Provider::Cpu,
    };
    let mut console = Vec::new();
    run(&config, &mut console).unwrap();
    assert_eq!(snapshot(&library), before);

    let console = String::from_utf8(console).unwrap();
    assert!(
        console.contains("muestra 3; leídas 2; con error 1"),
        "{console}"
    );
    assert!(!console.contains("privada"), "{console}");
    assert!(!console.contains("secreta"), "{console}");
    let page = std::fs::read_to_string(out.join("report.html")).unwrap();
    assert!(page.contains("foto-privada.png"));
    let words = sample
        .lines
        .iter()
        .flat_map(|line| line.text.split_whitespace())
        .filter(|word| word.len() > 3);
    let mut recognised = 0;
    for word in words {
        assert!(!console.contains(word), "{word}");
        recognised += usize::from(page.contains(word));
    }
    assert!(recognised > 0, "the report has none of the synthetic text");
    assert_eq!(std::fs::read_dir(out.join("thumbs")).unwrap().count(), 2);

    let error = run(&config, &mut Vec::new()).unwrap_err();
    assert!(error.contains("already has a report"), "{error}");
}
