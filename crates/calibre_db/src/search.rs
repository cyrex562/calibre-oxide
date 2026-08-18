use crate::cache::Cache;
use std::sync::{Arc, Mutex};

/// Stand-in for `calibre.db.search.SearchQueryParser`: currently only
/// case-insensitive substring matching on `title`, via a real SQL
/// query over every book (not a hardcoded id range -- the comments
/// that used to sit here described an "assume ids are 1..100"
/// fallback that was never actually implemented; the query below
/// already covers every book). Real calibre's search syntax
/// (`author:`, `tag:`, date ranges, boolean operators, saved
/// searches -- `search.py`, ~1000 lines) is a separate, larger
/// follow-up; this is not it.
pub fn search(cache: &Arc<Mutex<Cache>>, query: &str) -> anyhow::Result<Vec<i32>> {
    let query_lower = query.to_lowercase();
    let cache_guard = cache.lock().unwrap();
    let conn = cache_guard.backend.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT id FROM books WHERE lower(title) LIKE ?")?;
    let like_query = format!("%{query_lower}%");
    let rows = stmt.query_map([&like_query], |row| row.get(0))?;
    rows.collect::<rusqlite::Result<Vec<i32>>>()
        .map_err(Into::into)
}
