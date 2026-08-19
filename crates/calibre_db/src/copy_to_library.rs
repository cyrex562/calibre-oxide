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
use std::sync::{Arc, Mutex};

/// Builds the three lookup maps [`find_identical_books`] needs
/// (lowercase author name -> author ids, author id -> book ids, book
/// id -> title) from every book currently in `cache`.
fn duplicate_detection_maps(
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
    src_cache: &Arc<Mutex<Cache>>,
    dest_cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    check_duplicates: bool,
) -> Result<Option<i32>> {
    // 1. Fetch Source Data
    let (title, authors, sort, author_sort, uuid, _path_rel) = {
        let guard = src_cache.lock().unwrap();
        let backend = &guard.backend;
        let title = backend.field_for(book_id, "title")?.unwrap_or_default();
        let sort = backend.field_for(book_id, "sort")?.unwrap_or_default();
        let author_sort = backend
            .field_for(book_id, "author_sort")?
            .unwrap_or_default();
        let uuid = backend.field_for(book_id, "uuid")?.unwrap_or_default();
        let path = backend
            .field_for(book_id, "path")?
            .context("No path info")?;
        let authors = real_authors(&guard, book_id)?;

        (title, authors, sort, author_sort, uuid, path)
    };

    // 2. Check Duplicates in Dest
    if check_duplicates {
        let guard = dest_cache.lock().unwrap();
        let (author_map, aid_to_bids, title_map) = duplicate_detection_maps(&guard)?;
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
    {
        let guard = dest_cache.lock().unwrap();
        guard.backend.update(new_book_id, "sort", &sort)?;
        guard
            .backend
            .update(new_book_id, "author_sort", &author_sort)?;
        // We usually want to preserve UUID or generate new one? copy logic usually preserves.
        guard.backend.update(new_book_id, "uuid", &uuid)?;
    }

    // 5. Copy Files: not yet real -- `add_book` (the free function
    // above) doesn't set the new book's `path` at all, so there's
    // nowhere to copy files *to* yet. Out of scope for #221
    // (duplicate detection); needs `add_book` here to become a real
    // `Cache::add_book`-style call before this can do anything.

    Ok(Some(new_book_id))
}
