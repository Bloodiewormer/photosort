mod jpeg;
mod moves;
mod xmp;

pub use jpeg::{embed_xmp_in_jpeg, extract_xmp_from_jpeg, is_jpeg};
pub use moves::{MoveContext, UndoReport, move_with_options, undo_last_move};
pub use xmp::{XmpData, generate_xmp, xml_escape};

use catalog::{AssetWork, Assignment, LabelKind, Status};
use fsguard::{DestPath, GuardError, io_reason};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FOLDER_CHARS: usize = 80;

#[derive(Debug, Clone, Default)]
pub struct ExportItem {
    pub source: PathBuf,
    /// What the catalog knows about the image; goes into the manifest.
    pub labels: Vec<Assignment>,
    /// Optional work ownership relation (`belongs_to`); goes into the manifest when not rejected.
    pub belongs_to: Option<AssetWork>,
    /// Named persons identified in the photo.
    pub persons: Vec<String>,
    /// Hierarchical category paths, e.g. "Manga|Scanlation|Página promocional".
    pub hierarchical_categories: Vec<String>,
    /// Library root that contains `source`, used by [`Layout::PreserveFolders`].
    pub root: Option<PathBuf>,
}

impl ExportItem {
    pub fn new(source: PathBuf) -> Self {
        Self {
            source,
            labels: Vec::new(),
            belongs_to: None,
            persons: Vec::new(),
            hierarchical_categories: Vec::new(),
            root: None,
        }
    }
}

/// How exported copies are arranged inside the destination folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Flat,
    ByCategory,
    ByEntity,
    ByYear,
    PreserveFolders,
}

/// Whether the originals stay where they are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Operation {
    #[default]
    Copy,
    Move,
}

impl Operation {
    /// The word the manifest uses.
    fn name(self) -> &'static str {
        match self {
            Self::Copy => "copiar",
            Self::Move => "mover",
        }
    }
}

/// Configuration options for export operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportOptions {
    pub layout: Layout,
    pub write_xmp_sidecars: bool,
    pub embed_jpeg_tags: bool,
    /// What [`preview_with_options`] counts. Moving is only done by [`move_with_options`].
    pub operation: Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportProgress {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Continue,
    Stop,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Copied(PathBuf),
    /// An identical file (same SHA-256) was already there, so nothing was written.
    AlreadyPresent(PathBuf),
    /// The original now is at this path.
    Moved(PathBuf),
    /// A move left a copy at `dest`, but the original is still where it was.
    KeptOriginal {
        dest: PathBuf,
        reason: String,
    },
    /// The guard refused to follow a link in the destination. The manifest records it as failed.
    Blocked(String),
    Failed(String),
}

#[derive(Debug)]
pub struct ExportReport {
    pub outcomes: Vec<(PathBuf, Outcome)>,
    /// Per source, the manifest's `warning`: something optional that did not happen.
    pub warnings: Vec<(PathBuf, String)>,
    pub manifest_json: PathBuf,
    pub manifest_csv: PathBuf,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub operation: Operation,
    /// Files that would be copied or moved.
    pub files: usize,
    pub bytes: u64,
    /// Of `files`, how many are on the destination's disk, where moving is instant.
    pub same_volume: usize,
    /// Of `files`, how many are on another disk: moving them copies and then recycles.
    pub other_volume: usize,
    pub bytes_other_volume: u64,
    /// Of `files`, how many would get a numbered name because the name is taken by different bytes.
    pub renamed: usize,
    pub already_present: usize,
    /// Sources that could not be read.
    pub unreadable: usize,
    /// Sources whose place in the destination goes through a link the guard refuses to follow.
    pub blocked: usize,
    /// Available bytes on the destination volume, queried from its nearest existing ancestor.
    pub free_bytes: Option<u64>,
}

impl Preview {
    /// True when the bytes to write fit in the destination volume's free space (or when unknown).
    /// Moving within a disk writes nothing.
    pub fn fits(&self) -> bool {
        let needed = match self.operation {
            Operation::Copy => self.bytes,
            Operation::Move => self.bytes_other_volume,
        };
        self.free_bytes.is_none_or(|free| needed <= free)
    }

    fn count_refused(&mut self, e: &GuardError) {
        if e.is_link() {
            self.blocked += 1;
        } else {
            self.unreadable += 1;
        }
    }
}

/// Produces a safe single Windows/NTFS folder name from arbitrary label text.
pub fn sanitize_folder_name(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let mut s: String = replaced
        .trim_end_matches(['.', ' '])
        .chars()
        .take(MAX_FOLDER_CHARS)
        .collect();
    s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() {
        return "_".to_string();
    }
    if is_windows_reserved_name(&s) {
        s.insert(0, '_');
        if s.chars().count() > MAX_FOLDER_CHARS {
            s = s.chars().take(MAX_FOLDER_CHARS).collect();
            s = s.trim_end_matches(['.', ' ']).to_string();
        }
    }
    if s.is_empty() { "_".to_string() } else { s }
}

