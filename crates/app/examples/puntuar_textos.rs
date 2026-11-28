//! Scores any list of descriptions against the images a `calibrar` run already embedded, so new
//! wordings can be compared without running the image model again.
//! Usage: cargo run -p app --release --example puntuar_textos -- <sample folder> <calibrar output folder> <models folder> <texts file, one per line>
//! Writes `textos.csv` (idx, asset, one cosine column per text) into the output folder.

// Writes only to a path checked by the guard (`ensure_writable`), never inside the sample folder.
#![allow(clippy::disallowed_methods)]

use catalog::{Catalog, Filter, Sort};
use fsguard::{LibraryRoot, ensure_writable};
use std::fmt::Write as _;
use std::path::PathBuf;
use vision::{Provider, SessionOptions, SigLip2Text, init_runtime};

fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [sample, out, models, texts] = args.as_slice() else {
        panic!("uso: puntuar_textos <carpeta de muestra> <salida de calibrar> <modelos> <textos>");
    };
    let roots = [LibraryRoot::new(sample).expect("carpeta de muestra")];
    let db = ensure_writable(&out.join("catalogo.sqlite"), &roots).unwrap();
    let cat = Catalog::open(&db).expect("catálogo de calibrar");
    let texts: Vec<String> = std::fs::read_to_string(texts)
        .expect("textos")
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect();

    let _env = init_runtime(Provider::Cpu, None).unwrap();
    let mut enc = SigLip2Text::load(models, &SessionOptions::default()).unwrap();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    let vecs = enc.embed_texts(&refs).unwrap();

    let mut csv = String::from("idx,asset");
    for t in &texts {
        write!(csv, ",\"{}\"", t.replace('"', "'")).unwrap();
    }
    csv.push('\n');
    let entries = cat.entries(&Filter::default(), Sort::PathAsc).unwrap();
    for (i, e) in entries.iter().enumerate() {
        let Some((_, img)) = cat.asset_embedding(e.asset).unwrap() else {
            continue;
        };
        write!(csv, "{i},{}", e.asset.0).unwrap();
        for v in &vecs {
            write!(csv, ",{:.4}", vision::cosine_similarity(v, &img)).unwrap();
        }
        csv.push('\n');
    }
    let p = ensure_writable(&out.join("textos.csv"), &roots).unwrap();
    std::fs::write(p.as_path(), csv.as_bytes()).unwrap();
    println!("{} textos x {} imágenes", texts.len(), entries.len());
}
