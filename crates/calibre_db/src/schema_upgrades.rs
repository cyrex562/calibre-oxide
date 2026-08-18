//! Port of `old_src/src/calibre/db/schema_upgrades.py`'s `SchemaUpgrade`
//! (issue #201's schema-upgrade gap, following #203/#204).
//!
//! Upstream drives this by repeatedly looking up
//! `getattr(self, f'upgrade_version_{uv}', None)` where `uv` is the
//! current `PRAGMA user_version`, running it, bumping the version, and
//! looping until no such method exists (currently: versions 1 through
//! 25, landing on 26 -- the same version the bundled
//! `metadata_sqlite.sql` a brand-new library gets directly). This only
//! matters for a library created by an *older* calibre version;
//! [`crate::backend::Backend::new`] only calls this after the
//! `user_version == 0` brand-new-library path, so the two paths never
//! overlap. [`upgrade_to_latest`] mirrors the same
//! `BEGIN EXCLUSIVE TRANSACTION` / dispatch-by-name / `COMMIT` or
//! `ROLLBACK` structure, using a match on the version number in place
//! of `getattr`.
//!
//! # What's simplified: `field_metadata`-driven tag-browser views
//!
//! `upgrade_version_10`/`upgrade_version_11` build `tag_browser_*`
//! views for every *categorized* field by iterating live
//! `field_metadata` entries, which would include custom columns.
//! Custom columns aren't ported yet (tracked separately), so this
//! ports the *effect* for the fixed set of built-in categorized
//! fields (`authors`, `tags`, `publishers`, `series`, `ratings`) using
//! the exact same view SQL the bundled `metadata_sqlite.sql` uses
//! (verified identical, not re-derived) instead of a general
//! `field_metadata` walk. A library with custom columns being
//! migrated through this code won't get `tag_browser_custom_column_*`
//! views -- consistent with custom columns not being supported yet
//! anywhere else in this crate either.
//!
//! # Preserved upstream inconsistency: no `tag_browser_ratings` gap
//!
//! Confirmed by reading every `upgrade_version_N`: none of them create
//! `tag_browser_ratings`/`tag_browser_filtered_ratings` as a *named,
//! literal* step -- they come from the same `field_metadata`-driven
//! loop as the other categorized fields (`ratings` has `is_category`
//! and a `link_column`, same shape as `authors`/`tags`/etc.), which is
//! why this port includes `ratings` in the fixed set above rather than
//! treating it as a special case.
//!
//! # Not ported: `upgrade_version_19`, `upgrade_version_24`
//!
//! - `upgrade_version_19` migrates the RSS "custom recipes" stored in
//!   the `feeds` table out to files on disk, via
//!   `calibre.web.feeds.recipes` -- calibre-oxide has no recipes/feeds
//!   subsystem at all yet. This step makes no schema or data changes
//!   (it only writes external files), so skipping its body while still
//!   advancing `user_version` doesn't desync the schema from later
//!   steps -- it just means recipes in an upgraded library's `feeds`
//!   table are left as rows instead of being exported to files, which
//!   is a real gap if/when recipes are ever ported here.
//! - `upgrade_version_24` calls `self.db.reindex_annotations()`, an
//!   FTS-pipeline method not ported yet (`db/fts/*` is its own
//!   large gap per #201). No schema change either; same trade-off.

use rusqlite::{Connection, Result};
use std::path::Path;

pub struct SchemaUpgrade;

impl SchemaUpgrade {
    /// Runs every `upgrade_version_N` step needed to bring `conn` from
    /// its current `PRAGMA user_version` up to the latest (26), in one
    /// transaction -- matching upstream's `BEGIN EXCLUSIVE
    /// TRANSACTION` / per-step `PRAGMA user_version=uv+1` / `COMMIT`
    /// (or `ROLLBACK` on any failure).
    pub fn upgrade_to_latest(conn: &mut Connection, library_path: &Path) -> Result<()> {
        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION")?;
        let result = Self::run_steps(conn, library_path);
        match result {
            Ok(()) => conn.execute_batch("COMMIT")?,
            Err(_) => {
                conn.execute_batch("ROLLBACK").ok();
                return result;
            }
        }
        Ok(())
    }

    fn run_steps(conn: &Connection, library_path: &Path) -> Result<()> {
        loop {
            let uv: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let step: fn(&Connection, &Path) -> Result<()> = match uv {
                1 => upgrade_version_1,
                2 => upgrade_version_2,
                3 => upgrade_version_3,
                4 => upgrade_version_4,
                5 => upgrade_version_5,
                6 => upgrade_version_6,
                7 => upgrade_version_7,
                8 => upgrade_version_8,
                9 => upgrade_version_9,
                10 => upgrade_version_10,
                11 => upgrade_version_11,
                12 => upgrade_version_12,
                13 => upgrade_version_13,
                14 => upgrade_version_14,
                15 => upgrade_version_15,
                16 => upgrade_version_16,
                17 => upgrade_version_17,
                18 => upgrade_version_18,
                19 => upgrade_version_19,
                20 => upgrade_version_20,
                21 => upgrade_version_21,
                22 => upgrade_version_22,
                23 => upgrade_version_23,
                24 => upgrade_version_24,
                25 => upgrade_version_25,
                _ => return Ok(()),
            };
            step(conn, library_path)?;
            conn.execute_batch(&format!("PRAGMA user_version={}", uv + 1))?;
        }
    }
}

fn upgrade_version_1(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP INDEX IF EXISTS authors_idx;
         CREATE INDEX authors_idx ON books (author_sort COLLATE NOCASE, sort COLLATE NOCASE);
         DROP INDEX IF EXISTS series_idx;
         CREATE INDEX series_idx ON series (name COLLATE NOCASE);
         DROP INDEX IF EXISTS series_sort_idx;
         CREATE INDEX series_sort_idx ON books (series_index, id);",
    )
}

