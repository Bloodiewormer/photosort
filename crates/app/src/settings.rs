use fsguard::{DestPath, GuardError, LibraryRoot, ensure_export_dir, ensure_writable};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const SETTINGS_FILE: &str = "settings.json";
const SETTINGS_TMP: &str = "settings.json.tmp";
const SETTINGS_BAD: &str = "settings.json.bad";
const CATALOG_DB: &str = "catalog.sqlite";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    System,
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum OcrDevice {
    #[default]
    DirectMl,
    Cpu,
}

fn default_auto_analyze() -> bool {
    true
}

fn default_stage_rules() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub library_roots: Vec<PathBuf>,
    #[serde(default)]
    pub last_export_dir: Option<PathBuf>,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default = "default_auto_analyze")]
    pub auto_analyze: bool,
    #[serde(default)]
    pub ocr_models_dir: Option<PathBuf>,
    #[serde(default)]
    pub vision_models_dir: Option<PathBuf>,
    #[serde(default)]
    pub ocr_device: OcrDevice,
    /// Master switch for automatic background review. OFF by default.
    #[serde(default)]
    pub background_review: bool,
    /// Fast rule-based stage (enabled by default).
    #[serde(default = "default_stage_rules")]
    pub stage_rules: bool,
    /// Image recognition stage (SigLIP 2 + YuNet). OFF until enabled.
    #[serde(default)]
    pub stage_vision: bool,
    /// Face grouping stage (SFace face recognition). OFF by default.
    #[serde(default)]
    pub stage_face_grouping: bool,
    /// OCR + works matching stage. OFF until enabled (slowest).
    #[serde(default)]
    pub stage_ocr: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            library_roots: Vec::new(),
            last_export_dir: None,
            theme: Theme::default(),
            auto_analyze: default_auto_analyze(),
            ocr_models_dir: None,
            vision_models_dir: None,
            ocr_device: OcrDevice::default(),
            background_review: false,
            stage_rules: default_stage_rules(),
            stage_vision: false,
            stage_face_grouping: false,
            stage_ocr: false,
        }
    }
}

#[derive(Debug)]
pub struct LoadOutcome {
    pub settings: Settings,
    pub warning: Option<String>,
}

