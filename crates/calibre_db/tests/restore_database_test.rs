use calibre_db::library::Library;
use calibre_db::restore;
use calibre_ebooks::metadata::MetaInformation;
use std::fs;
use tempfile::tempdir;

fn write_opf(dir: &std::path::Path, meta: &MetaInformation) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("metadata.opf"), meta.to_xml()).unwrap();
}

#[test]
fn test_restore_database_flow() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();

    {
        let _library = Library::create(library_path.clone()).expect("Failed to create library");
        let book_dir = library_path.join("Test Author").join("Test Book");
        let mut meta = MetaInformation::default();
        meta.title = "Test Book".to_string();
        meta.authors = vec!["Test Author".to_string()];
        meta.uuid = Some("test-uuid-123".to_string());
        write_opf(&book_dir, &meta);
    }

    let db_path = library_path.join("metadata.db");
    if db_path.exists() {
        fs::remove_file(&db_path).unwrap();
    }

    let report = restore::restore_database(&library_path, |_msg| {}).expect("Restore failed");
    assert_eq!(report.restored, 1);
    assert!(report.failed.is_empty());

    let library = Library::open(library_path.clone()).expect("Failed to open restored library");
    let books = library.list_books().expect("Failed to list books");

    assert_eq!(books.len(), 1);
    let book = &books[0];
    assert_eq!(book.title, "Test Book");
    assert_eq!(book.author_sort.as_deref(), Some("Test Author"));
    assert_eq!(book.uuid.as_deref(), Some("test-uuid-123"));
}

#[test]
fn restore_preserves_the_original_book_id_from_the_opfs_calibre_identifier() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    {
        let _library = Library::create(library_path.clone()).unwrap();
        let book_dir = library_path.join("Author").join("Book");
        let mut meta = MetaInformation::default();
        meta.title = "Book".to_string();
        meta.set_identifier("calibre", "42");
        write_opf(&book_dir, &meta);
    }
    fs::remove_file(library_path.join("metadata.db")).unwrap();

    restore::restore_database(&library_path, |_| {}).unwrap();

    let library = Library::open(library_path.clone()).unwrap();
    let books = library.list_books().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].id, 42);
}

#[test]
fn restore_database_removes_a_stale_pre_restore_backup_through_the_real_library_handle() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();

    {
        let _library = Library::create(library_path.clone()).expect("Failed to create library");
        let book_dir = library_path.join("Test Author").join("Test Book");
        let mut meta = MetaInformation::default();
        meta.title = "Test Book".to_string();
        meta.authors = vec!["Test Author".to_string()];
        write_opf(&book_dir, &meta);
    }

    // Leave `metadata.db` in place (unlike the other tests above) so
    // `restore_database`'s backup branch runs, and pre-seed a stale
    // backup from some earlier restore attempt so the removal branch
    // actually executes.
    let backup_path = library_path.join("metadata_pre_restore.db");
    fs::write(&backup_path, b"stale backup").unwrap();

    let report = restore::restore_database(&library_path, |_| {}).expect("Restore failed");
    assert_eq!(report.restored, 1);

    // The stale backup was removed and replaced by the just-backed-up
    // real `metadata.db` -- not left as the old stale content.
    assert_ne!(fs::read(&backup_path).unwrap(), b"stale backup");

    // restore_database now removes the stale backup and renames
    // metadata.db to the backup path through the real LibraryHandle
    // (issue #93's crate-wide write-path retrofit), not raw
    // `fs::remove_file`/`fs::rename` -- prove it by checking real
    // journal entries landed for both (tag-substring match, not real
    // deserialization: `OperationDescriptor`/`JournalEntry` are
    // private to `library_handle.rs`).
    let journal_dir = library_path.join(".calibre-oxide").join("journal");
    let journaled_op_count = |tag: &str| {
        fs::read_dir(&journal_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("op"))
            .filter(|e| {
                fs::read_to_string(e.path())
                    .map(|content| content.contains(tag))
                    .unwrap_or(false)
            })
            .count()
    };
    assert_eq!(
        journaled_op_count("DeleteFile"),
        1,
        "expected a real journaled delete of the stale backup"
    );
    assert_eq!(
        journaled_op_count("RenameFile"),
        1,
        "expected a real journaled rename of metadata.db to the backup path"
    );
}

#[test]
fn restore_database_fails_fast_if_another_writer_already_holds_the_library_lock() {
    // Port of issue #93's crate-wide write-path retrofit: restore_database
    // now acquires LibraryHandle's real exclusive writer lock (§7) up
    // front and holds it for the whole rebuild, not just around the
    // metadata.db backup rename -- the point being that the long book-
    // rescanning loop is protected too, not just the one rename step.
    // Proving that directly would need a race (start a real restore in
    // a background thread, try to grab a second handle mid-run), which
    // is exactly the kind of timing-dependent test this project avoids.
    // This proves the equivalent, deterministic half instead: if
    // something else already holds the lock *before* restore_database
    // even starts, it fails immediately rather than proceeding --
    // which only happens if it's really acquiring the same real lock
    // up front, not lazily deep inside the loop.
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    {
        let _library = Library::create(library_path.clone()).expect("Failed to create library");
    }

    let _handle = calibre_db::library_handle::LibraryHandle::open(&library_path).unwrap();

    let err = restore::restore_database(&library_path, |_| {}).unwrap_err();
    assert!(
        err.chain().any(|e| e.to_string().contains("writer lock")),
        "expected an AlreadyLocked-flavored error somewhere in the chain, got: {err:#}"
    );
}