fn upgrade_version_2(conn: &Connection, _library_path: &Path) -> Result<()> {
    fn fkc_script(ltable: &str, table: &str, ltable_col: &str) -> String {
        format!(
            "DROP TRIGGER IF EXISTS fkc_delete_books_{ltable}_link;
             CREATE TRIGGER fkc_delete_on_{table}
             BEFORE DELETE ON {table}
             BEGIN
                 SELECT CASE
                     WHEN (SELECT COUNT(id) FROM books_{ltable}_link WHERE {ltable_col}=OLD.id) > 0
                     THEN RAISE(ABORT, 'Foreign key violation: {table} is still referenced')
                 END;
             END;
             DELETE FROM {table} WHERE (SELECT COUNT(id) FROM books_{ltable}_link WHERE {ltable_col}={table}.id) < 1;"
        )
    }
    conn.execute_batch(&fkc_script("authors", "authors", "author"))?;
    conn.execute_batch(&fkc_script("publishers", "publishers", "publisher"))?;
    conn.execute_batch(&fkc_script("tags", "tags", "tag"))?;
    conn.execute_batch(&fkc_script("series", "series", "series"))?;
    Ok(())
}

fn upgrade_version_3(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP VIEW IF EXISTS meta;
         CREATE VIEW meta AS
         SELECT id, title,
                (SELECT concat(name) FROM authors WHERE authors.id IN (SELECT author from books_authors_link WHERE book=books.id)) authors,
                (SELECT name FROM publishers WHERE publishers.id IN (SELECT publisher from books_publishers_link WHERE book=books.id)) publisher,
                (SELECT rating FROM ratings WHERE ratings.id IN (SELECT rating from books_ratings_link WHERE book=books.id)) rating,
                timestamp,
                (SELECT MAX(uncompressed_size) FROM data WHERE book=books.id) size,
                (SELECT concat(name) FROM tags WHERE tags.id IN (SELECT tag from books_tags_link WHERE book=books.id)) tags,
                (SELECT text FROM comments WHERE book=books.id) comments,
                (SELECT name FROM series WHERE series.id IN (SELECT series FROM books_series_link WHERE book=books.id)) series,
                series_index,
                sort,
                author_sort,
                (SELECT concat(format) FROM data WHERE data.book=books.id) formats,
                isbn,
                path
         FROM books;",
    )
}

fn upgrade_version_4(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE TEMPORARY TABLE books_backup(id,title,sort,timestamp,series_index,author_sort,isbn,path);
         INSERT INTO books_backup SELECT id,title,sort,timestamp,series_index,author_sort,isbn,path FROM books;
         DROP TABLE books;
         CREATE TABLE books ( id      INTEGER PRIMARY KEY AUTOINCREMENT,
                              title     TEXT NOT NULL DEFAULT 'Unknown' COLLATE NOCASE,
                              sort      TEXT COLLATE NOCASE,
                              timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                              pubdate   TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                              series_index REAL NOT NULL DEFAULT 1.0,
                              author_sort TEXT COLLATE NOCASE,
                              isbn TEXT DEFAULT '' COLLATE NOCASE,
                              lccn TEXT DEFAULT '' COLLATE NOCASE,
                              path TEXT NOT NULL DEFAULT '',
                              flags INTEGER NOT NULL DEFAULT 1
                         );
         INSERT INTO
             books (id,title,sort,timestamp,pubdate,series_index,author_sort,isbn,path)
             SELECT id,title,sort,timestamp,timestamp,series_index,author_sort,isbn,path FROM books_backup;
         DROP TABLE books_backup;

         DROP VIEW IF EXISTS meta;
         CREATE VIEW meta AS
         SELECT id, title,
                (SELECT concat(name) FROM authors WHERE authors.id IN (SELECT author from books_authors_link WHERE book=books.id)) authors,
                (SELECT name FROM publishers WHERE publishers.id IN (SELECT publisher from books_publishers_link WHERE book=books.id)) publisher,
                (SELECT rating FROM ratings WHERE ratings.id IN (SELECT rating from books_ratings_link WHERE book=books.id)) rating,
                timestamp,
                (SELECT MAX(uncompressed_size) FROM data WHERE book=books.id) size,
                (SELECT concat(name) FROM tags WHERE tags.id IN (SELECT tag from books_tags_link WHERE book=books.id)) tags,
                (SELECT text FROM comments WHERE book=books.id) comments,
                (SELECT name FROM series WHERE series.id IN (SELECT series FROM books_series_link WHERE book=books.id)) series,
                series_index,
                sort,
                author_sort,
                (SELECT concat(format) FROM data WHERE data.book=books.id) formats,
                isbn,
                path,
                lccn,
                pubdate,
                flags
         FROM books;",
    )
}

fn upgrade_version_5(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX authors_idx ON books (author_sort COLLATE NOCASE);
         CREATE INDEX books_idx ON books (sort COLLATE NOCASE);
         CREATE TRIGGER books_delete_trg
             AFTER DELETE ON books
             BEGIN
                 DELETE FROM books_authors_link WHERE book=OLD.id;
                 DELETE FROM books_publishers_link WHERE book=OLD.id;
                 DELETE FROM books_ratings_link WHERE book=OLD.id;
                 DELETE FROM books_series_link WHERE book=OLD.id;
                 DELETE FROM books_tags_link WHERE book=OLD.id;
                 DELETE FROM data WHERE book=OLD.id;
                 DELETE FROM comments WHERE book=OLD.id;
                 DELETE FROM conversion_options WHERE book=OLD.id;
         END;
         CREATE TRIGGER books_insert_trg
             AFTER INSERT ON books
             BEGIN
             UPDATE books SET sort=title_sort(NEW.title) WHERE id=NEW.id;
         END;
         CREATE TRIGGER books_update_trg
             AFTER UPDATE ON books
             BEGIN
             UPDATE books SET sort=title_sort(NEW.title) WHERE id=NEW.id;
         END;

         UPDATE books SET sort=title_sort(title) WHERE sort IS NULL;",
    )
}

