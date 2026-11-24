//! The check that stops the change log silently regressing (#901).
//!
//! #899 makes `.calibre-oxide/changes/` the authoritative record of a
//! library and `metadata.db` a disposable derived cache. That claim is only
//! true while **every** durable write reaches the log, and nothing in the
//! type system enforces it: a write that updates the database and forgets to
//! append an op looks identical to one that does, right up until somebody
//! rebuilds the cache and the edit is gone.
//!
//! That is not hypothetical. It was found twice in a week by accident --
//! `embed_metadata` rewrote a file and updated its checksum but never
//! reached the log, and `calibredb set_metadata title` went through
//! `update_book_metadata`, which did not either.
//!
//! # How this audits
//!
//! By *effect*, not by reading source. Each [`Case`] performs one real write
//! through the public API, then rebuilds a second library from the first
//! one's log **alone** and compares a full snapshot of both. A silent write
//! shows up as a difference, whatever route it took and however it is
//! spelled -- which is why this is not a grep for `self.record(`.
//!
//! Every case also asserts the operation *changed* the snapshot. Without
//! that a case whose write silently did nothing would pass vacuously: both
//! libraries equally unchanged.
//!
//! # Known gaps are listed, not hidden
//!
//! [`KNOWN_UNLOGGED`] names writes that are deliberately not in the log yet,
//! each with a reason and an issue. The list is checked against the source,
//! so it cannot quietly grow stale in either direction: a method added to it
//! that no longer exists fails, and a mutating method that is in neither
//! this list nor a [`Case`] fails.

use std::path::Path;

use crate::cache::Cache;
use calibre_ebooks::metadata::MetaInformation;

/// Everything about a library that a rebuild has to reproduce, as sorted
/// human-readable lines so a failing diff says what differed.
///
/// Deliberately wide. The snapshot the original rebuild test used compared
/// only uuid, title, author_sort, path and formats, so a silent write to
/// tags, rating, identifiers or a custom column passed it. Breadth here is
/// what makes the audit able to see anything.
fn snapshot(cache: &Cache) -> Vec<String> {
    let mut lines = Vec::new();

    let books: Vec<(i32, String)> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, uuid FROM books ORDER BY uuid").unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().map(Result::unwrap).collect()
    };

    let labels: Vec<String> = {
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT label, name, datatype, is_multiple, display, normalized FROM custom_columns ORDER BY label").unwrap();
        let rows: Vec<(String, String, String, i32, String, i32)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (label, name, datatype, multiple, display, normalized) in &rows {
            lines.push(format!("column {label} name={name} type={datatype} multiple={multiple} normalized={normalized} display={display}"));
        }
        rows.into_iter().map(|(label, ..)| label).collect()
    };

    for (id, uuid) in &books {
        // Every field `set_field` can write, read back the way a client
        // would. `last_modified` is excluded: it is stamped by a trigger
        // on every write, so it differs between two libraries by
        // construction and says nothing about what was recorded.
        for field in [
            "title", "sort", "author_sort", "series_index", "timestamp", "pubdate", "comments", "series", "publisher", "rating", "tags", "languages",
            "authors", "identifiers", "path",
        ] {
            let value = cache.field_for(*id, field).unwrap().unwrap_or_default();
            lines.push(format!("book {uuid} {field}={value}"));
        }
        // Not through `field_for`, which returns `None` for `has_cover`:
        // the first version of this snapshot read it that way, saw an
        // empty string before and after, and so could not see a cover
        // being set at all.
        lines.push(format!("book {uuid} has_cover={}", cache.has_cover(*id).unwrap()));
        for (format, name) in cache.format_file_names(*id).unwrap() {
            lines.push(format!("book {uuid} format {format}={name}"));
        }
        for label in &labels {
            let value = cache.get_custom_column_value(*id, label).unwrap().unwrap_or_default();
            lines.push(format!("book {uuid} custom {label}={value}"));
            // A series column's index (empty for any other column).
            let index = cache.get_custom_column_value(*id, &format!("{label}_index")).unwrap().unwrap_or_default();
            lines.push(format!("book {uuid} custom {label}_index={index}"));
        }
    }

    let conn = cache.backend.conn.lock().unwrap();
    let mut stmt = conn.prepare("SELECT key, val FROM preferences ORDER BY key").unwrap();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap() {
        let (key, val) = row.unwrap();
        lines.push(format!("pref {key}={val}"));
    }
    lines
}

/// Copies only `.calibre-oxide/changes`, `snapshots` and the install id --
/// the "somebody deleted the database" scenario.
fn clone_log_only(from: &Path, to: &Path) {
    let src = from.join(crate::constants::LIBRARY_HANDLE_DIR_NAME);
    let dst = to.join(crate::constants::LIBRARY_HANDLE_DIR_NAME);
    for sub in ["changes", "snapshots"] {
        std::fs::create_dir_all(dst.join(sub)).unwrap();
        if let Ok(entries) = std::fs::read_dir(src.join(sub)) {
            for entry in entries.flatten() {
                std::fs::copy(entry.path(), dst.join(sub).join(entry.file_name())).unwrap();
            }
        }
    }
    if let Ok(id) = std::fs::read(src.join("install-id")) {
        std::fs::write(dst.join("install-id"), id).unwrap();
    }
}

