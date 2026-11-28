use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf, Prefix};
use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};

/// A directory the user registered as a photo library. Nothing below it is ever deleted, rewritten or
/// overwritten; files only move through [`move_no_replace`] and [`recycle`].
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

    /// True when `path` is this library or lies below it, compared as the guard compares.
    pub fn contains(&self, path: &Path) -> bool {
        key(path).starts_with(&key(&self.0))
    }
}

/// A path proven to lie outside every protected folder: the library roots for [`ensure_writable`], the
/// app's own data for [`ensure_move_dir`]. It grows only through [`DestPath::join_name`].
#[derive(Debug, Clone)]
pub struct DestPath {
    path: PathBuf,
    /// The folder that was validated: nothing derived from it may lead outside of it.
    base: PathBuf,
    roots: Arc<[PathBuf]>,
}

impl DestPath {
    pub fn as_path(&self) -> &Path {
        &self.path
    }

    /// A child of this path, only when `name` is a single plain file name (no separators, no `..`)
    /// and, if something already has that name, it is not a link that leads elsewhere.
    pub fn join_name(&self, name: &OsStr) -> Result<DestPath, GuardError> {
        let mut parts = Path::new(name).components();
        if !matches!(
            (parts.next(), parts.next()),
            (Some(Component::Normal(_)), None)
        ) {
            return Err(GuardError::InvalidName(name.to_owned()));
        }
        let child = DestPath {
            path: self.path.join(name),
            base: self.base.clone(),
            roots: Arc::clone(&self.roots),
        };
        child.verify()?;
        Ok(child)
    }

    /// Resolves the path as the filesystem would right now. A link can appear after the path was
    /// built, so call this again after creating a folder and before writing.
    pub fn verify(&self) -> Result<(), GuardError> {
        let real = resolve(&self.path)?;
        refuse_inside(&real, &self.roots)?;
        if !key(&real).starts_with(&key(&self.base)) {
            return Err(GuardError::LeavesDestination {
                path: self.path.clone(),
                target: real,
            });
        }
        Ok(())
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
    InvalidName(OsString),
    /// Only ordinary files are moved: not folders, links or other reparse points.
    NotAMovableFile(PathBuf),
    /// Photos are never moved into the app's own data.
    InsideAppData {
        path: PathBuf,
    },
    /// A link below the destination points outside of it.
    LeavesDestination {
        path: PathBuf,
        target: PathBuf,
    },
    /// A link whose target is missing: where a write would end up cannot be checked.
    BrokenLink(PathBuf),
    Io(io::Error),
}

impl GuardError {
    /// True when a link is what got a path refused. For paths grown from an already validated
    /// [`DestPath`], ending up inside a library can only happen through a link.
    pub fn is_link(&self) -> bool {
        matches!(
            self,
            Self::InsideLibrary { .. } | Self::LeavesDestination { .. } | Self::BrokenLink(_)
        )
    }
}

/// The reason of an I/O error, in Spanish for the common cases. Any other error keeps its own text:
/// ours is already in Spanish, and the system's comes in the language of Windows.
pub fn io_reason(e: &io::Error) -> String {
    // Only the system's errors are replaced: ours carry an inner error that says what happened.
    let known = match e.kind() {
        _ if e.get_ref().is_some() => None,
        io::ErrorKind::NotFound => Some("no se encontró el archivo o la carpeta"),
        io::ErrorKind::PermissionDenied => Some("acceso denegado"),
        io::ErrorKind::StorageFull => Some("no queda espacio en el disco"),
        io::ErrorKind::AlreadyExists => Some("ya existe un archivo con ese nombre"),
        _ => None,
    };
    known.map_or_else(|| e.to_string(), str::to_string)
}

/// Only the last component: the full path is already shown next to the message, or adds nothing.
fn short(path: &Path) -> std::path::Display<'_> {
    Path::new(path.file_name().unwrap_or(path.as_os_str())).display()
}

