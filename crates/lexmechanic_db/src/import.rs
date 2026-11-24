//! Importing a folder tree **into** the library, copying: the job behind
//! "Add folder" (port of calibre's `recursive_import` /
//! `import_book_directory[_multiple]`, with the weaknesses of the first port
//! removed).
//!
//! That first port (`adding.rs`) took the title from the filename and the
//! author as "Unknown", dropped walk errors silently, aborted the whole run
//! on the first failure, copied every file flat into the library root and
//! had no progress or cancel. This is the NAS-scale version: it is a job.
//!
//! # What it does
//!
//! 1. **Walks** the source recursively. Every directory or entry that cannot
//!    be read is *collected* in the report, never dropped; the report says
//!    whether the walk was conclusive. Symlinked directories are not followed.
//!    The library's own folder is skipped when it lies inside the source, and a
//!    source inside the library is refused (that is what *scan* is for).
//! 2. **Groups** files into books, calibre's two modes ([`ImportMode`]):
//!    * *one book per folder* -- every book file in a folder is one book,
//!      its formats; two files with the same extension in a folder cannot
//!      both be formats of it, so the later one is **reported**, not lost;
//!    * *multiple books per folder* -- files sharing a filename stem
//!      (case-insensitive) in the same folder are one book's formats.
//! 3. **Streams each file once**: read from the source in 1 MiB chunks,
//!    hashed with BLAKE3 and written to a staging file under
//!    `.lexmechanic/import-staging` in the same pass. The staged copy is
//!    then re-read and its size and BLAKE3 compared with what was read from
//!    the source: a copy that does not match is discarded and reported.
//!    The source is only ever *read*.
//! 4. **Decides duplicates** ([`DuplicatePolicy`]) from the hash, before the
//!    expensive part.
//! 5. **Reads real metadata**: the embedded metadata of the staged copy
//!    (title, authors, series, publisher, tags, identifiers, languages,
//!    comments, rating, cover), falling back to the filename for a file with
//!    nothing in it; then calibre-style `.opf` sidecars in the *source* folder
//!    -- `<stem>.opf` beside a book file, or `metadata.opf` when the book is
//!    alone in its folder -- which take precedence, with `cover.jpg`/`cover.png`
//!    beside the `metadata.opf` as the cover when the book has none. Duplicate
//!    checks use the final values.
//! 6. **Writes in batches**: [`Cache::import_books_batch`] creates the
//!    books in one transaction and renames the verified staged files into
//!    calibre's `Author/Title (id)/Title - Author.ext` layout.
//!
//! # Resumable
//!
//! A ledger in the checksum sidecar records what became of every source file
//! (path, size, mtime, content hash, outcome). A rerun skips an already
//! imported file **without reading it**, when its size and mtime are
//! unchanged; a file that changed is read and decided afresh. A file skipped
//! as a duplicate under one policy is skipped again only under the same
//! policy -- change the policy and it is reconsidered. Cancelling loses at
//! most the batch in flight: a batch is committed whole or not at all.
//!
//! # Duplicate policies
//!
//! All but [`DuplicatePolicy::AddAnyway`] first skip a file whose **content**
//! (BLAKE3) is already in the library or was already imported earlier in this
//! run. On top of that:
//! * [`DuplicatePolicy::SkipTitleAuthor`] skips a *book* whose title and authors
//!   match an existing book's after normalisation ([`crate::dedupe`]: case,
//!   diacritics, punctuation, leading articles, `(z-lib.org)`-style noise,
//!   `Last, First` against `First Last`, initials; *any* shared author
//!   matches); a book with no real author is never matched on title alone,
//!   since "scan0042" is a filename, not a title. Fuzzy matching is not used
//!   here: skipping a file is irreversible enough to want strong evidence;
//! * [`DuplicatePolicy::AddFormatsToExisting`] (calibre's "automerge") adds a
//!   matching book's new formats to the existing record instead of making a
//!   second one -- formats it already has are skipped, never replaced.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::{Cache, ImportFile, ImportItem};
use crate::checksums::LedgerRow;
use crate::constants::LIBRARY_HANDLE_DIR_NAME;
use crate::scan_pipeline::{hash_stream, FileDigest, JobControl, PipelineOptions, HASH_BUFFER_BYTES, MAX_REPORTED};