fn upgrade_version_6(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP VIEW IF EXISTS meta;
         CREATE VIEW meta AS
         SELECT id, title,
                (SELECT sortconcat(bal.id, name) FROM books_authors_link AS bal JOIN authors ON(author = authors.id) WHERE book = books.id) authors,
                (SELECT name FROM publishers WHERE publishers.id IN (SELECT publisher from books_publishers_link WHERE book=books.id)) publisher,
                (SELECT rating FROM ratings WHERE ratings.id IN (SELECT rating from books_ratings_link WHERE book=books.id)) rating,
                timestamp,
                (SELECT MAX(uncompressed_size) FROM data WHERE book=books.id) size,
                (SELECT concat(name) FROM tags WHERE tags.id IN (SELECT tag from books_tags_link WHERE book=books.id)) tags,
                (SELECT text FROM comments WHERE book=books.id) comments,
                (SELECT name FROM series WHERE series.id IN (SELECT series FROM books_series_link WHERE book=books.id)) series,
                series_index,
                sort,
                author_sort,
                (SELECT concat(format) FROM data WHERE data.book=books.id) formats,
                isbn,
                path,
                lccn,
                pubdate,
                flags
         FROM books;",
    )
}

fn upgrade_version_7(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "ALTER TABLE books ADD COLUMN uuid TEXT;
         DROP TRIGGER IF EXISTS books_insert_trg;
         DROP TRIGGER IF EXISTS books_update_trg;
         UPDATE books SET uuid=uuid4();

         CREATE TRIGGER books_insert_trg AFTER INSERT ON books
         BEGIN
             UPDATE books SET sort=title_sort(NEW.title),uuid=uuid4() WHERE id=NEW.id;
         END;

         CREATE TRIGGER books_update_trg AFTER UPDATE ON books
         BEGIN
             UPDATE books SET sort=title_sort(NEW.title) WHERE id=NEW.id;
         END;

         DROP VIEW IF EXISTS meta;
         CREATE VIEW meta AS
         SELECT id, title,
                (SELECT sortconcat(bal.id, name) FROM books_authors_link AS bal JOIN authors ON(author = authors.id) WHERE book = books.id) authors,
                (SELECT name FROM publishers WHERE publishers.id IN (SELECT publisher from books_publishers_link WHERE book=books.id)) publisher,
                (SELECT rating FROM ratings WHERE ratings.id IN (SELECT rating from books_ratings_link WHERE book=books.id)) rating,
                timestamp,
                (SELECT MAX(uncompressed_size) FROM data WHERE book=books.id) size,
                (SELECT concat(name) FROM tags WHERE tags.id IN (SELECT tag from books_tags_link WHERE book=books.id)) tags,
                (SELECT text FROM comments WHERE book=books.id) comments,
                (SELECT name FROM series WHERE series.id IN (SELECT series FROM books_series_link WHERE book=books.id)) series,
                series_index,
                sort,
                author_sort,
                (SELECT concat(format) FROM data WHERE data.book=books.id) formats,
                isbn,
                path,
                lccn,
                pubdate,
                flags,
                uuid
         FROM books;",
    )
}

fn upgrade_version_8(conn: &Connection, _library_path: &Path) -> Result<()> {
    fn create_tag_browser_view(
        conn: &Connection,
        table_name: &str,
        column_name: &str,
    ) -> Result<()> {
        conn.execute_batch(&format!(
            "DROP VIEW IF EXISTS tag_browser_{table_name};
             CREATE VIEW tag_browser_{table_name} AS SELECT
                 id,
                 name,
                 (SELECT COUNT(id) FROM books_{table_name}_link WHERE {column_name}={table_name}.id) count
             FROM {table_name};"
        ))
    }
    create_tag_browser_view(conn, "authors", "author")?;
    create_tag_browser_view(conn, "tags", "tag")?;
    create_tag_browser_view(conn, "publishers", "publisher")?;
    create_tag_browser_view(conn, "series", "series")?;
    Ok(())
}

fn upgrade_version_9(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE custom_columns (
             id       INTEGER PRIMARY KEY AUTOINCREMENT,
             label    TEXT NOT NULL,
             name     TEXT NOT NULL,
             datatype TEXT NOT NULL,
             mark_for_delete   BOOL DEFAULT 0 NOT NULL,
             editable BOOL DEFAULT 1 NOT NULL,
             display  TEXT DEFAULT '{}' NOT NULL,
             is_multiple BOOL DEFAULT 0 NOT NULL,
             normalized BOOL NOT NULL,
             UNIQUE(label)
         );
         CREATE INDEX IF NOT EXISTS custom_columns_idx ON custom_columns (label);
         CREATE INDEX IF NOT EXISTS formats_idx ON data (format);",
    )
}

/// The fixed set of built-in categorized fields
/// `upgrade_version_10`/`upgrade_version_11`'s real `field_metadata`
/// walk would visit (see the module docs for why this is fixed
/// instead of data-driven, and why `ratings` is included).
const STD_CATEGORY_FIELDS: &[(&str, &str, &str)] = &[
    ("authors", "author", "name"),
    ("tags", "tag", "name"),
    ("publishers", "publisher", "name"),
    ("series", "series", "name"),
    ("ratings", "rating", "rating"),
];

fn upgrade_version_10(conn: &Connection, _library_path: &Path) -> Result<()> {
    for &(table_name, column_name, view_column_name) in STD_CATEGORY_FIELDS {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                [format!("books_{table_name}_link")],
                |row| row.get::<_, i64>(0).map(|_| true),
            )
            .unwrap_or(false);
        if !exists {
            continue;
        }
        conn.execute_batch(&format!(
            "DROP VIEW IF EXISTS tag_browser_{table_name};
             CREATE VIEW tag_browser_{table_name} AS SELECT
                 id,
                 {view_column_name},
                 (SELECT COUNT(id) FROM books_{table_name}_link WHERE {column_name}={table_name}.id) count
             FROM {table_name};
             DROP VIEW IF EXISTS tag_browser_filtered_{table_name};
             CREATE VIEW tag_browser_filtered_{table_name} AS SELECT
                 id,
                 {view_column_name},
                 (SELECT COUNT(books_{table_name}_link.id) FROM books_{table_name}_link WHERE
                     {column_name}={table_name}.id AND books_list_filter(book)) count
             FROM {table_name};"
        ))?;
    }
    Ok(())
}

