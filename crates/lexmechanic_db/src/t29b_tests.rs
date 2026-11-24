//! Tranche 29b: the engine write path, the field store and its caches.
//!
//! Besides the regression tests, this file holds `#[ignore]`d timing probes
//! (`cargo test -p lexmechanic_db --release t29b_probe -- --ignored --nocapture`)
//! that build a synthetic library of `T29B_N` books (default 30,000) straight
//! into SQLite and print how long the paths this tranche touches take.

use std::time::Instant;

use crate::cache::Cache;

const WORDS: [&str; 24] = [
    "history", "quantum", "machine", "garden", "river", "silent", "empire", "winter", "theory", "poetry", "letters", "atlas", "voyage", "memory", "shadow", "numbers", "forest", "signal", "orchard",
    "ledger", "harbor", "compass", "eclipse", "meadow",
];

fn lcg(state: &mut u64) -> u64 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *state >> 33
}

/// A library of `n` books written straight into SQLite: authors, tags,
/// series, publishers, comments and a PDF row per book.
pub(crate) fn big_library(n: usize) -> (tempfile::TempDir, Cache) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path()).unwrap();
    {
        let mut conn = cache.backend.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        let mut seed = 7u64;
        let authors = (n / 6).max(8);
        let series = (n / 25).max(4);
        for a in 0..authors {
            tx.execute("INSERT INTO authors (name) VALUES (?1)", [format!("Author{a} Surname{}", a % 997)]).unwrap();
        }
        for t in 0..500 {
            tx.execute("INSERT INTO tags (name) VALUES (?1)", [format!("tag {t}")]).unwrap();
        }
        for s in 0..series {
            tx.execute("INSERT INTO series (name) VALUES (?1)", [format!("Series {s}")]).unwrap();
        }
        for p in 0..1000 {
            tx.execute("INSERT INTO publishers (name) VALUES (?1)", [format!("Publisher {p}")]).unwrap();
        }
        for i in 0..n {
            let title = format!("{} {} {} {}", WORDS[lcg(&mut seed) as usize % 24], WORDS[lcg(&mut seed) as usize % 24], WORDS[lcg(&mut seed) as usize % 24], i);
            tx.execute("INSERT INTO books (title, author_sort, path) VALUES (?1, ?2, ?3)", (&title, format!("Surname{}, Author{}", i % 997, i % authors), format!("Folder{}/{}", i % 500, i))).unwrap();
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO books_authors_link (book, author) VALUES (?1, ?2)", (id, (i % authors) as i64 + 1)).unwrap();
            for k in 0..3 {
                let tag = (lcg(&mut seed) % 500) as i64 + 1;
                let _ = tx.execute("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES (?1, ?2)", (id, tag + k * 0));
            }
            if i % 3 == 0 {
                tx.execute("INSERT INTO books_series_link (book, series) VALUES (?1, ?2)", (id, (i % series) as i64 + 1)).unwrap();
            }
            tx.execute("INSERT INTO books_publishers_link (book, publisher) VALUES (?1, ?2)", (id, (lcg(&mut seed) % 1000) as i64 + 1)).unwrap();
            tx.execute("INSERT INTO comments (book, text) VALUES (?1, ?2)", (id, format!("<p>About {title}. {} {} {} {} {} {}</p>", WORDS[(i + 1) % 24], WORDS[(i + 5) % 24], WORDS[(i + 7) % 24], WORDS[(i + 11) % 24], WORDS[(i + 13) % 24], WORDS[(i + 17) % 24]))).unwrap();
            tx.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'PDF', ?2, ?3)", (id, 10_000 + (i as i64 % 5000), format!("file{i}"))).unwrap();
        }
        tx.commit().unwrap();
    }
    (dir, cache)
}

fn t<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let s = Instant::now();
    let r = f();
    println!("{label:<66} {:>9.1} ms", s.elapsed().as_secs_f64() * 1000.0);
    r
}

fn probe_size() -> usize {
    std::env::var("T29B_N").ok().and_then(|v| v.parse().ok()).unwrap_or(30_000)
}

