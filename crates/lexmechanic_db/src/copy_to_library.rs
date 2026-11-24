//! Port of `old_src/src/calibre/db/copy_to_library.py`'s `copy_one_book`
//! (issue #221, a #201 follow-up).
//!
//! # Scope
//!
//! [`copy_one_book_detailed`] copies a book through the destination's
//! own write paths: every format file, the cover, title/authors/series/
//! tags/publisher/rating/identifiers/comments/languages/dates, and custom
//! column values where the destination has a same-labelled, same-typed
//! column (the rest are reported, not copied). The destination book gets a
//! uuid of its own and every write is change-logged (#965; before that it
//! created a bare `books` row with no files, which made a *move* delete
//! the source's only copy). [`remove_source_after_copy`] is what a move
//! does afterwards: it verifies every format byte-for-byte first.
//!
//! Still not ported from upstream's `copy_one_book`: extra files,
//! conversion options and annotations, and the `add_formats_to_existing`
//! automerge action. Duplicate handling is a plain
//! `check_duplicates: bool`: when true and
//! [`find_duplicate_books`] finds a same-author, same-normalised-title match
//! in the destination (the [`crate::dedupe`] exact tier, since tranche 8;
//! before that [`crate::utils::find_identical_books`]), the copy is skipped
//! (`Ok(None)`).
//!
//! Also fixed while wiring this in: the source book's author list was
//! a documented heuristic (`vec![author_sort]`, treating the whole
//! joined `author_sort` string as a single author name) rather than
//! the real per-author list -- inaccurate for any multi-author book,
//! and directly undermines duplicate detection's author-intersection
//! step. Now queries the real `authors`/`books_authors_link` tables,
//! same join order `Cache::field_for`'s `authors` field already uses.
//!
//! Building the destination library's author/title maps is a real,
//! disclosed O(n) full-table-scan simplification (three `SELECT *`
//! queries), not indexed/incremental lookups -- fine for the
//! correctness this issue is about, not a performance pass.

use crate::cache::Cache;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::collections::HashSet;

/// Builds the three lookup maps [`crate::utils::find_identical_books`] needs
/// (lowercase author name -> author ids, author id -> book ids, book
/// id -> title) from every book currently in `cache`.
pub(crate) fn duplicate_detection_maps(
    cache: &Cache,
) -> Result<(
    IndexMap<String, Vec<i32>>,
    IndexMap<i32, Vec<i32>>,
    IndexMap<i32, String>,
)> {
    let conn = cache.backend.conn.lock().unwrap();

    let mut author_map: IndexMap<String, Vec<i32>> = IndexMap::new();
    let mut stmt = conn.prepare("SELECT id, name FROM authors")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, name) = row?;
        author_map
            .entry(name.trim().to_lowercase())
            .or_default()
            .push(id);
    }
    drop(stmt);

    let mut aid_to_bids: IndexMap<i32, Vec<i32>> = IndexMap::new();
    let mut stmt = conn.prepare("SELECT author, book FROM books_authors_link")?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))?;
    for row in rows {
        let (author_id, book_id) = row?;
        aid_to_bids.entry(author_id).or_default().push(book_id);
    }
    drop(stmt);

    let mut title_map: IndexMap<i32, String> = IndexMap::new();
    let mut stmt = conn.prepare("SELECT id, title FROM books")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, title) = row?;
        title_map.insert(id, title);
    }

    Ok((author_map, aid_to_bids, title_map))
}

/// The add-time duplicate check (issue #424's `cdb_add_book`, and
/// [`copy_one_book`]'s `check_duplicates`): books already in `cache` with the
/// same *normalised* title and an overlapping author -- the
/// [`crate::dedupe`] exact tier, not its fuzzy one, so adding a book is only
/// refused on strong evidence. `The Hobbit` matches `Hobbit, The (z-lib.org)`,
/// `Tolkien, J.R.R.` matches `J. R. R. Tolkien`, and an `Unknown` author
/// matches nothing.
///
/// Differs from `calibre.db.utils.find_identical_books` (which
/// [`crate::utils::find_identical_books`] still ports for `legacy.rs`) in the
/// ways `dedupe`'s module doc lists; the one that matters here is that *any*
/// shared author matches rather than all of them.
pub fn find_duplicate_books(cache: &Cache, title: &str, authors: &[String]) -> Result<HashSet<i32>> {
    Ok(crate::dedupe::ExactIndex::build(cache)?.lookup(title, authors))
}

/// Real whole-library duplicate scan (issue #762): each group of 2+ books
/// whose normalised titles and authors match exactly ([`crate::dedupe`]'s
/// `title_author_exact` tier, with identifiers and fuzzy matching left out;
/// [`crate::dedupe::scan_library`] has everything). Sorted by lowest book id;
/// a book matching nothing is not returned.
pub fn scan_library_for_duplicates(cache: &Cache) -> Result<Vec<Vec<i32>>> {
    let mut records = crate::dedupe::load_records(cache)?;
    for r in &mut records {
        r.identifiers.clear();
    }
    let mut groups: Vec<Vec<i32>> = crate::dedupe::find_groups(&records, &[], crate::dedupe::Strength::Exact).into_iter().map(|g| g.book_ids).collect();
    groups.sort_by_key(|g| g[0]);
    Ok(groups)
}

