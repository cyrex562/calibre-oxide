//! Port of `old_src/src/calibre/db/copy_to_library.py`'s `copy_one_book`
//! (issue #221, a #201 follow-up).
//!
//! # Scope of this pass
//!
//! Upstream's real `copy_one_book` also copies extra/format files,
//! conversion options, and annotations, and supports a full
//! `duplicate_action` enum (`add`/`add_formats_to_existing`/others,
//! the latter driving an "automerge" path that adds incoming formats
//! to an existing identical book rather than skipping). This crate's
//! pre-existing `copy_one_book` signature is already narrower than
//! that (a plain `check_duplicates: bool`, not the real enum, and no
//! format-file copying at all -- `add_book`, the free function this
//! calls, is a DB-row-only helper, not `Cache::add_book`), and this
//! pass keeps that narrower shape rather than expanding it. What's
//! real now: when `check_duplicates` is true and
//! [`crate::utils::find_identical_books`] (real, tested, previously
//! unused here) finds a same-author/near-same-title match in the
//! destination library, the copy is skipped (`Ok(None)`) instead of
//! silently proceeding as if no duplicate existed -- matching
//! upstream's `duplicate_action != 'add'` branch's simplest case
//! (report + skip), not the `add_formats_to_existing` automerge case.
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

use crate::adding::add_book;
use crate::cache::Cache;
use crate::utils::find_identical_books;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use std::collections::HashSet;

/// Builds the three lookup maps [`find_identical_books`] needs
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

/// Real duplicate detection (issue #424's `cdb_add_book`), reusing
/// the exact same author-intersection algorithm [`copy_one_book`]'s
/// own `check_duplicates` path already uses -- see this module's own
/// doc for what's narrower here than upstream's real
/// `find_identical_books` (a same-author/near-same-title match, not
/// upstream's full fuzzy-title heuristics).
pub fn find_duplicate_books(cache: &Cache, title: &str, authors: &[String]) -> Result<HashSet<i32>> {
    let (author_map, aid_to_bids, title_map) = duplicate_detection_maps(cache)?;
    Ok(find_identical_books(title, authors, &author_map, &aid_to_bids, &title_map))
}

/// Real whole-library duplicate scan (issue #762), reusing
/// [`duplicate_detection_maps`]/[`find_identical_books`] exactly as
/// they already are for the per-candidate add-time check -- the maps
/// are built once (a real, disclosed O(n) full-table-scan, matching
/// [`duplicate_detection_maps`]'s own doc), then every book is matched
/// against them once, which is O(n) map lookups per book rather than a
/// naive O(n²) re-scan. Returns each group of 2+ books this crate's
/// existing same-author/near-same-title heuristic considers likely
/// duplicates of each other, sorted by book id for stable output; a
/// book with no match to any other book isn't returned at all.
pub fn scan_library_for_duplicates(cache: &Cache) -> Result<Vec<Vec<i32>>> {
    let (author_map, aid_to_bids, title_map) = duplicate_detection_maps(cache)?;

    let mut seen: HashSet<i32> = HashSet::new();
    let mut groups: Vec<Vec<i32>> = Vec::new();
    for (&book_id, title) in &title_map {
        if seen.contains(&book_id) {
            continue;
        }
        let authors = real_authors(cache, book_id)?;
        let matches = find_identical_books(title, &authors, &author_map, &aid_to_bids, &title_map);
        for &id in &matches {
            seen.insert(id);
        }
        if matches.len() > 1 {
            let mut group: Vec<i32> = matches.into_iter().collect();
            group.sort_unstable();
            groups.push(group);
        }
    }
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

pub fn copy_one_book(
    src_cache: &Cache,
    dest_cache: &Cache,
    book_id: i32,
    check_duplicates: bool,
) -> Result<Option<i32>> {
    // 1. Fetch Source Data
    let (title, authors, sort, author_sort, uuid, _path_rel) = {
        let backend = &src_cache.backend;
        let title = backend.field_for(book_id, "title")?.unwrap_or_default();
        let sort = backend.field_for(book_id, "sort")?.unwrap_or_default();
        let author_sort = backend
            .field_for(book_id, "author_sort")?
            .unwrap_or_default();
        let uuid = backend.field_for(book_id, "uuid")?.unwrap_or_default();
        let path = backend
            .field_for(book_id, "path")?
            .context("No path info")?;
        let authors = real_authors(src_cache, book_id)?;

        (title, authors, sort, author_sort, uuid, path)
    };

    // 2. Check Duplicates in Dest
    if check_duplicates {
        let (author_map, aid_to_bids, title_map) = duplicate_detection_maps(dest_cache)?;
        let matches = find_identical_books(&title, &authors, &author_map, &aid_to_bids, &title_map);
        if !matches.is_empty() {
            // A same-author/near-same-title book already exists in the
            // destination -- report "duplicate, nothing added" rather
            // than the (larger, separate) automerge path real
            // calibre's `add_formats_to_existing` action takes.
            return Ok(None);
        }
    }

    // 3. Add to Dest
    // add_book generates a new book_id and inserts into `books` table
    let new_book_id = add_book(dest_cache, &title, &authors)?;

    // 4. Update core metadata
    dest_cache.backend.update(new_book_id, "sort", &sort)?;
    dest_cache
        .backend
        .update(new_book_id, "author_sort", &author_sort)?;
    // We usually want to preserve UUID or generate new one? copy logic usually preserves.
    dest_cache.backend.update(new_book_id, "uuid", &uuid)?;

    // 5. Copy Files: not yet real -- `add_book` (the free function
    // above) doesn't set the new book's `path` at all, so there's
    // nowhere to copy files *to* yet. Out of scope for #221
    // (duplicate detection); needs `add_book` here to become a real
    // `Cache::add_book`-style call before this can do anything.

    Ok(Some(new_book_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;

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
mod content_duplicate_tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;

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