#[test]
fn restore_relinks_tags_series_publisher_rating_and_identifiers() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    {
        let _library = Library::create(library_path.clone()).unwrap();
        let book_dir = library_path.join("Author").join("Book");
        let mut meta = MetaInformation::default();
        meta.title = "Book".to_string();
        meta.authors = vec!["Jane Doe".to_string()];
        meta.tags = vec!["fiction".to_string(), "adventure".to_string()];
        meta.series = Some("The Series".to_string());
        meta.series_index = 2.0;
        meta.publisher = Some("Acme Books".to_string());
        meta.rating = Some(8.0);
        meta.comments = Some("Great read".to_string());
        meta.set_identifier("isbn", "1234567890");
        write_opf(&book_dir, &meta);
    }
    fs::remove_file(library_path.join("metadata.db")).unwrap();

    restore::restore_database(&library_path, |_| {}).unwrap();

    let cache = calibre_db::cache::Cache::new(&library_path).unwrap();
    let ids = cache.all_book_ids().unwrap();
    assert_eq!(ids.len(), 1);
    let id = ids[0];

    let tags = cache.field_for(id, "tags").unwrap().unwrap();
    let mut tags: Vec<&str> = tags.split(", ").collect();
    tags.sort_unstable();
    assert_eq!(tags, vec!["adventure", "fiction"]);
    assert_eq!(
        cache.field_for(id, "series").unwrap(),
        Some("The Series".to_string())
    );
    assert_eq!(
        cache.field_for(id, "publisher").unwrap(),
        Some("Acme Books".to_string())
    );
    assert_eq!(
        cache.field_for(id, "rating").unwrap(),
        Some("8".to_string())
    );
    assert_eq!(
        cache.field_for(id, "comments").unwrap(),
        Some("Great read".to_string())
    );
    assert_eq!(
        cache.field_for(id, "identifiers").unwrap(),
        Some("isbn:1234567890".to_string())
    );
    assert_eq!(
        cache.field_for(id, "authors").unwrap(),
        Some("Jane Doe".to_string())
    );
}

#[test]
fn restore_rediscovers_format_files_and_cover_in_the_book_directory() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    {
        let _library = Library::create(library_path.clone()).unwrap();
        let book_dir = library_path.join("Author").join("Book");
        let mut meta = MetaInformation::default();
        meta.title = "Book".to_string();
        write_opf(&book_dir, &meta);
        fs::write(book_dir.join("book.epub"), b"epub content").unwrap();
        fs::write(book_dir.join("book.mobi"), b"mobi content longer").unwrap();
        fs::write(book_dir.join("cover.jpg"), b"jpeg bytes").unwrap();
    }
    fs::remove_file(library_path.join("metadata.db")).unwrap();

    restore::restore_database(&library_path, |_| {}).unwrap();

    let cache = calibre_db::cache::Cache::new(&library_path).unwrap();
    let id = cache.all_book_ids().unwrap()[0];
    let formats = cache.field_for(id, "formats").unwrap().unwrap();
    let mut formats: Vec<&str> = formats.split(", ").collect();
    formats.sort_unstable();
    assert_eq!(formats, vec!["EPUB", "MOBI"]);
    assert!(cache.has_cover(id).unwrap());
}

#[test]
fn restore_skips_a_directory_with_no_readable_opf_but_still_restores_the_rest() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    {
        let _library = Library::create(library_path.clone()).unwrap();

        let good_dir = library_path.join("Author").join("Good Book");
        let mut meta = MetaInformation::default();
        meta.title = "Good Book".to_string();
        write_opf(&good_dir, &meta);

        let bad_dir = library_path.join("Author").join("Bad Book");
        fs::create_dir_all(&bad_dir).unwrap();
        fs::write(bad_dir.join("metadata.opf"), b"not valid opf xml <<<").unwrap();
    }
    fs::remove_file(library_path.join("metadata.db")).unwrap();

    let report = restore::restore_database(&library_path, |_| {}).unwrap();

    assert_eq!(report.restored, 1);
    assert_eq!(report.failed.len(), 1);

    let library = Library::open(library_path.clone()).unwrap();
    let books = library.list_books().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].title, "Good Book");
}

#[test]
fn restore_from_opf_relinks_authors_and_tags_for_an_existing_book() {
    let dir = tempdir().unwrap();
    let library_path = dir.path().to_path_buf();
    let cache = std::sync::Arc::new(std::sync::Mutex::new(
        calibre_db::cache::Cache::new(&library_path).unwrap(),
    ));

    let source = library_path.join("src.epub");
    fs::write(&source, b"x").unwrap();
    let mut meta = MetaInformation::default();
    meta.title = "Original".to_string();
    meta.authors = vec!["Old Author".to_string()];
    let book_id = {
        let guard = cache.lock().unwrap();
        guard.add_book(&source, &meta).unwrap()
    };

    let book_dir = {
        let guard = cache.lock().unwrap();
        let rel = guard.field_for(book_id, "path").unwrap().unwrap();
        library_path.join(rel)
    };
    let mut updated = MetaInformation::default();
    updated.title = "Updated Title".to_string();
    updated.authors = vec!["New Author".to_string()];
    updated.tags = vec!["nonfiction".to_string()];
    write_opf(&book_dir, &updated);

    restore::restore_from_opf(&cache, book_id).unwrap();

    let guard = cache.lock().unwrap();
    assert_eq!(
        guard.field_for(book_id, "title").unwrap(),
        Some("Updated Title".to_string())
    );
    assert_eq!(
        guard.field_for(book_id, "authors").unwrap(),
        Some("New Author".to_string())
    );
    assert_eq!(
        guard.field_for(book_id, "tags").unwrap(),
        Some("nonfiction".to_string())
    );
}
