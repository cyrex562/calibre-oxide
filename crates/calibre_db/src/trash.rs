//! The library's own trash: `.caltrash` (port of calibre 6.x's
//! `backend.py` trash, `cache.py`'s `move_book_from_trash` /
//! `move_format_from_trash` / `expire_old_trash`, and `gui2/trash.py`).
//!
//! # Why this exists
//!
//! Deleting used to mean "send the file to the OS wastebasket". Windows has
//! no Recycle Bin for a file on an SMB share, and neither do most NAS mounts
//! on Linux, so on exactly the library this app is meant for, `trash::delete`
//! failed and delete (and merge, which deletes its sources) left the file
//! where it was. calibre never had the problem: since 6.0 a library delete
//! moves the book into `.caltrash` inside the library -- the OS bin is not
//! involved -- and that is what this ports.
//!
//! # calibre's semantics, as ported
//!
//! * Books go to `.caltrash/b/<book id>/`, formats removed from a book to
//!   `.caltrash/f/<book id>/<ext>` plus a `metadata.json` (title, authors).
//! * Entries expire after `expire_old_trash_after` seconds -- a library
//!   preference, default 14 days, never less than one day -- and are
//!   removed for good. calibre checks at most hourly; here
//!   [`Cache::expire_trash_if_due`] does.
//! * An entry can be listed, restored, deleted for good, and the whole trash
//!   emptied.
//! * Moves are renames inside the library folder, never copy-and-delete.
//!
//! # Where this deviates, and why
//!
//! calibre moves a book's **whole folder**: in its layout the folder is the
//! book's alone. Here a folder may be shared (a tracked folder of loose
//! files, or the library root), so only the book's *own* files move -- the
//! `data` rows' files, plus `cover.jpg`/`metadata.opf` when the folder is
//! dedicated -- exactly the set `delete_book_with` has always touched, and
//! siblings and unrelated files are never moved. Consequences:
//!
//! * The entry holds the files flat under their own names, and an
//!   `oxide-trash.json` manifest with what calibre gets from `metadata.opf`
//!   (title, authors, ...) **and** where each file came from, relative to the
//!   library root. That is what lets a trashed in-place file be restored to
//!   its original path. calibre can read the files of a dedicated-folder
//!   book (they are laid out as it expects), but the manifest is what this
//!   app restores from.
//! * Restoring puts the file back at its original relative path. If
//!   something else has taken that name since, it is restored beside it as
//!   `name (restored).ext` rather than over it.
//! * A restored book keeps its **uuid** (so the change log and the ignore
//!   list agree about it) and its id when that id is free, a new id
//!   otherwise. calibre refuses when the id exists.
//! * The sidecar cover (`.calibre-oxide/covers/<uuid>.jpg`) travels with the
//!   book as `oxide-cover.jpg`; calibre's is `cover.jpg` in the folder.
//! * Entry expiry is by the manifest's `deleted_at`, not the directory's
//!   mtime, which a restore of a neighbour or a backup tool can change.
//!
//! # Interaction with the ignore list and the scanner
//!
//! A trashed file is gone from its folder, so a rescan cannot re-add it, and
//! trashing records **no** ignore entry. (A file that could *not* be trashed
//! stays and is ignored, as before.) The scanner does not walk `.caltrash`.
//! Restoring clears any ignore entry for the file's paths, so the book is
//! never both present and hidden.
//!
//! # The change log
//!
//! A trash is a [`ChangeOp::BookRemoved`](crate::change_log::ChangeOp)
//! and a format trash a `FormatRemoved`; a restore is the same ops as
//! adding the book (`BookAdded`, its fields, its path, a `FormatSet` per
//! file), under the **same uuid**. So a rebuild from the log lands on the
//! right state for any sequence of trash and restore, and needs nothing
//! from `.caltrash` itself -- which stays on disk and is listed afterwards.
//! Replay also drops any ignore entry that names a file the rebuilt
//! library tracks (see `change_log::apply`).
//!
//! # Crash safety
//!
//! Every move is [`LibraryHandle::rename_atomic`](crate::library_handle::LibraryHandle::rename_atomic),
//! so it is journaled and, on a network share, retried. The manifest is
//! written *before* the first file moves, so a crash mid-move leaves an
//! entry that says what it was.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::{path_is_plain, Cache, OwnFile};
use crate::constants::TRASH_DIR_NAME;