fn is_windows_reserved_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or("").trim_end_matches(' ');
    let upper = base.to_ascii_uppercase();
    matches!(
        upper.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn first_label_folder(labels: &[Assignment], kind: LabelKind, fallback: &str) -> String {
    labels
        .iter()
        .filter(|l| l.status != Status::Rejected && l.kind == kind)
        .min_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        })
        .map(|l| sanitize_folder_name(&l.name))
        .unwrap_or_else(|| fallback.to_string())
}

fn year_from_system_time(time: SystemTime) -> Option<String> {
    let secs: i64 = match time.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).ok()?,
        Err(e) => {
            let d = e.duration();
            let s = i64::try_from(d.as_secs()).ok()?;
            if d.subsec_nanos() > 0 { -s - 1 } else { -s }
        }
    };
    let days = secs.div_euclid(86_400);
    // Howard Hinnant's civil-from-days algorithm (proleptic Gregorian calendar).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (0..=9999).contains(&year).then(|| format!("{year:04}"))
}

/// Strips `root` prefix from `source`, with case-insensitive fallback on Windows.
fn relative_to_root(source: &Path, root: &Path) -> Option<PathBuf> {
    if let Ok(rel) = source.strip_prefix(root) {
        return Some(rel.to_path_buf());
    }
    let s_comps: Vec<_> = source.components().collect();
    let r_comps: Vec<_> = root.components().collect();
    if s_comps.len() >= r_comps.len() {
        let matches = s_comps.iter().zip(r_comps.iter()).all(|(sc, rc)| {
            sc.as_os_str().to_string_lossy().to_lowercase()
                == rc.as_os_str().to_string_lossy().to_lowercase()
        });
        if matches {
            let mut rel = PathBuf::new();
            for c in &s_comps[r_comps.len()..] {
                rel.push(c);
            }
            return Some(rel);
        }
    }
    None
}

/// Computes the folder component names for an item according to layout.
fn subfolder_components_for(
    item: &ExportItem,
    md: &fs::Metadata,
    layout: Layout,
    root_folders: &HashMap<&Path, OsString>,
) -> Result<Vec<OsString>, String> {
    match layout {
        Layout::Flat => Ok(Vec::new()),
        Layout::ByCategory => Ok(vec![OsString::from(first_label_folder(
            &item.labels,
            LabelKind::Category,
            "Sin categoría",
        ))]),
        Layout::ByEntity => Ok(vec![OsString::from(first_label_folder(
            &item.labels,
            LabelKind::Entity,
            "Sin obra",
        ))]),
        Layout::ByYear => Ok(vec![OsString::from(
            md.modified()
                .ok()
                .and_then(year_from_system_time)
                .unwrap_or_else(|| "Sin fecha".to_string()),
        )]),
        Layout::PreserveFolders => {
            let Some(root) = &item.root else {
                return Err("no se sabe de qué biblioteca es el archivo".to_string());
            };
            let Some(rel) = relative_to_root(&item.source, root) else {
                return Err("el archivo no está dentro de su biblioteca".to_string());
            };
            let rel_parent = rel.parent().unwrap_or_else(|| Path::new(""));
            let mut comps = Vec::new();
            if let Some(folder) = root_folders.get(root.as_path()) {
                comps.push(folder.clone());
            }
            for c in rel_parent.components() {
                match c {
                    Component::Normal(p) => comps.push(p.to_os_string()),
                    _ => return Err("la ruta de origen tiene una parte no válida".to_string()),
                }
            }
            Ok(comps)
        }
    }
}

/// Where a source would go: `same` means the destination already holds identical bytes.
struct Resolved {
    target: DestPath,
    same: bool,
}