fn upgrade_version_11(conn: &Connection, _library_path: &Path) -> Result<()> {
    // Verified identical to the bundled `metadata_sqlite.sql`'s final
    // `tag_browser_*`/`tag_browser_filtered_*` views for these five
    // fields -- see the module docs.
    conn.execute_batch(
        "DROP VIEW IF EXISTS tag_browser_authors;
         CREATE VIEW tag_browser_authors AS SELECT
             id, name,
             (SELECT COUNT(id) FROM books_authors_link WHERE author=authors.id) count,
             (SELECT AVG(ratings.rating) FROM books_authors_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.author=authors.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0) avg_rating,
             sort AS sort
         FROM authors;
         DROP VIEW IF EXISTS tag_browser_filtered_authors;
         CREATE VIEW tag_browser_filtered_authors AS SELECT
             id, name,
             (SELECT COUNT(books_authors_link.id) FROM books_authors_link WHERE
                 author=authors.id AND books_list_filter(book)) count,
             (SELECT AVG(ratings.rating) FROM books_authors_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.author=authors.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0 AND
              books_list_filter(bl.book)) avg_rating,
             sort AS sort
         FROM authors;

         DROP VIEW IF EXISTS tag_browser_tags;
         CREATE VIEW tag_browser_tags AS SELECT
             id, name,
             (SELECT COUNT(id) FROM books_tags_link WHERE tag=tags.id) count,
             (SELECT AVG(ratings.rating) FROM books_tags_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.tag=tags.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0) avg_rating,
             name AS sort
         FROM tags;
         DROP VIEW IF EXISTS tag_browser_filtered_tags;
         CREATE VIEW tag_browser_filtered_tags AS SELECT
             id, name,
             (SELECT COUNT(books_tags_link.id) FROM books_tags_link WHERE
                 tag=tags.id AND books_list_filter(book)) count,
             (SELECT AVG(ratings.rating) FROM books_tags_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.tag=tags.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0 AND
              books_list_filter(bl.book)) avg_rating,
             name AS sort
         FROM tags;

         DROP VIEW IF EXISTS tag_browser_publishers;
         CREATE VIEW tag_browser_publishers AS SELECT
             id, name,
             (SELECT COUNT(id) FROM books_publishers_link WHERE publisher=publishers.id) count,
             (SELECT AVG(ratings.rating) FROM books_publishers_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.publisher=publishers.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0) avg_rating,
             name AS sort
         FROM publishers;
         DROP VIEW IF EXISTS tag_browser_filtered_publishers;
         CREATE VIEW tag_browser_filtered_publishers AS SELECT
             id, name,
             (SELECT COUNT(books_publishers_link.id) FROM books_publishers_link WHERE
                 publisher=publishers.id AND books_list_filter(book)) count,
             (SELECT AVG(ratings.rating) FROM books_publishers_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.publisher=publishers.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0 AND
              books_list_filter(bl.book)) avg_rating,
             name AS sort
         FROM publishers;

         DROP VIEW IF EXISTS tag_browser_series;
         CREATE VIEW tag_browser_series AS SELECT
             id, name,
             (SELECT COUNT(id) FROM books_series_link WHERE series=series.id) count,
             (SELECT AVG(ratings.rating) FROM books_series_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.series=series.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0) avg_rating,
             (title_sort(name)) AS sort
         FROM series;
         DROP VIEW IF EXISTS tag_browser_filtered_series;
         CREATE VIEW tag_browser_filtered_series AS SELECT
             id, name,
             (SELECT COUNT(books_series_link.id) FROM books_series_link WHERE
                 series=series.id AND books_list_filter(book)) count,
             (SELECT AVG(ratings.rating) FROM books_series_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.series=series.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0 AND
              books_list_filter(bl.book)) avg_rating,
             (title_sort(name)) AS sort
         FROM series;

         DROP VIEW IF EXISTS tag_browser_ratings;
         CREATE VIEW tag_browser_ratings AS SELECT
             id, rating,
             (SELECT COUNT(id) FROM books_ratings_link WHERE rating=ratings.id) count,
             (SELECT AVG(ratings.rating) FROM books_ratings_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.rating=ratings.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0) avg_rating,
             rating AS sort
         FROM ratings;
         DROP VIEW IF EXISTS tag_browser_filtered_ratings;
         CREATE VIEW tag_browser_filtered_ratings AS SELECT
             id, rating,
             (SELECT COUNT(books_ratings_link.id) FROM books_ratings_link WHERE
                 rating=ratings.id AND books_list_filter(book)) count,
             (SELECT AVG(ratings.rating) FROM books_ratings_link AS tl, books_ratings_link AS bl, ratings
              WHERE tl.rating=ratings.id AND bl.book=tl.book AND ratings.id = bl.rating AND ratings.rating <> 0 AND
              books_list_filter(bl.book)) avg_rating,
             rating AS sort
         FROM ratings;

         UPDATE authors SET sort=author_to_author_sort(name);",
    )?;

    // Upstream also walks `sqlite_master` for `custom_column_*` tables
    // with a matching `books_custom_column_*_link` table and builds a
    // `tag_browser_custom_column_N` view for each -- skipped, no
    // custom columns exist to have such tables in the first place (see
    // module docs).
    Ok(())
}

fn upgrade_version_12(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS preferences;
         CREATE TABLE preferences(id INTEGER PRIMARY KEY,
                                  key TEXT NOT NULL,
                                  val TEXT NOT NULL,
                                  UNIQUE(key));",
    )
}

fn upgrade_version_13(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS metadata_dirtied;
         CREATE TABLE metadata_dirtied(id INTEGER PRIMARY KEY,
                              book INTEGER NOT NULL,
                              UNIQUE(book));
         INSERT INTO metadata_dirtied (book) SELECT id FROM books;",
    )
}

fn upgrade_version_14(conn: &Connection, library_path: &Path) -> Result<()> {
    conn.execute_batch("ALTER TABLE books ADD COLUMN has_cover BOOL DEFAULT 0")?;

    let mut stmt = conn.prepare("SELECT id, path FROM books")?;
    let rows: Vec<(i64, Option<String>)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_>>()?;
    drop(stmt);

    for (id, path) in rows {
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            continue;
        };
        // Python: `path.replace('/', os.sep)` -- a no-op on Linux/macOS
        // (`os.sep == '/'`), which is what this crate targets (see
        // `docs/AGENT_PORTING_GUIDE.md`).
        let cover_path = library_path.join(path).join("cover.jpg");
        if cover_path.exists() {
            conn.execute("UPDATE books SET has_cover=1 WHERE id=?", [id])?;
        }
    }
    Ok(())
}

