//! Merge books: fold duplicate records into one.
//!
//! Port of calibre's "Merge book records" (`gui2/actions/edit_metadata.py`
//! `merge_books` / `merge_metadata` / `merge_data_files`, backed by
//! `db/cache.py` `merge_book_metadata` and `merge_extra_files`).
//!
//! # What calibre does, and what this ports
//!
//! `merge_books` has three modes, kept here as [`MergeMode`]:
//!
//! * **Merge and delete** (default): every source's formats are added to the
//!   destination with `replace=False`, so **the destination's own copy of a
//!   format always wins**; calibre does not compare size or date. Metadata is
//!   merged (`merge_book_metadata`), data files are merged
//!   (`merge_extra_files`, conflicts auto-renamed), and the sources are
//!   deleted -- a source's duplicate formats are "permanently deleted" with
//!   it.
//! * **Safe merge** ([`MergeMode::MergeKeepSources`]): formats and metadata
//!   merged, data files *not*, sources untouched.
//! * **Formats only** ([`MergeMode::FormatsOnly`]): formats moved, no
//!   metadata, sources deleted.
//!
//! `merge_book_metadata`'s field rules, all "fill what the destination lacks":
//!
//! | field | rule |
//! | --- | --- |
//! | comments | appended to the destination's after a blank line, when the source's differs from the destination's original |
//! | title, authors | only when the destination's is empty / "Unknown" |
//! | tags | union |
//! | cover | the destination's, else the first source's that has one |
//! | publisher, rating, series (+ index), pubdate | only when the destination has none |
//! | identifiers | union; on a clash the destination wins, then the earlier source |
//! | custom `comments` | appended like comments |
//! | custom `bool` `int` `float` `rating` `datetime` | only when the destination has none |
//! | custom `series` | name and index, only when the destination has none |
//! | custom `enumeration` and single `text` | only when the destination has none |
//! | custom multiple `text` | union |
//!
//! # Deviations from calibre
//!
//! * **Languages are unioned.** calibre's `merge_book_metadata` never touches
//!   languages; they would be lost with the source.
//! * **Comments are not appended twice.** calibre appends a source's comments
//!   whenever they differ from the destination's *original*, so re-running a
//!   merge (or merging two sources with equal comments) repeats them. Here a
//!   source's comments already contained in the destination's are skipped,
//!   which makes a merge safe to repeat after a crash.
//! * **No alternate covers** (`save_alternate_cover`): a source's cover that
//!   loses to the destination's is dropped with the source.
//! * **Rating is "fill if empty"**, as calibre does it, not "take the max".
//! * **Option to keep a conflicting format** ([`MergeOptions::keep_duplicate_formats`]):
//!   calibre discards it; with this on, the source's differing copy is saved
//!   in the destination's `data/` folder instead. Byte-identical duplicates
//!   are never kept: they add nothing.
//! * **Files are copied, verified, then the source is trashed**, never
//!   moved: see below. calibre reads each file into the destination and
//!   deletes the source book to its trash; this does the same -- into the
//!   library's own `.caltrash`, data files included, never the OS recycle bin -- but only
//!   after checking size and BLAKE3 of what landed.
//! * **Not one SQL transaction.** Each metadata field goes through the same
//!   write path as an ordinary edit (so the change log records it and a
//!   rebuild reproduces it); each of those is its own `BEGIN IMMEDIATE`.
//!
//! # Order of operations and crash semantics
//!
//! 1. Validate. Nothing is touched if a book is missing.
//! 2. Per source, per format: copy into the destination
//!    ([`Cache::add_format`]: journaled copy, hash recorded in the checksum
//!    sidecar, `FormatSet` logged), then re-hash the destination file and
//!    compare size and BLAKE3 with the source's. A bad copy is removed again
//!    and that source is marked failed.
//! 3. Metadata, then data files (copied, verified).
//! 4. Only now, per source that passed every check: the source is deleted
//!    through [`crate::removal::delete_with_file`] (into the library's own
//!    `.caltrash` -- also on a network share --, never a recursive directory
//!    delete, and a file the destination now claims --
//!    the same file in a shared folder -- is kept).
//!
//! So at every instant the destination holds a superset of what it had, and a
//! source goes only after its content is verified in the destination. A crash
//! or error after step 2 leaves the destination complete with the source still
//! present; a crash before leaves some formats copied and the source intact.
//! Running the merge again finishes the job: formats already copied compare
//! identical and are skipped, tags/identifiers are unions, comments are
//! not repeated. A source that fails is reported and left alone while the
//! others carry on.
//!
//! In-place books (`books.path` is a folder shared with siblings) work the
//! same way: copying a file to a destination in the same folder records the
//! existing file against the destination, and deleting the source then sees
//! the file is claimed by another book and leaves it. Data files of a source
//! that shares its folder are not moved (they are the siblings' too).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::cache::Cache;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// Formats, metadata and data files into the destination; sources deleted.
    #[default]
    MergeAndDelete,
    /// calibre's "safe merge": formats and metadata; sources kept as they were.
    MergeKeepSources,
    /// Formats only; sources deleted; metadata untouched.
    FormatsOnly,
}