fn key(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

/// First free name (`a.jpg`, `a (1).jpg`, ...) or the identical file already there. `reserved` holds
/// names (mapped to their SHA-256) that an earlier item of the same preview claimed in this folder.
fn resolve(
    dest: &DestPath,
    name: &OsStr,
    sha: &str,
    reserved: &HashMap<String, String>,
    tagged: Option<&[u8]>,
) -> Result<Resolved, GuardError> {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(name);
    let mut n = 0u32;
    loop {
        let mut candidate = OsString::from(stem);
        if n == 0 {
            candidate = name.to_owned();
        } else {
            candidate.push(format!(" ({n})"));
            if let Some(ext) = path.extension() {
                candidate.push(".");
                candidate.push(ext);
            }
        }
        n += 1;
        let target = dest.join_name(&candidate)?;
        if let Some(existing_sha) = reserved.get(&key(&candidate)) {
            if existing_sha == sha {
                return Ok(Resolved { target, same: true });
            }
            continue;
        }
        if fs::symlink_metadata(target.as_path()).is_err() {
            return Ok(Resolved {
                target,
                same: false,
            });
        }
        if scanner::sha256_file(target.as_path()).is_ok_and(|h| h == sha)
            || tagged.is_some_and(|t| fs::read(target.as_path()).is_ok_and(|b| b == t))
        {
            return Ok(Resolved { target, same: true });
        }
    }
}

/// Metadata is read before the bytes so the original's access time is the one that gets restored.
fn read_source(path: &Path) -> Result<(fs::Metadata, String), String> {
    let md = fs::metadata(path).map_err(|e| io_reason(&e))?;
    if !md.is_file() {
        return Err("no es un archivo".into());
    }
    let sha = scanner::sha256_file(path).map_err(|e| io_reason(&e))?;
    Ok((md, sha))
}

fn times_of(md: &fs::Metadata) -> io::Result<FileTimes> {
    let mut times = FileTimes::new().set_modified(md.modified()?);
    if let Ok(accessed) = md.accessed() {
        times = times.set_accessed(accessed);
    }
    #[cfg(windows)]
    if let Ok(created) = md.created() {
        times = std::os::windows::fs::FileTimesExt::set_created(times, created);
    }
    Ok(times)
}

#[allow(clippy::disallowed_methods)] // creates a single subfolder component inside the DestPath destination
fn ensure_subfolder(dir: &DestPath) -> io::Result<()> {
    match fs::create_dir(dir.as_path()) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    // What is there now may be a link, not the folder that was just asked for.
    dir.verify().map_err(io::Error::other)
}

/// First-level folder of each library root for [`Layout::PreserveFolders`], only when the items come
/// from several roots: the root's own name, numbered when two share it (`D:\Fotos`, `E:\Fotos (2)`).
fn root_folders(items: &[ExportItem]) -> HashMap<&Path, OsString> {
    let mut roots: Vec<&Path> = items.iter().filter_map(|it| it.root.as_deref()).collect();
    roots.sort();
    roots.dedup();
    let mut out = HashMap::new();
    if roots.len() < 2 {
        return out;
    }
    let mut taken = HashSet::new();
    for root in roots {
        // A drive root such as `D:\` has no file name: use the whole path, sanitized.
        let base = sanitize_folder_name(
            &root
                .file_name()
                .unwrap_or(root.as_os_str())
                .to_string_lossy(),
        );
        let mut name = base.clone();
        let mut n = 2;
        while !taken.insert(name.to_lowercase()) {
            name = format!("{base} ({n})");
            n += 1;
        }
        out.insert(root, OsString::from(name));
    }
    out
}

/// Ensures each subfolder component exists sequentially inside `dest`, returning the leaf `DestPath`.
fn ensure_dir_chain(dest: &DestPath, comps: &[OsString]) -> io::Result<DestPath> {
    let mut cur = dest.clone();
    for comp in comps {
        cur = cur.join_name(comp).map_err(io::Error::other)?;
        ensure_subfolder(&cur)?;
    }
    Ok(cur)
}

/// Resolves target directory by traversing subfolder components using `join_name`.
fn resolve_target_dir(dest: &DestPath, comps: &[OsString]) -> Result<DestPath, GuardError> {
    let mut cur = dest.clone();
    for comp in comps {
        cur = cur.join_name(comp)?;
    }
    Ok(cur)
}

/// Writes `part`, checks it against `sha`, gives it the original's dates and renames it to `target`
/// without replacing anything. Both paths are in the destination. A failed copy leaves nothing behind.
#[allow(clippy::disallowed_methods)] // the only place that creates files: every path is a DestPath
fn write_verified(
    source: &Path,
    part: &DestPath,
    target: &DestPath,
    sha: &str,
    md: &fs::Metadata,
) -> io::Result<()> {
    let mut created = false;
    let result = (|| {
        let mut src = File::open(source)?;
        part.verify().map_err(io::Error::other)?;
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(part.as_path())?;
        created = true;
        io::copy(&mut src, &mut out)?;
        out.sync_all()?;
        if scanner::sha256_file(part.as_path())? != sha {
            return Err(io::Error::other(
                "la copia no coincide con el original (SHA-256)",
            ));
        }
        out.set_times(times_of(md)?)?;
        drop(out);
        // A long copy gives time for the folder to change under us: both paths are checked again.
        fsguard::rename_no_replace(part, target).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                io::Error::new(e.kind(), "otro archivo ocupó el nombre mientras se copiaba")
            } else {
                e
            }
        })
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(part.as_path()); // our own temporary file, in the destination
    }
    result
}