impl fmt::Display for GuardError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InsideLibrary { root, .. } => write!(
                f,
                "la ruta lleva dentro de la biblioteca «{}», donde no se escribe nada",
                short(root)
            ),
            Self::ContainsLibrary { root, .. } => write!(
                f,
                "la carpeta contiene la biblioteca «{}»; elige otra fuera de todas las bibliotecas",
                short(root)
            ),
            Self::ParentDirComponent(_) => f.write_str("no se aceptan rutas con «..»"),
            Self::InvalidName(name) => write!(
                f,
                "«{}» no es un nombre de archivo válido",
                name.to_string_lossy()
            ),
            Self::LeavesDestination { path, .. } => write!(
                f,
                "el enlace «{}» sale de la carpeta de destino",
                short(path)
            ),
            Self::BrokenLink(path) => write!(f, "el enlace «{}» está roto", short(path)),
            Self::NotAMovableFile(path) => write!(
                f,
                "«{}» no es un archivo normal; las carpetas y los enlaces no se mueven",
                short(path)
            ),
            Self::InsideAppData { .. } => {
                f.write_str("la carpeta está dentro de los datos de Hojeo; elige otra")
            }
            Self::Io(e) => write!(f, "no se pudo comprobar la ruta: {}", io_reason(e)),
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
    // `name:stream` writes into `name`, through it if it is a link: never a plain name on Windows.
    #[cfg(windows)]
    if let Some(Component::Normal(name)) = path
        .components()
        .find(|c| matches!(c, Component::Normal(n) if n.as_encoded_bytes().contains(&b':')))
    {
        return Err(GuardError::InvalidName(name.to_owned()));
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
                // The entry is there but its target is not: it is a link, not a missing folder.
                if std::fs::symlink_metadata(current).is_ok() {
                    return Err(GuardError::BrokenLink(current.to_path_buf()));
                }
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

fn refuse_inside(real: &Path, roots: &[PathBuf]) -> Result<(), GuardError> {
    let candidate = key(real);
    match roots.iter().find(|root| candidate.starts_with(&key(root))) {
        Some(root) => Err(GuardError::InsideLibrary {
            path: real.to_path_buf(),
            root: root.clone(),
        }),
        None => Ok(()),
    }
}

/// Like [`ensure_writable`], and also refuses a folder that contains a library, so copies are never
/// mixed in with originals.
pub fn ensure_export_dir(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    let dest = ensure_writable(path, roots)?;
    let dest_key = key(&dest.path);
    for root in roots {
        if key(&root.0).starts_with(&dest_key) {
            return Err(GuardError::ContainsLibrary {
                path: dest.path,
                root: root.0.clone(),
            });
        }
    }
    Ok(dest)
}

/// Returns a [`DestPath`] only if `path` is neither a library root nor below one.
pub fn ensure_writable(path: &Path, roots: &[LibraryRoot]) -> Result<DestPath, GuardError> {
    let real = resolve(path)?;
    let roots: Arc<[PathBuf]> = roots.iter().map(|r| r.0.clone()).collect();
    refuse_inside(&real, &roots)?;
    Ok(DestPath {
        base: real.clone(),
        path: real,
        roots,
    })
}

/// A file that may be moved or sent to the Recycle Bin: an existing ordinary file, not a link. It only
/// comes from [`ensure_move_source`].
#[derive(Debug, Clone)]
pub struct MoveSource(PathBuf);

impl MoveSource {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

/// True for symbolic links and, on Windows, for junctions and every other reparse point.
fn is_link_or_reparse(md: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    md.file_type().is_symlink()
}

// shortcut: cloud placeholders (OneDrive files on demand) are reparse points too and stay where they
// are; tell the tags apart if the user needs to move them.
pub fn ensure_move_source(path: &Path) -> Result<MoveSource, GuardError> {
    // First the checks every path gets: `..` and `name:stream` are refused here.
    let real = resolve(path)?;
    let md = std::fs::symlink_metadata(path)?;
    if !md.is_file() || is_link_or_reparse(&md) {
        return Err(GuardError::NotAMovableFile(path.to_path_buf()));
    }
    Ok(MoveSource(real))
}

/// The destination of a move: inside a library or outside all of them, never inside the app's data.
pub fn ensure_move_dir(path: &Path, app_data: &Path) -> Result<DestPath, GuardError> {
    let real = resolve(path)?;
    let roots: Arc<[PathBuf]> = Arc::from([resolve(app_data)?]);
    if refuse_inside(&real, &roots).is_err() {
        return Err(GuardError::InsideAppData { path: real });
    }
    Ok(DestPath {
        base: real.clone(),
        path: real,
        roots,
    })
}

/// Moves an original within its volume without touching its bytes, dates or attributes, and never over
/// an existing file. Another volume gives [`io::ErrorKind::CrossesDevices`] and a taken name
/// [`io::ErrorKind::AlreadyExists`].
pub fn move_no_replace(src: &MoveSource, dst: &DestPath) -> io::Result<()> {
    dst.verify().map_err(io::Error::other)?;
    rename_without_replacing(&src.0, &dst.path, false)
}

/// Gives a finished temporary file its final name inside a destination, never over an existing file.
/// It returns once the new name is on disk: an original may be retired right after.
pub fn rename_no_replace(from: &DestPath, to: &DestPath) -> io::Result<()> {
    from.verify().map_err(io::Error::other)?;
    to.verify().map_err(io::Error::other)?;
    rename_without_replacing(&from.path, &to.path, true)
}

/// The `\\?\` form, which has no length limit and keeps names exactly as written. Only the folder is
/// resolved, so the last component itself is what gets renamed.
#[cfg(windows)]
fn verbatim_wide(path: &Path) -> io::Result<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    };
    let full = std::fs::canonicalize(parent)?.join(name);
    Ok(full.as_os_str().encode_wide().chain([0]).collect())
}