#[test]
#[ignore]
fn t29b_probe_field_store_and_writes() {
    let n = probe_size();
    let (_dir, cache) = t(&format!("build a {n}-book library"), || big_library(n));
    let ids = cache.all_book_ids().unwrap();
    println!("books: {}", ids.len());
    {
        // Where the cold load goes, table by table (main connection, for the profile only).
        use crate::tables::*;
        let conn = cache.backend.conn.lock().unwrap();
        t("  profile: read_book_columns (books, one scan)", || read_book_columns(&conn).unwrap());
        t("  profile: comments", || OneToOneTable::read(&conn, "comments", "book", "text").unwrap());
        t("  profile: size", || SizeTable::read(&conn).unwrap());
        t("  profile: series", || ManyToOneTable::read(&conn, "series", "name", "books_series_link", "series").unwrap());
        t("  profile: publisher", || ManyToOneTable::read(&conn, "publishers", "name", "books_publishers_link", "publisher").unwrap());
        t("  profile: rating", || read_rating_table(&conn).unwrap());
        t("  profile: tags", || ManyToManyTable::read(&conn, "tags", "name", "books_tags_link", "tag", "id").unwrap());
        t("  profile: languages", || ManyToManyTable::read(&conn, "languages", "lang_code", "books_languages_link", "lang_code", "item_order").unwrap());
        t("  profile: authors", || AuthorsTable::read(&conn).unwrap());
        t("  profile: formats", || FormatsTable::read(&conn).unwrap());
        t("  profile: identifiers", || IdentifiersTable::read(&conn).unwrap());
        t("  profile: whole StandardTables::read", || StandardTables::read(&conn).unwrap());
    }
    let store = t("fields() cold (full load)", || cache.fields().unwrap());
    t("fields() warm", || cache.fields().unwrap());
    t("FieldStore deep clone", || (*store).clone());
    // A write while a reader holds the snapshot (what a request in flight does).
    for i in 0..3 {
        t("set_field rating, snapshot held, then fields()", || {
            cache.set_field(ids[10 + i], "rating", "4").unwrap();
            cache.fields().unwrap()
        });
    }
    drop(store);
    for i in 0..3 {
        t("set_field rating, no snapshot held, then fields()", || {
            cache.set_field(ids[20 + i], "rating", "6").unwrap();
            drop(cache.fields().unwrap());
        });
    }
    for i in 0..3 {
        let held = cache.fields().unwrap();
        t("set_field title, snapshot held, then fields()", || {
            cache.set_field(ids[30 + i], "title", &format!("Retitled {i}")).unwrap();
            cache.fields().unwrap()
        });
        drop(held);
    }
    // Per-write cost, no reader: set_field only.
    t("20 x set_field(rating) (the per-write tax)", || {
        for i in 0..20 {
            cache.set_field(ids[100 + i], "rating", "8").unwrap();
        }
    });
    // Where the per-write cost goes: the SQLite commit, or the change log.
    t("20 x raw `UPDATE books` autocommit (WAL commit, synchronous=FULL)", || {
        let conn = cache.backend.conn.lock().unwrap();
        for i in 0..20 {
            conn.execute("UPDATE books SET series_index = series_index + 0 WHERE id = ?1", [ids[200 + i]]).unwrap();
        }
    });
    t("20 x change_log.append(FieldSet) (file + fsync + dir)", || {
        let log = cache.backend.change_log().unwrap();
        for i in 0..20 {
            log.append(crate::change_log::ChangeOp::FieldSet { book: format!("probe-{i}"), field: "rating".into(), value: Some("8".into()) }).unwrap();
        }
    });
    t("1 x change_log.append(Batch of 20 FieldSet)", || {
        let log = cache.backend.change_log().unwrap();
        let ops = (0..20).map(|i| crate::change_log::ChangeOp::FieldSet { book: format!("probe-{i}"), field: "rating".into(), value: Some("8".into()) }).collect();
        log.append(crate::change_log::ChangeOp::Batch { ops }).unwrap();
    });
    // A 200-book uniform batch, and an undo-shaped batch in 110 groups.
    let batch: Vec<i32> = ids.iter().copied().skip(1000).take(200).collect();
    t("set_fields_for_books: 200 books, one group", || cache.set_fields_for_books(&batch, &[("publisher".to_string(), "Bulk House".to_string())]));
    t("set_fields_for_books x110 groups (what the batch endpoint does today)", || {
        for chunk in batch.chunks(2) {
            cache.set_fields_for_books(chunk, &[("publisher".to_string(), format!("Undo House {}", chunk[0]))]);
        }
    });
    let items: Vec<(i32, Vec<(String, String)>)> = batch.iter().map(|id| (*id, vec![("publisher".to_string(), format!("Undo House {}", id / 2))])).collect();
    t("set_fields_per_book: the same 200 books, one transaction", || cache.set_fields_per_book(&items));
    // The external-write reload.
    t("fields() after a write by another connection (full reload)", || {
        let other = Cache::new(_dir.path()).unwrap();
        other.set_field(ids[3], "rating", "6").unwrap();
        drop(other);
        cache.fields().unwrap()
    });
}