/// What every case starts from: two books with real content, a custom
/// column, and some shared metadata so the bulk renames have something to
/// rename. Built entirely through recorded operations, so any difference
/// afterwards is the case's own doing.
struct Fixture {
    first: i32,
    second: i32,
}

fn build_fixture(dir: &Path, cache: &Cache) -> Fixture {
    let make = |name: &str, title: &str, author: &str| {
        let source = dir.join(name);
        std::fs::write(&source, format!("%PDF-1.4 {name}")).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec![author.to_string()];
        cache.add_book(&source, &meta).unwrap()
    };
    let first = make("first.pdf", "First Book", "Shared Author");
    let second = make("second.pdf", "Second Book", "Shared Author");
    cache.set_field(first, "tags", "shared-tag, first-only").unwrap();
    cache.set_field(second, "tags", "shared-tag").unwrap();
    cache.set_field(first, "publisher", "Shared Press").unwrap();
    cache.set_field(second, "publisher", "Shared Press").unwrap();
    cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();
    // One column of every other datatype, calibre's shapes, so each kind of
    // value write is proven to reach the log (and to replay).
    cache.add_custom_column("genre", "Genre", "text", true).unwrap();
    cache.add_custom_column("saga", "Saga", "series", false).unwrap();
    let moods = serde_json::json!({"enum_values": ["Happy", "Sad"], "enum_colors": [], "use_decorations": false});
    cache.add_custom_column_with_display("mood", "Mood", "enumeration", false, &moods).unwrap();
    cache.add_custom_column("stars", "Stars", "rating", false).unwrap();
    cache.add_custom_column("pages", "Pages", "int", false).unwrap();
    cache.add_custom_column("score", "Score", "float", false).unwrap();
    cache.add_custom_column("read", "Read", "bool", false).unwrap();
    cache.add_custom_column("finished", "Finished", "datetime", false).unwrap();
    cache.add_custom_column("notes", "Notes", "comments", false).unwrap();
    // Values already in place, so a case that clears one has something to
    // clear (setting and clearing in one case would be no observable change).
    cache.set_custom_column_value(first, "genre", "Zebra, Apple, Mango").unwrap();
    cache.set_custom_column_value(first, "saga", "The Saga [2.5]").unwrap();
    cache.set_custom_column_value(second, "saga", "The Saga [3]").unwrap();
    cache.set_custom_column_value(first, "mood", "Happy").unwrap();
    cache.set_custom_column_value(first, "stars", "6").unwrap();
    Fixture { first, second }
}

fn id_of(cache: &Cache, table: &str, name: &str) -> i32 {
    let conn = cache.backend.conn.lock().unwrap();
    conn.query_row(&format!("SELECT id FROM {table} WHERE name = ?1"), [name], |r| r.get(0)).unwrap()
}

/// How a case performs its write: through `Cache`, or through `Library`,
/// which is the API `calibredb` uses and which had writes of its own.
enum Act {
    Cache(fn(&Cache, &Fixture)),
    Library(fn(&mut crate::library::Library, &Fixture)),
}

struct Case {
    name: &'static str,
    act: Act,
}

macro_rules! case {
    ($name:expr, $act:expr) => {
        Case { name: $name, act: Act::Cache($act) }
    };
}
macro_rules! lib_case {
    ($name:expr, $act:expr) => {
        Case { name: $name, act: Act::Library($act) }
    };
}

/// A calibre-made library (calibre's schema, one book with its files) at
/// `Nested Calibre` under `library`, left as calibre leaves one: no
/// `.calibre-oxide` folder.
fn nested_calibre_library(library: &Path) {
    let nested = library.join("Nested Calibre");
    {
        let n = Cache::new(&nested).unwrap();
        n.add_custom_column("shelf", "Shelf", "text", false).unwrap();
        n.add_custom_column("nested_only", "Nested Only", "series", false).unwrap();
        {
            let conn = n.backend.conn.lock().unwrap();
            conn.execute("INSERT INTO books (id, title, author_sort, path, has_cover) VALUES (1, 'Nested Book', 'Nested Author', 'Nested Author/Nested Book (1)', 1)", []).unwrap();
            conn.execute("INSERT INTO authors (name, sort) VALUES ('Nested Author', 'Author, Nested')", []).unwrap();
            conn.execute("INSERT INTO books_authors_link (book, author) SELECT 1, id FROM authors WHERE name = 'Nested Author'", []).unwrap();
            conn.execute("INSERT INTO data (book, format, uncompressed_size, name) VALUES (1, 'PDF', 13, 'Nested Book - Nested Author')", []).unwrap();
        }
        n.set_custom_column_value(1, "shelf", "Nested Shelf").unwrap();
        n.set_custom_column_value(1, "nested_only", "Nested Saga [2]").unwrap();
        n.set_field(1, "tags", "nested tag").unwrap();
        let dir = nested.join("Nested Author/Nested Book (1)");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Nested Book - Nested Author.pdf"), b"%PDF-1.4 nest").unwrap();
        std::fs::write(dir.join("cover.jpg"), b"\xFF\xD8\xFF\xE0 nested cover").unwrap();
    }
    std::fs::remove_dir_all(nested.join(crate::constants::LIBRARY_HANDLE_DIR_NAME)).unwrap();
}