/// Under `.lexmechanic`: where verified copies wait to be renamed into place.
pub const STAGING_DIR_NAME: &str = "import-staging";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ImportMode {
    /// calibre's "assume one book per folder".
    OneBookPerFolder,
    /// calibre's "assume multiple books per folder": formats grouped by
    /// filename stem. Spelled `multiple_books_per_folder` on the wire, as the
    /// route documents and the web UI sends; `multiple_per_folder` is still
    /// read so jobs saved before the rename keep resuming.
    #[default]
    #[serde(rename = "multiple_books_per_folder", alias = "multiple_per_folder")]
    MultiplePerFolder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DuplicatePolicy {
    /// Skip a file whose content is already in the library. The default.
    #[default]
    SkipExactContent,
    /// Also skip a book whose title and authors match an existing book's.
    SkipTitleAuthor,
    /// Add a matching book's new formats to the existing record
    /// (calibre's "automerge").
    AddFormatsToExisting,
    /// Add everything, duplicates included.
    AddAnyway,
}

impl DuplicatePolicy {
    fn name(self) -> &'static str {
        match self {
            DuplicatePolicy::SkipExactContent => "skip_exact_content",
            DuplicatePolicy::SkipTitleAuthor => "skip_title_author",
            DuplicatePolicy::AddFormatsToExisting => "add_formats_to_existing",
            DuplicatePolicy::AddAnyway => "add_anyway",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportOptions {
    pub mode: ImportMode,
    pub policy: DuplicatePolicy,
    /// Descend into subfolders. Off: only the folder itself.
    pub recursive: bool,
    /// Keep the source's folders: a book from `<source>/Fiction/SciFi/Dune.pdf`
    /// goes to `<library>/Fiction/SciFi/Dune.pdf` instead of calibre's
    /// `Author/Title (id)/Title - Author.pdf`. A name already taken there gets a
    /// number (`Dune (1).pdf`); nothing is replaced.
    pub keep_structure: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions { mode: ImportMode::default(), policy: DuplicatePolicy::default(), recursive: true, keep_structure: false }
    }
}

/// The folder of `dir` under `source`, as a library-relative path: each part
/// made safe as a folder name (characters a platform refuses become `_`, a
/// trailing dot or space goes, a reserved name gets a `_`), `""` for `source`
/// itself. A part that would be the library's own state folder is renamed too,
/// and so is `data`: a book's attached files live in `<its folder>/data`, so a
/// source folder of that name would hold another book's files in the
/// first book's data directory.
pub fn kept_folder(source: &Path, dir: &Path) -> String {
    let Ok(rel) = dir.strip_prefix(source) else { return String::new() };
    let mut parts: Vec<String> = Vec::new();
    for part in rel.components().filter_map(|c| c.as_os_str().to_str()) {
        let mut name: String = part.chars().map(|c| if c.is_control() || "<>:\"|?*/\\".contains(c) { '_' } else { c }).collect();
        name = name.trim().trim_end_matches(['.', ' ']).to_string();
        if name.is_empty() {
            name = "_".to_string();
        }
        let lower = name.to_lowercase();
        if lexmechanic_utils::filenames::is_windows_reserved_name(&name) || lower == LIBRARY_HANDLE_DIR_NAME.to_lowercase() || lower == crate::constants::LEGACY_LIBRARY_HANDLE_DIR_NAME || lower == ".caltrash" || lower == crate::constants::DATA_DIR_NAME {
            name = format!("_{name}");
        }
        parts.push(name.chars().take(120).collect());
    }
    parts.join("/")
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PathProblem {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AddedBook {
    pub book_id: i32,
    pub title: String,
    pub authors: Vec<String>,
    pub formats: Vec<String>,
    /// Where it was imported from.
    pub source: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Duplicate {
    pub path: String,
    /// The book already in the library, when known.
    pub duplicate_of: Option<i32>,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct ImportReport {
    pub source: String,
    pub files_seen: usize,
    pub books_found: usize,
    pub books_added: usize,
    pub formats_added_to_existing: usize,
    pub skipped_already_imported: usize,
    pub skipped_duplicate_content: usize,
    pub skipped_duplicate_title: usize,
    /// Capped at [`MAX_REPORTED`]; the counts above are exact.
    pub added: Vec<AddedBook>,
    pub duplicates: Vec<Duplicate>,
    /// A second file of the same format in a one-book-per-folder folder.
    pub skipped_same_format: Vec<String>,
    /// Files that could not be imported, each with why.
    pub failed: Vec<PathProblem>,
    /// Folders or entries the walk could not read.
    pub walk_errors: Vec<PathProblem>,
    /// Books skipped because they had no author to match on.
    pub unmatched_without_author: usize,
    pub cancelled: bool,
    /// False if the walk had errors or the job was cancelled: the source was
    /// not entirely seen.
    pub conclusive: bool,
}

fn capped<T>(list: &mut Vec<T>, item: T) {
    if list.len() < MAX_REPORTED {
        list.push(item);
    }
}

// ---------------------------------------------------------------------
// Walking
// ---------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Candidate {
    path: PathBuf,
    dir: PathBuf,
    stem: String,
    ext: String,
    size: u64,
    mtime_ns: i64,
}

fn mtime_ns(meta: &fs::Metadata) -> i64 {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as i64).unwrap_or(0)
}

fn walk(root: &Path, recursive: bool, skip: Option<&Path>, extensions: &HashSet<String>, control: &JobControl, errors: &mut Vec<PathProblem>) -> Vec<Candidate> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if control.is_cancelled() {
            break;
        }
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                errors.push(PathProblem { path: dir.to_string_lossy().to_string(), error: e.to_string() });
                continue;
            }
        };
        let mut subdirs = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    errors.push(PathProblem { path: dir.to_string_lossy().to_string(), error: format!("an entry could not be read: {e}") });
                    continue;
                }
            };
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else { continue };
            let lower = name.to_ascii_lowercase();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(e) => {
                    errors.push(PathProblem { path: path.to_string_lossy().to_string(), error: e.to_string() });
                    continue;
                }
            };
            if file_type.is_dir() {
                if recursive && !crate::scan::is_junk_dir(&lower) && skip.is_none_or(|s| path != s) {
                    subdirs.push(path);
                }
                continue;
            }
            if file_type.is_symlink() && path.is_dir() {
                continue;
            }
            if crate::scan::is_junk_name(&lower) {
                continue;
            }
            let Some(ext) = path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()) else { continue };
            if !extensions.contains(&ext) {
                continue;
            }
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(e) => {
                    errors.push(PathProblem { path: path.to_string_lossy().to_string(), error: e.to_string() });
                    continue;
                }
            };
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(&name).to_string();
            control.seen.fetch_add(1, Ordering::Relaxed);
            out.push(Candidate { path, dir: dir.clone(), stem, ext, size: meta.len(), mtime_ns: mtime_ns(&meta) });
        }
        subdirs.sort();
        // Depth-first in sorted order: the stack pops last-first.
        stack.extend(subdirs.into_iter().rev());
    }
    out
}