// ---------------------------------------------------------------------------
// Regression tests
// ---------------------------------------------------------------------------

use crate::checksums::HashRecord;
use crate::pdf_similarity::{PageHash, PdfFingerprint, PdfStatus};

fn fingerprint(pages: u32) -> PdfFingerprint {
    PdfFingerprint { page_count: pages, pages: vec![PageHash { index: 0, bits: [1, 2, 3, 4], mask: [u64::MAX; 4], blank: false }], text_chars: 100, has_text: true, scanned: false, minhash: vec![1, 2, 3] }
}

/// What `pdf_status` was before it became one query: four counts, three of them joins.
fn pdf_status_four_queries(cache: &Cache) -> PdfStatus {
    let conn = cache.backend.conn.lock().unwrap();
    let version = crate::pdf_similarity::FINGERPRINT_VERSION;
    let count = |sql: &str| -> usize { conn.query_row(sql, [version], |r| r.get::<_, i64>(0)).unwrap() as usize };
    let hashed_join = "FROM data d JOIN checksums_db.file_checksums c ON c.book_id = d.book AND c.kind = 'format' AND c.key = d.format";
    let fp_join = "JOIN checksums_db.pdf_fingerprints p ON p.blake3_hex = c.blake3_hex AND p.version = ?1";
    let fingerprinted = count(&format!("SELECT count(*) {hashed_join} {fp_join} WHERE UPPER(d.format) = 'PDF' AND p.status = 'ok'"));
    let failed = count(&format!("SELECT count(*) {hashed_join} {fp_join} WHERE UPPER(d.format) = 'PDF' AND p.status = 'failed'"));
    let hashed = count(&format!("SELECT count(*) {hashed_join} WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1"));
    let all = count("SELECT count(*) FROM data d WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1");
    PdfStatus { fingerprinted, failed, pending: hashed.saturating_sub(fingerprinted + failed), unhashed: all.saturating_sub(hashed) }
}

/// A library of 40 books with a PDF each (and an EPUB for every fifth), in every state the status counts: unhashed, hashed
/// and owed a fingerprint, fingerprinted, failed, two books sharing a hash, and a fingerprint of an older version.
fn library_in_every_pdf_state() -> (tempfile::TempDir, Cache) {
    let (dir, cache) = big_library(40);
    let ids = cache.all_book_ids().unwrap();
    {
        let conn = cache.backend.conn.lock().unwrap();
        for id in ids.iter().step_by(5) {
            conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 10, 'e')", [id]).unwrap();
        }
        // One format stored in lower case, which `UPPER(format) = 'PDF'` counted and `= 'PDF'` must too.
        conn.execute("UPDATE data SET format = 'pdf' WHERE book = ?1 AND format = 'PDF'", [ids[1]]).unwrap();
    }
    let store = cache.checksums();
    let mut records = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if i % 7 == 6 {
            continue; // never hashed
        }
        let hash = if i % 11 == 10 { "shared-hash".to_string() } else { format!("hash-{i}") };
        let key = if i == 1 { "pdf" } else { "PDF" };
        records.push(HashRecord { book_id: *id, kind: "format".into(), key: key.into(), hash, size: 1000 + i as i64, identity: None });
        if i % 5 == 0 {
            records.push(HashRecord { book_id: *id, kind: "format".into(), key: "EPUB".into(), hash: format!("epub-{i}"), size: 10, identity: None });
        }
    }
    store.record_many(&records).unwrap();
    for (i, r) in records.iter().filter(|r| r.key.eq_ignore_ascii_case("pdf")).enumerate() {
        match i % 4 {
            0 => store.record_pdf_fingerprint(&r.hash, &Ok(fingerprint(3))).unwrap(),
            1 => store.record_pdf_fingerprint(&r.hash, &Err("broken".to_string())).unwrap(),
            _ => {}
        }
    }
    // A row of an older fingerprint version counts as owed, not done.
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE checksums_db.pdf_fingerprints SET version = version - 1 WHERE blake3_hex = 'hash-3'", []).unwrap();
    }
    (dir, cache)
}