/// One real write per public mutating operation. Adding a method that
/// mutates the database means adding a case here, and
/// [`every_mutating_method_is_audited`] fails until you do.
const CASES: &[Case] = &[
    // `set_field`, once per field it can write.
    case!("set_field title", |c, f| c.set_field(f.first, "title", "Renamed").unwrap()),
    case!("set_field sort", |c, f| c.set_field(f.first, "sort", "Sorted Differently").unwrap()),
    case!("set_field author_sort", |c, f| c.set_field(f.first, "author_sort", "Author, Sorted").unwrap()),
    case!("set_field series_index", |c, f| c.set_field(f.first, "series_index", "3.5").unwrap()),
    case!("set_field timestamp", |c, f| c.set_field(f.first, "timestamp", "2001-02-03T04:05:06+00:00").unwrap()),
    case!("set_field pubdate", |c, f| c.set_field(f.first, "pubdate", "1999-12-31T00:00:00+00:00").unwrap()),
    case!("set_field comments", |c, f| c.set_field(f.first, "comments", "A comment.").unwrap()),
    case!("set_field series", |c, f| c.set_field(f.first, "series", "A Series").unwrap()),
    case!("set_field publisher", |c, f| c.set_field(f.first, "publisher", "Other Press").unwrap()),
    case!("set_field rating", |c, f| c.set_field(f.first, "rating", "8").unwrap()),
    case!("set_field tags", |c, f| c.set_field(f.first, "tags", "alpha, beta").unwrap()),
    case!("set_field languages", |c, f| c.set_field(f.first, "languages", "fra").unwrap()),
    case!("set_field authors", |c, f| c.set_field(f.first, "authors", "Brand New Author").unwrap()),
    case!("set_field identifiers", |c, f| c.set_field(f.first, "identifiers", "isbn:9780000000002").unwrap()),
    // The others.
    case!("update_book_metadata", |c, f| c.update_book_metadata(f.first, "Via Update", "Via Update Author").unwrap()),
    case!("rename_author", |c, _| {
        let id = id_of(c, "authors", "Shared Author");
        c.rename_author(id, "Renamed Author").unwrap()
    }),
    case!("rename_author (merging into an existing one)", |c, f| {
        c.set_field(f.second, "authors", "Existing Author").unwrap();
        let id = id_of(c, "authors", "Shared Author");
        c.rename_author(id, "Existing Author").unwrap()
    }),
    case!("rename_tag", |c, _| {
        let id = id_of(c, "tags", "shared-tag");
        c.rename_tag(id, "renamed-tag").unwrap()
    }),
    case!("rename_publisher", |c, _| {
        let id = id_of(c, "publishers", "Shared Press");
        c.rename_publisher(id, "Renamed Press").unwrap()
    }),
    case!("set_custom_column_value", |c, f| c.set_custom_column_value(f.first, "shelf", "Top Shelf").unwrap()),
    case!("set_custom_column_value (multiple text, order kept)", |c, f| c.set_custom_column_value(f.second, "genre", "Mango, Zebra, apple, MANGO").unwrap()),
    case!("set_custom_column_value (multiple text, cleared)", |c, f| c.set_custom_column_value(f.first, "genre", "").unwrap()),
    case!("set_custom_column_value (series, with index)", |c, f| c.set_custom_column_value(f.second, "saga", "Another Saga [4]").unwrap()),
    case!("set_custom_column_value (series, index kept)", |c, f| c.set_custom_column_value(f.first, "saga", "Renamed Saga").unwrap()),
    case!("set_custom_column_value (series index)", |c, f| c.set_custom_column_value(f.first, "saga_index", "9").unwrap()),
    case!("set_custom_column_value (series, cleared)", |c, f| c.set_custom_column_value(f.first, "saga", "").unwrap()),
    case!("set_custom_column_value (enumeration)", |c, f| c.set_custom_column_value(f.first, "mood", "Sad").unwrap()),
    case!("set_custom_column_value (rating)", |c, f| c.set_custom_column_value(f.second, "stars", "9").unwrap()),
    case!("set_custom_column_value (rating, cleared)", |c, f| c.set_custom_column_value(f.first, "stars", "0").unwrap()),
    case!("set_custom_column_value (int)", |c, f| c.set_custom_column_value(f.first, "pages", "412").unwrap()),
    case!("set_custom_column_value (float)", |c, f| c.set_custom_column_value(f.first, "score", "8.25").unwrap()),
    case!("set_custom_column_value (bool)", |c, f| c.set_custom_column_value(f.first, "read", "true").unwrap()),
    case!("set_custom_column_value (datetime)", |c, f| c.set_custom_column_value(f.first, "finished", "2022-01-15 00:00:00+00:00").unwrap()),
    case!("set_custom_column_value (comments)", |c, f| c.set_custom_column_value(f.first, "notes", "<p>hi</p>").unwrap()),
    case!("add_custom_column_with_display (enumeration)", |c, _| {
        let display = serde_json::json!({"enum_values": ["One", "Two"], "enum_colors": [], "use_decorations": false});
        c.add_custom_column_with_display("choice", "Choice", "enumeration", false, &display).unwrap();
    }),
    case!("covers::set_cover", |c, f| crate::covers::set_cover(c, f.first, b"\xFF\xD8\xFF\xE0 not really a jpeg").unwrap()),
    // Already recorded -- kept so they stay that way.
    case!("remove_format", |c, f| c.remove_format(f.first, "PDF").unwrap()),
    case!("delete_book", |c, f| c.delete_book(f.second).unwrap()),
    // The trash: a book trashed and restored comes back under its own uuid
    // with its fields, and an edit made afterwards must replay onto it. A
    // restore that logged nothing would leave the rebuilt library without
    // the book at all.
    case!("trash_book + restore_trashed_book", |c, f| {
        c.delete_book(f.second).unwrap();
        let entry = c.list_trash().unwrap().remove(0);
        let restored = c.restore_trashed_book(&entry.key).unwrap();
        c.set_field(restored.book_id, "tags", "back-from-the-trash").unwrap();
    }),
    case!("remove_format + restore_trashed_format", |c, f| {
        c.remove_format(f.first, "PDF").unwrap();
        let entry = c.list_trash().unwrap().remove(0);
        c.restore_trashed_format(&entry.key, "PDF").unwrap();
        c.set_field(f.first, "tags", "format-came-back").unwrap();
    }),
    // The folder importer's batch write: books laid out `Author/Title (id)`,
    // with their formats and hashes, from files staged in the library.
    case!("import_books_batch", |c, _| {
        let staged = c.backend.library_path.join(crate::constants::LIBRARY_HANDLE_DIR_NAME).join("import-staging").join("audit");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("imported.pdf"), b"%PDF imported").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "Imported".to_string();
        meta.authors = vec!["Import Author".to_string()];
        let item = crate::cache::ImportItem {
            metadata: meta,
            files: vec![crate::cache::ImportFile { staged: staged.join("imported.pdf"), ext: "pdf".to_string(), size: 13, hash: blake3::hash(b"%PDF imported").to_hex().to_string() }],
        };
        c.import_books_batch(&[item]).remove(0).unwrap();
    }),
    case!("trash_book (stays removed)", |c, f| {
        c.delete_book_with(f.first, crate::cache::BookFiles::Trash).unwrap();
        c.empty_trash().unwrap();
    }),
    case!("rename_format_files", |c, f| { c.rename_format_files(f.first, "renamed-on-disk").unwrap(); }),
    case!("set_preference", |c, _| c.set_preference("audit_pref", "audit_value").unwrap()),
    case!("add_custom_column", |c, _| { c.add_custom_column("extra", "Extra", "text", false).unwrap(); }),
    case!("remove_custom_column", |c, _| c.remove_custom_column("shelf").unwrap()),
    case!("add_format", |c, f| {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("second.epub");
        std::fs::write(&incoming, b"PK epub stand-in").unwrap();
        c.add_format(f.first, &incoming, "epub", true).unwrap();
    }),
    // Merge books: formats, every metadata rule, a cover, custom columns,
    // and the deletion of the sources, all through the ordinary write
    // paths -- so a rebuild from the log must reproduce the merged library.
    case!("merge_books (merge and delete)", |c, f| {
        let dir = tempfile::tempdir().unwrap();
        let incoming = dir.path().join("second.epub");
        std::fs::write(&incoming, b"PK second's epub").unwrap();
        c.add_format(f.second, &incoming, "epub", true).unwrap();
        c.set_field(f.second, "comments", "second's comment").unwrap();
        c.set_field(f.second, "identifiers", "isbn:9780000000009").unwrap();
        c.set_field(f.second, "languages", "eng").unwrap();
        c.set_field(f.second, "rating", "7").unwrap();
        c.set_field(f.second, "series", "Merged Series").unwrap();
        c.set_custom_column_value(f.second, "shelf", "Second Shelf").unwrap();
        c.set_custom_column_value(f.second, "genre", "Zebra, Quokka").unwrap();
        c.set_custom_column_value(f.second, "mood", "Sad").unwrap();
        c.set_custom_column_value(f.second, "pages", "99").unwrap();
        crate::covers::set_cover(c, f.second, b"\xFF\xD8\xFF\xE0 second's cover").unwrap();
        crate::merge::merge_books(c, f.first, &[f.second], &crate::merge::MergeOptions::default()).unwrap();
    }),
    case!("merge_books (keep sources, dry run changes nothing)", |c, f| {
        let dry = crate::merge::MergeOptions { dry_run: true, ..Default::default() };
        crate::merge::merge_books(c, f.first, &[f.second], &dry).unwrap();
        let keep = crate::merge::MergeOptions { mode: crate::merge::MergeMode::MergeKeepSources, ..Default::default() };
        c.set_field(f.second, "comments", "to be copied").unwrap();
        crate::merge::merge_books(c, f.first, &[f.second], &keep).unwrap();
    }),
    // A batch registered in one transaction and one run of change-log
    // entries, as the scan does it: several books, one of them with a
    // second author, one with a cover, all of which must be in the log.
    case!("register_books_in_place (batch)", |c, _| {
        let library = c.backend.library_path.clone();
        let mut items = Vec::new();
        for (i, name) in ["Batch One.pdf", "Batch Two.pdf", "Batch Three.pdf"].iter().enumerate() {
            let bytes = format!("%PDF-1.4 batch {i}");
            std::fs::write(library.join(name), &bytes).unwrap();
            let mut meta = MetaInformation::default();
            meta.title = name.trim_end_matches(".pdf").to_string();
            meta.authors = if i == 1 { vec!["First Batch Author".to_string(), "Second Batch Author".to_string()] } else { vec!["Unknown".to_string()] };
            if i == 2 {
                meta.cover_data = (Some("jpg".to_string()), b"\xFF\xD8\xFF\xE0 batch cover".to_vec());
            }
            items.push(crate::cache::InPlaceFile {
                relative_path: name.to_string(),
                metadata: meta,
                size: bytes.len() as i64,
                hash: blake3::hash(bytes.as_bytes()).to_hex().to_string(),
                identity: None,
                queue_enrichment: true,
            });
        }
        for result in c.register_books_in_place(&items) {
            result.unwrap();
        }
    }),
    // The scan's metadata pass, as one transaction and one log entry: a title,
    // two authors and a cover written over a placeholder book.
    case!("write_enrichment_batch", |c, _| {
        let bytes = b"%PDF-1.4 enrich me".to_vec();
        std::fs::write(c.backend.library_path.join("Enrich Me.pdf"), &bytes).unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "Enrich Me".to_string();
        meta.authors = vec!["Unknown".to_string()];
        let item = crate::cache::InPlaceFile { relative_path: "Enrich Me.pdf".to_string(), metadata: meta, size: bytes.len() as i64, hash: blake3::hash(&bytes).to_hex().to_string(), identity: None, queue_enrichment: true };
        let id = c.register_books_in_place(&[item]).remove(0).unwrap();
        let write = crate::enrich_batch::EnrichmentWrite {
            book_id: id,
            label: "Enrich Me".to_string(),
            stem: "Enrich Me".to_string(),
            title: Some("A Real Title".to_string()),
            authors: vec!["Real One".to_string(), "Real Two".to_string()],
            cover: Some(b"\xFF\xD8\xFF\xE0 enriched cover".to_vec()),
        };
        for result in c.write_enrichment_batch(&[write], None) {
            result.unwrap();
        }
    }),
    case!("register_book_in_place", |c, _| {
        std::fs::write(c.backend.library_path.join("Loose Book.pdf"), b"%PDF-1.4 loose").unwrap();
        let mut meta = MetaInformation::default();
        meta.title = "Loose Book".to_string();
        meta.authors = vec!["Loose Author".to_string()];
        c.register_book_in_place("Loose Book.pdf", &meta).unwrap();
    }),
    // A book in the old `<author>/<title>/` layout, relocated to a new
    // folder (what organize does). `path` was written with raw SQL here.
    case!("set_book_path (relocation)", |c, f| {
        let library = c.backend.library_path.clone();
        let (format, stem) = c.format_file_names(f.first).unwrap().remove(0);
        let rel = "Shared Author/First Book";
        std::fs::create_dir_all(library.join(rel)).unwrap();
        let name = format!("{stem}.{}", format.to_lowercase());
        std::fs::rename(library.join(&name), library.join(rel).join(&name)).unwrap();
        c.set_book_path(f.first, rel).unwrap();
        c.set_book_path(f.first, "Shared Author/Retitled").unwrap();
    }),
    case!("set_book_location", |c, f| {
        let (format, _) = c.format_file_names(f.first).unwrap().remove(0);
        let names: std::collections::BTreeMap<String, String> = [(format, "Relocated".to_string())].into();
        c.set_book_location(f.first, Some("Shared Author/Relocated"), &names).unwrap();
    }),
    case!("place_book_in_folder", |c, f| c.place_book_in_folder(f.first, "Shared Author/First Book (1)").unwrap()),
    // A calibre library nested in this one, brought in place: its book with
    // its author, a custom column this library lacks and one it has.
    case!("absorb_library", |c, _| {
        nested_calibre_library(&c.backend.library_path);
        crate::absorb::absorb_library(c, "Nested Calibre", &Default::default(), &crate::scan_pipeline::PipelineOptions::with_workers(1), &crate::scan_pipeline::JobControl::default()).unwrap();
    }),
    case!("unabsorb_library", |c, f| {
        nested_calibre_library(&c.backend.library_path);
        crate::absorb::absorb_library(c, "Nested Calibre", &Default::default(), &crate::scan_pipeline::PipelineOptions::with_workers(1), &crate::scan_pipeline::JobControl::default()).unwrap();
        crate::absorb::unabsorb_library(c, "Nested Calibre", &crate::scan_pipeline::JobControl::default()).unwrap();
        // Something must differ from the start for the case to prove anything.
        c.set_field(f.first, "title", "Changed After").unwrap();
    }),
    case!("clear_preference", |c, _| {
        c.set_preference("audit_pref", "audit_value").unwrap();
        c.clear_preference("audit_pref").unwrap();
        c.set_preference("kept_pref", "kept").unwrap();
    }),
    // JSON-valued and namespaced: where saved searches, virtual libraries
    // and user categories actually live.
    case!("Cache::set_pref (plain key)", |c, _| c.set_pref("virtual_libraries", &serde_json::json!({"Fiction": "tags:fiction"}), None).unwrap()),
    case!("Cache::set_pref (namespaced)", |c, _| c.set_pref("saved_searches", &serde_json::json!({"Unread": "not rating:true"}), Some("gui")).unwrap()),
    // The `Library` API is what `calibredb` calls, and it had writes of
    // its own that went round `Cache` entirely.
    lib_case!("Library::set_preference", |l, _| l.set_preference("library_pref", "library_value").unwrap()),
    lib_case!("Library::set_metadata title", |l, f| l.set_metadata(f.first, "title", "Via CLI").unwrap()),
    lib_case!("Library::set_metadata author", |l, f| l.set_metadata(f.first, "author", "CLI Author").unwrap()),
    lib_case!("Library::set_metadata sort", |l, f| l.set_metadata(f.first, "sort", "CLI Sort").unwrap()),
    lib_case!("Library::set_metadata author_sort", |l, f| l.set_metadata(f.first, "author_sort", "CLI, Author").unwrap()),
    lib_case!("Library::set_metadata pubdate", |l, f| l.set_metadata(f.first, "pubdate", "1980-01-02T00:00:00+00:00").unwrap()),
    lib_case!("Library::set_metadata timestamp", |l, f| l.set_metadata(f.first, "timestamp", "1981-02-03T00:00:00+00:00").unwrap()),
    lib_case!("Library::set_metadata series_index", |l, f| l.set_metadata(f.first, "series_index", "7").unwrap()),
];

