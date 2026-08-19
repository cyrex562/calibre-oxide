//! Port of `old_src/src/calibre/db/adding.py` (issue #219, a #201
//! follow-up): bulk directory-import scanning -- distinct from
//! single-book add (`Cache.add_books` upstream, real since #216;
//! [`add_book`] below is an unrelated older DB-row-only helper this
//! file already had, kept as-is since `copy_to_library.rs` depends on
//! its exact narrow signature).
//!
//! # What's real
//!
//! The full filename-filtering and directory-scanning pipeline:
//! [`splitext`]/[`path_ok`]/[`compile_glob`]/[`compile_rule`]/
//! [`filter_filename`]/[`metadata_extensions`]/[`list_only_files_in_dir`]/
//! [`allow_path`]/[`find_books_in_directory`]/[`create_format_map`],
//! plus the real orchestration on top:
//! [`import_book_directory`] (one book per directory, matching
//! upstream's `single_book_per_directory=True`),
//! [`import_book_directory_multiple`] (group files within a directory
//! into separate books by matching filename stem), and
//! [`recursive_import`] (walk a directory tree, dispatching to
//! either of the above per subdirectory).
//!
//! # Disclosed simplifications
//!
//! - **No real per-format metadata extraction.** Upstream's
//!   `metadata_from_formats` parses each detected ebook file for
//!   embedded title/author/etc. metadata; this crate has real
//!   per-format metadata *readers* scattered across
//!   `calibre_ebooks::metadata::*` but no unified "sniff format, pick
//!   reader, extract" dispatcher (that's its own follow-up). Title is
//!   derived from the filename instead (underscores/dashes replaced
//!   with spaces), author is always `"Unknown"` -- functionally
//!   correct directory scanning and book/format creation, just not
//!   real embedded-metadata extraction.
//! - **No duplicate detection wired in** (`add_duplicates` upstream).
//!   `crate::utils::find_identical_books` is real and tested (#221),
//!   but wiring it into this import path too is its own follow-up,
//!   not part of this pass.
//! - **Not ported at all**: `add_catalog`/`add_news` (need
//!   `Cache._search`/`_create_book_entry`-style internals this crate
//!   doesn't have in the same shape) and the import-plugin hooks
//!   (`run_import_plugins`/`run_import_plugins_before_metadata` --
//!   this crate has no import-plugin system to hook into).
//! - `compile_glob` implements Python's `fnmatch.translate` semantics
//!   itself (`*`/`?`/`[seq]`/`[!seq]`), not a borrowed glob library --
//!   there wasn't one in this workspace's dependencies already, and
//!   the syntax is small enough to write directly rather than add one
//!   for a single caller.
//! - Directory listing uses `std::fs::read_dir` directly rather than
//!   porting upstream's Windows-long-path/`unicode_listdir`
//!   encode-decode dance -- both exist purely for Python 2/3
//!   filesystem-encoding compatibility that doesn't apply here.

use crate::cache::Cache;
use anyhow::Result;
use calibre_ebooks::metadata::MetaInformation;
use indexmap::IndexMap;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Port of `splitext`: everything before the last `.` in the file
/// name (not the whole path's final component only -- matches
/// `os.path.splitext`, which operates on the full path string) as the
/// "key", and the lowercased extension without the dot. A dotfile
/// with no other dot (`.bashrc`) has no extension, matching
/// `Path::extension`'s own handling of a leading dot.
pub fn splitext(path: &Path) -> (PathBuf, String) {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => {
            let ext = ext.to_lowercase();
            let mut stem = path.to_path_buf();
            stem.set_extension("");
            (stem, ext)
        }
        None => (path.to_path_buf(), String::new()),
    }
}

/// Port of `path_ok`: not a directory, and actually openable for
/// reading (a real open/close, not a permission-bit check -- simpler
/// and correct across platforms).
pub fn path_ok(path: &Path) -> bool {
    !path.is_dir() && std::fs::File::open(path).is_ok()
}