#[test]
fn pdf_status_in_one_query_counts_exactly_what_the_four_queries_did() {
    let (_dir, cache) = library_in_every_pdf_state();
    let want = pdf_status_four_queries(&cache);
    assert!(want.fingerprinted > 0 && want.failed > 0 && want.pending > 0 && want.unhashed > 0, "the fixture covers every state: {want:?}");
    assert_eq!(cache.checksums().pdf_status().unwrap(), want);
    // On the main connection too (no reader), as on a network library.
    let no_reader = crate::checksums::ChecksumStore::new(cache.backend.conn.clone(), &cache.backend.library_path);
    assert_eq!(no_reader.pdf_status().unwrap(), want);
}

#[test]
fn the_checksum_reads_do_not_wait_for_the_main_connection() {
    let (_dir, cache) = library_in_every_pdf_state();
    let store = cache.checksums();
    // Warm up: the reader attaches the sidecar on its first use, which does need the main connection.
    store.pdf_status().unwrap();
    // Hold the main connection, as a long write or a scan on it would.
    let held = cache.backend.conn.lock().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reading = std::thread::spawn(move || {
        let status = store.pdf_status().unwrap();
        let sharing = store.books_sharing_content("format").unwrap();
        let prints = store.pdf_fingerprints().unwrap();
        let _ = store.pdf_fingerprints_pending().unwrap();
        let _ = store.all_format_records().unwrap();
        let _ = store.empty_files("format").unwrap();
        tx.send((status, sharing.len(), prints.len())).unwrap();
    });
    let got = rx.recv_timeout(std::time::Duration::from_secs(20));
    drop(held);
    reading.join().unwrap();
    let (status, sharing, prints) = got.expect("the reads finished while the main connection was held");
    assert!(status.fingerprinted > 0);
    assert_eq!(sharing, 1, "the two books sharing a hash");
    assert!(prints > 0);
}

#[test]
fn a_read_through_the_reader_sees_what_the_main_connection_committed() {
    let (_dir, cache) = big_library(5);
    let id = cache.all_book_ids().unwrap()[0];
    let store = cache.checksums();
    store.record_hash(id, "format", "PDF", "abc", 5).unwrap();
    assert_eq!(store.pdf_status().unwrap().pending, 1);
    store.record_pdf_fingerprint("abc", &Ok(fingerprint(2))).unwrap();
    assert_eq!(store.pdf_status().unwrap().fingerprinted, 1);
    cache.set_field(id, "title", "Renamed").unwrap();
    assert_eq!(cache.field_for(id, "title").unwrap().as_deref(), Some("Renamed"));
}

#[test]
fn both_connections_wait_five_seconds_for_a_lock() {
    let (_dir, cache) = big_library(3);
    let timeout = |conn: &rusqlite::Connection| -> i64 { conn.query_row("PRAGMA busy_timeout", [], |r| r.get(0)).unwrap() };
    assert_eq!(timeout(&cache.backend.conn.lock().unwrap()), 5000);
    let reader = cache.backend.reader().expect("a local library has a reader");
    assert_eq!(timeout(&reader.lock().unwrap()), 5000);
    // And the reader cannot write.
    assert!(reader.lock().unwrap().execute("DELETE FROM books", []).is_err());
}