/// Formats whose metadata is richest first, so the book's title and cover
/// come from the best file in the group.
fn metadata_priority(ext: &str) -> usize {
    ["epub", "azw3", "kepub", "mobi", "azw", "fb2", "docx", "odt", "djvu", "pdf", "cbz", "txt"].iter().position(|e| *e == ext).unwrap_or(100)
}

struct Group {
    files: Vec<Candidate>,
    /// The only book in its folder, so a `metadata.opf` there is about this
    /// book (always true in one-book-per-folder mode).
    solo_in_dir: bool,
}

fn group(candidates: Vec<Candidate>, mode: ImportMode, skipped_same_format: &mut Vec<String>) -> Vec<Group> {
    let mut by_key: Vec<(String, Vec<Candidate>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for c in candidates {
        let key = match mode {
            ImportMode::OneBookPerFolder => c.dir.to_string_lossy().to_string(),
            ImportMode::MultiplePerFolder => format!("{}\u{0}{}", c.dir.to_string_lossy(), c.stem.to_lowercase()),
        };
        match index.get(&key) {
            Some(i) => by_key[*i].1.push(c),
            None => {
                index.insert(key.clone(), by_key.len());
                by_key.push((key, vec![c]));
            }
        }
    }
    let mut groups = Vec::new();
    for (_, files) in by_key {
        let mut seen_ext = HashSet::new();
        let mut kept = Vec::new();
        for f in files {
            if seen_ext.insert(f.ext.clone()) {
                kept.push(f);
            } else {
                capped(skipped_same_format, f.path.to_string_lossy().to_string());
            }
        }
        kept.sort_by(|a, b| metadata_priority(&a.ext).cmp(&metadata_priority(&b.ext)).then(a.ext.cmp(&b.ext)));
        groups.push(Group { files: kept, solo_in_dir: true });
    }
    let mut per_dir: HashMap<PathBuf, usize> = HashMap::new();
    for g in &groups {
        *per_dir.entry(g.files[0].dir.clone()).or_default() += 1;
    }
    for g in &mut groups {
        g.solo_in_dir = per_dir[&g.files[0].dir] == 1;
    }
    groups
}

/// What a book's sidecars say, read from the **source** folder (the staged
/// copy has no neighbours).
#[derive(Default)]
struct Sidecar {
    opf: Option<lexmechanic_ebooks::metadata::MetaInformation>,
    cover: Option<(String, Vec<u8>)>,
}

/// Cap on a sidecar cover: a sane cover is a few hundred KiB.
const SIDECAR_COVER_MAX: u64 = 32 * 1024 * 1024;

/// calibre-style sidecars of a group: `<stem>.opf` next to a book file, and
/// `metadata.opf` in the folder when the book is alone in it (what calibre's
/// own library layout and `calibredb export` produce). The per-file sidecar
/// wins over the folder-level one. A `cover.jpg`/`cover.png` next to the
/// `metadata.opf` that was used is the fallback cover.
fn read_sidecars(g: &Group) -> Sidecar {
    let mut out = Sidecar::default();
    let parse = |p: &Path| -> Option<lexmechanic_ebooks::metadata::MetaInformation> {
        let text = fs::read_to_string(p).ok()?;
        lexmechanic_ebooks::opf::parse_opf(&text).ok()
    };
    for f in &g.files {
        let p = f.dir.join(format!("{}.opf", f.stem));
        if let Some(m) = parse(&p) {
            out.opf = Some(m);
            break;
        }
    }
    if out.opf.is_none() && g.solo_in_dir {
        let dir = &g.files[0].dir;
        if let Some(m) = parse(&dir.join("metadata.opf")) {
            out.opf = Some(m);
        }
    }
    if out.opf.is_some() && g.solo_in_dir {
        let dir = &g.files[0].dir;
        for name in ["cover.jpg", "cover.jpeg", "cover.png"] {
            let p = dir.join(name);
            if fs::metadata(&p).map(|m| m.is_file() && m.len() > 0 && m.len() <= SIDECAR_COVER_MAX).unwrap_or(false) {
                if let Ok(bytes) = fs::read(&p) {
                    out.cover = Some((p.extension().and_then(|e| e.to_str()).unwrap_or("jpg").to_string(), bytes));
                    break;
                }
            }
        }
    }
    out
}

/// Lays `found`'s real values over `mi`: a value `found` does not have (or
/// has only as the parser's placeholder -- "Unknown", language "und") leaves
/// `mi` as it was. Used for embedded metadata, then again for the OPF
/// sidecar, which therefore wins.
fn overlay(mi: &mut lexmechanic_ebooks::metadata::MetaInformation, found: &lexmechanic_ebooks::metadata::MetaInformation) {
    // Junk titles/authors (see `junk_metadata`) count as absent, so the
    // filename the caller already put in `mi` stays.
    if let Some(title) = crate::junk_metadata::clean_title(&found.title).filter(|t| t != "Unknown") {
        mi.title = title;
    }
    let real: Vec<String> = crate::junk_metadata::clean_authors(&found.authors).into_iter().filter(|a| a != "Unknown").collect();
    if !real.is_empty() {
        mi.authors = real;
    }
    if found.title_sort.as_deref().is_some_and(|s| !s.trim().is_empty()) {
        mi.title_sort = found.title_sort.clone();
    }
    if found.pubdate.is_some() {
        mi.pubdate = found.pubdate;
    }
    if let Some(p) = found.publisher.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        mi.publisher = Some(p.to_string());
    }
    let tags: Vec<String> = found.tags.iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    if !tags.is_empty() {
        mi.tags = tags;
    }
    if let Some(series) = found.series.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        mi.series = Some(series.to_string());
        mi.series_index = found.series_index;
    }
    let langs: Vec<String> = found.languages.iter().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && l != "und").collect();
    if !langs.is_empty() {
        mi.languages = langs;
    }
    if found.comments.as_deref().is_some_and(|c| !c.trim().is_empty()) {
        mi.comments = found.comments.clone();
    }
    if found.rating.is_some_and(|r| r > 0.0) {
        mi.rating = found.rating;
    }
    for (k, v) in &found.identifiers {
        // `calibre:<id>` points into the library the OPF came from.
        if !k.is_empty() && !v.trim().is_empty() && k != "calibre" {
            mi.identifiers.insert(k.clone(), v.trim().to_string());
        }
    }
    if !found.cover_data.1.is_empty() {
        mi.cover_data = found.cover_data.clone();
    }
}

