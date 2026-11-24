//! Organize a library's files by a template (tranche 14b).
//!
//! Books scanned in place keep the paths the user gave them. This moves each
//! book's **own format files** to where a calibre save-to-disk style template
//! says (default `{author_sort}/{title}`: folder `Author`, file `Title.ext`),
//! inside the library.
//!
//! # Safety rules
//!
//! - **A dry run comes first.** [`plan`] changes nothing and returns every move
//!   (from -> to), what was skipped and a fingerprint of the plan;
//!   [`execute`] takes that plan and refuses (the route re-plans and compares)
//!   if the library no longer produces the same fingerprint.
//! - **Never overwrite.** A target that exists, or that another book in the
//!   plan takes, gets ` (2)`, ` (3)`, ... on the file name.
//! - **Only the book's own files.** In a tracked library many books share one
//!   folder (the delete-safety rule of tranche 1); nothing else in a folder is
//!   touched, and no folder is moved as a whole. `cover.jpg` / `metadata.opf`
//!   sidecars are left where they are (covers live under `.lexmechanic`);
//!   an old folder is removed only once it is empty (`remove_dir`, never
//!   recursive). A book's attached files (`<folder>/data`, or `data/<uuid>` for
//!   a book in the library root) move with it when the folder is the book's
//!   alone and so is its destination (see [`plan_data_dir`]); when it is not
//!   (another book is in either folder, or another book of the same plan moves
//!   into the same destination), the files stay where they are and the plan
//!   carries a warning for each book that leaves some behind.
//! - **Journaled.** Each rename is `LibraryHandle::rename_atomic` (or its
//!   `_no_replace` form: the target is never replaced, even by a file that
//!   appears after the check). The database follows through
//!   `Cache::set_book_location`, which writes the folder and every format's
//!   name in one transaction and one change-log entry, so a rebuild reproduces
//!   the result and a failure leaves no half-written names.
//! - **One book at a time, all or nothing.** If a rename of one book's files
//!   fails (a different volume behind a junction, a locked file), the files of
//!   that book already moved go back and the book is reported as failed; nothing
//!   is copied across volumes.
//! - **Move to a folder.** [`plan_to`] with [`Target::Folder`] is the same plan
//!   and the same executor with an explicit destination folder instead of a
//!   template: every file keeps its name (a taken name still gets ` (2)`), the
//!   folder is created if missing, and the run is recorded and undone like any
//!   other (`POST /move-books`, `POST /unorganize-library`). The folder is
//!   checked by [`validate_folder`]: relative to the library, no `..`, nothing
//!   reserved, no symlink on the way out.
//! - **Undoable.** Every run writes `.lexmechanic/organize-runs/<run_id>.jsonl`
//!   with each book's old and new place; [`undo`] reverses it, skipping books
//!   changed since. The journal is append-only and each book's entry is on disk
//!   *before* its files move (see [`crate::run_journal`]), so a crash leaves no
//!   move without an undo entry.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use lexmechanic_utils::filenames::sanitize_file_name;
use lexmechanic_utils::formatter::string_format;

use crate::cache::Cache;
use crate::formatter_functions::{CacheCatalog, CacheFunctions, CacheValueSource};
use crate::scan_pipeline::JobControl;

pub const DEFAULT_TEMPLATE: &str = "{author_sort}/{title}";