/// Port of `compile_glob` (via Python's `fnmatch.translate`):
/// `*`/`?`/`[seq]`/`[!seq]` glob syntax to a case-insensitive,
/// fully-anchored regex.
pub fn compile_glob(pattern: &str) -> Result<Regex> {
    let mut re = String::from("(?i)^");
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '[' => {
                let mut j = i + 1;
                let negate = j < chars.len() && (chars[j] == '!' || chars[j] == '^');
                if negate {
                    j += 1;
                }
                let class_start = j;
                if j < chars.len() && chars[j] == ']' {
                    j += 1;
                }
                while j < chars.len() && chars[j] != ']' {
                    j += 1;
                }
                if j >= chars.len() {
                    // Unterminated `[...]` -- fnmatch.translate treats
                    // a stray `[` as a literal.
                    re.push_str("\\[");
                } else {
                    re.push('[');
                    if negate {
                        re.push('^');
                    }
                    let class: String = chars[class_start..j].iter().collect();
                    re.push_str(&class.replace('\\', "\\\\"));
                    re.push(']');
                    i = j;
                }
            }
            c => re.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    re.push('$');
    Regex::new(&re).map_err(Into::into)
}

/// One filename-filter rule: `match_type` is one of
/// `startswith`/`endswith`/`glob`/`regexp`, optionally prefixed
/// `not_` to negate; `action` `"add"` means matching files are
/// included, anything else means excluded. Port of the config shape
/// `compile_rule` consumes.
pub struct FilterRuleConfig {
    pub match_type: String,
    pub query: String,
    pub action: String,
}

/// A [`FilterRuleConfig`] compiled into a real matcher, per
/// `compile_rule`.
pub struct CompiledRule {
    matcher: Box<dyn Fn(&str) -> bool + Send + Sync>,
    pub add: bool,
}

/// Port of `compile_rule`.
pub fn compile_rule(rule: &FilterRuleConfig) -> Result<CompiledRule> {
    let mt = rule.match_type.as_str();
    let base: Box<dyn Fn(&str) -> bool + Send + Sync> = if mt.contains("with") {
        let q = rule.query.to_lowercase();
        if mt.contains("startswith") {
            Box::new(move |filename: &str| filename.to_lowercase().starts_with(&q))
        } else {
            Box::new(move |filename: &str| filename.to_lowercase().ends_with(&q))
        }
    } else if mt.contains("glob") {
        let re = compile_glob(&rule.query)?;
        Box::new(move |filename: &str| re.is_match(filename))
    } else {
        // Python's `re.match` anchors at the start only, not the end.
        let re = Regex::new(&format!("^(?:{})", rule.query))?;
        Box::new(move |filename: &str| re.is_match(filename))
    };
    let matcher: Box<dyn Fn(&str) -> bool + Send + Sync> = if mt.starts_with("not_") {
        Box::new(move |f: &str| !base(f))
    } else {
        base
    };
    Ok(CompiledRule {
        matcher,
        add: rule.action == "add",
    })
}

/// Port of `filter_filename`: the first matching rule's `add` value,
/// or `None` if no rule matches.
pub fn filter_filename(rules: &[CompiledRule], filename: &str) -> Option<bool> {
    rules.iter().find(|r| (r.matcher)(filename)).map(|r| r.add)
}