#[test]
fn a_batch_of_fingerprints_is_marked_then_recorded_and_unmarked_together() {
    let (_dir, cache) = library_in_every_pdf_state();
    let store = cache.checksums();
    store.mark_inflight_many("fingerprint", &["h1", "h2", "h3"]).unwrap();
    assert_eq!(store.take_inflight("fingerprint").unwrap(), vec!["h1", "h2", "h3"], "marked before any file is read");
    store.mark_inflight_many("fingerprint", &["h1", "h2", "h3"]).unwrap();
    store.finish_pdf_fingerprints("fingerprint", &[("h1", Ok(fingerprint(1))), ("h2", Err("nope".to_string()))], &["h1", "h2", "h3"]).unwrap();
    assert!(store.has_pdf_fingerprint("h1").unwrap());
    assert_eq!(store.pdf_fingerprint_error("h2").unwrap().as_deref(), Some("nope"));
    assert!(!store.has_pdf_fingerprint("h3").unwrap(), "a file not read is not recorded");
    assert!(store.take_inflight("fingerprint").unwrap().is_empty(), "every mark of the batch is gone");
}

#[test]
fn a_write_that_meets_another_connections_lock_waits_for_it() {
    assert!(crate::checksums::is_busy_error(&rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY), None)));
    assert!(crate::checksums::is_busy_error(&rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED), None)));
    assert!(!crate::checksums::is_busy_error(&rusqlite::Error::QueryReturnedNoRows));
    // A second connection holds the write lock for a while, then lets go.
    let (_dir, cache) = big_library(3);
    let id = cache.all_book_ids().unwrap()[0];
    let store = cache.checksums();
    store.initialize().unwrap();
    let db_path = cache.backend.db_path.clone();
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || {
        let other = rusqlite::Connection::open(db_path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE").unwrap();
        locked_tx.send(()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        other.execute_batch("COMMIT").unwrap();
    });
    locked_rx.recv().unwrap();
    let started = Instant::now();
    store.mark_inflight_many("fingerprint", &["k"]).unwrap();
    store.finish_pdf_fingerprints("fingerprint", &[("k", Ok(fingerprint(1)))], &["k"]).unwrap();
    store.record_many(&[HashRecord { book_id: id, kind: "format".into(), key: "PDF".into(), hash: "x".into(), size: 1, identity: None }]).unwrap();
    assert!(started.elapsed() >= std::time::Duration::from_millis(300), "the writes waited for the lock");
    holder.join().unwrap();
    assert!(store.has_pdf_fingerprint("k").unwrap());
}

#[test]
fn the_books_columns_read_in_one_scan_are_the_ones_read_one_scan_each() {
    let (_dir, cache) = big_library(60);
    {
        // Some NULLs, as a book from a foreign tool can have.
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET isbn = NULL, pubdate = NULL, author_sort = NULL WHERE id % 7 = 0", []).unwrap();
    }
    let conn = cache.backend.conn.lock().unwrap();
    let together = crate::tables::read_book_columns(&conn).unwrap();
    for (column, got) in ["title", "sort", "author_sort", "isbn", "path", "timestamp", "pubdate", "last_modified", "series_index", "uuid"].iter().zip(together.iter()) {
        let want = crate::tables::OneToOneTable::read(&conn, "books", "id", column).unwrap();
        assert_eq!(got, &want, "{column}");
    }
}

const ALL_FIELDS: [&str; 20] = [
    "title", "sort", "author_sort", "isbn", "path", "timestamp", "pubdate", "last_modified", "series_index", "comments", "uuid", "size", "series", "publisher", "rating", "tags", "languages",
    "authors", "formats", "identifiers",
];

fn assert_same_store(a: &crate::fields::FieldStore, b: &crate::fields::FieldStore, ids: &[i32], context: &str) {
    for &id in ids {
        for f in ALL_FIELDS {
            assert_eq!(a.field_for(id, f), b.field_for(id, f), "{context}: book {id} field {f}");
        }
    }
    assert_eq!(a.tables().title, b.tables().title, "{context}");
    assert_eq!(a.tables().formats, b.tables().formats, "{context}");
}

