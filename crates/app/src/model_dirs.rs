use crate::settings::Settings;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Vision,
    Ocr,
}

/// Formats verification errors from `vision::verify_all` or `ocr::verify_models` into a concise
/// Spanish explanation suitable for showing inline in Settings.
pub fn format_verify_error(kind: ModelKind, errors: &[String]) -> String {
    let expected = match kind {
        ModelKind::Vision => "siglip2 y yunet",
        ModelKind::Ocr => "los archivos de modelos OCR",
    };
    let Some(first) = errors.first() else {
        return format!("Carpeta no válida: no contiene {expected}");
    };
    let rest = errors.len().saturating_sub(1);
    let summary = if first.contains("size ") || first.contains("sha256 ") {
        let file = first.split(':').next().unwrap_or(first).trim();
        format!("{file} no coincide con el archivo esperado")
    } else {
        let file = first.split(':').next().unwrap_or(first).trim();
        format!("falta {file}")
    };
    if rest == 0 {
        format!("Carpeta no válida: {summary}")
    } else if rest == 1 {
        format!("Carpeta no válida: {summary} y 1 archivo más")
    } else {
        format!("Carpeta no válida: {summary} y {rest} archivos más")
    }
}

/// Validates a picked Vision models folder (`siglip2` + `yunet`), activates `stage_vision`,
/// and infers a valid OCR folder in `<picked>/ocr` or sibling `<picked>/../ocr`.
pub fn apply_picked_vision_dir(
    settings: &mut Settings,
    picked: &Path,
) -> Result<Option<PathBuf>, String> {
    apply_picked_vision_dir_with(
        settings,
        picked,
        vision::manifest::verify_all,
        ocr::manifest::verify_models,
    )
}

/// Validates a picked OCR models folder, activates `stage_ocr`, and infers a valid Vision folder
/// in `<picked>/..`.
pub fn apply_picked_ocr_dir(
    settings: &mut Settings,
    picked: &Path,
) -> Result<Option<PathBuf>, String> {
    apply_picked_ocr_dir_with(
        settings,
        picked,
        vision::manifest::verify_all,
        ocr::manifest::verify_models,
    )
}

/// Testable implementation of [`apply_picked_vision_dir`] with injectable verifiers.
pub fn apply_picked_vision_dir_with<FV, FO>(
    settings: &mut Settings,
    picked: &Path,
    verify_vision: FV,
    verify_ocr: FO,
) -> Result<Option<PathBuf>, String>
where
    FV: Fn(&Path) -> Result<(), Vec<String>>,
    FO: Fn(&Path) -> Result<(), Vec<String>>,
{
    let mut near = vec![picked.join("models")];
    near.extend(picked.ancestors().skip(1).take(2).map(Path::to_path_buf));
    let picked = &find_dir(picked, &near, &verify_vision)
        .map_err(|errs| format_verify_error(ModelKind::Vision, &errs))?;
    settings.vision_models_dir = Some(picked.to_path_buf());
    settings.stage_vision = true;

    let needs_ocr = settings
        .ocr_models_dir
        .as_deref()
        .is_none_or(|cur| verify_ocr(cur).is_err());
    if !needs_ocr {
        return Ok(None);
    }

    let mut candidates = vec![picked.join("ocr")];
    if let Some(parent) = picked.parent() {
        candidates.push(parent.join("ocr"));
    }
    for cand in candidates {
        if cand.is_dir() && verify_ocr(&cand).is_ok() {
            settings.ocr_models_dir = Some(cand.clone());
            settings.stage_ocr = true;
            return Ok(Some(cand));
        }
    }
    Ok(None)
}

/// Testable implementation of [`apply_picked_ocr_dir`] with injectable verifiers.
pub fn apply_picked_ocr_dir_with<FV, FO>(
    settings: &mut Settings,
    picked: &Path,
    verify_vision: FV,
    verify_ocr: FO,
) -> Result<Option<PathBuf>, String>
where
    FV: Fn(&Path) -> Result<(), Vec<String>>,
    FO: Fn(&Path) -> Result<(), Vec<String>>,
{
    let mut near = vec![picked.join("ocr"), picked.join("models").join("ocr")];
    near.extend(picked.parent().map(|p| p.join("ocr")));
    let picked = &find_dir(picked, &near, &verify_ocr)
        .map_err(|errs| format_verify_error(ModelKind::Ocr, &errs))?;
    settings.ocr_models_dir = Some(picked.to_path_buf());
    settings.stage_ocr = true;

    let needs_vision = settings
        .vision_models_dir
        .as_deref()
        .is_none_or(|cur| verify_vision(cur).is_err());
    if !needs_vision {
        return Ok(None);
    }

    if let Some(parent) = picked.parent()
        && parent.is_dir()
        && verify_vision(parent).is_ok()
    {
        settings.vision_models_dir = Some(parent.to_path_buf());
        settings.stage_vision = true;
        return Ok(Some(parent.to_path_buf()));
    }
    Ok(None)
}