/// Why two books were reported as duplicates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateKind {
    /// Their titles and authors match. What upstream's Find Duplicates
    /// compares, and what this crate compared exclusively until #892.
    Metadata,
    /// They hold a byte-identical file. Strictly stronger evidence, and
    /// it needs no metadata to have been filled in at all.
    Content,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateGroup {
    pub book_ids: Vec<i32>,
    pub kind: DuplicateKind,
}

/// Books holding byte-identical files (#892).
///
/// Complements [`scan_library_for_duplicates`], which compares titles
/// and authors, and fixes both of that approach's blind spots: it misses
/// identical files whose metadata was filled in differently, and it
/// pairs genuinely different books that happen to share a title.
///
/// Only sees books whose formats have a recorded hash. One added before
/// checksums existed, or by real calibre (which never writes this
/// sidecar), is invisible here rather than reported as unique --
/// the same limitation `check_library`'s corruption check has.
pub fn scan_library_for_content_duplicates(cache: &Cache) -> Result<Vec<Vec<i32>>> {
    let mut groups: Vec<Vec<i32>> = cache.checksums().books_sharing_content("format")?.into_iter().map(|(_, ids)| ids).collect();
    groups.sort_by_key(|g| g[0]);
    groups.dedup();
    Ok(groups)
}

/// Both kinds of duplicate in one pass, content first.
///
/// Content is reported ahead of metadata, and a pair found by content is
/// not reported again by metadata: byte-identical is the stronger claim,
/// and telling somebody the same two books are duplicates twice for two
/// reasons is noise rather than information.
pub fn scan_library_for_all_duplicates(cache: &Cache) -> Result<Vec<DuplicateGroup>> {
    let content = scan_library_for_content_duplicates(cache)?;
    let mut reported: HashSet<i32> = HashSet::new();
    let mut groups: Vec<DuplicateGroup> = Vec::new();

    for ids in content {
        reported.extend(ids.iter().copied());
        groups.push(DuplicateGroup { book_ids: ids, kind: DuplicateKind::Content });
    }

    for ids in scan_library_for_duplicates(cache)? {
        // Skip a metadata group whose books are already accounted for by
        // content; keep one that brings in a book content did not.
        if ids.iter().all(|id| reported.contains(id)) {
            continue;
        }
        groups.push(DuplicateGroup { book_ids: ids, kind: DuplicateKind::Metadata });
    }
    Ok(groups)
}

/// `(title, authors)` for one book id -- port of upstream's
/// `{'title': m.title, 'authors': m.authors}` per-duplicate report
/// shape.
pub fn book_title_and_authors(cache: &Cache, book_id: i32) -> Result<(String, Vec<String>)> {
    let title = cache.field_for(book_id, "title")?.unwrap_or_default();
    let authors = real_authors(cache, book_id)?;
    Ok((title, authors))
}

/// The real per-author names for `book_id` (`authors` joined through
/// `books_authors_link`, in link order) -- not `author_sort`, which is
/// a single free-text field that may not even be one name per author.
fn real_authors(cache: &Cache, book_id: i32) -> Result<Vec<String>> {
    let conn = cache.backend.conn.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT authors.name FROM books_authors_link \
         JOIN authors ON authors.id = books_authors_link.author \
         WHERE books_authors_link.book = ? ORDER BY books_authors_link.id",
    )?;
    let rows = stmt.query_map([book_id], |row| row.get::<_, String>(0))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

/// What [`copy_one_book_detailed`] created in the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CopiedBook {
    /// The new book's id in the destination library.
    pub book_id: i32,
    /// Custom columns the source book has a value for that the
    /// destination has no same-labelled, same-type column for. Their
    /// values were **not** copied.
    pub skipped_custom_columns: Vec<String>,
}

/// Copies one book into `dest_cache`. See [`copy_one_book_detailed`].
pub fn copy_one_book(src_cache: &Cache, dest_cache: &Cache, book_id: i32, check_duplicates: bool) -> Result<Option<i32>> {
    Ok(copy_one_book_detailed(src_cache, dest_cache, book_id, check_duplicates)?.map(|c| c.book_id))
}

struct SourceFormat {
    format: String,
    size: i64,
    path: std::path::PathBuf,
}

fn source_formats(src_cache: &Cache, book_id: i32) -> Result<Vec<SourceFormat>> {
    Ok(src_cache
        .format_file_info(book_id)?
        .into_iter()
        .map(|f| SourceFormat { format: f.fmt, size: f.size, path: f.path })
        .collect())
}

/// Copies one book -- every format file, the cover, the metadata and
/// the custom-column values that have a counterpart -- into
/// `dest_cache`, through the destination's own write paths
/// ([`Cache::add_book`], [`Cache::add_format`], [`Cache::set_field`],
/// [`crate::covers::set_cover`]), so every write is change-logged and
/// the destination book gets a uuid of its own.
///
/// Returns `Ok(None)` when `check_duplicates` is set and the
/// destination already has a same-author/near-same-title book.
///
/// Nothing is created in the destination unless every source format
/// file exists, and if a later step fails the partly-built destination
/// book is removed again; the source is never touched here.
pub fn copy_one_book_detailed(src_cache: &Cache, dest_cache: &Cache, book_id: i32, check_duplicates: bool) -> Result<Option<CopiedBook>> {
    if check_duplicates {
        // One book: one index. A caller copying many builds the index once
        // itself and uses [`copy_one_book_with_index`] (or [`copy_books`]).
        let mut index = crate::dedupe::ExactIndex::build(dest_cache)?;
        copy_one_book_with_index(src_cache, dest_cache, book_id, Some(&mut index))
    } else {
        copy_one_book_with_index(src_cache, dest_cache, book_id, None)
    }
}