fn upgrade_version_15(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "UPDATE OR IGNORE tags SET name=REPLACE(name, ',', ';');
         UPDATE OR IGNORE tags SET name=REPLACE(name, ',', ';;');
         UPDATE OR IGNORE tags SET name=REPLACE(name, ',', '');",
    )
}

fn upgrade_version_16(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS books_update_trg;
         CREATE TRIGGER books_update_trg
             AFTER UPDATE ON books
             BEGIN
             UPDATE books SET sort=title_sort(NEW.title)
                          WHERE id=NEW.id AND OLD.title <> NEW.title;
             END;",
    )
}

fn upgrade_version_17(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS books_plugin_data;
         CREATE TABLE books_plugin_data(id INTEGER PRIMARY KEY,
                                      book INTEGER NOT NULL,
                                      name TEXT NOT NULL,
                                      val TEXT NOT NULL,
                                      UNIQUE(book,name));
         DROP TRIGGER IF EXISTS books_delete_trg;
         CREATE TRIGGER books_delete_trg
             AFTER DELETE ON books
             BEGIN
                 DELETE FROM books_authors_link WHERE book=OLD.id;
                 DELETE FROM books_publishers_link WHERE book=OLD.id;
                 DELETE FROM books_ratings_link WHERE book=OLD.id;
                 DELETE FROM books_series_link WHERE book=OLD.id;
                 DELETE FROM books_tags_link WHERE book=OLD.id;
                 DELETE FROM data WHERE book=OLD.id;
                 DELETE FROM comments WHERE book=OLD.id;
                 DELETE FROM conversion_options WHERE book=OLD.id;
                 DELETE FROM books_plugin_data WHERE book=OLD.id;
         END;",
    )
}

fn upgrade_version_18(conn: &Connection, _library_path: &Path) -> Result<()> {
    // Upstream computes this default from `isoformat(DEFAULT_DATE, sep=' ')`
    // where `DEFAULT_DATE = datetime(2000, 1, 1, tzinfo=utc)` -- the exact
    // same literal the bundled `metadata_sqlite.sql` uses for this column's
    // default on a brand-new library, confirmed identical rather than
    // recomputed.
    conn.execute_batch(
        "DROP TABLE IF EXISTS library_id;
         CREATE TABLE library_id ( id   INTEGER PRIMARY KEY,
                                   uuid TEXT NOT NULL,
                                   UNIQUE(uuid)
         );

         DROP TABLE IF EXISTS identifiers;
         CREATE TABLE identifiers  ( id     INTEGER PRIMARY KEY,
                                     book   INTEGER NOT NULL,
                                     type   TEXT NOT NULL DEFAULT 'isbn' COLLATE NOCASE,
                                     val    TEXT NOT NULL COLLATE NOCASE,
                                     UNIQUE(book, type)
         );

         DROP TABLE IF EXISTS languages;
         CREATE TABLE languages    ( id        INTEGER PRIMARY KEY,
                                     lang_code TEXT NOT NULL COLLATE NOCASE,
                                     UNIQUE(lang_code)
         );

         DROP TABLE IF EXISTS books_languages_link;
         CREATE TABLE books_languages_link ( id INTEGER PRIMARY KEY,
                                             book INTEGER NOT NULL,
                                             lang_code INTEGER NOT NULL,
                                             item_order INTEGER NOT NULL DEFAULT 0,
                                             UNIQUE(book, lang_code)
         );

         DROP TRIGGER IF EXISTS fkc_delete_on_languages;
         CREATE TRIGGER fkc_delete_on_languages
         BEFORE DELETE ON languages
         BEGIN
             SELECT CASE
                 WHEN (SELECT COUNT(id) FROM books_languages_link WHERE lang_code=OLD.id) > 0
                 THEN RAISE(ABORT, 'Foreign key violation: language is still referenced')
             END;
         END;

         DROP TRIGGER IF EXISTS fkc_delete_on_languages_link;
         CREATE TRIGGER fkc_delete_on_languages_link
         BEFORE INSERT ON books_languages_link
         BEGIN
           SELECT CASE
               WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
               THEN RAISE(ABORT, 'Foreign key violation: book not in books')
               WHEN (SELECT id from languages WHERE id=NEW.lang_code) IS NULL
               THEN RAISE(ABORT, 'Foreign key violation: lang_code not in languages')
           END;
         END;

         DROP TRIGGER IF EXISTS fkc_update_books_languages_link_a;
         CREATE TRIGGER fkc_update_books_languages_link_a
         BEFORE UPDATE OF book ON books_languages_link
         BEGIN
             SELECT CASE
                 WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                 THEN RAISE(ABORT, 'Foreign key violation: book not in books')
             END;
         END;
         DROP TRIGGER IF EXISTS fkc_update_books_languages_link_b;
         CREATE TRIGGER fkc_update_books_languages_link_b
         BEFORE UPDATE OF lang_code ON books_languages_link
         BEGIN
             SELECT CASE
                 WHEN (SELECT id from languages WHERE id=NEW.lang_code) IS NULL
                 THEN RAISE(ABORT, 'Foreign key violation: lang_code not in languages')
             END;
         END;

         DROP INDEX IF EXISTS books_languages_link_aidx;
         CREATE INDEX books_languages_link_aidx ON books_languages_link (lang_code);
         DROP INDEX IF EXISTS books_languages_link_bidx;
         CREATE INDEX books_languages_link_bidx ON books_languages_link (book);
         DROP INDEX IF EXISTS languages_idx;
         CREATE INDEX languages_idx ON languages (lang_code COLLATE NOCASE);

         DROP TRIGGER IF EXISTS books_delete_trg;
         CREATE TRIGGER books_delete_trg
             AFTER DELETE ON books
             BEGIN
                 DELETE FROM books_authors_link WHERE book=OLD.id;
                 DELETE FROM books_publishers_link WHERE book=OLD.id;
                 DELETE FROM books_ratings_link WHERE book=OLD.id;
                 DELETE FROM books_series_link WHERE book=OLD.id;
                 DELETE FROM books_tags_link WHERE book=OLD.id;
                 DELETE FROM books_languages_link WHERE book=OLD.id;
                 DELETE FROM data WHERE book=OLD.id;
                 DELETE FROM comments WHERE book=OLD.id;
                 DELETE FROM conversion_options WHERE book=OLD.id;
                 DELETE FROM books_plugin_data WHERE book=OLD.id;
                 DELETE FROM identifiers WHERE book=OLD.id;
         END;

         INSERT INTO identifiers (book, val) SELECT id,isbn FROM books WHERE isbn;

         ALTER TABLE books ADD COLUMN last_modified TIMESTAMP NOT NULL DEFAULT '2000-01-01 00:00:00+00:00';",
    )
}