/// A free name for the temporary file a copy to `target` is written to: `name.part`, or
/// `name.1.part`, `name.2.part`... One left behind by a power cut stays as it is.
fn part_of(dir: &DestPath, target: &DestPath) -> io::Result<DestPath> {
    for n in 0u32.. {
        let mut name = target.as_path().file_name().unwrap().to_owned();
        if n > 0 {
            name.push(format!(".{n}"));
        }
        name.push(".part");
        let part = dir.join_name(&name).map_err(io::Error::other)?;
        if fs::symlink_metadata(part.as_path()).is_err() {
            return Ok(part);
        }
    }
    unreachable!("the counter never ends before a free name is found")
}

/// Inserts or replaces XMP metadata in an already verified JPEG copy located at `target`.
/// Writes a `.tagpart` beside it, checks it, gives it the original's dates and renames it over the copy,
/// so a failure leaves the verified copy as it was. Returns the new SHA-256, or `None` if not a JPEG.
#[allow(clippy::disallowed_methods)] // every path is a DestPath; the only file replaced is our own copy
fn embed_jpeg_copy(
    target: &DestPath,
    target_dir: &DestPath,
    xmp_xml: &str,
    md: &fs::Metadata,
) -> Result<Option<String>, String> {
    if !starts_as_jpeg(target.as_path()) {
        return Ok(None);
    }
    let bytes = fs::read(target.as_path()).map_err(|e| io_reason(&e))?;
    let tagged = jpeg::embed_xmp_in_jpeg(&bytes, xmp_xml.as_bytes())?;

    let mut tagpart_name = target.as_path().file_name().unwrap().to_owned();
    tagpart_name.push(".tagpart");
    let tagpart = target_dir
        .join_name(&tagpart_name)
        .map_err(|e| e.to_string())?;

    let mut created = false;
    let write_res = (|| -> io::Result<String> {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(tagpart.as_path())?;
        created = true;
        f.write_all(&tagged)?;
        f.sync_all()?;
        if fs::read(tagpart.as_path())? != tagged {
            return Err(io::Error::other(
                "la copia con etiquetas no se escribió bien",
            ));
        }
        // Hashed before the dates are set, like write_verified, so reading it does not move them.
        let new_sha = scanner::sha256_file(tagpart.as_path())?;
        f.set_times(times_of(md)?)?;
        drop(f);
        fs::rename(tagpart.as_path(), target.as_path())?;
        Ok(new_sha)
    })();

    match write_res {
        Ok(new_sha) => Ok(Some(new_sha)),
        Err(e) => {
            if created {
                let _ = fs::remove_file(tagpart.as_path()); // our own temporary file, in the destination
            }
            Err(io_reason(&e))
        }
    }
}

/// The XMP packet for what the catalog knows about `item` (rejected labels left out).
fn xmp_for(item: &ExportItem) -> String {
    let names = |kind: LabelKind| -> Vec<&str> {
        item.labels
            .iter()
            .filter(|l| l.status != Status::Rejected && l.kind == kind)
            .map(|l| l.name.as_str())
            .collect()
    };
    let mut works = names(LabelKind::Entity);
    if let Some(w) = &item.belongs_to
        && w.status != Status::Rejected
        && !w.work_name.is_empty()
        && !works.contains(&w.work_name.as_str())
    {
        works.push(&w.work_name);
    }
    let persons: Vec<&str> = item.persons.iter().map(String::as_str).collect();
    let hierarchical: Vec<&str> = item
        .hierarchical_categories
        .iter()
        .map(String::as_str)
        .collect();
    xmp::generate_xmp(&xmp::XmpData {
        categories: &names(LabelKind::Category),
        tags: &names(LabelKind::Tag),
        works: &works,
        persons: &persons,
        hierarchical_categories: &hierarchical,
    })
}

/// True when the file begins with the JPEG SOI marker; reads two bytes, so videos are never loaded.
fn starts_as_jpeg(path: &Path) -> bool {
    let mut head = [0u8; 2];
    File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && jpeg::is_jpeg(&head)
}

/// The bytes a tagged copy of `source` has, so exporting again recognises the copy it left.
fn tagged_bytes_of(source: &Path, xmp_xml: &str) -> Option<Vec<u8>> {
    if !starts_as_jpeg(source) {
        return None;
    }
    let bytes = fs::read(source).ok()?;
    jpeg::embed_xmp_in_jpeg(&bytes, xmp_xml.as_bytes()).ok()
}

struct Done {
    outcome: Outcome,
    sha: String,
    tagged_sha: Option<String>,
    /// Something optional (sidecar, tagging) that did not happen; the copy itself is fine.
    warning: Option<String>,
    size: u64,
}

