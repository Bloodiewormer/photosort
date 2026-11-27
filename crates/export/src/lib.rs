use catalog::{AssetWork, Assignment, LabelKind, Status};
use fsguard::DestPath;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_FOLDER_CHARS: usize = 80;

#[derive(Debug, Clone)]
pub struct ExportItem {
    pub source: PathBuf,
    /// What the catalog knows about the image; goes into the manifest.
    pub labels: Vec<Assignment>,
    /// Optional work ownership relation (`belongs_to`); goes into the manifest when not rejected.
    pub belongs_to: Option<AssetWork>,
}

/// How exported copies are arranged inside the destination folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    #[default]
    Flat,
    ByCategory,
    ByEntity,
    ByYear,
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
    Failed(String),
}

#[derive(Debug)]
pub struct ExportReport {
    pub outcomes: Vec<(PathBuf, Outcome)>,
    pub manifest_json: PathBuf,
    pub manifest_csv: PathBuf,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Files that would be copied.
    pub files: usize,
    pub bytes: u64,
    /// Of `files`, how many would get a numbered name because the name is taken by different bytes.
    pub renamed: usize,
    pub already_present: usize,
    /// Sources that could not be read.
    pub unreadable: usize,
    /// Available bytes on the destination volume, queried from its nearest existing ancestor.
    pub free_bytes: Option<u64>,
}

impl Preview {
    /// True when the bytes to copy fit in the destination volume's free space (or when unknown).
    pub fn fits(&self) -> bool {
        self.free_bytes.is_none_or(|free| self.bytes <= free)
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

fn subfolder_for(item: &ExportItem, md: &fs::Metadata, layout: Layout) -> Option<String> {
    match layout {
        Layout::Flat => None,
        Layout::ByCategory => Some(first_label_folder(
            &item.labels,
            LabelKind::Category,
            "Sin categoría",
        )),
        Layout::ByEntity => Some(first_label_folder(
            &item.labels,
            LabelKind::Entity,
            "Sin obra",
        )),
        Layout::ByYear => Some(
            md.modified()
                .ok()
                .and_then(year_from_system_time)
                .unwrap_or_else(|| "Sin fecha".to_string()),
        ),
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
) -> Result<Resolved, String> {
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
        let target = dest
            .join_name(&candidate)
            .ok_or_else(|| format!("{name:?} is not a plain file name"))?;
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
        if scanner::sha256_file(target.as_path()).is_ok_and(|h| h == sha) {
            return Ok(Resolved { target, same: true });
        }
    }
}

/// Metadata is read before the bytes so the original's access time is the one that gets restored.
fn read_source(path: &Path) -> Result<(fs::Metadata, String), String> {
    let md = fs::metadata(path).map_err(|e| e.to_string())?;
    if !md.is_file() {
        return Err("not a file".into());
    }
    let sha = scanner::sha256_file(path).map_err(|e| e.to_string())?;
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
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// Writes `part`, checks it against `sha`, gives it the original's dates and renames it to `target`.
/// Both paths are in the destination. A failed copy leaves nothing behind.
// shortcut: checking that `target` is free and renaming are two steps, so another program creating the same
// name in between could be overwritten; fine for one user on one machine, use a link-based rename if not.
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
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(part.as_path())?;
        created = true;
        io::copy(&mut src, &mut out)?;
        out.sync_all()?;
        if scanner::sha256_file(part.as_path())? != sha {
            return Err(io::Error::other(
                "the copy does not match the original (SHA-256)",
            ));
        }
        out.set_times(times_of(md)?)?;
        drop(out);
        if fs::symlink_metadata(target.as_path()).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "the name was taken while copying",
            ));
        }
        fs::rename(part.as_path(), target.as_path())
    })();
    if result.is_err() && created {
        let _ = fs::remove_file(part.as_path()); // our own temporary file, in the destination
    }
    result
}

struct Done {
    outcome: Outcome,
    sha: String,
    size: u64,
}