/// [`copy_one_book_detailed`] against a duplicate index the caller owns.
///
/// With `Some(index)` a book matching one already in the destination is
/// skipped (`Ok(None)`), and a book that is copied is added to `index`, so
/// the index stays current for the next book -- building it again for every
/// book was O(n^2) over a bulk copy. With `None` nothing is checked.
///
/// The destination book is filed in `Author/Title (id)/` (the layout of
/// [`crate::import`] and of calibre itself), not dropped in the library root.
pub fn copy_one_book_with_index(src_cache: &Cache, dest_cache: &Cache, book_id: i32, mut index: Option<&mut crate::dedupe::ExactIndex>) -> Result<Option<CopiedBook>> {
    // 1. Read the source. Everything is read straight from the tables so
    // that values containing separators survive.
    type Row = (String, Option<String>, Option<String>, f64, Option<String>, Option<String>);
    let (title, sort, author_sort, series_index, timestamp, pubdate): Row = {
        let conn = src_cache.backend.conn.lock().unwrap();
        conn.query_row(
            "SELECT title, sort, author_sort, series_index, timestamp, pubdate FROM books WHERE id = ?1",
            [book_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )
        .with_context(|| format!("book {book_id} not found in the source library"))?
    };
    let authors = real_authors(src_cache, book_id)?;

    // 2. Check Duplicates in Dest
    if let Some(index) = index.as_deref() {
        let matches = index.lookup(&title, &authors);
        if !matches.is_empty() {
            // A same-author, same-normalised-title book already exists in the
            // destination -- report "duplicate, nothing added" rather
            // than the (larger, separate) automerge path real
            // calibre's `add_formats_to_existing` action takes.
            return Ok(None);
        }
    }

    // 3. Every format file must be readable *before* anything is created.
    let formats = source_formats(src_cache, book_id)?;
    for f in &formats {
        if !f.path.is_file() {
            anyhow::bail!("the {} file of book {book_id} is missing from the source library ({}); nothing was copied", f.format, f.path.display());
        }
    }

    let mut mi = lexmechanic_ebooks::metadata::MetaInformation::default();
    mi.title = title;
    if !authors.is_empty() {
        mi.authors = authors;
    }

    // 4. Create the destination book and file it under `Author/Title (id)`:
    //    the row first (the id is in the folder's name), then its formats
    //    through `add_format`, which writes into the book's folder.
    let new_id = dest_cache.add_book_db_entry(&mi, "")?;

    let result = (|| -> Result<Vec<String>> {
        let author = mi.authors.first().map(String::as_str).unwrap_or("Unknown");
        let (folder, _) = crate::cache::import_names(new_id, &mi.title, author);
        dest_cache.place_book_in_folder(new_id, &folder)?;
        for f in &formats {
            if !dest_cache.add_format(new_id, &f.path, &f.format, true)? {
                anyhow::bail!("could not add the {} format to the new book", f.format);
            }
        }

        // 5. Metadata, each through `set_field` so it is logged.
        if let Some(v) = sort.filter(|s| !s.is_empty()) {
            dest_cache.set_field(new_id, "sort", &v)?;
        }
        if let Some(v) = author_sort.filter(|s| !s.is_empty()) {
            dest_cache.set_field(new_id, "author_sort", &v)?;
        }
        dest_cache.set_field(new_id, "series_index", &series_index.to_string())?;
        if let Some(v) = pubdate.filter(|s| !s.is_empty()) {
            dest_cache.set_field(new_id, "pubdate", &v)?;
        }
        let (series, publisher, comments, rating, tags, languages, identifiers) = {
            let conn = src_cache.backend.conn.lock().unwrap();
            let one = |sql: &str| -> rusqlite::Result<Option<String>> {
                match conn.query_row(sql, [book_id], |r| r.get::<_, String>(0)) {
                    Ok(v) => Ok(Some(v)),
                    Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                    Err(e) => Err(e),
                }
            };
            let many = |sql: &str| -> rusqlite::Result<Vec<String>> {
                let mut stmt = conn.prepare(sql)?;
                let rows = stmt.query_map([book_id], |r| r.get::<_, String>(0))?;
                rows.collect()
            };
            (
                one("SELECT s.name FROM series s JOIN books_series_link l ON l.series = s.id WHERE l.book = ?1")?,
                one("SELECT p.name FROM publishers p JOIN books_publishers_link l ON l.publisher = p.id WHERE l.book = ?1")?,
                one("SELECT text FROM comments WHERE book = ?1")?,
                one("SELECT CAST(r.rating AS TEXT) FROM ratings r JOIN books_ratings_link l ON l.rating = r.id WHERE l.book = ?1")?,
                many("SELECT t.name FROM tags t JOIN books_tags_link l ON l.tag = t.id WHERE l.book = ?1 ORDER BY l.id")?,
                many("SELECT g.lang_code FROM languages g JOIN books_languages_link l ON l.lang_code = g.id WHERE l.book = ?1 ORDER BY l.id")?,
                many("SELECT type || ':' || val FROM identifiers WHERE book = ?1")?,
            )
        };
        if let Some(v) = series {
            dest_cache.set_field(new_id, "series", &v)?;
        }
        if let Some(v) = publisher {
            dest_cache.set_field(new_id, "publisher", &v)?;
        }
        if let Some(v) = comments {
            dest_cache.set_field(new_id, "comments", &v)?;
        }
        if let Some(v) = rating {
            dest_cache.set_field(new_id, "rating", &v)?;
        }
        if !tags.is_empty() {
            dest_cache.set_field(new_id, "tags", &tags.join(", "))?;
        }
        if !languages.is_empty() {
            dest_cache.set_field(new_id, "languages", &languages.join(", "))?;
        }
        if !identifiers.is_empty() {
            dest_cache.set_field(new_id, "identifiers", &identifiers.join(","))?;
        }

        // 6. Cover.
        if src_cache.has_cover(book_id).unwrap_or(false) {
            let cover = crate::covers::cover_path(src_cache, book_id)?;
            if cover.is_file() {
                let bytes = std::fs::read(&cover).with_context(|| format!("reading cover {}", cover.display()))?;
                crate::covers::set_cover(dest_cache, new_id, &bytes)?;
            }
        }

        // 7. Custom columns with a same-labelled, same-typed counterpart.
        let dest_columns = dest_cache.custom_column_label_map()?;
        let mut skipped = Vec::new();
        for (label, src_col) in src_cache.custom_column_label_map()? {
            let Some(value) = src_cache.get_custom_column_value(book_id, &label)? else { continue };
            let compatible = dest_columns
                .get(&label)
                .is_some_and(|d| d.get("datatype") == src_col.get("datatype") && d.get("is_multiple") == src_col.get("is_multiple"));
            // A same-typed column can still refuse a value: an enumeration
            // whose list in the destination does not include it. That is a
            // skipped column, not a failed copy.
            let enum_ok = src_col.get("datatype").and_then(|d| d.as_str()) != Some("enumeration")
                || dest_columns
                    .get(&label)
                    .and_then(|d| d["display"]["enum_values"].as_array())
                    .is_some_and(|allowed| allowed.iter().any(|a| a.as_str() == Some(value.as_str())));
            if compatible && enum_ok {
                dest_cache.set_custom_column_value(new_id, &label, &value)?;
                // A series' index travels with its name.
                if let Some(index) = src_cache.get_custom_series_index(book_id, &label)? {
                    dest_cache.set_custom_column_value(new_id, &format!("{label}_index"), &index.to_string())?;
                }
            } else {
                skipped.push(label);
            }
        }
        skipped.sort();

        // 8. The source's timestamp last: adding formats stamped "now".
        if let Some(v) = timestamp.filter(|s| !s.is_empty()) {
            dest_cache.set_field(new_id, "timestamp", &v)?;
        }
        Ok(skipped)
    })();

    match result {
        Ok(skipped_custom_columns) => {
            if let Some(index) = index.as_deref_mut() {
                index.insert(new_id, &mi.title, &mi.authors);
            }
            Ok(Some(CopiedBook { book_id: new_id, skipped_custom_columns }))
        }
        Err(e) => {
            // Do not leave a half-built book behind. It only holds files
            // this function just copied, so sending them to the
            // wastebasket loses nothing.
            if let Err(cleanup) = dest_cache.delete_book(new_id) {
                log::warn!("copy of book {book_id} failed ({e:#}) and the partial copy {new_id} could not be removed: {cleanup:#}");
            }
            Err(e.context(format!("copying book {book_id}")))
        }
    }
}

/// Checks that `dest_id` in `dest_cache` really holds everything
/// `src_id` in `src_cache` does: every format present with the same
/// size and the same content hash (both files are hashed as they are on
/// disk now, not recorded values that could be stale), and a cover if
/// the source has one. This is what a *move* must establish before the
/// source is removed.
pub fn verify_copy(src_cache: &Cache, dest_cache: &Cache, src_id: i32, dest_id: i32) -> Result<()> {
    let dest: std::collections::HashMap<String, crate::cache::FormatFileInfo> =
        dest_cache.format_file_info(dest_id)?.into_iter().map(|f| (f.fmt.to_uppercase(), f)).collect();
    for f in source_formats(src_cache, src_id)? {
        let key = f.format.to_uppercase();
        let d = dest.get(&key).with_context(|| format!("the destination book has no {key} format"))?;
        let dest_len = std::fs::metadata(&d.path).with_context(|| format!("the destination {key} file {} is missing", d.path.display()))?.len();
        let src_len = std::fs::metadata(&f.path).with_context(|| format!("the source {key} file {} is missing", f.path.display()))?.len();
        if dest_len != src_len {
            anyhow::bail!("the destination {key} file is {dest_len} bytes but the source's is {src_len}");
        }
        // Streamed in megabyte chunks: this used to read both whole files
        // into memory, which for a large PDF or audiobook is the file twice.
        let hash = |p: &std::path::Path| -> Result<String> { Ok(crate::scan_pipeline::hash_stream(p, None).with_context(|| format!("reading {}", p.display()))?.hash) };
        if hash(&d.path)? != hash(&f.path)? {
            anyhow::bail!("the destination {key} file differs from the source's");
        }
    }
    if src_cache.has_cover(src_id).unwrap_or(false) && crate::covers::cover_path(src_cache, src_id)?.is_file() {
        let cover = crate::covers::cover_path(dest_cache, dest_id)?;
        if !dest_cache.has_cover(dest_id).unwrap_or(false) || !cover.is_file() {
            anyhow::bail!("the destination book has no cover");
        }
    }
    Ok(())
}

/// Removes `src_id` from the source library **after** verifying that
/// `dest_id` holds a complete copy ([`verify_copy`]). If verification
/// fails the source is left untouched and the error says why. The
/// source's files go to the wastebasket
/// ([`crate::removal::delete_with_file`]); files that could not be
/// trashed are returned in the report's `failed`.
pub fn remove_source_after_copy(src_cache: &Cache, dest_cache: &Cache, src_id: i32, dest_id: i32) -> Result<crate::removal::RemovalReport> {
    verify_copy(src_cache, dest_cache, src_id, dest_id).with_context(|| format!("not removing source book {src_id}: the copy could not be verified"))?;
    crate::removal::delete_with_file(src_cache, src_id)
}

/// What to do in a bulk copy.
#[derive(Debug, Clone, Copy, Default)]
pub struct CopyOptions {
    /// Skip a book the destination already has (same normalised title and an
    /// overlapping author).
    pub check_duplicates: bool,
    /// Remove each source book after its copy has been verified.
    pub move_books: bool,
}

/// What happened to one source book of a bulk copy.
#[derive(Debug, Clone, Default, serde::Serialize, PartialEq, Eq)]
pub struct CopyEntry {
    pub source_id: i32,
    /// The copy (or, for a move, the removal) went through. A duplicate
    /// skipped is `ok` with no `copied_as`.
    pub ok: bool,
    /// The new book's id in the destination.
    pub copied_as: Option<i32>,
    /// Skipped because the destination has it already.
    pub duplicate: bool,
    pub error: Option<String>,
    pub skipped_custom_columns: Vec<String>,
    pub warnings: Vec<String>,
    /// For a move: the source book was removed.
    pub source_removed: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CopyReport {
    pub entries: Vec<CopyEntry>,
    pub copied: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub removed_from_source: usize,
    pub cancelled: bool,
}

/// Copies (or moves) `book_ids` from `src_cache` into `dest_cache`: the cross
/// library copy as a job body. Counts on `control` (`total`, `added` for books
/// copied, `already_known` for duplicates skipped, `errors`) and stops between
/// books when it is cancelled; what was copied stays.
///
/// The duplicate index is built **once** and kept current as books are added,
/// and a failing book is reported without stopping the rest.
pub fn copy_books(src_cache: &Cache, dest_cache: &Cache, book_ids: &[i32], options: CopyOptions, control: &crate::scan_pipeline::JobControl) -> Result<CopyReport> {
    use std::sync::atomic::Ordering;
    control.set_phase("copying");
    control.total.store(book_ids.len() as u64, Ordering::Relaxed);
    let mut index = if options.check_duplicates { Some(crate::dedupe::ExactIndex::build(dest_cache)?) } else { None };
    let mut report = CopyReport::default();
    for &book_id in book_ids {
        if control.is_cancelled() {
            report.cancelled = true;
            break;
        }
        let mut entry = CopyEntry { source_id: book_id, ..Default::default() };
        match copy_one_book_with_index(src_cache, dest_cache, book_id, index.as_mut()) {
            Ok(Some(copied)) => {
                entry.ok = true;
                entry.copied_as = Some(copied.book_id);
                entry.skipped_custom_columns = copied.skipped_custom_columns;
                report.copied += 1;
                control.added.fetch_add(1, Ordering::Relaxed);
                if options.move_books {
                    // The source goes only once the copy has been verified
                    // file by file; otherwise it stays and the entry says why.
                    match remove_source_after_copy(src_cache, dest_cache, book_id, copied.book_id) {
                        Ok(removal) => {
                            entry.source_removed = true;
                            report.removed_from_source += 1;
                            entry.warnings = removal.failed.iter().map(|(p, why)| format!("could not remove {p} from the source library: {why}")).collect();
                        }
                        Err(e) => {
                            entry.ok = false;
                            entry.error = Some(format!("copied to the destination as book {}, but the source book was kept: {e:#}", copied.book_id));
                            report.failed += 1;
                            control.errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
            Ok(None) => {
                entry.ok = true;
                entry.duplicate = true;
                report.duplicates += 1;
                control.already_known.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                entry.error = Some(format!("{e:#}"));
                report.failed += 1;
                control.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
        report.entries.push(entry);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lexmechanic_ebooks::metadata::MetaInformation;

    fn add_book_with(cache: &Cache, dir: &std::path::Path, name: &str, title: &str, authors: &[&str]) -> i32 {
        let source = dir.join(name);
        std::fs::write(&source, b"content").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = authors.iter().map(|s| s.to_string()).collect();
        cache.add_book(&source, &meta).unwrap()
    }

    #[test]
    fn scan_library_for_duplicates_groups_two_real_near_duplicate_books() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let a = add_book_with(&cache, dir.path(), "a.txt", "The Great Test", &["Ada Lovelace"]);
        let b = add_book_with(&cache, dir.path(), "b.txt", "The Great Test", &["Ada Lovelace"]);
        let _unrelated = add_book_with(&cache, dir.path(), "c.txt", "Something Else Entirely", &["Grace Hopper"]);

        let groups = scan_library_for_duplicates(&cache).unwrap();

        assert_eq!(groups.len(), 1, "{groups:?}");
        let mut group = groups[0].clone();
        group.sort_unstable();
        let mut expected = vec![a, b];
        expected.sort_unstable();
        assert_eq!(group, expected);
    }

    #[test]
    fn scan_library_for_duplicates_reports_no_groups_for_a_real_library_of_distinct_books() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        add_book_with(&cache, dir.path(), "a.txt", "Book One", &["Author One"]);
        add_book_with(&cache, dir.path(), "b.txt", "Book Two", &["Author Two"]);

        let groups = scan_library_for_duplicates(&cache).unwrap();

        assert!(groups.is_empty(), "{groups:?}");
    }

    #[test]
    fn scan_library_for_duplicates_finds_a_group_of_three() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        let a = add_book_with(&cache, dir.path(), "a.txt", "Triple Book", &["Same Author"]);
        let b = add_book_with(&cache, dir.path(), "b.txt", "Triple Book", &["Same Author"]);
        let c = add_book_with(&cache, dir.path(), "c.txt", "Triple Book", &["Same Author"]);

        let groups = scan_library_for_duplicates(&cache).unwrap();

        assert_eq!(groups.len(), 1, "{groups:?}");
        let mut group = groups[0].clone();
        group.sort_unstable();
        let mut expected = vec![a, b, c];
        expected.sort_unstable();
        assert_eq!(group, expected);
    }
}

#[cfg(test)]
mod copy_tests {
    use super::*;
    use lexmechanic_ebooks::metadata::MetaInformation;

    struct Fixture {
        _staging: tempfile::TempDir,
        _src_dir: tempfile::TempDir,
        _dest_dir: tempfile::TempDir,
        src: Cache,
        dest: Cache,
        src_id: i32,
    }

    fn fixture() -> Fixture {
        let staging = tempfile::tempdir().unwrap();
        let src_dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let src = Cache::new(src_dir.path()).unwrap();
        let dest = Cache::new(dest_dir.path()).unwrap();

        let epub = staging.path().join("dune.epub");
        let pdf = staging.path().join("dune.pdf");
        std::fs::write(&epub, b"EPUB BYTES \x00\x01\x02").unwrap();
        std::fs::write(&pdf, b"%PDF distinct bytes").unwrap();
        let mut mi = MetaInformation::default();
        mi.title = "Dune".to_string();
        mi.authors = vec!["Frank Herbert".to_string(), "Brian Herbert".to_string()];
        let src_id = src.add_book(&epub, &mi).unwrap();
        src.add_format(src_id, &pdf, "pdf", true).unwrap();
        src.set_field(src_id, "series", "Dune Chronicles").unwrap();
        src.set_field(src_id, "series_index", "1").unwrap();
        src.set_field(src_id, "tags", "scifi, classic").unwrap();
        src.set_field(src_id, "publisher", "Chilton").unwrap();
        src.set_field(src_id, "rating", "8").unwrap();
        src.set_field(src_id, "comments", "Spice.").unwrap();
        src.set_field(src_id, "languages", "eng").unwrap();
        src.set_field(src_id, "identifiers", "isbn:9780441172719").unwrap();
        src.set_field(src_id, "pubdate", "1965-08-01T00:00:00+00:00").unwrap();
        crate::covers::set_cover(&src, src_id, b"JPEG COVER BYTES").unwrap();
        Fixture { _staging: staging, _src_dir: src_dir, _dest_dir: dest_dir, src, dest, src_id }
    }

    fn format_file(cache: &Cache, id: i32, fmt: &str) -> std::path::PathBuf {
        cache.format_file_info(id).unwrap().into_iter().find(|f| f.fmt.eq_ignore_ascii_case(fmt)).unwrap().path
    }

    #[test]
    fn a_copy_carries_every_format_the_cover_and_the_metadata() {
        let f = fixture();
        let copied = copy_one_book_detailed(&f.src, &f.dest, f.src_id, false).unwrap().unwrap();
        let id = copied.book_id;

        // Formats are real files with the same bytes.
        assert_eq!(f.dest.format_file_info(id).unwrap().len(), 2);
        assert_eq!(std::fs::read(format_file(&f.dest, id, "epub")).unwrap(), b"EPUB BYTES \x00\x01\x02");
        assert_eq!(std::fs::read(format_file(&f.dest, id, "pdf")).unwrap(), b"%PDF distinct bytes");
        // Cover.
        assert!(f.dest.has_cover(id).unwrap());
        assert_eq!(std::fs::read(crate::covers::cover_path(&f.dest, id).unwrap()).unwrap(), b"JPEG COVER BYTES");
        // Metadata, with real author links.
        assert_eq!(f.dest.field_for(id, "title").unwrap().unwrap(), "Dune");
        assert_eq!(real_authors(&f.dest, id).unwrap(), vec!["Frank Herbert", "Brian Herbert"]);
        assert_eq!(f.dest.field_for(id, "series").unwrap().unwrap(), "Dune Chronicles");
        assert_eq!(f.dest.field_for(id, "publisher").unwrap().unwrap(), "Chilton");
        assert_eq!(f.dest.field_for(id, "comments").unwrap().unwrap(), "Spice.");
        assert_eq!(f.dest.field_for(id, "rating").unwrap(), f.src.field_for(f.src_id, "rating").unwrap());
        assert_eq!(f.dest.field_for(id, "languages").unwrap(), f.src.field_for(f.src_id, "languages").unwrap());
        assert_eq!(f.dest.field_for(id, "identifiers").unwrap(), f.src.field_for(f.src_id, "identifiers").unwrap());
        assert_eq!(f.dest.field_for(id, "pubdate").unwrap(), f.src.field_for(f.src_id, "pubdate").unwrap());
        let mut tags: Vec<String> = f.dest.field_for(id, "tags").unwrap().unwrap().split(',').map(|t| t.trim().to_string()).collect();
        tags.sort();
        assert_eq!(tags, vec!["classic", "scifi"]);
        // A new identity, not the source's.
        assert_ne!(f.dest.book_uuid(id).unwrap(), f.src.book_uuid(f.src_id).unwrap());
        // The source is untouched by a copy.
        assert_eq!(f.src.format_file_info(f.src_id).unwrap().len(), 2);
    }

    #[test]
    fn custom_columns_copy_only_where_name_and_type_match() {
        let f = fixture();
        f.src.add_custom_column("mood", "Mood", "text", false).unwrap();
        f.src.add_custom_column("pages", "Pages", "int", false).unwrap();
        f.src.add_custom_column("srconly", "Source only", "text", false).unwrap();
        f.dest.add_custom_column("mood", "Mood", "text", false).unwrap();
        f.dest.add_custom_column("pages", "Pages", "text", false).unwrap();
        f.src.set_custom_column_value(f.src_id, "mood", "dusty").unwrap();
        f.src.set_custom_column_value(f.src_id, "pages", "412").unwrap();
        f.src.set_custom_column_value(f.src_id, "srconly", "x").unwrap();

        let copied = copy_one_book_detailed(&f.src, &f.dest, f.src_id, false).unwrap().unwrap();

        assert_eq!(f.dest.get_custom_column_value(copied.book_id, "mood").unwrap().as_deref(), Some("dusty"));
        assert_eq!(f.dest.get_custom_column_value(copied.book_id, "pages").unwrap(), None);
        assert_eq!(copied.skipped_custom_columns, vec!["pages".to_string(), "srconly".to_string()]);
    }

    #[test]
    fn duplicates_are_still_skipped_when_asked_to_check() {
        let f = fixture();
        assert!(copy_one_book_detailed(&f.src, &f.dest, f.src_id, true).unwrap().is_some());
        assert!(copy_one_book_detailed(&f.src, &f.dest, f.src_id, true).unwrap().is_none());
        assert_eq!(f.dest.all_book_ids().unwrap().len(), 1);
    }

    #[test]
    fn a_move_removes_the_source_only_after_verifying_the_copy() {
        let f = fixture();
        let src_epub = format_file(&f.src, f.src_id, "epub");
        let id = copy_one_book(&f.src, &f.dest, f.src_id, false).unwrap().unwrap();

        let report = remove_source_after_copy(&f.src, &f.dest, f.src_id, id).unwrap();

        assert!(report.failed.is_empty(), "{report:?}");
        assert!(f.src.all_book_ids().unwrap().is_empty());
        assert!(!src_epub.exists());
        // The destination still has everything.
        assert_eq!(std::fs::read(format_file(&f.dest, id, "epub")).unwrap(), b"EPUB BYTES \x00\x01\x02");
    }

    #[test]
    fn a_move_keeps_the_source_when_the_destination_does_not_match() {
        let f = fixture();
        let id = copy_one_book(&f.src, &f.dest, f.src_id, false).unwrap().unwrap();
        // Corrupt the destination copy: same size, different bytes.
        std::fs::write(format_file(&f.dest, id, "epub"), b"XXXX BYTES \x00\x01\x02").unwrap();

        let err = remove_source_after_copy(&f.src, &f.dest, f.src_id, id).unwrap_err();

        assert!(format!("{err:#}").contains("differs"), "{err:#}");
        assert_eq!(f.src.all_book_ids().unwrap(), vec![f.src_id]);
        assert!(format_file(&f.src, f.src_id, "epub").exists());

        // A truncated destination fails too.
        std::fs::write(format_file(&f.dest, id, "epub"), b"short").unwrap();
        assert!(remove_source_after_copy(&f.src, &f.dest, f.src_id, id).is_err());
        // And so does a destination format that is missing entirely.
        std::fs::remove_file(format_file(&f.dest, id, "pdf")).unwrap();
        assert!(remove_source_after_copy(&f.src, &f.dest, f.src_id, id).is_err());
        assert_eq!(f.src.all_book_ids().unwrap(), vec![f.src_id]);
    }

    #[test]
    fn a_copy_that_cannot_read_a_source_file_creates_nothing_and_touches_nothing() {
        let f = fixture();
        std::fs::remove_file(format_file(&f.src, f.src_id, "pdf")).unwrap();

        let err = copy_one_book_detailed(&f.src, &f.dest, f.src_id, false).unwrap_err();

        assert!(format!("{err:#}").contains("missing"), "{err:#}");
        assert!(f.dest.all_book_ids().unwrap().is_empty(), "a half-built book was left in the destination");
        assert_eq!(f.src.all_book_ids().unwrap(), vec![f.src_id]);
        assert!(format_file(&f.src, f.src_id, "epub").exists());
    }

    #[test]
    fn the_copy_is_known_to_the_destination_by_its_own_uuid() {
        let f = fixture();
        let id = copy_one_book(&f.src, &f.dest, f.src_id, false).unwrap().unwrap();
        let uuid = f.dest.book_uuid(id).unwrap().unwrap();
        assert_eq!(f.dest.book_id_for_uuid(&uuid).unwrap(), Some(id));
        assert_ne!(Some(uuid), f.src.book_uuid(f.src_id).unwrap());
    }
}

#[cfg(test)]
mod content_duplicate_tests {
    use super::*;
    use lexmechanic_ebooks::metadata::MetaInformation;

    fn add(dir: &std::path::Path, cache: &Cache, title: &str, author: &str, file: &str, bytes: &[u8]) -> i32 {
        let source = dir.join(file);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, bytes).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    fn library() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    /// The blind spot the metadata-only comparison has: the same file
    /// added twice under different titles is invisible to it.
    #[test]
    fn identical_files_with_different_metadata_are_found() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "Boiler Manual", "Acme", "src/one.pdf", b"IDENTICAL BYTES");
        let b = add(dir.path(), &cache, "Untitled Scan 42", "Unknown", "src/two.pdf", b"IDENTICAL BYTES");

        // Metadata alone sees nothing here.
        assert!(scan_library_for_duplicates(&cache).unwrap().is_empty());

        let groups = scan_library_for_content_duplicates(&cache).unwrap();
        assert_eq!(groups, vec![vec![a.min(b), a.max(b)]]);
    }

    /// Zero-byte files all hash alike, and are not the same book: they are
    /// excluded from content groups and listed on their own.
    #[test]
    fn empty_files_are_not_content_duplicates_but_are_listed() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "Empty One", "A", "src/e1.pdf", b"");
        let b = add(dir.path(), &cache, "Empty Two", "B", "src/e2.pdf", b"");
        let c = add(dir.path(), &cache, "Real One", "C", "src/r1.pdf", b"REAL BYTES");
        let d = add(dir.path(), &cache, "Real Two", "D", "src/r2.pdf", b"REAL BYTES");
        let groups = scan_library_for_content_duplicates(&cache).unwrap();
        assert_eq!(groups, vec![vec![c.min(d), c.max(d)]], "only the non-empty pair groups");
        let empties: Vec<i32> = cache.checksums().empty_files("format").unwrap().into_iter().map(|(id, _)| id).collect();
        assert_eq!(empties, vec![a.min(b), a.max(b)]);
    }

    #[test]
    fn different_files_are_not_content_duplicates() {
        let (dir, cache) = library();
        add(dir.path(), &cache, "One", "A", "src/one.pdf", b"FIRST");
        add(dir.path(), &cache, "Two", "B", "src/two.pdf", b"SECOND");
        assert!(scan_library_for_content_duplicates(&cache).unwrap().is_empty());
    }

    /// A book whose own two formats happen to hold identical bytes is
    /// one book, not a duplicate of itself.
    #[test]
    fn a_books_own_identical_formats_are_not_a_duplicate() {
        let (dir, cache) = library();
        let id = add(dir.path(), &cache, "One Book", "A", "src/book.pdf", b"SAME BYTES");
        let epub = dir.path().join("src/book.epub");
        std::fs::write(&epub, b"SAME BYTES").unwrap();
        cache.add_format(id, &epub, "epub", true).unwrap();

        assert!(scan_library_for_content_duplicates(&cache).unwrap().is_empty());
    }

    /// Content is the stronger claim, so a pair found both ways is
    /// reported once, as content.
    #[test]
    fn a_pair_found_both_ways_is_reported_once() {
        let (dir, cache) = library();
        add(dir.path(), &cache, "Same Title", "Same Author", "src/one.pdf", b"IDENTICAL");
        add(dir.path(), &cache, "Same Title", "Same Author", "src/two.pdf", b"IDENTICAL");

        let groups = scan_library_for_all_duplicates(&cache).unwrap();
        assert_eq!(groups.len(), 1, "{groups:?}");
        assert_eq!(groups[0].kind, DuplicateKind::Content);
    }

    /// A metadata group that brings in a book content did not see is
    /// still worth reporting.
    #[test]
    fn a_metadata_only_group_is_still_reported() {
        let (dir, cache) = library();
        add(dir.path(), &cache, "Shared Title", "An Author", "src/one.pdf", b"DIFFERENT ONE");
        add(dir.path(), &cache, "Shared Title", "An Author", "src/two.pdf", b"DIFFERENT TWO");

        let groups = scan_library_for_all_duplicates(&cache).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].kind, DuplicateKind::Metadata);
    }

    #[test]
    fn a_library_with_no_duplicates_reports_none() {
        let (dir, cache) = library();
        add(dir.path(), &cache, "One", "A", "src/one.pdf", b"FIRST");
        assert!(scan_library_for_all_duplicates(&cache).unwrap().is_empty());
    }

    /// Covers are not compared against format files: two books whose
    /// covers match say nothing about their contents.
    #[test]
    fn covers_are_not_compared_with_format_files() {
        let (dir, cache) = library();
        let a = add(dir.path(), &cache, "One", "A", "src/one.pdf", b"SHARED BYTES");
        let b = add(dir.path(), &cache, "Two", "B", "src/two.pdf", b"OTHER");
        // Give the second book a cover whose bytes match the first
        // book's *format* file.
        crate::covers::set_cover(&cache, b, b"SHARED BYTES").unwrap();

        assert!(scan_library_for_content_duplicates(&cache).unwrap().is_empty(), "a cover matched a format file");
        let _ = a;
    }
}