#[test]
fn the_snapshot_loaded_on_the_reader_is_the_one_the_main_connection_loads() {
    let (_dir, cache) = big_library(80);
    let ids = cache.all_book_ids().unwrap();
    cache.set_field(ids[3], "tags", "a, b").unwrap();
    cache.set_field(ids[4], "series", "Saga").unwrap();
    let via_reader = cache.fields().unwrap();
    assert_eq!(cache.field_cache_full_loads(), 1);
    let direct = crate::fields::FieldStore::load(&cache.backend.conn.lock().unwrap()).unwrap();
    assert_same_store(&via_reader, &direct, &ids, "reader load");
}

/// Edits of every kind, with a reader holding the snapshot throughout, leave
/// the snapshot equal to a fresh load, and copy only the tables they touch.
#[test]
fn a_patched_snapshot_shares_every_table_the_edit_did_not_touch() {
    let (_dir, cache) = big_library(120);
    let ids = cache.all_book_ids().unwrap();
    let before = cache.fields().unwrap();
    cache.set_field(ids[5], "rating", "6").unwrap();
    let after = cache.fields().unwrap();
    assert_eq!(cache.field_cache_full_loads(), 1, "patched, not reloaded");
    assert!(!std::sync::Arc::ptr_eq(&before, &after));
    let (b, a) = (before.tables(), after.tables());
    assert!(!std::sync::Arc::ptr_eq(&b.rating, &a.rating), "the rating table was copied");
    assert!(std::sync::Arc::ptr_eq(&b.title, &a.title), "the title table was shared, not copied");
    assert!(std::sync::Arc::ptr_eq(&b.comments, &a.comments));
    assert!(std::sync::Arc::ptr_eq(&b.tags, &a.tags));
    assert!(std::sync::Arc::ptr_eq(&b.authors, &a.authors));
    assert!(std::sync::Arc::ptr_eq(&b.formats, &a.formats));
    // The reader's old snapshot did not change under it.
    assert_eq!(before.field_for(ids[5], "rating"), None);
    assert_eq!(after.field_for(ids[5], "rating").as_deref(), Some("6"));
    // A write that changes nothing the store holds does not even move its version.
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET flags = 1 WHERE id = ?1", [ids[6]]).unwrap();
        conn.execute("UPDATE books SET has_cover = 1 WHERE id = ?1", [ids[6]]).unwrap();
    }
    let same = cache.fields().unwrap();
    assert_eq!(same.version(), after.version(), "a write to columns the store does not read leaves its version alone");

    // Every kind of edit, a snapshot held across each, then compared with a fresh load.
    let mut held = vec![cache.fields().unwrap()];
    cache.set_field(ids[1], "title", "Zebra Crossing").unwrap();
    held.push(cache.fields().unwrap());
    cache.set_field(ids[2], "tags", "red, blue").unwrap();
    held.push(cache.fields().unwrap());
    cache.set_field(ids[3], "authors", "Ann Author & Bob Writer").unwrap();
    held.push(cache.fields().unwrap());
    cache.set_field(ids[4], "series", "A New Series").unwrap();
    held.push(cache.fields().unwrap());
    cache.set_field(ids[7], "publisher", "Fresh House").unwrap();
    cache.set_field(ids[8], "comments", "<p>hello</p>").unwrap();
    cache.set_field(ids[9], "identifiers", "isbn:123,doi:456").unwrap();
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (?1, 'EPUB', 900, 'z')", [ids[10]]).unwrap();
        conn.execute("DELETE FROM data WHERE book = ?1", [ids[11]]).unwrap();
        conn.execute("DELETE FROM books_tags_link WHERE book = ?1", [ids[12]]).unwrap();
        conn.execute("UPDATE tags SET name = 'renamed tag' WHERE id = 3", []).unwrap();
    }
    let patched = cache.fields().unwrap();
    assert_eq!(cache.field_cache_full_loads(), 1, "all of it patched");
    let fresh = crate::fields::FieldStore::load(&cache.backend.conn.lock().unwrap()).unwrap();
    assert_same_store(&patched, &fresh, &ids, "after every kind of edit");
    // The snapshots held along the way kept what they had.
    assert_eq!(held[0].field_for(ids[1], "title"), before.field_for(ids[1], "title"));
    assert_eq!(held[1].field_for(ids[1], "title").as_deref(), Some("Zebra Crossing"));
    assert_eq!(held[1].field_for(ids[2], "tags"), before.field_for(ids[2], "tags"));
}