// ---------------------------------------------------------------------
// One read: copy into staging while hashing
// ---------------------------------------------------------------------

struct Staged {
    path: PathBuf,
    digest: FileDigest,
}

/// Copies `source` to `dest`, hashing as it goes: **one** read of the
/// source. A cancel is noticed between chunks and surfaces as
/// [`std::io::ErrorKind::Interrupted`], with the partial file removed.
fn stage_copy(source: &Path, dest: &Path, control: &JobControl) -> std::io::Result<FileDigest> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = fs::File::open(source)?;
    control.file_opens.fetch_add(1, Ordering::Relaxed);
    let mut output = fs::File::create(dest)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0u8; HASH_BUFFER_BYTES];
    let mut size = 0u64;
    let result = (|| -> std::io::Result<()> {
        loop {
            if control.is_cancelled() {
                return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            output.write_all(&buffer[..n])?;
            size += n as u64;
            control.bytes_hashed.fetch_add(n as u64, Ordering::Relaxed);
        }
        output.flush()?;
        // Durable before it is renamed into the library.
        output.sync_all()
    })();
    drop(output);
    if let Err(e) = result {
        let _ = fs::remove_file(dest);
        return Err(e);
    }
    Ok(FileDigest { hash: hasher.finalize().to_hex().to_string(), size })
}