/// Copies one item, or moves it when there is a `mover` (its context and batch number).
fn copy_one(
    item: &ExportItem,
    dest: &DestPath,
    options: ExportOptions,
    root_folders: &HashMap<&Path, OsString>,
    mover: Option<(&MoveContext, i64)>,
) -> io::Result<Done> {
    let (md, sha) = read_source(&item.source).map_err(io::Error::other)?;
    let name = item
        .source
        .file_name()
        .ok_or_else(|| io::Error::other("la ruta no tiene nombre de archivo"))?;
    let comps = subfolder_components_for(item, &md, options.layout, root_folders)
        .map_err(io::Error::other)?;
    let target_dir = ensure_dir_chain(dest, &comps)?;

    let xmp_xml = (options.write_xmp_sidecars || options.embed_jpeg_tags).then(|| xmp_for(item));
    let tagged = match &xmp_xml {
        Some(x) if options.embed_jpeg_tags => tagged_bytes_of(&item.source, x),
        _ => None,
    };
    let resolved = resolve(&target_dir, name, &sha, &HashMap::new(), tagged.as_deref())
        .map_err(io::Error::other)?;
    let mut path = resolved.target.as_path().to_path_buf();
    let mut tagged_sha = None;
    let mut warnings: Vec<String> = Vec::new();

    let outcome = if resolved.same {
        if tagged.is_some() {
            // The copy found may be the tagged one an earlier export left.
            let on_disk = scanner::sha256_file(&path)?;
            if on_disk != sha {
                tagged_sha = Some(on_disk);
            }
        }
        Outcome::AlreadyPresent(path.clone())
    } else if let Some((ctx, batch)) = mover {
        let place = moves::Place {
            dir: &target_dir,
            target: &resolved.target,
        };
        let (outcome, log_warning) = moves::move_one(&item.source, place, &sha, &md, ctx, batch)?;
        warnings.extend(log_warning);
        if let Outcome::Moved(moved) | Outcome::KeptOriginal { dest: moved, .. } = &outcome {
            path.clone_from(moved);
        }
        outcome
    } else {
        let part = part_of(&target_dir, &resolved.target)?;
        write_verified(&item.source, &part, &resolved.target, &sha, &md)?;
        Outcome::Copied(path.clone())
    };

    if let Some(xmp_xml) = &xmp_xml {
        if options.write_xmp_sidecars {
            let base_name = resolved
                .target
                .as_path()
                .file_name()
                .unwrap()
                .to_string_lossy();
            // An identical sidecar from an earlier export is kept instead of adding `name (1).xmp`.
            let same = target_dir
                .join_name(OsStr::new(&format!("{base_name}.xmp")))
                .is_ok_and(|p| fs::read(p.as_path()).is_ok_and(|b| b == xmp_xml.as_bytes()));
            if !same && let Err(e) = write_new(&target_dir, &base_name, "xmp", xmp_xml) {
                warnings.push(format!("no se escribió el archivo XMP: {}", io_reason(&e)));
            }
        }
        if options.embed_jpeg_tags && matches!(outcome, Outcome::Copied(_)) {
            match embed_jpeg_copy(&resolved.target, &target_dir, xmp_xml, &md) {
                Ok(new_sha) => tagged_sha = new_sha,
                Err(e) => warnings.push(format!(
                    "no se incrustaron las etiquetas, la copia queda sin ellas: {e}"
                )),
            }
        }
    }

    Ok(Done {
        outcome,
        sha,
        tagged_sha,
        warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
        size: md.len(),
    })
}

/// Why an item was not exported: [`Outcome::Blocked`] when the guard refused a link.
fn failure(e: &io::Error) -> Outcome {
    let by_link = e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<GuardError>())
        .is_some_and(GuardError::is_link);
    if by_link {
        Outcome::Blocked(io_reason(e))
    } else {
        Outcome::Failed(io_reason(e))
    }
}

/// Copies the originals byte for byte into `dest` using [`Layout::Flat`], verifies every copy
/// against its SHA-256 and writes a manifest. Nothing is ever overwritten, moved or deleted.
pub fn export(items: &[ExportItem], dest: &DestPath) -> io::Result<ExportReport> {
    export_with(items, dest, Layout::Flat, &mut |_| Control::Continue)
}

/// Copies the originals byte for byte into `dest` using `layout`, verifies every copy against its
/// SHA-256, calls `progress` after each item and writes a manifest for all items processed.
pub fn export_with(
    items: &[ExportItem],
    dest: &DestPath,
    layout: Layout,
    progress: &mut dyn FnMut(&ExportProgress) -> Control,
) -> io::Result<ExportReport> {
    export_with_options(
        items,
        dest,
        ExportOptions {
            layout,
            write_xmp_sidecars: false,
            embed_jpeg_tags: false,
            operation: Operation::Copy,
        },
        progress,
    )
}

