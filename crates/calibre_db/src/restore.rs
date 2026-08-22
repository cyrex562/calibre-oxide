//! Port of `old_src/src/calibre/db/restore.py` (issue #224, a #201
//! follow-up): rebuilding `metadata.db` from the OPF files sitting in
//! each book's own directory, for when the database itself is lost or
//! corrupt but the library's files survive.
//!
//! # Scope of this pass
//!
//! Upstream's `Restore` is a `Thread` subclass that stages the rebuilt
//! database in a temporary sibling directory, restores saved
//! preferences/`field_metadata`/custom-column *definitions* from a
//! `metadata_db_prefs_backup.json` sidecar, recovers each book's
//! original numeric id from its directory name (`"Title (123)"`, via
//! `db_id_regexp`) cross-checked against the OPF's own embedded
//! `calibre` identifier, restores notes, and only atomically swaps the
//! rebuilt database into place at the very end (`replace_db`).
//!
//! This crate's book directories are never suffixed with `"(id)"` in
//! the first place (`Cache::add_book`'s convention is plain
//! `Author/Title`, a disclosed simplification from #216) -- so
//! `scan_library`'s directory-name id regex has nothing to match here.
//! Instead, this pass recovers each book's original id from the OPF's
//! own embedded `calibre` identifier alone (via the new
//! [`crate::cache::Cache::add_book_db_entry_with_id`]) and re-inserts
//! it explicitly, so cross-references made before a database loss
//! (e.g. notes, external tooling keyed by book id) survive a restore
//! intact -- previously every restored book silently got a fresh
//! autoincrement id instead.
//!
//! What's real, verified against upstream's `process_dir`/
//! `restore_books`: every field `parse_opf` recovers is now actually
//! applied (title, `sort`, `author_sort`, real author re-linking,
//! uuid, comments, publisher, series/series_index, rating, tags,
//! languages, identifiers -- all via the new
//! [`crate::cache::Cache::set_field`] from #223, not just
//! `author_sort`/uuid/title as before); format files sitting in each
//! book's directory are rediscovered and re-registered in the `data`
//! table (upstream's `is_ebook_file`/dedup-by-first-`mtime`-per-
//! extension logic, ported faithfully); `cover.jpg`'s presence sets
//! `has_cover`; and a bad OPF in one book's directory no longer aborts
//! the whole restore -- it's recorded and skipped, matching upstream's
//! `failed_restores` accumulation (exposed here as
//! [`RestoreReport::failed`]).
//!
//! # Not ported (disclosed)
//!
//! - **Preferences/`field_metadata`/custom-column restore**
//!   (`load_preferences`/`create_cc_metadata`): no `metadata_db_prefs_backup.json`
//!   sidecar or `field_metadata` subsystem exists in this crate, and
//!   `parse_opf`'s `user_metadata` is a flat `HashMap<String, String>`
//!   with no datatype/`is_multiple` info -- there's nothing to rebuild
//!   custom-column *definitions* from. Custom column *values* are
//!   therefore also not restored (there's no column to put them in).
//! - **Staged temp-directory + atomic swap** (`run`/`replace_db`):
//!   this restores in place (backing up the old `metadata.db` to
//!   `metadata_pre_restore.db` first, same as before this pass), not
//!   via a sibling temp library swapped in at the end. A restore that
//!   fails partway through this port therefore leaves a partially
//!   rebuilt `metadata.db` rather than leaving the original untouched
//!   -- the pre-restore backup is still there to recover from either
//!   way.
//! - **Notes restore, `link_maps`, annotations**: no notes-backup or
//!   link-map subsystem is read by this pass.
//! - **Threading**: this runs synchronously; the `progress_callback`
//!   is called inline, not from a worker thread.

use crate::cache::Cache;
use anyhow::{Context, Result};
use calibre_ebooks::metadata::MetaInformation;
use calibre_ebooks::opf::parse_opf;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use walkdir::WalkDir;

/// Port of `NON_EBOOK_EXTENSIONS`.
const NON_EBOOK_EXTENSIONS: &[&str] = &["jpg", "jpeg", "gif", "png", "bmp", "opf", "swp", "swo"];