/// Which books to organize.
#[derive(Debug, Clone)]
pub enum Scope {
    All,
    Ids(Vec<i32>),
    /// Books whose `path` is this folder or inside it (relative to the library).
    PathPrefix(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedFile {
    pub format: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedBook {
    pub book_id: i32,
    pub title: String,
    pub old_path: String,
    pub new_path: String,
    /// Format -> file stem, before and after.
    pub old_names: BTreeMap<String, String>,
    pub new_names: BTreeMap<String, String>,
    /// Only the files that actually move.
    pub files: Vec<PlannedFile>,
    /// The name had to be suffixed to avoid an existing or planned file.
    pub suffixed: bool,
    /// The book's own `data/` folder of attached files, moved with it (see
    /// [`plan_data_dir`]). Journaled with the book, so an undo moves it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PlannedDir>,
}

/// A folder that moves with a book: `from` and `to`, relative to the library.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlannedDir {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skipped {
    pub book_id: i32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizePlan {
    pub template: String,
    pub books: Vec<PlannedBook>,
    pub skipped: Vec<Skipped>,
    /// Books already where the template puts them.
    pub already_organized: usize,
    pub fingerprint: String,
    /// Things worth knowing about books that are still planned (a path so long
    /// Windows tools may refuse it).
    pub warnings: Vec<Skipped>,
    /// A folder rename that only changes case: the directory itself is renamed
    /// (through a temporary name) after the books move, because on a
    /// case-insensitive volume moving the files leaves the old spelling.
    pub case_fix_folder: Option<String>,
}

impl OrganizePlan {
    pub fn file_count(&self) -> usize {
        self.books.iter().map(|b| b.files.len()).sum()
    }
    pub fn conflicts(&self) -> usize {
        self.books.iter().filter(|b| b.suffixed).count()
    }
}

fn join_rel(dir: &str, file: &str) -> String {
    if dir.is_empty() {
        file.to_string()
    } else if file.is_empty() {
        dir.to_string()
    } else {
        format!("{dir}/{file}")
    }
}

/// Absolute paths longer than this get a warning in the plan: Windows tools
/// that predate long-path support stop at 260.
const MAX_COMFORTABLE_PATH: usize = 240;

fn clip(component: &str) -> String {
    component.chars().take(120).collect::<String>().trim().trim_end_matches('.').to_string()
}

/// The template's `/`-separated components for one book: sanitized, with empty
/// and `..` ones dropped, falling back to the book id.
fn components(cache: &Cache, book_id: i32, template: &str) -> Result<Vec<String>> {
    let values = CacheValueSource::new(cache, book_id)?;
    let functions = CacheFunctions::new(cache, book_id);
    let rendered = string_format::evaluate_template(template, &values, &CacheCatalog, &functions).map_err(|e| anyhow!("{e}"))?;
    // Each component gets the checks a typed-in folder gets (`validate_folder`):
    // a device name like CON or NUL is not creatable on Windows, so it is mapped
    // aside (`_CON`) the way calibre's save-to-disk does.
    let mut parts: Vec<String> = rendered
        .split(['/', '\\'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(sanitize_file_name)
        .map(|s| clip(&s))
        .map(|s| lexmechanic_utils::filenames::safe_component(&s))
        .filter(|s| !s.is_empty() && s != "..")
        .collect();
    if parts.is_empty() {
        parts.push(book_id.to_string());
    }
    Ok(parts)
}

/// What a template gives one book: its folder (`/`-separated, possibly empty)
/// and file stem. Empty path segments are dropped (a value that is missing
/// leaves no `//` and no empty folder); a template that renders to nothing
/// falls back to the book id.
pub fn render(cache: &Cache, book_id: i32, template: &str) -> Result<(String, String)> {
    let parts = components(cache, book_id, template)?;
    let (stem, dirs) = parts.split_last().expect("components is never empty");
    Ok((dirs.join("/"), stem.clone()))
}

fn fingerprint(template: &str, books: &[PlannedBook]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(template.as_bytes());
    for b in books {
        h.update(format!("{}:{}>{};", b.book_id, b.old_path, b.new_path).as_bytes());
        for f in &b.files {
            h.update(format!("{}:{}>{};", f.format, f.from, f.to).as_bytes());
        }
        if let Some(d) = &b.data_dir {
            h.update(format!("data:{}>{};", d.from, d.to).as_bytes());
        }
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

fn in_scope(cache: &Cache, scope: &Scope) -> Result<Vec<i32>> {
    let mut ids = match scope {
        Scope::All => cache.all_book_ids()?,
        Scope::Ids(ids) => ids.clone(),
        Scope::PathPrefix(prefix) => {
            let mut out = Vec::new();
            for id in cache.all_book_ids()? {
                let path = cache.field_for(id, "path")?.unwrap_or_default();
                if lexmechanic_utils::filenames::path_is_within(&path, prefix) {
                    out.push(id);
                }
            }
            out
        }
    };
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

/// Where a plan puts the books.
#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    /// A save-to-disk style template: folder and file name from the metadata.
    Template(&'a str),
    /// One folder, relative to the library (already through [`validate_folder`]);
    /// each file keeps its own name.
    Folder(&'a str),
    /// Rename a folder: every book at `from` (or below it) moves to the same
    /// place under `to`, keeping its sub-folders and file names. Both are
    /// relative to the library, `to` already through [`validate_folder`]. Use
    /// with [`Scope::PathPrefix`]`(from)`.
    Rebase { from: &'a str, to: &'a str },
}

/// `path` with its leading `from` folder replaced by `to` (`None` when `path`
/// is not at or under `from`). Segment-wise, under the one path folding rule
/// ([`lexmechanic_utils::filenames::path_key`]).
fn rebase_path(path: &str, from: &str, to: &str) -> Option<String> {
    let (p, f) = (path.trim_matches('/'), from.trim_matches('/'));
    if !lexmechanic_utils::filenames::path_is_within(p, f) {
        return None;
    }
    // Folding keeps the number of segments, but not the number of bytes.
    let skip = f.split('/').filter(|s| !s.is_empty()).count();
    let rest: Vec<&str> = p.split('/').skip(skip).collect();
    Some(join_rel(to, &rest.join("/")))
}

/// A destination folder from the user, checked and normalised: `/`-separated,
/// relative to the library root. Rejects an empty path, an absolute one (a
/// drive, a UNC path, a leading slash), `..`, characters no platform allows in
/// a name, reserved names, and the library's own state folders. `.` components
/// and repeated separators are dropped.
pub fn validate_folder(folder: &str) -> std::result::Result<String, String> {
    let f = folder.trim().replace('\\', "/");
    if f.is_empty() {
        return Err("a destination folder is required".into());
    }
    if f.starts_with('/') || f.contains(':') || Path::new(&f).is_absolute() {
        return Err("the folder must be relative to the library (no drive, no leading slash)".into());
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in f.split('/') {
        let part = part.trim();
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            return Err("the folder must stay inside the library (no `..`)".into());
        }
        if part.chars().any(|c| c.is_control() || "<>:\"|?*".contains(c)) {
            return Err(format!("`{part}` has a character a folder name cannot have"));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(format!("`{part}` ends with a dot or a space"));
        }
        if lexmechanic_utils::filenames::is_windows_reserved_name(part) {
            return Err(format!("`{part}` is a reserved name"));
        }
        let lower = part.to_lowercase();
        if lower == crate::constants::LIBRARY_HANDLE_DIR_NAME.to_lowercase() || lower == crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME || lower == ".caltrash" {
            return Err(format!("`{part}` is the library's own folder"));
        }
        if part.chars().count() > 120 {
            return Err("a folder name is too long".into());
        }
        parts.push(part);
    }
    if parts.is_empty() {
        return Err("a destination folder is required".into());
    }
    Ok(parts.join("/"))
}

/// Whether any existing part of `rel` under `library` is a symlink or junction
/// (a way out of the library).
fn passes_through_link(library: &Path, rel: &str) -> bool {
    let mut at = library.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        at.push(part);
        match std::fs::symlink_metadata(&at) {
            Ok(m) if m.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    false
}

/// What a plan needs to know about the disk, answered from one directory
/// listing per folder instead of one `stat` per file.
///
/// Planning asks "is this source file there?" for every format of every book
/// and "is this target taken?" for every candidate name. On a share each of
/// those is a round trip; a folder listing answers all the questions about
/// that folder at once. Listings are read the first time a folder is asked
/// about and kept for the plan (a plan changes nothing on disk, and
/// [`execute`] re-checks as it moves: it never replaces an existing file).
///
/// Names are matched the way the library's volume does: ignoring case when a
/// probe of the volume says it does ([`volume_ignores_case`]).
struct DiskView {
    library: std::path::PathBuf,
    fold: bool,
    dirs: std::collections::HashMap<String, Listing>,
    links: std::collections::HashMap<String, LinkState>,
}

enum Listing {
    /// The folder does not exist (or is not a folder): nothing is in it.
    Absent,
    /// It could not be listed; ask the file system about each name instead.
    Unreadable,
    Entries(std::collections::HashMap<String, EntryKind>),
}

#[derive(Clone, Copy)]
enum EntryKind {
    File,
    Dir,
    /// A symlink: only the file system knows what it points at.
    Link,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LinkState {
    Link,
    Plain,
    Missing,
}

/// Whether the volume holding `library` treats `Name` and `name` as one file.
///
/// The server's own platform does not say: a Linux server can mount a
/// case-insensitive SMB share, and a Windows one a case-sensitive folder. It
/// is asked of the volume itself, without writing anything: an entry of the
/// library's top folder is looked up under its other case, unless that
/// spelling is also listed (two real entries, so the answer would be a lie).
/// An empty or unreadable library falls back to the platform's usual rule.
fn volume_ignores_case(library: &Path) -> bool {
    let platform = cfg!(windows) || cfg!(target_os = "macos");
    let Ok(entries) = std::fs::read_dir(library) else { return platform };
    let mut names: Vec<String> = entries.flatten().take(200).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    // `metadata.db` is in every library root and is plain ASCII: the safest name to flip. It goes
    // first, even when the folder lists more than 200 entries.
    if !names.iter().any(|n| n == "metadata.db") && library.join("metadata.db").exists() {
        names.insert(0, "metadata.db".to_string());
    }
    if let Some(at) = names.iter().position(|n| n == "metadata.db") {
        names.swap(0, at);
    }
    probe_case(library, &names).unwrap_or(platform)
}

/// Looks one of `names` up under its other case. A name whose upper-casing is not a plain
/// per-letter mapping (`Straße` -> `STRASSE`, ligatures, the Turkish dotless i) is skipped: a
/// volume that folds case by its own table would not find that spelling and the answer would be
/// "case-sensitive" on a folding volume.
fn probe_case(library: &Path, names: &[String]) -> Option<bool> {
    for name in names {
        if !name.is_ascii() {
            continue;
        }
        let alt = if name.to_uppercase() != *name { name.to_uppercase() } else { name.to_lowercase() };
        if alt == *name || names.iter().any(|n| n != name && n.to_lowercase() == alt.to_lowercase()) {
            continue;
        }
        return Some(std::fs::symlink_metadata(library.join(&alt)).is_ok());
    }
    None
}

impl DiskView {
    fn new(library: &Path) -> Self {
        DiskView { library: library.to_path_buf(), fold: volume_ignores_case(library), dirs: Default::default(), links: Default::default() }
    }

    fn fold_name(&self, name: &str) -> String {
        if self.fold {
            name.to_lowercase()
        } else {
            name.to_string()
        }
    }

    fn split(rel: &str) -> (&str, &str) {
        match rel.rfind('/') {
            Some(i) => (&rel[..i], &rel[i + 1..]),
            None => ("", rel),
        }
    }

    /// The entry named by `rel` (a `/`-separated path under the library), as
    /// the folder listing has it. `Err(())`: the folder could not be listed.
    fn entry(&mut self, rel: &str) -> std::result::Result<Option<EntryKind>, ()> {
        let (dir, name) = Self::split(rel);
        let dir_key = if self.fold { dir.to_lowercase() } else { dir.to_string() };
        if !self.dirs.contains_key(&dir_key) {
            // A folder its parent's listing does not have is not there: no need to ask for it.
            let absent_by_parent = !dir.is_empty() && matches!(self.entry(dir), Ok(None));
            let listing = if absent_by_parent {
                Listing::Absent
            } else {
                match std::fs::read_dir(self.library.join(dir)) {
                Ok(rd) => {
                    let mut map = std::collections::HashMap::new();
                    let mut failed = false;
                    for e in rd {
                        let Ok(e) = e else {
                            failed = true;
                            break;
                        };
                        let Ok(ft) = e.file_type() else {
                            failed = true;
                            break;
                        };
                        let kind = if ft.is_symlink() {
                            EntryKind::Link
                        } else if ft.is_dir() {
                            EntryKind::Dir
                        } else {
                            EntryKind::File
                        };
                        map.insert(self.fold_name(&e.file_name().to_string_lossy()), kind);
                    }
                    if failed {
                        Listing::Unreadable
                    } else {
                        Listing::Entries(map)
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Listing::Absent,
                Err(_) => Listing::Unreadable,
                }
            };
            self.dirs.insert(dir_key.clone(), listing);
        }
        match &self.dirs[&dir_key] {
            Listing::Absent => Ok(None),
            Listing::Unreadable => Err(()),
            Listing::Entries(map) => Ok(map.get(&self.fold_name(name)).copied()),
        }
    }

    /// `library/rel` is a file (a symlink counts when it points at one).
    fn is_file(&mut self, rel: &str) -> bool {
        match self.entry(rel) {
            Ok(Some(EntryKind::File)) => true,
            Ok(Some(EntryKind::Dir)) | Ok(None) => false,
            Ok(Some(EntryKind::Link)) | Err(()) => self.library.join(rel).is_file(),
        }
    }

    /// `library/rel` exists (a broken symlink does not).
    fn exists(&mut self, rel: &str) -> bool {
        match self.entry(rel) {
            Ok(Some(EntryKind::File)) | Ok(Some(EntryKind::Dir)) => true,
            Ok(None) => false,
            Ok(Some(EntryKind::Link)) | Err(()) => self.library.join(rel).exists(),
        }
    }

    /// [`passes_through_link`], remembering what each path prefix was.
    fn passes_through_link(&mut self, rel: &str) -> bool {
        let mut prefix = String::new();
        for part in rel.split('/').filter(|p| !p.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let state = match self.links.get(&prefix) {
                Some(s) => *s,
                None => {
                    // From the folder's listing (one read per folder) rather than
                    // an `lstat` per path; only an unreadable folder is asked about.
                    let s = match self.entry(&prefix) {
                        Ok(Some(EntryKind::Link)) => LinkState::Link,
                        Ok(Some(_)) => LinkState::Plain,
                        Ok(None) => LinkState::Missing,
                        Err(()) => match std::fs::symlink_metadata(self.library.join(&prefix)) {
                            Ok(m) if m.file_type().is_symlink() => LinkState::Link,
                            Ok(_) => LinkState::Plain,
                            Err(_) => LinkState::Missing,
                        },
                    };
                    self.links.insert(prefix.clone(), s);
                    s
                }
            };
            match state {
                LinkState::Link => return true,
                LinkState::Plain => {}
                LinkState::Missing => break,
            }
        }
        false
    }
}

/// Every book's folder (lower-cased, `/`-separated) with its id, read once and
/// only when a plan meets a book with attachments to think about.
#[derive(Default)]
struct BookPaths {
    all: Option<Vec<(String, i32)>>,
}

impl BookPaths {
    fn get(&mut self, cache: &Cache) -> Result<&[(String, i32)]> {
        if self.all.is_none() {
            let conn = cache.backend.conn.lock().unwrap();
            let mut stmt = conn.prepare("SELECT id, path FROM books")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i32>(0)?, r.get::<_, Option<String>>(1)?.unwrap_or_default())))?;
            let mut all = Vec::new();
            for row in rows {
                let (id, path) = row?;
                all.push((lexmechanic_utils::filenames::path_key(&path).trim_matches('/').to_string(), id));
            }
            self.all = Some(all);
        }
        Ok(self.all.as_deref().unwrap_or(&[]))
    }
}

/// Where a book's attached files live (see `extra_files::data_dir`): the `data`
/// folder of its own folder, or `data/<uuid>` for a book in the library root.
fn data_rel(path: &str, uuid: &str) -> String {
    if path.is_empty() {
        format!("data/{uuid}")
    } else {
        format!("{path}/data")
    }
}

/// The attachments folder that moves with a book, if there is one and the move is
/// safe; otherwise `None`, with a plan warning when the folder is the book's but
/// cannot be handled.
///
/// The folder is the book's own when no other book has it as its folder (a source
/// tree's `Reports/data` can be a book's folder, its files not attachments), the
/// book's folder is not shared with other books (they would lose it), nothing
/// inside it is another book's folder, and the destination is free (no folder
/// there, no other book already in the destination folder, which would share
/// the attachments).
#[allow(clippy::too_many_arguments)]
fn plan_data_dir(cache: &Cache, disk: &mut DiskView, paths: &mut BookPaths, taken_data: &mut HashSet<String>, warnings: &mut Vec<Skipped>, book_id: i32, old_path: &str, new_path: &str) -> Result<Option<PlannedDir>> {
    use lexmechanic_utils::filenames::path_key;
    if path_key(old_path) == path_key(new_path) {
        return Ok(None);
    }
    let uuid = cache.book_uuid(book_id)?.unwrap_or_default();
    if uuid.is_empty() && (old_path.is_empty() || new_path.is_empty()) {
        return Ok(None);
    }
    let (from, to) = (data_rel(old_path, &uuid), data_rel(new_path, &uuid));
    match disk.entry(&from) {
        Ok(Some(EntryKind::Dir)) => {}
        Ok(Some(EntryKind::Link)) => {
            warnings.push(Skipped { book_id, reason: format!("{from} is a link, so the files attached to this book there were not moved with it") });
            return Ok(None);
        }
        _ => return Ok(None),
    }
    let warn = |warnings: &mut Vec<Skipped>, why: &str| warnings.push(Skipped { book_id, reason: format!("the files attached to this book in {from} stay where they are: {why}") });
    if !old_path.is_empty() {
        let from_key = path_key(&from);
        let own_key = path_key(old_path).trim_matches('/').to_string();
        let nested = format!("{from_key}/");
        let others = paths.get(cache)?;
        if others.iter().any(|(k, id)| *id != book_id && *k == from_key) {
            // Another book's folder: what is in it are that book's files.
            return Ok(None);
        }
        if others.iter().any(|(k, id)| *id != book_id && k.starts_with(&nested)) {
            warn(warnings, "other books' folders are inside it");
            return Ok(None);
        }
        if others.iter().any(|(k, id)| *id != book_id && *k == own_key) {
            warn(warnings, "other books share this book's folder, and the attachments are shared with them");
            return Ok(None);
        }
    }
    if !new_path.is_empty() {
        let dest_key = path_key(new_path).trim_matches('/').to_string();
        if paths.get(cache)?.iter().any(|(k, id)| *id != book_id && *k == dest_key) {
            warn(warnings, &format!("other books are already in {new_path}, and would share them"));
            return Ok(None);
        }
    }
    if taken_data.contains(&path_key(&to)) || disk.exists(&to) {
        warn(warnings, &format!("{to} is already taken"));
        return Ok(None);
    }
    taken_data.insert(path_key(&to));
    Ok(Some(PlannedDir { from, to }))
}

/// The dry run: what [`execute`] would do. Changes nothing.
pub fn plan(cache: &Cache, template: &str, scope: &Scope) -> Result<OrganizePlan> {
    plan_to(cache, Target::Template(template), scope)
}

/// [`plan`] for any [`Target`].
pub fn plan_to(cache: &Cache, target: Target<'_>, scope: &Scope) -> Result<OrganizePlan> {
    let library = &cache.backend.library_path;
    let label = match target {
        Target::Template(t) => t.to_string(),
        Target::Folder(f) => format!("move to {f}"),
        Target::Rebase { from, to } => format!("rename folder {from} to {to}"),
    };
    if let Target::Folder(f) | Target::Rebase { to: f, .. } = target {
        if passes_through_link(library, f) {
            return Err(anyhow!("the folder {f} goes through a link out of the library"));
        }
    }
    let mut books = Vec::new();
    let mut skipped = Vec::new();
    let mut warnings = Vec::new();
    let mut already = 0usize;
    // Folder listings and link checks, read once per folder for the whole plan.
    let mut disk = DiskView::new(library);
    // Destination folders already checked for a link out of the library.
    let mut link_checked: std::collections::HashMap<String, bool> = std::collections::HashMap::new();
    // Lower-cased relative targets taken by earlier books in this plan.
    let mut taken: HashSet<String> = HashSet::new();
    // Lower-cased `data/` folders earlier books in this plan take along, and every
    // book's folder (read only if some book has attachments to move).
    let mut taken_data: HashSet<String> = HashSet::new();
    let mut paths = BookPaths::default();

    for book_id in in_scope(cache, scope)? {
        let old_path = cache.field_for(book_id, "path")?.unwrap_or_default().replace('\\', "/").trim_matches('/').to_string();
        let title = cache.field_for(book_id, "title")?.unwrap_or_default();
        let formats = cache.format_files(book_id)?;
        if formats.is_empty() {
            skipped.push(Skipped { book_id, reason: "the book has no files".into() });
            continue;
        }
        // The destination folder, and the file name when the target decides it
        // (a template does; a folder keeps each file's own).
        let (new_path, fixed_stem): (String, Option<String>) = match target {
            Target::Template(template) => {
                let parts = match components(cache, book_id, template) {
                    Ok(p) => p,
                    Err(e) => {
                        skipped.push(Skipped { book_id, reason: format!("the template failed: {e}") });
                        continue;
                    }
                };
                let (stem, dirs) = parts.split_last().expect("components is never empty");
                (dirs.join("/"), Some(stem.clone()))
            }
            Target::Folder(folder) => (folder.to_string(), None),
            Target::Rebase { from, to } => match rebase_path(&old_path, from, to) {
                Some(p) => (p, None),
                None => {
                    skipped.push(Skipped { book_id, reason: format!("the book is not inside {from}") });
                    continue;
                }
            },
        };
        // Every kind of target, not just a typed-in folder: a template can render
        // into a folder that is a symlink or junction to somewhere else.
        let linked = match link_checked.get(&new_path) {
            Some(l) => *l,
            None => {
                let l = disk.passes_through_link(&new_path);
                link_checked.insert(new_path.clone(), l);
                l
            }
        };
        if linked {
            skipped.push(Skipped { book_id, reason: format!("the folder {new_path} goes through a link out of the library") });
            continue;
        }
        let stem_for = |old_stem: &str, n: usize| -> String {
            let base = fixed_stem.clone().unwrap_or_else(|| old_stem.to_string());
            if n == 0 {
                base
            } else {
                format!("{base} ({})", n + 1)
            }
        };

        // Each format's current file, and the extension it really has on disk.
        let mut current: Vec<(String, String, String, String)> = Vec::new(); // (FORMAT, old stem, ext, from rel)
        let mut missing = None;
        for (name, format) in &formats {
            let found = [format.to_lowercase(), format.to_uppercase()].into_iter().find(|ext| disk.is_file(&join_rel(&old_path, &format!("{name}.{ext}"))));
            match found {
                Some(ext) => current.push((format.to_uppercase(), name.clone(), ext.clone(), join_rel(&old_path, &format!("{name}.{ext}")))),
                None => {
                    missing = Some(join_rel(&old_path, &format!("{name}.{}", format.to_lowercase())));
                    break;
                }
            }
        }
        if let Some(file) = missing {
            skipped.push(Skipped { book_id, reason: format!("the file {file} is not on disk") });
            continue;
        }

        // The first suffix at which every target is free (or is the book's own file).
        let mut chosen: Option<(usize, bool)> = None;
        for n in 0..1000 {
            let ok = current.iter().all(|(_, old_stem, ext, from)| {
                let to = join_rel(&new_path, &format!("{}.{ext}", stem_for(old_stem, n)));
                let (to_l, from_l) = (lexmechanic_utils::filenames::path_key(&to), lexmechanic_utils::filenames::path_key(from));
                to_l == from_l || (!taken.contains(&to_l) && !disk.exists(&to))
            });
            if ok {
                chosen = Some((n, n > 0));
                break;
            }
        }
        let Some((suffix_n, suffixed)) = chosen else {
            skipped.push(Skipped { book_id, reason: "no free file name".into() });
            continue;
        };

        let mut files = Vec::new();
        let (mut old_names, mut new_names) = (BTreeMap::new(), BTreeMap::new());
        for (format, old_stem, ext, from) in &current {
            let new_stem = stem_for(old_stem, suffix_n);
            let to = join_rel(&new_path, &format!("{new_stem}.{ext}"));
            taken.insert(lexmechanic_utils::filenames::path_key(&to));
            let absolute_len = library.join(&to).to_string_lossy().chars().count();
            if absolute_len > MAX_COMFORTABLE_PATH {
                warnings.push(Skipped { book_id, reason: format!("the new path is {absolute_len} characters long; some Windows programs refuse paths over {MAX_COMFORTABLE_PATH}: {to}") });
            }
            old_names.insert(format.clone(), old_stem.clone());
            new_names.insert(format.clone(), new_stem);
            if &to != from {
                files.push(PlannedFile { format: format.clone(), from: from.clone(), to });
            }
        }
        if files.is_empty() {
            already += 1;
            continue;
        }
        let data_dir = plan_data_dir(cache, &mut disk, &mut paths, &mut taken_data, &mut warnings, book_id, &old_path, &new_path)?;
        books.push(PlannedBook { book_id, title, old_path, new_path, old_names, new_names, files, suffixed, data_dir });
    }
    // Two books planned into one folder cannot both own its `data/`, whichever is planned first:
    // the one that took it along would show its attachments on the other (and deleting it would
    // leave them behind). Each of them keeps its attachments where they are, with a warning.
    let mut into_folder: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for b in &books {
        *into_folder.entry(lexmechanic_utils::filenames::path_key(&b.new_path).trim_matches('/').to_string()).or_default() += 1;
    }
    for b in books.iter_mut() {
        let shared = !b.new_path.is_empty() && into_folder.get(lexmechanic_utils::filenames::path_key(&b.new_path).trim_matches('/')).is_some_and(|n| *n > 1);
        if shared {
            if let Some(d) = b.data_dir.take() {
                warnings.push(Skipped { book_id: b.book_id, reason: format!("the files attached to this book in {} stay where they are: other books are moving into {}, and would share them", d.from, b.new_path) });
            }
        }
    }
    let fingerprint = fingerprint(&label, &books);
    // A rename of a folder that only changes its case needs the directory
    // renamed too (`execute` does it once the books are in place).
    let case_fix_folder = match target {
        Target::Rebase { from, to } if from != to && lexmechanic_utils::filenames::path_key(from) == lexmechanic_utils::filenames::path_key(to) => Some(to.to_string()),
        _ => None,
    };
    Ok(OrganizePlan { template: label, books, skipped, already_organized: already, fingerprint, warnings, case_fix_folder })
}

/// Whether another book's folder is exactly `rel` (a `data` folder that is a book of its own,
/// not an attachments folder).
fn another_book_is_at(cache: &Cache, book_id: i32, rel: &str) -> bool {
    let conn = cache.backend.conn.lock().unwrap();
    conn.query_row("SELECT COUNT(*) FROM books WHERE id != ?1 AND lower(path) = lower(?2)", (book_id, rel), |row| row.get::<_, i64>(0)).map(|n| n > 0).unwrap_or(false)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrganizeRun {
    pub run_id: String,
    pub template: String,
    pub created_at: i64,
    pub books: Vec<PlannedBook>,
    #[serde(default)]
    pub undone_at: Option<i64>,
    /// Books with an entry in the journal whose move was never confirmed: the
    /// process died somewhere between writing the entry and recording the move.
    /// Their files may be in either place; [`undo`] looks.
    #[serde(default)]
    pub pending: Vec<i32>,
    /// Directories renamed only in case at the end of the run (`Fiction` to
    /// `fiction`), journaled before each rename. [`undo`] renames them back.
    #[serde(default)]
    pub dir_case: Vec<DirCaseRename>,
}

/// One directory renamed only in case: `from` to `to`, inside `parent` (relative
/// to the library root, `""` for the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirCaseRename {
    pub parent: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OrganizeReport {
    pub run_id: String,
    pub books_planned: usize,
    pub books_moved: usize,
    pub files_moved: usize,
    pub failed: Vec<Skipped>,
    pub cancelled: bool,
    /// Things that went wrong without making the run fail (a follow-up write
    /// that did not land). The moves themselves are all in the undo journal.
    pub warnings: Vec<String>,
}

/// The run's undo record. A run that moved nothing leaves none.
pub fn load_run(library: &Path, run_id: &str) -> Result<OrganizeRun> {
    crate::run_journal::ORGANIZE_RUNS.load(library, run_id)
}

/// Runs, newest first: `(run_id, created_at, books, undone)`.
pub fn list_runs(library: &Path) -> Vec<(String, i64, usize, bool)> {
    crate::run_journal::ORGANIZE_RUNS.list(library)
}

/// Removes `rel` (a folder inside the library) and its parents while they are
/// empty. Non-recursive: a folder with anything in it stays.
fn prune_empty_dirs(library: &Path, rel: &str) {
    let mut rel = rel.trim_matches('/').to_string();
    while !rel.is_empty() {
        if std::fs::remove_dir(library.join(&rel)).is_err() {
            break;
        }
        rel = match rel.rfind('/') {
            Some(i) => rel[..i].to_string(),
            None => String::new(),
        };
    }
}

/// Makes the directories along `rel` carry exactly `rel`'s spelling.
///
/// A folder rename that only changes case (`fiction` to `Fiction`) moves the
/// books' files, but on a case-insensitive volume the files land in the
/// directory that is already there and it keeps its old spelling. Renaming a
/// directory to a name that differs only in case is refused by some
/// filesystems and a no-op on others, so it goes through a temporary name.
///
/// Each rename is written to the run's undo `journal` (and fsynced) before it
/// is made, so undoing the run can restore the old spelling. If that entry
/// cannot be written the directory is left as it is.
fn fix_dir_case(cache: &Cache, rel: &str, journal: &mut crate::run_journal::RunJournal) -> Result<()> {
    let library = &cache.backend.library_path;
    let handle = cache.backend.write_handle()?;
    let mut at = library.to_path_buf();
    let mut parent_rel = String::new();
    for part in rel.split('/').filter(|p| !p.is_empty()) {
        let key = lexmechanic_utils::filenames::path_key(part);
        let mut exact = false;
        let mut other: Option<String> = None;
        for entry in std::fs::read_dir(&at)?.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == part {
                exact = true;
            } else if lexmechanic_utils::filenames::path_key(&name) == key && entry.path().is_dir() {
                other = Some(name);
            }
        }
        if let (false, Some(name)) = (exact, other) {
            journal.append_dir_case(&parent_rel, &name, part).context("could not write the undo entry for the folder's new spelling, so it was not renamed")?;
            rename_dir_case(&handle, &at, &name, part)?;
        }
        at.push(part);
        if !parent_rel.is_empty() {
            parent_rel.push('/');
        }
        parent_rel.push_str(part);
    }
    Ok(())
}

/// Renames the directory `from` inside `parent` to `to` (a name differing only
/// in case) through a temporary name.
fn rename_dir_case(handle: &crate::library_handle::LibraryHandle, parent: &Path, from: &str, to: &str) -> Result<()> {
    let temp = parent.join(format!("{to}.case-{}", &uuid::Uuid::new_v4().simple().to_string()[..6]));
    handle.rename_atomic(&parent.join(from), &temp).with_context(|| format!("renaming {from} aside"))?;
    handle.rename_atomic_no_replace(&temp, &parent.join(to)).with_context(|| format!("renaming {from} to {to}"))?;
    Ok(())
}

/// Puts a directory's old spelling back (undo of [`fix_dir_case`]): when
/// `parent` holds a directory named exactly `rename.to` and none named exactly
/// `rename.from`. Returns whether it renamed anything.
fn restore_dir_case(cache: &Cache, rename: &DirCaseRename) -> Result<bool> {
    let parent = cache.backend.library_path.join(&rename.parent);
    let (mut has_to, mut has_from) = (false, false);
    for entry in std::fs::read_dir(&parent)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == rename.to && entry.path().is_dir() {
            has_to = true;
        }
        if name == rename.from {
            has_from = true;
        }
    }
    if !has_to || has_from {
        return Ok(false);
    }
    let handle = cache.backend.write_handle()?;
    rename_dir_case(&handle, &parent, &rename.to, &rename.from)?;
    Ok(true)
}

/// Moves the book's files, all or none. If a move fails, the ones already done
/// are put back, and anything that could not be put back is named in the error
/// (the files are then split between the two places, and the person has to know).
fn move_files(cache: &Cache, files: &[(String, String)]) -> Result<()> {
    let library = &cache.backend.library_path;
    let handle = cache.backend.write_handle()?;
    let mut done: Vec<(&String, &String)> = Vec::new();
    for (from, to) in files {
        let (from_abs, to_abs) = (library.join(from), library.join(to));
        let step = (|| -> Result<()> {
            if !from_abs.is_file() {
                return Err(anyhow!("{from} is not there"));
            }
            // A case-only rename names the same file on a case-insensitive
            // volume; anything else already at the target is somebody's file.
            let same_file = to_abs.exists() && lexmechanic_utils::filenames::samefile(&from_abs, &to_abs);
            if to_abs.exists() && !same_file {
                return Err(anyhow!("{to} already exists"));
            }
            if let Some(parent) = to_abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let moved = if same_file {
                handle.rename_atomic(&from_abs, &to_abs)
            } else {
                // The check above and the move are one step here, so a file
                // that appears in between is refused, not replaced.
                handle.rename_atomic_no_replace(&from_abs, &to_abs)
            };
            moved.map_err(|e| anyhow!("could not move {from} to {to} (a rename cannot cross volumes): {e}"))
        })();
        if let Err(e) = step {
            let mut stuck = Vec::new();
            for (f, t) in done.into_iter().rev() {
                if let Err(why) = handle.rename_atomic(&library.join(t), &library.join(f)) {
                    stuck.push(format!("{t} could not be put back to {f}: {why}"));
                }
            }
            if stuck.is_empty() {
                return Err(e);
            }
            return Err(anyhow!("{e:#}; the files already moved were not all put back: {}", stuck.join("; ")));
        }
        done.push((from, to));
    }
    Ok(())
}

/// Moves a book's attachments folder from `from` to `to` (relative to the
/// library), never into a place that is taken. `must_exist` false makes a
/// missing `from` a no-op (an undo, when the folder has been deleted since);
/// returns whether anything moved.
fn move_dir(cache: &Cache, from: &str, to: &str, must_exist: bool) -> Result<bool> {
    let library = &cache.backend.library_path;
    let (from_abs, to_abs) = (library.join(from), library.join(to));
    if !from_abs.is_dir() {
        if must_exist {
            return Err(anyhow!("{from} is not there"));
        }
        return Ok(false);
    }
    if to_abs.exists() {
        return Err(anyhow!("{to} already exists"));
    }
    if let Some(parent) = to_abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let handle = cache.backend.write_handle()?;
    handle.rename_atomic_no_replace(&from_abs, &to_abs).map_err(|e| anyhow!("could not move {from} to {to} (a rename cannot cross volumes): {e}"))?;
    Ok(true)
}

/// A cover kept the calibre way, as `<book folder>/cover.jpg` (absorbed calibre
/// libraries), is found through `books.path`. The move leaves `cover.jpg` where
/// it is (the folder may be shared), so once the path changes the cover would be
/// lost. Copy it to the sidecar store first; `covers::cover_path` then finds it
/// there after the move, and the legacy one again after an undo.
fn keep_cover(cache: &Cache, book_id: i32) -> Result<()> {
    let Some(legacy) = crate::covers::legacy_cover_path(cache, book_id)?.filter(|p| p.is_file()) else {
        return Ok(());
    };
    let sidecar = crate::covers::sidecar_cover_path(cache, book_id)?;
    if sidecar.is_file() {
        return Ok(());
    }
    if let Some(parent) = sidecar.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(&legacy, &sidecar).with_context(|| format!("copying {} to {}", legacy.display(), sidecar.display()))?;
    Ok(())
}

/// Records where a book's files are now: the folder and every format's name in
/// one transaction (and one change-log entry), or nothing.
fn record(cache: &Cache, book_id: i32, path: &str, names: &BTreeMap<String, String>, current_path: &str) -> Result<()> {
    cache.set_book_location(book_id, (path != current_path).then_some(path), names)
}

/// Carries out a plan made by [`plan`] (the caller has checked its
/// fingerprint). A cancel stops between books.
///
/// Each book's undo entry is written, and fsynced, before its first file moves;
/// a book whose entry cannot be written is not moved.
pub fn execute(cache: &Cache, plan: &OrganizePlan, control: &JobControl) -> Result<OrganizeReport> {
    let library = cache.backend.library_path.clone();
    let header = OrganizeRun { run_id: crate::run_journal::ORGANIZE_RUNS.new_run_id(), template: plan.template.clone(), created_at: crate::trash::now_secs(), ..Default::default() };
    let mut report = OrganizeReport { run_id: header.run_id.clone(), books_planned: plan.books.len(), ..Default::default() };
    let mut journal: Option<crate::run_journal::RunJournal> = None;
    let mut old_dirs: HashSet<String> = HashSet::new();
    for book in &plan.books {
        if control.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let pairs: Vec<(String, String)> = book.files.iter().map(|f| (f.from.clone(), f.to.clone())).collect();
        if let Err(e) = keep_cover(cache, book.book_id) {
            report.failed.push(Skipped { book_id: book.book_id, reason: format!("could not keep its cover: {e:#}") });
            continue;
        }
        // The undo entry goes to disk first. The journal itself is opened on the
        // first book, so a run that moves nothing leaves no file; if it cannot be
        // opened nothing has moved yet and the run stops.
        if journal.is_none() {
            journal = Some(crate::run_journal::ORGANIZE_RUNS.begin(&library, &header.run_id, &header).context("could not start the undo journal, so nothing was moved")?);
        }
        let j = journal.as_mut().expect("opened above");
        if let Err(e) = j.append_book(book) {
            report.failed.push(Skipped { book_id: book.book_id, reason: format!("could not write its undo entry, so it was not moved: {e:#}") });
            continue;
        }
        if let Err(e) = move_files(cache, &pairs) {
            // move_files put back what it had moved; if it could not, the error says so
            // and the entry stays pending for undo to look at.
            if !format!("{e:#}").contains("not all put back") {
                let _ = j.abandon(book.book_id);
            }
            report.failed.push(Skipped { book_id: book.book_id, reason: format!("{e:#}") });
            continue;
        }
        let back: Vec<(String, String)> = pairs.iter().map(|(f, t)| (t.clone(), f.clone())).collect();
        // The attachments go with the book (the entry above names them too).
        let mut data_moved = false;
        if let Some(d) = &book.data_dir {
            match move_dir(cache, &d.from, &d.to, true) {
                Ok(moved) => data_moved = moved,
                Err(e) => {
                    let mut reason = format!("its attached files could not be moved: {e:#}");
                    match move_files(cache, &back) {
                        Ok(()) => {
                            let _ = j.abandon(book.book_id);
                        }
                        Err(rollback) => reason.push_str(&format!("; and the files could not be put back: {rollback:#}")),
                    }
                    report.failed.push(Skipped { book_id: book.book_id, reason });
                    continue;
                }
            }
        }
        if let Err(e) = record(cache, book.book_id, &book.new_path, &book.new_names, &book.old_path) {
            // The files moved and the rows did not: put the files back.
            let mut reason = format!("the library could not record the move: {e:#}");
            if let (true, Some(d)) = (data_moved, &book.data_dir) {
                if let Err(rollback) = move_dir(cache, &d.to, &d.from, false) {
                    reason.push_str(&format!("; and the attached files could not be put back: {rollback:#}"));
                }
            }
            match move_files(cache, &back) {
                Ok(()) => {
                    let _ = j.abandon(book.book_id);
                }
                Err(rollback) => reason.push_str(&format!("; and the files could not be put back: {rollback:#}")),
            }
            report.failed.push(Skipped { book_id: book.book_id, reason });
            continue;
        }
        if let Err(e) = j.mark_done(book.book_id) {
            report.warnings.push(format!("could not mark book {} done in the undo journal (undo will still find it): {e:#}", book.book_id));
        }
        old_dirs.insert(book.old_path.clone());
        report.books_moved += 1;
        report.files_moved += pairs.len();
    }
    for dir in &old_dirs {
        prune_empty_dirs(&library, dir);
    }
    if let Some(folder) = &plan.case_fix_folder {
        if report.books_moved > 0 && !report.cancelled {
            // The journal was opened for the first book, so it exists whenever one moved.
            if let Some(j) = journal.as_mut() {
                if let Err(e) = fix_dir_case(cache, folder, j) {
                    report.warnings.push(format!("the books were moved but the folder could not be renamed to {folder}: {e:#}"));
                }
            }
        }
    }
    Ok(report)
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UndoReport {
    pub run_id: String,
    pub books_restored: usize,
    pub files_restored: usize,
    /// Books left alone, and why (changed since the run).
    pub skipped: Vec<Skipped>,
    /// Follow-up writes that failed without making the undo fail.
    pub warnings: Vec<String>,
}

/// Puts a run's books back where they were.
pub fn undo(cache: &Cache, run_id: &str, control: &JobControl) -> Result<UndoReport> {
    let library = cache.backend.library_path.clone();
    let run = load_run(&library, run_id)?;
    if run.undone_at.is_some() {
        return Err(anyhow!("run {run_id} was already undone"));
    }
    let mut report = UndoReport { run_id: run_id.to_string(), ..Default::default() };
    let mut new_dirs: HashSet<String> = HashSet::new();
    for book in run.books.iter().rev() {
        if control.is_cancelled() {
            break;
        }
        let current = cache.field_for(book.book_id, "path").ok().flatten().unwrap_or_default().replace('\\', "/");
        if run.pending.contains(&book.book_id) && current == book.old_path {
            // The entry was written and the process died before the move was
            // confirmed: the files may be at the new place with the rows still at
            // the old. Move back whatever is at the new place; there is nothing
            // to record, the rows never changed.
            let handle = cache.backend.write_handle()?;
            let mut restored = 0;
            for f in &book.files {
                let (to_abs, from_abs) = (library.join(&f.to), library.join(&f.from));
                if to_abs.is_file() && !from_abs.exists() && handle.rename_atomic_no_replace(&to_abs, &from_abs).is_ok() {
                    restored += 1;
                }
            }
            // The attachments, if the run got as far as moving them.
            let data_back = match &book.data_dir {
                Some(d) if library.join(&d.to).is_dir() && !library.join(&d.from).exists() => move_dir(cache, &d.to, &d.from, false).unwrap_or(false),
                _ => false,
            };
            if restored > 0 || data_back {
                report.books_restored += 1;
                report.files_restored += restored;
                new_dirs.insert(book.new_path.clone());
            }
            continue;
        }
        if current != book.new_path {
            report.skipped.push(Skipped { book_id: book.book_id, reason: "the book has been moved or removed since".into() });
            continue;
        }
        let back: Vec<(String, String)> = book.files.iter().map(|f| (f.to.clone(), f.from.clone())).collect();
        // The attachments first, the reverse of the run: if the files then cannot
        // move, the attachments go forward again and the book is left as it was.
        // A folder deleted since is not an error.
        let mut data_back = false;
        if let Some(d) = &book.data_dir {
            match move_dir(cache, &d.to, &d.from, false) {
                Ok(moved) => data_back = moved,
                Err(e) => {
                    report.skipped.push(Skipped { book_id: book.book_id, reason: format!("its attached files could not be moved back: {e:#}") });
                    continue;
                }
            }
        }
        if let Err(e) = move_files(cache, &back) {
            let mut reason = format!("{e:#}");
            if let (true, Some(d)) = (data_back, &book.data_dir) {
                if let Err(rollback) = move_dir(cache, &d.from, &d.to, false) {
                    reason.push_str(&format!("; and the attached files could not be put back: {rollback:#}"));
                }
            }
            report.skipped.push(Skipped { book_id: book.book_id, reason });
            continue;
        }
        if let Err(e) = record(cache, book.book_id, &book.old_path, &book.old_names, &book.new_path) {
            let fwd: Vec<(String, String)> = back.iter().map(|(f, t)| (t.clone(), f.clone())).collect();
            let mut reason = format!("the library could not record the move: {e:#}");
            if let (true, Some(d)) = (data_back, &book.data_dir) {
                if let Err(rollback) = move_dir(cache, &d.from, &d.to, false) {
                    reason.push_str(&format!("; and the attached files could not be put back: {rollback:#}"));
                }
            }
            if let Err(rollback) = move_files(cache, &fwd) {
                reason.push_str(&format!("; and the files could not be put back: {rollback:#}"));
            }
            report.skipped.push(Skipped { book_id: book.book_id, reason });
            continue;
        }
        new_dirs.insert(book.new_path.clone());
        report.books_restored += 1;
        report.files_restored += back.len();
    }
    for dir in &new_dirs {
        prune_empty_dirs(&library, dir);
    }
    // Directories the run renamed only in case get their old spelling back, last
    // renamed first (a child was renamed under its parent's new spelling). Only
    // when every book came back: with books left behind, the folder they are in
    // keeps the name their records use.
    if report.skipped.is_empty() && !control.is_cancelled() {
        for rename in run.dir_case.iter().rev() {
            if let Err(e) = restore_dir_case(cache, rename) {
                report.warnings.push(format!("the books are back but the folder {} could not be renamed back to {}: {e:#}", rename.to, rename.from));
            }
        }
    }
    if report.skipped.is_empty() && !control.is_cancelled() {
        // The undo itself is done; failing to say so must not make it an error.
        if let Err(e) = crate::run_journal::ORGANIZE_RUNS.mark_undone(&library, run_id, crate::trash::now_secs()) {
            report.warnings.push(format!("the books are restored but the run could not be marked undone: {e:#}"));
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::{walk, ScanOptions};
    use std::time::{Duration, SystemTime};

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn write(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, bytes).unwrap();
    }

    fn index(dir: &Path, cache: &Cache) {
        let report = walk(dir, &ScanOptions::default(), SystemTime::now() + Duration::from_secs(3600)).unwrap();
        crate::scan::index_scan(cache, dir, &report);
    }

    fn book_with_title(cache: &Cache, title: &str) -> i32 {
        cache.all_book_ids().unwrap().into_iter().find(|id| cache.field_for(*id, "title").unwrap().unwrap_or_default() == title).unwrap_or_else(|| panic!("no book titled {title}"))
    }

    fn run(cache: &Cache, template: &str) -> (OrganizePlan, OrganizeReport) {
        let plan = plan(cache, template, &Scope::All).unwrap();
        let report = execute(cache, &plan, &JobControl::new()).unwrap();
        (plan, report)
    }

    #[test]
    fn the_plan_changes_nothing_and_names_every_move() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);

        let plan = plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
        assert_eq!(plan.books.len(), 2);
        let mut targets: Vec<&str> = plan.books.iter().flat_map(|b| b.files.iter().map(|f| f.to.as_str())).collect();
        targets.sort();
        assert_eq!(targets, ["Sorted/Alpha.pdf", "Sorted/Beta.pdf"]);
        assert!(dir.path().join("inbox/Alpha.pdf").exists(), "a dry run moves nothing");
        assert!(!dir.path().join("Sorted").exists());
        assert_eq!(plan.fingerprint, super::plan(&cache, "Sorted/{title}", &Scope::All).unwrap().fingerprint, "the fingerprint is stable");
        assert_ne!(plan.fingerprint, super::plan(&cache, "Other/{title}", &Scope::All).unwrap().fingerprint);
    }

    #[test]
    fn execute_moves_only_the_books_own_files_and_updates_the_database() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
        write(&dir.path().join("inbox/notes.txt"), b"not a book");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");

        let plan = plan(&cache, "Sorted/{title}", &Scope::Ids(vec![alpha])).unwrap();
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!(report.books_moved, 1, "{report:?}");
        assert!(dir.path().join("Sorted/Alpha.pdf").is_file());
        assert!(!dir.path().join("inbox/Alpha.pdf").exists());
        // The neighbour in the shared folder and the unrelated file are untouched.
        assert!(dir.path().join("inbox/Beta.pdf").is_file());
        assert!(dir.path().join("inbox/notes.txt").is_file());
        assert_eq!(cache.field_for(alpha, "path").unwrap().unwrap(), "Sorted");
        assert_eq!(cache.format_files(alpha).unwrap(), vec![("Alpha".to_string(), "PDF".to_string())]);
        // Nothing left to do.
        assert!(super::plan(&cache, "Sorted/{title}", &Scope::Ids(vec![alpha])).unwrap().books.is_empty());
    }

    /// T29a #12 / T29d: a book's own `data/` attachments now move with it (and the plan says
    /// so); a `data` folder that is another book's own folder is not "attachments".
    #[test]
    fn the_plan_names_attachments_that_would_stay_behind() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/data/notes.xyz"), b"attached");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");

        let p = plan(&cache, "Sorted/{title}", &Scope::Ids(vec![alpha])).unwrap();
        assert_eq!(p.books.len(), 1);
        let moved = p.books[0].data_dir.clone().expect("the attachments folder moves with the book");
        assert_eq!(moved.from, "inbox/data");
        let report = execute(&cache, &p, &JobControl::new()).unwrap();
        assert_eq!(report.books_moved, 1);
        assert!(dir.path().join(&moved.to).join("notes.xyz").is_file(), "moved with the book, never deleted");

        // A book of its own at `<folder>/data` is not an attachments folder.
        let (dir, cache) = library();
        write(&dir.path().join("Reports/report.pdf"), b"%PDF report");
        write(&dir.path().join("Reports/data/dataset.pdf"), b"%PDF dataset");
        index(dir.path(), &cache);
        let report_book = book_with_title(&cache, "report");
        let p = plan(&cache, "Sorted/{title}", &Scope::Ids(vec![report_book])).unwrap();
        assert!(p.warnings.iter().all(|w| !w.reason.contains("attached")), "{:?}", p.warnings);
    }

    /// An absorbed calibre book keeps its cover as `<folder>/cover.jpg`, found
    /// through `books.path`. Organizing leaves that file in place, so the cover
    /// must still resolve after the move (and after an undo).
    #[test]
    fn a_calibre_style_cover_survives_the_move_and_the_undo() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/cover.jpg"), b"\xff\xd8 alpha cover");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");
        assert_eq!(std::fs::read(crate::covers::cover_path(&cache, alpha).unwrap()).unwrap(), b"\xff\xd8 alpha cover");

        let plan = plan(&cache, "Sorted/{title}", &Scope::Ids(vec![alpha])).unwrap();
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert_eq!(report.books_moved, 1, "{report:?}");
        assert_eq!(std::fs::read(crate::covers::cover_path(&cache, alpha).unwrap()).unwrap(), b"\xff\xd8 alpha cover", "the cover is still found after the move");

        undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(std::fs::read(crate::covers::cover_path(&cache, alpha).unwrap()).unwrap(), b"\xff\xd8 alpha cover", "and after the undo");
    }

    #[test]
    fn a_collision_gets_a_suffix_and_nothing_is_overwritten() {
        let (dir, cache) = library();
        write(&dir.path().join("a/Same.pdf"), b"%PDF one");
        write(&dir.path().join("b/Same.pdf"), b"%PDF two");
        write(&dir.path().join("Sorted/Same.pdf"), b"%PDF already here");
        index(dir.path(), &cache);

        let (plan, report) = run(&cache, "Sorted/{title}");
        assert_eq!(report.failed.len(), 0, "{report:?}");
        assert!(plan.conflicts() >= 1);
        let mut names: Vec<String> = std::fs::read_dir(dir.path().join("Sorted")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), 3, "{names:?}");
        assert_eq!(std::fs::read(dir.path().join("Sorted/Same.pdf")).unwrap(), b"%PDF already here", "the file that was there is untouched");
    }

    /// Case folding is the volume's, found by looking an entry up under its other
    /// case -- not the server's operating system -- and nothing is written.
    #[test]
    fn the_disk_view_folds_case_as_the_volume_does() {
        let dir = tempfile::tempdir().unwrap();
        // Nothing to ask the volume about: the platform's usual rule.
        assert_eq!(volume_ignores_case(dir.path()), cfg!(windows) || cfg!(target_os = "macos"));
        write(&dir.path().join("Probe.txt"), b"x");
        let ignores = std::fs::metadata(dir.path().join("PROBE.TXT")).is_ok();
        assert_eq!(volume_ignores_case(dir.path()), ignores);
        assert_eq!(DiskView::new(dir.path()).fold, ignores);
        // Both spellings listed (a case-sensitive volume): no answer from them, and none written.
        if !ignores {
            write(&dir.path().join("PROBE.TXT"), b"y");
            assert!(!volume_ignores_case(dir.path()));
        }
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), if ignores { 1 } else { 2 });
    }

    /// A name with a non-letter-for-letter case mapping is no evidence either way, and the
    /// library's own `metadata.db` is what the probe asks first.
    #[test]
    fn the_case_probe_skips_irregular_names_and_asks_metadata_db_first() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("metadata.db"), b"x");
        let ignores = std::fs::metadata(dir.path().join("METADATA.DB")).is_ok();
        // `Straße` upper-cases to `STRASSE`: a volume that folds case would not find it under that spelling.
        write(&dir.path().join("Straße"), b"x");
        assert_eq!(probe_case(dir.path(), &["Straße".to_string()]), None);
        assert_eq!(probe_case(dir.path(), &["Straße".to_string(), "metadata.db".to_string()]), Some(ignores));
        for i in 0..250 {
            write(&dir.path().join(format!("Entry{i:03}.txt")), b"x");
        }
        assert_eq!(volume_ignores_case(dir.path()), ignores);
    }

    /// The directory-listing view must answer what `is_file()`/`exists()` on the
    /// real path would: files, folders, missing names, names in a missing folder,
    /// names that differ only in case, and the root.
    #[test]
    fn the_disk_view_answers_like_the_file_system() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("a/b/Book.pdf"), b"x");
        write(&dir.path().join("a/b/Other.PDF"), b"x");
        write(&dir.path().join("Root.epub"), b"x");
        std::fs::create_dir_all(dir.path().join("a/b/Sub.pdf")).unwrap();
        let mut view = DiskView::new(dir.path());
        for rel in [
            "a/b/Book.pdf",
            "a/b/book.pdf",
            "A/B/BOOK.PDF",
            "a/b/Other.PDF",
            "a/b/other.pdf",
            "a/b/Sub.pdf",
            "a/b/Missing.pdf",
            "a/b",
            "a",
            "nope/Book.pdf",
            "nope/deeper/Book.pdf",
            "Root.epub",
            "root.EPUB",
            "Missing.epub",
            "a/b/Book.pdf/inside",
        ] {
            // Asked twice: the second answer comes from the remembered listing.
            for _ in 0..2 {
                assert_eq!(view.is_file(rel), dir.path().join(rel).is_file(), "is_file({rel})");
                assert_eq!(view.exists(rel), dir.path().join(rel).exists(), "exists({rel})");
            }
        }
    }

    fn attached(cache: &Cache, id: i32) -> Vec<String> {
        let mut names: Vec<String> = crate::extra_files::list_extra_files(cache, id, crate::constants::DATA_FILE_PATTERN).unwrap().into_iter().map(|f| f.relpath).collect();
        names.sort();
        names
    }

    /// A book with a folder of its own takes its `data/` attachments along, the
    /// run journal records it, and Undo brings it back.
    #[test]
    fn the_attachments_folder_moves_with_its_book_and_comes_back_on_undo() {
        let (dir, cache) = library();
        write(&dir.path().join("Alpha/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("Alpha/data/notes.dat"), b"notes");
        write(&dir.path().join("Alpha/data/sub/deep.dat"), b"deep");
        write(&dir.path().join("Beta/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");
        assert_eq!(attached(&cache, alpha), ["data/notes.dat", "data/sub/deep.dat"]);

        let plan = plan(&cache, "Moved/{title}/{title}", &Scope::All).unwrap();
        let planned = plan.books.iter().find(|b| b.book_id == alpha).unwrap();
        assert_eq!(planned.data_dir, Some(PlannedDir { from: "Alpha/data".into(), to: "Moved/Alpha/data".into() }));
        assert!(plan.books.iter().find(|b| b.title == "Beta").unwrap().data_dir.is_none(), "a book with no attachments plans none");
        assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
        assert!(dir.path().join("Alpha/data/notes.dat").exists(), "a dry run moves nothing");
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");

        assert_eq!(std::fs::read(dir.path().join("Moved/Alpha/data/notes.dat")).unwrap(), b"notes");
        assert_eq!(std::fs::read(dir.path().join("Moved/Alpha/data/sub/deep.dat")).unwrap(), b"deep");
        assert!(!dir.path().join("Alpha").exists(), "the old folder is gone once it is empty");
        assert_eq!(attached(&cache, alpha), ["data/notes.dat", "data/sub/deep.dat"], "the book still finds its attachments");
        assert_eq!(load_run(dir.path(), &report.run_id).unwrap().books.iter().find(|b| b.book_id == alpha).unwrap().data_dir, planned.data_dir, "journaled");

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert!(undone.skipped.is_empty(), "{undone:?}");
        assert_eq!(std::fs::read(dir.path().join("Alpha/data/notes.dat")).unwrap(), b"notes");
        assert_eq!(std::fs::read(dir.path().join("Alpha/data/sub/deep.dat")).unwrap(), b"deep");
        assert!(!dir.path().join("Moved/Alpha").exists());
        assert_eq!(attached(&cache, alpha), ["data/notes.dat", "data/sub/deep.dat"]);
    }

    /// A book in the library root keeps its attachments under `data/<uuid>`; they
    /// follow it into a folder (and back).
    #[test]
    fn a_root_books_attachments_follow_it_into_a_folder_and_back() {
        let (dir, cache) = library();
        write(&dir.path().join("Solo.pdf"), b"%PDF solo");
        index(dir.path(), &cache);
        let solo = book_with_title(&cache, "Solo");
        crate::extra_files::add_extra_files(&cache, solo, &std::collections::HashMap::from([("data/note.dat".to_string(), b"hi".to_vec())]), false).unwrap();
        assert_eq!(attached(&cache, solo), ["data/note.dat"]);

        let (_, report) = run(&cache, "Sorted/{title}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert_eq!(cache.field_for(solo, "path").unwrap().unwrap(), "Sorted");
        assert_eq!(attached(&cache, solo), ["data/note.dat"], "found in the new folder's data/");
        assert!(dir.path().join("Sorted/data/note.dat").is_file());

        undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(attached(&cache, solo), ["data/note.dat"]);
        assert!(dir.path().join("Solo.pdf").is_file());
        assert!(!dir.path().join("Sorted/data").exists());
    }

    /// What it cannot move it says: a folder shared with other books keeps its
    /// attachments, and the plan warns.
    #[test]
    fn a_shared_folders_attachments_stay_and_the_plan_says_so() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/A.pdf"), b"%PDF a");
        write(&dir.path().join("inbox/B.pdf"), b"%PDF b");
        write(&dir.path().join("inbox/data/x.dat"), b"x");
        index(dir.path(), &cache);
        let a = book_with_title(&cache, "A");

        let plan = plan(&cache, "Sorted/{title}", &Scope::Ids(vec![a])).unwrap();
        assert!(plan.books[0].data_dir.is_none());
        assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
        assert!(plan.warnings[0].reason.contains("inbox/data") && plan.warnings[0].reason.contains("share"), "{}", plan.warnings[0].reason);
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(dir.path().join("inbox/data/x.dat").is_file(), "left where it was");
    }

    /// `Reports/data` can be another book's folder: its files are that book's, not
    /// attachments, and nothing of it moves or is warned about.
    #[test]
    fn another_books_data_folder_is_not_taken() {
        let (dir, cache) = library();
        write(&dir.path().join("Reports/report.pdf"), b"%PDF report");
        write(&dir.path().join("Reports/data/dataset.pdf"), b"%PDF dataset");
        index(dir.path(), &cache);
        let report_id = book_with_title(&cache, "report");

        let plan = plan(&cache, "Moved/{title}", &Scope::Ids(vec![report_id])).unwrap();
        assert_eq!(plan.books.len(), 1);
        assert!(plan.books[0].data_dir.is_none());
        assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
        execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(dir.path().join("Reports/data/dataset.pdf").is_file());
        assert!(dir.path().join("Moved/report.pdf").is_file());
    }

    /// A book with attachments and a book without, planned into one folder (in either order):
    /// the attachments do not move into a folder the other book shares (finding 9).
    #[test]
    fn attachments_do_not_move_into_a_folder_another_planned_book_shares() {
        for (first, second) in [("Aaa", "Zzz"), ("Zzz", "Aaa")] {
            let (dir, cache) = library();
            // `Aaa` has no attachments, `Zzz` has; the names decide who is planned first.
            let (plain, with_data) = (first, second);
            write(&dir.path().join(format!("{plain}/{plain}.pdf")), b"%PDF plain");
            write(&dir.path().join(format!("{with_data}/{with_data}.pdf")), b"%PDF data");
            write(&dir.path().join(format!("{with_data}/data/n.dat")), b"n");
            index(dir.path(), &cache);

            let plan = plan(&cache, "Shared/{title}", &Scope::All).unwrap();
            assert!(plan.books.iter().all(|b| b.data_dir.is_none()), "{:?}", plan.books.iter().map(|b| (&b.title, &b.data_dir)).collect::<Vec<_>>());
            assert_eq!(plan.warnings.len(), 1, "{:?}", plan.warnings);
            assert!(plan.warnings[0].reason.contains("share"), "{}", plan.warnings[0].reason);
            let report = execute(&cache, &plan, &JobControl::new()).unwrap();
            assert!(report.failed.is_empty(), "{report:?}");
            assert!(!dir.path().join("Shared/data").exists());
            assert!(dir.path().join(format!("{with_data}/data/n.dat")).is_file(), "left where it was");
        }
    }

    /// Two books with attachments moving into one folder: neither can own the `data/` there, so both stay.
    #[test]
    fn two_books_cannot_take_the_same_destination_data_folder() {
        let (dir, cache) = library();
        write(&dir.path().join("One/One.pdf"), b"%PDF one");
        write(&dir.path().join("One/data/a.dat"), b"a");
        write(&dir.path().join("Two/Two.pdf"), b"%PDF two");
        write(&dir.path().join("Two/data/b.dat"), b"b");
        index(dir.path(), &cache);

        let plan = plan(&cache, "Shared/{title}", &Scope::All).unwrap();
        assert_eq!(plan.books.iter().filter(|b| b.data_dir.is_some()).count(), 0, "{:?}", plan.books.iter().map(|b| &b.data_dir).collect::<Vec<_>>());
        assert_eq!(plan.warnings.len(), 2, "{:?}", plan.warnings);
        let report = execute(&cache, &plan, &JobControl::new()).unwrap();
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(!dir.path().join("Shared/data").exists());
        for p in ["One/data/a.dat", "Two/data/b.dat"] {
            assert!(dir.path().join(p).is_file(), "{p} stayed where it was");
        }
    }

    #[test]
    fn the_disk_view_remembers_link_checks_per_prefix() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("real/inner")).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        // A symlink needs a privilege on some systems; where it cannot be made, only the plain cases run.
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(elsewhere.path(), dir.path().join("link")).is_ok();
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(elsewhere.path(), dir.path().join("link")).is_ok();
        #[cfg(not(any(unix, windows)))]
        let linked = false;
        let mut view = DiskView::new(dir.path());
        for rel in ["real", "real/inner", "real/inner/new", "missing/deeper", "", "link", "link/inner", "link/inner/deeper"] {
            assert_eq!(view.passes_through_link(rel), passes_through_link(dir.path(), rel), "{rel} (symlink made: {linked})");
        }
        if linked {
            assert!(view.passes_through_link("link/inner"));
        }
    }

    /// Many books into one target folder, with files already there (one differing
    /// only in case from a target): every target is free or suffixed, nothing
    /// planned lands on an existing file, and the plan is stable.
    #[test]
    fn a_big_plan_avoids_every_existing_file_and_every_other_target() {
        let (dir, cache) = library();
        for i in 0..40 {
            write(&dir.path().join(format!("in{i}/Same.pdf")), format!("%PDF {i}").as_bytes());
        }
        for i in 0..40 {
            write(&dir.path().join(format!("in{}/Other{i}.pdf", i % 4)), format!("%PDF o{i}").as_bytes());
        }
        write(&dir.path().join("Sorted/Same.pdf"), b"%PDF already here");
        write(&dir.path().join("Sorted/other3.PDF"), b"%PDF already here too");
        index(dir.path(), &cache);

        let first = plan(&cache, "Sorted/{title}", &Scope::All).unwrap();
        assert_eq!(first.fingerprint, plan(&cache, "Sorted/{title}", &Scope::All).unwrap().fingerprint);
        let mut taken: HashSet<String> = HashSet::new();
        for file in first.books.iter().flat_map(|b| b.files.iter()) {
            let key = lexmechanic_utils::filenames::path_key(&file.to);
            assert!(taken.insert(key), "two planned moves to {}", file.to);
            // Whatever the platform's file system calls "the same name" is not planned over.
            if dir.path().join(&file.to).exists() {
                assert_eq!(file.to, file.from, "{} already exists and is not the book's own", file.to);
            }
        }
        assert!(first.conflicts() >= 40, "all the `Same` books but one, and the case-clash, are numbered: {}", first.conflicts());
        let report = execute(&cache, &first, &JobControl::new()).unwrap();
        assert_eq!(report.failed.len(), 0, "{report:?}");
        assert_eq!(std::fs::read(dir.path().join("Sorted/Same.pdf")).unwrap(), b"%PDF already here");
    }

    #[test]
    fn the_result_survives_a_rebuild_from_the_change_log() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");
        let (_, report) = run(&cache, "Sorted/{title}");
        assert_eq!(report.books_moved, 1);

        cache.rebuild_from_change_log().unwrap();
        let alpha_again = book_with_title(&cache, "Alpha");
        assert_eq!(cache.field_for(alpha_again, "path").unwrap().unwrap(), "Sorted", "book {alpha}");
        assert!(dir.path().join(format!("{}/Alpha.pdf", cache.field_for(alpha_again, "path").unwrap().unwrap())).is_file());
    }

    #[test]
    fn undo_puts_everything_back_and_a_run_cannot_be_undone_twice() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);
        let (_, report) = run(&cache, "Sorted/{title}");
        assert_eq!(report.books_moved, 2);
        assert!(!dir.path().join("inbox").exists(), "the emptied folder is removed");

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(undone.books_restored, 2, "{undone:?}");
        assert!(dir.path().join("inbox/Alpha.pdf").is_file());
        assert!(dir.path().join("inbox/Beta.pdf").is_file());
        assert!(!dir.path().join("Sorted").exists());
        let alpha = book_with_title(&cache, "Alpha");
        assert_eq!(cache.field_for(alpha, "path").unwrap().unwrap(), "inbox");
        assert!(undo(&cache, &report.run_id, &JobControl::new()).is_err());
        assert!(list_runs(dir.path())[0].3, "the run is marked undone");
    }

    #[test]
    fn a_book_moved_since_is_skipped_by_undo() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        index(dir.path(), &cache);
        let (_, report) = run(&cache, "Sorted/{title}");
        let alpha = book_with_title(&cache, "Alpha");
        cache.set_book_path(alpha, "Elsewhere").unwrap();
        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(undone.books_restored, 0);
        assert_eq!(undone.skipped.len(), 1);
        assert!(dir.path().join("Sorted/Alpha.pdf").is_file(), "nothing was moved");
    }

    #[test]
    fn a_failed_rename_rolls_the_books_other_files_back() {
        let (dir, cache) = library();
        // on its second step must leave the first file where it was.
        write(&dir.path().join("x/one.txt"), b"1");
        let pairs = vec![("x/one.txt".to_string(), "y/one.txt".to_string()), ("x/missing.txt".to_string(), "y/missing.txt".to_string())];
        assert!(move_files(&cache, &pairs).is_err());
        assert!(dir.path().join("x/one.txt").is_file());
        assert!(!dir.path().join("y/one.txt").exists());
    }

    #[test]
    fn a_template_cannot_escape_the_library() {
        let (dir, cache) = library();
        write(&dir.path().join("Alpha.pdf"), b"%PDF alpha");
        index(dir.path(), &cache);
        let plan = plan(&cache, "../../{title}", &Scope::All).unwrap();
        for f in plan.books.iter().flat_map(|b| &b.files) {
            assert!(!f.to.contains(".."), "{f:?}");
        }
    }

    #[test]
    fn the_path_prefix_scope_selects_a_folder() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("other/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);
        let p = plan(&cache, "Sorted/{title}", &Scope::PathPrefix("inbox".into())).unwrap();
        assert_eq!(p.books.len(), 1);
        assert_eq!(p.books[0].title, "Alpha");
    }

    // ---- Move to a folder (tranche 16) ----

    #[test]
    fn a_folder_is_validated_relative_to_the_library() {
        assert_eq!(validate_folder("Sorted/PDFs").unwrap(), "Sorted/PDFs");
        assert_eq!(validate_folder(r"  Sorted\Two//./Three ").unwrap(), "Sorted/Two/Three");
        for bad in ["", "   ", ".", "..", "../x", "a/../../b", "a/..", "/abs", r"\\server\share", "C:/x", r"C:\x", "x:y", "a/CON", "nul.txt/b", "a/.caltrash", ".lexmechanic/x", "bad*name", "a/b?", "trailing.", "tab\there"] {
            assert!(validate_folder(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn the_folder_plan_changes_nothing_keeps_names_and_names_every_move() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");

        let p = plan_to(&cache, Target::Folder("Keep/These"), &Scope::Ids(vec![alpha])).unwrap();
        assert_eq!(p.books.len(), 1);
        assert_eq!(p.books[0].files[0].to, "Keep/These/Alpha.pdf");
        assert_eq!(p.books[0].new_names["PDF"], "Alpha", "the file keeps its name");
        assert!(dir.path().join("inbox/Alpha.pdf").exists() && !dir.path().join("Keep").exists(), "a dry run moves nothing");
        assert_eq!(p.fingerprint, plan_to(&cache, Target::Folder("Keep/These"), &Scope::Ids(vec![alpha])).unwrap().fingerprint);
        assert_ne!(p.fingerprint, plan_to(&cache, Target::Folder("Other"), &Scope::Ids(vec![alpha])).unwrap().fingerprint);
    }

    #[test]
    fn move_to_a_folder_creates_it_updates_the_database_and_is_undone_by_the_organize_undo() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        write(&dir.path().join("inbox/Beta.pdf"), b"%PDF beta");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");

        let p = plan_to(&cache, Target::Folder("Keep/These"), &Scope::Ids(vec![alpha])).unwrap();
        let report = execute(&cache, &p, &JobControl::new()).unwrap();
        assert_eq!((report.books_moved, report.failed.len()), (1, 0), "{report:?}");
        assert!(dir.path().join("Keep/These/Alpha.pdf").is_file());
        assert!(dir.path().join("inbox/Beta.pdf").is_file(), "the neighbour stays");
        assert_eq!(cache.field_for(alpha, "path").unwrap().unwrap(), "Keep/These");
        // Already there: nothing to do.
        assert!(plan_to(&cache, Target::Folder("Keep/These"), &Scope::Ids(vec![alpha])).unwrap().books.is_empty());

        let undone = undo(&cache, &report.run_id, &JobControl::new()).unwrap();
        assert_eq!(undone.books_restored, 1, "{undone:?}");
        assert!(dir.path().join("inbox/Alpha.pdf").is_file());
        assert!(!dir.path().join("Keep").exists(), "the folder the move made is gone again");
        assert_eq!(cache.field_for(alpha, "path").unwrap().unwrap(), "inbox");
    }

    #[test]
    fn moving_two_books_to_one_folder_numbers_the_second_and_overwrites_nothing() {
        let (dir, cache) = library();
        write(&dir.path().join("a/Same.pdf"), b"%PDF one");
        write(&dir.path().join("b/Same.pdf"), b"%PDF two");
        write(&dir.path().join("Dest/Same.pdf"), b"%PDF already here");
        index(dir.path(), &cache);
        let ids: Vec<i32> = cache.all_book_ids().unwrap().into_iter().filter(|id| cache.field_for(*id, "path").unwrap().unwrap_or_default() != "Dest").collect();
        assert_eq!(ids.len(), 2);

        let p = plan_to(&cache, Target::Folder("Dest"), &Scope::Ids(ids)).unwrap();
        assert_eq!(p.conflicts(), 2, "both find the name taken: one by the file there, one by the first book");
        let report = execute(&cache, &p, &JobControl::new()).unwrap();
        assert_eq!(report.failed.len(), 0, "{report:?}");
        let mut names: Vec<String> = std::fs::read_dir(dir.path().join("Dest")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        assert_eq!(names, ["Same (2).pdf", "Same (3).pdf", "Same.pdf"]);
        assert_eq!(std::fs::read(dir.path().join("Dest/Same.pdf")).unwrap(), b"%PDF already here");
    }

    #[test]
    fn a_folder_that_leaves_the_library_through_a_link_is_refused() {
        let (dir, cache) = library();
        let outside = tempfile::tempdir().unwrap();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        index(dir.path(), &cache);
        let link = dir.path().join("Escape");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(outside.path(), &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.path(), &link).is_ok();
        if !made {
            return; // no permission to make a symlink here
        }
        assert!(plan_to(&cache, Target::Folder("Escape/Sub"), &Scope::All).is_err());
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none(), "nothing was written outside");
    }

    #[test]
    fn a_folder_move_survives_a_rebuild_from_the_change_log() {
        let (dir, cache) = library();
        write(&dir.path().join("inbox/Alpha.pdf"), b"%PDF alpha");
        index(dir.path(), &cache);
        let alpha = book_with_title(&cache, "Alpha");
        let p = plan_to(&cache, Target::Folder("Keep"), &Scope::Ids(vec![alpha])).unwrap();
        assert_eq!(execute(&cache, &p, &JobControl::new()).unwrap().books_moved, 1);

        cache.rebuild_from_change_log().unwrap();
        let again = book_with_title(&cache, "Alpha");
        assert_eq!(cache.field_for(again, "path").unwrap().unwrap(), "Keep");
        assert!(dir.path().join("Keep/Alpha.pdf").is_file());
    }
}
