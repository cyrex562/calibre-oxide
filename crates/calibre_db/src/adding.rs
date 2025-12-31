use crate::cache::Cache;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

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