#[derive(Debug)]
pub enum SettingsError {
    NotADirectory(PathBuf),
    RootContainsDataDir,
    RootInsideDataDir,
    Guard(GuardError),
    Io(io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADirectory(p) => {
                write!(f, "La ruta no es una carpeta válida: {}", p.display())
            }
            Self::RootContainsDataDir => write!(
                f,
                "La carpeta elegida contiene el directorio de datos de photosort; elige otra carpeta."
            ),
            Self::RootInsideDataDir => write!(
                f,
                "La carpeta elegida está dentro del directorio de datos de photosort; elige otra carpeta."
            ),
            // Only the data folder reaches the guard from here, and the guard's text no longer names it.
            Self::Guard(e) => write!(
                f,
                "El guardián de seguridad rechazó la carpeta de datos de photosort: {e}"
            ),
            Self::Io(e) => write!(f, "Error de lectura o escritura de configuración: {e}"),
            Self::Json(e) => write!(f, "Error al serializar la configuración: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

impl From<GuardError> for SettingsError {
    fn from(e: GuardError) -> Self {
        Self::Guard(e)
    }
}

impl From<io::Error> for SettingsError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for SettingsError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

/// Resolves the app data directory: `--data-dir` flag, `PHOTOSORT_DATA_DIR` env var,
/// or `%LOCALAPPDATA%\photosort`.
pub fn resolve_data_dir(cli_override: Option<&Path>) -> PathBuf {
    if let Some(p) = cli_override {
        return p.to_path_buf();
    }
    if let Some(env_dir) = std::env::var_os("PHOTOSORT_DATA_DIR")
        && !env_dir.is_empty()
    {
        return PathBuf::from(env_dir);
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA")
        && !local.is_empty()
    {
        return PathBuf::from(local).join("photosort");
    }
    if let Some(home) = std::env::var_os("USERPROFILE")
        && !home.is_empty()
    {
        return PathBuf::from(home).join(".photosort");
    }
    PathBuf::from("photosort-data")
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

impl Settings {
    /// Validates and adds a library root, refusing any folder that contains or lies inside `data_dir`.
    pub fn add_library_root(
        &mut self,
        candidate: &Path,
        data_dir: &Path,
    ) -> Result<PathBuf, SettingsError> {
        let md =
            fs::metadata(candidate).map_err(|_| SettingsError::NotADirectory(candidate.into()))?;
        if !md.is_dir() {
            return Err(SettingsError::NotADirectory(candidate.to_path_buf()));
        }
        let root = LibraryRoot::new(candidate)?;
        // `ensure_export_dir` proves `data_dir` is neither inside `root` nor a parent of `root`,
        // resolving symlinks/junctions and matching case-insensitively on NTFS.
        match ensure_export_dir(data_dir, std::slice::from_ref(&root)) {
            Ok(_) => {}
            Err(GuardError::InsideLibrary { .. }) => {
                return Err(SettingsError::RootContainsDataDir);
            }
            Err(GuardError::ContainsLibrary { .. }) => {
                return Err(SettingsError::RootInsideDataDir);
            }
            Err(e) => return Err(SettingsError::Guard(e)),
        }
        let canonical = root.as_path().to_path_buf();
        let needle = path_key(&canonical);
        if !self.library_roots.iter().any(|p| path_key(p) == needle) {
            self.library_roots.push(canonical.clone());
        }
        Ok(canonical)
    }

    pub fn remove_library_root(&mut self, path: &Path) -> bool {
        let needle = path_key(path);
        let before = self.library_roots.len();
        self.library_roots.retain(|p| path_key(p) != needle);
        self.library_roots.len() != before
    }

    /// Resolves currently existing library roots for passing to `fsguard` and indexing.
    pub fn valid_roots(&self) -> Vec<LibraryRoot> {
        self.library_roots
            .iter()
            .filter_map(|p| LibraryRoot::new(p).ok())
            .collect()
    }

    /// Returns the currently enabled review stages with their current versions.
    pub fn enabled_stage_specs(&self) -> Vec<catalog::StageSpec> {
        let mut out = Vec::with_capacity(3);
        if self.stage_rules {
            out.push(catalog::StageSpec::new(rules::STAGE, rules::VERSION));
        }
        if self.stage_vision {
            out.push(catalog::StageSpec::new(
                catalog::VISION_STAGE,
                catalog::VISION_VERSION,
            ));
        }
        if self.stage_ocr {
            out.push(catalog::StageSpec::new(
                catalog::OCR_STAGE,
                catalog::OCR_VERSION,
            ));
        }
        out
    }

    /// Builds a [`crate::review_engine::ReviewConfig`] reflecting the saved preferences.
    pub fn to_review_config(&self) -> crate::review_engine::ReviewConfig {
        crate::review_engine::ReviewConfig {
            enabled: self.background_review,
            rules: self.stage_rules,
            vision: self.stage_vision,
            ocr: self.stage_ocr,
            roots: self.library_roots.clone(),
            vision_models_dir: self.vision_models_dir.clone(),
            ocr_models_dir: self.ocr_models_dir.clone(),
            device: self.ocr_device,
            chunk_size: crate::review_engine::DEFAULT_CHUNK_SIZE,
            error_backoff: crate::review_engine::DEFAULT_ERROR_BACKOFF,
        }
    }
}

/// Creates a directory already proven to be outside every library root.
#[allow(clippy::disallowed_methods)] // `dir` is a fsguard DestPath
fn create_dir_dest(dir: &DestPath) -> io::Result<()> {
    fs::create_dir_all(dir.as_path())
}

/// Writes bytes to `tmp` and atomically renames to `target`. Both paths are `DestPath`.
#[allow(clippy::disallowed_methods)] // both `tmp` and `target` are fsguard DestPaths
fn write_atomic_dest(tmp: &DestPath, target: &DestPath, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(tmp.as_path())?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(tmp.as_path(), target.as_path())
}

/// Preserves a corrupt settings file as `backup` without ever overwriting an existing backup.
#[allow(clippy::disallowed_methods)] // both `corrupt` and `backup` are fsguard DestPaths
fn preserve_corrupt_dest(corrupt: &DestPath, backup: &DestPath) -> io::Result<()> {
    if fs::symlink_metadata(backup.as_path()).is_ok() {
        return Ok(());
    }
    fs::rename(corrupt.as_path(), backup.as_path())
}

/// Loads `settings.json` from `data_dir`. If the file is corrupt, falls back to defaults and
/// preserves the broken file as `settings.json.bad` without overwriting an existing `.bad` file.
pub fn load(data_dir: &Path) -> LoadOutcome {
    let path = data_dir.join(SETTINGS_FILE);
    let raw = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return LoadOutcome {
                settings: Settings::default(),
                warning: None,
            };
        }
        Err(e) => {
            return LoadOutcome {
                settings: Settings::default(),
                warning: Some(format!(
                    "No se pudo leer la configuración ({e}); se usan los valores predeterminados."
                )),
            };
        }
    };

    match serde_json::from_slice::<Settings>(&raw) {
        Ok(settings) => LoadOutcome {
            settings,
            warning: None,
        },
        Err(e) => {
            if let Ok(dir_dest) = ensure_writable(data_dir, &[])
                && let (Ok(src), Ok(bak)) = (
                    dir_dest.join_name(OsStr::new(SETTINGS_FILE)),
                    dir_dest.join_name(OsStr::new(SETTINGS_BAD)),
                )
            {
                let _ = preserve_corrupt_dest(&src, &bak);
            }
            LoadOutcome {
                settings: Settings::default(),
                warning: Some(format!(
                    "La configuración estaba dañada ({e}); se conservó en {SETTINGS_BAD} y se restauraron los valores predeterminados."
                )),
            }
        }
    }
}

/// Atomically writes `settings.json` inside `data_dir` through `fsguard::DestPath`.
pub fn save(data_dir: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let roots = settings.valid_roots();
    let dir_dest = ensure_export_dir(data_dir, &roots)?;
    create_dir_dest(&dir_dest)?;
    let tmp = dir_dest
        .join_name(OsStr::new(SETTINGS_TMP))
        .map_err(io::Error::other)?;
    let target = dir_dest
        .join_name(OsStr::new(SETTINGS_FILE))
        .map_err(io::Error::other)?;
    let bytes = serde_json::to_vec_pretty(settings)?;
    write_atomic_dest(&tmp, &target, &bytes)?;
    Ok(())
}

/// Ensures `data_dir` exists outside all `roots` and returns the guarded path to `catalog.sqlite`.
pub fn db_dest_path(data_dir: &Path, roots: &[LibraryRoot]) -> Result<DestPath, SettingsError> {
    let dir_dest = ensure_export_dir(data_dir, roots)?;
    create_dir_dest(&dir_dest)?;
    dir_dest
        .join_name(OsStr::new(CATALOG_DB))
        .map_err(|e| SettingsError::Io(io::Error::other(e)))
}
