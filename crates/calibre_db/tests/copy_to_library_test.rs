use calibre_db::backend::Backend;
use calibre_db::cache::Cache;
use calibre_db::copy_to_library::copy_one_book;
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
        drop(conn);
        Cache::new(src_dir.path()).unwrap()
    };

    // Setup Dest DB (empty).
    let dest_cache = Cache::new(dest_dir.path()).unwrap();

    // Perform Copy
    let new_id = copy_one_book(&src_cache, &dest_cache, 1, false)
        .expect("Copy failed")
        .unwrap();

    // Verify
    let title = dest_cache.field_for(new_id, "title").unwrap().unwrap();
    let author = dest_cache.field_for(new_id, "author_sort").unwrap().unwrap();

    assert_eq!(title, "Source Book");
    assert_eq!(author, "Author A");
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

    let src_cache = Cache::new(src_dir.path()).unwrap();
    let src_book_id = seed_book_with_author(&src_cache, "The Great Book", "Jane Doe");

    let dest_cache = Cache::new(dest_dir.path()).unwrap();
    // `fuzzy_title` (which `find_identical_books` uses) lowercases
    // and collapses whitespace, so this counts as a near-match,
    // not just a byte-identical title.
    seed_book_with_author(&dest_cache, "the   GREAT book", "Jane Doe");

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, true).unwrap();

    assert!(result.is_none(), "duplicate should be skipped, not copied");
    assert_eq!(dest_cache.all_book_ids().unwrap().len(), 1);
}

#[test]
fn copy_one_book_adds_when_no_duplicate_exists() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let src_cache = Cache::new(src_dir.path()).unwrap();
    let src_book_id = seed_book_with_author(&src_cache, "Unique Book", "Jane Doe");

    let dest_cache = Cache::new(dest_dir.path()).unwrap();
    seed_book_with_author(&dest_cache, "A Completely Different Book", "John Smith");

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, true).unwrap();

    assert!(result.is_some());
    assert_eq!(dest_cache.all_book_ids().unwrap().len(), 2);
}

#[test]
fn copy_one_book_ignores_duplicates_when_not_asked_to_check() {
    let src_dir = tempdir().unwrap();
    let dest_dir = tempdir().unwrap();

    let src_cache = Cache::new(src_dir.path()).unwrap();
    let src_book_id = seed_book_with_author(&src_cache, "The Great Book", "Jane Doe");

    let dest_cache = Cache::new(dest_dir.path()).unwrap();
    seed_book_with_author(&dest_cache, "The Great Book", "Jane Doe");

    let result = copy_one_book(&src_cache, &dest_cache, src_book_id, false).unwrap();

    assert!(
        result.is_some(),
        "check_duplicates=false should always copy"
    );
    assert_eq!(dest_cache.all_book_ids().unwrap().len(), 2);
}

#[test]
fn find_duplicate_books_matches_same_author_same_title() {
    use calibre_db::copy_to_library::find_duplicate_books;

    let dir = tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    let existing_id = seed_book_with_author(&cache, "The Great Book", "Jane Doe");
    seed_book_with_author(&cache, "A Different Book", "John Smith");

    let dups = find_duplicate_books(&cache, "The Great Book", &["Jane Doe".to_string()]).unwrap();
    assert_eq!(dups, [existing_id].into_iter().collect());
}

#[test]
fn find_duplicate_books_is_empty_for_a_genuinely_new_book() {
    use calibre_db::copy_to_library::find_duplicate_books;

    let dir = tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    seed_book_with_author(&cache, "The Great Book", "Jane Doe");

    let dups = find_duplicate_books(&cache, "A Brand New Book", &["Someone Else".to_string()]).unwrap();
    assert!(dups.is_empty());
}

#[test]
fn book_title_and_authors_reports_the_real_per_author_list() {
    use calibre_db::copy_to_library::book_title_and_authors;

    let dir = tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    let book_id = seed_book_with_author(&cache, "The Great Book", "Jane Doe");

    let (title, authors) = book_title_and_authors(&cache, book_id).unwrap();
    assert_eq!(title, "The Great Book");
    assert_eq!(authors, vec!["Jane Doe".to_string()]);
}
