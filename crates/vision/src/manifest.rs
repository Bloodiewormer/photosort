//! The only vision model files this crate accepts. Every file is verified by byte length and
//! SHA-256 before any session or tokenizer is loaded.

use std::path::Path;

pub struct PinnedFile {
    /// Relative path under the models root (e.g. `siglip2/vision_model_fp16.onnx`).
    pub path: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    pub source: &'static str,
    pub license: &'static str,
}

pub const SIGLIP2_VISION: &str = "siglip2/vision_model_fp16.onnx";
pub const SIGLIP2_TEXT: &str = "siglip2/text_model_int8.onnx";
pub const SIGLIP2_TOKENIZER: &str = "siglip2/tokenizer.json";
pub const SIGLIP2_CONFIG: &str = "siglip2/config.json";
pub const SIGLIP2_PREPROCESSOR: &str = "siglip2/preprocessor_config.json";
pub const SIGLIP2_TOKENIZER_CONFIG: &str = "siglip2/tokenizer_config.json";
pub const YUNET_MODEL: &str = "yunet/face_detection_yunet_2023mar.onnx";

const SIGLIP2_SOURCE: &str = "onnx-community/siglip2-base-patch16-256-ONNX rev d1114256522a37ffa257a0a58017348ab0058db2 (google/siglip2-base-patch16-256)";
const YUNET_SOURCE: &str =
    "opencv/face_detection_yunet rev 3cc26e7f1014a5ee5d74a42acee58bafc9d0a310";

pub const PINNED_SIGLIP2: [PinnedFile; 6] = [
    PinnedFile {
        path: SIGLIP2_VISION,
        size: 186_131_676,
        sha256: "fe9ad8020a6d3d98d394c9be8f07064066135fc2f87ec11692de0b677c0ac4db",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
    PinnedFile {
        path: SIGLIP2_TEXT,
        size: 283_438_275,
        sha256: "6f59b39d880c413042314b79302b74d0dd93b273caf8fbfdb1eb2df61a7fefd4",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
    PinnedFile {
        path: SIGLIP2_TOKENIZER,
        size: 34_363_039,
        sha256: "cb9140fae3ac5122c972d37adf83e1248471a38147ad76f8215c8872c6fd8322",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
    PinnedFile {
        path: SIGLIP2_CONFIG,
        size: 458,
        sha256: "795a8e216ebb51ad5043e128e557c45279b686bd672221fbe01279a7b8cb79c9",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
    PinnedFile {
        path: SIGLIP2_PREPROCESSOR,
        size: 394,
        sha256: "d14ba2ee3fd816f3de8abaddc31953565128eaf37c73ad4bed32101a98465aff",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
    PinnedFile {
        path: SIGLIP2_TOKENIZER_CONFIG,
        size: 47_240,
        sha256: "7c3a247599e741bceba1a3fe0285aea88d1044dc1fad2caa1e48cdd9fd25f630",
        source: SIGLIP2_SOURCE,
        license: "Apache-2.0",
    },
];

pub const PINNED_YUNET: [PinnedFile; 1] = [PinnedFile {
    path: YUNET_MODEL,
    size: 232_589,
    sha256: "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    source: YUNET_SOURCE,
    license: "MIT",
}];

/// Checks every pinned SigLIP 2 file under `models_root`.
pub fn verify_siglip2(models_root: &Path) -> Result<(), Vec<String>> {
    verify(models_root, &PINNED_SIGLIP2)
}

/// Checks the pinned YuNet file under `models_root`.
pub fn verify_yunet(models_root: &Path) -> Result<(), Vec<String>> {
    verify(models_root, &PINNED_YUNET)
}

/// Checks all pinned vision files (SigLIP 2 + YuNet) under `models_root`.
pub fn verify_all(models_root: &Path) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if let Err(e) = verify_siglip2(models_root) {
        errors.extend(e);
    }
    if let Err(e) = verify_yunet(models_root) {
        errors.extend(e);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn verify(dir: &Path, pinned: &[PinnedFile]) -> Result<(), Vec<String>> {
    let errors: Vec<String> = pinned.iter().filter_map(|p| check(dir, p).err()).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn check(dir: &Path, pinned: &PinnedFile) -> Result<(), String> {
    let rel = pinned.path;
    let path = dir.join(rel);
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("{rel}: {e}"))?
        .len();
    if size != pinned.size {
        return Err(format!("{rel}: size {size}, expected {}", pinned.size));
    }
    let sha = scanner::sha256_file(&path).map_err(|e| format!("{rel}: {e}"))?;
    if sha != pinned.sha256 {
        return Err(format!("{rel}: sha256 {sha}, expected {}", pinned.sha256));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // test fixtures written inside a temporary folder
mod tests {
    use super::*;
    use std::fs;

    // "abc", from FIPS 180-2.
    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    fn fixture(path: &'static str) -> PinnedFile {
        PinnedFile {
            path,
            size: 3,
            sha256: ABC,
            source: "test",
            license: "MIT",
        }
    }

    #[test]
    fn accepts_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/a"), "abc").unwrap();
        assert_eq!(verify(dir.path(), &[fixture("sub/a")]), Ok(()));
    }

    #[test]
    fn reports_missing_resized_and_tampered() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("longer"), "abcd").unwrap();
        fs::write(dir.path().join("tampered"), "abd").unwrap();
        let pinned = [fixture("missing"), fixture("longer"), fixture("tampered")];
        let errors = verify(dir.path(), &pinned).unwrap_err();
        assert_eq!(errors.len(), 3, "{errors:?}");
        assert!(errors[0].starts_with("missing: "));
        assert!(errors[1].contains("size 4"));
        assert!(errors[2].contains("sha256"));
    }

    #[test]
    fn empty_folder_misses_all_seven_pinned_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(verify_all(dir.path()).unwrap_err().len(), 7);
    }
}
