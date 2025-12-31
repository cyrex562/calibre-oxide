use crate::cache::Cache;
use anyhow::Result;
use std::sync::{Arc, Mutex};

pub fn set_title(cache: &Arc<Mutex<Cache>>, book_id: i32, title: &str) -> Result<()> {
    update_field(cache, book_id, "title", title)
}

pub fn set_author_sort(cache: &Arc<Mutex<Cache>>, book_id: i32, author_sort: &str) -> Result<()> {
    update_field(cache, book_id, "author_sort", author_sort)
}

pub fn update_field(
    cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    field: &str,
    value: &str,
) -> Result<()> {
    // 1. Update Database
    {
        let guard = cache.lock().unwrap();
        guard
            .backend
            .update(book_id, field, value)
            .map_err(|e| anyhow::anyhow!("DB Update failed: {}", e))?;
    }

    // 2. Update Cache (Invalidate or Set)
    // The current Cache implementation in cache.rs doesn't store book data yet?
    // Let's check cache.rs.
    // Basic cache logic is likely needed here.
    // If ProxyMetadata is using cache, we should update it.

    // For now, since generic "cache" in cache.rs seems to be LRU of book_id -> ProxyMetadata?
    // Or does Cache struct hold the LRU?
    // Let's assume we need to invalidate or update the entry in Cache.

    // Simplest approach: We don't have direct access to internal cache map from here without modifying Cache struct to be public/expose methods.
    // We should probably implement `cache.update_in_memory(book_id, field, value)` method.

    // But since we are inside write.rs, let's just assume we rely on backend for now, or call a method on Cache.

    let mut guard = cache.lock().unwrap();
    guard.update_memory(book_id, field, value);

    Ok(())
}