impl MergeMode {
    fn deletes_sources(self) -> bool {
        !matches!(self, MergeMode::MergeKeepSources)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MergeOptions {
    pub mode: MergeMode,
    /// Keep a source's *different* copy of a format the destination already
    /// has, as a data file, instead of discarding it with the source.
    pub keep_duplicate_formats: bool,
    /// Compute and report what would happen; change nothing.
    pub dry_run: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct SourceReport {
    pub book_id: i32,
    pub title: String,
    /// Formats copied into the destination.
    pub formats_moved: Vec<String>,
    /// Formats the destination had with the same bytes; nothing to add.
    pub formats_identical: Vec<String>,
    /// Different copies the destination's own beat; discarded with the source.
    pub formats_discarded: Vec<String>,
    /// Different copies kept as `(format, data file relpath)`.
    pub formats_kept_as_data_file: Vec<(String, String)>,
    /// Different copies left on a source that is being kept.
    pub formats_left_on_source: Vec<String>,
    /// Data files copied into the destination.
    pub data_files_moved: Vec<String>,
    /// Source book removed.
    pub deleted: bool,
    /// Files sent to the wastebasket (relative paths; in a dry run, what would be).
    pub trashed: Vec<String>,
    /// `(path, why)` that could not be trashed and were left in place.
    pub failed: Vec<(String, String)>,
    /// Why this source was not merged or not deleted. It is intact.
    pub error: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct MergeReport {
    pub dest_id: i32,
    pub mode: MergeMode,
    pub dry_run: bool,
    pub sources: Vec<SourceReport>,
    /// Fields of the destination that changed (`#label` for custom columns).
    pub metadata_changed: Vec<String>,
    /// The source whose cover the destination took.
    pub cover_from: Option<i32>,
    pub warnings: Vec<String>,
}

impl MergeReport {
    /// Every file sent to the wastebasket.
    pub fn trashed(&self) -> Vec<String> {
        self.sources.iter().flat_map(|s| s.trashed.iter().cloned()).collect()
    }
}

#[cfg(test)]
thread_local! {
    /// Name of a checkpoint at which `merge_books` should fail, to prove
    /// what a failure there leaves behind.
    pub(crate) static FAIL_AT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    /// Overwrite each copy in the destination before it is verified.
    pub(crate) static CORRUPT_COPY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn checkpoint(name: &'static str) -> Result<()> {
    #[cfg(test)]
    if FAIL_AT.with(|f| f.get()) == Some(name) {
        bail!("injected failure at {name}");
    }
    let _ = name;
    Ok(())
}

fn digest(path: &Path) -> Result<crate::scan_pipeline::FileDigest> {
    crate::scan_pipeline::hash_stream(path, None).with_context(|| format!("reading {}", path.display()))
}

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn rel(cache: &Cache, p: &Path) -> String {
    p.strip_prefix(&cache.backend.library_path).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Merges `src_ids` into `dest_id`. See the module docs.
///
/// An `Err` means nothing was done, or a step *between* sources failed in a
/// way that is not one source's fault (a metadata write); whatever was
/// already copied stays in the destination and no source has been deleted.
/// A single source failing is reported in its [`SourceReport::error`] instead.
pub fn merge_books(cache: &Cache, dest_id: i32, src_ids: &[i32], opts: &MergeOptions) -> Result<MergeReport> {
    // ---- 1. validate -------------------------------------------------
    let mut sources: Vec<i32> = Vec::new();
    for id in src_ids {
        if *id != dest_id && !sources.contains(id) {
            sources.push(*id);
        }
    }
    if sources.is_empty() {
        bail!("nothing to merge: no source books besides the destination");
    }
    for id in std::iter::once(&dest_id).chain(sources.iter()) {
        if cache.field_for(*id, "path")?.is_none() {
            bail!("book {id} does not exist");
        }
    }
    let dry = opts.dry_run;
    let mut report = MergeReport { dest_id, mode: opts.mode, dry_run: dry, ..Default::default() };

    // ---- 2. formats ---------------------------------------------------
    let mut dest_formats: HashMap<String, PathBuf> = cache.format_file_info(dest_id)?.into_iter().map(|f| (f.fmt.to_uppercase(), f.path)).collect();
    // Where each moved format landed, to re-check right before deleting.
    let mut landed: HashMap<i32, Vec<(String, PathBuf)>> = HashMap::new();

    for &src in &sources {
        let mut sr = SourceReport { book_id: src, title: cache.field_for(src, "title")?.unwrap_or_default(), ..Default::default() };
        for f in cache.format_file_info(src)? {
            let fmt = f.fmt.to_uppercase();
            if !f.path.is_file() {
                sr.warnings.push(format!("the {fmt} file {} is missing on disk, so it cannot be merged", f.path.display()));
                continue;
            }
            let outcome: Result<()> = (|| {
                match dest_formats.get(&fmt).cloned() {
                    None => {
                        let at = if dry {
                            // Where it would land: the destination's folder, same
                            // name. Equal to the source's own path when they share a
                            // folder, in which case the file is claimed, not trashed.
                            match (cache.book_dir(dest_id)?, f.path.file_name()) {
                                (Some(dir), Some(name)) => dir.join(name),
                                _ => f.path.clone(),
                            }
                        } else {
                            copy_format_verified(cache, dest_id, &f.path, &fmt)?
                        };
                        dest_formats.insert(fmt.clone(), at.clone());
                        landed.entry(src).or_default().push((fmt.clone(), at));
                        sr.formats_moved.push(fmt.clone());
                    }
                    Some(dest_path) => {
                        let same = lexmechanic_utils::filenames::samefile(&dest_path, &f.path) || digest(&dest_path)?.hash == digest(&f.path)?.hash;
                        if same {
                            landed.entry(src).or_default().push((fmt.clone(), dest_path));
                            sr.formats_identical.push(fmt.clone());
                        } else if !opts.mode.deletes_sources() {
                            sr.formats_left_on_source.push(fmt.clone());
                        } else if opts.keep_duplicate_formats {
                            let name = f.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{fmt}.{}", fmt.to_lowercase()));
                            if dry {
                                sr.formats_kept_as_data_file.push((fmt.clone(), format!("data/{name}")));
                            } else {
                                let original = digest(&f.path)?;
                                let (relpath, hash) = crate::extra_files::copy_extra_file_in(cache, dest_id, &f.path, &format!("data/{name}"))?;
                                if hash != original.hash {
                                    bail!("the copy of {name} kept as a data file does not match the original");
                                }
                                sr.formats_kept_as_data_file.push((fmt.clone(), relpath));
                            }
                        } else {
                            sr.formats_discarded.push(fmt.clone());
                        }
                    }
                }
                Ok(())
            })();
            if let Err(e) = outcome {
                sr.error = Some(format!("{fmt}: {e:#}"));
                break;
            }
        }
        report.sources.push(sr);
    }
    if !dry {
        checkpoint("after_formats")?;
    }

    // ---- 3. metadata, data files --------------------------------------
    if !dry && opts.mode != MergeMode::FormatsOnly {
        let ids: Vec<i32> = report.sources.iter().map(|s| s.book_id).collect();
        merge_metadata(cache, dest_id, &ids, &mut report)?;
        checkpoint("after_metadata")?;
    }
    if opts.mode == MergeMode::MergeAndDelete {
        for sr in report.sources.iter_mut().filter(|s| s.error.is_none()) {
            if let Err(e) = merge_data_files(cache, dest_id, sr, dry) {
                sr.error = Some(format!("data files: {e:#}"));
            }
        }
    } else if opts.mode == MergeMode::FormatsOnly && !dry {
        for sr in report.sources.iter_mut() {
            if has_own_data_files(cache, sr.book_id) {
                sr.warnings.push("its data files are not merged in this mode and are left in its folder".to_string());
            }
        }
    }

    // ---- 4. delete sources ---------------------------------------------
    if opts.mode.deletes_sources() {
        for sr in report.sources.iter_mut() {
            if sr.error.is_some() {
                continue;
            }
            // Everything this source contributed is still where we put it.
            for (fmt, at) in landed.get(&sr.book_id).into_iter().flatten() {
                if !dry && !at.is_file() {
                    sr.error = Some(format!("the {fmt} file expected at {} is gone; the source was not deleted", at.display()));
                    break;
                }
            }
            if sr.error.is_some() {
                continue;
            }
            if dry {
                sr.trashed = cache.format_file_info(sr.book_id)?.into_iter().filter(|f| f.path.is_file()).filter(|f| !landed.get(&sr.book_id).into_iter().flatten().any(|(_, at)| lexmechanic_utils::filenames::samefile(at, &f.path))).map(|f| rel(cache, &f.path)).collect();
                sr.deleted = true;
                continue;
            }
            if let Err(e) = checkpoint("before_delete") {
                sr.error = Some(format!("{e:#}"));
                continue;
            }
            match crate::removal::delete_with_file(cache, sr.book_id) {
                Ok(removal) => {
                    sr.deleted = true;
                    sr.trashed.extend(removal.deleted);
                    sr.failed = removal.failed;
                }
                Err(e) => sr.error = Some(format!("the destination is complete but the source could not be deleted: {e:#}")),
            }
        }
    }
    Ok(report)
}

/// Copies one format file into `dest_id` and proves it arrived: the file
/// the destination now points at has the source's size and BLAKE3. A copy
/// that fails the check is removed again (never when it *is* the source's
/// file) and the error says so.
fn copy_format_verified(cache: &Cache, dest_id: i32, src_path: &Path, fmt: &str) -> Result<PathBuf> {
    let want = digest(src_path)?;
    // `replace = false`: the caller has established the destination has no
    // row for `fmt`, and a source that already sits where it would land (the
    // in-place sibling case) is simply claimed, not copied. It cannot
    // overwrite anything: `free_format_stem` never picks a name another file
    // occupies.
    if !cache.add_format(dest_id, src_path, fmt, false)? {
        bail!("the destination already has a {fmt} format");
    }
    let landed = cache
        .format_file_info(dest_id)?
        .into_iter()
        .find(|f| f.fmt.eq_ignore_ascii_case(fmt))
        .map(|f| f.path)
        .with_context(|| format!("the destination has no {fmt} after the copy"))?;
    #[cfg(test)]
    if CORRUPT_COPY.with(|c| c.get()) && !lexmechanic_utils::filenames::samefile(&landed, src_path) {
        std::fs::write(&landed, b"corrupted in flight")?;
    }
    let got = digest(&landed).and_then(|d| if d.hash == want.hash && d.size == want.size { Ok(()) } else { bail!("size or content differs") });
    if let Err(e) = got {
        if !lexmechanic_utils::filenames::samefile(&landed, src_path) {
            let _ = cache.remove_format_permanently(dest_id, fmt);
        }
        bail!("the copy of {fmt} in the destination could not be verified ({e:#}); it was removed and the source left alone");
    }
    Ok(landed)
}

fn has_own_data_files(cache: &Cache, book_id: i32) -> bool {
    crate::extra_files::list_extra_files(cache, book_id, crate::constants::DATA_FILE_PATTERN).map(|v| !v.is_empty()).unwrap_or(false)
}

/// calibre's `merge_extra_files`: copy the source's data files into the
/// destination (conflicts auto-renamed) and verify. The originals are moved
/// to the in-library trash with the source book when it is deleted. Skipped, with a warning, when the source
/// shares its folder with other books, because `data/` is then theirs too.
fn merge_data_files(cache: &Cache, dest_id: i32, sr: &mut SourceReport, dry: bool) -> Result<()> {
    let path = cache.field_for(sr.book_id, "path")?.unwrap_or_default();
    if !path.is_empty() && !cache.folder_is_dedicated(sr.book_id, &path)? {
        if has_own_data_files(cache, sr.book_id) {
            sr.warnings.push("it shares its folder with other books, so the folder's data files were not moved".to_string());
        }
        return Ok(());
    }
    let files = crate::extra_files::list_extra_files(cache, sr.book_id, crate::constants::DATA_FILE_PATTERN)?;
    for file in files {
        if dry {
            sr.data_files_moved.push(file.relpath.clone());
            sr.trashed.push(rel(cache, &file.file_path));
            continue;
        }
        let want = digest(&file.file_path)?;
        let (relpath, hash) = crate::extra_files::copy_extra_file_in(cache, dest_id, &file.file_path, &file.relpath)?;
        if hash != want.hash {
            bail!("the copy of {} does not match the original", file.relpath);
        }
        sr.data_files_moved.push(relpath);
        // The original is *not* removed here: it stays until the source book
        // is deleted, which moves it into the library's own `.caltrash`
        // together with the book (a restore brings it back). It used to go to
        // the OS recycle bin, which a network share does not have, so the
        // file was left behind and the source's folder could not go.
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

fn is_unknown(s: &str) -> bool {
    let s = s.trim();
    s.is_empty() || s.eq_ignore_ascii_case("unknown")
}

/// calibre's `is_date_undefined`: absent, or in year 101 or earlier.
fn is_null_date(s: Option<&str>) -> bool {
    match s.map(str::trim) {
        None | Some("") | Some("None") => true,
        Some(s) => s.split(['-', 'T', ' ']).next().and_then(|y| y.parse::<i32>().ok()).is_some_and(|y| y <= 101),
    }
}

fn union_list(dest: &[String], extra: &[String]) -> Vec<String> {
    let mut out: Vec<String> = dest.to_vec();
    for item in extra {
        if !out.iter().any(|o| o.eq_ignore_ascii_case(item)) {
            out.push(item.clone());
        }
    }
    out
}

fn split(s: &Option<String>, sep: &str) -> Vec<String> {
    s.as_deref().unwrap_or("").split(sep).map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect()
}

fn parse_identifiers(s: &Option<String>) -> Vec<(String, String)> {
    s.as_deref().unwrap_or("").split(',').filter_map(|p| p.trim().split_once(':')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn append_comments(dest: Option<String>, orig_dest: &Option<String>, src: Option<String>) -> Option<String> {
    let src = src?;
    if orig_dest.as_deref() == Some(src.as_str()) {
        return None;
    }
    match dest {
        None => Some(src),
        Some(d) if d.contains(&src) => None,
        Some(d) => Some(format!("{d}\n\n{src}")),
    }
}

/// `merge_book_metadata`: see the module docs for the rules.
fn merge_metadata(cache: &Cache, dest: i32, sources: &[i32], report: &mut MergeReport) -> Result<()> {
    let mut changed: Vec<String> = Vec::new();
    let mut set = |cache: &Cache, field: &str, value: &str| -> Result<()> {
        cache.set_field(dest, field, value)?;
        if !changed.iter().any(|c| c == field) {
            changed.push(field.to_string());
        }
        Ok(())
    };
    let get = |id: i32, field: &str| -> Result<Option<String>> { Ok(clean(cache.field_for(id, field)?)) };

    let orig_comments = get(dest, "comments")?;
    for &src in sources {
        if let Some(c) = append_comments(get(dest, "comments")?, &orig_comments, get(src, "comments")?) {
            set(cache, "comments", &c)?;
        }
        if get(dest, "title")?.is_none_or(|t| is_unknown(&t)) {
            if let Some(t) = get(src, "title")?.filter(|t| !is_unknown(t)) {
                set(cache, "title", &t)?;
                if let Some(s) = get(src, "sort")? {
                    set(cache, "sort", &s)?;
                }
            }
        }
        let dest_authors = get(dest, "authors")?;
        if dest_authors.as_deref().is_none_or(|a| a.split(" & ").next().is_none_or(is_unknown)) {
            if let Some(a) = get(src, "authors")?.filter(|a| !a.split(" & ").next().is_none_or(is_unknown)) {
                set(cache, "authors", &a)?;
                if let Some(s) = get(src, "author_sort")? {
                    set(cache, "author_sort", &s)?;
                }
            }
        }
        let tags = split(&get(dest, "tags")?, ", ");
        let merged = union_list(&tags, &split(&get(src, "tags")?, ", "));
        if merged.len() != tags.len() {
            set(cache, "tags", &merged.join(", "))?;
        }
        let langs = split(&get(dest, "languages")?, ", ");
        let merged = union_list(&langs, &split(&get(src, "languages")?, ", "));
        if merged.len() != langs.len() {
            set(cache, "languages", &merged.join(", "))?;
        }
        for field in ["publisher", "rating"] {
            if get(dest, field)?.is_none_or(|v| v == "0") {
                if let Some(v) = get(src, field)?.filter(|v| v != "0") {
                    set(cache, field, &v)?;
                }
            }
        }
        if get(dest, "series")?.is_none() {
            if let Some(s) = get(src, "series")? {
                set(cache, "series", &s)?;
                if let Some(i) = get(src, "series_index")? {
                    set(cache, "series_index", &i)?;
                }
            }
        }
        if is_null_date(get(dest, "pubdate")?.as_deref()) {
            if let Some(p) = get(src, "pubdate")?.filter(|p| !is_null_date(Some(p))) {
                set(cache, "pubdate", &p)?;
            }
        }
        let mut idents = parse_identifiers(&get(dest, "identifiers")?);
        let before = idents.len();
        for (k, v) in parse_identifiers(&get(src, "identifiers")?) {
            if !idents.iter().any(|(ek, _)| *ek == k) {
                idents.push((k, v));
            }
        }
        if idents.len() != before {
            set(cache, "identifiers", &idents.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(","))?;
        }
    }

    // Cover: the destination's, else the first source's that has one.
    if !cover_present(cache, dest) {
        for &src in sources {
            if cover_present(cache, src) {
                let bytes = std::fs::read(crate::covers::cover_path(cache, src)?)?;
                crate::covers::set_cover(cache, dest, &bytes)?;
                changed.push("cover".to_string());
                report.cover_from = Some(src);
                break;
            }
        }
    }

    // Custom columns.
    let mut labels: Vec<(String, serde_json::Value)> = cache.custom_column_label_map()?.into_iter().collect();
    labels.sort_by(|a, b| a.0.cmp(&b.0));
    for (label, info) in labels {
        let datatype = info["datatype"].as_str().unwrap_or("").to_string();
        let multiple = info["is_multiple"].as_bool().unwrap_or(false);
        if datatype == "composite" || info["mark_for_delete"].as_bool().unwrap_or(false) {
            continue;
        }
        let key = format!("#{label}");
        let read = |id: i32| -> Result<Option<String>> { Ok(clean(cache.get_custom_column_value(id, &label)?)) };
        let orig = read(dest)?;
        for &src in sources {
            let (dv, sv) = (read(dest)?, read(src)?);
            let Some(sv) = sv else { continue };
            let write = |value: &str| -> Result<()> { cache.set_custom_column_value(dest, &label, value) };
            let wrote = match datatype.as_str() {
                "comments" => match append_comments(dv.clone(), &orig, Some(sv.clone())) {
                    Some(c) => {
                        write(&c)?;
                        true
                    }
                    None => false,
                },
                "bool" | "int" | "float" | "rating" | "datetime" | "enumeration" if dv.is_none() => {
                    write(&sv)?;
                    true
                }
                "series" if dv.is_none() => {
                    write(&sv)?;
                    if let Some(idx) = cache.get_custom_series_index(src, &label)? {
                        cache.set_custom_column_value(dest, &format!("{label}_index"), &idx.to_string())?;
                    }
                    true
                }
                "text" if !multiple && dv.is_none() => {
                    write(&sv)?;
                    true
                }
                "text" if multiple => {
                    let def = cache.custom_column_def(&label)?.context("column vanished")?;
                    let (sep, joiner) = def.separators();
                    let dl = split(&dv, sep);
                    let merged = union_list(&dl, &split(&Some(sv), sep));
                    if merged.len() != dl.len() {
                        write(&merged.join(joiner))?;
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if wrote && !changed.contains(&key) {
                changed.push(key.clone());
            }
        }
    }
    report.metadata_changed = changed;
    Ok(())
}

fn cover_present(cache: &Cache, book_id: i32) -> bool {
    cache.has_cover(book_id).unwrap_or(false) && crate::covers::cover_path(cache, book_id).map(|p| p.is_file()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Choosing a destination, and bulk merging of exact-content groups
// ---------------------------------------------------------------------------

/// What makes one record a better survivor than another. The web UI reads
/// the same three numbers (from the duplicates scan) and applies
/// [`best_destination`]'s rule, so the two agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BookSummary {
    pub book_id: i32,
    pub formats: u32,
    /// How many of authors (known), series, publisher, tags, comments,
    /// rating, identifiers, languages are filled in.
    pub metadata: u32,
    pub has_cover: bool,
}

pub fn book_summary(cache: &Cache, book_id: i32) -> Result<BookSummary> {
    let filled = |field: &str| -> Result<bool> { Ok(clean(cache.field_for(book_id, field)?).is_some_and(|v| v != "0")) };
    let mut metadata = 0;
    for field in ["series", "publisher", "tags", "comments", "rating", "identifiers", "languages"] {
        metadata += filled(field)? as u32;
    }
    if clean(cache.field_for(book_id, "authors")?).is_some_and(|a| !is_unknown(a.split(" & ").next().unwrap_or(""))) {
        metadata += 1;
    }
    Ok(BookSummary { book_id, formats: cache.format_file_info(book_id)?.len() as u32, metadata, has_cover: cover_present(cache, book_id) })
}

/// The record to merge the others into: most formats, then most metadata,
/// then has a cover, then the oldest (lowest id).
pub fn best_destination(books: &[BookSummary]) -> Option<i32> {
    books.iter().max_by(|a, b| (a.formats, a.metadata, a.has_cover).cmp(&(b.formats, b.metadata, b.has_cover)).then(b.book_id.cmp(&a.book_id))).map(|b| b.book_id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedGroup {
    pub dest: i32,
    pub sources: Vec<i32>,
}

/// One planned merge per set of books linked by byte-identical files
/// (overlapping content groups are joined, so a book never appears in two
/// merges). **Exact-content groups only**: title/author matches are never
/// planned. Sorted, so the plan is stable.
pub fn plan_content_merges(cache: &Cache) -> Result<Vec<PlannedGroup>> {
    let groups = crate::copy_to_library::scan_library_for_content_duplicates(cache)?;
    let mut parent: HashMap<i32, i32> = HashMap::new();
    fn find(parent: &mut HashMap<i32, i32>, x: i32) -> i32 {
        let p = *parent.entry(x).or_insert(x);
        if p == x {
            return x;
        }
        let root = find(parent, p);
        parent.insert(x, root);
        root
    }
    for group in &groups {
        for pair in group.windows(2) {
            let (a, b) = (find(&mut parent, pair[0]), find(&mut parent, pair[1]));
            if a != b {
                parent.insert(a.max(b), a.min(b));
            }
        }
    }
    let ids: Vec<i32> = parent.keys().copied().collect();
    let mut components: HashMap<i32, Vec<i32>> = HashMap::new();
    for id in ids {
        let root = find(&mut parent, id);
        components.entry(root).or_default().push(id);
    }
    let mut plan = Vec::new();
    for (_, mut members) in components {
        members.sort_unstable();
        let summaries: Vec<BookSummary> = members.iter().map(|id| book_summary(cache, *id)).collect::<Result<_>>()?;
        let dest = best_destination(&summaries).expect("a component is never empty");
        plan.push(PlannedGroup { dest, sources: members.into_iter().filter(|m| *m != dest).collect() });
    }
    plan.sort_by_key(|g| g.dest.min(g.sources.iter().copied().min().unwrap_or(g.dest)));
    Ok(plan)
}

/// A stable fingerprint of a plan, so an execute request can say which plan
/// the user confirmed and be refused if the library has since changed.
pub fn plan_fingerprint(plan: &[PlannedGroup]) -> String {
    let mut h = blake3::Hasher::new();
    for g in plan {
        h.update(format!("{}<-{:?};", g.dest, g.sources).as_bytes());
    }
    h.finalize().to_hex().to_string()[..16].to_string()
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct BulkReport {
    pub groups_planned: usize,
    pub groups_merged: usize,
    pub books_merged: usize,
    pub trashed: Vec<String>,
    /// `(dest, error)` for groups that failed in whole or in part.
    pub errors: Vec<(i32, String)>,
    pub cancelled: bool,
}

/// Runs `plan` (mode [`MergeMode::MergeAndDelete`]) group by group,
/// stopping between groups when `control` is cancelled. A group that fails
/// is recorded and the next one runs; progress is `total` / `seen` / `errors`
/// on `control`.
pub fn execute_content_merges(cache: &Cache, plan: &[PlannedGroup], keep_duplicate_formats: bool, control: &crate::scan_pipeline::JobControl) -> BulkReport {
    use std::sync::atomic::Ordering;
    let mut out = BulkReport { groups_planned: plan.len(), ..Default::default() };
    control.total.store(plan.len() as u64, Ordering::Relaxed);
    control.set_phase("merging");
    let opts = MergeOptions { mode: MergeMode::MergeAndDelete, keep_duplicate_formats, dry_run: false };
    for group in plan {
        if control.is_cancelled() {
            out.cancelled = true;
            break;
        }
        match merge_books(cache, group.dest, &group.sources, &opts) {
            Ok(report) => {
                out.trashed.extend(report.trashed());
                let mut ok = true;
                for s in &report.sources {
                    if let Some(e) = &s.error {
                        out.errors.push((group.dest, format!("book {}: {e}", s.book_id)));
                        ok = false;
                    } else if s.deleted {
                        out.books_merged += 1;
                    }
                }
                if ok {
                    out.groups_merged += 1;
                } else {
                    control.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(e) => {
                out.errors.push((group.dest, format!("{e:#}")));
                control.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        control.seen.fetch_add(1, Ordering::Relaxed);
    }
    out
}

#[cfg(test)]
mod tests;
