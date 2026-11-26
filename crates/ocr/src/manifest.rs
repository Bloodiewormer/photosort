//! The only model files the engine accepts. Release assets can be replaced upstream, so every file
//! is checked against these sizes and hashes before it is loaded.

use std::path::Path;

pub struct PinnedFile {
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
    /// Upstream release (GitHub `GreatV/oar-ocr`) the file comes from.
    pub release: &'static str,
}

pub const DETECTOR: &str = "pp-ocrv6_small_det.onnx";
pub const REC_V6: &str = "pp-ocrv6_small_rec.onnx";
pub const DICT_V6: &str = "ppocrv6_dict.txt";
pub const REC_KOREAN: &str = "korean_pp-ocrv5_mobile_rec.onnx";
pub const DICT_KOREAN: &str = "ppocrv5_korean_dict.txt";

pub const PINNED: [PinnedFile; 5] = [
    PinnedFile {
        name: DETECTOR,
        size: 9_880_512,
        sha256: "d73e0058b7a8086bbd57f3d10b8bcd4ff95363f67e06e2762b5e814fe9c9410e",
        release: "v0.7.0 (4e350472b058ef78a640634bc9095fc3836c3e34)",
    },
    PinnedFile {
        name: REC_V6,
        size: 21_159_378,
        sha256: "5435fd747c9e0efe15a96d0b378d5bd157e9492ed8fd80edf08f30d02fa24634",
        release: "v0.7.0 (4e350472b058ef78a640634bc9095fc3836c3e34)",
    },
    PinnedFile {
        name: DICT_V6,
        size: 74_947,
        sha256: "b5f2bfe2bdd9448429e3e82b51c789775d9b42f2403d082b00662eb77e401c5d",
        release: "v0.7.0 (4e350472b058ef78a640634bc9095fc3836c3e34)",
    },
    PinnedFile {
        name: REC_KOREAN,
        size: 13_446_374,
        sha256: "2d7ed96308065a86103325d22af07a88c4d06afc009f21602a4882342c0cc054",
        release: "v0.3.0 (7a0ea9cbe5de7d9e4c8415b032f4854fb688245e)",
    },
    PinnedFile {
        name: DICT_KOREAN,
        size: 47_451,
        sha256: "a88071c68c01707489baa79ebe0405b7beb5cca229f4fc94cc3ef992328802d7",
        release: "v0.3.0 (7a0ea9cbe5de7d9e4c8415b032f4854fb688245e)",
    },
];

/// Checks every pinned file in `dir`. The error lists all problems, not only the first one.
pub fn verify_models(dir: &Path) -> Result<(), Vec<String>> {
    verify(dir, &PINNED)
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
    let name = pinned.name;
    let path = dir.join(name);
    let size = std::fs::metadata(&path)
        .map_err(|e| format!("{name}: {e}"))?
        .len();
    if size != pinned.size {
        return Err(format!("{name}: size {size}, expected {}", pinned.size));
    }
    let sha = scanner::sha256_file(&path).map_err(|e| format!("{name}: {e}"))?;
    if sha != pinned.sha256 {
        return Err(format!("{name}: sha256 {sha}, expected {}", pinned.sha256));
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

    fn fixture(name: &'static str) -> PinnedFile {
        PinnedFile {
            name,
            size: 3,
            sha256: ABC,
            release: "test",
        }
    }

    #[test]
    fn accepts_matching_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a"), "abc").unwrap();
        assert_eq!(verify(dir.path(), &[fixture("a")]), Ok(()));
    }

    #[test]
    fn reports_every_missing_resized_and_tampered_file() {
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
    fn an_empty_folder_is_missing_all_five_pinned_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(verify_models(dir.path()).unwrap_err().len(), 5);
    }
}