/// The audit. Run per case so a failure names the operation.
fn run_case(case: &Case) -> Result<(), String> {
    let original_dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(original_dir.path()).unwrap();
    let fixture = build_fixture(original_dir.path(), &cache);

    let before = snapshot(&cache);
    match &case.act {
        Act::Cache(act) => act(&cache, &fixture),
        Act::Library(act) => {
            // A second handle on the same library, as `calibredb` would
            // have. The snapshot still reads through `cache`.
            let mut library = crate::library::Library::open(original_dir.path().to_path_buf()).unwrap();
            act(&mut library, &fixture);
        }
    }
    // Read through a freshly opened `Cache`, not `cache`: a write made
    // through a *different* handle (the `Library` cases) is invisible to
    // `cache`'s in-memory field store, which made those cases report "no
    // observable effect". Reopening is also what a user does after the
    // CLI has written to the library.
    let after = snapshot(&Cache::new(original_dir.path()).unwrap());

    if before == after {
        return Err(format!("{}: the operation had no observable effect, so this case proves nothing", case.name));
    }

    let rebuilt_dir = tempfile::tempdir().unwrap();
    clone_log_only(original_dir.path(), rebuilt_dir.path());
    let rebuilt = Cache::new(rebuilt_dir.path()).unwrap();
    rebuilt.rebuild_from_change_log().unwrap();
    let replayed = snapshot(&rebuilt);

    if replayed == after {
        return Ok(());
    }
    let missing: Vec<&String> = after.iter().filter(|l| !replayed.contains(l)).collect();
    let extra: Vec<&String> = replayed.iter().filter(|l| !after.contains(l)).collect();
    Err(format!("{}: a rebuild from the log does not reproduce the library.\n  lost in the rebuild: {missing:#?}\n  invented by the rebuild: {extra:#?}", case.name))
}