#[test]
fn a_sort_fields_version_moves_only_with_the_tables_it_reads() {
    let (_dir, cache) = big_library(30);
    let ids = cache.all_book_ids().unwrap();
    let v = |fields: &[&str]| cache.fields().unwrap().fields_version(fields);
    let (title, authors, rating, series) = (v(&["title"]), v(&["authors"]), v(&["rating"]), v(&["series"]));
    cache.set_field(ids[1], "rating", "4").unwrap();
    assert_eq!(v(&["title"]), title, "a rating is not a title");
    assert_eq!(v(&["authors"]), authors);
    assert_ne!(v(&["rating"]), rating);
    cache.set_field(ids[1], "comments", "<p>x</p>").unwrap();
    assert_eq!(v(&["title"]), title);
    cache.set_field(ids[2], "title", "A Different Title").unwrap();
    assert_ne!(v(&["title"]), title);
    assert_eq!(v(&["series"]), series, "retitling a book does not change which books there are");
    // A book appearing, or going, changes every sort.
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES ('A New Arrival')", []).unwrap();
    }
    assert_ne!(v(&["series"]), series, "a new book is in every sort");
    let with_new = v(&["authors"]);
    {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("DELETE FROM books WHERE title = 'A New Arrival'", []).unwrap();
    }
    assert_ne!(v(&["authors"]), with_new, "and a book that is gone is out of every sort");
    // A field it knows nothing about falls back to the whole store.
    let store = cache.fields().unwrap();
    assert_eq!(store.fields_version(&["#custom"]), store.version());
}