/// Port of `BOOK_EXTENSIONS` (`calibre.ebooks`) plus `"opf"`, per
/// `metadata_extensions`.
pub fn metadata_extensions() -> &'static std::collections::HashSet<&'static str> {
    use std::sync::OnceLock;
    static EXTS: OnceLock<std::collections::HashSet<&'static str>> = OnceLock::new();
    EXTS.get_or_init(|| {
        [
            "lrf", "rar", "zip", "rtf", "lit", "txt", "txtz", "text", "htm", "xhtm", "html",
            "htmlz", "xhtml", "pdf", "pdb", "updb", "pdr", "prc", "mobi", "azw", "doc", "epub",
            "fb2", "fbz", "djv", "djvu", "lrx", "cbr", "cb7", "cbz", "cbc", "oebzip", "rb", "imp",
            "odt", "chm", "tpz", "azw1", "pml", "pmlz", "mbp", "tan", "snb", "xps", "oxps", "azw4",
            "book", "zbf", "pobi", "docx", "docm", "md", "textile", "markdown", "ibook", "ibooks",
            "iba", "azw3", "ps", "kepub", "kfx", "kpf", "opf",
        ]
        .into_iter()
        .collect()
    })
}

/// Port of `list_only_files_in_dir`: every regular file directly in
/// `root` (not recursive), optionally sorted by modification time
/// (ties/errors sort as if `UNIX_EPOCH`, matching upstream's `0`
/// fallback).
pub fn list_only_files_in_dir(root: &Path, sort_by_mtime: bool) -> Vec<PathBuf> {
    let mut items: Vec<PathBuf> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|entry| entry.path())
        .collect();
    if sort_by_mtime {
        items.sort_by_key(|p| {
            std::fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
    }
    items
}

/// Port of `allow_path`: an explicit filter-rule verdict wins;
/// otherwise fall back to "is this a known book/metadata extension".
pub fn allow_path(path: &Path, ext: &str, rules: &[CompiledRule]) -> bool {
    let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match filter_filename(rules, basename) {
        Some(action) => action,
        None => metadata_extensions().contains(ext),
    }
}

/// Port of `find_books_in_directory`: one group of format-file paths
/// per detected book. `single_book_per_directory=true` treats every
/// allowed file in `dirpath` as one book (grouped by extension, so at
/// most one file per format); `false` groups files by matching
/// filename stem (case-insensitive) into separate books, matching
/// upstream's `icu_lower(key)` grouping.
pub fn find_books_in_directory(
    dirpath: &Path,
    single_book_per_directory: bool,
    rules: &[CompiledRule],
) -> Vec<Vec<PathBuf>> {
    let dirpath = dirpath
        .canonicalize()
        .unwrap_or_else(|_| dirpath.to_path_buf());
    let mut groups = Vec::new();

    if single_book_per_directory {
        let mut formats: IndexMap<String, PathBuf> = IndexMap::new();
        for path in list_only_files_in_dir(&dirpath, false) {
            let (_, ext) = splitext(&path);
            if allow_path(&path, &ext, rules) {
                formats.insert(ext, path);
            }
        }
        if !formats.is_empty() {
            groups.push(formats.into_values().collect());
        }
    } else {
        let mut books: IndexMap<String, IndexMap<String, PathBuf>> = IndexMap::new();
        for path in list_only_files_in_dir(&dirpath, true) {
            let (key, ext) = splitext(&path);
            if allow_path(&path, &ext, rules) {
                let key_lower = key.to_string_lossy().to_lowercase();
                books.entry(key_lower).or_default().insert(ext, path);
            }
        }
        for formats in books.into_values() {
            if !formats.is_empty() {
                groups.push(formats.into_values().collect());
            }
        }
    }
    groups
}

/// Port of `create_format_map`: uppercased extension -> path, `OPF`
/// excluded (it's metadata, not a format).
pub fn create_format_map(formats: &[PathBuf]) -> IndexMap<String, PathBuf> {
    let mut map = IndexMap::new();
    for path in formats {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let ext = ext.to_uppercase();
            if ext == "OPF" {
                continue;
            }
            map.insert(ext, path.clone());
        }
    }
    map
}

fn title_from_stem(stem: &Path) -> String {
    let base = stem
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown");
    let title = base.replace(['_', '-'], " ");
    let title = title.trim();
    if title.is_empty() {
        "Unknown".to_string()
    } else {
        title.to_string()
    }
}

