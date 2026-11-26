use app::demo::{self, DemoConfig};
use app::model::{ChipMode, FilterState, GridLayout, Selection};
use app::screenshot::{self, ScreenshotController};
use app::settings::{self, Settings};
use app::ui::PhotosortApp;
use app::worker::{
    IndexEvent, IndexWorker, PreviewWorker, ThumbPool, ThumbRequest, ThumbnailProvider,
};
use catalog::{Catalog, LabelKind, Sort, Source};
use fsguard::{DestPath, ensure_writable};
use image::{ImageBuffer, ImageFormat, Rgba};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

fn main() -> ExitCode {
    let args = match screenshot::parse_args(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if args.help {
        println!("{}", screenshot::usage_text());
        return ExitCode::SUCCESS;
    }

    if args.self_test {
        return if run_self_test() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let _demo_guard;
    let data_dir = if args.demo {
        match demo::prepare_demo(
            args.data_dir.as_deref(),
            DemoConfig {
                count: args.demo_count,
                seed: demo::DEFAULT_SEED,
            },
            args.theme,
        ) {
            Ok(setup) => {
                let dir = setup.data_dir.clone();
                _demo_guard = Some(setup);
                dir
            }
            Err(e) => {
                eprintln!("No se pudo preparar el modo demo: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        _demo_guard = None;
        settings::resolve_data_dir(args.data_dir.as_deref())
    };

    let (screenshot_ctrl, screenshot_handle) = if let Some(out_path) = &args.screenshot {
        let roots = settings::load(&data_dir).settings.valid_roots();
        let dest = match screenshot::guard_screenshot_path(out_path, &roots) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("Ruta de captura rechazada: {e}");
                return ExitCode::FAILURE;
            }
        };
        let (ctrl, handle) =
            ScreenshotController::new(dest, roots, args.screenshot_delay_frames, args.window_size);
        (Some(ctrl), Some(handle))
    } else {
        (None, None)
    };

    let [win_w, win_h] = args.window_size;
    let theme_override = args.theme;
    let native_options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("photosort")
            .with_inner_size([win_w as f32, win_h as f32])
            .with_min_inner_size([840.0_f32.min(win_w as f32), 560.0_f32.min(win_h as f32)]),
        ..Default::default()
    };

    let result = eframe::run_native(
        "photosort",
        native_options,
        Box::new(move |cc| {
            let mut photosort =
                PhotosortApp::new(cc, data_dir, app::thumb_provider::open_thumbnails);
            if let Some(theme) = theme_override {
                photosort = photosort.with_theme_override(&cc.egui_ctx, theme);
            }
            if let Some(ctrl) = screenshot_ctrl {
                photosort = photosort.with_screenshot(ctrl);
            }
            Ok(Box::new(photosort))
        }),
    );

    if let Err(e) = result {
        eprintln!("No se pudo iniciar la interfaz gráfica: {e}");
        return ExitCode::FAILURE;
    }

    if let Some(handle) = screenshot_handle {
        return match handle.take_result() {
            Some(Ok(_)) => ExitCode::SUCCESS,
            Some(Err(e)) => {
                eprintln!("Error al capturar pantalla: {e}");
                ExitCode::FAILURE
            }
            None => {
                eprintln!("Error: la ventana se cerró antes de completar la captura.");
                ExitCode::FAILURE
            }
        };
    }

    ExitCode::SUCCESS
}

#[allow(clippy::disallowed_methods)] // `dir` is a fsguard DestPath inside the temp self-test dir
fn create_dir_dest(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

#[allow(clippy::disallowed_methods)] // `dest` is a fsguard DestPath inside the temp self-test dir
fn write_synthetic_png(dest: &DestPath, w: u32, h: u32, color: [u8; 4]) -> Result<(), String> {
    let file = File::create(dest.as_path()).map_err(|e| e.to_string())?;
    let mut writer = BufWriter::new(file);
    let img = ImageBuffer::from_pixel(w, h, Rgba(color));
    img.write_to(&mut writer, ImageFormat::Png)
        .map_err(|e| e.to_string())
}

struct FakeThumbs {
    thumb_png: PathBuf,
}

impl ThumbnailProvider for FakeThumbs {
    fn get(&self, _source: &Path, _sha256: &str) -> io::Result<PathBuf> {
        Ok(self.thumb_png.clone())
    }
}

fn check(name: &str, result: Result<(), String>, all_ok: &mut bool) {
    match result {
        Ok(()) => println!("PASS: {name}"),
        Err(e) => {
            println!("FAIL: {name}: {e}");
            *all_ok = false;
        }
    }
}

fn run_self_test() -> bool {
    let mut all_ok = true;
    let tmp = match tempfile::tempdir() {
        Ok(t) => t,
        Err(e) => {
            println!("FAIL: temp_dir: {e}");
            return false;
        }
    };

    let lib_dir = tmp.path().join("library");
    let data_dir = tmp.path().join("data");
    let thumbs_dir = tmp.path().join("thumbs");

    let prep = (|| -> Result<PathBuf, String> {
        let lib_dest = ensure_writable(&lib_dir, &[]).map_err(|e| e.to_string())?;
        let thumbs_dest = ensure_writable(&thumbs_dir, &[]).map_err(|e| e.to_string())?;
        create_dir_dest(&lib_dest).map_err(|e| e.to_string())?;
        create_dir_dest(&thumbs_dest).map_err(|e| e.to_string())?;

        for (name, color) in [
            ("alpha.png", [220, 60, 60, 255]),
            ("beta.png", [60, 180, 90, 255]),
            ("gamma.png", [60, 110, 220, 255]),
        ] {
            let p = lib_dest
                .join_name(OsStr::new(name))
                .ok_or_else(|| "invalid png name".to_string())?;
            write_synthetic_png(&p, 32, 24, color)?;
        }
        let thumb_p = thumbs_dest
            .join_name(OsStr::new("fake_thumb.png"))
            .ok_or_else(|| "invalid thumb name".to_string())?;
        write_synthetic_png(&thumb_p, 16, 12, [200, 200, 50, 255])?;
        Ok(thumb_p.as_path().to_path_buf())
    })();

    let thumb_png = match prep {
        Ok(p) => {
            println!("PASS: synthetic_fixtures");
            p
        }
        Err(e) => {
            println!("FAIL: synthetic_fixtures: {e}");
            return false;
        }
    };

    // 1. Settings validation & persistence
    let mut cfg = Settings::default();
    let settings_res = (|| -> Result<(), String> {
        if cfg.add_library_root(tmp.path(), &data_dir).is_ok() {
            return Err("expected root containing data_dir to be refused".into());
        }
        cfg.add_library_root(&lib_dir, &data_dir)
            .map_err(|e| e.to_string())?;
        settings::save(&data_dir, &cfg).map_err(|e| e.to_string())?;
        let reloaded = settings::load(&data_dir);
        if reloaded.settings.library_roots.len() != 1 || reloaded.warning.is_some() {
            return Err("reloaded settings mismatch".into());
        }
        Ok(())
    })();
    check("settings_persistence_and_guard", settings_res, &mut all_ok);

    // 2. Background indexing worker
    let roots = cfg.valid_roots();
    let db_worker = match settings::db_dest_path(&data_dir, &roots) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: db_path: {e}");
            return false;
        }
    };
    let db_ui = match settings::db_dest_path(&data_dir, &roots) {
        Ok(d) => d,
        Err(e) => {
            println!("FAIL: db_path_ui: {e}");
            return false;
        }
    };

    let worker = IndexWorker::start(db_worker, roots, None);
    let mut saw_progress = false;
    let index_res = loop {
        match worker.recv() {
            Ok(IndexEvent::Progress { .. }) => saw_progress = true,
            Ok(IndexEvent::RootDone { .. }) => {}
            Ok(IndexEvent::Finished {
                stopped,
                new,
                errors,
                ..
            }) => {
                if stopped || new != 3 || errors != 0 || !saw_progress {
                    break Err(format!(
                        "unexpected index summary: stopped={stopped} new={new} errors={errors} saw_progress={saw_progress}"
                    ));
                }
                break Ok(());
            }
            Ok(IndexEvent::Failed(e)) => break Err(e),
            Err(e) => break Err(e.to_string()),
        }
    };
    check("index_worker_end_to_end", index_res, &mut all_ok);

    // 3. Catalog browse + labels + model filter/selection/grid
    let browse_res = (|| -> Result<Vec<catalog::Entry>, String> {
        let cat = Catalog::open(&db_ui).map_err(|e| e.to_string())?;
        let all = cat
            .entries(&FilterState::default().to_catalog_filter(), Sort::PathAsc)
            .map_err(|e| e.to_string())?;
        if all.len() != 3 {
            return Err(format!("expected 3 entries, got {}", all.len()));
        }
        let tag = cat
            .label(LabelKind::Tag, "prueba")
            .map_err(|e| e.to_string())?;
        cat.suggest(all[0].asset, tag, Source::Ai, 0.7)
            .map_err(|e| e.to_string())?;
        cat.accept(all[0].asset, tag).map_err(|e| e.to_string())?;
        cat.reject(all[1].asset, tag).map_err(|e| e.to_string())?;

        let in_use = cat.labels_in_use().map_err(|e| e.to_string())?;
        if in_use.len() != 1 || in_use[0].images != 1 {
            return Err(format!("unexpected labels_in_use: {in_use:?}"));
        }

        let mut filter = FilterState::default();
        filter.set_chip(tag, Some(ChipMode::Require));
        let filtered = cat
            .entries(&filter.to_catalog_filter(), Sort::PathAsc)
            .map_err(|e| e.to_string())?;
        if filtered.len() != 1 || filtered[0].asset != all[0].asset {
            return Err("filter did not isolate accepted asset".into());
        }

        let mut sel = Selection::default();
        sel.click(0, all.len());
        sel.shift_click(2, all.len());
        if sel.len() != 3 {
            return Err("selection range mismatch".into());
        }
        let grid = GridLayout::compute(600.0, 172.0, 206.0, 10.0, all.len());
        if grid.columns < 1 || grid.row_count != 1 {
            return Err(format!("unexpected grid layout: {grid:?}"));
        }
        Ok(all)
    })();
    let entries = match browse_res {
        Ok(e) => {
            println!("PASS: catalog_and_model");
            e
        }
        Err(e) => {
            println!("FAIL: catalog_and_model: {e}");
            return false;
        }
    };

    // 4. Thumbnail pool (fake provider) + original preview worker
    let media_res = (|| -> Result<(), String> {
        let pool = ThumbPool::new(2, Arc::new(FakeThumbs { thumb_png }), None);
        pool.request(ThumbRequest {
            source: entries[0].path.clone(),
            sha256: entries[0].sha256.clone(),
        });
        let thumb = pool.recv().map_err(|e| e.to_string())?;
        let img = thumb.image?;
        if img.size != [16, 12] {
            return Err(format!("unexpected thumb size: {:?}", img.size));
        }

        let preview = PreviewWorker::new(16, None);
        preview.request(entries[0].path.clone());
        let loaded = preview.recv().map_err(|e| e.to_string())?;
        let pimg = loaded.image?;
        if pimg.size != [16, 12] {
            return Err(format!("unexpected preview size: {:?}", pimg.size));
        }
        Ok(())
    })();
    check("thumbnail_pool_and_preview_worker", media_res, &mut all_ok);

    // 5. Real thumbnails: the cache must appear under the data dir, never inside the library
    let real_thumbs_res = (|| -> Result<(), String> {
        let (provider, warning) =
            app::thumb_provider::open_thumbnails(&data_dir, &cfg.valid_roots());
        if let Some(w) = warning {
            return Err(w);
        }
        let pool = ThumbPool::new(2, provider, None);
        pool.request(ThumbRequest {
            source: entries[0].path.clone(),
            sha256: entries[0].sha256.clone(),
        });
        let thumb = pool.recv().map_err(|e| e.to_string())?;
        let img = thumb.image?;
        if img.size != [32, 24] {
            return Err(format!("unexpected thumb size: {:?}", img.size));
        }
        let cached = fs::read_dir(data_dir.join("cache"))
            .map_err(|e| e.to_string())?
            .count();
        if cached != 1 {
            return Err(format!("expected 1 cached thumbnail, found {cached}"));
        }
        Ok(())
    })();
    check("real_thumbnails_end_to_end", real_thumbs_res, &mut all_ok);

    // Ensure TempDir drops cleanly at the end of self-test.
    drop(tmp);
    all_ok
}
