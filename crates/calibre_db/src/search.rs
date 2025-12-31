use crate::cache::Cache;
use std::sync::{Arc, Mutex};

/// A simplified search function that mimics `calibre.db.search.SearchQueryParser`.
/// currently only supports basic case-insensitive substring matching on the 'title' field.
pub fn search(cache: &Arc<Mutex<Cache>>, query: &str) -> anyhow::Result<Vec<i32>> {
    let query_lower = query.to_lowercase();
    let mut matched_ids = Vec::new();

    // Locking the cache/backend to iterate.
    // In a real implementation we might want to iterate without holding the lock for the whole duration
    // or use a snapshot. For now, we lock.
    let cache_guard = cache.lock().unwrap();

    // We need a way to iterate over all books.
    // Backend doesn't expose iteration yet, so we probably need to fetch all IDs first.
    // For this Sprint, let's assume valid IDs are 1..100 or implement `get_all_ids` in backend.

    // Stub: Let's assume we can get all IDs. We need to add `all_book_ids` to Cache/Backend.
    // For now, let's query the DB for all IDs.

    // Use SQL LIKE for basic title matching

    // Alternative: Use SQL LIKE.
    let conn = cache_guard.backend.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT id FROM books WHERE lower(title) LIKE ?")?;
    let like_query = format!("%{}%", query_lower);
    let rows = stmt.query_map([&like_query], |row| row.get(0))?;

    for r in rows {
        matched_ids.push(r?);
    }

    Ok(matched_ids)
}