/// The user may pick the models folder itself, a folder above it or one of its subfolders: the
/// first valid one of `picked` and `near` wins. The error is the one for `picked`.
fn find_dir<F>(picked: &Path, near: &[PathBuf], verify: &F) -> Result<PathBuf, Vec<String>>
where
    F: Fn(&Path) -> Result<(), Vec<String>>,
{
    let errs = match verify(picked) {
        Ok(()) => return Ok(picked.to_path_buf()),
        Err(errs) => errs,
    };
    near.iter()
        .find(|c| c.is_dir() && verify(c).is_ok())
        .cloned()
        .ok_or(errs)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures inside a temporary directory
mod tests {
    use super::*;
    use std::fs;

    fn fake_verify_vision(dir: &Path) -> Result<(), Vec<String>> {
        let mut errs = Vec::new();
        for rel in ["siglip2/model.onnx", "yunet/face.onnx"] {
            if !dir.join(rel).is_file() {
                errs.push(format!("{rel}: missing"));
            }
        }
        if errs.is_empty() { Ok(()) } else { Err(errs) }
    }

    fn fake_verify_ocr(dir: &Path) -> Result<(), Vec<String>> {
        if dir.join("det.onnx").is_file() {
            Ok(())
        } else {
            Err(vec!["det.onnx: missing".into()])
        }
    }

    fn seed_fake_vision(dir: &Path) {
        fs::create_dir_all(dir.join("siglip2")).unwrap();
        fs::create_dir_all(dir.join("yunet")).unwrap();
        fs::write(dir.join("siglip2/model.onnx"), b"v").unwrap();
        fs::write(dir.join("yunet/face.onnx"), b"y").unwrap();
    }

    fn seed_fake_ocr(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("det.onnx"), b"o").unwrap();
    }

    #[test]
    fn picking_vision_infers_child_or_sibling_ocr_and_enables_stages() {
        // Case 1: <models>/ocr child folder
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        seed_fake_vision(&models);
        seed_fake_ocr(&models.join("ocr"));

        let mut s = Settings::default();
        let inferred =
            apply_picked_vision_dir_with(&mut s, &models, fake_verify_vision, fake_verify_ocr)
                .unwrap();
        assert_eq!(s.vision_models_dir.as_deref(), Some(models.as_path()));
        assert_eq!(inferred.as_deref(), Some(models.join("ocr").as_path()));
        assert_eq!(
            s.ocr_models_dir.as_deref(),
            Some(models.join("ocr").as_path())
        );
        assert!(s.stage_vision);
        assert!(s.stage_ocr);

        // Case 2: sibling ../ocr folder
        let tmp2 = tempfile::tempdir().unwrap();
        let vis = tmp2.path().join("vision");
        let ocr = tmp2.path().join("ocr");
        seed_fake_vision(&vis);
        seed_fake_ocr(&ocr);

        let mut s2 = Settings::default();
        let inferred2 =
            apply_picked_vision_dir_with(&mut s2, &vis, fake_verify_vision, fake_verify_ocr)
                .unwrap();
        assert_eq!(inferred2.as_deref(), Some(ocr.as_path()));
        assert_eq!(s2.ocr_models_dir.as_deref(), Some(ocr.as_path()));
        assert!(s2.stage_vision);
        assert!(s2.stage_ocr);
    }

    #[test]
    fn picking_a_folder_above_or_inside_the_models_finds_them() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        seed_fake_vision(&models);
        seed_fake_ocr(&models.join("ocr"));

        let mut above = Settings::default();
        apply_picked_vision_dir_with(&mut above, tmp.path(), fake_verify_vision, fake_verify_ocr)
            .unwrap();
        assert_eq!(above.vision_models_dir.as_deref(), Some(models.as_path()));
        assert_eq!(above.ocr_models_dir, Some(models.join("ocr")));

        let mut inside = Settings::default();
        let siglip = models.join("siglip2");
        apply_picked_vision_dir_with(&mut inside, &siglip, fake_verify_vision, fake_verify_ocr)
            .unwrap();
        assert_eq!(inside.vision_models_dir.as_deref(), Some(models.as_path()));

        let mut ocr_above = Settings::default();
        apply_picked_ocr_dir_with(
            &mut ocr_above,
            tmp.path(),
            fake_verify_vision,
            fake_verify_ocr,
        )
        .unwrap();
        assert_eq!(ocr_above.ocr_models_dir, Some(models.join("ocr")));
        assert_eq!(
            ocr_above.vision_models_dir.as_deref(),
            Some(models.as_path())
        );
    }

    #[test]
    fn picking_ocr_infers_parent_vision_and_rejects_empty_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let models = tmp.path().join("models");
        let ocr_dir = models.join("ocr");
        seed_fake_vision(&models);
        seed_fake_ocr(&ocr_dir);

        let mut s = Settings::default();
        let inferred =
            apply_picked_ocr_dir_with(&mut s, &ocr_dir, fake_verify_vision, fake_verify_ocr)
                .unwrap();
        assert_eq!(s.ocr_models_dir.as_deref(), Some(ocr_dir.as_path()));
        assert_eq!(inferred.as_deref(), Some(models.as_path()));
        assert_eq!(s.vision_models_dir.as_deref(), Some(models.as_path()));
        assert!(s.stage_ocr);
        assert!(s.stage_vision);

        // Empty folder is rejected with a reason and does not enable the stage
        let empty = tempfile::tempdir().unwrap();
        let mut s_bad = Settings::default();
        let err = apply_picked_vision_dir_with(
            &mut s_bad,
            empty.path(),
            fake_verify_vision,
            fake_verify_ocr,
        )
        .unwrap_err();
        assert!(err.contains("siglip2/model.onnx"), "{err}");
        assert!(s_bad.vision_models_dir.is_none());
        assert!(!s_bad.stage_vision);
    }
}
