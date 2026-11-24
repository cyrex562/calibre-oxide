//! Copy a library to another folder, and keep that copy up to date: the engine
//! behind "Export a copy..." (a travel copy) and the backup mirror.
//!
//! One planner and one executor serve two modes.
//!
//! * [`Mode::Travel`] -- a library you take with you (a laptop, a USB disk),
//!   optionally only some books. It opens as a library of its own, so it gets
//!   its **own identity**: a fresh library uuid in `metadata.db`, a fresh
//!   `install-id` and no `library.id`, and the change log is rebuilt as a single
//!   snapshot under that new origin (see "Change log" below). Edits made on the
//!   copy are protected: a refresh refuses to overwrite a file that no longer
//!   matches what the last refresh wrote, and says so (a *conflict*).
//! * [`Mode::Mirror`] -- a one-way, incremental, whole-library backup, for
//!   restoring. It keeps the library's identity: same uuid, same `install-id`,
//!   the change log copied as it is.
//!
//! # What is copied
//!
//! Every format file (`books.path` + `data.name` + format), each book folder's
//! `cover.jpg`, `metadata.opf` and `data/` extras, the covers under
//! `.lexmechanic/covers/`, the change log (`changes/`, `snapshots/`),
//! the other durable state under `.lexmechanic/`, `.calnotes/` and the
//! databases. Files that are not part of the library (anything else in the root
//! folder) are not touched. What is **never** copied is one list, [`is_excluded`],
//! shared with the zip export: the trash, the writer lock and journal, job
//! records, import staging, run records, SQLite sidecars, `*.tmp-*` and
//! absorbed-library markers.
//!
//! # Databases
//!
//! `metadata.db`, `full-text-search.db`, `.calnotes/notes.db` and
//! `.lexmechanic/checksums.db` are never copied byte for byte (a running library
//! has a WAL). Each is snapshotted with `VACUUM INTO` into a local scratch folder,
//! changed there if the mode needs it, hashed, and *that* is what is compared and
//! written. The target is never opened as a database, on any tier: a database is
//! one more file written by streaming it, which is the only safe way over SMB.
//!
//! # Subset (travel only)
//!
//! A scope of book ids or a folder prefix prunes the scratch `metadata.db`: the
//! other books go (the schema's own triggers remove their links), then the rows
//! nothing refers to any more -- authors, tags, series, publishers, languages and
//! custom-column values, as calibre's cleanup would. Checksums, notes and covers
//! are filtered to the same books. The **full-text index is left out** of a subset
//! (it is derived, holds every book's text, and is rebuilt on the copy by the
//! indexing job); a whole-library travel copy and a mirror include it.
//!
//! # Change log
//!
//! The log is the library's authority (`metadata.db` can be rebuilt from it), and
//! it is per origin. A travel copy cannot keep the source's origin (its edits
//! would pass for the source's), so the copy does not receive `changes/` and
//! `snapshots/`. It gets one **rebased snapshot**: the source's effective history,
//! filtered to the chosen books, re-chained under the copy's own origin (same
//! stamps, same operations, sequence `1..n`). Opening the copy continues that
//! chain, and a rebuild of the copy from its log yields its books (and only
//! them). The snapshot is deterministic, so an unchanged source gives an unchanged
//! snapshot and a refresh leaves it alone.
//!
//! # Incremental refresh
//!
//! `<target>/.lexmechanic-mirror.json` records, per file, the size, the source
//! mtime (which is also set on the copy) and the BLAKE3 of what was written. A
//! file is *unchanged* -- with no read of it -- when the source's size and mtime
//! equal the manifest's. Only when the mtime moved is the source hashed. The copy
//! is judged "untouched" by its size and mtime against the manifest (2 seconds of
//! tolerance for SMB and FAT); only a mismatch there is hashed.
//!
//! # Safety
//!
//! Nothing is hard-deleted from a copy: a file the library no longer has moves to
//! `.mirror-trash/<run-id>/`, and only if it still matches the manifest. Writes go to
//! `<name>.tmp-<id>`, are fsynced, verified by BLAKE3, stamped with the source
//! mtime and renamed. The databases follow the book files and the manifest is
//! written last (and every so often before that), so a run that is interrupted
//! resumes by planning again: what was finished is already in the manifest.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::cache::Cache;
use crate::change_log::{Change, ChangeOp, Hlc, SnapshotHeader, Watermark};
use crate::constants::{CHECKSUMS_DB_NAME, FTS_DB_NAME, LIBRARY_HANDLE_DIR_NAME, NOTES_DB_NAME, NOTES_DIR_NAME, TRASH_DIR_NAME};
use crate::library_handle::StorageTier;
use crate::scan_pipeline::JobControl;

/// The manifest, in the target's root.
pub const MANIFEST_NAME: &str = ".lexmechanic-mirror.json";
/// A copy made before the rename carries its manifest under this name. It is
/// still read (so that copy is refreshed, not mistaken for a stranger's
/// folder) and is removed once the new manifest is written.
const LEGACY_MANIFEST_NAME: &str = ".calibre-oxide-mirror.json";
/// Where a refresh puts what it removed from a copy.
pub const MIRROR_TRASH_DIR: &str = ".mirror-trash";
/// The library preference remembering where copies go.
pub const TARGETS_PREF: &str = "lexmechanic.mirror_targets";
/// The name this preference had before the rename; still read, never written.
pub const LEGACY_TARGETS_PREF: &str = "oxide.mirror_targets";
/// Two mtimes this close are the same (SMB shares and FAT round to 2 s).
pub const MTIME_TOLERANCE_MS: i64 = 2000;

// ---------------------------------------------------------------------
// What is never copied
// ---------------------------------------------------------------------

/// Names directly under `.lexmechanic/` that are per-process, per-run or
/// transient: never part of a copy and never in the zip export.
const EXCLUDED_STATE: &[&str] = &["writer.lock", "writer.owner", "journal", "jobs.json", "import-staging", "prepare-stage.json", "organize-runs", "retitle-runs", "absorbed-libraries.json"];

/// Whether a library-relative path (any separator) is left out of every copy
/// and archive of a library:
///
/// * the trash (`.caltrash`) and a copy's own `.mirror-trash`, and the manifest;
/// * under `.lexmechanic/`: `writer.lock`, `writer.owner`, `journal/`,
///   `jobs.json`, `import-staging/`, `prepare-stage.json`, `organize-runs/`,
///   `retitle-runs/`, `absorbed-libraries.json`, and anything half written
///   (`*.partial`);
/// * SQLite sidecars (`*.db-wal`, `*.db-shm`, `*.db-journal`);
/// * `*.tmp-*` (an interrupted write) and `*.absorbed-by-lexmechanic`.
///
/// A denylist on purpose: a new durable file is carried by default, rather than
/// silently missing from every backup (which is how the change log once was).
pub fn is_excluded(rel: &str) -> bool {
    let lower = rel.replace('\\', "/").to_ascii_lowercase();
    let parts: Vec<&str> = lower.split('/').filter(|p| !p.is_empty()).collect();
    let Some(name) = parts.last() else { return false };
    if parts.iter().any(|p| *p == TRASH_DIR_NAME || *p == MIRROR_TRASH_DIR) {
        return true;
    }
    if parts.len() == 1 && (*name == MANIFEST_NAME || *name == LEGACY_MANIFEST_NAME) {
        return true;
    }
    // The pre-rename state folder is migrated into `.lexmechanic` on open; a
    // leftover one is never copied.
    if parts[0] == crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME {
        return true;
    }
    if parts[0] == LIBRARY_HANDLE_DIR_NAME {
        if let Some(inside) = parts.get(1) {
            if EXCLUDED_STATE.contains(inside) || (parts.len() == 2 && inside.ends_with(".partial")) {
                return true;
            }
        }
    }
    if name.contains(".tmp-") || name.ends_with(".absorbed-by-lexmechanic") || name.ends_with(".absorbed-by-calibre-oxide") {
        return true;
    }
    ["-wal", "-shm", "-journal"].iter().any(|suffix| name.strip_suffix(suffix).is_some_and(|base| base.ends_with(".db")))
}

/// A change-log history snapshot (`.lexmechanic/snapshots/*`). Its name changes
/// whenever the library's history does, so a refresh writes the new one and drops
/// the old one; that is internal bookkeeping the user should never see or confirm.
fn is_history_snapshot(rel: &str) -> bool {
    rel.replace('\\', "/").to_ascii_lowercase().starts_with(&format!("{LIBRARY_HANDLE_DIR_NAME}/snapshots/"))
}

/// A consistent copy of a SQLite database at `dest` (`VACUUM INTO`: one read
/// snapshot, including what is still in the `-wal`, needing no companion files
/// to open). Shared by the zip export and the mirror.
pub fn snapshot_database(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let conn = rusqlite::Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE).with_context(|| format!("opening {}", src.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))?;
    if dest.exists() {
        fs::remove_file(dest)?;
    }
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy().as_ref()]).with_context(|| format!("snapshotting {}", src.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Travel,
    Mirror,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Travel => "travel",
            Mode::Mirror => "mirror",
        }
    }
}

/// Which books a travel copy holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MirrorScope {
    #[default]
    All,
    BookIds(Vec<i32>),
    /// Books whose folder is this one or inside it (relative to the library).
    FolderPrefix(String),
}