#[cfg(windows)]
fn rename_without_replacing(from: &Path, to: &Path, write_through: bool) -> io::Result<()> {
    use windows_sys::Win32::Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW};
    let (from, to) = (verbatim_wide(from)?, verbatim_wide(to)?);
    let flags = if write_through {
        MOVEFILE_WRITE_THROUGH
    } else {
        0
    };
    // SAFETY: both are null-terminated UTF-16. Never replacing, never copying across volumes.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// shortcut: comprobar y renombrar no es atómico fuera de Windows, solo afecta al CI de Linux
#[cfg(not(windows))]
#[allow(clippy::disallowed_methods)] // único lugar que mueve o manda a la Papelera un original; nunca borra
fn rename_without_replacing(from: &Path, to: &Path, _write_through: bool) -> io::Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(io::Error::from(io::ErrorKind::AlreadyExists));
    }
    std::fs::rename(from, to)
}

/// `path` as the shell takes it, ending in two NULs, or `None` when it could read it as something else.
/// The shell does not take the `\\?\` form, and without it Windows drops trailing dots and spaces,
/// which could name another file. Network paths have no Recycle Bin.
#[cfg(windows)]
fn shell_path(path: &Path) -> Option<Vec<u16>> {
    use std::os::windows::ffi::OsStrExt;
    const MAX_PATH: usize = 260;
    let simple = dunce::simplified(path);
    let local = matches!(
        simple.components().next(),
        Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_))
    );
    let mut wide: Vec<u16> = simple.as_os_str().encode_wide().collect();
    let wildcard = wide
        .iter()
        .any(|&c| c == u16::from(b'*') || c == u16::from(b'?'));
    if !local || !simple.is_absolute() || wildcard || wide.len() >= MAX_PATH {
        return None;
    }
    wide.extend([0, 0]);
    Some(wide)
}

/// The application's main window, so that a question from Windows opens in front of it.
static DIALOG_OWNER: AtomicIsize = AtomicIsize::new(0);

/// Tells [`recycle`] which window owns the questions Windows asks. `0` means none.
pub fn set_dialog_owner(hwnd: isize) {
    DIALOG_OWNER.store(hwnd, Ordering::Relaxed);
}

/// True only on a local fixed disk: removable drives, network shares and discs may have no Recycle Bin.
#[cfg(windows)]
fn on_fixed_disk(path: &[u16]) -> bool {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetVolumePathNameW};
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
    let mut volume = [0u16; 261];
    // SAFETY: `path` is null-terminated UTF-16 and `volume` is as long as stated.
    unsafe {
        GetVolumePathNameW(path.as_ptr(), volume.as_mut_ptr(), volume.len() as u32) != 0
            && GetDriveTypeW(volume.as_ptr()) == DRIVE_FIXED
    }
}