// ---------------------------------------------------------------------
// The job
// ---------------------------------------------------------------------

/// `(normalised title, normalised author)` -> the book that has them, one
/// entry per author. `None` for books added during this run whose id is not
/// known yet. Keys are [`crate::dedupe::exact_keys`], the same ones the
/// add-time check and the scan's exact tier use, so an import skips
/// `Hobbit, The (z-lib.org)` by `Tolkien, J. R. R.` when `The Hobbit` by
/// `J.R.R. Tolkien` is already there -- and never matches on an unknown
/// author.
fn title_author_index(cache: &Cache) -> Result<HashMap<(String, String), Option<i32>>> {
    let mut index = HashMap::new();
    for record in crate::dedupe::load_records(cache)? {
        for key in crate::dedupe::exact_keys(&record.title, &record.authors) {
            index.entry(key).or_insert(Some(record.id));
        }
    }
    Ok(index)
}

/// A path as text for the report and the ledger. `canonicalize` on Windows
/// yields `\\?\C:\...` (and `\\?\UNC\server\share\...`), which is
/// correct and unreadable; this gives the form a person would type. One
/// spelling everywhere, so the ledger's keys agree with themselves.
fn path_string(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if let Some(rest) = s.strip_prefix("\\\\?\\UNC\\") {
        return format!("\\\\{rest}");
    }
    match s.strip_prefix("\\\\?\\") {
        Some(rest) => rest.to_string(),
        None => s,
    }
}

/// What happened to one file of a group while it was being staged.
enum Fate {
    /// Not read: the ledger says it was done before.
    AlreadyImported,
    Staged(Staged),
    Failed(String),
    Cancelled,
}

struct Planned {
    group: usize,
    /// `(index into the group's files, staged copy)`.
    keep: Vec<(usize, Staged)>,
}

