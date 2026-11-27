// Fixtures are created in temp dirs, never inside a real library root.
#![allow(clippy::disallowed_methods)]

use app::ocr_worker::{OcrEvent, OcrScope, OcrWorker};
use app::settings::OcrDevice;
use app::worker::{IndexEvent, IndexWorker};
use app::works_worker::{WorksEvent, WorksWorker};
use catalog::{Catalog, LabelId, LabelKind, PROMO_CATEGORY, Source};
use fsguard::{LibraryRoot, ensure_writable};
use ocr::synth::{Category, Fonts, Kind, Sample, Split, generate};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

/// Groups ground-truth promo-page title lines by their `title` slot into full title strings.
fn extract_promo_titles(sample: &Sample) -> Vec<String> {
    let mut by_slot: BTreeMap<usize, Vec<&str>> = BTreeMap::new();
    for line in &sample.lines {
        if line.kind == Kind::Title
            && let Some(slot) = line.title
        {
            by_slot.entry(slot).or_default().push(line.text.as_str());
        }
    }
    by_slot.into_values().map(|parts| parts.join(" ")).collect()
}

/// Real-engine end-to-end test on the held-out synthetic promo pages (`ocr::synth`).
/// Creates works from the held-out ground-truth titles PLUS distractor works from the Tune split,
/// runs real OCR and work matching, and reports recall and false positives.
#[test]
#[ignore = "necesita los modelos: ver CONTRIBUTING.md"]
fn real_engine_held_out_promo_pages_recall_and_false_positives() {
    let models_dir =
        PathBuf::from(std::env::var_os("PHOTOSORT_OCR_MODELS").expect("set PHOTOSORT_OCR_MODELS"));

    let fonts = Fonts::load().unwrap();
    let held_out_promos: Vec<Sample> = generate(Split::HeldOut, &fonts)
        .into_iter()
        .filter(|s| s.category == Category::Promo)
        .collect();
    let tune_promos: Vec<Sample> = generate(Split::Tune, &fonts)
        .into_iter()
        .filter(|s| s.category == Category::Promo)
        .collect();
    assert!(!held_out_promos.is_empty());
    assert!(!tune_promos.is_empty());

    let tmp = tempfile::tempdir().unwrap();
    let library = tmp.path().join("library");
    fs::create_dir_all(&library).unwrap();

    let mut page_paths = Vec::with_capacity(held_out_promos.len());
    let mut page_truth_titles: Vec<Vec<String>> = Vec::with_capacity(held_out_promos.len());
    for (idx, sample) in held_out_promos.iter().enumerate() {
        let path = library.join(format!("held_out_promo_{idx:02}.png"));
        sample.image.save(&path).unwrap();
        page_paths.push(path);
        page_truth_titles.push(extract_promo_titles(sample));
    }

    let root = LibraryRoot::new(&library).unwrap();
    let root_path = root.as_path().to_path_buf();
    let data = tmp.path().join("data");
    fs::create_dir_all(&data).unwrap();
    let db_path = data.join("catalog.sqlite");

    let db_index = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_main = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_ocr = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();
    let db_works = ensure_writable(&db_path, std::slice::from_ref(&root)).unwrap();

    // 1. Index held-out promo pages.
    let indexer = IndexWorker::start(db_index, vec![root], None);
    while let Ok(ev) = indexer.recv() {
        match ev {
            IndexEvent::Finished { stopped, new, .. } => {
                assert!(!stopped);
                assert_eq!(new, held_out_promos.len());
                break;
            }
            IndexEvent::Failed(e) => panic!("index failed: {e}"),
            _ => {}
        }
    }

    // 2. Create works from held-out ground-truth titles PLUS distractor works from Tune split.
    let cat = Catalog::open(&db_main).unwrap();
    let mut expected_by_page: Vec<HashSet<LabelId>> = Vec::with_capacity(held_out_promos.len());
    let mut all_held_out_ids: HashSet<LabelId> = HashSet::new();
    for titles in &page_truth_titles {
        let mut set = HashSet::new();
        for t in titles {
            let id = cat.create_work(t).unwrap();
            set.insert(id);
            all_held_out_ids.insert(id);
        }
        expected_by_page.push(set);
    }

    let mut distractor_ids: HashSet<LabelId> = HashSet::new();
    for sample in &tune_promos {
        for t in extract_promo_titles(sample) {
            let id = cat.create_work(&t).unwrap();
            if !all_held_out_ids.contains(&id) {
                distractor_ids.insert(id);
            }
        }
    }
    assert!(!distractor_ids.is_empty());

    // 3. Run real OCR engine (DirectML) on the held-out promo pages.
    let ocr = OcrWorker::start(
        db_ocr,
        OcrScope::Roots(vec![root_path]),
        models_dir,
        OcrDevice::DirectMl,
        None,
    );
    while let Ok(ev) = ocr.recv() {
        match ev {
            OcrEvent::Finished {
                stopped,
                processed,
                errors,
            } => {
                assert!(!stopped);
                assert_eq!(processed, held_out_promos.len());
                assert_eq!(errors, 0);
                break;
            }
            OcrEvent::Failed(e) => panic!("real OCR engine failed: {e}"),
            _ => {}
        }
    }

    // 4. Also exercise WorksWorker across the OCR'd assets (idempotent).
    let works_worker = WorksWorker::start(db_works, None);
    while let Ok(ev) = works_worker.recv() {
        match ev {
            WorksEvent::Finished {
                stopped, processed, ..
            } => {
                assert!(!stopped);
                assert_eq!(processed, held_out_promos.len());
                break;
            }
            WorksEvent::Failed(e) => panic!("works worker failed: {e}"),
            WorksEvent::Progress { .. } => {}
        }
    }

    // 5. Measure recall and false positives (print only aggregate numbers, never titles/OCR text).
    let mut total_expected = 0usize;
    let mut total_tp = 0usize;
    let mut total_fp = 0usize;
    let mut promo_detected = 0usize;
    let mut page_stats: HashMap<usize, (usize, usize, usize)> = HashMap::new();

    for (idx, path) in page_paths.iter().enumerate() {
        let asset = cat
            .asset_of(&dunce::canonicalize(path).unwrap())
            .unwrap()
            .unwrap();
        let labels = cat.labels_of(asset).unwrap();
        if labels
            .iter()
            .any(|a| a.kind == LabelKind::Category && a.name == PROMO_CATEGORY)
        {
            promo_detected += 1;
        }
        let suggested_entities: HashSet<LabelId> = labels
            .iter()
            .filter(|a| a.kind == LabelKind::Entity && a.source == Source::Ocr)
            .map(|a| a.label)
            .collect();

        let expected = &expected_by_page[idx];
        let tp = suggested_entities.intersection(expected).count();
        let fp = suggested_entities.difference(expected).count();
        total_expected += expected.len();
        total_tp += tp;
        total_fp += fp;
        page_stats.insert(idx, (tp, expected.len(), fp));
    }

    let recall = if total_expected == 0 {
        0.0
    } else {
        total_tp as f64 / total_expected as f64
    };
    eprintln!(
        "e2e_promo_works: pages={} works_total={} (held_out={} distractors={}) recall={}/{} ({:.1}%) false_positives={} promo_detected={}/{}",
        held_out_promos.len(),
        all_held_out_ids.len() + distractor_ids.len(),
        all_held_out_ids.len(),
        distractor_ids.len(),
        total_tp,
        total_expected,
        recall * 100.0,
        total_fp,
        promo_detected,
        held_out_promos.len()
    );

    assert!(total_tp > 0, "expected real OCR + matcher to find works");
    assert_eq!(promo_detected, held_out_promos.len());
}