/// Copies the originals byte for byte into `dest` using `options`, verifies every copy against its
/// SHA-256, optionally writes XMP sidecars and embeds tags in JPEG copies, and writes a manifest.
pub fn export_with_options(
    items: &[ExportItem],
    dest: &DestPath,
    options: ExportOptions,
    progress: &mut dyn FnMut(&ExportProgress) -> Control,
) -> io::Result<ExportReport> {
    let options = ExportOptions {
        operation: Operation::Copy,
        ..options
    };
    run(items, dest, options, None, progress)
}

/// The loop and manifest that copying and moving share.
#[allow(clippy::disallowed_methods)] // creating the destination folder: `dest` is a DestPath
fn run(
    items: &[ExportItem],
    dest: &DestPath,
    options: ExportOptions,
    mover: Option<(&MoveContext, i64)>,
    progress: &mut dyn FnMut(&ExportProgress) -> Control,
) -> io::Result<ExportReport> {
    // Checked on both sides: the folder may have become a link since it was chosen.
    dest.verify().map_err(io::Error::other)?;
    fs::create_dir_all(dest.as_path())?;
    dest.verify().map_err(io::Error::other)?;
    let total = items.len();
    let mut outcomes = Vec::with_capacity(total);
    let mut all_warnings = Vec::new();
    let mut files: Vec<Value> = Vec::with_capacity(total);
    let has_tagged_hashes = options.embed_jpeg_tags;
    let mut csv = String::from(
        "source_path,dest_path,sha256,size,status,labels,belongs_to_work,belongs_to_chapter,belongs_to_page",
    );
    if has_tagged_hashes {
        csv.push_str(",sha256_after_tagging");
    }
    csv.push_str(",operacion\n");
    let (mut copied, mut present, mut failed, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let (mut moved, mut kept) = (0u64, 0u64);
    let mut stopped = false;

    let root_folders = root_folders(items);

    for (idx, item) in items.iter().enumerate() {
        let (outcome, dest_path, sha, tagged_sha, warning, size) =
            match copy_one(item, dest, options, &root_folders, mover) {
                Ok(Done {
                    outcome,
                    sha,
                    tagged_sha,
                    warning,
                    size,
                }) => {
                    let path = match &outcome {
                        Outcome::Copied(p)
                        | Outcome::AlreadyPresent(p)
                        | Outcome::Moved(p)
                        | Outcome::KeptOriginal { dest: p, .. } => Some(p.clone()),
                        Outcome::Blocked(_) | Outcome::Failed(_) => None,
                    };
                    (outcome, path, Some(sha), tagged_sha, warning, Some(size))
                }
                Err(e) => (failure(&e), None, None, None, None, None),
            };
        let status = match &outcome {
            Outcome::Copied(_) => {
                copied += 1;
                bytes += size.unwrap_or(0);
                "copied".to_string()
            }
            Outcome::AlreadyPresent(_) => {
                present += 1;
                "already_present".to_string()
            }
            Outcome::Moved(_) => {
                moved += 1;
                bytes += size.unwrap_or(0);
                "moved".to_string()
            }
            Outcome::KeptOriginal { reason, .. } => {
                kept += 1;
                format!("kept_original: {reason}")
            }
            Outcome::Blocked(msg) | Outcome::Failed(msg) => {
                failed += 1;
                format!("failed: {msg}")
            }
        };
        let applied: Vec<&Assignment> = item
            .labels
            .iter()
            .filter(|l| l.status != Status::Rejected)
            .collect();
        let source = item.source.to_string_lossy();
        let dest_text = dest_path.as_ref().map(|p| p.to_string_lossy().into_owned());

        let applied_work = item
            .belongs_to
            .as_ref()
            .filter(|w| w.status != Status::Rejected);
        let belongs_to_json = applied_work.map(|w| {
            json!({
                "work": w.work_name,
                "chapter": w.chapter,
                "page": w.page,
                "source": lower(w.source),
                "confidence": w.confidence,
                "status": lower(w.status),
            })
        });

        let mut file_obj = json!({
            "source_path": source,
            "dest_path": dest_text,
            "sha256": sha,
            "size": size,
            "status": status,
            "operacion": options.operation.name(),
            "labels": applied.iter().map(|l| json!({
                "kind": lower(l.kind),
                "name": l.name,
                "source": lower(l.source),
                "confidence": l.confidence,
                "status": lower(l.status),
            })).collect::<Vec<_>>(),
            "belongs_to": belongs_to_json,
        });
        if let Some(final_sha) = &tagged_sha {
            file_obj["sha256_original"] = json!(sha);
            file_obj["sha256_after_tagging"] = json!(final_sha);
        }
        if let Some(w) = warning {
            file_obj["warning"] = json!(w);
            all_warnings.push((item.source.clone(), w));
        }
        files.push(file_obj);

        let label_text = applied
            .iter()
            .map(|l| format!("{}:{}", lower(l.kind), l.name))
            .collect::<Vec<_>>()
            .join("; ");
        let (work_col, chapter_col, page_col) = match applied_work {
            Some(w) => (
                w.work_name.clone(),
                w.chapter.clone(),
                w.page.map(|p| p.to_string()).unwrap_or_default(),
            ),
            None => (String::new(), String::new(), String::new()),
        };
        let mut fields = vec![
            source.into_owned(),
            dest_text.unwrap_or_default(),
            sha.unwrap_or_default(),
            size.map(|s| s.to_string()).unwrap_or_default(),
            status,
            label_text,
            work_col,
            chapter_col,
            page_col,
        ];
        if has_tagged_hashes {
            fields.push(tagged_sha.unwrap_or_default());
        }
        fields.push(options.operation.name().to_string());
        csv.push_str(
            &fields
                .iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push('\n');
        outcomes.push((item.source.clone(), outcome));

        if progress(&ExportProgress {
            done: idx + 1,
            total,
        }) == Control::Stop
        {
            stopped = true;
            break;
        }
    }

    let manifest = json!({
        "summary": {
            "copied": copied,
            "already_present": present,
            "moved": moved,
            "kept_original": kept,
            "failed": failed,
            "bytes": bytes,
            "stopped": stopped,
        },
        "files": files,
    });
    let manifest_json = write_new(
        dest,
        "manifest",
        "json",
        &serde_json::to_string_pretty(&manifest)?,
    )?;
    let manifest_csv = write_new(dest, "manifest", "csv", &csv)?;
    Ok(ExportReport {
        outcomes,
        warnings: all_warnings,
        manifest_json,
        manifest_csv,
        stopped,
    })
}

/// What [`export`] would do, without writing anything.
pub fn preview(items: &[ExportItem], dest: &DestPath) -> io::Result<Preview> {
    preview_with(items, dest, Layout::Flat)
}

/// What [`export_with`] would do for `layout`, without writing anything.
pub fn preview_with(items: &[ExportItem], dest: &DestPath, layout: Layout) -> io::Result<Preview> {
    preview_with_options(
        items,
        dest,
        ExportOptions {
            layout,
            write_xmp_sidecars: false,
            embed_jpeg_tags: false,
            operation: Operation::Copy,
        },
    )
}

/// What [`export_with_options`] would do for `options`, without writing anything.
pub fn preview_with_options(
    items: &[ExportItem],
    dest: &DestPath,
    options: ExportOptions,
) -> io::Result<Preview> {
    let mut p = Preview {
        operation: options.operation,
        files: 0,
        bytes: 0,
        same_volume: 0,
        other_volume: 0,
        bytes_other_volume: 0,
        renamed: 0,
        already_present: 0,
        unreadable: 0,
        blocked: 0,
        free_bytes: available_space(dest.as_path()),
    };
    let mut reserved: HashMap<Option<String>, HashMap<String, String>> = HashMap::new();
    let root_folders = root_folders(items);
    #[cfg(windows)]
    let dest_volume = nearest_existing_dir(dest.as_path()).and_then(|dir| volume(&dir));
    #[cfg(not(windows))]
    let dest_volume = volume(dest.as_path());

    for item in items {
        let Ok((md, sha)) = read_source(&item.source) else {
            p.unreadable += 1;
            continue;
        };
        let Some(name) = item.source.file_name() else {
            p.unreadable += 1;
            continue;
        };
        let comps = match subfolder_components_for(item, &md, options.layout, &root_folders) {
            Ok(c) => c,
            Err(_) => {
                p.unreadable += 1;
                continue;
            }
        };
        let target_dir = match resolve_target_dir(dest, &comps) {
            Ok(d) => d,
            Err(e) => {
                p.count_refused(&e);
                continue;
            }
        };
        let folder_key = if comps.is_empty() {
            None
        } else {
            Some(
                comps
                    .iter()
                    .map(|c| c.to_string_lossy().to_lowercase())
                    .collect::<Vec<_>>()
                    .join("/"),
            )
        };
        let folder_reserved = reserved.entry(folder_key).or_default();
        let tagged = if options.embed_jpeg_tags {
            tagged_bytes_of(&item.source, &xmp_for(item))
        } else {
            None
        };
        let resolved = match resolve(&target_dir, name, &sha, folder_reserved, tagged.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                p.count_refused(&e);
                continue;
            }
        };
        if resolved.same {
            p.already_present += 1;
            continue;
        }
        let chosen = resolved.target.as_path().file_name().unwrap();
        p.files += 1;
        p.bytes += md.len();
        // Unknown counts as another disk: that is the case that needs free space.
        if volume(&item.source).is_some_and(|v| Some(v) == dest_volume) {
            p.same_volume += 1;
        } else {
            p.other_volume += 1;
            p.bytes_other_volume += md.len();
        }
        if chosen != name {
            p.renamed += 1;
        }
        folder_reserved.insert(key(chosen), sha);
    }
    Ok(p)
}

/// What tells one volume from another: the one mounted where `path` lies, which a folder with
/// another disk mounted on it does not share with its drive letter. `None` when Windows cannot say.
#[cfg(windows)]
fn volume(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut mount = vec![0u16; wide.len().max(261)];
    // SAFETY: `wide` is null-terminated UTF-16 and `mount` is as long as stated.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), mount.as_mut_ptr(), mount.len() as u32) } == 0 {
        return None;
    }
    // `\\?\Volume{GUID}\` is 49 characters long.
    let mut guid = [0u16; 64];
    // SAFETY: `mount` was null-terminated by the call above and `guid` is as long as stated.
    let named = unsafe {
        GetVolumeNameForVolumeMountPointW(mount.as_ptr(), guid.as_mut_ptr(), guid.len() as u32)
    } != 0;
    // Network shares have no such name: their mount point is what tells them apart.
    let name: &[u16] = if named { &guid } else { &mount };
    let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    Some(String::from_utf16_lossy(&name[..end]).to_lowercase())
}

