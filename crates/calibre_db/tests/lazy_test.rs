use calibre_db::backend::Backend;
use calibre_db::lazy::ProxyMetadata;
use std::sync::{Arc, Mutex};
use tempfile::tempdir;

#[test]
fn test_proxy_metadata_basics() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();

    // Populate DB with a book (the real calibre schema already exists
    // via `Backend::new`).
    {
        let conn = backend.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO books (id, title, sort, author_sort, uuid) VALUES (1, 'The Rust Book', 'Rust Book, The', 'Klabnik, Steve', '123-uuid')",
            []
        ).unwrap();
    }

    let backend_ref = Arc::new(Mutex::new(backend));
    let mut proxy = ProxyMetadata::new(1, backend_ref);

    // Initial state, should fetch from DB
    let title = proxy.get_title();
    assert_eq!(title, "The Rust Book");

    // Second fetch should hit cache
    let title2 = proxy.get_title();
    assert_eq!(title2, "The Rust Book");

    // Check another field. The real schema's `books_insert_trg`
    // overwrites `uuid` with a fresh `uuid4()` on every insert
    // (matching upstream), so the inserted placeholder value never
    // survives -- just check a real UUID came back.
    let uuid = proxy.get_field("uuid");
    assert!(
        uuid.as_deref()
            .is_some_and(|u| uuid::Uuid::parse_str(u).is_ok()),
        "{uuid:?}"
    );

    // Check Missing field. `path` no longer works as a "definitely
    // unset" example against the real schema -- `books.path` is
    // `NOT NULL DEFAULT ''`, so it comes back as `Some("")`, not
    // `None`. Use a field name `Backend::field_for` doesn't recognize
    // at all instead, which is what actually exercises the "missing"
    // path.
    let missing = proxy.get_field("not_a_real_field");
    assert_eq!(missing, None);
}

#[test]
fn test_proxy_metadata_manual_cache() {
    let dir = tempdir().unwrap();
    let backend = Backend::new(dir.path()).unwrap();
    let backend_ref = Arc::new(Mutex::new(backend));

    let mut proxy = ProxyMetadata::new(2, backend_ref);

    // We can't set cache directly as it's private, but if we had a setter...
    // For now, let's just rely on the getter behavior.
    assert_eq!(proxy.get_field("random_field"), None);
}