fn upgrade_version_19(_conn: &Connection, _library_path: &Path) -> Result<()> {
    // Not ported -- see the module docs. No schema/data change either
    // way, so nothing to do here beyond advancing the version, which
    // the caller handles.
    Ok(())
}

fn upgrade_version_20(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch("ALTER TABLE authors ADD COLUMN link TEXT NOT NULL DEFAULT '';")
}

fn upgrade_version_21(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TRIGGER IF EXISTS series_insert_trg;
         DROP TRIGGER IF EXISTS series_update_trg;

         UPDATE series SET sort=title_sort(name);

         CREATE TRIGGER series_insert_trg
             AFTER INSERT ON series
             BEGIN
               UPDATE series SET sort=title_sort(NEW.name) WHERE id=NEW.id;
             END;

         CREATE TRIGGER series_update_trg
             AFTER UPDATE ON series
             BEGIN
               UPDATE series SET sort=title_sort(NEW.name) WHERE id=NEW.id;
             END;",
    )
}

fn upgrade_version_22(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS last_read_positions;
         CREATE TABLE last_read_positions ( id INTEGER PRIMARY KEY,
             book INTEGER NOT NULL,
             format TEXT NOT NULL COLLATE NOCASE,
             user TEXT NOT NULL,
             device TEXT NOT NULL,
             cfi TEXT NOT NULL,
             epoch REAL NOT NULL,
             pos_frac REAL NOT NULL DEFAULT 0,
             UNIQUE(user, device, book, format)
         );
         DROP INDEX IF EXISTS lrp_idx;
         CREATE INDEX lrp_idx ON last_read_positions (book);

         DROP TRIGGER IF EXISTS books_delete_trg;
         CREATE TRIGGER books_delete_trg
             AFTER DELETE ON books
             BEGIN
                 DELETE FROM books_authors_link WHERE book=OLD.id;
                 DELETE FROM books_publishers_link WHERE book=OLD.id;
                 DELETE FROM books_ratings_link WHERE book=OLD.id;
                 DELETE FROM books_series_link WHERE book=OLD.id;
                 DELETE FROM books_tags_link WHERE book=OLD.id;
                 DELETE FROM books_languages_link WHERE book=OLD.id;
                 DELETE FROM data WHERE book=OLD.id;
                 DELETE FROM last_read_positions WHERE book=OLD.id;
                 DELETE FROM comments WHERE book=OLD.id;
                 DELETE FROM conversion_options WHERE book=OLD.id;
                 DELETE FROM books_plugin_data WHERE book=OLD.id;
                 DELETE FROM identifiers WHERE book=OLD.id;
         END;

         DROP TRIGGER IF EXISTS fkc_lrp_insert;
         DROP TRIGGER IF EXISTS fkc_lrp_update;
         CREATE TRIGGER fkc_lrp_insert
                 BEFORE INSERT ON last_read_positions
                 BEGIN
                     SELECT CASE
                         WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                         THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                     END;
                 END;
         CREATE TRIGGER fkc_lrp_update
                 BEFORE UPDATE OF book ON last_read_positions
                 BEGIN
                     SELECT CASE
                         WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                         THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                     END;
                 END;",
    )
}