/// Imports `source` into `cache`'s library. See the module docs.
///
/// Returns when every file has been handled or `control` is cancelled
/// ([`ImportReport::cancelled`] says which). An `Err` means the job could not
/// start at all (a bad source, an unreadable library); per-file trouble is
/// in the report.
pub fn import_folder(cache: &Cache, source: &Path, options: &ImportOptions, pipeline: &PipelineOptions, control: &JobControl) -> Result<ImportReport> {
    let source = fs::canonicalize(source).with_context(|| format!("the folder {} cannot be opened", source.display()))?;
    if !source.is_dir() {
        bail!("{} is not a folder", source.display());
    }
    let library = fs::canonicalize(&cache.backend.library_path).unwrap_or_else(|_| cache.backend.library_path.clone());
    if source.starts_with(&library) {
        bail!("{} is inside the library already. To add the books in it, scan the library instead of importing.", source.display());
    }
    let skip_library = library.starts_with(&source).then_some(library.clone());

    let mut report = ImportReport { source: path_string(&source), ..Default::default() };
    let checksums = cache.checksums();
    checksums.initialize()?;

    control.set_phase("walking");
    let extensions: HashSet<String> = crate::adding::metadata_extensions().iter().filter(|e| **e != "opf").map(|e| e.to_string()).collect();
    let mut walk_errors = Vec::new();
    let candidates = walk(&source, options.recursive, skip_library.as_deref(), &extensions, control, &mut walk_errors);
    report.files_seen = candidates.len();
    report.walk_errors = walk_errors;
    let groups = group(candidates, options.mode, &mut report.skipped_same_format);
    report.books_found = groups.len();
    if control.is_cancelled() {
        report.cancelled = true;
        return Ok(report);
    }

    let staging_root = cache.backend.library_path.join(LIBRARY_HANDLE_DIR_NAME).join(STAGING_DIR_NAME);
    // Anything here is left from a job that was killed: one job per library.
    let _ = fs::remove_dir_all(&staging_root);
    fs::create_dir_all(&staging_root).with_context(|| format!("creating {}", staging_root.display()))?;

    control.set_phase("importing");
    control.total.store(groups.iter().map(|g| g.files.len() as u64).sum(), Ordering::Relaxed);

    let policy = options.policy;
    let mut title_index = if matches!(policy, DuplicatePolicy::SkipTitleAuthor | DuplicatePolicy::AddFormatsToExisting) { title_author_index(cache)? } else { HashMap::new() };
    // Content seen earlier in this run, so two copies in one tree are one book
    // even before either is committed.
    let mut run_hashes: HashMap<String, Option<i32>> = HashMap::new();

    let batch_groups = (pipeline.batch_size / 4).clamp(1, 25);
    let result = (|| -> Result<()> {
        for (batch_no, batch) in groups.chunks(batch_groups).enumerate() {
            if control.is_cancelled() {
                break;
            }
            let staging = staging_root.join(batch_no.to_string());
            let done = import_batch(cache, batch, &source, &staging, options, pipeline, control, &mut title_index, &mut run_hashes, &mut report)?;
            let _ = fs::remove_dir_all(&staging);
            if !done {
                break;
            }
            pipeline.batch_done(control);
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&staging_root);
    result?;

    report.cancelled = control.is_cancelled();
    report.conclusive = report.walk_errors.is_empty() && !report.cancelled;
    Ok(report)
}

/// One batch of groups. `Ok(false)` means the job was cancelled and nothing
/// from this batch was committed.
#[allow(clippy::too_many_arguments)]
fn import_batch(
    cache: &Cache,
    batch: &[Group],
    source: &Path,
    staging: &Path,
    options: &ImportOptions,
    pipeline: &PipelineOptions,
    control: &JobControl,
    title_index: &mut HashMap<(String, String), Option<i32>>,
    run_hashes: &mut HashMap<String, Option<i32>>,
    report: &mut ImportReport,
) -> Result<bool> {
    let checksums = cache.checksums();
    let policy = options.policy;

    // 1. What the ledger already knows: skipped without reading.
    let mut jobs: Vec<(usize, usize)> = Vec::new();
    let mut fates: HashMap<(usize, usize), Fate> = HashMap::new();
    for (gi, g) in batch.iter().enumerate() {
        for (fi, file) in g.files.iter().enumerate() {
            let known = checksums.import_ledger_lookup(&path_string(&file.path))?.is_some_and(|row| {
                row.size == file.size as i64 && row.mtime_ns == file.mtime_ns && (row.outcome == "imported" || row.outcome == "merged" || row.policy == policy.name())
            });
            if known {
                fates.insert((gi, fi), Fate::AlreadyImported);
            } else {
                jobs.push((gi, fi));
            }
        }
    }

    // 2. One read each, in parallel: copy to staging while hashing, then
    //    prove the copy.
    let staged = crate::scan_pipeline::parallel_map(&jobs, pipeline.workers, control, |&(gi, fi)| {
        let file = &batch[gi].files[fi];
        let dest = staging.join(format!("{gi}_{fi}")).join(file.path.file_name().unwrap_or_default());
        match stage_copy(&file.path, &dest, control) {
            Ok(digest) => {
                #[cfg(test)]
                if CORRUPT_STAGED.with(|c| c.get()) {
                    let _ = fs::write(&dest, b"damaged on the way");
                }
                control.hashed.fetch_add(1, Ordering::Relaxed);
                if digest.size != file.size {
                    let _ = fs::remove_file(&dest);
                    return Fate::Failed("changed while it was being read; import again to pick it up".to_string());
                }
                match hash_stream(&dest, None) {
                    Ok(check) if check == digest => Fate::Staged(Staged { path: dest, digest }),
                    Ok(_) => {
                        let _ = fs::remove_file(&dest);
                        Fate::Failed("the copy did not match the original (size or BLAKE3); it was discarded".to_string())
                    }
                    Err(e) => {
                        let _ = fs::remove_file(&dest);
                        Fate::Failed(format!("the copy could not be verified: {e}"))
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => Fate::Cancelled,
            Err(e) => Fate::Failed(e.to_string()),
        }
    });
    let mut cancelled = control.is_cancelled();
    for (&(gi, fi), fate) in jobs.iter().zip(staged) {
        match fate {
            Some(Fate::Cancelled) | None => cancelled = true,
            Some(fate) => {
                fates.insert((gi, fi), fate);
            }
        }
    }
    if cancelled {
        return Ok(false);
    }

    // 3. Decide, per file, then per group. Sequential: it consults the
    //    database and the run's own history.
    let mut ledger: Vec<(String, LedgerRow, Option<String>)> = Vec::new();
    let mut planned: Vec<Planned> = Vec::new();
    for (gi, g) in batch.iter().enumerate() {
        let mut keep: Vec<(usize, Staged)> = Vec::new();
        for (fi, file) in g.files.iter().enumerate() {
            let source_path = path_string(&file.path);
            match fates.remove(&(gi, fi)) {
                Some(Fate::AlreadyImported) => {
                    report.skipped_already_imported += 1;
                    control.already_known.fetch_add(1, Ordering::Relaxed);
                }
                Some(Fate::Failed(why)) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    capped(&mut report.failed, PathProblem { path: source_path, error: why });
                }
                Some(Fate::Staged(staged)) => {
                    let duplicate_of = if policy == DuplicatePolicy::AddAnyway {
                        None
                    } else if let Some(prior) = run_hashes.get(&staged.digest.hash) {
                        Some(*prior)
                    } else {
                        checksums.find_by_hash(&staged.digest.hash)?.into_iter().find(|(_, kind, _)| kind == "format").map(|(id, _, _)| Some(id))
                    };
                    match duplicate_of {
                        Some(of) => {
                            report.skipped_duplicate_content += 1;
                            control.ignored.fetch_add(1, Ordering::Relaxed);
                            capped(&mut report.duplicates, Duplicate { path: source_path.clone(), duplicate_of: of });
                            ledger.push((source_path, LedgerRow { hash: staged.digest.hash.clone(), size: file.size as i64, mtime_ns: file.mtime_ns, outcome: "duplicate_content".into(), policy: policy.name().into() }, None));
                            let _ = fs::remove_file(&staged.path);
                        }
                        None => {
                            // Claimed now, so a twin later in this very batch
                            // is a duplicate of it.
                            run_hashes.insert(staged.digest.hash.clone(), None);
                            keep.push((fi, staged));
                        }
                    }
                }
                Some(Fate::Cancelled) | None => {}
            }
        }
        if !keep.is_empty() {
            planned.push(Planned { group: gi, keep });
        }
    }

    // 4. Real embedded metadata, from the staged copy of the group's best
    //    format. In parallel: a PDF cover render is not free.
    let metas = crate::scan_pipeline::parallel_map(&planned, pipeline.workers, control, |p| {
        let best = p.keep.iter().min_by_key(|(fi, _)| metadata_priority(&batch[p.group].files[*fi].ext)).expect("a plan keeps at least one file");
        let embedded = lexmechanic_ebooks::metadata::get_metadata(&best.1.path).ok();
        (embedded, read_sidecars(&batch[p.group]))
    });
    if control.is_cancelled() {
        return Ok(false);
    }

    // 5. Title/author policy, then build the books.
    let mut items: Vec<ImportItem> = Vec::new();
    let mut item_sources: Vec<Vec<(usize, usize)>> = Vec::new();
    let mut merges: Vec<(i32, Vec<(usize, Staged)>, usize)> = Vec::new();
    for (plan, meta) in planned.into_iter().zip(metas) {
        let g = &batch[plan.group];
        let first = &g.files[plan.keep[0].0];
        let mut mi = lexmechanic_ebooks::metadata::MetaInformation::default();
        mi.title = first.stem.clone();
        mi.authors = vec!["Unknown".to_string()];
        // Embedded metadata first, then the sidecar OPF over it (calibre's
        // add does the same). Everything below -- duplicate checks and the
        // book itself -- sees the final values.
        if let Some((embedded, sidecar)) = meta {
            if let Some(found) = embedded {
                overlay(&mut mi, &found);
            }
            if let Some(opf) = &sidecar.opf {
                overlay(&mut mi, opf);
            }
            if mi.cover_data.1.is_empty() {
                if let Some(cover) = sidecar.cover {
                    mi.cover_data = (Some(cover.0), cover.1);
                }
            }
        }

        if matches!(policy, DuplicatePolicy::SkipTitleAuthor | DuplicatePolicy::AddFormatsToExisting) {
            let keys = crate::dedupe::exact_keys(&mi.title, &mi.authors);
            if keys.is_empty() {
                report.unmatched_without_author += 1;
            } else {
                if let Some(existing) = keys.iter().find_map(|k| title_index.get(k).copied()) {
                    let exists_with = existing;
                    if policy == DuplicatePolicy::AddFormatsToExisting {
                        if let Some(id) = exists_with {
                            merges.push((id, plan.keep, plan.group));
                            continue;
                        }
                    }
                    for (fi, staged) in plan.keep {
                        let file = &g.files[fi];
                        report.skipped_duplicate_title += 1;
                        control.ignored.fetch_add(1, Ordering::Relaxed);
                        capped(&mut report.duplicates, Duplicate { path: path_string(&file.path), duplicate_of: exists_with });
                        ledger.push((path_string(&file.path), LedgerRow { hash: staged.digest.hash.clone(), size: file.size as i64, mtime_ns: file.mtime_ns, outcome: "duplicate_title".into(), policy: policy.name().into() }, None));
                        let _ = fs::remove_file(&staged.path);
                    }
                    continue;
                }
                for key in keys {
                    title_index.entry(key).or_insert(None);
                }
            }
        }

        let mut files = Vec::new();
        let mut sources = Vec::new();
        for (fi, staged) in &plan.keep {
            let file = &g.files[*fi];
            run_hashes.insert(staged.digest.hash.clone(), None);
            files.push(ImportFile { staged: staged.path.clone(), ext: file.ext.clone(), size: staged.digest.size as i64, hash: staged.digest.hash.clone() });
            sources.push((plan.group, *fi));
        }
        let placement = options.keep_structure.then(|| {
            let first = &g.files[plan.keep[0].0];
            crate::cache::ImportPlacement { folder: kept_folder(source, &first.dir), stem: first.stem.clone() }
        });
        items.push(ImportItem { metadata: mi, files, placement });
        item_sources.push(sources);
    }

    // 6. One transaction for the batch.
    let results = cache.import_books_batch(&items);
    for ((item, sources), result) in items.iter().zip(&item_sources).zip(results) {
        match result {
            Ok(book) => {
                report.books_added += 1;
                control.added.fetch_add(1, Ordering::Relaxed);
                for hash in item.files.iter().map(|f| f.hash.clone()) {
                    run_hashes.insert(hash, Some(book.id));
                }
                for key in crate::dedupe::exact_keys(&item.metadata.title, &item.metadata.authors) {
                    title_index.insert(key, Some(book.id));
                }
                let first_source = &batch[sources[0].0].files[sources[0].1];
                capped(
                    &mut report.added,
                    AddedBook {
                        book_id: book.id,
                        title: item.metadata.title.clone(),
                        authors: item.metadata.authors.clone(),
                        formats: item.files.iter().map(|f| f.ext.to_uppercase()).collect(),
                        source: path_string(&first_source.dir),
                    },
                );
                for ((gi, fi), file) in sources.iter().zip(&item.files) {
                    let source_file = &batch[*gi].files[*fi];
                    ledger.push((path_string(&source_file.path), LedgerRow { hash: file.hash.clone(), size: source_file.size as i64, mtime_ns: source_file.mtime_ns, outcome: "imported".into(), policy: policy.name().into() }, book.uuid.clone()));
                }
            }
            Err(e) => {
                for ((gi, fi), file) in sources.iter().zip(&item.files) {
                    run_hashes.remove(&file.hash);
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    capped(&mut report.failed, PathProblem { path: path_string(&batch[*gi].files[*fi].path), error: format!("{e:#}") });
                }
            }
        }
    }

    // 7. Formats added to a book that was already there.
    for (book_id, keep, gi) in merges {
        let have: Vec<String> = cache.format_file_names(book_id)?.into_iter().map(|(f, _)| f.to_uppercase()).collect();
        let uuid = cache.book_uuid(book_id)?;
        for (fi, staged) in keep {
            let file = &batch[gi].files[fi];
            let source_path = path_string(&file.path);
            let format = file.ext.to_uppercase();
            if have.contains(&format) {
                report.skipped_duplicate_title += 1;
                control.ignored.fetch_add(1, Ordering::Relaxed);
                capped(&mut report.duplicates, Duplicate { path: source_path.clone(), duplicate_of: Some(book_id) });
                ledger.push((source_path, LedgerRow { hash: staged.digest.hash.clone(), size: file.size as i64, mtime_ns: file.mtime_ns, outcome: "duplicate_title".into(), policy: policy.name().into() }, uuid.clone()));
                continue;
            }
            // The staged file keeps its original name, so it lands under it.
            match cache.add_format(book_id, &staged.path, &file.ext, false) {
                Ok(true) => {
                    let landed = cache.format_file_info(book_id)?.into_iter().find(|f| f.fmt.eq_ignore_ascii_case(&file.ext));
                    let verified = landed.as_ref().is_some_and(|f| hash_stream(&f.path, None).map(|d| d == staged.digest).unwrap_or(false));
                    if !verified {
                        let _ = cache.remove_format_permanently(book_id, &file.ext);
                        control.errors.fetch_add(1, Ordering::Relaxed);
                        capped(&mut report.failed, PathProblem { path: source_path, error: "the copy added to the existing book did not match the original; it was removed".to_string() });
                        continue;
                    }
                    report.formats_added_to_existing += 1;
                    ledger.push((source_path, LedgerRow { hash: staged.digest.hash.clone(), size: file.size as i64, mtime_ns: file.mtime_ns, outcome: "merged".into(), policy: policy.name().into() }, uuid.clone()));
                }
                Ok(false) => {
                    report.skipped_duplicate_title += 1;
                    capped(&mut report.duplicates, Duplicate { path: source_path, duplicate_of: Some(book_id) });
                }
                Err(e) => {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                    capped(&mut report.failed, PathProblem { path: source_path, error: format!("{e:#}") });
                }
            }
        }
    }

    checksums.import_ledger_record(&ledger)?;
    Ok(true)
}

#[cfg(test)]
thread_local! {
    /// Test hook: damage every staged copy right after it is written, to
    /// prove the verification catches it. Only effective with one worker
    /// (the copy then runs on the calling thread).
    pub(crate) static CORRUPT_STAGED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
#[path = "import_tests.rs"]
mod tests;