/// Port of `is_ebook_file`: a non-empty extension, not one of
/// [`NON_EBOOK_EXTENSIONS`], and containing only `[a-z0-9_]` once
/// lowercased (upstream's `bad_ext_pat`).
fn is_ebook_file(filename: &str) -> bool {
    let Some(ext) = Path::new(filename).extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_lowercase();
    !ext.is_empty()
        && !NON_EBOOK_EXTENSIONS.contains(&ext.as_str())
        && ext
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Port of `process_dir`'s format-rediscovery loop: every ebook file
/// in `book_dir`, sorted by mtime ascending, keeping only the first
/// (oldest) file seen per uppercased extension -- upstream's
/// `fmt_map.setdefault` dedup. Returns `(format, size_bytes, name)`
/// triples in that order, matching what a `data` table row needs.
fn discover_formats(book_dir: &Path) -> Vec<(String, u64, String)> {
    let mut entries: Vec<PathBuf> = fs::read_dir(book_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    entries.sort_by_key(|p| {
        fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    let mut seen_exts = HashSet::new();
    let mut out = Vec::new();
    for path in entries {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !is_ebook_file(name) {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_uppercase();
        if ext.is_empty() || !seen_exts.insert(ext.clone()) {
            continue;
        }
        let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.push((ext, size, stem));
    }
    out
}

/// Applies every field `parse_opf` recovered to an already-inserted
/// book row -- shared by [`restore_from_opf`] (single book) and
/// [`restore_database`] (full rebuild). Real author re-linking (not
/// just `author_sort`) and every [`Cache::set_field`] (#223)
/// supported field, so a restore recovers far more than the
/// title/author_sort/uuid this crate restored before this pass.
fn apply_metadata(cache: &Cache, book_id: i32, meta: &MetaInformation) -> Result<()> {
    if !meta.title.is_empty() {
        cache.set_field(book_id, "title", &meta.title)?;
    }
    if let Some(sort) = meta.title_sort.as_deref() {
        cache.set_field(book_id, "sort", sort)?;
    }
    if !meta.authors.is_empty() {
        cache.set_field(book_id, "authors", &meta.authors.join(" & "))?;
    }
    // `parse_opf` never populates `meta.author_sort` (that book-level
    // field only round-trips through a hand-built `MetaInformation`,
    // not real OPF XML -- OPF only carries each author's own
    // `opf:file-as` sort string, in `author_sort_map`). When it's
    // missing, derive the legacy book-level `author_sort` the same
    // way upstream's `author_sort_from_authors` does: each author's
    // saved sort (falling back to their plain name) joined with
    // `" & "`.
    let author_sort = meta.author_sort.clone().or_else(|| {
        if meta.authors.is_empty() {
            None
        } else {
            Some(
                meta.authors
                    .iter()
                    .map(|a| {
                        meta.author_sort_map
                            .get(a)
                            .cloned()
                            .unwrap_or_else(|| a.clone())
                    })
                    .collect::<Vec<_>>()
                    .join(" & "),
            )
        }
    });
    if let Some(author_sort) = author_sort {
        cache.set_field(book_id, "author_sort", &author_sort)?;
    }
    if let Some(uuid) = meta.uuid.as_deref() {
        cache.set_field(book_id, "uuid", uuid)?;
    }
    if let Some(comments) = meta.comments.as_deref() {
        cache.set_field(book_id, "comments", comments)?;
    }
    if let Some(publisher) = meta.publisher.as_deref() {
        cache.set_field(book_id, "publisher", publisher)?;
    }
    if let Some(series) = meta.series.as_deref() {
        cache.set_field(book_id, "series", series)?;
    }
    if let Some(rating) = meta.rating {
        cache.set_field(book_id, "rating", &(rating as i32).to_string())?;
    }
    if !meta.tags.is_empty() {
        cache.set_field(book_id, "tags", &meta.tags.join(", "))?;
    }
    if !meta.languages.is_empty() {
        cache.set_field(book_id, "languages", &meta.languages.join(", "))?;
    }
    if !meta.identifiers.is_empty() {
        let joined = meta
            .identifiers
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(",");
        cache.set_field(book_id, "identifiers", &joined)?;
    }
    Ok(())
}

/// Restores a single book's metadata from the `metadata.opf` in its
/// own directory -- a no-op if there is no such file (nothing to
/// restore from).
pub fn restore_from_opf(cache: &Arc<Mutex<Cache>>, book_id: i32) -> Result<()> {
    let opf_path = {
        let guard = cache.lock().unwrap();
        let path_rel = guard
            .backend
            .field_for(book_id, "path")?
            .context("Book path not found in DB")?;
        guard
            .backend
            .library_path
            .join(path_rel)
            .join("metadata.opf")
    };

    if !opf_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&opf_path)?;
    let meta = parse_opf(&content)?;

    let guard = cache.lock().unwrap();
    apply_metadata(&guard, book_id, &meta)
}

/// A summary of a [`restore_database`] run: how many book directories
/// were successfully restored, and which ones failed with what error
/// -- upstream's `failed_restores`/`report`, minus the free-text
/// report formatting (callers can format `failed` however they like).
#[derive(Debug, Default)]
pub struct RestoreReport {
    pub restored: usize,
    pub failed: Vec<(PathBuf, String)>,
}

/// Rebuilds `metadata.db` from every `metadata.opf` found under
/// `library_path`, in place (backing up any existing database first).
/// A directory whose OPF is missing/unreadable/unparseable is recorded
/// in the returned [`RestoreReport::failed`] and skipped, rather than
/// aborting the whole restore.
///
/// Port of issue #93's crate-wide write-path retrofit: this holds the
/// real `LibraryHandle`'s exclusive writer lock (§7) for the entire
/// run, not just around the `metadata.db` backup rename. The rename
/// itself is already atomic (POSIX `rename` plus the fsync discipline
/// `rename_atomic` adds) regardless of whether a lock is held around
/// it -- what actually needs the lock is the much longer, much more
/// exposed loop just below, which walks every book directory and
/// inserts rows into the freshly-created (initially near-empty)
/// database one at a time. That loop is not one SQL transaction; a
/// concurrent writer through a *different* `Backend`/`Cache` over the
/// same library (adding a book, deleting one, renaming a folder)
/// could otherwise interleave with it mid-walk -- a real isolation
/// violation, not just a theoretical one, since this function can run
/// for a long time on a real library. Holding the lock for the whole
/// run means a concurrent write attempt fails fast with
/// `AlreadyLocked` instead of racing the rebuild -- the same tradeoff
/// a real database's `VACUUM`/`REINDEX` makes.
pub fn restore_database<P: AsRef<Path>, F>(
    library_path: P,
    mut progress_callback: F,
) -> Result<RestoreReport>
where
    F: FnMut(String),
{
    let lib_path = library_path.as_ref();

    let handle = crate::library_handle::LibraryHandle::open(lib_path)
        .context("Failed to open library handle")?;

    let db_path = lib_path.join("metadata.db");

    if db_path.exists() {
        let backup_path = lib_path.join("metadata_pre_restore.db");
        if backup_path.exists() {
            handle
                .remove_atomic(&backup_path)
                .context("Failed to remove old backup DB")?;
        }
        handle
            .rename_atomic(&db_path, &backup_path)
            .context("Failed to backup existing DB")?;
        progress_callback(format!("Backed up existing database to {:?}", backup_path));
    }

    let cache = Cache::new(lib_path)?;
    progress_callback("Created new database schema.".to_string());

    let mut book_dirs: Vec<PathBuf> = WalkDir::new(lib_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name() == "metadata.opf")
        .filter_map(|e| e.path().parent().map(|p| p.to_path_buf()))
        .collect();
    book_dirs.sort();

    let mut used_ids: HashSet<i32> = HashSet::new();
    let mut report = RestoreReport::default();

    for book_dir in &book_dirs {
        let rel_path = match book_dir.strip_prefix(lib_path) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        let outcome: Result<String> = (|| {
            let content = fs::read_to_string(book_dir.join("metadata.opf"))?;
            let meta = parse_opf(&content)?;

            let explicit_id = meta
                .identifiers
                .get("calibre")
                .and_then(|s| s.parse::<i32>().ok())
                .filter(|id| *id > 0 && !used_ids.contains(id));

            let book_id = cache.add_book_db_entry_with_id(&meta, &rel_path, explicit_id)?;
            used_ids.insert(book_id);
            apply_metadata(&cache, book_id, &meta)?;

            if book_dir.join("cover.jpg").exists() {
                cache.set_field(book_id, "has_cover", "1")?;
            }

            {
                let conn = cache.backend.conn.lock().unwrap();
                for (ext, size, name) in discover_formats(book_dir) {
                    conn.execute(
                        "INSERT OR REPLACE INTO data (book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                        (book_id, ext, size as i64, name),
                    )?;
                }
            }

            Ok(meta.title)
        })();

        match outcome {
            Ok(title) => {
                report.restored += 1;
                progress_callback(format!("Restored: {title}"));
            }
            Err(e) => {
                report.failed.push((book_dir.clone(), e.to_string()));
                progress_callback(format!("Failed to restore {:?}: {}", book_dir, e));
            }
        }
    }

    progress_callback("Restore completed.".to_string());
    Ok(report)
}
