use catalog::{Assignment, Status};
use fsguard::DestPath;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct ExportItem {
    pub source: PathBuf,
    /// What the catalog knows about the image; goes into the manifest.
    pub labels: Vec<Assignment>,
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
}

#[derive(Debug, PartialEq, Eq)]
pub struct Preview {
    /// Files that would be copied.
    pub files: usize,
    pub bytes: u64,
    /// Of `files`, how many would get a numbered name because the name is taken by different bytes.
    pub renamed: usize,
    pub already_present: usize,
    /// Sources that could not be read.
    pub unreadable: usize,
}

/// Where a source would go: `same` means the destination already holds identical bytes.
struct Resolved {
    target: DestPath,
    same: bool,
}

fn key(name: &OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}

/// First free name (`a.jpg`, `a (1).jpg`, ...) or the identical file already there. `reserved` holds names
/// that an earlier item of the same preview has claimed but not written.
fn resolve(
    dest: &DestPath,
    name: &OsStr,
    sha: &str,
    reserved: &HashSet<String>,
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
        if reserved.contains(&key(&candidate)) {
            continue;
        }
        let target = dest
            .join_name(&candidate)
            .ok_or_else(|| format!("{name:?} is not a plain file name"))?;
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

fn copy_one(source: &Path, dest: &DestPath) -> Result<Done, String> {
    let (md, sha) = read_source(source)?;
    let name = source.file_name().ok_or("the path has no file name")?;
    let resolved = resolve(dest, name, &sha, &HashSet::new())?;
    let path = resolved.target.as_path().to_path_buf();
    let outcome = if resolved.same {
        Outcome::AlreadyPresent(path)
    } else {
        let mut part_name = resolved.target.as_path().file_name().unwrap().to_owned();
        part_name.push(".part");
        let part = dest
            .join_name(&part_name)
            .ok_or("the temporary name is not valid")?;
        write_verified(source, &part, &resolved.target, &sha, &md).map_err(|e| e.to_string())?;
        Outcome::Copied(path)
    };
    Ok(Done {
        outcome,
        sha,
        size: md.len(),
    })
}

/// Copies the originals byte for byte into `dest`, verifies every copy against its SHA-256 and writes
/// a manifest. Nothing is ever overwritten, moved or deleted.
#[allow(clippy::disallowed_methods)] // creating the destination folder: `dest` is a DestPath
pub fn export(items: &[ExportItem], dest: &DestPath) -> io::Result<ExportReport> {
    fs::create_dir_all(dest.as_path())?;
    let mut outcomes = Vec::with_capacity(items.len());
    let mut files: Vec<Value> = Vec::with_capacity(items.len());
    let mut csv = String::from("source_path,dest_path,sha256,size,status,labels\n");
    let (mut copied, mut present, mut failed, mut bytes) = (0u64, 0u64, 0u64, 0u64);

    for item in items {
        let (outcome, dest_path, sha, size) = match copy_one(&item.source, dest) {
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
        }));
        let label_text = applied
            .iter()
            .map(|l| format!("{}:{}", lower(l.kind), l.name))
            .collect::<Vec<_>>()
            .join("; ");
        let fields = [
            source.into_owned(),
            dest_text.unwrap_or_default(),
            sha.unwrap_or_default(),
            size.map(|s| s.to_string()).unwrap_or_default(),
            status,
            label_text,
        ];
        csv.push_str(&fields.map(|f| csv_field(&f)).join(","));
        csv.push('\n');
        outcomes.push((item.source.clone(), outcome));
    }

    let manifest = json!({
        "summary": {"copied": copied, "already_present": present, "failed": failed, "bytes": bytes},
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
    })
}

/// What [`export`] would do, without writing anything.
pub fn preview(items: &[ExportItem], dest: &DestPath) -> io::Result<Preview> {
    let mut p = Preview {
        files: 0,
        bytes: 0,
        renamed: 0,
        already_present: 0,
        unreadable: 0,
    };
    let mut reserved = HashSet::new();
    for item in items {
        let resolved = read_source(&item.source).ok().and_then(|(md, sha)| {
            let name = item.source.file_name()?;
            Some((md, name, resolve(dest, name, &sha, &reserved).ok()?))
        });
        let Some((md, name, resolved)) = resolved else {
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
        reserved.insert(key(chosen));
    }
    Ok(p)
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