fn copy_one(item: &ExportItem, dest: &DestPath, layout: Layout) -> Result<Done, String> {
    let (md, sha) = read_source(&item.source)?;
    let name = item.source.file_name().ok_or("the path has no file name")?;
    let sub = subfolder_for(item, &md, layout);
    let target_dir = match &sub {
        Some(folder) => dest
            .join_name(OsStr::new(folder))
            .ok_or_else(|| format!("invalid subfolder name {folder:?}"))?,
        None => dest.clone(),
    };
    let resolved = resolve(&target_dir, name, &sha, &HashMap::new())?;
    let path = resolved.target.as_path().to_path_buf();
    let outcome = if resolved.same {
        Outcome::AlreadyPresent(path)
    } else {
        if sub.is_some() {
            ensure_subfolder(&target_dir).map_err(|e| e.to_string())?;
        }
        let mut part_name = resolved.target.as_path().file_name().unwrap().to_owned();
        part_name.push(".part");
        let part = target_dir
            .join_name(&part_name)
            .ok_or("the temporary name is not valid")?;
        write_verified(&item.source, &part, &resolved.target, &sha, &md)
            .map_err(|e| e.to_string())?;
        Outcome::Copied(path)
    };
    Ok(Done {
        outcome,
        sha,
        size: md.len(),
    })
}

/// Copies the originals byte for byte into `dest` using [`Layout::Flat`], verifies every copy
/// against its SHA-256 and writes a manifest. Nothing is ever overwritten, moved or deleted.
pub fn export(items: &[ExportItem], dest: &DestPath) -> io::Result<ExportReport> {
    export_with(items, dest, Layout::Flat, &mut |_| Control::Continue)
}

/// Copies the originals byte for byte into `dest` using `layout`, verifies every copy against its
/// SHA-256, calls `progress` after each item and writes a manifest for all items processed.
#[allow(clippy::disallowed_methods)] // creating the destination folder: `dest` is a DestPath
pub fn export_with(
    items: &[ExportItem],
    dest: &DestPath,
    layout: Layout,
    progress: &mut dyn FnMut(&ExportProgress) -> Control,
) -> io::Result<ExportReport> {
    fs::create_dir_all(dest.as_path())?;
    let total = items.len();
    let mut outcomes = Vec::with_capacity(total);
    let mut files: Vec<Value> = Vec::with_capacity(total);
    let mut csv = String::from(
        "source_path,dest_path,sha256,size,status,labels,belongs_to_work,belongs_to_chapter,belongs_to_page\n",
    );
    let (mut copied, mut present, mut failed, mut bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut stopped = false;

    for (idx, item) in items.iter().enumerate() {
        let (outcome, dest_path, sha, size) = match copy_one(item, dest, layout) {
            Ok(Done { outcome, sha, size }) => {
                let path = match &outcome {
                    Outcome::Copied(p) | Outcome::AlreadyPresent(p) => Some(p.clone()),
                    Outcome::Failed(_) => None,
                };
                (outcome, path, Some(sha), Some(size))
            }
            Err(msg) => (Outcome::Failed(msg), None, None, None),
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
            Outcome::Failed(msg) => {
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

        files.push(json!({
            "source_path": source,
            "dest_path": dest_text,
            "sha256": sha,
            "size": size,
            "status": status,
            "labels": applied.iter().map(|l| json!({
                "kind": lower(l.kind),
                "name": l.name,
                "source": lower(l.source),
                "confidence": l.confidence,
                "status": lower(l.status),
            })).collect::<Vec<_>>(),
            "belongs_to": belongs_to_json,
        }));
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
        let fields = [
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
        csv.push_str(&fields.map(|f| csv_field(&f)).join(","));
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
    let mut p = Preview {
        files: 0,
        bytes: 0,
        renamed: 0,
        already_present: 0,
        unreadable: 0,
        free_bytes: available_space(dest.as_path()),
    };
    let mut reserved: HashMap<Option<String>, HashMap<String, String>> = HashMap::new();
    for item in items {
        let Ok((md, sha)) = read_source(&item.source) else {
            p.unreadable += 1;
            continue;
        };
        let Some(name) = item.source.file_name() else {
            p.unreadable += 1;
            continue;
        };
        let sub = subfolder_for(item, &md, layout);
        let target_dir = match &sub {
            Some(folder) => match dest.join_name(OsStr::new(folder)) {
                Some(d) => d,
                None => {
                    p.unreadable += 1;
                    continue;
                }
            },
            None => dest.clone(),
        };
        let folder_key = sub.as_deref().map(|s| s.to_lowercase());
        let folder_reserved = reserved.entry(folder_key).or_default();
        let Ok(resolved) = resolve(&target_dir, name, &sha, folder_reserved) else {
            p.unreadable += 1;
            continue;
        };
        if resolved.same {
            p.already_present += 1;
            continue;
        }
        let chosen = resolved.target.as_path().file_name().unwrap();
        p.files += 1;
        p.bytes += md.len();
        if chosen != name {
            p.renamed += 1;
        }
        folder_reserved.insert(key(chosen), sha);
    }
    Ok(p)
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
            .ok_or_else(|| io::Error::other("not a plain file name"))?;
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