/// **The invariant.** Every write the public API offers survives a rebuild
/// from the log alone.
#[test]
fn every_write_survives_a_rebuild_from_the_log() {
    let failures: Vec<String> = CASES.iter().filter_map(|case| run_case(case).err()).collect();
    assert!(failures.is_empty(), "{} of {} operations are not in the change log:\n\n{}", failures.len(), CASES.len(), failures.join("\n\n"));
}



// ---------------------------------------------------------------------------
// The tripwire for writes that have not been added yet.
// ---------------------------------------------------------------------------

/// Why a function that writes to the database does not need its own case.
#[derive(Debug)]
enum Account {
    /// Exercised by the named [`Case`] -- which must exist.
    Case(&'static str),
    /// Replay's own primitive: it applies a logged change to the database
    /// and records nothing, or records the op it is the inverse of.
    Replay(&'static str),
    /// Initialisation or self-healing on load; not a user write.
    Repair(&'static str),
    /// A raw primitive with no production caller; production code goes
    /// through `Cache`. Asserted by review, not by this test.
    Primitive(&'static str),
    /// A real write that is **not** in the log, on purpose, for now. The
    /// reason must name the issue tracking it.
    Unlogged(&'static str),
    /// Writes a private scratch copy of a database (a snapshot being
    /// prepared for export), never the open library.
    Scratch(&'static str),
}
use Account::*;

/// Every non-test function in `calibre_db` that executes a mutating
/// statement, and how it is accounted for.
///
/// **Adding a method that writes to the database fails
/// [`every_mutating_function_is_accounted_for`] until it appears here.**
/// That is the point: the failure is the prompt to either give it a [`Case`]
/// above -- which proves it reaches the log -- or say why it need not.
const ACCOUNTED: &[(&str, Account)] = &[
    ("cache.rs::set_field_inner", Case("set_field title")),
    ("cache.rs::set_many_to_one_field", Case("set_field publisher")),
    ("cache.rs::set_rating_field", Case("set_field rating")),
    ("cache.rs::set_identifiers_field", Case("set_field identifiers")),
    ("cache.rs::update_book_metadata", Case("update_book_metadata")),
    ("cache.rs::rename_item_in_db", Case("rename_tag")),
    ("cache.rs::add_custom_column_with_display", Case("add_custom_column")),
    ("custom_columns.rs::write_value_inner", Case("set_custom_column_value (multiple text, order kept)")),
    ("custom_columns.rs::write_series_index", Case("set_custom_column_value (series index)")),
    ("custom_columns.rs::item_id", Case("set_custom_column_value (series, with index)")),
    ("custom_columns.rs::remove_unused_items", Case("set_custom_column_value (multiple text, cleared)")),
    ("custom_columns.rs::purge_book", Case("delete_book")),
    ("custom_columns.rs::create_schema", Case("add_custom_column_with_display (enumeration)")),
    ("custom_columns.rs::drop_schema", Case("remove_custom_column")),
    ("cache.rs::remove_custom_column", Case("remove_custom_column")),
    ("cache.rs::set_preference", Case("set_preference")),
    ("cache.rs::clear_preference", Case("clear_preference")),
    ("cache.rs::add_format", Case("add_format")),
    ("cache.rs::remove_format_inner", Case("remove_format")),
    ("cache.rs::delete_book_rows", Case("delete_book")),
    ("cache.rs::set_book_location", Case("set_book_location")),
    ("cache.rs::rename_format_files", Case("rename_format_files")),
    ("cache.rs::register_books_in_place", Case("register_books_in_place (batch)")),
    ("enrich_batch.rs::write_enrichment_batch", Case("write_enrichment_batch")),
    ("cache.rs::link_many_to_many_in", Case("set_field tags")),
    ("cache.rs::insert_book_in_tx", Case("register_books_in_place (batch)")),
    ("cache.rs::import_books_batch", Case("import_books_batch")),
    ("cache.rs::place_book_in_folder", Case("place_book_in_folder")),
    ("absorb.rs::register_one", Case("absorb_library")),
    ("mirror.rs::prune_checksums", Scratch("prunes the travel copy's checksums snapshot in the mirror's scratch folder")),
    ("mirror.rs::prune_notes", Scratch("prunes the travel copy's notes snapshot in the mirror's scratch folder")),
    ("mirror.rs::rewrite_metadata", Scratch("prunes and re-identifies the copy's metadata.db snapshot in the mirror's scratch folder")),
    ("absorb.rs::intern", Case("absorb_library")),
    ("absorb.rs::unabsorb_library", Case("unabsorb_library")),
    ("covers.rs::set_cover", Case("covers::set_cover")),
    ("backend.rs::set_pref", Case("Cache::set_pref (plain key)")),
    ("cache.rs::set_book_path", Replay("records a `path` FieldSet itself; exercised by the set_book_path (relocation) case")),
    ("cache.rs::record_format_row", Replay("replay's FormatSet applier; records the op it applies")),
    ("cache.rs::set_format_name", Replay("records a FormatSet; exercised by the rename_format_files case")),
    ("cache.rs::forget_format_row", Replay("replay's FormatRemoved applier; records the op it applies")),
    ("cache.rs::insert_book_with_uuid", Replay("creates the row a replayed BookAdded describes")),
    ("backend.rs::new_inner", Repair("seeds the default preferences and schema when a library is first opened")),
    ("backend.rs::library_id", Repair("get-or-create of the database file's own id on first read; identity of the cache, not library content")),
    ("tables.rs::read_rating_table", Repair("deletes legacy rating-0 rows while loading; idempotent self-healing, not a user write")),
    ("backend.rs::update", Primitive("raw column write; production callers go through Cache::set_field (copy_to_library stopped using it in #965)")),
    ("backend.rs::insert_book", Primitive("raw row insert; production callers go through Cache::add_book")),
    ("backend.rs::delete_pref", Primitive("no production caller")),
    ("library.rs::insert_test_book", Primitive("test helper")),
    ("annotations.rs::set_annotations_for_book", Unlogged("annotations exist only in metadata.db, so a rebuild destroys them -- decision needed, #967")),
    ("cache.rs::set_last_read_position", Unlogged("reading position exists only in metadata.db -- decision needed, #967")),
    ("legacy.rs::delete_item_using_id", Unlogged("legacy-API item deletes; no production caller, needs an ItemDeleted op, #968")),
];

/// Files whose writes are not to `metadata.db`'s library content, so the
/// scan skips them -- each is its own durable store with its own recovery.
const EXCLUDED: &[(&str, &str)] = &[
    ("field_feed_tests.rs", "tests of the field snapshot, which write rows with raw SQL on purpose"),
    ("schema_upgrades.rs", "one-time schema migrations"),
    ("checksums.rs", "the checksum sidecar db"),
    ("orphans.rs", "the checksum sidecar db"),
    ("removal.rs", "the checksum sidecar db"),
    ("pdf_similarity/", "tests of the PDF fingerprint cache, which lives in the checksum sidecar db"),
    ("restore.rs", "legacy OPF restore (#951); rebuilds metadata.db rather than writing library content"),
    ("change_log/", "the log itself"),
    ("fts/", "the full-text-search sidecar db, rebuilt from book files"),
    ("notes/", "the notes sidecar db, already its own store"),
];

/// Non-test functions containing a mutating statement, as `file::fn`.
fn mutating_functions() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let statement = regex_lite_mutating();
    let mut found = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let relative = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
            if EXCLUDED.iter().any(|(prefix, _)| relative == *prefix || relative.starts_with(prefix)) {
                continue;
            }
            // CRLF checkouts (Windows, autocrlf) would hide the test-module marker below.
            let source = std::fs::read_to_string(&path).unwrap().replace("\r\n", "\n");
            // Everything before the test module; the files that have one
            // put it last.
            let production = source.split("\n#[cfg(test)]\nmod ").next().unwrap();
            let mut name: Option<String> = None;
            let mut body = String::new();
            let mut flush = |name: &Option<String>, body: &str, found: &mut Vec<String>| {
                if let Some(n) = name {
                    if statement(body) {
                        found.push(format!("{relative}::{n}"));
                    }
                }
            };
            for line in production.lines() {
                if let Some(n) = function_name(line) {
                    flush(&name, &body, &mut found);
                    name = Some(n);
                    body.clear();
                }
                body.push_str(line);
                body.push('\n');
            }
            flush(&name, &body, &mut found);
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The name of the function a line declares, if it declares one.
fn function_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("pub(crate) ").or_else(|| trimmed.strip_prefix("pub ")).unwrap_or(trimmed);
    let rest = rest.strip_prefix("fn ")?;
    let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

/// Whether a function body runs a statement that changes a row or table.
fn regex_lite_mutating() -> impl Fn(&str) -> bool {
    const STARTS: &[&str] = &["INSERT INTO ", "INSERT OR ", "UPDATE ", "DELETE FROM ", "DROP TABLE ", "CREATE TABLE ", "REPLACE INTO "];
    |body: &str| {
        // Only inside string literals, so a comment saying "UPDATE the
        // row" does not count.
        //
        // `contains`, not `starts_with`: a batch like
        // `execute_batch("PRAGMA ...; INSERT INTO ...")` does not begin
        // with the statement.
        body.split('"').skip(1).step_by(2).any(|literal| {
            STARTS.iter().any(|start| literal.contains(start)) && (!literal.contains("UPDATE ") || literal.contains(" SET ") || STARTS.iter().filter(|s| **s != "UPDATE ").any(|start| literal.contains(start)))
        })
    }
}

/// **The tripwire.** A function that writes to the database and is neither
/// audited nor explained fails here, so a new write path cannot ship silent.
#[test]
fn every_mutating_function_is_accounted_for() {
    let found = mutating_functions();
    let accounted: Vec<&str> = ACCOUNTED.iter().map(|(name, _)| *name).collect();

    let unaccounted: Vec<&String> = found.iter().filter(|f| !accounted.contains(&f.as_str())).collect();
    assert!(
        unaccounted.is_empty(),
        "these functions write to the database and are neither covered by a change-log audit case nor listed in ACCOUNTED:\n  {}\n\nGive each a `Case` (which proves it reaches the log) or add it to ACCOUNTED with a reason.",
        unaccounted.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
    );

    // And the other direction, so the table cannot rot: an entry whose
    // function no longer writes (or no longer exists) is stale.
    let stale: Vec<&str> = accounted.iter().copied().filter(|a| !found.iter().any(|f| f == a)).collect();
    assert!(stale.is_empty(), "ACCOUNTED lists functions that no longer write to the database: {stale:?}");
}

/// A case an entry points at has to exist, or the claim is empty.
#[test]
fn every_account_that_names_a_case_names_a_real_one() {
    for (function, account) in ACCOUNTED {
        if let Case(name) = account {
            assert!(CASES.iter().any(|c| c.name == *name), "{function} says it is covered by the case {name:?}, which does not exist");
        }
    }
}

/// "Not logged yet" must say where it is tracked, or it is just an
/// undocumented hole with better formatting.
#[test]
fn every_unlogged_write_names_the_issue_tracking_it() {
    for (function, account) in ACCOUNTED {
        if let Unlogged(reason) = account {
            assert!(reason.contains('#'), "{function} is unlogged but its reason names no issue: {reason}");
        }
    }
}
