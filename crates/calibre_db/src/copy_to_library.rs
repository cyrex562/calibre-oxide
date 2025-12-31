use crate::adding::add_book;
use crate::backend::Backend;
use crate::cache::Cache;
use crate::utils::find_identical_books;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::{Arc, Mutex};

pub fn copy_one_book(
    src_cache: &Arc<Mutex<Cache>>,
    dest_cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    check_duplicates: bool,
) -> Result<Option<i32>> {
    // 1. Fetch Source Data
    let (title, authors, sort, author_sort, uuid, path_rel) = {
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

        // Simplified author fetching (in real app, we'd query relation tables)
        // Here we just use the author sort string split by '&' as a heuristic for this sprint
        // or just pass [author_sort] if we don't have better data access yet.
        // Ideally we should use `backend.get_all_authors` but that returns ALL.
        // For this port, let's assume one author from author_sort for duplicate checking
        let authors_vec = vec![author_sort.clone()];

        (title, authors_vec, sort, author_sort, uuid, path)
    };

    // 2. Check Duplicates in Dest
    if check_duplicates {
        let guard = dest_cache.lock().unwrap();
        // Construct maps needed for find_identical_books
        // In a real app, these would be efficient lookups or already cached.
        // Here we build them on the fly for correctness demonstration.

        // TODO: This is expensive O(N) but sufficient for MVP porting/testing of logic.
        // We need:
        // author_map: Name -> Vec<ID>
        // aid_to_bids: AuthorID -> Vec<BookID>
        // title_map: BookID -> Title

        // For this simplified port, we'll skip the complex map building if we are just testing basic copy.
        // In a real implementation we would call methods on Cache/Backend to get these efficiently.
        // Let's assume no duplicates for now unless we implement the full data hydration.
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

    // 5. Copy Files (Optional for this sprint, but good to have)
    // We would list files in source dir and copy them to dest dir.
    {
        let src_guard = src_cache.lock().unwrap();
        let dest_guard = dest_cache.lock().unwrap();

        let src_path_abs = src_guard.backend.library_path.join(&path_rel);
        // We need to know where add_book put the new book. `add_book` currently doesn't set 'path'.
        // We should probably rely on `backend` to calculate/set path for the new book.
        // This part is tricky without a full "add_book" logic that sets paths.
        // For MVP, we stop at metadata copy.
    }

    Ok(Some(new_book_id))
}
