use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::copy_to_library::copy_one_book;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_copy_book_basic() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    // Setup Src DB: `Backend::new` creates the real calibre schema for
    // a fresh dir -- just seed one row into it.
    let src_cache = {
        let backend = Backend::new(src_dir.path()).unwrap();
        let conn = backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title, sort, author_sort, uuid, path) VALUES ('Source Book', 'Source Book', 'Author A', 'uuid-src', 'book_path')", []).unwrap();
        Arc::new(Mutex::new(Cache::new(src_dir.path()).unwrap()))
    };

    // Setup Dest DB (empty).
    let dest_cache = Arc::new(Mutex::new(Cache::new(dest_dir.path()).unwrap()));

    // Perform Copy
    let new_id = copy_one_book(&src_cache, &dest_cache, 1, false)
        .expect("Copy failed")
        .unwrap();

    // Verify
    {
        let guard = dest_cache.lock().unwrap();
        let title = guard.field_for(new_id, "title").unwrap().unwrap();
        let author = guard.field_for(new_id, "author_sort").unwrap().unwrap();

        assert_eq!(title, "Source Book");
        assert_eq!(author, "Author A");
    }
}

fn seed_book_with_author(cache: &Cache, title: &str, author: &str) -> i32 {
    let conn = cache.backend.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO books (title, author_sort) VALUES (?1, ?2)",
        [title, author],
    )
    .unwrap();
    let book_id = conn.last_insert_rowid() as i32;
    conn.execute("INSERT OR IGNORE INTO authors (name) VALUES (?1)", [author])
        .unwrap();
    let author_id: i32 = conn
        .query_row("SELECT id FROM authors WHERE name = ?1", [author], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)",
        (book_id, author_id),
    )
    .unwrap();
    book_id
}

#[test]
fn copy_one_book_skips_a_same_author_near_same_title_duplicate_when_checking() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let src_cache = Arc::new(Mutex::new(Cache::new(src_dir.path()).unwrap()));
    let src_book_id = {
        let guard = src_cache.lock().unwrap();
        seed_book_with_author(&guard, "The Great Book", "Jane Doe")
    };

    let dest_cache = Arc::new(Mutex::new(Cache::new(dest_dir.path()).unwrap()));
    {
        let guard = dest_cache.lock().unwrap();
        // `fuzzy_title` (which `find_identical_books` uses) lowercases
        // and collapses whitespace, so this counts as a near-match,
        // not just a byte-identical title.
        seed_book_with_author(&guard, "the   GREAT book", "Jane Doe");
    }

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, true).unwrap();

    assert!(result.is_none(), "duplicate should be skipped, not copied");
    assert_eq!(dest_cache.lock().unwrap().all_book_ids().unwrap().len(), 1);
}

#[test]
fn copy_one_book_adds_when_no_duplicate_exists() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let src_cache = Arc::new(Mutex::new(Cache::new(src_dir.path()).unwrap()));
    let src_book_id = {
        let guard = src_cache.lock().unwrap();
        seed_book_with_author(&guard, "Unique Book", "Jane Doe")
    };

    let dest_cache = Arc::new(Mutex::new(Cache::new(dest_dir.path()).unwrap()));
    {
        let guard = dest_cache.lock().unwrap();
        seed_book_with_author(&guard, "A Completely Different Book", "John Smith");
    }

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, true).unwrap();

    assert!(result.is_some());
    assert_eq!(dest_cache.lock().unwrap().all_book_ids().unwrap().len(), 2);
}

#[test]
fn copy_one_book_ignores_duplicates_when_not_asked_to_check() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let src_cache = Arc::new(Mutex::new(Cache::new(src_dir.path()).unwrap()));
    let src_book_id = {
        let guard = src_cache.lock().unwrap();
        seed_book_with_author(&guard, "The Great Book", "Jane Doe")
    };

    let dest_cache = Arc::new(Mutex::new(Cache::new(dest_dir.path()).unwrap()));
    {
        let guard = dest_cache.lock().unwrap();
        seed_book_with_author(&guard, "The Great Book", "Jane Doe");
    }

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, false).unwrap();

    assert!(
        result.is_some(),
        "check_duplicates=false should always copy"
    );
    assert_eq!(dest_cache.lock().unwrap().all_book_ids().unwrap().len(), 2);
}