#[cfg(not(windows))]
fn volume(_path: &Path) -> Option<String> {
    Some(String::new())
}

#[cfg(windows)]
fn nearest_existing_dir(path: &Path) -> Option<PathBuf> {
    let mut cur = path;
    loop {
        if fs::metadata(cur).is_ok_and(|md| md.is_dir()) {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Queries available bytes for the current user on the volume holding `path`'s nearest existing
/// ancestor directory. Uses `windows-sys` (`GetDiskFreeSpaceExW`), which is already in the
/// workspace dependency graph (no extra crates added).
#[cfg(windows)]
fn available_space(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let ancestor = nearest_existing_dir(path)?;
    let wide: Vec<u16> = ancestor
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free_to_caller: u64 = 0;
    // SAFETY: `wide` is null-terminated UTF-16 and `free_to_caller` is a valid out-pointer.
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free_to_caller)
}

#[cfg(not(windows))]
fn available_space(_path: &Path) -> Option<u64> {
    None
}

fn lower(value: impl std::fmt::Debug) -> String {
    format!("{value:?}").to_lowercase()
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Writes a new file next to the copies, as `base.ext`, `base (1).ext`, ... Never overwrites.
// shortcut: written in place, so a crash mid-write leaves a partial manifest; it is small and never overwrites.
#[allow(clippy::disallowed_methods)] // creates a file in the destination: `dest` is a DestPath
fn write_new(dest: &DestPath, base: &str, ext: &str, content: &str) -> io::Result<PathBuf> {
    for n in 0u32.. {
        let name = if n == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base} ({n}).{ext}")
        };
        let target = dest
            .join_name(OsStr::new(&name))
            .map_err(io::Error::other)?;
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target.as_path())
        {
            Ok(mut file) => {
                file.write_all(content.as_bytes())?;
                file.sync_all()?;
                return Ok(target.as_path().to_path_buf());
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    unreachable!("the counter never ends before a free name is found")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn relative_to_root_ignores_case_beyond_ascii() {
        assert_eq!(
            relative_to_root(
                Path::new(r"C:\Fotos\Ñandú\x.jpg"),
                Path::new(r"c:\FOTOS\ñANDÚ")
            ),
            Some(PathBuf::from("x.jpg"))
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_volume_is_the_same_however_the_path_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let plain = dunce::canonicalize(tmp.path()).unwrap();
        // With the `\\?\` prefix.
        let verbatim = fs::canonicalize(tmp.path()).unwrap();
        assert!(volume(&plain).is_some());
        assert_eq!(volume(&plain), volume(&verbatim));
        assert_eq!(volume(&plain), volume(&plain.join("no").join("existe")));
    }

    #[test]
    fn year_from_system_time_handles_modern_leap_pre_epoch_and_overflow() {
        assert_eq!(year_from_system_time(UNIX_EPOCH).as_deref(), Some("1970"));
        // 2024-02-29 12:00:00 UTC
        let leap = UNIX_EPOCH + Duration::from_secs(1_709_208_000);
        assert_eq!(year_from_system_time(leap).as_deref(), Some("2024"));
        // 1969-12-31 23:59:59.5 UTC
        let pre = UNIX_EPOCH - Duration::from_millis(500);
        assert_eq!(year_from_system_time(pre).as_deref(), Some("1969"));
        // Year > 9999 returns None so Layout::ByYear falls back to "Sin fecha".
        if let Some(far) = UNIX_EPOCH.checked_add(Duration::from_secs(300_000_000_000)) {
            assert_eq!(year_from_system_time(far), None);
        }
    }
}