impl MirrorScope {
    pub fn label(&self) -> String {
        match self {
            MirrorScope::All => "the whole library".into(),
            MirrorScope::BookIds(ids) => format!("{} selected books", ids.len()),
            MirrorScope::FolderPrefix(p) => format!("the folder {p}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MirrorOptions {
    pub mode: Mode,
    pub scope: MirrorScope,
    /// Files the library no longer has (or that left the scope) move to the
    /// copy's `.mirror-trash`.
    pub remove_missing: bool,
    /// Overwrite (or remove) files that were changed in the copy since the last
    /// run, instead of reporting a conflict.
    pub overwrite_changed: bool,
}

impl MirrorOptions {
    pub fn new(mode: Mode) -> Self {
        MirrorOptions { mode, scope: MirrorScope::All, remove_missing: true, overwrite_changed: false }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    /// The request cannot be honoured; the message says why, for the user.
    #[error("{0}")]
    Refused(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, MirrorError>;

fn refuse<T>(message: impl Into<String>) -> Result<T> {
    Err(MirrorError::Refused(message.into()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Format,
    Extra,
    Cover,
    Notes,
    /// `changes/` and `snapshots/`.
    Log,
    State,
    Database,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Format => "format",
            Kind::Extra => "extra",
            Kind::Cover => "cover",
            Kind::Notes => "notes",
            Kind::Log => "log",
            Kind::State => "state",
            Kind::Database => "database",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    CopyNew,
    Update,
    Unchanged,
    Remove,
    Conflict,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::CopyNew => "copy_new",
            Action::Update => "update",
            Action::Unchanged => "unchanged",
            Action::Remove => "remove",
            Action::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    /// The source file's mtime, which is also set on the copy.
    pub mtime_ms: i64,
    pub blake3: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    /// The source library's uuid.
    pub source_id: String,
    pub mode: Mode,
    pub run_id: String,
    pub written_at_ms: i64,
    pub scope: MirrorScope,
    /// False while a run is in progress or was interrupted.
    pub complete: bool,
    /// Travel: the copy's own origin (`install-id`) and library uuid, kept
    /// across refreshes so they stay stable.
    #[serde(default)]
    pub travel_origin: Option<String>,
    #[serde(default)]
    pub travel_library_id: Option<String>,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone)]
pub struct PlanItem {
    pub rel: String,
    pub kind: Kind,
    pub action: Action,
    /// Bytes to write (copy/update), or on the copy (remove).
    pub size: u64,
    pub mtime_ms: i64,
    /// What the source file is, where known without reading it.
    pub blake3: Option<String>,
    /// Where to read it: the source file, or the scratch snapshot.
    pub source: Option<PathBuf>,
    pub generated: bool,
    pub reason: Option<String>,
    /// What the manifest should say about this file if nothing is written for
    /// it (see [`execute`]).
    pub entry: Option<ManifestFile>,
    /// What the source's library recorded as this file's hash, used to verify
    /// the copy.
    pub recorded: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Count {
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct KindTotals {
    pub copy_new: Count,
    pub update: Count,
    pub unchanged: Count,
    pub remove: Count,
    pub conflict: Count,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Totals {
    pub copy_new: Count,
    pub update: Count,
    pub unchanged: Count,
    pub remove: Count,
    pub conflict: Count,
    /// What a run would write: new plus updated.
    pub bytes_to_copy: u64,
    pub by_kind: BTreeMap<String, KindTotals>,
}

/// What a run would do. Holds the scratch folder with the database snapshots
/// the plan was made from, so [`execute`] writes exactly those.
pub struct MirrorPlan {
    pub mode: Mode,
    pub target: PathBuf,
    pub source_id: String,
    pub options: MirrorOptions,
    pub tier: StorageTier,
    pub new_target: bool,
    pub books: usize,
    pub items: Vec<PlanItem>,
    pub totals: Totals,
    pub fingerprint: String,
    /// Library files the database lists that are not on disk.
    pub missing_sources: Vec<String>,
    /// Travel: change-log entries made on the copy itself (not from a refresh).
    pub copy_edits: Vec<String>,
    pub travel_origin: Option<String>,
    pub travel_library_id: Option<String>,
    _scratch: Arc<tempfile::TempDir>,
    /// Scratch folders of earlier plans whose database snapshot this plan reuses
    /// (see [`checksums_snapshot`]); kept alive as long as the plan is.
    _reused: Vec<Arc<tempfile::TempDir>>,
}

impl MirrorPlan {
    pub fn items_with(&self, action: Action) -> impl Iterator<Item = &PlanItem> {
        self.items.iter().filter(move |i| i.action == action)
    }
}

// ---------------------------------------------------------------------
// Hashing and small helpers
// ---------------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn mtime_ms(meta: &fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn same_mtime(a: i64, b: i64) -> bool {
    (a - b).abs() <= MTIME_TOLERANCE_MS
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// A library-relative path with forward slashes and no `.`/`..`/empty parts;
/// `None` for anything that could leave the library.
fn clean_rel(path: &str) -> Option<String> {
    let norm = path.replace('\\', "/");
    let mut parts = Vec::new();
    for part in norm.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    Some(parts.join("/"))
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// A `/`-separated relative path of plain names: no empty, `.` or `..` part,
/// no drive, root or backslash.
fn is_plain_rel(rel: &str) -> bool {
    !rel.is_empty() && rel.split('/').all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains(['\\', ':']))
}

fn abs(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn rel_of(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(rel.components().filter_map(|c| if let Component::Normal(s) = c { s.to_str().map(str::to_string) } else { None }).collect::<Vec<_>>().join("/"))
}

/// `resources/<2>/<alg>-<digest>` under `.calnotes/` for a notes resource hash
/// (the same mapping as `NotesConnection::path_for_resource`).
fn notes_resource_rel(resource_hash: &str) -> String {
    use lexmechanic_utils::filenames::sanitize_file_name;
    let (alg, digest) = resource_hash.split_once(':').unwrap_or(("raw", resource_hash));
    let (alg, digest) = (sanitize_file_name(alg), sanitize_file_name(digest));
    let prefix: String = digest.chars().take(2).collect();
    format!("{NOTES_DIR_NAME}/resources/{prefix}/{alg}-{digest}")
}

// ---------------------------------------------------------------------
// Target guards
// ---------------------------------------------------------------------

fn lowered(p: &Path) -> PathBuf {
    if cfg!(windows) {
        PathBuf::from(p.to_string_lossy().to_lowercase())
    } else {
        p.to_path_buf()
    }
}

/// The path with its existing part canonicalized (links and junctions
/// resolved) and the not-yet-existing tail appended.
fn resolve_through_links(path: &Path) -> io::Result<PathBuf> {
    let mut tail = Vec::new();
    let mut probe = path.to_path_buf();
    loop {
        match probe.canonicalize() {
            Ok(canon) => {
                return Ok(tail.iter().rev().fold(canon, |p, part: &std::ffi::OsString| p.join(part)));
            }
            Err(_) => {
                let Some(name) = probe.file_name().map(|n| n.to_os_string()) else { return Err(io::Error::new(io::ErrorKind::NotFound, "no part of the path exists")) };
                tail.push(name);
                if !probe.pop() {
                    return Err(io::Error::new(io::ErrorKind::NotFound, "no part of the path exists"));
                }
            }
        }
    }
}

/// Checks `raw` as a place to put a copy of the library at `source`: absolute,
/// no `..`, not the library or inside it or containing it -- including through
/// a symlink or junction.
pub fn validate_target(source: &Path, raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return refuse("Choose the folder to put the copy in.");
    }
    let path = PathBuf::from(raw);
    if !path.is_absolute() {
        return refuse(format!("{raw} is not a full path. Give the whole path, starting from a drive or a share."));
    }
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return refuse(format!("{raw} contains \"..\". Give the folder's real path."));
    }
    let resolved = resolve_through_links(&path).map_err(|e| MirrorError::Refused(format!("{raw} cannot be reached: {e}")))?;
    let src = source.canonicalize().map_err(|e| MirrorError::Other(anyhow::anyhow!("resolving the library folder: {e}")))?;
    let (resolved_l, src_l) = (lowered(&resolved), lowered(&src));
    let through_link = !lowered(&path).starts_with(lowered(source)) && !lowered(source).starts_with(lowered(&path));
    if resolved_l == src_l {
        return refuse("That folder is the library itself. Choose a different folder for the copy.");
    }
    if resolved_l.starts_with(&src_l) {
        let how = if through_link { " (it reaches the library through a link)" } else { "" };
        return refuse(format!("That folder is inside the library{how}. A copy inside the library would be copied into itself."));
    }
    if src_l.starts_with(&resolved_l) {
        return refuse("That folder contains the library. Choose a folder that is not above it.");
    }
    if path.is_file() {
        return refuse(format!("{raw} is a file, not a folder."));
    }
    Ok(path)
}

pub fn read_manifest(target: &Path) -> anyhow::Result<Option<Manifest>> {
    let mut path = target.join(MANIFEST_NAME);
    if !path.exists() && target.join(LEGACY_MANIFEST_NAME).exists() {
        path = target.join(LEGACY_MANIFEST_NAME);
    }
    match fs::read_to_string(&path) {
        Ok(text) => {
            let manifest: Manifest = serde_json::from_str(&text).with_context(|| format!("{} is not a readable manifest", path.display()))?;
            // The manifest lives in the copy, which may be on a USB disk or a
            // share someone else can write: every path in it must stay inside
            // the copy, or a refresh could set aside files elsewhere.
            if let Some(bad) = manifest.files.iter().find(|f| !is_plain_rel(&f.path)) {
                anyhow::bail!("{} lists a path outside the copy ({:?}); it was not written by Lexmechanic", path.display(), bad.path);
            }
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Written to a temp name, fsynced and renamed: the manifest is whole or absent.
fn write_manifest(target: &Path, manifest: &Manifest) -> io::Result<()> {
    let path = target.join(MANIFEST_NAME);
    let tmp = target.join(format!("{MANIFEST_NAME}.tmp-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(manifest).map_err(io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, &path)?;
    let _ = fs::remove_file(target.join(LEGACY_MANIFEST_NAME));
    Ok(())
}

pub fn classify_target(target: &Path) -> StorageTier {
    // The nearest existing ancestor decides, as the folder may not exist yet.
    let mut probe = target.to_path_buf();
    while !probe.exists() && probe.pop() {}
    crate::library_handle::classify_storage_tier(&probe)
}

// ---------------------------------------------------------------------
// Source inventory
// ---------------------------------------------------------------------

struct SourceFile {
    rel: String,
    kind: Kind,
    size: u64,
    mtime_ms: i64,
    path: PathBuf,
    generated: bool,
    /// Known hash: generated files (always), formats (when the library recorded one).
    hash: Option<String>,
    recorded: Option<String>,
}

struct BookRow {
    id: i32,
    uuid: String,
    path: String,
}

/// The books in scope: ids and uuids, in order.
struct Chosen {
    ids: HashSet<i32>,
    uuids: HashSet<String>,
}

fn load_books(cache: &Cache) -> anyhow::Result<(Vec<BookRow>, HashMap<i32, Vec<(String, String)>>)> {
    let conn = cache.backend.conn.lock().unwrap();
    let mut books = Vec::new();
    {
        let mut stmt = conn.prepare("SELECT id, uuid, path FROM books ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok(BookRow { id: r.get(0)?, uuid: r.get::<_, Option<String>>(1)?.unwrap_or_default(), path: r.get::<_, Option<String>>(2)?.unwrap_or_default() }))?;
        for row in rows {
            books.push(row?);
        }
    }
    let mut formats: HashMap<i32, Vec<(String, String)>> = HashMap::new();
    let mut stmt = conn.prepare("SELECT book, name, format FROM data ORDER BY book, format")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
    for row in rows {
        let (book, name, format) = row?;
        formats.entry(book).or_default().push((name, format));
    }
    Ok((books, formats))
}

fn choose(books: &[BookRow], scope: &MirrorScope) -> Chosen {
    let mut chosen = Chosen { ids: HashSet::new(), uuids: HashSet::new() };
    let wanted: Option<HashSet<i32>> = if let MirrorScope::BookIds(ids) = scope { Some(ids.iter().copied().collect()) } else { None };
    let prefix = if let MirrorScope::FolderPrefix(p) = scope { Some(p.trim_matches(['/', '\\']).replace('\\', "/").to_lowercase()) } else { None };
    for book in books {
        let take = match (&wanted, &prefix) {
            (Some(ids), _) => ids.contains(&book.id),
            (_, Some(prefix)) => {
                let path = book.path.replace('\\', "/").trim_matches('/').to_lowercase();
                prefix.is_empty() || path == *prefix || path.starts_with(&format!("{prefix}/"))
            }
            _ => true,
        };
        if take {
            chosen.ids.insert(book.id);
            chosen.uuids.insert(book.uuid.clone());
        }
    }
    chosen
}

fn stat_file(path: &Path) -> Option<(u64, i64)> {
    let meta = fs::metadata(path).ok()?;
    meta.is_file().then(|| (meta.len(), mtime_ms(&meta)))
}

struct Scratch {
    dir: Arc<tempfile::TempDir>,
    counter: usize,
}

impl Scratch {
    fn next(&mut self, name: &str) -> PathBuf {
        self.counter += 1;
        self.dir.path().join(format!("{}-{name}", self.counter))
    }
}

fn generated_file(rel: &str, kind: Kind, path: PathBuf) -> anyhow::Result<SourceFile> {
    let meta = fs::metadata(&path)?;
    Ok(SourceFile { rel: rel.to_string(), kind, size: meta.len(), mtime_ms: mtime_ms(&meta), hash: Some(hash_file(&path)?), path, generated: true, recorded: None })
}

// ---------------------------------------------------------------------
// Travel: identity, pruning, the rebased change log
// ---------------------------------------------------------------------

/// Opens a scratch copy of a library database with the functions and
/// tokenizers its triggers and indexes need.
fn open_scratch_db(path: &Path) -> anyhow::Result<rusqlite::Connection> {
    let conn = rusqlite::Connection::open(path)?;
    crate::backend::register_functions(&conn)?;
    Ok(conn)
}

fn table_exists(conn: &rusqlite::Connection, schema: &str, name: &str) -> bool {
    conn.query_row(&format!("SELECT 1 FROM {schema}.sqlite_master WHERE type='table' AND name=?1"), [name], |_| Ok(())).is_ok()
}

fn tables_with_column(conn: &rusqlite::Connection, column: &str) -> anyhow::Result<Vec<String>> {
    let names: Vec<String> = {
        let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut out = Vec::new();
    for name in names {
        let has: bool = conn.query_row("SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2", [&name, column], |r| r.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(false);
        if has {
            out.push(name);
        }
    }
    Ok(out)
}

/// Prepares the scratch `metadata.db` for writing: without the list of copy
/// targets (machine-local, and rewritten by every run -- left in, it would make
/// the database "changed" after each one); with a new library uuid when
/// `library_uuid` is given (a travel copy's own identity); and, for a subset,
/// with only `keep`'s books and what they refer to.
fn rewrite_metadata(path: &Path, keep: Option<&HashSet<i32>>, library_uuid: Option<&str>) -> anyhow::Result<()> {
    let conn = open_scratch_db(path)?;
    // Written then deleted, so the database goes through the same writes whether
    // or not the row was there: SQLite's header counts them, and two snapshots of
    // the same content must be the same bytes for an unchanged library to be
    // seen as unchanged.
    for key in [TARGETS_PREF, LEGACY_TARGETS_PREF] {
        conn.execute("INSERT OR REPLACE INTO preferences (key, val) VALUES (?1, 'null')", [key])?;
        conn.execute("DELETE FROM preferences WHERE key = ?1", [key])?;
    }
    if let Some(uuid) = library_uuid {
        conn.execute("DELETE FROM library_id", [])?;
        conn.execute("INSERT INTO library_id (uuid) VALUES (?1)", [uuid])?;
    }
    if let Some(keep) = keep {
        conn.execute_batch("CREATE TEMP TABLE keep_books (id INTEGER PRIMARY KEY)")?;
        {
            let mut ins = conn.prepare("INSERT OR IGNORE INTO keep_books (id) VALUES (?1)")?;
            for id in keep {
                ins.execute([id])?;
            }
        }
        // The schema's `books_delete_trg` removes each book's links, formats,
        // comments, identifiers and so on.
        conn.execute("DELETE FROM books WHERE id NOT IN (SELECT id FROM keep_books)", [])?;
        // Whatever else carries a `book` column (custom-column values and
        // links, dirtied marks, pages) and was not cleaned by the trigger.
        for table in tables_with_column(&conn, "book")? {
            if table == "books" || table == "keep_books" {
                continue;
            }
            conn.execute(&format!("DELETE FROM \"{table}\" WHERE book NOT IN (SELECT id FROM books)"), [])?;
        }
        // Rows nothing refers to any more: what calibre's own cleanup removes.
        for (table, link, column) in [("authors", "books_authors_link", "author"), ("tags", "books_tags_link", "tag"), ("series", "books_series_link", "series"), ("publishers", "books_publishers_link", "publisher"), ("languages", "books_languages_link", "lang_code")] {
            if table_exists(&conn, "main", table) && table_exists(&conn, "main", link) {
                conn.execute(&format!("DELETE FROM \"{table}\" WHERE id NOT IN (SELECT {column} FROM \"{link}\")"), [])?;
            }
        }
        let links: Vec<String> = {
            let mut stmt = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'books_custom_column_%_link'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for link in links {
            let n = link.trim_start_matches("books_custom_column_").trim_end_matches("_link");
            let values = format!("custom_column_{n}");
            if table_exists(&conn, "main", &values) {
                conn.execute(&format!("DELETE FROM \"{values}\" WHERE id NOT IN (SELECT value FROM \"{link}\")"), [])?;
            }
        }
        conn.execute_batch("DROP TABLE temp.keep_books")?;
    }
    // One self-contained file: no WAL beside it, and no dead pages after a prune.
    let _: String = conn.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    conn.execute_batch("VACUUM")?;
    Ok(())
}

/// Filters the scratch `checksums.db` to the chosen books.
fn prune_checksums(path: &Path, keep: &HashSet<i32>, keep_uuids: &HashSet<String>) -> anyhow::Result<()> {
    let conn = open_scratch_db(path)?;
    conn.execute_batch("CREATE TEMP TABLE keep_books (id INTEGER PRIMARY KEY)")?;
    {
        let mut ins = conn.prepare("INSERT OR IGNORE INTO keep_books (id) VALUES (?1)")?;
        for id in keep {
            ins.execute([id])?;
        }
    }
    for table in ["file_checksums", "enrichment_queue"] {
        if table_exists(&conn, "main", table) {
            conn.execute(&format!("DELETE FROM {table} WHERE book_id NOT IN (SELECT id FROM keep_books)"), [])?;
        }
    }
    if table_exists(&conn, "main", "import_ledger") {
        conn.execute("DELETE FROM import_ledger WHERE book_uuid IS NULL", [])?;
        let uuids: Vec<String> = {
            let mut stmt = conn.prepare("SELECT DISTINCT book_uuid FROM import_ledger")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<std::result::Result<_, _>>()?
        };
        for uuid in uuids.into_iter().filter(|u| !keep_uuids.contains(u)) {
            conn.execute("DELETE FROM import_ledger WHERE book_uuid = ?1", [uuid])?;
        }
    }
    if table_exists(&conn, "main", "pdf_fingerprints") && table_exists(&conn, "main", "file_checksums") {
        conn.execute("DELETE FROM pdf_fingerprints WHERE blake3_hex NOT IN (SELECT blake3_hex FROM file_checksums)", [])?;
    }
    if table_exists(&conn, "main", "inflight") {
        conn.execute("DELETE FROM inflight", [])?;
    }
    conn.execute_batch("DROP TABLE temp.keep_books")?;
    let _: String = conn.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    conn.execute_batch("VACUUM")?;
    Ok(())
}

/// Filters the scratch `notes.db` to notes on items that survived the prune of
/// `metadata` (also a scratch file). Returns the resource hashes still linked.
fn prune_notes(path: &Path, metadata: &Path) -> anyhow::Result<HashSet<String>> {
    let conn = open_scratch_db(path)?;
    conn.execute("ATTACH DATABASE ?1 AS meta", [metadata.to_string_lossy().as_ref()])?;
    for (colname, table) in [("authors", "authors"), ("tags", "tags"), ("series", "series"), ("publishers", "publishers"), ("languages", "languages")] {
        if table_exists(&conn, "meta", table) {
            conn.execute(&format!("DELETE FROM notes WHERE colname = '{colname}' AND item NOT IN (SELECT id FROM meta.{table})"), [])?;
        }
    }
    let custom: Vec<(i64, String)> = if table_exists(&conn, "meta", "custom_columns") {
        let mut stmt = conn.prepare("SELECT id, label FROM meta.custom_columns")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<std::result::Result<_, _>>()?
    } else {
        Vec::new()
    };
    for (id, label) in custom {
        let table = format!("custom_column_{id}");
        if table_exists(&conn, "meta", &table) {
            conn.execute(&format!("DELETE FROM notes WHERE colname = ?1 AND item NOT IN (SELECT id FROM meta.\"{table}\")"), [format!("#{label}")])?;
        }
    }
    conn.execute("DELETE FROM resources WHERE hash NOT IN (SELECT resource FROM notes_resources_link)", [])?;
    let hashes: HashSet<String> = {
        let mut stmt = conn.prepare("SELECT hash FROM resources")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    conn.execute_batch("DETACH DATABASE meta")?;
    let _: String = conn.query_row("PRAGMA journal_mode=DELETE", [], |r| r.get(0))?;
    conn.execute_batch("VACUUM")?;
    Ok(hashes)
}

/// `op` restricted to the chosen books and without the copy-list preference;
/// `None` when nothing of it is left.
fn filter_op(op: &ChangeOp, keep: Option<&HashSet<String>>) -> Option<ChangeOp> {
    match op {
        ChangeOp::PrefSet { key, .. } if key == TARGETS_PREF || key == LEGACY_TARGETS_PREF => None,
        ChangeOp::Batch { ops } => {
            let inner: Vec<ChangeOp> = ops.iter().filter_map(|o| filter_op(o, keep)).collect();
            (!inner.is_empty()).then_some(ChangeOp::Batch { ops: inner })
        }
        other => match (other.book(), keep) {
            (Some(book), Some(keep)) if !keep.contains(book) => None,
            _ => Some(other.clone()),
        },
    }
}

struct Rebased {
    name: String,
    bytes: Vec<u8>,
    /// Cover blobs the kept changes point at.
    blobs: HashSet<String>,
}

/// The source's effective history, filtered, re-chained under `origin` as one
/// snapshot. Deterministic: the same history gives the same bytes.
fn rebase_change_log(cache: &Cache, origin: &str, keep_uuids: Option<&HashSet<String>>) -> anyhow::Result<Rebased> {
    let log = cache.backend.change_log()?;
    let replay = log.replay()?;
    let mut rebased: Vec<Change> = Vec::new();
    let mut blobs = HashSet::new();
    let mut prev: Option<String> = None;
    let mut last = Hlc::ZERO;
    for change in replay.changes() {
        let Some(op) = filter_op(&change.op, keep_uuids) else { continue };
        let mut collect = vec![&op];
        while let Some(o) = collect.pop() {
            match o {
                ChangeOp::CoverSet { blob: Some(b), .. } => {
                    blobs.insert(b.clone());
                }
                ChangeOp::Batch { ops } => collect.extend(ops.iter()),
                _ => {}
            }
        }
        let next = Change::new(change.hlc, origin.to_string(), rebased.len() as u64 + 1, prev.clone(), op);
        prev = Some(next.hash.clone());
        last = last.max(next.hlc);
        rebased.push(next);
    }
    let mut watermarks = BTreeMap::new();
    if let Some(tip) = rebased.last() {
        watermarks.insert(origin.to_string(), Watermark { seq: tip.seq, hash: tip.hash.clone() });
    }
    let header = SnapshotHeader { watermarks, taken_at: last };
    let mut bytes = serde_json::to_vec(&header)?;
    bytes.push(b'\n');
    for change in &rebased {
        bytes.extend_from_slice(&serde_json::to_vec(change)?);
        bytes.push(b'\n');
    }
    Ok(Rebased { name: format!("{}.jsonl", last.file_prefix()), bytes, blobs })
}

// ---------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------

fn walk_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_map(std::result::Result::ok).filter(|e| e.file_type().is_file()).map(|e| e.into_path()).collect()
}

/// Plans a run: what a copy of `cache`'s library at `target` would have to do.
/// Changes nothing at `target` and never opens a database there.
pub fn plan(cache: &Cache, target: &str, options: &MirrorOptions) -> Result<MirrorPlan> {
    plan_controlled(cache, target, options, None)
}

/// [`plan`] as a job: names what it is doing on `control` (`mirror_listing`,
/// `mirror_databases`, `mirror_comparing`) and counts the files it has listed in
/// `seen`, and stops (with an error) when cancelled.
pub fn plan_controlled(cache: &Cache, target: &str, options: &MirrorOptions, control: Option<&JobControl>) -> Result<MirrorPlan> {
    let phase = |name: &str| {
        if let Some(c) = control {
            c.set_phase(name);
        }
    };
    let cancelled = || control.is_some_and(|c| c.is_cancelled());
    phase("mirror_listing");
    let source = cache.backend.library_path.clone();
    let target = validate_target(&source, target)?;
    if options.mode == Mode::Mirror && options.scope != MirrorScope::All {
        return refuse("A backup mirror always holds the whole library. Use a travel copy for some of the books.");
    }
    if crate::storage_safety::is_open_in_process(&target) {
        return refuse("That folder is a library that is open right now. Choose a folder that is not in use.");
    }
    let source_id = cache.library_id();

    // New target, or one an earlier run made?
    let mut old_manifest = None;
    let mut new_target = true;
    if target.is_dir() {
        let empty = fs::read_dir(&target).map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty {
            new_target = false;
            match read_manifest(&target) {
                Ok(Some(m)) => old_manifest = Some(m),
                Ok(None) => return refuse(format!("{} is not empty, and it was not made by \"Export a copy\". Choose an empty folder (or a new one) so nothing in it is overwritten.", target.display())),
                Err(e) => return refuse(format!("{} has a copy manifest that cannot be read ({e:#}). Choose another folder.", target.display())),
            }
        }
    }
    if let Some(m) = &old_manifest {
        if m.source_id != source_id {
            return refuse(format!("{} is a copy of a different library. Choose another folder.", target.display()));
        }
        if m.mode != options.mode {
            let (has, asked) = (if m.mode == Mode::Travel { "a travel copy" } else { "a backup mirror" }, if options.mode == Mode::Travel { "a travel copy" } else { "a backup mirror" });
            return refuse(format!("{} holds {has}, not {asked}. Choose another folder, or use the same kind as before.", target.display()));
        }
    }
    let tier = classify_target(&target);

    let travel = options.mode == Mode::Travel;
    let subset = travel && options.scope != MirrorScope::All;
    let mut scratch = Scratch { dir: Arc::new(tempfile::tempdir().map_err(anyhow::Error::from)?), counter: 0 };
    let mut reused: Vec<Arc<tempfile::TempDir>> = Vec::new();

    let (books, formats) = load_books(cache)?;
    let chosen = choose(&books, if travel { &options.scope } else { &MirrorScope::All });
    let keep_ids = subset.then_some(&chosen.ids);
    let keep_uuids = subset.then_some(&chosen.uuids);

    let travel_origin = travel.then(|| old_manifest.as_ref().and_then(|m| m.travel_origin.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().simple().to_string()[..16].to_string()));
    let travel_library_id = travel.then(|| old_manifest.as_ref().and_then(|m| m.travel_library_id.clone()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()));

    let recorded = cache.checksums().all_format_records().unwrap_or_default();
    let mut files: BTreeMap<String, SourceFile> = BTreeMap::new();
    let mut missing = Vec::new();
    let mut add = |file: SourceFile| {
        if !is_excluded(&file.rel) {
            files.entry(file.rel.clone()).or_insert(file);
        }
    };

    // Book files.
    let mut seen_dirs: HashSet<String> = HashSet::new();
    if let Some(c) = control {
        c.total.store(chosen.ids.len() as u64, std::sync::atomic::Ordering::Relaxed);
    }
    for book in books.iter().filter(|b| chosen.ids.contains(&b.id)) {
        if cancelled() {
            return refuse("cancelled");
        }
        if let Some(c) = control {
            c.seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let Some(dir) = clean_rel(&book.path) else { continue };
        for (name, format) in formats.get(&book.id).into_iter().flatten() {
            let lower = join_rel(&dir, &format!("{name}.{}", format.to_lowercase()));
            let upper = join_rel(&dir, &format!("{name}.{}", format.to_uppercase()));
            let found = [lower.clone(), upper].into_iter().find_map(|rel| stat_file(&abs(&source, &rel)).map(|s| (rel, s)));
            match found {
                Some((rel, (size, mtime))) => {
                    let rec = recorded.get(&(book.id, format.to_uppercase())).filter(|(_, rsize, _)| *rsize as u64 == size).map(|(h, _, _)| h.clone());
                    add(SourceFile { path: abs(&source, &rel), rel, kind: Kind::Format, size, mtime_ms: mtime, generated: false, hash: None, recorded: rec });
                }
                None => missing.push(lower),
            }
        }
        if !dir.is_empty() && seen_dirs.insert(dir.clone()) {
            for (name, kind) in [("cover.jpg", Kind::Cover), ("metadata.opf", Kind::Extra)] {
                let rel = join_rel(&dir, name);
                if let Some((size, mtime)) = stat_file(&abs(&source, &rel)) {
                    add(SourceFile { path: abs(&source, &rel), rel, kind, size, mtime_ms: mtime, generated: false, hash: None, recorded: None });
                }
            }
            let data = abs(&source, &join_rel(&dir, "data"));
            if data.is_dir() {
                for path in walk_files(&data) {
                    if let (Some(rel), Some((size, mtime))) = (rel_of(&source, &path), stat_file(&path)) {
                        add(SourceFile { path, rel, kind: Kind::Extra, size, mtime_ms: mtime, generated: false, hash: None, recorded: None });
                    }
                }
            }
        }
    }

    // The rebased change log (travel) decides which cover blobs survive.
    let mut rebased = None;
    if travel {
        rebased = Some(rebase_change_log(cache, travel_origin.as_deref().unwrap(), keep_uuids)?);
    }

    // `.lexmechanic/`: covers, the log, other durable state, checksums.db.
    phase("mirror_reading");
    let state_dir = source.join(LIBRARY_HANDLE_DIR_NAME);
    let mut generated: Vec<SourceFile> = Vec::new();
    let mut checksums_scratch: Option<PathBuf> = None;
    for path in walk_files(&state_dir) {
        let Some(rel) = rel_of(&source, &path) else { continue };
        if is_excluded(&rel) {
            continue;
        }
        let inside = rel.strip_prefix(&format!("{LIBRARY_HANDLE_DIR_NAME}/")).unwrap_or(&rel).to_string();
        let first = inside.split('/').next().unwrap_or("");
        let name = inside.rsplit('/').next().unwrap_or("");
        if name.ends_with(".db") && !inside.contains('/') {
            if subset && name != CHECKSUMS_DB_NAME {
                continue;
            }
            // A whole-library plan reads the checksum database as it is, so an unchanged one
            // need not be snapshotted again; a subset prunes its copy, so makes its own.
            let dest = if name == CHECKSUMS_DB_NAME && keep_ids.is_none() {
                checksums_snapshot(cache, &path, &mut scratch, name, &mut reused)?
            } else {
                let dest = scratch.next(name);
                snapshot_database(&path, &dest)?;
                dest
            };
            if name == CHECKSUMS_DB_NAME {
                if let (Some(ids), Some(uuids)) = (keep_ids, keep_uuids) {
                    prune_checksums(&dest, ids, uuids)?;
                }
                checksums_scratch = Some(dest.clone());
            }
            generated.push(generated_file(&rel, Kind::Database, dest)?);
            continue;
        }
        let Some((size, mtime)) = stat_file(&path) else { continue };
        let kind = match first {
            "covers" => Kind::Cover,
            "changes" | "snapshots" => Kind::Log,
            _ => Kind::State,
        };
        if travel && (kind == Kind::Log || inside == "install-id" || inside == crate::constants::LIBRARY_ID_FILE_NAME) {
            continue;
        }
        if kind == Kind::Cover && subset {
            let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
            let blobs = rebased.as_ref().map(|r| &r.blobs);
            if !(chosen.uuids.contains(stem) || blobs.is_some_and(|b| b.contains(stem) || b.contains(name))) {
                continue;
            }
        }
        add(SourceFile { path, rel, kind, size, mtime_ms: mtime, generated: false, hash: None, recorded: None });
    }
    let _ = checksums_scratch;

    // metadata.db, full-text-search.db, notes.
    let mut pruned_metadata: Option<PathBuf> = None;
    let metadata_src = source.join("metadata.db");
    if metadata_src.is_file() {
        let dest = scratch.next("metadata.db");
        snapshot_database(&metadata_src, &dest)?;
        rewrite_metadata(&dest, keep_ids, travel_library_id.as_deref())?;
        pruned_metadata = Some(dest.clone());
        generated.push(generated_file("metadata.db", Kind::Database, dest)?);
    }
    let fts_src = source.join(FTS_DB_NAME);
    if fts_src.is_file() && !subset {
        let dest = scratch.next(FTS_DB_NAME);
        snapshot_database(&fts_src, &dest)?;
        generated.push(generated_file(FTS_DB_NAME, Kind::Database, dest)?);
    }
    let notes_dir = source.join(NOTES_DIR_NAME);
    let mut kept_resources: Option<HashSet<String>> = None;
    let notes_db = notes_dir.join(NOTES_DB_NAME);
    if notes_db.is_file() {
        let dest = scratch.next(NOTES_DB_NAME);
        snapshot_database(&notes_db, &dest)?;
        if subset {
            if let Some(meta) = &pruned_metadata {
                kept_resources = Some(prune_notes(&dest, meta)?);
            }
        }
        generated.push(generated_file(&format!("{NOTES_DIR_NAME}/{NOTES_DB_NAME}"), Kind::Database, dest)?);
    }
    let kept_resource_paths: Option<HashSet<String>> = kept_resources.map(|hashes| hashes.iter().map(|h| notes_resource_rel(h)).collect());
    for path in walk_files(&notes_dir) {
        let Some(rel) = rel_of(&source, &path) else { continue };
        if rel.ends_with(".db") || is_excluded(&rel) {
            continue;
        }
        if kept_resource_paths.as_ref().is_some_and(|keep| !keep.contains(&rel)) {
            continue;
        }
        if let Some((size, mtime)) = stat_file(&path) {
            add(SourceFile { path, rel, kind: Kind::Notes, size, mtime_ms: mtime, generated: false, hash: None, recorded: None });
        }
    }
    if let Some(r) = &rebased {
        let rel = format!("{LIBRARY_HANDLE_DIR_NAME}/snapshots/{}", r.name);
        let dest = scratch.next(&r.name);
        fs::write(&dest, &r.bytes).map_err(anyhow::Error::from)?;
        generated.push(generated_file(&rel, Kind::Log, dest)?);
    }
    for g in generated {
        add(g);
    }
    drop(add);

    // Travel: identity files are the copy's own, written by its first open; the
    // copy's `install-id` is set to the origin the rebased log was made for.
    if let Some(origin) = &travel_origin {
        let dest = scratch.next("install-id");
        fs::write(&dest, origin).map_err(anyhow::Error::from)?;
        let rel = format!("{LIBRARY_HANDLE_DIR_NAME}/install-id");
        files.insert(rel.clone(), generated_file(&rel, Kind::State, dest).map_err(MirrorError::Other)?);
    }

    // Edits the copy made to its own change log (not written by a refresh).
    let mut copy_edits = Vec::new();
    if let Some(m) = &old_manifest {
        if travel {
            let known: HashSet<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
            let changes = target.join(LIBRARY_HANDLE_DIR_NAME).join("changes");
            if let Ok(entries) = fs::read_dir(&changes) {
                for entry in entries.flatten() {
                    let rel = format!("{LIBRARY_HANDLE_DIR_NAME}/changes/{}", entry.file_name().to_string_lossy());
                    if !known.contains(rel.as_str()) && !is_excluded(&rel) {
                        copy_edits.push(rel);
                    }
                }
            }
            copy_edits.sort();
        }
    }

    if cancelled() {
        return refuse("cancelled");
    }
    phase("mirror_comparing");
    let items = diff(&files, &target, old_manifest.as_ref(), options, &copy_edits);
    let totals = totals_of(&items);
    let fingerprint = fingerprint_of(&target, &source_id, options, &items);
    Ok(MirrorPlan {
        mode: options.mode,
        target,
        source_id,
        options: options.clone(),
        tier,
        new_target,
        books: chosen.ids.len(),
        items,
        totals,
        fingerprint,
        missing_sources: missing,
        copy_edits,
        travel_origin,
        travel_library_id,
        _scratch: scratch.dir,
        _reused: reused,
    })
}

/// The last snapshot of `checksums.db` a plan made, and what the library looked
/// like then. `VACUUM INTO` of a library-sized checksum database takes seconds
/// (3.5 s for 51 MB), and every preview and every run planned afresh: the same
/// snapshot, again, when nothing had written to it.
struct CachedSnapshot {
    source: PathBuf,
    stamp: String,
    dir: Arc<tempfile::TempDir>,
    file: PathBuf,
}

fn snapshot_cache() -> &'static std::sync::Mutex<Option<CachedSnapshot>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<Option<CachedSnapshot>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// What `checksums.db` looks like from here: this connection's own change count
/// (it writes the attached database), the database's `data_version` (which moves
/// when any *other* connection commits), and the file sizes. `None` when the
/// sidecar is not attached, which means no caching.
fn checksums_stamp(cache: &Cache, path: &Path) -> Option<String> {
    let (changes, version) = {
        let conn = cache.backend.conn.lock().unwrap();
        let version: i64 = conn.query_row("PRAGMA checksums_db.data_version", [], |r| r.get(0)).ok()?;
        let changes: i64 = conn.query_row("SELECT total_changes()", [], |r| r.get(0)).ok()?;
        (changes, version)
    };
    let len = |p: &Path| fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    Some(format!("{changes}/{version}/{}/{}", len(path), len(Path::new(&wal))))
}

/// A snapshot of `checksums.db` for a plan: the one made for the last plan when
/// the stamp says nothing wrote since, else a new one (which is remembered).
/// The folder to keep alive is pushed on `reused` when an old one is used.
fn checksums_snapshot(cache: &Cache, src: &Path, scratch: &mut Scratch, name: &str, reused: &mut Vec<Arc<tempfile::TempDir>>) -> anyhow::Result<PathBuf> {
    let stamp = checksums_stamp(cache, src);
    if let Some(stamp) = &stamp {
        let held = snapshot_cache().lock().unwrap();
        if let Some(c) = held.as_ref().filter(|c| c.source == src && c.stamp == *stamp && c.file.is_file()) {
            reused.push(c.dir.clone());
            return Ok(c.file.clone());
        }
    }
    let dest = scratch.next(name);
    snapshot_database(src, &dest)?;
    if let Some(stamp) = stamp {
        // The stamp was read before the snapshot: a write in between makes the
        // next stamp differ, so the worst case is one snapshot too many.
        *snapshot_cache().lock().unwrap() = Some(CachedSnapshot { source: src.to_path_buf(), stamp, dir: scratch.dir.clone(), file: dest.clone() });
    }
    Ok(dest)
}

/// How a source file compares with what the manifest says was written.
enum SourceState {
    Same { refreshed_mtime: bool },
    Changed { hash: Option<String> },
}

fn source_state(s: &SourceFile, m: &ManifestFile) -> SourceState {
    if s.generated {
        return if s.hash.as_deref() == Some(m.blake3.as_str()) { SourceState::Same { refreshed_mtime: false } } else { SourceState::Changed { hash: s.hash.clone() } };
    }
    if s.size != m.size {
        return SourceState::Changed { hash: None };
    }
    if s.mtime_ms == m.mtime_ms {
        return SourceState::Same { refreshed_mtime: false };
    }
    // The mtime moved: the content has to be looked at.
    match hash_file(&s.path) {
        Ok(h) if h == m.blake3 => SourceState::Same { refreshed_mtime: true },
        Ok(h) => SourceState::Changed { hash: Some(h) },
        Err(_) => SourceState::Changed { hash: None },
    }
}

/// Whether the file on the copy is still what the manifest says: size and mtime
/// (with tolerance), and only when those differ, its content.
fn copy_untouched(path: &Path, m: &ManifestFile) -> bool {
    let Some((size, mtime)) = stat_file(path) else { return false };
    if size != m.size {
        return false;
    }
    same_mtime(mtime, m.mtime_ms) || hash_file(path).is_ok_and(|h| h == m.blake3)
}

fn entry_for(s: &SourceFile, hash: &str) -> ManifestFile {
    ManifestFile { path: s.rel.clone(), size: s.size, mtime_ms: s.mtime_ms, blake3: hash.to_string(), kind: s.kind }
}

fn item_for(s: &SourceFile, action: Action, reason: Option<String>, entry: Option<ManifestFile>) -> PlanItem {
    PlanItem { rel: s.rel.clone(), kind: s.kind, action, size: s.size, mtime_ms: s.mtime_ms, blake3: s.hash.clone(), source: Some(s.path.clone()), generated: s.generated, reason, entry, recorded: s.recorded.clone() }
}

fn diff(files: &BTreeMap<String, SourceFile>, target: &Path, old: Option<&Manifest>, options: &MirrorOptions, copy_edits: &[String]) -> Vec<PlanItem> {
    let manifest: HashMap<&str, &ManifestFile> = old.map(|m| m.files.iter().map(|f| (f.path.as_str(), f)).collect()).unwrap_or_default();
    let overwrite = options.overwrite_changed;
    let mut items = Vec::with_capacity(files.len());

    for (rel, s) in files {
        let dest = abs(target, rel);
        let m = manifest.get(rel.as_str()).copied();
        let on_copy = stat_file(&dest);
        let old_entry = m.cloned();
        let item = match (m, on_copy) {
            (None, None) => item_for(s, Action::CopyNew, None, None),
            (Some(m), None) => item_for(s, Action::CopyNew, Some("it was missing from the copy".into()), Some(m.clone())),
            (None, Some((size, mtime))) => {
                // Not written by a refresh. If it is the very file (an earlier run
                // finished it but stopped before saving the manifest), adopt it.
                let ours = if s.generated { s.hash.as_deref().zip(hash_file(&dest).ok()).is_some_and(|(a, b)| a == b) } else { size == s.size && same_mtime(mtime, s.mtime_ms) && hash_file(&s.path).ok().zip(hash_file(&dest).ok()).is_some_and(|(a, b)| a == b) };
                if ours {
                    let hash = if s.generated { s.hash.clone().unwrap_or_default() } else { hash_file(&dest).unwrap_or_default() };
                    item_for(s, Action::Unchanged, Some("already in the copy".into()), Some(entry_for(s, &hash)))
                } else if overwrite {
                    item_for(s, Action::Update, Some("replaces a different file that was already there".into()), None)
                } else {
                    item_for(s, Action::Conflict, Some("a different file is already there and was not written by a refresh".into()), None)
                }
            }
            (Some(m), Some(_)) => match source_state(s, m) {
                SourceState::Same { refreshed_mtime } => {
                    let mut entry = m.clone();
                    if refreshed_mtime {
                        entry.mtime_ms = s.mtime_ms;
                    }
                    item_for(s, Action::Unchanged, None, Some(entry))
                }
                SourceState::Changed { hash } => {
                    let mut item = if copy_untouched(&dest, m) {
                        item_for(s, Action::Update, None, old_entry)
                    } else if overwrite {
                        item_for(s, Action::Update, Some("overwrites changes made in the copy".into()), old_entry)
                    } else {
                        item_for(s, Action::Conflict, Some(format!("{rel} in the copy was changed since the last refresh; refreshing would discard those changes")), old_entry)
                    };
                    if item.blake3.is_none() {
                        item.blake3 = hash;
                    }
                    item
                }
            },
        };
        items.push(item);
    }

    // What the library no longer has.
    for (rel, m) in &manifest {
        if files.contains_key(*rel) {
            continue;
        }
        let dest = abs(target, rel);
        let Some((size, _)) = stat_file(&dest) else { continue };
        let untouched = copy_untouched(&dest, m);
        let (action, reason) = if !options.remove_missing {
            (Action::Unchanged, Some("no longer in the library (kept in the copy)".to_string()))
        } else if untouched || overwrite {
            (Action::Remove, Some("no longer in the library".to_string()))
        } else {
            (Action::Conflict, Some("no longer in the library, but changed in the copy, so it was not removed".to_string()))
        };
        items.push(PlanItem { rel: (*rel).to_string(), kind: m.kind, action, size, mtime_ms: m.mtime_ms, blake3: Some(m.blake3.clone()), source: None, generated: false, reason, entry: Some((*m).clone()), recorded: None });
    }

    // The copy's own change-log entries are chained to the copy's baseline
    // snapshot and recorded in its metadata.db: the three belong together. A
    // refresh that writes, replaces or removes the database or any snapshot
    // (a new source history gives the snapshot a new name, so this is not only
    // "replacing the same file") would leave the edits pointing at a baseline
    // that is gone. Without `overwrite_changed` all of those wait; with it, the
    // database is always rewritten so the edits are set aside with it.
    if !copy_edits.is_empty() {
        let snapshots = format!("{LIBRARY_HANDLE_DIR_NAME}/snapshots/");
        let is_db = |item: &PlanItem| item.rel == "metadata.db" || item.rel.starts_with(&snapshots);
        let touches = items.iter().any(|i| is_db(i) && matches!(i.action, Action::Update | Action::CopyNew | Action::Remove));
        if touches {
            for item in items.iter_mut().filter(|i| is_db(i)) {
                if !overwrite && matches!(item.action, Action::Update | Action::CopyNew | Action::Remove) {
                    item.reason = Some(format!("the copy has {} change(s) of its own; refreshing would discard them", copy_edits.len()));
                    item.action = Action::Conflict;
                } else if overwrite && item.rel == "metadata.db" && item.action == Action::Unchanged && item.source.is_some() {
                    item.reason = Some("replaces the copy's edited database".into());
                    item.action = Action::Update;
                }
            }
        }
    }
    items.sort_by(|a, b| a.rel.cmp(&b.rel));
    items
}

fn totals_of(items: &[PlanItem]) -> Totals {
    let mut t = Totals::default();
    for item in items {
        let bump = |c: &mut Count| {
            c.files += 1;
            c.bytes += item.size;
        };
        let kind = t.by_kind.entry(item.kind.name().to_string()).or_default();
        match item.action {
            Action::CopyNew => {
                bump(&mut t.copy_new);
                bump(&mut kind.copy_new);
                t.bytes_to_copy += item.size;
            }
            Action::Update => {
                bump(&mut t.update);
                bump(&mut kind.update);
                t.bytes_to_copy += item.size;
            }
            Action::Unchanged => {
                bump(&mut t.unchanged);
                bump(&mut kind.unchanged);
            }
            // An older history snapshot being replaced is housekeeping, not a
            // file the user lost: never in "to remove", never a confirmation.
            Action::Remove if is_history_snapshot(&item.rel) => {}
            Action::Remove => {
                bump(&mut t.remove);
                bump(&mut kind.remove);
            }
            Action::Conflict => {
                bump(&mut t.conflict);
                bump(&mut kind.conflict);
            }
        }
    }
    t
}

/// Identifies what a run would do. A database snapshot is a different set of
/// bytes whenever anything in it moved, so generated files count by name only:
/// the fingerprint is about *which* files change, not a hash a second plan could
/// legitimately differ in.
fn fingerprint_of(target: &Path, source_id: &str, options: &MirrorOptions, items: &[PlanItem]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(format!("{}|{}|{}|{:?}|{}|{}\n", target.display(), source_id, options.mode.name(), options.scope, options.remove_missing, options.overwrite_changed).as_bytes());
    for item in items {
        if item.generated {
            h.update(format!("g:{}\n", item.rel).as_bytes());
        } else {
            h.update(format!("{}:{}:{}:{}\n", item.rel, item.action.name(), item.size, item.mtime_ms).as_bytes());
        }
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

// ---------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct MirrorReport {
    pub run_id: String,
    pub mode: String,
    pub target: String,
    pub copied: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub bytes_copied: u64,
    pub conflicts: Vec<Problem>,
    pub failures: Vec<Problem>,
    pub cancelled: bool,
    /// Moved aside because the copy's own edits were overwritten on request.
    pub edits_set_aside: usize,
    pub trash: Option<String>,
    pub complete: bool,
}

/// Marks an I/O error that must not be retried or mistaken for the target
/// failing: the source side, a verification mismatch, a cancel.
#[derive(Debug)]
struct Marker(String);
impl std::fmt::Display for Marker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Marker {}

fn marked(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::Other, Marker(message.into()))
}

fn is_marked(e: &io::Error) -> bool {
    e.get_ref().is_some_and(|inner| inner.is::<Marker>())
}

fn copy_attempt(src: &Path, dest: &Path, mtime_ms: i64, expect: Option<&str>, control: &JobControl) -> io::Result<(u64, String)> {
    let mut input = fs::File::open(src).map_err(|e| marked(format!("cannot read the library file: {e}")))?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let name = dest.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let tmp = dest.with_file_name(format!("{name}.tmp-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
    let result = (|| -> io::Result<(u64, String)> {
        let mut out = fs::File::create(&tmp)?;
        let mut hasher = blake3::Hasher::new();
        let mut buf = vec![0u8; 1 << 20];
        let mut total = 0u64;
        loop {
            if control.is_cancelled() {
                return Err(marked("cancelled"));
            }
            let n = input.read(&mut buf).map_err(|e| marked(format!("cannot read the library file: {e}")))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            total += n as u64;
        }
        let hash = hasher.finalize().to_hex().to_string();
        if let Some(expected) = expect {
            if expected != hash {
                return Err(marked("what was read does not match the hash the library has for it; the file changed or is damaged"));
            }
        }
        out.flush()?;
        out.sync_all()?;
        if mtime_ms > 0 {
            out.set_modified(UNIX_EPOCH + std::time::Duration::from_millis(mtime_ms as u64))?;
        }
        drop(out);
        fs::rename(&tmp, dest)?;
        Ok((total, hash))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn copy_one(item: &PlanItem, dest: &Path, tier: StorageTier, control: &JobControl) -> io::Result<(u64, String)> {
    let src = item.source.as_ref().ok_or_else(|| marked("nothing to copy"))?;
    let expect = item.blake3.as_deref().or(item.recorded.as_deref());
    if tier == StorageTier::Network {
        crate::library_handle::retry_io_while(|e| !is_marked(e), || copy_attempt(src, dest, item.mtime_ms, expect, control))
    } else {
        copy_attempt(src, dest, item.mtime_ms, expect, control)
    }
}

/// Moves `rel` from the copy into `.mirror-trash/<run>/`, then tidies the folders
/// it leaves empty.
fn set_aside(target: &Path, rel: &str, run_id: &str, park_history: bool) -> io::Result<()> {
    let from = abs(target, rel);
    if is_history_snapshot(rel) && !park_history {
        // An older history snapshot, replaced by the one this run writes: internal
        // state, not the user's file. Deleted outright; parking a library-sized
        // snapshot in `.mirror-trash` on every refresh would fill the copy's disk.
        // (Only called once the new one has copied. When the copy's own edits were set
        // aside this run, the caller parks it with them instead: they chain to it.)
        return fs::remove_file(&from);
    }
    let to = abs(&target.join(MIRROR_TRASH_DIR).join(run_id), rel);
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&from, &to)?;
    let mut dir = from.parent().map(Path::to_path_buf);
    while let Some(d) = dir {
        if d == target || fs::remove_dir(&d).is_err() {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    Ok(())
}

fn target_reachable(target: &Path) -> bool {
    fs::metadata(target).is_ok()
}

/// Files copied at once: `LEXMECHANIC_MIRROR_WORKERS` if set (1 to 16), else 6 on a
/// network target (where each copy mostly waits) and 4 on a local one. A copy
/// is still written to a temporary name, fsynced, and renamed into place, so a
/// file cut short is never taken for a finished one; the fsyncs overlap instead
/// of queueing.
fn copy_workers(tier: StorageTier) -> usize {
    if let Some(n) = lexmechanic_utils::env::var("LEXMECHANIC_MIRROR_WORKERS").ok().and_then(|v| v.trim().parse::<usize>().ok()) {
        return n.clamp(1, 16);
    }
    if tier == StorageTier::Network {
        6
    } else {
        4
    }
}

/// Runs `plan`: copies and updates what it says, sets aside what it removes,
/// then writes the databases and, last, the manifest. A cancel stops between
/// files (what is done is recorded); the target becoming unreachable aborts the
/// run with an error rather than treating anything as missing.
pub fn execute(cache: &Cache, plan: &MirrorPlan, control: &JobControl) -> anyhow::Result<MirrorReport> {
    let every = (plan.items.len() / 20).max(500);
    execute_inner(cache, plan, control, every, None, copy_workers(plan.tier))
}

fn execute_inner(_cache: &Cache, plan: &MirrorPlan, control: &JobControl, checkpoint_every: usize, after_file: Option<&dyn Fn(usize)>, workers: usize) -> anyhow::Result<MirrorReport> {
    let target = &plan.target;
    fs::create_dir_all(target).with_context(|| format!("creating {}", target.display()))?;
    let run_id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    let mut report = MirrorReport { run_id: run_id.clone(), mode: plan.mode.name().into(), target: target.display().to_string(), ..Default::default() };

    // The manifest as it stands: every item's "if nothing is written" entry.
    let mut entries: BTreeMap<String, ManifestFile> = BTreeMap::new();
    for item in &plan.items {
        if let Some(entry) = &item.entry {
            entries.insert(item.rel.clone(), entry.clone());
        }
    }
    let manifest_of = |entries: &BTreeMap<String, ManifestFile>, complete: bool| Manifest {
        version: 1,
        source_id: plan.source_id.clone(),
        mode: plan.mode,
        run_id: run_id.clone(),
        written_at_ms: now_ms(),
        scope: plan.options.scope.clone(),
        complete,
        travel_origin: plan.travel_origin.clone(),
        travel_library_id: plan.travel_library_id.clone(),
        files: entries.values().cloned().collect(),
    };

    let writes: Vec<&PlanItem> = plan.items.iter().filter(|i| matches!(i.action, Action::CopyNew | Action::Update)).collect();
    control.total.store(writes.len() as u64, std::sync::atomic::Ordering::Relaxed);
    control.already_known.store(plan.totals.unchanged.files as u64, std::sync::atomic::Ordering::Relaxed);
    for item in &plan.items {
        match item.action {
            Action::Conflict => report.conflicts.push(Problem { path: item.rel.clone(), message: item.reason.clone().unwrap_or_default() }),
            Action::Unchanged => report.unchanged += 1,
            _ => {}
        }
    }

    // Book files first, databases after, in this order within each group.
    let (books, dbs): (Vec<&PlanItem>, Vec<&PlanItem>) = writes.into_iter().partition(|i| !i.generated);
    let mut dbs = dbs;
    dbs.sort_by_key(|i| (i.rel == "metadata.db", i.kind != Kind::Log, i.rel.clone()));

    // On request, the copy's own edits make way for the refreshed database.
    if plan.options.overwrite_changed && !plan.copy_edits.is_empty() && dbs.iter().any(|i| i.rel == "metadata.db") {
        for rel in &plan.copy_edits {
            if set_aside(target, rel, &run_id, false).is_ok() {
                report.edits_set_aside += 1;
            }
        }
    }

    let mut done = 0usize;
    let mut since_checkpoint = 0usize;
    let mut aborted: Option<String> = None;
    control.set_phase("mirror_files");
    for (phase, group) in [("mirror_files", &books), ("mirror_databases", &dbs)] {
        control.set_phase(phase);
        // What becomes of one finished copy. Returns false to stop the group
        // (a cancel, or the target gone).
        let mut record = |item: &PlanItem, result: io::Result<(u64, String)>| -> bool {
            match result {
                Ok((size, hash)) => {
                    entries.insert(item.rel.clone(), ManifestFile { path: item.rel.clone(), size, mtime_ms: item.mtime_ms, blake3: hash, kind: item.kind });
                    if item.action == Action::CopyNew {
                        report.copied += 1;
                    } else {
                        report.updated += 1;
                    }
                    report.bytes_copied += size;
                    control.bytes_hashed.fetch_add(size, std::sync::atomic::Ordering::Relaxed);
                    control.added.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    done += 1;
                    since_checkpoint += 1;
                    if let Some(hook) = after_file {
                        hook(done);
                    }
                    if since_checkpoint >= checkpoint_every {
                        since_checkpoint = 0;
                        let _ = write_manifest(target, &manifest_of(&entries, false));
                    }
                    true
                }
                Err(e) if is_marked(&e) && e.to_string() == "cancelled" => false,
                Err(e) => {
                    control.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if !is_marked(&e) && !target_reachable(target) {
                        aborted = Some(format!("{} stopped answering while {} was being written: {e}", target.display(), item.rel));
                        return false;
                    }
                    report.failures.push(Problem { path: item.rel.clone(), message: e.to_string() });
                    true
                }
            }
        };

        if workers <= 1 || group.len() <= 1 {
            for item in group {
                if control.is_cancelled() {
                    break;
                }
                let result = copy_one(item, &abs(target, &item.rel), plan.tier, control);
                if !record(item, result) {
                    break;
                }
            }
        } else {
            // A bounded pool: on a share each copy spends most of its time waiting
            // for the far end (an open, a write, an fsync), so several in flight
            // is several times the files a second. Results come back to this
            // thread, which alone updates the report and the manifest, so a
            // checkpoint still lists only files that were fully written and
            // renamed into place.
            let next = std::sync::atomic::AtomicUsize::new(0);
            let stop = std::sync::atomic::AtomicBool::new(false);
            let (tx, rx) = std::sync::mpsc::channel::<(usize, io::Result<(u64, String)>)>();
            std::thread::scope(|scope| {
                let (next, stop) = (&next, &stop);
                for _ in 0..workers.min(group.len()) {
                    let tx = tx.clone();
                    scope.spawn(move || loop {
                        if stop.load(std::sync::atomic::Ordering::Relaxed) || control.is_cancelled() {
                            break;
                        }
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = group.get(i) else { break };
                        let result = copy_one(item, &abs(target, &item.rel), plan.tier, control);
                        if tx.send((i, result)).is_err() {
                            break;
                        }
                    });
                }
                drop(tx);
                for (i, result) in rx {
                    // Keep draining after a stop: copies already in flight finish and are recorded.
                    if !record(&group[i], result) {
                        stop.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            });
        }
        if aborted.is_some() || control.is_cancelled() {
            break;
        }
    }

    if let Some(message) = aborted {
        anyhow::bail!("{message}. The copy was not finished; run it again when the folder is back (what was written is kept).");
    }

    // Removals, only if the run was neither cut short nor failed to copy something: the old
    // history snapshot goes only once its replacement has landed, and a failed copy (a full
    // disk) must leave the copy with the snapshot it has. They stay planned for the next run.
    if !control.is_cancelled() && report.failures.is_empty() {
        control.set_phase("mirror_removing");
        // Edits made on the copy chain to the snapshot being replaced: park it with them.
        let park_history = report.edits_set_aside > 0;
        let mut parked_history = false;
        for item in plan.items.iter().filter(|i| i.action == Action::Remove) {
            match set_aside(target, &item.rel, &run_id, park_history) {
                Ok(()) => {
                    entries.remove(&item.rel);
                    if !is_history_snapshot(&item.rel) {
                        report.removed += 1;
                    } else if park_history {
                        parked_history = true;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    entries.remove(&item.rel);
                }
                Err(e) => report.failures.push(Problem { path: item.rel.clone(), message: format!("could not move it to {MIRROR_TRASH_DIR}: {e}") }),
            }
        }
        if report.removed > 0 || report.edits_set_aside > 0 || parked_history {
            report.trash = Some(format!("{MIRROR_TRASH_DIR}/{run_id}"));
        }
    }

    control.set_phase("mirror_manifest");
    report.cancelled = control.is_cancelled();
    report.complete = !report.cancelled && report.failures.is_empty();
    write_manifest(target, &manifest_of(&entries, report.complete)).with_context(|| format!("writing the manifest in {}", target.display()))?;
    Ok(report)
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use lexmechanic_ebooks::metadata::MetaInformation;

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn add(cache: &Cache, scratch: &Path, title: &str, author: &str, body: &str) -> i32 {
        let file = scratch.join(format!("{title}.txt"));
        std::fs::write(&file, body).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&file, &meta).unwrap()
    }

    /// A library with three books and a cover, plus a scratch folder for sources.
    fn sample() -> (tempfile::TempDir, Cache, tempfile::TempDir, Vec<i32>) {
        let (dir, cache) = library();
        let scratch = tempfile::tempdir().unwrap();
        let ids = vec![add(&cache, scratch.path(), "Alpha", "Ann Author", "alpha body"), add(&cache, scratch.path(), "Beta", "Ben Writer", "beta body"), add(&cache, scratch.path(), "Gamma", "Ann Author", "gamma body")];
        crate::covers::set_cover(&cache, ids[0], b"cover bytes").unwrap();
        (dir, cache, scratch, ids)
    }

    fn target_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn run(cache: &Cache, target: &Path, options: &MirrorOptions) -> (MirrorPlan, MirrorReport) {
        let plan = plan(cache, target.to_str().unwrap(), options).unwrap();
        let report = execute(cache, &plan, &JobControl::new()).unwrap();
        (plan, report)
    }

    fn refuse_message(result: Result<MirrorPlan>) -> String {
        match result {
            Err(MirrorError::Refused(m)) => m,
            Err(other) => panic!("expected a refusal, got {other}"),
            Ok(_) => panic!("expected a refusal"),
        }
    }

    fn mirror_opts() -> MirrorOptions {
        MirrorOptions::new(Mode::Mirror)
    }

    fn travel_opts(scope: MirrorScope) -> MirrorOptions {
        MirrorOptions { scope, ..MirrorOptions::new(Mode::Travel) }
    }

    fn rel_files(root: &Path) -> Vec<String> {
        let mut out: Vec<String> = walk_files(root).iter().filter_map(|p| rel_of(root, p)).collect();
        out.sort();
        out
    }

    #[test]
    fn a_new_target_plans_every_file_and_writes_nothing() {
        let (_dir, cache, _s, _ids) = sample();
        let target = target_dir();
        let dest = target.path().join("copy");
        let plan = plan(&cache, dest.to_str().unwrap(), &mirror_opts()).unwrap();
        assert!(plan.new_target);
        assert_eq!(plan.books, 3);
        assert_eq!(plan.totals.copy_new.files, plan.items.len());
        assert_eq!(plan.totals.by_kind["format"].copy_new.files, 3);
        assert!(plan.totals.by_kind.contains_key("database"));
        assert!(plan.totals.bytes_to_copy > 0);
        assert!(!dest.exists(), "a dry run writes nothing");
        let again = super::plan(&cache, dest.to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(plan.fingerprint, again.fingerprint, "the fingerprint is stable");
    }

    #[test]
    fn a_copy_made_before_the_rename_is_still_recognised_and_gets_the_new_manifest() {
        let (_dir, cache, _s, _ids) = sample();
        let target = target_dir();
        let (_, report) = run(&cache, target.path(), &mirror_opts());
        assert!(report.complete, "{report:?}");
        // Make the copy look like one an older build wrote.
        std::fs::rename(target.path().join(MANIFEST_NAME), target.path().join(LEGACY_MANIFEST_NAME)).unwrap();

        let manifest = read_manifest(target.path()).unwrap().expect("the old manifest is read");
        assert!(manifest.complete);

        // The next run writes the new name and retires the old one.
        let (_, again) = run(&cache, target.path(), &mirror_opts());
        assert!(again.complete, "{again:?}");
        assert!(target.path().join(MANIFEST_NAME).is_file());
        assert!(!target.path().join(LEGACY_MANIFEST_NAME).exists());
    }

    #[test]
    fn a_run_copies_the_library_and_a_second_run_is_all_unchanged_without_reading_sources() {
        let (dir, cache, _s, _ids) = sample();
        let target = target_dir();
        let (first, report) = run(&cache, target.path(), &mirror_opts());
        assert!(report.complete && report.failures.is_empty(), "{report:?}");
        assert_eq!(report.copied, first.totals.copy_new.files);
        assert!(target.path().join(MANIFEST_NAME).is_file());

        let second = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert!(!second.new_target);
        assert_eq!(second.totals.copy_new.files + second.totals.update.files + second.totals.remove.files + second.totals.conflict.files, 0, "{:?}", second.totals);
        assert_eq!(second.totals.unchanged.files, second.items.len());

        // Cheap: with the sources unreadable, an unchanged second plan must not need them. Make
        // every book file unreadable by hashing: if the planner opened them it would fail to
        // match; instead count by removing read access is not portable, so prove it by making the
        // content differ with the same size and mtime -- a rehash would notice, a size+mtime check
        // must not.
        let book = rel_files(dir.path()).into_iter().find(|r| r.ends_with(".txt")).unwrap();
        let path = dir.path().join(&book);
        let meta = std::fs::metadata(&path).unwrap();
        let mtime = meta.modified().unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
        let third = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        let item = third.items.iter().find(|i| i.rel == book).unwrap();
        assert_eq!(item.action, Action::Unchanged, "same size and mtime as the manifest: not rehashed");
    }

    #[test]
    fn a_changed_file_is_updated_and_a_removed_book_goes_to_the_mirror_trash() {
        let (dir, cache, _s, ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());

        let changed = rel_files(dir.path()).into_iter().find(|r| r.contains("Alpha") && r.ends_with(".txt")).unwrap();
        std::fs::write(dir.path().join(&changed), "alpha body, edited and longer").unwrap();
        let removed_book = rel_files(dir.path()).into_iter().find(|r| r.contains("Gamma") && r.ends_with(".txt")).unwrap();
        cache.delete_book_keep_files(ids[2]).unwrap();
        std::fs::remove_file(dir.path().join(&removed_book)).ok();

        let plan = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(plan.items.iter().find(|i| i.rel == changed).unwrap().action, Action::Update);
        assert!(plan.items.iter().any(|i| i.rel == removed_book && i.action == Action::Remove), "{:?}", plan.items.iter().map(|i| (&i.rel, i.action)).collect::<Vec<_>>());
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(report.complete, "{report:?}");
        assert_eq!(std::fs::read_to_string(target.path().join(&changed)).unwrap(), "alpha body, edited and longer");
        assert!(!target.path().join(&removed_book).exists());
        let trashed = target.path().join(MIRROR_TRASH_DIR).join(&report.run_id).join(&removed_book);
        assert_eq!(std::fs::read_to_string(trashed).unwrap(), "gamma body", "removed files are kept, not deleted");
        assert!(rel_files(target.path()).iter().all(|r| !r.starts_with(&format!("{MIRROR_TRASH_DIR}/")) || r.contains(&report.run_id)));
    }

    #[test]
    fn remove_missing_can_be_turned_off() {
        let (dir, cache, _s, ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        let book = rel_files(dir.path()).into_iter().find(|r| r.contains("Gamma") && r.ends_with(".txt")).unwrap();
        cache.delete_book_keep_files(ids[2]).unwrap();
        std::fs::remove_file(dir.path().join(&book)).ok();
        let opts = MirrorOptions { remove_missing: false, ..mirror_opts() };
        let (plan, report) = run(&cache, target.path(), &opts);
        assert_eq!(plan.totals.remove.files, 0);
        assert!(target.path().join(&book).exists(), "kept: {report:?}");
    }

    #[test]
    fn an_edit_in_the_copy_is_a_conflict_unless_overwriting_is_asked_for() {
        let (dir, cache, _s, _ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        let file = rel_files(dir.path()).into_iter().find(|r| r.contains("Alpha") && r.ends_with(".txt")).unwrap();
        // The library changes the file, and so does somebody in the copy.
        std::fs::write(dir.path().join(&file), "alpha, changed in the library").unwrap();
        std::fs::write(target.path().join(&file), "alpha, edited in the copy").unwrap();

        let plan = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        let item = plan.items.iter().find(|i| i.rel == file).unwrap();
        assert_eq!(item.action, Action::Conflict);
        assert!(item.reason.as_deref().unwrap().contains("discard"));
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!(report.conflicts.len(), 1);
        assert_eq!(std::fs::read_to_string(target.path().join(&file)).unwrap(), "alpha, edited in the copy", "a conflict is skipped, not overwritten");
        // Still a conflict next time: the manifest kept the old entry.
        let again = super::plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(again.items.iter().find(|i| i.rel == file).unwrap().action, Action::Conflict);

        let forced = MirrorOptions { overwrite_changed: true, ..mirror_opts() };
        let (_, report) = run(&cache, target.path(), &forced);
        assert!(report.conflicts.is_empty() && report.complete, "{report:?}");
        assert_eq!(std::fs::read_to_string(target.path().join(&file)).unwrap(), "alpha, changed in the library");
    }

    #[test]
    fn a_file_edited_in_the_copy_is_not_removed_when_the_library_drops_it() {
        let (dir, cache, _s, ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        let file = rel_files(dir.path()).into_iter().find(|r| r.contains("Gamma") && r.ends_with(".txt")).unwrap();
        cache.delete_book_keep_files(ids[2]).unwrap();
        std::fs::remove_file(dir.path().join(&file)).ok();
        std::fs::write(target.path().join(&file), "gamma, edited over there").unwrap();
        let plan = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(plan.items.iter().find(|i| i.rel == file).unwrap().action, Action::Conflict);
        execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(target.path().join(&file).exists());
    }

    #[test]
    fn mtime_differences_within_two_seconds_are_the_same() {
        assert!(same_mtime(10_000, 11_999));
        assert!(same_mtime(11_999, 10_000));
        assert!(!same_mtime(10_000, 12_001));
        // A copy whose mtime the share rounded is still untouched.
        let (dir, cache, _s, _ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        let file = rel_files(dir.path()).into_iter().find(|r| r.contains("Beta") && r.ends_with(".txt")).unwrap();
        let on_copy = target.path().join(&file);
        let mtime = std::fs::metadata(&on_copy).unwrap().modified().unwrap();
        std::fs::File::options().write(true).open(&on_copy).unwrap().set_modified(mtime + std::time::Duration::from_millis(1500)).unwrap();
        std::fs::write(dir.path().join(&file), "beta body, longer now").unwrap();
        let plan = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(plan.items.iter().find(|i| i.rel == file).unwrap().action, Action::Update, "1.5 s of rounding is not an edit");
    }

    #[test]
    fn the_target_guards_refuse_unsafe_places() {
        let (dir, cache, _s, _ids) = sample();
        let src = dir.path();
        assert!(refuse_message(plan(&cache, src.to_str().unwrap(), &mirror_opts())).contains("library itself"));
        assert!(refuse_message(plan(&cache, src.join("inside").to_str().unwrap(), &mirror_opts())).contains("inside the library"));
        let parent = src.parent().unwrap();
        assert!(refuse_message(plan(&cache, parent.to_str().unwrap(), &mirror_opts())).contains("contains the library"));
        assert!(refuse_message(plan(&cache, "relative/path", &mirror_opts())).contains("full path"));
        let dotted = format!("{}/../elsewhere", target_dir().path().display());
        assert!(refuse_message(plan(&cache, &dotted, &mirror_opts())).contains(".."));
        assert!(refuse_message(plan(&cache, "  ", &mirror_opts())).contains("Choose"));
    }

    #[test]
    fn a_non_empty_target_without_a_manifest_and_a_foreign_manifest_are_refused() {
        let (_dir, cache, _s, _ids) = sample();
        let target = target_dir();
        std::fs::write(target.path().join("precious.txt"), "mine").unwrap();
        assert!(refuse_message(plan(&cache, target.path().to_str().unwrap(), &mirror_opts())).contains("not empty"));

        // A copy of another library.
        let (_other_dir, other, _s2, _ids2) = sample();
        let other_target = target_dir();
        run(&other, other_target.path(), &mirror_opts());
        assert!(refuse_message(plan(&cache, other_target.path().to_str().unwrap(), &mirror_opts())).contains("different library"));

        // The right library but the other kind of copy.
        let mine = target_dir();
        run(&cache, mine.path(), &mirror_opts());
        assert!(refuse_message(plan(&cache, mine.path().to_str().unwrap(), &travel_opts(MirrorScope::All))).contains("backup mirror"));
        // A mirror is never a subset.
        assert!(refuse_message(plan(&cache, mine.path().to_str().unwrap(), &MirrorOptions { scope: MirrorScope::BookIds(vec![1]), ..mirror_opts() })).contains("whole library"));
    }

    #[test]
    fn a_link_into_the_library_is_refused() {
        let (dir, cache, _s, _ids) = sample();
        let links = target_dir();
        let link = links.path().join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(dir.path(), &link).is_ok();
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").args(["/C", "mklink", "/J", link.to_str().unwrap(), dir.path().to_str().unwrap()]).output().map(|o| o.status.success()).unwrap_or(false);
        if !made {
            eprintln!("cannot create a link here; skipping");
            return;
        }
        let message = refuse_message(plan(&cache, link.join("copy").to_str().unwrap(), &mirror_opts()));
        assert!(message.contains("inside the library") && message.contains("link"), "{message}");
    }

    #[test]
    fn a_library_this_process_has_open_is_refused() {
        let (_dir, cache, _s, _ids) = sample();
        let (other_dir, _other_cache) = library();
        // `_other_cache` holds `other_dir` open.
        assert!(refuse_message(plan(&cache, other_dir.path().to_str().unwrap(), &mirror_opts())).contains("open"));
    }

    #[test]
    fn exclusions_are_one_list() {
        for rel in [".caltrash/x.pdf", ".caltrash", ".mirror-trash/run/a.pdf", ".lexmechanic/writer.lock", ".lexmechanic/writer.owner", ".lexmechanic/journal/1.op", ".lexmechanic/jobs.json", ".lexmechanic/jobs.json.partial", ".lexmechanic/import-staging/a/b.pdf", ".lexmechanic/prepare-stage.json", ".lexmechanic/organize-runs/r.json", ".lexmechanic/retitle-runs/r.json", ".lexmechanic/absorbed-libraries.json", "metadata.db-wal", "metadata.db-shm", ".calnotes/notes.db-journal", "A/B/book.epub.tmp-1a2b", "Old/metadata.db.absorbed-by-lexmechanic", MANIFEST_NAME, "NAS\\.caltrash\\x"] {
            assert!(is_excluded(rel), "{rel} should be excluded");
        }
        for rel in ["metadata.db", "full-text-search.db", ".lexmechanic/changes/0001.json", ".lexmechanic/snapshots/a.jsonl", ".lexmechanic/covers/u.jpg", ".lexmechanic/checksums.db", ".lexmechanic/install-id", ".calnotes/notes.db", "A/B/book.epub", "A/B/notes-journal.pdf", "A/data/x.pdf"] {
            assert!(!is_excluded(rel), "{rel} should be carried");
        }
    }

    #[test]
    fn exclusions_are_honoured_in_a_run() {
        let (dir, cache, scratch, ids) = sample();
        // A trash folder and junk lying around.
        let _ = ids;
        std::fs::create_dir_all(dir.path().join(".caltrash/x")).unwrap();
        std::fs::write(dir.path().join(".caltrash/x/old.txt"), "old").unwrap();
        let state = dir.path().join(LIBRARY_HANDLE_DIR_NAME);
        std::fs::write(state.join("jobs.json"), "{}").unwrap();
        std::fs::create_dir_all(state.join("import-staging")).unwrap();
        std::fs::write(state.join("import-staging/x.pdf"), "x").unwrap();
        std::fs::write(dir.path().join("metadata.db-wal.keep"), "not a sidecar").unwrap();
        let _ = scratch;
        let target = target_dir();
        let (_, report) = run(&cache, target.path(), &mirror_opts());
        assert!(report.complete, "{report:?}");
        let copied = rel_files(target.path());
        assert!(copied.iter().all(|r| *r == MANIFEST_NAME || !is_excluded(r)), "{copied:?}");
        assert!(!copied.iter().any(|r| r.contains("caltrash") || r.contains("jobs.json") || r.contains("import-staging") || r.contains("writer.") || r.contains("journal/")), "{copied:?}");
        assert!(copied.contains(&"metadata.db".to_string()));
    }

    #[test]
    fn the_mirror_keeps_identity_and_opens_as_the_same_library() {
        let (dir, cache, _s, ids) = sample();
        cache.set_field(ids[0], "rating", "8").unwrap();
        let source_id = cache.library_id();
        let install = std::fs::read_to_string(dir.path().join(LIBRARY_HANDLE_DIR_NAME).join("install-id")).unwrap();
        let target = target_dir();
        let (_, report) = run(&cache, target.path(), &mirror_opts());
        assert!(report.complete, "{report:?}");

        let copy = Cache::new(target.path()).unwrap();
        assert_eq!(copy.library_id(), source_id);
        assert_eq!(std::fs::read_to_string(target.path().join(LIBRARY_HANDLE_DIR_NAME).join("install-id")).unwrap(), install);
        assert_eq!(copy.all_book_ids().unwrap(), cache.all_book_ids().unwrap());
        for id in copy.all_book_ids().unwrap() {
            assert_eq!(copy.field_for(id, "title").unwrap(), cache.field_for(id, "title").unwrap());
            assert_eq!(copy.field_for(id, "authors").unwrap(), cache.field_for(id, "authors").unwrap());
        }
        assert_eq!(copy.field_for(ids[0], "rating").unwrap(), cache.field_for(ids[0], "rating").unwrap());
        // The book files are really there and identical.
        for (name, format) in copy.format_files(ids[0]).unwrap() {
            let rel = join_rel(copy.field_for(ids[0], "path").unwrap().unwrap().trim_matches('/'), &format!("{name}.{}", format.to_lowercase()));
            assert_eq!(std::fs::read(target.path().join(&rel)).unwrap_or_else(|e| panic!("{rel}: {e}: {:?}", rel_files(target.path()))), std::fs::read(dir.path().join(&rel)).unwrap());
        }
        // And the log came along as it was.
        assert_eq!(copy.backend.change_log().unwrap().entry_count(), cache.backend.change_log().unwrap().entry_count());
    }

    #[test]
    fn an_unchanged_database_is_not_rewritten() {
        let (_dir, cache, _s, _ids) = sample();
        for opts in [mirror_opts(), travel_opts(MirrorScope::All)] {
            let target = target_dir();
            run(&cache, target.path(), &opts);
            let again = plan(&cache, target.path().to_str().unwrap(), &opts).unwrap();
            let changed: Vec<_> = again.items.iter().filter(|i| i.action != Action::Unchanged).map(|i| (&i.rel, i.action)).collect();
            assert!(changed.is_empty(), "{:?}: {changed:?}", opts.mode);
        }
    }

    #[test]
    fn a_travel_subset_has_exactly_the_chosen_books_and_no_orphans() {
        let (dir, cache, _s, ids) = sample();
        cache.set_field(ids[1], "tags", "only-beta").unwrap();
        cache.set_field(ids[0], "tags", "shared").unwrap();
        cache.set_field(ids[2], "tags", "shared").unwrap();
        cache.set_field(ids[1], "series", "Beta Saga").unwrap();
        let target = target_dir();
        let scope = MirrorScope::BookIds(vec![ids[0], ids[2]]);
        let (plan, report) = run(&cache, target.path(), &travel_opts(scope));
        assert!(report.complete, "{report:?}");
        assert_eq!(plan.books, 2);

        let copy = Cache::new(target.path()).unwrap();
        assert_eq!(copy.all_book_ids().unwrap(), vec![ids[0], ids[2]]);
        let conn = rusqlite::Connection::open(target.path().join("metadata.db")).unwrap();
        let count = |sql: &str| conn.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(count("SELECT COUNT(*) FROM books"), 2);
        // No dangling links and no orphans.
        for (link, column, table) in [("books_authors_link", "author", "authors"), ("books_tags_link", "tag", "tags"), ("books_series_link", "series", "series")] {
            assert_eq!(count(&format!("SELECT COUNT(*) FROM {link} WHERE book NOT IN (SELECT id FROM books)")), 0, "{link}");
            assert_eq!(count(&format!("SELECT COUNT(*) FROM {link} WHERE {column} NOT IN (SELECT id FROM {table})")), 0, "{link}");
            assert_eq!(count(&format!("SELECT COUNT(*) FROM {table} WHERE id NOT IN (SELECT {column} FROM {link})")), 0, "orphans in {table}");
        }
        assert_eq!(count("SELECT COUNT(*) FROM data WHERE book NOT IN (SELECT id FROM books)"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM tags WHERE name = 'only-beta'"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM authors WHERE name = 'Ben Writer'"), 0);
        assert_eq!(count("SELECT COUNT(*) FROM series"), 0);
        let fk: i64 = conn.prepare("PRAGMA foreign_key_check").unwrap().query_map([], |_| Ok(())).unwrap().count() as i64;
        assert_eq!(fk, 0);
        drop(conn);
        // The excluded book's files and cover are not there; the chosen ones are.
        let files = rel_files(target.path());
        assert!(!files.iter().any(|f| f.contains("Beta")), "{files:?}");
        assert!(files.iter().any(|f| f.contains("Alpha") && f.ends_with(".txt")));
        assert!(!rel_files(target.path()).iter().any(|f| f.ends_with(FTS_DB_NAME)), "a subset has no full-text index");
        // Search works on the copy.
        let found = crate::search::search(&copy, "title:Alpha").unwrap();
        assert_eq!(found, vec![ids[0]]);
        drop(dir);
    }

    #[test]
    fn a_travel_copy_has_its_own_identity_and_a_log_that_rebuilds_its_books() {
        let (dir, cache, _s, ids) = sample();
        let source_id = cache.library_id();
        let source_install = std::fs::read_to_string(dir.path().join(LIBRARY_HANDLE_DIR_NAME).join("install-id")).unwrap();
        cache.set_field(ids[0], "rating", "6").unwrap();
        let target = target_dir();
        let scope = MirrorScope::BookIds(vec![ids[0], ids[1]]);
        let (_, report) = run(&cache, target.path(), &travel_opts(scope));
        assert!(report.complete, "{report:?}");

        let state = target.path().join(LIBRARY_HANDLE_DIR_NAME);
        let install = std::fs::read_to_string(state.join("install-id")).unwrap();
        assert_ne!(install.trim(), source_install.trim(), "a fresh origin");
        assert!(!state.join("changes").exists() || std::fs::read_dir(state.join("changes")).unwrap().count() == 0, "the source's loose entries are not copied");
        assert!(std::fs::read_dir(state.join("snapshots")).unwrap().count() >= 1, "one rebased snapshot");

        let copy = Cache::new(target.path()).unwrap();
        assert_ne!(copy.library_id(), source_id, "a fresh library uuid");
        assert_eq!(copy.backend.change_log().unwrap().origin().as_str(), install.trim());
        assert_eq!(copy.all_book_ids().unwrap(), vec![ids[0], ids[1]]);

        // The log verifies, and a rebuild from it brings back the copy's books -- and only those.
        assert!(copy.backend.change_log().unwrap().verify().unwrap().is_clean());
        let rebuilt_dir = tempfile::tempdir().unwrap();
        let log_state = rebuilt_dir.path().join(LIBRARY_HANDLE_DIR_NAME);
        copy_dir(&state.join("snapshots"), &log_state.join("snapshots"));
        std::fs::create_dir_all(log_state.join("changes")).unwrap();
        std::fs::copy(state.join("install-id"), log_state.join("install-id")).unwrap();
        let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
        let replay = rebuilt.rebuild_from_change_log().unwrap();
        assert_eq!(replay.skipped_unknown_book, 0);
        let mut titles: Vec<String> = rebuilt.all_book_ids().unwrap().into_iter().map(|id| rebuilt.field_for(id, "title").unwrap().unwrap()).collect();
        titles.sort();
        assert_eq!(titles, ["Alpha", "Beta"]);
        let alpha = rebuilt.all_book_ids().unwrap().into_iter().find(|id| rebuilt.field_for(*id, "title").unwrap().as_deref() == Some("Alpha")).unwrap();
        assert_eq!(rebuilt.field_for(alpha, "rating").unwrap().as_deref(), Some("6"));

        // The copy continues the chain with its own edits.
        copy.set_field(ids[1], "rating", "4").unwrap();
        assert!(copy.backend.change_log().unwrap().verify().unwrap().is_clean());
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }

    #[test]
    fn refreshing_a_travel_copy_refuses_to_clobber_its_edited_database() {
        let (_dir, cache, _s, ids) = sample();
        let target = target_dir();
        let opts = travel_opts(MirrorScope::All);
        run(&cache, target.path(), &opts);

        // Edit on the laptop.
        {
            let copy = Cache::new(target.path()).unwrap();
            copy.set_field(ids[0], "rating", "10").unwrap();
        }
        // And in the library.
        cache.set_field(ids[1], "rating", "2").unwrap();

        let plan = plan(&cache, target.path().to_str().unwrap(), &opts).unwrap();
        assert!(!plan.copy_edits.is_empty(), "the copy's own log entries are noticed");
        let db = plan.items.iter().find(|i| i.rel == "metadata.db").unwrap();
        assert_eq!(db.action, Action::Conflict, "{:?}", db.reason);
        assert!(db.reason.as_deref().unwrap().contains("discard"));
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(report.conflicts.iter().any(|c| c.path == "metadata.db"));
        {
            let copy = Cache::new(target.path()).unwrap();
            assert_eq!(copy.field_for(ids[0], "rating").unwrap().as_deref(), Some("10"), "the edit survived the refresh");
        }

        // On request the edits are set aside (not deleted) and the library's state wins.
        let forced = MirrorOptions { overwrite_changed: true, ..opts };
        let (_, report) = run(&cache, target.path(), &forced);
        assert!(report.complete && report.edits_set_aside > 0, "{report:?}");
        let copy = Cache::new(target.path()).unwrap();
        assert_ne!(copy.field_for(ids[0], "rating").unwrap().as_deref(), Some("10"));
        assert_eq!(copy.field_for(ids[1], "rating").unwrap().as_deref(), Some("2"));
        assert!(target.path().join(MIRROR_TRASH_DIR).join(&report.run_id).exists());
    }

    /// Found on the trial library: the library changed a field and changed it
    /// back, so its database snapshot was byte-identical but its history grew,
    /// which gives the copy's baseline snapshot a new name. The refresh then
    /// added the new snapshot and removed the old one -- neither "replaces"
    /// metadata.db -- and left the copy's own edit chained to a baseline that
    /// was gone.
    #[test]
    fn a_new_history_baseline_waits_for_the_copys_own_edits_too() {
        let (_dir, cache, _s, ids) = sample();
        let target = target_dir();
        let opts = travel_opts(MirrorScope::All);
        run(&cache, target.path(), &opts);
        {
            let copy = Cache::new(target.path()).unwrap();
            copy.set_field(ids[0], "rating", "10").unwrap();
        }
        let before = cache.field_for(ids[1], "rating").unwrap();
        cache.set_field(ids[1], "rating", "2").unwrap();
        cache.set_field(ids[1], "rating", before.as_deref().unwrap_or("")).unwrap();

        let plan = plan(&cache, target.path().to_str().unwrap(), &opts).unwrap();
        assert!(!plan.copy_edits.is_empty());
        let snapshots: Vec<&PlanItem> = plan.items.iter().filter(|i| i.rel.contains("/snapshots/")).collect();
        assert!(!snapshots.is_empty());
        assert!(snapshots.iter().all(|i| matches!(i.action, Action::Conflict | Action::Unchanged)), "{snapshots:?}");
        assert!(plan.items.iter().all(|i| !(i.rel.contains("/snapshots/") && matches!(i.action, Action::CopyNew | Action::Remove | Action::Update))));
        execute(&cache, &plan, &JobControl::new()).unwrap();
        {
            let copy = Cache::new(target.path()).unwrap();
            assert_eq!(copy.field_for(ids[0], "rating").unwrap().as_deref(), Some("10"));
        }

        // Overwriting rewrites the database too, so edits, baseline and
        // database move together.
        let forced = MirrorOptions { overwrite_changed: true, ..opts };
        let (_, report) = run(&cache, target.path(), &forced);
        assert!(report.complete && report.edits_set_aside > 0, "{report:?}");
        let copy = Cache::new(target.path()).unwrap();
        assert_ne!(copy.field_for(ids[0], "rating").unwrap().as_deref(), Some("10"));
    }

    /// A refresh after any edit gives the copy's baseline snapshot a new name.
    /// Dropping the old one is internal housekeeping: not "to remove", no
    /// confirmation, and nothing parked in `.mirror-trash`.
    #[test]
    fn a_replaced_history_snapshot_is_not_a_removal_and_is_not_trashed() {
        let (_dir, cache, _s, ids) = sample();
        let target = target_dir();
        let opts = travel_opts(MirrorScope::All);
        run(&cache, target.path(), &opts);
        let before = cache.field_for(ids[1], "rating").unwrap();
        cache.set_field(ids[1], "rating", "2").unwrap();
        cache.set_field(ids[1], "rating", before.as_deref().unwrap_or("")).unwrap();

        let plan = plan(&cache, target.path().to_str().unwrap(), &opts).unwrap();
        let old: Vec<&PlanItem> = plan.items.iter().filter(|i| i.rel.contains("/snapshots/") && i.action == Action::Remove).collect();
        assert!(!old.is_empty(), "the old baseline is dropped: {:?}", plan.items.iter().map(|i| (&i.rel, i.action)).collect::<Vec<_>>());
        assert_eq!(plan.totals.remove.files, 0, "{:?}", plan.totals);
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!(report.removed, 0);
        assert!(report.trash.is_none(), "{report:?}");
        assert!(!target.path().join(MIRROR_TRASH_DIR).exists());
        assert!(old.iter().all(|i| !target.path().join(&i.rel).exists()), "the old snapshot is gone");
    }

    fn snapshot_rels(root: &Path) -> Vec<String> {
        let dir = root.join(LIBRARY_HANDLE_DIR_NAME).join("snapshots");
        let mut out: Vec<String> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
        out.sort();
        out
    }

    /// T29a #3: set-aside edits chain to the old snapshot, so an overwrite parks it with them
    /// rather than deleting it.
    #[test]
    fn an_overwrite_parks_the_old_history_snapshot_with_the_set_aside_edits() {
        let (_dir, cache, _s, ids) = sample();
        let target = target_dir();
        let opts = travel_opts(MirrorScope::All);
        run(&cache, target.path(), &opts);
        let old = snapshot_rels(target.path());
        assert!(!old.is_empty());
        {
            let copy = Cache::new(target.path()).unwrap();
            copy.set_field(ids[0], "rating", "10").unwrap();
        }
        let before = cache.field_for(ids[1], "rating").unwrap();
        cache.set_field(ids[1], "rating", "2").unwrap();
        cache.set_field(ids[1], "rating", before.as_deref().unwrap_or("")).unwrap();

        let forced = MirrorOptions { overwrite_changed: true, ..opts };
        let (_, report) = run(&cache, target.path(), &forced);
        assert!(report.complete && report.edits_set_aside > 0, "{report:?}");
        let now = snapshot_rels(target.path());
        let gone: Vec<&String> = old.iter().filter(|n| !now.contains(n)).collect();
        assert!(!gone.is_empty(), "the history changed, so the baseline was renamed: {old:?} -> {now:?}");
        let parked = target.path().join(MIRROR_TRASH_DIR).join(&report.run_id).join(LIBRARY_HANDLE_DIR_NAME).join("snapshots");
        for name in gone {
            assert!(parked.join(name).is_file(), "{name} was deleted instead of parked next to the edits it anchors");
        }
        assert!(report.trash.is_some());
    }

    /// T29a #3: a new snapshot that failed to copy must not cost the copy its old one.
    #[test]
    fn a_failed_snapshot_copy_leaves_the_old_snapshot_in_place() {
        let (_dir, cache, _s, ids) = sample();
        let target = target_dir();
        let opts = travel_opts(MirrorScope::All);
        run(&cache, target.path(), &opts);
        let old = snapshot_rels(target.path());
        let before = cache.field_for(ids[1], "rating").unwrap();
        cache.set_field(ids[1], "rating", "2").unwrap();
        cache.set_field(ids[1], "rating", before.as_deref().unwrap_or("")).unwrap();

        let plan = plan(&cache, target.path().to_str().unwrap(), &opts).unwrap();
        let fresh: Vec<&PlanItem> = plan.items.iter().filter(|i| i.rel.contains("/snapshots/") && i.action == Action::CopyNew).collect();
        assert!(!fresh.is_empty());
        // Something the new snapshot cannot be written over (as a full disk would stop it).
        for item in &fresh {
            std::fs::create_dir_all(target.path().join(&item.rel)).unwrap();
        }
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(!report.failures.is_empty(), "{report:?}");
        assert!(!report.complete);
        for name in &old {
            assert!(target.path().join(LIBRARY_HANDLE_DIR_NAME).join("snapshots").join(name).is_file(), "{name} was deleted though its replacement never landed");
        }
    }

    #[test]
    fn a_leftover_pre_rename_state_folder_is_never_copied() {
        assert!(is_excluded(".calibre-oxide/snapshots/1.jsonl"));
        assert!(is_excluded(".calibre-oxide"));
        assert!(!is_excluded(".lexmechanic/covers/a.jpg"));
    }

    #[test]
    fn a_cancelled_run_resumes_without_recopying_finished_files() {
        let (_dir, cache, scratch, _ids) = sample();
        for n in 0..6 {
            add(&cache, scratch.path(), &format!("Extra {n}"), "Zed", &format!("body {n}"));
        }
        let target = target_dir();
        let first = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        let control = JobControl::new();
        let stopper = control.clone();
        let report = execute_inner(&cache, &first, &control, 1, Some(&move |done| {
            if done == 4 {
                stopper.cancel();
            }
        }), 1)
        .unwrap();
        assert!(report.cancelled && !report.complete);
        assert_eq!(report.copied, 4);
        let manifest = read_manifest(target.path()).unwrap().unwrap();
        assert!(!manifest.complete);
        assert_eq!(manifest.files.len(), 4, "progress is recorded");
        let written: Vec<(String, std::time::SystemTime)> = manifest.files.iter().map(|f| (f.path.clone(), std::fs::metadata(target.path().join(&f.path)).unwrap().modified().unwrap())).collect();

        let second = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(second.totals.unchanged.files, 4);
        assert_eq!(second.totals.copy_new.files, first.totals.copy_new.files - 4);
        let report = execute(&cache, &second, &JobControl::new()).unwrap();
        assert_eq!(report.copied, first.totals.copy_new.files - 4, "only what was left");
        assert!(report.complete);
        for (path, mtime) in written {
            assert_eq!(std::fs::metadata(target.path().join(&path)).unwrap().modified().unwrap(), mtime, "{path} was not rewritten");
        }
        assert!(read_manifest(target.path()).unwrap().unwrap().complete);
    }

    #[test]
    fn the_list_of_copy_targets_does_not_make_the_database_look_changed() {
        let (_dir, cache, _s, _ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        // What the server does after every run.
        cache.backend.set_pref(TARGETS_PREF, &serde_json::json!([{"target": "somewhere"}])).unwrap();
        let again = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        let changed: Vec<_> = again.items.iter().filter(|i| i.action != Action::Unchanged).map(|i| &i.rel).collect();
        assert!(changed.is_empty(), "{changed:?}");
        let copy_db = rusqlite::Connection::open(target.path().join("metadata.db")).unwrap();
        let n: i64 = copy_db.query_row("SELECT COUNT(*) FROM preferences WHERE key = ?1", [TARGETS_PREF], |r| r.get(0)).unwrap();
        assert_eq!(n, 0, "the copy does not carry the list");
    }

    #[test]
    fn a_file_finished_but_missing_from_the_manifest_is_adopted() {
        let (_dir, cache, _s, _ids) = sample();
        let target = target_dir();
        run(&cache, target.path(), &mirror_opts());
        // Simulate a crash after a file landed and before the manifest was saved.
        let mut manifest = read_manifest(target.path()).unwrap().unwrap();
        let victim = manifest.files.iter().find(|f| f.kind == Kind::Format).unwrap().path.clone();
        manifest.files.retain(|f| f.path != victim);
        write_manifest(target.path(), &manifest).unwrap();
        let plan = plan(&cache, target.path().to_str().unwrap(), &mirror_opts()).unwrap();
        assert_eq!(plan.items.iter().find(|i| i.rel == victim).unwrap().action, Action::Unchanged);
        execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(read_manifest(target.path()).unwrap().unwrap().files.iter().any(|f| f.path == victim));
    }

    #[test]
    fn a_rebuilt_checksum_filter_keeps_only_the_chosen_books() {
        let (dir, cache, _s, ids) = sample();
        let _ = cache.checksums().record_hash(ids[0], "format", "TXT", "aaaa", 1);
        let _ = cache.checksums().record_hash(ids[1], "format", "TXT", "bbbb", 1);
        let target = target_dir();
        let (_, report) = run(&cache, target.path(), &travel_opts(MirrorScope::BookIds(vec![ids[0]])));
        assert!(report.complete, "{report:?}");
        let conn = rusqlite::Connection::open(target.path().join(LIBRARY_HANDLE_DIR_NAME).join(CHECKSUMS_DB_NAME)).unwrap();
        let books: Vec<i32> = conn.prepare("SELECT DISTINCT book_id FROM file_checksums").unwrap().query_map([], |r| r.get(0)).unwrap().map(|r| r.unwrap()).collect();
        assert_eq!(books, vec![ids[0]]);
        // The cover of the book that did not make it is not in the copy either.
        let covers = rel_files(&target.path().join(LIBRARY_HANDLE_DIR_NAME).join("covers"));
        assert!(covers.len() <= 1, "{covers:?}");
        drop(dir);
    }

    #[test]
    fn a_manifest_path_outside_the_copy_is_refused() {
        for bad in ["../x.pdf", "a/../../x.pdf", "/abs.pdf", "C:/x.pdf", "a\\..\\x","", "a//b", "./a"] {
            assert!(!super::is_plain_rel(bad), "{bad:?} should be refused");
        }
        assert!(super::is_plain_rel("Author/Title (1)/book.pdf"));
        let target = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "version": 1, "source_id": "s", "mode": "mirror", "run_id": "r", "written_at_ms": 0,
            "scope": "all", "complete": true,
            "files": [{"path": "../outside.pdf", "size": 1, "mtime_ms": 0, "blake3": "x", "kind": "format"}]
        });
        std::fs::write(target.path().join(super::MANIFEST_NAME), manifest.to_string()).unwrap();
        let err = super::read_manifest(target.path()).unwrap_err().to_string();
        assert!(err.contains("outside the copy"), "{err}");
    }

}
