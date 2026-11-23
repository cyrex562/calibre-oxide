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
//!   sidecars are left where they are (covers live under `.calibre-oxide`);
//!   an old folder is removed only once it is empty (`remove_dir`, never
//!   recursive).
//! - **Journaled.** Each rename is `LibraryHandle::rename_atomic`. The
//!   database follows through `Cache::set_book_path` and
//!   `Cache::set_format_name`, which write the change log, so a rebuild
//!   reproduces the result.
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
//! - **Undoable.** Every run writes `.calibre-oxide/organize-runs/<run_id>.json`
//!   with each book's old and new place; [`undo`] reverses it, skipping books
//!   changed since.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use calibre_utils::filenames::sanitize_file_name;
use calibre_utils::formatter::string_format;

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
    } else {
        format!("{dir}/{file}")
    }
}

fn clip(component: &str) -> String {
    component.chars().take(120).collect::<String>().trim().trim_end_matches('.').to_string()
}

/// The template's `/`-separated components for one book: sanitized, with empty
/// and `..` ones dropped, falling back to the book id.
fn components(cache: &Cache, book_id: i32, template: &str) -> Result<Vec<String>> {
    let values = CacheValueSource::new(cache, book_id)?;
    let functions = CacheFunctions::new(cache, book_id);
    let rendered = string_format::evaluate_template(template, &values, &CacheCatalog, &functions).map_err(|e| anyhow!("{e}"))?;
    let mut parts: Vec<String> = rendered.split(['/', '\\']).map(str::trim).filter(|s| !s.is_empty()).map(sanitize_file_name).map(|s| clip(&s)).filter(|s| !s.is_empty() && s != "..").collect();
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
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

fn in_scope(cache: &Cache, scope: &Scope) -> Result<Vec<i32>> {
    let mut ids = match scope {
        Scope::All => cache.all_book_ids()?,
        Scope::Ids(ids) => ids.clone(),
        Scope::PathPrefix(prefix) => {
            let prefix = prefix.trim_matches(['/', '\\']).replace('\\', "/").to_lowercase();
            let mut out = Vec::new();
            for id in cache.all_book_ids()? {
                let path = cache.field_for(id, "path")?.unwrap_or_default().replace('\\', "/").to_lowercase();
                if prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/")) {
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
/// is not at or under `from`). Segment-wise and case-insensitive on `from`.
fn rebase_path(path: &str, from: &str, to: &str) -> Option<String> {
    let (p, f) = (path.trim_matches('/'), from.trim_matches('/'));
    if p.eq_ignore_ascii_case(f) {
        return Some(to.to_string());
    }
    let head = p.get(..f.len())?;
    let rest = p.get(f.len()..)?.strip_prefix('/')?;
    head.eq_ignore_ascii_case(f).then(|| join_rel(to, rest))
}

/// Names Windows will not let a file or folder have.
const RESERVED: [&str; 22] = ["con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9"];

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
        let stem = part.split('.').next().unwrap_or(part).to_lowercase();
        if RESERVED.contains(&stem.as_str()) {
            return Err(format!("`{part}` is a reserved name"));
        }
        let lower = part.to_lowercase();
        if lower == crate::constants::LIBRARY_HANDLE_DIR_NAME.to_lowercase() || lower == ".caltrash" || lower == ".calibre-oxide" {
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
    let mut already = 0usize;
    // Lower-cased relative targets taken by earlier books in this plan.
    let mut taken: HashSet<String> = HashSet::new();

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
            let found = [format.to_lowercase(), format.to_uppercase()].into_iter().find(|ext| library.join(join_rel(&old_path, &format!("{name}.{ext}"))).is_file());
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
                let (to_l, from_l) = (to.to_lowercase(), from.to_lowercase());
                to_l == from_l || (!taken.contains(&to_l) && !library.join(&to).exists())
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
            taken.insert(to.to_lowercase());
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
        books.push(PlannedBook { book_id, title, old_path, new_path, old_names, new_names, files, suffixed });
    }
    let fingerprint = fingerprint(&label, &books);
    Ok(OrganizePlan { template: label, books, skipped, already_organized: already, fingerprint })
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrganizeRun {
    pub run_id: String,
    pub template: String,
    pub created_at: i64,
    pub books: Vec<PlannedBook>,
    #[serde(default)]
    pub undone_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct OrganizeReport {
    pub run_id: String,
    pub books_planned: usize,
    pub books_moved: usize,
    pub files_moved: usize,
    pub failed: Vec<Skipped>,
    pub cancelled: bool,
}

fn runs_dir(library: &Path) -> std::path::PathBuf {
    library.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("organize-runs")
}

fn save_run(library: &Path, run: &OrganizeRun) -> Result<()> {
    let dir = runs_dir(library);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", run.run_id));
    let temp = path.with_extension("json.partial");
    std::fs::write(&temp, serde_json::to_vec_pretty(run)?)?;
    std::fs::rename(&temp, &path)?;
    Ok(())
}

pub fn load_run(library: &Path, run_id: &str) -> Result<OrganizeRun> {
    if run_id.is_empty() || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(anyhow!("not a run id"));
    }
    let text = std::fs::read_to_string(runs_dir(library).join(format!("{run_id}.json"))).with_context(|| format!("no organize run {run_id}"))?;
    Ok(serde_json::from_str(&text)?)
}

/// Runs, newest first: `(run_id, created_at, books, undone)`.
pub fn list_runs(library: &Path) -> Vec<(String, i64, usize, bool)> {
    let mut out: Vec<(String, i64, usize, bool)> = std::fs::read_dir(runs_dir(library))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<OrganizeRun>(&t).ok())
        .map(|r| (r.run_id, r.created_at, r.books.len(), r.undone_at.is_some()))
        .collect();
    out.sort_by_key(|r| std::cmp::Reverse(r.1));
    out
}

fn new_run_id() -> String {
    format!("org-{}-{}", crate::trash::now_secs(), &uuid::Uuid::new_v4().simple().to_string()[..4])
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

/// Moves the book's files, all or none. Returns the number moved.
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
            // Case-only renames name the same file on a case-insensitive volume.
            if to_abs.exists() && from.to_lowercase() != to.to_lowercase() {
                return Err(anyhow!("{to} already exists"));
            }
            if let Some(parent) = to_abs.parent() {
                std::fs::create_dir_all(parent)?;
            }
            handle.rename_atomic(&from_abs, &to_abs).map_err(|e| anyhow!("could not move {from} to {to} (a rename cannot cross volumes): {e}"))
        })();
        if let Err(e) = step {
            for (f, t) in done.into_iter().rev() {
                let _ = handle.rename_atomic(&library.join(t), &library.join(f));
            }
            return Err(e);
        }
        done.push((from, to));
    }
    Ok(())
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

fn record(cache: &Cache, book_id: i32, path: &str, names: &BTreeMap<String, String>, current_path: &str) -> Result<()> {
    for (format, stem) in names {
        cache.set_format_name(book_id, format, stem)?;
    }
    if path != current_path {
        cache.set_book_path(book_id, path)?;
    }
    Ok(())
}

/// Carries out a plan made by [`plan`] (the caller has checked its
/// fingerprint). A cancel stops between books.
pub fn execute(cache: &Cache, plan: &OrganizePlan, control: &JobControl) -> Result<OrganizeReport> {
    let library = cache.backend.library_path.clone();
    let mut run = OrganizeRun { run_id: new_run_id(), template: plan.template.clone(), created_at: crate::trash::now_secs(), ..Default::default() };
    let mut report = OrganizeReport { run_id: run.run_id.clone(), books_planned: plan.books.len(), ..Default::default() };
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
        if let Err(e) = move_files(cache, &pairs) {
            report.failed.push(Skipped { book_id: book.book_id, reason: format!("{e:#}") });
            continue;
        }
        if let Err(e) = record(cache, book.book_id, &book.new_path, &book.new_names, &book.old_path) {
            // The files moved and the rows did not: put the files back.
            let back: Vec<(String, String)> = pairs.iter().map(|(f, t)| (t.clone(), f.clone())).collect();
            let _ = move_files(cache, &back);
            report.failed.push(Skipped { book_id: book.book_id, reason: format!("the library could not record the move: {e:#}") });
            continue;
        }
        old_dirs.insert(book.old_path.clone());
        report.books_moved += 1;
        report.files_moved += pairs.len();
        run.books.push(book.clone());
        if run.books.len() % 100 == 0 {
            let _ = save_run(&library, &run);
        }
    }
    for dir in &old_dirs {
        prune_empty_dirs(&library, dir);
    }
    if !run.books.is_empty() {
        save_run(&library, &run).context("recording the run")?;
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
}

/// Puts a run's books back where they were.
pub fn undo(cache: &Cache, run_id: &str, control: &JobControl) -> Result<UndoReport> {
    let library = cache.backend.library_path.clone();
    let mut run = load_run(&library, run_id)?;
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
        if current != book.new_path {
            report.skipped.push(Skipped { book_id: book.book_id, reason: "the book has been moved or removed since".into() });
            continue;
        }
        let back: Vec<(String, String)> = book.files.iter().map(|f| (f.to.clone(), f.from.clone())).collect();
        if let Err(e) = move_files(cache, &back) {
            report.skipped.push(Skipped { book_id: book.book_id, reason: format!("{e:#}") });
            continue;
        }
        if let Err(e) = record(cache, book.book_id, &book.old_path, &book.old_names, &book.new_path) {
            let fwd: Vec<(String, String)> = back.iter().map(|(f, t)| (t.clone(), f.clone())).collect();
            let _ = move_files(cache, &fwd);
            report.skipped.push(Skipped { book_id: book.book_id, reason: format!("the library could not record the move: {e:#}") });
            continue;
        }
        new_dirs.insert(book.new_path.clone());
        report.books_restored += 1;
        report.files_restored += back.len();
    }
    for dir in &new_dirs {
        prune_empty_dirs(&library, dir);
    }
    if report.skipped.is_empty() && !control.is_cancelled() {
        run.undone_at = Some(crate::trash::now_secs());
        save_run(&library, &run)?;
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
        for bad in ["", "   ", ".", "..", "../x", "a/../../b", "a/..", "/abs", r"\\server\share", "C:/x", r"C:\x", "x:y", "a/CON", "nul.txt/b", "a/.caltrash", ".calibre-oxide/x", "bad*name", "a/b?", "trailing.", "tab\there"] {
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