fn upgrade_version_23(conn: &Connection, _library_path: &Path) -> Result<()> {
    conn.execute_batch(
        "DROP TABLE IF EXISTS annotations_dirtied;
         CREATE TABLE annotations_dirtied(id INTEGER PRIMARY KEY,
                              book INTEGER NOT NULL,
                              UNIQUE(book));
         DROP TABLE IF EXISTS annotations;
         CREATE TABLE annotations ( id INTEGER PRIMARY KEY,
             book INTEGER NOT NULL,
             format TEXT NOT NULL COLLATE NOCASE,
             user_type TEXT NOT NULL,
             user TEXT NOT NULL,
             timestamp REAL NOT NULL,
             annot_id TEXT NOT NULL,
             annot_type TEXT NOT NULL,
             annot_data TEXT NOT NULL,
             searchable_text TEXT NOT NULL DEFAULT '',
             UNIQUE(book, user_type, user, format, annot_type, annot_id)
         );

         DROP INDEX IF EXISTS annot_idx;
         CREATE INDEX annot_idx ON annotations (book);

         DROP TABLE IF EXISTS annotations_fts;
         DROP TABLE IF EXISTS annotations_fts_stemmed;
         CREATE VIRTUAL TABLE annotations_fts USING fts5(searchable_text,
             content = 'annotations', content_rowid = 'id', tokenize = 'unicode61 remove_diacritics 2');
         CREATE VIRTUAL TABLE annotations_fts_stemmed USING fts5(searchable_text,
             content = 'annotations', content_rowid = 'id', tokenize = 'porter unicode61 remove_diacritics 2');

         DROP TRIGGER IF EXISTS annotations_fts_insert_trg;
         CREATE TRIGGER annotations_fts_insert_trg AFTER INSERT ON annotations
         BEGIN
             INSERT INTO annotations_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
             INSERT INTO annotations_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
         END;

         DROP TRIGGER IF EXISTS annotations_fts_delete_trg;
         CREATE TRIGGER annotations_fts_delete_trg AFTER DELETE ON annotations
         BEGIN
             INSERT INTO annotations_fts(annotations_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
             INSERT INTO annotations_fts_stemmed(annotations_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
         END;

         DROP TRIGGER IF EXISTS annotations_fts_update_trg;
         CREATE TRIGGER annotations_fts_update_trg AFTER UPDATE ON annotations
         BEGIN
             INSERT INTO annotations_fts(annotations_fts, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
             INSERT INTO annotations_fts(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
             INSERT INTO annotations_fts_stemmed(annotations_fts_stemmed, rowid, searchable_text) VALUES('delete', OLD.id, OLD.searchable_text);
             INSERT INTO annotations_fts_stemmed(rowid, searchable_text) VALUES (NEW.id, NEW.searchable_text);
         END;

         DROP TRIGGER IF EXISTS books_delete_trg;
         CREATE TRIGGER books_delete_trg
             AFTER DELETE ON books
             BEGIN
                 DELETE FROM books_authors_link WHERE book=OLD.id;
                 DELETE FROM books_publishers_link WHERE book=OLD.id;
                 DELETE FROM books_ratings_link WHERE book=OLD.id;
                 DELETE FROM books_series_link WHERE book=OLD.id;
                 DELETE FROM books_tags_link WHERE book=OLD.id;
                 DELETE FROM books_languages_link WHERE book=OLD.id;
                 DELETE FROM data WHERE book=OLD.id;
                 DELETE FROM last_read_positions WHERE book=OLD.id;
                 DELETE FROM annotations WHERE book=OLD.id;
                 DELETE FROM comments WHERE book=OLD.id;
                 DELETE FROM conversion_options WHERE book=OLD.id;
                 DELETE FROM books_plugin_data WHERE book=OLD.id;
                 DELETE FROM identifiers WHERE book=OLD.id;
         END;

         DROP TRIGGER IF EXISTS fkc_annot_insert;
         DROP TRIGGER IF EXISTS fkc_annot_update;
         CREATE TRIGGER fkc_annot_insert
                 BEFORE INSERT ON annotations
                 BEGIN
                     SELECT CASE
                         WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                         THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                     END;
                 END;
         CREATE TRIGGER fkc_annot_update
                 BEFORE UPDATE OF book ON annotations
                 BEGIN
                     SELECT CASE
                         WHEN (SELECT id from books WHERE id=NEW.book) IS NULL
                         THEN RAISE(ABORT, 'Foreign key violation: book not in books')
                     END;
                 END;",
    )
}

fn upgrade_version_24(_conn: &Connection, _library_path: &Path) -> Result<()> {
    // Not ported -- see the module docs. No schema/data change either
    // way.
    Ok(())
}

fn upgrade_version_25(conn: &Connection, _library_path: &Path) -> Result<()> {
    // Upstream also adds a `link` column to every normalized custom
    // column's table (`custom_column_N`) -- skipped, no custom columns
    // exist to have such tables (see module docs).
    conn.execute_batch(
        "ALTER TABLE publishers ADD COLUMN link TEXT NOT NULL DEFAULT '';
         ALTER TABLE series ADD COLUMN link TEXT NOT NULL DEFAULT '';
         ALTER TABLE tags ADD COLUMN link TEXT NOT NULL DEFAULT '';
         ALTER TABLE languages ADD COLUMN link TEXT NOT NULL DEFAULT '';
         ALTER TABLE ratings ADD COLUMN link TEXT NOT NULL DEFAULT '';",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built "version 1" starting schema: the baseline every
    /// `upgrade_version_N` in this file assumes already exists,
    /// derived by reading exactly what each step `ALTER`s, rebuilds,
    /// or otherwise depends on being present (e.g. `upgrade_version_4`
    /// backs up and restores `books(id,title,sort,timestamp,
    /// series_index,author_sort,isbn,path)`, implying that's the
    /// pre-v4 shape; `upgrade_version_20` adds `authors.link`,
    /// implying `authors` already has `id,name,sort`; `languages`
    /// doesn't exist at all until `upgrade_version_18` creates it).
    /// Deliberately *not* version 0: `user_version == 0` means
    /// "brand-new, empty database" both upstream (`DB.__init__`:
    /// `if self.user_version == 0: self.initialize_database()`) and in
    /// this port (`Backend::new`) -- it gets the latest schema
    /// directly and never runs this migration chain at all (matching
    /// `SchemaUpgrade.__init__`'s loop, which breaks immediately when
    /// `getattr(self, 'upgrade_version_0', None)` finds nothing,
    /// since no such method exists). The chain only ever runs for a
    /// library an *older calibre version* already created (and
    /// therefore already bumped to at least version 1) -- version 1
    /// is the lowest real starting point. There's no real "version 1"
    /// fixture available to test against instead -- calibre's own
    /// `db/tests/metadata.db` is already at version 26.
    fn version_1_schema() -> &'static str {
        "CREATE TABLE books (id INTEGER PRIMARY KEY AUTOINCREMENT,
             title TEXT NOT NULL DEFAULT 'Unknown' COLLATE NOCASE,
             sort TEXT COLLATE NOCASE,
             timestamp TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
             series_index REAL NOT NULL DEFAULT 1.0,
             author_sort TEXT COLLATE NOCASE,
             isbn TEXT DEFAULT '' COLLATE NOCASE,
             path TEXT NOT NULL DEFAULT '');
         CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, sort TEXT COLLATE NOCASE, UNIQUE(name));
         CREATE TABLE series (id INTEGER PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, sort TEXT COLLATE NOCASE, UNIQUE(name));
         CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, UNIQUE(name));
         CREATE TABLE publishers (id INTEGER PRIMARY KEY, name TEXT NOT NULL COLLATE NOCASE, sort TEXT COLLATE NOCASE, UNIQUE(name));
         CREATE TABLE ratings (id INTEGER PRIMARY KEY, rating INTEGER CHECK(rating > -1 AND rating < 11), UNIQUE(rating));
         CREATE TABLE data (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, format TEXT NOT NULL COLLATE NOCASE, uncompressed_size INTEGER NOT NULL, name TEXT NOT NULL, UNIQUE(book, format));
         CREATE TABLE comments (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, text TEXT NOT NULL COLLATE NOCASE, UNIQUE(book));
         CREATE TABLE conversion_options (id INTEGER PRIMARY KEY, format TEXT NOT NULL COLLATE NOCASE, book INTEGER, data BLOB NOT NULL, UNIQUE(format,book));
         CREATE TABLE feeds (id INTEGER PRIMARY KEY, title TEXT NOT NULL, script TEXT NOT NULL, UNIQUE(title));
         CREATE TABLE books_authors_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, author INTEGER NOT NULL, UNIQUE(book, author));
         CREATE TABLE books_publishers_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, publisher INTEGER NOT NULL, UNIQUE(book));
         CREATE TABLE books_ratings_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, rating INTEGER NOT NULL, UNIQUE(book, rating));
         CREATE TABLE books_series_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, series INTEGER NOT NULL, UNIQUE(book));
         CREATE TABLE books_tags_link (id INTEGER PRIMARY KEY, book INTEGER NOT NULL, tag INTEGER NOT NULL, UNIQUE(book, tag));
         PRAGMA user_version=1;"
    }

    fn open_version_1_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::backend::register_functions(&conn).unwrap();
        conn.execute_batch(version_1_schema()).unwrap();
        conn
    }

    #[test]
    fn upgrades_a_version_1_library_all_the_way_to_26() {
        let mut conn = open_version_1_db();
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        let uv: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(uv, 26);
    }

    #[test]
    fn upgrade_adds_columns_and_tables_introduced_across_the_chain() {
        let mut conn = open_version_1_db();
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();

        // v7: uuid column on books.
        conn.execute("SELECT uuid FROM books", []).unwrap();
        // v9: custom_columns table.
        conn.execute("SELECT * FROM custom_columns", []).unwrap();
        // v13: metadata_dirtied table.
        conn.execute("SELECT * FROM metadata_dirtied", []).unwrap();
        // v14: has_cover column.
        conn.execute("SELECT has_cover FROM books", []).unwrap();
        // v17: books_plugin_data table.
        conn.execute("SELECT * FROM books_plugin_data", []).unwrap();
        // v18: library_id/identifiers/languages tables, last_modified column.
        conn.execute("SELECT * FROM library_id", []).unwrap();
        conn.execute("SELECT * FROM identifiers", []).unwrap();
        conn.execute("SELECT * FROM languages", []).unwrap();
        conn.execute("SELECT last_modified FROM books", []).unwrap();
        // v20: authors.link.
        conn.execute("SELECT link FROM authors", []).unwrap();
        // v22: last_read_positions table.
        conn.execute("SELECT * FROM last_read_positions", [])
            .unwrap();
        // v23: annotations table + FTS.
        conn.execute("SELECT * FROM annotations", []).unwrap();
        conn.execute("SELECT * FROM annotations_fts", []).unwrap();
        // v25: link column on publishers/series/tags/languages/ratings.
        for table in ["publishers", "series", "tags", "languages", "ratings"] {
            conn.execute(&format!("SELECT link FROM {table}"), [])
                .unwrap();
        }
    }

    #[test]
    fn upgrade_preserves_existing_book_data_through_the_books_table_rebuild_at_v4() {
        let conn = open_version_1_db();
        conn.execute(
            "INSERT INTO books (title, sort, author_sort, isbn, path) VALUES ('My Title', 'Title, My', 'Doe, Jane', '1234', 'My Title (1)')",
            [],
        )
        .unwrap();
        let mut conn = conn;
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        let (title, isbn): (String, String) = conn
            .query_row("SELECT title, isbn FROM books WHERE id = 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(title, "My Title");
        assert_eq!(isbn, "1234");
    }

    #[test]
    fn upgrade_creates_working_tag_browser_views_including_ratings() {
        let mut conn = open_version_1_db();
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        for view in [
            "tag_browser_authors",
            "tag_browser_tags",
            "tag_browser_publishers",
            "tag_browser_series",
            "tag_browser_ratings",
            "tag_browser_filtered_authors",
            "tag_browser_filtered_ratings",
        ] {
            conn.execute(&format!("SELECT * FROM {view}"), []).unwrap();
        }
    }

    #[test]
    fn upgrade_is_idempotent_when_already_at_the_latest_version() {
        let mut conn = open_version_1_db();
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        // Running again against an already-current library must be a
        // real no-op, not an error (e.g. from re-adding a column).
        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        let uv: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(uv, 26);
    }

    #[test]
    fn upgrade_from_a_partial_version_runs_only_the_remaining_steps() {
        // Start from a schema already at v20 (i.e. only run 21-25) to
        // exercise resuming mid-chain, not just from scratch.
        let conn = open_version_1_db();
        let mut conn = conn;
        for uv in 0..20 {
            let step: fn(&Connection, &Path) -> Result<()> = match uv {
                1 => upgrade_version_1,
                2 => upgrade_version_2,
                3 => upgrade_version_3,
                4 => upgrade_version_4,
                5 => upgrade_version_5,
                6 => upgrade_version_6,
                7 => upgrade_version_7,
                8 => upgrade_version_8,
                9 => upgrade_version_9,
                10 => upgrade_version_10,
                11 => upgrade_version_11,
                12 => upgrade_version_12,
                13 => upgrade_version_13,
                14 => upgrade_version_14,
                15 => upgrade_version_15,
                16 => upgrade_version_16,
                17 => upgrade_version_17,
                18 => upgrade_version_18,
                19 => upgrade_version_19,
                _ => continue,
            };
            step(&conn, Path::new("/tmp/nonexistent-test-lib")).unwrap();
        }
        conn.execute_batch("PRAGMA user_version=20").unwrap();

        SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"))
            .unwrap();
        let uv: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(uv, 26);
        // v25's effect confirms 21-25 actually ran.
        conn.execute("SELECT link FROM series", []).unwrap();
    }

    #[test]
    fn upgrade_rolls_back_on_failure_leaving_user_version_unchanged() {
        let mut conn = open_version_1_db();
        // Sabotage: pre-create a table upgrade_version_9 also creates,
        // so its CREATE TABLE fails and the whole transaction rolls
        // back -- user_version must stay at 0, not partially advance.
        conn.execute_batch(
            "PRAGMA user_version=9; CREATE TABLE custom_columns (id INTEGER PRIMARY KEY);",
        )
        .unwrap();
        let result =
            SchemaUpgrade::upgrade_to_latest(&mut conn, Path::new("/tmp/nonexistent-test-lib"));
        assert!(result.is_err());
        let uv: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            uv, 9,
            "a failed migration must not leave a partially-bumped version"
        );
    }
}