/// Sends an original to the Recycle Bin; callers do it only once a verified copy exists. Like Del in
/// Explorer: Windows recycles without asking (as configured by default) and always asks before
/// deleting for good. `Ok(true)` only when the file really left its place; `Ok(false)` leaves it
/// there: not a fixed disk, a path the shell could misread, or the user said no.
#[cfg(windows)]
pub fn recycle(file: &MoveSource) -> io::Result<bool> {
    use windows_sys::Win32::Foundation::ERROR_CANCELLED;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_SILENT, FOF_WANTNUKEWARNING, SHFILEOPSTRUCTW,
        SHFileOperationW,
    };
    // It may have been replaced by a link or a folder since it was checked.
    let file = ensure_move_source(&file.0).map_err(io::Error::other)?;
    let Some(from) = shell_path(&file.0) else {
        return Ok(false);
    };
    if !on_fixed_disk(&from) {
        return Ok(false);
    }
    let mut operation = SHFILEOPSTRUCTW {
        hwnd: DIALOG_OWNER.load(Ordering::Relaxed) as _,
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        // No FOF_NOCONFIRMATION: it would also answer yes to deleting for good.
        fFlags: (FOF_ALLOWUNDO | FOF_SILENT | FOF_WANTNUKEWARNING) as u16,
        ..Default::default()
    };
    // SAFETY: `from` ends in two NULs and outlives the call; the other pointers are null, and a
    // window handle that is no longer valid only makes the call fail.
    let code = unsafe { SHFileOperationW(&mut operation) };
    if code as u32 == ERROR_CANCELLED {
        return Ok(false);
    }
    if code != 0 {
        return Err(io::Error::other(format!(
            "la Papelera de Windows devolvió el error {code}"
        )));
    }
    // A file that is still there was not retired, whether or not the user aborted.
    let gone =
        matches!(std::fs::symlink_metadata(&file.0), Err(e) if e.kind() == io::ErrorKind::NotFound);
    Ok(gone && operation.fAnyOperationsAborted == 0)
}

/// There is no Recycle Bin to rely on outside Windows, so nothing is ever retired.
#[cfg(not(windows))]
pub fn recycle(_file: &MoveSource) -> io::Result<bool> {
    Ok(false)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// A junction needs no administrator rights, unlike a symbolic link to a file: it is the reparse
    /// point that can always be made.
    #[allow(clippy::disallowed_methods)] // fixtures in a temp folder
    #[test]
    fn a_reparse_point_is_told_apart_from_an_ordinary_file() {
        let tmp = tempfile::tempdir().unwrap();
        let (file, folder, link) = (
            tmp.path().join("a.jpg"),
            tmp.path().join("fotos"),
            tmp.path().join("enlace"),
        );
        std::fs::write(&file, b"a").unwrap();
        std::fs::create_dir(&folder).unwrap();
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&folder)
            .output()
            .unwrap();
        assert!(made.status.success(), "could not create junction: {made:?}");
        let check = |path: &Path| is_link_or_reparse(&std::fs::symlink_metadata(path).unwrap());
        assert!(check(&link));
        assert!(!check(&file));
        assert!(!check(&folder));
    }

    /// Only the question is tested: temp folders are on a fixed disk, here and in CI.
    #[test]
    fn a_temp_folder_is_on_a_fixed_disk() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(on_fixed_disk(
            &shell_path(&tmp.path().join("a.jpg")).unwrap()
        ));
    }

    /// The Recycle Bin itself is never called by tests: this is the part that decides what it receives.
    #[test]
    fn the_shell_only_gets_paths_it_cannot_misread() {
        let text = |p: &str| shell_path(Path::new(p)).map(|w| String::from_utf16(&w).unwrap());
        assert_eq!(
            text(r"\\?\C:\fotos\a.jpg").as_deref(),
            Some("C:\\fotos\\a.jpg\0\0")
        );
        // Without the prefix, Windows would take this for `a.jpg`, another file.
        assert_eq!(text(r"\\?\C:\fotos\a.jpg "), None);
        assert_eq!(text(r"\\?\C:\fotos\a.jpg."), None);
        assert_eq!(text(r"\\?\UNC\servidor\fotos\a.jpg"), None);
        assert_eq!(text(r"\\servidor\fotos\a.jpg"), None);
        assert_eq!(text(r"C:\fotos\*.jpg"), None);
        assert_eq!(text(r"a.jpg"), None);
        assert_eq!(text(&format!(r"\\?\C:\{}\a.jpg", "x".repeat(300))), None);
    }
}
