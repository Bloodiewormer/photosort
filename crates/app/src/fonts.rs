use eframe::egui::{Context, FontData, FontDefinitions, FontFamily};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Windows system fonts used as fallback for OCR text, one per group, first found wins.
/// Microsoft YaHei covers kana as well as Han (GBK includes them), so one font serves
/// Japanese and Chinese; the Japanese fonts are only for systems without it.
/// Hangul needs its own font. Each file is ~13–19 MB kept in memory, hence one per group.
const FALLBACK_GROUPS: &[(&str, &[&str])] = &[
    (
        "cjk_han",
        &["msyh.ttc", "YuGothR.ttc", "msgothic.ttc", "simsun.ttc"],
    ),
    ("cjk_hangul", &["malgun.ttf", "gulim.ttc"]),
];

/// `%WINDIR%\Fonts`, or `C:\Windows\Fonts` when the variable is missing.
pub fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}

/// Adds Windows' CJK fonts as fallbacks so Japanese, Chinese and Korean OCR text does not
/// render as empty boxes. Returns the files loaded.
pub fn configure_cjk_fallback_fonts(ctx: &Context) -> Vec<&'static str> {
    configure_from(ctx, &windows_fonts_dir())
}

fn configure_from(ctx: &Context, fonts_dir: &Path) -> Vec<&'static str> {
    let mut defs = FontDefinitions::default();
    let mut loaded = Vec::new();
    for (name, candidates) in FALLBACK_GROUPS {
        let found = candidates.iter().find_map(|&file| {
            // A missing or unreadable font only means that script falls back to boxes.
            std::fs::read(fonts_dir.join(file))
                .ok()
                .map(|bytes| (file, bytes))
        });
        if let Some((file, bytes)) = found {
            defs.font_data
                .insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            // Appended after egui's own fonts, so Latin text keeps its typeface and metrics.
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                defs.families
                    .entry(family)
                    .or_default()
                    .push(name.to_string());
            }
            loaded.push(file);
        }
    }
    if !loaded.is_empty() {
        ctx.set_fonts(defs);
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Color32, FontId, RawInput};

    const JAPANESE: &str = "時計仕掛けのクロニクル第話";
    const CHINESE: &str = "简体中文测试时钟编年史第十二话";
    const KOREAN: &str = "바람공주제화한국어";

    /// Runs a frame so `set_fonts` takes effect, then reports glyph coverage and Latin width.
    fn coverage(ctx: &Context) -> ([bool; 3], f32) {
        let mut out = ctx.run_ui(RawInput::default(), |_| {});
        out.textures_delta.clear();
        let font = FontId::proportional(14.0);
        ctx.fonts_mut(|f| {
            let has = [JAPANESE, CHINESE, KOREAN].map(|s| f.has_glyphs(&font, s));
            let latin = f
                .layout_no_wrap("Etiquetas áéíóú ñ 123".into(), font.clone(), Color32::WHITE)
                .size()
                .x;
            (has, latin)
        })
    }

    #[test]
    fn system_fonts_cover_cjk_without_changing_latin_text() {
        let (plain_has, plain_latin) = coverage(&Context::default());
        assert_eq!(plain_has, [false; 3], "egui solo no trae fuentes CJK");

        let ctx = Context::default();
        let loaded = configure_cjk_fallback_fonts(&ctx);
        let (has, latin) = coverage(&ctx);
        assert_eq!(latin, plain_latin, "la fuente latina no debe cambiar");
        if loaded.contains(&"msyh.ttc") {
            assert!(
                has[0] && has[1],
                "Microsoft YaHei cubre japonés y chino: {loaded:?}"
            );
        }
        if loaded.contains(&"malgun.ttf") {
            assert!(has[2], "Malgun Gothic cubre coreano: {loaded:?}");
        }
    }

    #[test]
    fn missing_fonts_leave_egui_defaults() {
        let empty = tempfile::tempdir().unwrap();
        let ctx = Context::default();
        assert!(configure_from(&ctx, empty.path()).is_empty());
        assert_eq!(coverage(&ctx).0, [false; 3]);
    }
}
