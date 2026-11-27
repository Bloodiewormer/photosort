use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};

/// A directory the user registered as a photo library. Everything below it is read-only.
#[derive(Debug)]
pub struct LibraryRoot(PathBuf);

impl LibraryRoot {
    /// The directory must exist: it is resolved through symlinks and junctions.
    pub fn new(path: &Path) -> io::Result<Self> {
        dunce::canonicalize(path).map(Self)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// A path proven to lie outside every library root. The only way to get one is [`ensure_writable`].
#[derive(Debug, Clone)]
pub struct DestPath(PathBuf);

impl DestPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    /// A child of this path, only when `name` is a single plain file name (no separators, no `..`).
    pub fn join_name(&self, name: &OsStr) -> Option<DestPath> {
        let mut parts = Path::new(name).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(_)), None) => Some(DestPath(self.0.join(name))),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub enum GuardError {
    InsideLibrary {
        path: PathBuf,
        root: PathBuf,
    },
    /// The folder is a parent of a library: exporting there would mix copies with originals.
    ContainsLibrary {
        path: PathBuf,
        root: PathBuf,
    },
    ParentDirComponent(PathBuf),
    Io(io::Error),
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsideLibrary { path, root } => {
                write!(
                    f,
                    "refusing to write inside library root {}: {}",
                    root.display(),
                    path.display()
                )
            }
            Self::ContainsLibrary { path, root } => {
                write!(
                    f,
                    "{} contains the library {}; choose a folder outside every library",
                    path.display(),
                    root.display()
                )
            }
            Self::ParentDirComponent(path) => {
                write!(f, "paths with `..` are not accepted: {}", path.display())
            }
            Self::Io(e) => write!(f, "could not resolve path: {e}"),
        }
    }
}

impl std::error::Error for GuardError {}

impl From<io::Error> for GuardError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Resolves the path the way the filesystem will: symlinks and junctions of the nearest existing
/// ancestor are followed, and the part that does not exist yet is appended as is.
fn resolve(path: &Path) -> Result<PathBuf, GuardError> {
    // shortcut: `..` is refused instead of normalised, because lexical removal is wrong through a link.
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(GuardError::ParentDirComponent(path.to_path_buf()));
    }
    let abs = std::path::absolute(path)?;
    let mut missing: Vec<OsString> = Vec::new();
    let mut current = abs.as_path();
    loop {
        match dunce::canonicalize(current) {
            Ok(mut real) => {
                real.extend(missing.iter().rev());
                return Ok(real);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                match (current.file_name(), current.parent()) {
                    (Some(name), Some(parent)) => {
                        missing.push(name.to_owned());
                        current = parent;
                    }
                    _ => return Err(e.into()),
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Component-wise and case-insensitive (NTFS): `D:\Lib2` is not inside `D:\Lib`. The `\\?\` prefix
/// that `dunce` keeps on long paths is ignored, so they compare equal to short ones.
fn key(path: &Path) -> Vec<String> {
    path.components()
        .map(|c| {
            let text = match c {
                Component::Prefix(p) => match p.kind() {
                    Prefix::VerbatimDisk(disk) => format!("{}:", disk as char),
                    Prefix::VerbatimUNC(server, share) => format!(
                        r"\\{}\{}",
                        server.to_string_lossy(),
                        share.to_string_lossy()
                    ),
                    _ => c.as_os_str().to_string_lossy().into_owned(),
                },
                _ => c.as_os_str().to_string_lossy().into_owned(),
            };
            text.to_lowercase()
        })
        .collect()
}

/// Like [`ensure_writable`], and also refuses a folder that contains a library, so copies are never
/// mixed in with originals.
pub fn ensure_export_dir(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    let dest = ensure_writable(path, roots)?;
    let dest_key = key(&dest.0);
    for root in roots {
        if key(&root.0).starts_with(&dest_key) {
            return Err(GuardError::ContainsLibrary {
                path: dest.0,
                root: root.0.clone(),
            });
        }
    }
    Ok(dest)
}

/// Returns a [`DestPath`] only if `path` is neither a library root nor below one.
pub fn ensure_writable(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    let real = resolve(path)?;
    let candidate = key(&real);
    for root in roots {
        let root_key = key(&root.0);
        if candidate.starts_with(&root_key) {
            return Err(GuardError::InsideLibrary {
                path: real,
                root: root.0.clone(),
            });
        }
    }
    Ok(DestPath(real))
}