/// The preference calibre keeps the expiry in, in seconds.
pub const EXPIRE_PREF: &str = "expire_old_trash_after";
/// calibre's `DEFAULT_TRASH_EXPIRY_TIME_SECONDS`.
pub const DEFAULT_EXPIRY_SECONDS: i64 = 14 * 86400;
/// calibre never expires sooner than a day, whatever the preference says.
pub const MIN_EXPIRY_SECONDS: i64 = 86400;
const BOOK_MANIFEST: &str = "oxide-trash.json";
const FORMAT_MANIFEST: &str = "metadata.json";
const STORED_COVER: &str = "oxide-cover.jpg";

/// Standard fields a restore writes back, in the order it writes them.
const RESTORED_FIELDS: &[&str] = &[
    "sort", "author_sort", "series", "series_index", "publisher", "rating", "tags", "languages", "comments", "identifiers", "timestamp", "pubdate",
];

pub(crate) fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct TrashedFile {
    /// Where it was, relative to the library root, `/`-separated.
    rel: String,
    /// Its name inside the entry's directory.
    stored: String,
    /// Upper-case format for a book file; `None` for `cover.jpg` and
    /// `metadata.opf`.
    format: Option<String>,
    /// `data.name`: the file's stem.
    name: Option<String>,
    size: i64,
    hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BookManifest {
    version: u32,
    book_id: i32,
    uuid: String,
    deleted_at: i64,
    title: String,
    authors: Vec<String>,
    /// `books.path` at deletion time.
    path: String,
    fields: BTreeMap<String, String>,
    custom: BTreeMap<String, String>,
    files: Vec<TrashedFile>,
    has_cover: bool,
    /// Whether the sidecar cover travelled with the entry.
    #[serde(default)]
    stored_cover: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FormatMeta {
    name: String,
    /// The folder the file was in, relative to the library root.
    rel_dir: String,
    size: i64,
    hash: Option<String>,
    deleted_at: i64,
}

/// `f/<id>/metadata.json`: calibre's `title`/`authors`, plus what restoring
/// to the right place needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct FormatManifest {
    title: String,
    authors: Vec<String>,
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    book_id: i32,
    #[serde(default)]
    formats: BTreeMap<String, FormatMeta>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrashKind {
    Book,
    Format,
}

impl TrashKind {
    pub fn category(self) -> &'static str {
        match self {
            TrashKind::Book => "b",
            TrashKind::Format => "f",
        }
    }

    pub fn parse(s: &str) -> Option<TrashKind> {
        match s {
            "book" | "b" => Some(TrashKind::Book),
            "format" | "f" => Some(TrashKind::Format),
            _ => None,
        }
    }
}

/// One thing in the trash, as the listing shows it.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TrashEntry {
    pub kind: TrashKind,
    /// Names the entry for restore and delete. Usually the book id.
    pub key: String,
    pub book_id: i32,
    pub title: String,
    pub authors: String,
    /// Upper-case formats the entry holds.
    pub formats: Vec<String>,
    /// Bytes of book files.
    pub size: i64,
    /// Unix seconds.
    pub deleted_at: i64,
    /// Unix seconds; when expiry will remove it.
    pub expires_at: i64,
    /// For a book: its folder at deletion time; for a format: the folder it was in.
    pub original_path: String,
    /// Why this entry cannot be restored, if it cannot.
    pub problem: Option<String>,
}

