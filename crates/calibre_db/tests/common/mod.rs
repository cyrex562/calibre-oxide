//! A library built exactly the way real calibre builds one.
//!
//! calibre itself is not installed on the machines these tests run on,
//! so there is no way to ask it for a sample library. What we do have is
//! calibre's own source: `old_src/resources/metadata_sqlite.sql` is the
//! schema it executes on a new library, and
//! `old_src/src/calibre/db/backend.py::create_custom_column` is the code
//! that makes a custom column. This module replays both, then lays the
//! files out the way calibre does (`Author/Title (id)/Title - Author.ext`,
//! `cover.jpg`, `metadata.opf`).
//!
//! Nothing here goes through calibre_db's own write paths on purpose: the
//! point is a library produced by something *other* than the code under
//! test.
#![allow(dead_code)]

use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

/// calibre's real schema, verbatim.
const METADATA_SQLITE: &str = include_str!("../../../../old_src/resources/metadata_sqlite.sql");

/// What `calibre.db.backend` writes into `custom_columns` for each column.
pub struct CustomColumn {
    pub label: &'static str,
    pub name: &'static str,
    pub datatype: &'static str,
    pub is_multiple: bool,
    pub display: &'static str,
}

pub const CUSTOM_COLUMNS: &[CustomColumn] = &[
    CustomColumn { label: "genre", name: "Genre", datatype: "text", is_multiple: true, display: r#"{"is_names": false}"# },
    CustomColumn { label: "notes", name: "Notes", datatype: "comments", is_multiple: false, display: r#"{"interpret_as": "html"}"# },
    CustomColumn { label: "saga", name: "Saga", datatype: "series", is_multiple: false, display: "{}" },
    CustomColumn {
        label: "mood",
        name: "Mood",
        datatype: "enumeration",
        is_multiple: false,
        display: r#"{"enum_values": ["Happy", "Sad", "Tense"], "enum_colors": [], "use_decorations": false}"#,
    },
    CustomColumn { label: "pages", name: "Pages", datatype: "int", is_multiple: false, display: r#"{"number_format": null}"# },
    CustomColumn { label: "score", name: "Score", datatype: "float", is_multiple: false, display: r#"{"number_format": null}"# },
    CustomColumn { label: "stars", name: "Stars", datatype: "rating", is_multiple: false, display: r#"{"allow_half_ratings": false}"# },
    CustomColumn { label: "read", name: "Read", datatype: "bool", is_multiple: false, display: "{}" },
    CustomColumn { label: "finished", name: "Finished", datatype: "datetime", is_multiple: false, display: r#"{"date_format": null}"# },
];

/// One book of the fixture, and the values every test asserts against.
pub struct FixtureBook {
    pub id: i32,
    pub title: &'static str,
    pub sort: &'static str,
    pub authors: &'static [(&'static str, &'static str)], // (name, sort)
    pub author_sort: &'static str,
    pub tags: &'static [&'static str],
    pub series: Option<(&'static str, f64)>,
    pub publisher: Option<&'static str>,
    pub identifiers: &'static [(&'static str, &'static str)],
    pub languages: &'static [&'static str],
    pub comments: Option<&'static str>,
    pub rating: Option<i32>,
    pub has_cover: bool,
    pub formats: &'static [&'static str],
    pub timestamp: &'static str,
    pub pubdate: &'static str,
    pub custom: Custom,
}

/// Custom column values, by label.
pub struct Custom {
    pub genre: &'static [&'static str],
    pub notes: Option<&'static str>,
    pub saga: Option<(&'static str, f64)>,
    pub mood: Option<&'static str>,
    pub pages: Option<i64>,
    pub score: Option<f64>,
    pub stars: Option<i64>,
    pub read: Option<bool>,
    pub finished: Option<&'static str>,
}

pub const BOOKS: &[FixtureBook] = &[
    FixtureBook {
        id: 1,
        title: "The Hobbit",
        sort: "Hobbit, The",
        authors: &[("J. R. R. Tolkien", "Tolkien, J. R. R.")],
        author_sort: "Tolkien, J. R. R.",
        tags: &["Fantasy", "Adventure"],
        series: Some(("Middle-earth", 1.0)),
        publisher: Some("George Allen & Unwin"),
        identifiers: &[("isbn", "9780261102217")],
        languages: &["eng"],
        comments: Some("<p>Bilbo goes there and back again.</p>"),
        rating: Some(10),
        has_cover: true,
        formats: &["EPUB", "TXT"],
        timestamp: "2021-03-04 05:06:07+00:00",
        pubdate: "1937-09-21 00:00:00+00:00",
        custom: Custom {
            genre: &["High Fantasy", "Classic"],
            notes: Some("<p>Re-read every winter.</p>"),
            saga: Some(("Tolkien Legendarium", 1.0)),
            mood: Some("Happy"),
            pages: Some(310),
            score: Some(9.5),
            stars: Some(10),
            read: Some(true),
            finished: Some("2022-01-15 00:00:00+00:00"),
        },
    },
    FixtureBook {
        id: 2,
        title: "The Fellowship of the Ring",
        sort: "Fellowship of the Ring, The",
        authors: &[("J. R. R. Tolkien", "Tolkien, J. R. R.")],
        author_sort: "Tolkien, J. R. R.",
        tags: &["Fantasy"],
        series: Some(("Middle-earth", 2.0)),
        publisher: Some("George Allen & Unwin"),
        identifiers: &[("isbn", "9780261102354")],
        languages: &["eng"],
        comments: None,
        rating: Some(8),
        has_cover: true,
        formats: &["EPUB"],
        timestamp: "2021-03-05 05:06:07+00:00",
        pubdate: "1954-07-29 00:00:00+00:00",
        custom: Custom {
            genre: &["High Fantasy"],
            notes: None,
            saga: Some(("Tolkien Legendarium", 2.5)),
            mood: Some("Tense"),
            pages: Some(423),
            score: Some(8.25),
            stars: Some(8),
            read: Some(false),
            finished: None,
        },
    },
    FixtureBook {
        id: 3,
        title: "Good Omens",
        sort: "Good Omens",
        authors: &[("Terry Pratchett", "Pratchett, Terry"), ("Neil Gaiman", "Gaiman, Neil")],
        author_sort: "Pratchett, Terry & Gaiman, Neil",
        tags: &["Fantasy", "Humor"],
        series: None,
        publisher: Some("Gollancz"),
        identifiers: &[("isbn", "9780575048003"), ("goodreads", "12067")],
        languages: &["eng", "fra"],
        comments: Some("The nice and accurate prophecies of Agnes Nutter."),
        rating: Some(6),
        has_cover: true,
        formats: &["EPUB"],
        timestamp: "2021-04-01 00:00:00+00:00",
        pubdate: "1990-05-01 00:00:00+00:00",
        custom: Custom {
            genre: &["Comic Fantasy", "Classic"],
            notes: None,
            saga: None,
            mood: Some("Sad"),
            pages: Some(0),
            score: Some(0.0),
            stars: None,
            read: None,
            finished: Some("2023-06-30 12:00:00+00:00"),
        },
    },
    // Nothing optional: no cover, no series, no tags, no custom values.
    FixtureBook {
        id: 4,
        title: "Bare Bones",
        sort: "Bare Bones",
        authors: &[("Anon Ymous", "Ymous, Anon")],
        author_sort: "Ymous, Anon",
        tags: &[],
        series: None,
        publisher: None,
        identifiers: &[],
        languages: &[],
        comments: None,
        rating: None,
        has_cover: false,
        formats: &["EPUB"],
        timestamp: "2021-05-01 00:00:00+00:00",
        pubdate: "2000-01-01 00:00:00+00:00",
        custom: Custom { genre: &[], notes: None, saga: None, mood: None, pages: None, score: None, stars: None, read: None, finished: None },
    },
];

impl FixtureBook {
    /// `Author/Title (id)`, relative to the library root, as calibre names it.
    pub fn rel_dir(&self) -> String {
        format!("{}/{} ({})", self.authors[0].0.trim_end_matches('.'), self.title, self.id)
    }

    /// `Title - Author`, the stem of every format file.
    pub fn file_stem(&self) -> String {
        format!("{} - {}", self.title, self.authors[0].0)
    }

    pub fn format_bytes(&self, format: &str) -> Vec<u8> {
        format!("fixture {} payload of {}\n", format, self.title).into_bytes()
    }

    pub fn cover_bytes(&self) -> Vec<u8> {
        // Not a decodable picture; nothing here decodes it.
        let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
        v.extend_from_slice(format!("cover of {}", self.title).as_bytes());
        v
    }

    pub fn format_path(&self, root: &Path, format: &str) -> PathBuf {
        root.join(self.rel_dir()).join(format!("{}.{}", self.file_stem(), format.to_lowercase()))
    }
}

/// `calibre.ebooks.metadata.title_sort`, near enough for the fixture's titles.
fn title_sort(title: &str) -> String {
    for article in ["The ", "A ", "An "] {
        if let Some(rest) = title.strip_prefix(article) {
            return format!("{}, {}", rest, article.trim());
        }
    }
    title.to_string()
}

/// The SQL functions calibre's Python registers on every connection,
/// which the schema's triggers and views call.
fn register_calibre_functions(conn: &Connection) {
    conn.create_scalar_function("title_sort", 1, FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC, |ctx| {
        Ok(title_sort(&ctx.get::<String>(0)?))
    })
    .unwrap();
    conn.create_scalar_function("uuid4", 0, FunctionFlags::SQLITE_UTF8, |_| Ok(uuid_like())).unwrap();
    conn.create_scalar_function("books_list_filter", 1, FunctionFlags::SQLITE_UTF8, |_| Ok(true)).unwrap();
}

fn uuid_like() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    let n = N.fetch_add(1, Ordering::SeqCst);
    format!("00000000-0000-4000-8000-{n:012x}")
}

/// `Backend.create_custom_column`, transcribed. Returns the column number.
pub fn create_custom_column(conn: &Connection, c: &CustomColumn) -> i64 {
    let normalized = !matches!(c.datatype, "datetime" | "comments" | "int" | "bool" | "float" | "composite");
    let is_multiple = c.is_multiple && matches!(c.datatype, "text" | "composite");
    conn.execute(
        "INSERT INTO custom_columns(label,name,datatype,is_multiple,editable,display,normalized) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![c.label, c.name, c.datatype, is_multiple, true, c.display, normalized],
    )
    .unwrap();
    let num = conn.last_insert_rowid();

    let dt = match c.datatype {
        "rating" | "int" => "INT",
        "text" | "comments" | "series" | "composite" | "enumeration" => "TEXT",
        "float" => "REAL",
        "datetime" => "timestamp",
        "bool" => "BOOL",
        other => panic!("unsupported datatype {other}"),
    };
    let collate = if dt == "TEXT" { "COLLATE NOCASE" } else { "" };
    let table = format!("custom_column_{num}");
    let lt = format!("books_custom_column_{num}_link");

    let script = if normalized {
        let s_index = if c.datatype == "series" { "extra REAL," } else { "" };
        format!(
            r#"
CREATE TABLE {table}(
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    value {dt} NOT NULL {collate},
    link TEXT NOT NULL DEFAULT "",
    UNIQUE(value));
CREATE INDEX {table}_idx ON {table} (value {collate});
CREATE TABLE {lt}(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    book INTEGER NOT NULL,
    value INTEGER NOT NULL,
    {s_index}
    UNIQUE(book, value)
    );
CREATE INDEX {lt}_aidx ON {lt} (value);
CREATE INDEX {lt}_bidx ON {lt} (book);
CREATE TRIGGER fkc_update_{lt}_a
        BEFORE UPDATE OF book ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
CREATE TRIGGER fkc_update_{lt}_b
        BEFORE UPDATE OF author ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from {table} WHERE id=NEW.value) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: value not in {table}')
            END;
        END;
CREATE TRIGGER fkc_insert_{lt}
        BEFORE INSERT ON {lt}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                WHEN (SELECT id from {table} WHERE id=NEW.value) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: value not in {table}')
            END;
        END;
CREATE TRIGGER fkc_delete_{lt}
        AFTER DELETE ON {table}
        BEGIN
            DELETE FROM {lt} WHERE value=OLD.id;
        END;
CREATE VIEW tag_browser_{table} AS SELECT
    id,
    value,
    (SELECT COUNT(id) FROM {lt} WHERE value={table}.id) count,
    (SELECT AVG(r.rating)
     FROM {lt},
          books_ratings_link as bl,
          ratings as r
     WHERE {lt}.value={table}.id and bl.book={lt}.book and
           r.id = bl.rating and r.rating <> 0) avg_rating,
    value AS sort
FROM {table};
CREATE VIEW tag_browser_filtered_{table} AS SELECT
    id,
    value,
    (SELECT COUNT({lt}.id) FROM {lt} WHERE value={table}.id AND
    books_list_filter(book)) count,
    (SELECT AVG(r.rating)
     FROM {lt},
          books_ratings_link as bl,
          ratings as r
     WHERE {lt}.value={table}.id AND bl.book={lt}.book AND
           r.id = bl.rating AND r.rating <> 0 AND
           books_list_filter(bl.book)) avg_rating,
    value AS sort
FROM {table};
"#
        )
    } else {
        format!(
            r#"
CREATE TABLE {table}(
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    book  INTEGER,
    value {dt} NOT NULL {collate},
    UNIQUE(book));
CREATE INDEX {table}_idx ON {table} (book);
CREATE TRIGGER fkc_insert_{table}
        BEFORE INSERT ON {table}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
CREATE TRIGGER fkc_update_{table}
        BEFORE UPDATE OF book ON {table}
        BEGIN
            SELECT CASE
                WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                THEN RAISE(ABORT, 'Foreign key violation: book not in books')
            END;
        END;
"#
        )
    };
    conn.execute_batch(&script).unwrap();
    num
}

/// Get-or-create a row in a name-keyed table, returning its id.
fn intern(conn: &Connection, table: &str, column: &str, value: &str) -> i64 {
    conn.execute(&format!("INSERT OR IGNORE INTO {table}({column}) VALUES (?1)"), [value]).unwrap();
    conn.query_row(&format!("SELECT id FROM {table} WHERE {column} = ?1"), [value], |r| r.get(0)).unwrap()
}

/// The fixture on disk. Dropping it deletes the library.
pub struct CalibreFixture {
    pub dir: tempfile::TempDir,
}

impl CalibreFixture {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }
}

fn opf(book: &FixtureBook) -> String {
    let creators: String = book.authors.iter().map(|(n, s)| format!("    <dc:creator opf:role=\"aut\" opf:file-as=\"{s}\">{n}</dc:creator>\n")).collect();
    format!(
        "<?xml version='1.0' encoding='utf-8'?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\">\n  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n    <dc:title>{}</dc:title>\n{creators}  </metadata>\n</package>\n",
        book.title
    )
}

fn is_normalized(c: &CustomColumn) -> bool {
    !matches!(c.datatype, "datetime" | "comments" | "int" | "bool" | "float" | "composite")
}

/// A library with calibre's schema and the fixture's custom columns, but
/// no books -- what calibre has just after "New library" plus
/// "Add custom column" a few times.
pub fn build_empty_calibre_library() -> CalibreFixture {
    build_empty_calibre_library_with(false)
}

/// `plain_only` leaves out the normalized columns (those with a link
/// table: text-multiple, series, enumeration, rating).
pub fn build_empty_calibre_library_with(plain_only: bool) -> CalibreFixture {
    let dir = tempfile::tempdir().unwrap();
    let conn = Connection::open(dir.path().join("metadata.db")).unwrap();
    register_calibre_functions(&conn);
    // calibre's `Connection.executescript(metadata_sqlite)`.
    conn.execute_batch(METADATA_SQLITE).unwrap();
    conn.execute("INSERT INTO library_id(uuid) VALUES (?1)", [uuid_like()]).unwrap();
    for c in CUSTOM_COLUMNS.iter().filter(|c| !(plain_only && is_normalized(c))) {
        create_custom_column(&conn, c);
    }
    drop(conn);
    CalibreFixture { dir }
}

/// Builds the whole library: schema, custom columns, rows, files.
pub fn build_calibre_library() -> CalibreFixture {
    build_calibre_library_with(false)
}

pub fn build_calibre_library_with(plain_only: bool) -> CalibreFixture {
    let fixture = build_empty_calibre_library_with(plain_only);
    let root = fixture.root();
    let conn = Connection::open(root.join("metadata.db")).unwrap();
    register_calibre_functions(&conn);

    let col = |label: &str| -> i64 { conn.query_row("SELECT id FROM custom_columns WHERE label=?1", [label], |r| r.get(0)).unwrap_or(0) };

    for b in BOOKS {
        conn.execute(
            "INSERT INTO books(id, title, sort, author_sort, series_index, path, has_cover, timestamp, pubdate, last_modified) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?8)",
            params![b.id, b.title, b.sort, b.author_sort, b.series.map(|s| s.1).unwrap_or(1.0), b.rel_dir(), b.has_cover, b.timestamp, b.pubdate],
        )
        .unwrap();

        for (name, sort) in b.authors {
            conn.execute("INSERT OR IGNORE INTO authors(name, sort) VALUES (?1, ?2)", [name, sort]).unwrap();
            let aid: i64 = conn.query_row("SELECT id FROM authors WHERE name=?1", [name], |r| r.get(0)).unwrap();
            conn.execute("INSERT INTO books_authors_link(book, author) VALUES (?1, ?2)", params![b.id, aid]).unwrap();
        }
        for t in b.tags {
            let tid = intern(&conn, "tags", "name", t);
            conn.execute("INSERT INTO books_tags_link(book, tag) VALUES (?1, ?2)", params![b.id, tid]).unwrap();
        }
        if let Some((s, _)) = b.series {
            let sid = intern(&conn, "series", "name", s);
            conn.execute("INSERT INTO books_series_link(book, series) VALUES (?1, ?2)", params![b.id, sid]).unwrap();
        }
        if let Some(p) = b.publisher {
            let pid = intern(&conn, "publishers", "name", p);
            conn.execute("INSERT INTO books_publishers_link(book, publisher) VALUES (?1, ?2)", params![b.id, pid]).unwrap();
        }
        for (t, v) in b.identifiers {
            conn.execute("INSERT INTO identifiers(book, type, val) VALUES (?1, ?2, ?3)", params![b.id, t, v]).unwrap();
        }
        for (order, l) in b.languages.iter().enumerate() {
            let lid = intern(&conn, "languages", "lang_code", l);
            conn.execute("INSERT INTO books_languages_link(book, lang_code, item_order) VALUES (?1, ?2, ?3)", params![b.id, lid, order as i64]).unwrap();
        }
        if let Some(c) = b.comments {
            conn.execute("INSERT INTO comments(book, text) VALUES (?1, ?2)", params![b.id, c]).unwrap();
        }
        if let Some(r) = b.rating {
            let rid = intern(&conn, "ratings", "rating", &r.to_string());
            conn.execute("INSERT INTO books_ratings_link(book, rating) VALUES (?1, ?2)", params![b.id, rid]).unwrap();
        }

        // Files.
        let book_dir = root.join(b.rel_dir());
        std::fs::create_dir_all(&book_dir).unwrap();
        for f in b.formats {
            let bytes = b.format_bytes(f);
            std::fs::write(b.format_path(root, f), &bytes).unwrap();
            conn.execute(
                "INSERT INTO data(book, format, uncompressed_size, name) VALUES (?1, ?2, ?3, ?4)",
                params![b.id, f, bytes.len() as i64, b.file_stem()],
            )
            .unwrap();
        }
        if b.has_cover {
            std::fs::write(book_dir.join("cover.jpg"), b.cover_bytes()).unwrap();
        }
        std::fs::write(book_dir.join("metadata.opf"), opf(b)).unwrap();

        // Custom column values.
        let cv = &b.custom;
        let link = |label: &str| format!("books_custom_column_{}_link", col(label));
        let table = |label: &str| format!("custom_column_{}", col(label));
        for g in cv.genre.iter().filter(|_| !plain_only) {
            let vid = intern(&conn, &table("genre"), "value", g);
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", link("genre")), params![b.id, vid]).unwrap();
        }
        if let Some(n) = cv.notes {
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", table("notes")), params![b.id, n]).unwrap();
        }
        if let Some((s, extra)) = cv.saga.filter(|_| !plain_only) {
            let vid = intern(&conn, &table("saga"), "value", s);
            conn.execute(&format!("INSERT INTO {}(book, value, extra) VALUES (?1, ?2, ?3)", link("saga")), params![b.id, vid, extra]).unwrap();
        }
        if let Some(m) = cv.mood.filter(|_| !plain_only) {
            let vid = intern(&conn, &table("mood"), "value", m);
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", link("mood")), params![b.id, vid]).unwrap();
        }
        if let Some(p) = cv.pages {
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", table("pages")), params![b.id, p]).unwrap();
        }
        if let Some(s) = cv.score {
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", table("score")), params![b.id, s]).unwrap();
        }
        if let Some(s) = cv.stars.filter(|_| !plain_only) {
            let vid = intern(&conn, &table("stars"), "value", &s.to_string());
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", link("stars")), params![b.id, vid]).unwrap();
        }
        if let Some(r) = cv.read {
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", table("read")), params![b.id, r]).unwrap();
        }
        if let Some(d) = cv.finished {
            conn.execute(&format!("INSERT INTO {}(book, value) VALUES (?1, ?2)", table("finished")), params![b.id, d]).unwrap();
        }
    }
    drop(conn);
    fixture
}

/// Every table and view definition plus every row, as stable text, so two
/// states of a `metadata.db` can be compared. Opens its own connection.
pub fn dump_database(root: &Path) -> Vec<String> {
    let conn = Connection::open(root.join("metadata.db")).unwrap();
    register_calibre_functions(&conn);
    let mut out = Vec::new();
    let mut stmt = conn.prepare("SELECT type, name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name").unwrap();
    let objects: Vec<(String, String, Option<String>)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap().map(|r| r.unwrap()).collect();
    for (ty, name, sql) in objects {
        out.push(format!("{ty} {name}: {}", sql.unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ")));
        // Views and virtual-table shadow tables carry nothing worth dumping.
        if ty == "table" && !name.contains("_fts") {
            let mut s = conn.prepare(&format!("SELECT * FROM \"{name}\" ORDER BY 1")).unwrap();
            let n = s.column_count();
            let rows = s
                .query_map([], |r| {
                    let mut cells = Vec::new();
                    for i in 0..n {
                        cells.push(format!("{:?}", r.get_ref(i)?));
                    }
                    Ok(cells.join("|"))
                })
                .unwrap();
            for row in rows {
                out.push(format!("  {name}: {}", row.unwrap()));
            }
        }
    }
    out
}