/// Adds one book from a group of format-file paths (as produced by
/// [`find_books_in_directory`]): the first format becomes the book's
/// initial file (via [`Cache::add_book`]), every additional format is
/// attached via [`Cache::add_format`]. See this module's docs for the
/// filename-derived-title disclosed simplification.
fn add_book_from_formats(cache: &Arc<Mutex<Cache>>, formats: &[PathBuf]) -> Result<Option<i32>> {
    let Some(first) = formats.first() else {
        return Ok(None);
    };
    let (stem, _) = splitext(first);
    let mut meta = MetaInformation::default();
    meta.title = title_from_stem(&stem);
    meta.authors = vec!["Unknown".to_string()];

    let guard = cache.lock().unwrap();
    let book_id = guard.add_book(first, &meta)?;
    for extra in &formats[1..] {
        if let Some(ext) = extra.extension().and_then(|e| e.to_str()) {
            guard.add_format(book_id, extra, ext, true)?;
        }
    }
    Ok(Some(book_id))
}

/// Port of `import_book_directory`: one book from every allowed file
/// directly in `dirpath` (not recursive -- see [`recursive_import`]).
pub fn import_book_directory(
    cache: &Arc<Mutex<Cache>>,
    dirpath: &Path,
    rules: &[CompiledRule],
) -> Result<Option<i32>> {
    let groups = find_books_in_directory(dirpath, true, rules);
    match groups.into_iter().next() {
        Some(formats) => add_book_from_formats(cache, &formats),
        None => Ok(None),
    }
}

/// Port of `import_book_directory_multiple`: one book per
/// filename-stem group of allowed files directly in `dirpath`.
pub fn import_book_directory_multiple(
    cache: &Arc<Mutex<Cache>>,
    dirpath: &Path,
    rules: &[CompiledRule],
) -> Result<Vec<i32>> {
    let mut ids = Vec::new();
    for formats in find_books_in_directory(dirpath, false, rules) {
        if let Some(id) = add_book_from_formats(cache, &formats)? {
            ids.push(id);
        }
    }
    Ok(ids)
}

/// Port of `recursive_import`: walks `root` and every subdirectory,
/// dispatching each to [`import_book_directory`] or
/// [`import_book_directory_multiple`] depending on
/// `single_book_per_directory`. Returns every added book id.
pub fn recursive_import(
    cache: &Arc<Mutex<Cache>>,
    root: &Path,
    single_book_per_directory: bool,
    rules: &[CompiledRule],
) -> Result<Vec<i32>> {
    let mut ids = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_dir())
    {
        if single_book_per_directory {
            if let Some(id) = import_book_directory(cache, entry.path(), rules)? {
                ids.push(id);
            }
        } else {
            ids.extend(import_book_directory_multiple(cache, entry.path(), rules)?);
        }
    }
    Ok(ids)
}

/// Adds a new book to the database.
///
/// # Arguments
/// * `cache` - The database cache/backend access.
/// * `title` - The title of the book.
/// * `authors` - A list of authors (names).
///
/// # Returns
/// * `Result<i32>` - The ID of the newly created book.
pub fn add_book(cache: &Arc<Mutex<Cache>>, title: &str, authors: &[String]) -> Result<i32> {
    let uuid = Uuid::new_v4().to_string();

    // Basic title sort: simplified for now (Copy of title).
    // In full Calibre, this uses library prefixes rules.
    let sort = title.to_string();

    // Basic author sort: simplified.
    // In full Calibre, this uses AuthorSortMap and fancy logic.
    let author_sort = if authors.is_empty() {
        "Unknown".to_string()
    } else {
        authors.join(" & ")
    };

    let lock = cache.lock().unwrap();
    let book_id = lock
        .backend
        .insert_book(title, &sort, &author_sort, &uuid)?;

    // Note: This does NOT yet insert into the `authors` table or `books_authors_link` table.
    // That involves "many-many" field logic which is complex and partially handled in `write.py`.
    // For this sprint, we focus on the `books` table entry.

    Ok(book_id)
}