/// What a book moved into the trash did.
#[derive(Debug, Default)]
pub(crate) struct MoveOutcome {
    pub moved: Vec<String>,
    pub failed: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestoredBook {
    pub book_id: i32,
    pub uuid: String,
    pub title: String,
    /// Files put back, relative to the library root.
    pub restored: Vec<String>,
    /// `(original, where it went)` for files whose name had been taken.
    pub renamed: Vec<(String, String)>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RestoredFormat {
    pub book_id: i32,
    pub format: String,
    pub path: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct ExpireReport {
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
}

fn plain_name(name: &str) -> bool {
    !name.is_empty() && !name.contains(['/', '\\']) && name != "." && name != ".." && !name.contains(':')
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let raw = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&raw).with_context(|| format!("parsing {}", path.display()))
}

fn dir_mtime(path: &Path) -> i64 {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn file_size(path: &Path) -> i64 {
    fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0)
}

fn split_rel(rel: &str) -> (String, String) {
    match rel.rsplit_once('/') {
        Some((dir, file)) => (dir.to_string(), file.to_string()),
        None => (String::new(), rel.to_string()),
    }
}

impl Cache {
    /// `<library>/.caltrash`.
    pub fn trash_dir(&self) -> PathBuf {
        self.backend.library_path.join(TRASH_DIR_NAME)
    }

    fn ensure_trash_dir(&self) -> Result<PathBuf> {
        let dir = self.trash_dir();
        fs::create_dir_all(dir.join("b")).with_context(|| format!("creating {}", dir.display()))?;
        fs::create_dir_all(dir.join("f"))?;
        Ok(dir)
    }

    /// Renames `from` to `to` through the journaled handle. Refuses to
    /// replace anything (a plain rename silently would, on Windows) and
    /// never falls back to copy-and-delete: if the two are on different
    /// volumes this fails, and the file stays where it was.
    fn move_journaled(&self, from: &Path, to: &Path) -> Result<(), String> {
        #[cfg(test)]
        if crate::cache::FORCE_TRASH_FAILURE.with(|f| f.get()) {
            return Err("forced trash failure".to_string());
        }
        if to.exists() {
            return Err(format!("{} already exists", to.display()));
        }
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        let handle = self.backend.write_handle().map_err(|e| e.to_string())?;
        handle.rename_atomic(from, to).map_err(|e| e.to_string())
    }

    /// Everything a restore needs, read while the book still has its row.
    pub(crate) fn trash_manifest_for(&self, book_id: i32, uuid: Option<String>, title: &str, path_rel: &str, own: &[OwnFile]) -> Result<BookManifest> {
        let library = &self.backend.library_path;
        let mut fields = BTreeMap::new();
        for field in RESTORED_FIELDS {
            if let Some(value) = self.field_for(book_id, field)?.filter(|v| !v.is_empty()) {
                fields.insert((*field).to_string(), value);
            }
        }
        let mut custom = BTreeMap::new();
        let labels: Vec<(String, String)> = {
            let conn = self.backend.conn.lock().unwrap();
            crate::custom_columns::all_columns(&conn)?.into_iter().map(|c| (c.label, c.datatype)).collect()
        };
        for (label, datatype) in labels {
            let key = format!("#{label}");
            if let Some(value) = self.field_for(book_id, &key)?.filter(|v| !v.is_empty()) {
                custom.insert(key.clone(), value);
            }
            if datatype == "series" {
                if let Some(index) = self.get_custom_series_index(book_id, &label)? {
                    custom.insert(format!("#{label}_index"), index.to_string());
                }
            }
        }
        let authors: Vec<String> = self.author_names_for_book(book_id).unwrap_or_default();
        let files = own
            .iter()
            .map(|f| {
                let (_, stored) = split_rel(&f.rel);
                let (name, format) = if f.is_format {
                    let p = Path::new(&stored);
                    (p.file_stem().map(|s| s.to_string_lossy().to_string()), p.extension().map(|e| e.to_string_lossy().to_uppercase()))
                } else {
                    (None, None)
                };
                TrashedFile { rel: f.rel.clone(), stored, format, name, size: file_size(&library.join(&f.rel)), hash: f.hash.clone() }
            })
            .collect();
        Ok(BookManifest {
            version: 1,
            book_id,
            uuid: uuid.filter(|u| !u.is_empty()).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            deleted_at: now_secs(),
            title: title.to_string(),
            authors,
            path: path_rel.to_string(),
            fields,
            custom,
            files,
            has_cover: self.has_cover(book_id).unwrap_or(false),
            stored_cover: false,
        })
    }

    fn write_manifest<T: Serialize>(&self, path: &Path, manifest: &T) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        self.backend.write_handle()?.write_atomic(path, &bytes)?;
        Ok(())
    }

    /// Moves a deleted book's own files into `.caltrash/b/<id>/`. Called by
    /// `delete_book_with` after the row is gone. Files that fail to move are
    /// left where they are and reported; if nothing ends up in the trash no
    /// entry is kept (calibre too: nothing to restore from).
    pub(crate) fn move_book_to_trash(&self, mut manifest: BookManifest, sidecar_cover: Option<PathBuf>) -> MoveOutcome {
        let mut outcome = MoveOutcome::default();
        let library = self.backend.library_path.clone();
        let present: Vec<TrashedFile> = manifest.files.iter().filter(|f| library.join(&f.rel).is_file()).cloned().collect();
        if present.is_empty() {
            // Nothing on disk to keep. The sidecar cover is index state of
            // the deleted book; drop it as before.
            if let Some(cover) = sidecar_cover {
                if let Ok(handle) = self.backend.write_handle() {
                    let _ = handle.remove_atomic(&cover);
                }
            }
            return outcome;
        }

        let dir = match self.allocate_entry("b", manifest.book_id) {
            Ok((_, dir)) => dir,
            Err(e) => {
                for f in present {
                    outcome.failed.push((f.rel, format!("could not create the trash entry: {e:#}")));
                }
                return outcome;
            }
        };
        manifest.files = present;
        if let Err(e) = self.write_manifest(&dir.join(BOOK_MANIFEST), &manifest) {
            let _ = fs::remove_dir(&dir);
            for f in &manifest.files {
                outcome.failed.push((f.rel.clone(), format!("could not write the trash entry: {e:#}")));
            }
            return outcome;
        }

        let mut kept = Vec::new();
        for file in &manifest.files {
            match self.move_journaled(&library.join(&file.rel), &dir.join(&file.stored)) {
                Ok(()) => {
                    outcome.moved.push(file.rel.clone());
                    kept.push(file.clone());
                }
                Err(why) => outcome.failed.push((file.rel.clone(), why)),
            }
        }
        if let Some(cover) = sidecar_cover {
            // The cover is the book's own index state, but the restore wants
            // it back; a failure just means the restored book regenerates it.
            if self.move_journaled(&cover, &dir.join(STORED_COVER)).is_ok() {
                manifest.stored_cover = true;
            }
        }
        if kept.is_empty() {
            if let Ok(handle) = self.backend.write_handle() {
                let _ = handle.remove_atomic(&dir);
            }
        } else if !outcome.failed.is_empty() || manifest.stored_cover {
            manifest.files = kept;
            if let Err(e) = self.write_manifest(&dir.join(BOOK_MANIFEST), &manifest) {
                log::warn!("could not update the trash manifest in {}: {e:#}", dir.display());
            }
        }
        outcome
    }

    /// A fresh directory `.caltrash/<category>/<book id>[.n]`.
    fn allocate_entry(&self, category: &str, book_id: i32) -> Result<(String, PathBuf)> {
        let base = self.ensure_trash_dir()?.join(category);
        for n in 0..1000 {
            let key = if n == 0 { book_id.to_string() } else { format!("{book_id}.{n}") };
            let dir = base.join(&key);
            if !dir.exists() {
                fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
                return Ok((key, dir));
            }
        }
        bail!("no free trash slot for book {book_id}")
    }

    /// Moves one format file into `.caltrash/f/<book id>/<ext>`. Errors leave
    /// the file where it was.
    pub(crate) fn move_format_to_trash(&self, book_id: i32, fmt: &str, name: &str, path: &Path, hash: Option<String>) -> Result<()> {
        let base = self.ensure_trash_dir()?.join("f");
        let dir = base.join(book_id.to_string());
        fs::create_dir_all(&dir)?;
        let uuid = self.book_uuid(book_id)?.unwrap_or_default();
        let mut manifest: FormatManifest = read_json(&dir.join(FORMAT_MANIFEST)).ok().filter(|m: &FormatManifest| m.uuid == uuid).unwrap_or_else(|| FormatManifest {
            title: String::new(),
            authors: Vec::new(),
            uuid: uuid.clone(),
            book_id,
            formats: BTreeMap::new(),
        });
        manifest.title = self.field_for(book_id, "title")?.unwrap_or_default();
        manifest.authors = self.author_names_for_book(book_id).unwrap_or_default();
        let fmt_upper = fmt.to_uppercase();
        let rel_dir = self.field_for(book_id, "path")?.unwrap_or_default();
        manifest.formats.insert(fmt_upper.clone(), FormatMeta { name: name.to_string(), rel_dir, size: file_size(path), hash, deleted_at: now_secs() });

        let stored = dir.join(fmt.to_lowercase());
        let handle = self.backend.write_handle()?;
        if stored.exists() {
            // calibre overwrites: a second removal of the same format
            // supersedes the first.
            handle.remove_atomic(&stored)?;
        }
        self.write_manifest(&dir.join(FORMAT_MANIFEST), &manifest)?;
        if let Err(why) = self.move_journaled(path, &stored) {
            manifest.formats.remove(&fmt_upper);
            if manifest.formats.is_empty() {
                let _ = handle.remove_atomic(&dir);
            } else {
                let _ = self.write_manifest(&dir.join(FORMAT_MANIFEST), &manifest);
            }
            bail!("could not move {} into the trash: {why}", path.display());
        }
        Ok(())
    }

    // -----------------------------------------------------------------
    // Listing
    // -----------------------------------------------------------------

    /// Seconds after which an entry expires: the library's
    /// `expire_old_trash_after`, at least a day.
    pub fn trash_expiry_seconds(&self) -> i64 {
        let configured = self.get_preference(EXPIRE_PREF).ok().flatten().and_then(|v| v.trim().parse::<f64>().ok()).map(|v| v as i64);
        configured.unwrap_or(DEFAULT_EXPIRY_SECONDS).max(MIN_EXPIRY_SECONDS)
    }

    /// Everything in the trash, newest first.
    pub fn list_trash(&self) -> Result<Vec<TrashEntry>> {
        let root = self.trash_dir();
        let expiry = self.trash_expiry_seconds();
        let mut out = Vec::new();
        for kind in [TrashKind::Book, TrashKind::Format] {
            let Ok(read) = fs::read_dir(root.join(kind.category())) else { continue };
            for entry in read.flatten() {
                if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    continue;
                }
                let key = entry.file_name().to_string_lossy().to_string();
                let dir = entry.path();
                let listed = match kind {
                    TrashKind::Book => Self::describe_book_entry(&key, &dir, expiry),
                    TrashKind::Format => Self::describe_format_entry(&key, &dir, expiry),
                };
                if let Some(e) = listed {
                    out.push(e);
                }
            }
        }
        out.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at).then(a.key.cmp(&b.key)));
        Ok(out)
    }

    fn describe_book_entry(key: &str, dir: &Path, expiry: i64) -> Option<TrashEntry> {
        let book_id: i32 = key.split('.').next()?.parse().ok()?;
        match read_json::<BookManifest>(&dir.join(BOOK_MANIFEST)) {
            Ok(m) => Some(TrashEntry {
                kind: TrashKind::Book,
                key: key.to_string(),
                book_id: m.book_id,
                title: m.title.clone(),
                authors: m.authors.join(" & "),
                formats: m.files.iter().filter_map(|f| f.format.clone()).collect(),
                size: m.files.iter().filter(|f| f.format.is_some()).map(|f| f.size).sum(),
                deleted_at: m.deleted_at,
                expires_at: m.deleted_at + expiry,
                original_path: m.path,
                problem: None,
            }),
            Err(_) => {
                // Something calibre put there, or a half-written entry:
                // listed so it can be deleted, but not restorable here.
                let mut formats = Vec::new();
                let mut size = 0;
                for f in fs::read_dir(dir).ok()?.flatten() {
                    let name = f.file_name().to_string_lossy().to_string();
                    if name == "cover.jpg" || name == "metadata.opf" {
                        continue;
                    }
                    if let Some(ext) = Path::new(&name).extension() {
                        formats.push(ext.to_string_lossy().to_uppercase());
                        size += file_size(&f.path());
                    }
                }
                let at = dir_mtime(dir);
                Some(TrashEntry {
                    kind: TrashKind::Book,
                    key: key.to_string(),
                    book_id,
                    title: "(unknown book)".to_string(),
                    authors: String::new(),
                    formats,
                    size,
                    deleted_at: at,
                    expires_at: at + expiry,
                    original_path: String::new(),
                    problem: Some("this entry has no oxide-trash.json, so it cannot be restored here".to_string()),
                })
            }
        }
    }

    fn describe_format_entry(key: &str, dir: &Path, expiry: i64) -> Option<TrashEntry> {
        let book_id: i32 = key.parse().ok()?;
        let m: FormatManifest = read_json(&dir.join(FORMAT_MANIFEST)).ok()?;
        if m.formats.is_empty() {
            return None;
        }
        let deleted_at = m.formats.values().map(|f| f.deleted_at).max().unwrap_or_else(|| dir_mtime(dir));
        Some(TrashEntry {
            kind: TrashKind::Format,
            key: key.to_string(),
            book_id,
            title: if m.title.is_empty() { "(unknown book)".to_string() } else { m.title },
            authors: m.authors.join(" & "),
            formats: m.formats.keys().cloned().collect(),
            size: m.formats.values().map(|f| f.size).sum(),
            deleted_at,
            expires_at: deleted_at + expiry,
            original_path: m.formats.values().next().map(|f| f.rel_dir.clone()).unwrap_or_default(),
            problem: None,
        })
    }

    fn entry_dir(&self, kind: TrashKind, key: &str) -> Result<PathBuf> {
        if !plain_name(key) {
            bail!("{key:?} is not a trash entry");
        }
        let dir = self.trash_dir().join(kind.category()).join(key);
        if !dir.is_dir() {
            bail!("there is no {} {key} in the trash", if kind == TrashKind::Book { "book" } else { "format entry" });
        }
        Ok(dir)
    }

    // -----------------------------------------------------------------
    // Restore
    // -----------------------------------------------------------------

    /// Puts a trashed book back: same uuid, its id if free, every file at its
    /// original relative path (or beside it, if the name was taken).
    pub fn restore_trashed_book(&self, key: &str) -> Result<RestoredBook> {
        let dir = self.entry_dir(TrashKind::Book, key)?;
        let manifest: BookManifest = read_json(&dir.join(BOOK_MANIFEST)).context("this trash entry cannot be restored by this app (no oxide-trash.json)")?;
        if self.book_id_for_uuid(&manifest.uuid)?.is_some() {
            bail!("\"{}\" is already in the library", manifest.title);
        }
        // The manifest is data inside the library folder, so it is not
        // trusted to name paths outside it.
        if !manifest.path.is_empty() && !path_is_plain(&manifest.path) {
            bail!("the trash entry names an unusable folder {:?}", manifest.path);
        }
        for f in &manifest.files {
            if !path_is_plain(&f.rel) || !plain_name(&f.stored) {
                bail!("the trash entry names an unusable file {:?}", f.rel);
            }
            if !dir.join(&f.stored).is_file() {
                bail!("{} is missing from the trash entry", f.stored);
            }
        }
        let library = self.backend.library_path.clone();

        // Choose every destination before touching anything.
        struct Plan {
            file: TrashedFile,
            dest_rel: String,
            stem: Option<String>,
        }
        let mut plans: Vec<Plan> = Vec::new();
        let mut renamed = Vec::new();
        for file in &manifest.files {
            let (folder, name) = split_rel(&file.rel);
            let mut dest_rel = file.rel.clone();
            let mut stem = file.name.clone();
            if library.join(&dest_rel).exists() {
                if file.format.is_none() {
                    continue; // cover.jpg / metadata.opf already replaced by something else: leave it
                }
                let ext = Path::new(&name).extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
                let base = file.name.clone().unwrap_or_default();
                let mut n = 0;
                loop {
                    let candidate = if n == 0 { format!("{base} (restored)") } else { format!("{base} (restored {n})") };
                    let rel = if folder.is_empty() { format!("{candidate}.{ext}") } else { format!("{folder}/{candidate}.{ext}") };
                    if !library.join(&rel).exists() && !plans.iter().any(|p| p.dest_rel == rel) {
                        renamed.push((file.rel.clone(), rel.clone()));
                        dest_rel = rel;
                        stem = Some(candidate);
                        break;
                    }
                    n += 1;
                    if n > 1000 {
                        bail!("no free name to restore {} under", file.rel);
                    }
                }
            }
            plans.push(Plan { file: file.clone(), dest_rel, stem });
        }

        // The row, with the id it had if that is free. These go to the log
        // exactly as an ordinary add does, under the original uuid.
        let mut mi = calibre_ebooks::metadata::MetaInformation::default();
        mi.title = manifest.title.clone();
        mi.authors = manifest.authors.clone();
        mi.uuid = Some(manifest.uuid.clone());
        let book_id = self.add_book_db_entry_with_id(&mi, &manifest.path, Some(manifest.book_id))?;

        let fail = |this: &Cache, moved: &[(PathBuf, PathBuf)], e: anyhow::Error| -> anyhow::Error {
            // Undo: files back into the entry, the row away again.
            for (from, to) in moved.iter().rev() {
                let _ = this.move_journaled(to, from);
            }
            let _ = this.delete_book_with(book_id, crate::cache::BookFiles::Keep);
            e
        };

        let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new(); // (in the trash, restored)
        for plan in &plans {
            let from = dir.join(&plan.file.stored);
            let to = library.join(&plan.dest_rel);
            if let Err(why) = self.move_journaled(&from, &to) {
                return Err(fail(self, &moved, anyhow::anyhow!("could not restore {}: {why}", plan.dest_rel)));
            }
            moved.push((from, to));
        }

        // Metadata. Each is an ordinary logged write.
        let mut set = |field: &str, value: &str| {
            if let Err(e) = self.set_field(book_id, field, value) {
                log::warn!("restoring \"{}\": could not set {field}: {e:#}", manifest.title);
            }
        };
        for field in RESTORED_FIELDS {
            if let Some(value) = manifest.fields.get(*field) {
                set(field, value);
            }
        }
        for (label, value) in &manifest.custom {
            if let Err(e) = self.set_custom_column_value(book_id, label, value) {
                log::warn!("restoring \"{}\": could not set {label}: {e:#}", manifest.title);
            }
        }

        let mut restored = Vec::new();
        for plan in &plans {
            if let (Some(format), Some(stem)) = (&plan.file.format, &plan.stem) {
                if let Err(e) = self.record_format_row(book_id, format, stem, plan.file.size, plan.file.hash.as_deref()) {
                    return Err(fail(self, &moved, e));
                }
            }
            restored.push(plan.dest_rel.clone());
            // A restored file must never be skipped by the scanner.
            let _ = self.ignored().unignore(&plan.file.rel);
            let _ = self.ignored().unignore(&plan.dest_rel);
        }

        if manifest.has_cover {
            let sidecar = dir.join(STORED_COVER);
            let bytes = if manifest.stored_cover { fs::read(&sidecar).ok() } else { fs::read(library.join(&manifest.path).join("cover.jpg")).ok() };
            if let Some(bytes) = bytes.filter(|b| !b.is_empty()) {
                if let Err(e) = crate::covers::set_cover(self, book_id, &bytes) {
                    log::warn!("restoring \"{}\": could not put its cover back: {e:#}", manifest.title);
                }
            }
        }

        if let Ok(handle) = self.backend.write_handle() {
            if let Err(e) = handle.remove_atomic(&dir) {
                log::warn!("restored \"{}\" but could not remove its trash entry: {e}", manifest.title);
            }
        }
        Ok(RestoredBook { book_id, uuid: manifest.uuid, title: manifest.title, restored, renamed })
    }

    /// Puts a trashed format back on the book it came from.
    pub fn restore_trashed_format(&self, key: &str, fmt: &str) -> Result<RestoredFormat> {
        let dir = self.entry_dir(TrashKind::Format, key)?;
        let mut manifest: FormatManifest = read_json(&dir.join(FORMAT_MANIFEST)).context("this trash entry has no readable metadata.json")?;
        let fmt_upper = fmt.to_uppercase();
        let Some(meta) = manifest.formats.get(&fmt_upper).cloned() else {
            bail!("the trash holds no {fmt_upper} for that book");
        };
        let stored = dir.join(fmt_upper.to_lowercase());
        if !stored.is_file() {
            bail!("the {fmt_upper} file is missing from the trash");
        }
        // The book: by uuid (the id may have changed under a rebuild).
        let book_id = match self.book_id_for_uuid(&manifest.uuid)? {
            Some(id) => id,
            None if manifest.uuid.is_empty() => {
                let id = manifest.book_id;
                if self.book_uuid(id)?.is_none() {
                    bail!("the book this format came from is no longer in the library");
                }
                id
            }
            None => bail!("the book this format came from is no longer in the library (if it is in the trash, restore it first)"),
        };
        let have: Vec<(String, String)> = self.format_file_names(book_id)?;
        if have.iter().any(|(f, _)| f.eq_ignore_ascii_case(&fmt_upper)) {
            bail!("the book already has a {fmt_upper}; remove it before restoring this one");
        }
        let book_dir = self.book_dir(book_id)?.context("the book has no folder")?;
        let library = self.backend.library_path.clone();
        let folder = self.field_for(book_id, "path")?.unwrap_or_default();
        if !meta.name.is_empty() && !plain_name(&meta.name) {
            bail!("the trash entry names an unusable file {:?}", meta.name);
        }
        let ext = fmt_upper.to_lowercase();
        let mut stem = meta.name.clone();
        let mut n = 0;
        while book_dir.join(format!("{stem}.{ext}")).exists() {
            n += 1;
            stem = if n == 1 { format!("{} (restored)", meta.name) } else { format!("{} (restored {})", meta.name, n - 1) };
            if n > 1000 {
                bail!("no free name to restore {} under", meta.name);
            }
        }
        let dest = book_dir.join(format!("{stem}.{ext}"));
        if let Err(why) = self.move_journaled(&stored, &dest) {
            bail!("could not restore the {fmt_upper}: {why}");
        }
        if let Err(e) = self.record_format_row(book_id, &fmt_upper, &stem, meta.size, meta.hash.as_deref()) {
            let _ = self.move_journaled(&dest, &stored);
            return Err(e);
        }
        let rel = if folder.is_empty() { format!("{stem}.{ext}") } else { format!("{folder}/{stem}.{ext}") };
        let _ = self.ignored().unignore(&rel);
        let _ = library; // the library root is implied by `book_dir`

        manifest.formats.remove(&fmt_upper);
        if let Ok(handle) = self.backend.write_handle() {
            if manifest.formats.is_empty() {
                let _ = handle.remove_atomic(&dir);
            } else if let Err(e) = self.write_manifest(&dir.join(FORMAT_MANIFEST), &manifest) {
                log::warn!("restored the {fmt_upper} but could not update its trash entry: {e:#}");
            }
        }
        Ok(RestoredFormat { book_id, format: fmt_upper, path: rel })
    }

    // -----------------------------------------------------------------
    // Delete for good, empty, expire
    // -----------------------------------------------------------------

    /// Permanently removes one trash entry (calibre's `delete_trash_entry`).
    /// For a format entry, `fmt` removes just that format; `None` the lot.
    pub fn delete_trash_entry(&self, kind: TrashKind, key: &str, fmt: Option<&str>) -> Result<()> {
        let dir = self.entry_dir(kind, key)?;
        let handle = self.backend.write_handle()?;
        if let (TrashKind::Format, Some(fmt)) = (kind, fmt) {
            let mut manifest: FormatManifest = read_json(&dir.join(FORMAT_MANIFEST))?;
            let fmt_upper = fmt.to_uppercase();
            if manifest.formats.remove(&fmt_upper).is_some() && !manifest.formats.is_empty() {
                self.write_manifest(&dir.join(FORMAT_MANIFEST), &manifest)?;
                handle.remove_atomic(&dir.join(fmt_upper.to_lowercase()))?;
                return Ok(());
            }
        }
        handle.remove_atomic(&dir)?;
        Ok(())
    }

    /// Removes everything in the trash. Returns how many entries went.
    pub fn empty_trash(&self) -> Result<usize> {
        let mut n = 0;
        for entry in self.list_trash()? {
            self.delete_trash_entry(entry.kind, &entry.key, None)?;
            n += 1;
        }
        // Unlisted junk (calibre-made or half-written directories) too.
        let handle = self.backend.write_handle()?;
        for category in ["b", "f"] {
            if let Ok(read) = fs::read_dir(self.trash_dir().join(category)) {
                for e in read.flatten() {
                    if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        handle.remove_atomic(&e.path())?;
                        n += 1;
                    }
                }
            }
        }
        Ok(n)
    }

    /// Removes entries deleted more than the library's expiry ago.
    pub fn expire_old_trash(&self) -> Result<ExpireReport> {
        self.expire_old_trash_at(now_secs(), self.trash_expiry_seconds())
    }

    /// [`Cache::expire_old_trash`] with the clock and the age given, for tests.
    pub fn expire_old_trash_at(&self, now: i64, max_age: i64) -> Result<ExpireReport> {
        let mut report = ExpireReport::default();
        if !self.trash_dir().is_dir() {
            return Ok(report);
        }
        for entry in self.list_trash()? {
            if entry.deleted_at + max_age <= now {
                match self.delete_trash_entry(entry.kind, &entry.key, None) {
                    Ok(()) => report.removed.push(format!("{}/{}", entry.kind.category(), entry.key)),
                    Err(e) => report.failed.push((format!("{}/{}", entry.kind.category(), entry.key), format!("{e:#}"))),
                }
            }
        }
        Ok(report)
    }

    /// [`Cache::expire_old_trash`], at most once an hour per library for the
    /// life of the process (calibre's `last_expired_trash_at`), and never an
    /// error: it runs on open, and an unreadable trash must not stop that.
    pub fn expire_trash_if_due(&self) -> Option<ExpireReport> {
        static LAST: OnceLock<Mutex<std::collections::HashMap<PathBuf, i64>>> = OnceLock::new();
        let last = LAST.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
        let now = now_secs();
        {
            let mut map = last.lock().unwrap();
            if map.get(&self.backend.library_path).is_some_and(|t| now - *t < 3600) {
                return None;
            }
            map.insert(self.backend.library_path.clone(), now);
        }
        match self.expire_old_trash() {
            Ok(report) => Some(report),
            Err(e) => {
                log::warn!("could not expire the trash: {e:#}");
                None
            }
        }
    }
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
