use eframe::egui::{Context, FontDefinitions, FontFamily};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Report on fallback CJK fonts loaded at startup.
#[derive(Debug, Clone, Default)]
pub struct FontsLoadReport {
    pub loaded_fonts: Vec<String>,
    pub total_bytes: usize,
    pub elapsed_ms: f64,
}

/// Resolves the Windows Fonts directory from `%WINDIR%` or `%SystemRoot%`, defaulting to `C:\Windows\Fonts`.
pub fn windows_fonts_dir() -> PathBuf {
    let windir = env::var_os("WINDIR")
        .or_else(|| env::var_os("SystemRoot"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    windir.join("Fonts")
}

/// Adds Windows system fonts as fallbacks for Japanese, Chinese, and Korean text.
///
/// Only loads existing fonts in read-only mode, never fails if some or all are missing,
/// and appends them to egui's `Proportional` and `Monospace` families so Latin typography
/// and base metrics remain unchanged.
pub fn configure_cjk_fallback_fonts(ctx: &Context) -> FontsLoadReport {
    let start = Instant::now();
    let fonts_dir = windows_fonts_dir();

    // Priority lists: Japanese, Chinese (Simplified/Traditional), Korean.
    // Pick the first candidate found for each language to keep memory footprint bounded.
    let japanese_candidates = ["YuGothR.ttc", "msgothic.ttc", "meiryo.ttc"];
    let chinese_candidates = ["msyh.ttc", "simsun.ttc"];
    let korean_candidates = ["malgun.ttf", "gulim.ttc"];

    let mut report = FontsLoadReport::default();
    let mut font_defs = FontDefinitions::default();

    let groups: &[(&str, &[&str])] = &[
        ("cjk_japanese", &japanese_candidates),
        ("cjk_chinese", &chinese_candidates),
        ("cjk_korean", &korean_candidates),
    ];

    let mut newly_added = Vec::new();

    for (name, candidates) in groups {
        for &filename in *candidates {
            let path = fonts_dir.join(filename);
            if path.is_file() {
                match std::fs::read(&path) {
                    Ok(bytes) => {
                        let len = bytes.len();
                        report.total_bytes += len;
                        let font_name = name.to_string();
                        font_defs.font_data.insert(
                            font_name.clone(),
                            Arc::new(eframe::egui::FontData::from_owned(bytes)),
                        );
                        newly_added.push(font_name);
                        report
                            .loaded_fonts
                            .push(format!("{filename} ({:.1} MB)", len as f64 / 1_048_576.0));
                        break;
                    }
                    Err(e) => {
                        eprintln!("Advertencia: no se pudo leer la fuente {filename}: {e}");
                    }
                }
            }
        }
    }

    if !newly_added.is_empty() {
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            if let Some(list) = font_defs.families.get_mut(&family) {
                for font_name in &newly_added {
                    list.push(font_name.clone());
                }
            }
        }
        ctx.set_fonts(font_defs);
    }

    report.elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Color32, RawInput};

    #[test]
    fn configure_cjk_fallback_fonts_does_not_panic() {
        let ctx = Context::default();
        let report = configure_cjk_fallback_fonts(&ctx);
        // On Windows with standard fonts, at least one CJK font should be detected.
        if windows_fonts_dir().is_dir() {
            println!(
                "Fuentes CJK cargadas: {:?} ({:.1} MB en {:.2} ms)",
                report.loaded_fonts,
                report.total_bytes as f64 / 1_048_576.0,
                report.elapsed_ms
            );
        }
    }

    #[test]
    fn cjk_text_renders_without_panic() {
        let ctx = Context::default();
        let _ = configure_cjk_fallback_fonts(&ctx);

        let input = RawInput::default();
        let mut output = ctx.run_ui(input, |ui| {
            // Japanese text
            ui.label("時計仕掛けのクロニクル — 第12話");
            // Chinese text
            ui.label("简体中文测试：时钟编年史 第十二话");
            // Korean text
            ui.label("바람 공주 · 제3화 (한국어 OCR)");

            // Layout test directly in painter
            let font_id = eframe::egui::FontId::proportional(14.0);
            let gal = ui.painter().layout_no_wrap(
                "時計仕掛け / 바람 공주 / 中文".to_string(),
                font_id,
                Color32::WHITE,
            );
            assert!(gal.rect.width() > 0.0);
        });
        output.textures_delta.clear();
    }
}