#[test]
fn writers_that_arrive_together_share_one_change_log_entry_and_each_is_durable() {
    let (_dir, cache) = big_library(3);
    let log = cache.backend.change_log().unwrap();
    let before = log.entry_count();
    let threads = 12;
    let per_thread = 6;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let (log, barrier) = (log.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                for i in 0..per_thread {
                    log.append_grouped(crate::change_log::ChangeOp::FieldSet { book: format!("t{t}"), field: "rating".into(), value: Some(i.to_string()) }).unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let written = log.entry_count() - before;
    println!("{} ops from {threads} threads went into {written} entries", threads * per_thread);
    assert!(written >= 1 && written <= threads * per_thread);
    // Every op is on disk, once, and each thread's own come in the order it wrote them.
    let replay = log.replay().unwrap();
    let mut seen: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    fn walk(op: &crate::change_log::ChangeOp, seen: &mut std::collections::HashMap<String, Vec<String>>) {
        match op {
            crate::change_log::ChangeOp::Batch { ops } => ops.iter().for_each(|o| walk(o, seen)),
            crate::change_log::ChangeOp::FieldSet { book, value, .. } if book.starts_with('t') => seen.entry(book.clone()).or_default().push(value.clone().unwrap()),
            _ => {}
        }
    }
    for change in replay.changes() {
        walk(&change.op, &mut seen);
    }
    assert_eq!(seen.len(), threads);
    for (book, values) in seen {
        assert_eq!(values, (0..per_thread).map(|i| i.to_string()).collect::<Vec<_>>(), "{book}");
    }
    assert!(log.verify().unwrap().is_clean(), "the chain is intact");
    // A lone writer gets exactly the entry append would have written: the op itself.
    let lone_before = log.entry_count();
    log.append_grouped(crate::change_log::ChangeOp::FieldSet { book: "lone".into(), field: "rating".into(), value: None }).unwrap();
    assert_eq!(log.entry_count() - lone_before, 1);
}

/// How long the main connection is unavailable while a duplicate scan reads the checksums: a thread runs the scan's
/// reads on repeat while this one times the cheapest request there is (`all_book_ids`), which needs the connection.
#[test]
#[ignore]
fn t29b_probe_scan_blocks_the_library() {
    let n = probe_size();
    let (_dir, cache) = big_library(n);
    let ids = cache.all_book_ids().unwrap();
    let store = cache.checksums();
    let records: Vec<HashRecord> = ids.iter().map(|id| HashRecord { book_id: *id, kind: "format".into(), key: "PDF".into(), hash: format!("hash-{id}"), size: 1000, identity: None }).collect();
    t("record 30k hashes", || store.record_many(&records).unwrap());
    let outcomes: Vec<(String, Result<PdfFingerprint, String>)> = ids.iter().take(n * 5 / 6).map(|id| (format!("hash-{id}"), Ok(fingerprint(12)))).collect();
    t("record 25k fingerprints", || {
        for chunk in outcomes.chunks(500) {
            let refs: Vec<(&str, Result<PdfFingerprint, String>)> = chunk.iter().map(|(h, o)| (h.as_str(), o.clone())).collect();
            store.finish_pdf_fingerprints("fingerprint", &refs, &[]).unwrap();
        }
    });
    t("pdf_status", || store.pdf_status().unwrap());
    t("pdf_status (four-query form)", || pdf_status_four_queries(&cache));
    t("pdf_fingerprints()", || store.pdf_fingerprints().unwrap().len());
    t("books_sharing_content", || store.books_sharing_content("format").unwrap().len());
    // Scan on one thread, a cheap request timed on this one. "before": the scan's reads on the main connection with the
    // four-query status, as it was; "after": on the reader with the one-query status.
    for before in [true, false] {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let scanning = {
            let stop = stop.clone();
            let conn = cache.backend.conn.clone();
            let path = cache.backend.library_path.clone();
            let after_store = cache.checksums();
            let before_cache = if before { Some(cache.backend.conn.clone()) } else { None };
            std::thread::spawn(move || {
                let main_store = crate::checksums::ChecksumStore::new(conn.clone(), &path);
                let mut rounds = 0;
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    if before_cache.is_some() {
                        // The four counts, then the fingerprints, all on the main connection.
                        let c = conn.lock().unwrap();
                        let version = crate::pdf_similarity::FINGERPRINT_VERSION;
                        for sql in [
                            "SELECT count(*) FROM data d JOIN checksums_db.file_checksums c ON c.book_id = d.book AND c.kind = 'format' AND c.key = d.format JOIN checksums_db.pdf_fingerprints p ON p.blake3_hex = c.blake3_hex AND p.version = ?1 WHERE UPPER(d.format) = 'PDF' AND p.status = 'ok'",
                            "SELECT count(*) FROM data d JOIN checksums_db.file_checksums c ON c.book_id = d.book AND c.kind = 'format' AND c.key = d.format JOIN checksums_db.pdf_fingerprints p ON p.blake3_hex = c.blake3_hex AND p.version = ?1 WHERE UPPER(d.format) = 'PDF' AND p.status = 'failed'",
                            "SELECT count(*) FROM data d JOIN checksums_db.file_checksums c ON c.book_id = d.book AND c.kind = 'format' AND c.key = d.format WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1",
                            "SELECT count(*) FROM data d WHERE UPPER(d.format) = 'PDF' AND ?1 = ?1",
                        ] {
                            let _: i64 = c.query_row(sql, [version], |r| r.get(0)).unwrap();
                        }
                        drop(c);
                        main_store.pdf_fingerprints().unwrap();
                    } else {
                        after_store.pdf_status().unwrap();
                        after_store.pdf_fingerprints().unwrap();
                    }
                    rounds += 1;
                }
                rounds
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut worst = 0f64;
        let mut total = 0f64;
        let mut count = 0;
        let started = Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(5) {
            let s = Instant::now();
            let _ = cache.all_book_ids().unwrap();
            let _ = cache.field_for(ids[(count * 7) % ids.len()], "title").unwrap();
            let ms = s.elapsed().as_secs_f64() * 1000.0;
            worst = worst.max(ms);
            total += ms;
            count += 1;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let rounds = scanning.join().unwrap();
        println!("{} during {rounds} scan rounds: {count} requests, mean {:.1} ms, worst {:.1} ms", if before { "BEFORE" } else { "AFTER " }, total / count as f64, worst);
    }
}
