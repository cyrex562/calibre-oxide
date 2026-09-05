//! Port of `old_src/src/calibre/db/fields.py` (issue #222, phase 2 --
//! the typed field-access layer built on `tables.rs`'s (phase 1)
//! bulk-loaded [`StandardTables`]).
//!
//! # What this phase is
//!
//! Upstream's `Field`/`OneToOneField`/`ManyToOneField`/
//! `ManyToManyField`/`CompositeField`/etc. wrap a `Table` with a
//! typed access API (`for_book`, `iter_searchable_values`, ...) that
//! `cache.py`'s `field_for`/`set_field`/searching/sorting/categories
//! all go through instead of hitting SQL per call. This crate has no
//! per-field-datatype class hierarchy driven by
//! [`crate::field_metadata::FieldMetadata`] (issue #421 landed the
//! registry itself, but [`FieldStore`] hasn't been rewired to build
//! its match arms from it -- that rewiring is its own separate
//! follow-up, not attempted as part of #421), so [`FieldStore`] plays
//! that role directly: one struct wrapping [`StandardTables`], with a
//! [`FieldStore::field_for`] method that reproduces every arm of
//! [`crate::cache::Cache::field_for`]'s original per-call-SQL match,
//! reading from the in-memory tables instead. This is the piece
//! [`crate::cache::Cache::field_for`] (#222 phase 3) now actually
//! delegates to -- see `cache.rs`'s module doc for the cutover and
//! its cache-invalidation-on-write story.
//!
//! Replaces the previous `Field` trait/`BasicField` skeleton, which
//! had no real callers anywhere in the crate and added no value over
//! using [`crate::tables::TableType`] directly.

use crate::tables::StandardTables;
use rusqlite::{Connection, Result};

/// Real in-memory field-access layer: owns a [`StandardTables`]
/// snapshot and answers `field_for`-shaped queries against it,
/// matching [`crate::cache::Cache::field_for`]'s field set and value
/// contract (`Option<String>`, multi-value fields joined the same way
/// -- `" & "` for authors, `", "` for tags/languages/formats,
/// `"type:val,type:val"` for identifiers) exactly, so callers can't
/// tell the difference except by speed.
pub struct FieldStore {
    tables: StandardTables,
}

impl FieldStore {
    pub fn load(conn: &Connection) -> Result<Self> {
        Ok(Self {
            tables: StandardTables::read(conn)?,
        })
    }

    /// Every field [`crate::cache::Cache::field_for`] supports, except
    /// `"id"` -- there's no dedicated `id` table to bulk-load (a
    /// book's id *is* every table's map key), so that one stays a
    /// direct, trivial SQL lookup in `Cache::field_for` itself.
    pub fn field_for(&self, book_id: i32, field_name: &str) -> Option<String> {
        let t = &self.tables;
        match field_name {
            "title" => t.title.book_col_map.get(&book_id).cloned(),
            "sort" => t.sort.book_col_map.get(&book_id).cloned(),
            "author_sort" => t.author_sort.book_col_map.get(&book_id).cloned(),
            "isbn" => t.isbn.book_col_map.get(&book_id).cloned(),
            "path" => t.path.book_col_map.get(&book_id).cloned(),
            "timestamp" => t.timestamp.book_col_map.get(&book_id).cloned(),
            "pubdate" => t.pubdate.book_col_map.get(&book_id).cloned(),
            "last_modified" => t.last_modified.book_col_map.get(&book_id).cloned(),
            "series_index" => t.series_index.book_col_map.get(&book_id).cloned(),
            "comments" => t.comments.book_col_map.get(&book_id).cloned(),
            "uuid" => t.uuid.book_col_map.get(&book_id).cloned(),
            "size" => t.size.book_col_map.get(&book_id).map(|n| n.to_string()),
            "series" => many_to_one(&t.series, book_id),
            "publisher" => many_to_one(&t.publisher, book_id),
            "rating" => many_to_one(&t.rating, book_id),
            "tags" => many_to_many(&t.tags, book_id, ", "),
            "languages" => many_to_many(&t.languages, book_id, ", "),
            "authors" => {
                let ids = t.authors.book_col_map.get(&book_id)?;
                let names: Vec<&str> = ids
                    .iter()
                    .filter_map(|id| t.authors.id_map.get(id).map(|s| s.as_str()))
                    .collect();
                if names.is_empty() {
                    None
                } else {
                    Some(names.join(" & "))
                }
            }
            "formats" => {
                let fmts = t.formats.book_col_map.get(&book_id)?;
                if fmts.is_empty() {
                    None
                } else {
                    Some(fmts.join(", "))
                }
            }
            "identifiers" => {
                let idents = t.identifiers.book_col_map.get(&book_id)?;
                if idents.is_empty() {
                    None
                } else {
                    Some(
                        idents
                            .iter()
                            .map(|(k, v)| format!("{k}:{v}"))
                            .collect::<Vec<_>>()
                            .join(","),
                    )
                }
            }
            _ => None,
        }
    }

    /// Port of `author_sorts`/`get_link('authors', ...)`/
    /// `author_links`'s own per-author lookups (issue #514) --
    /// keyed by author *name* since that's what a template caller
    /// has (a book's author-name list), not the internal id.
    pub fn author_sort_for_name(&self, name: &str) -> Option<String> {
        let t = &self.tables;
        let id = t.authors.id_map.iter().find(|(_, n)| n.as_str() == name)?.0;
        t.authors.asort_map.get(id).cloned()
    }

    pub fn author_link_for_name(&self, name: &str) -> Option<String> {
        let t = &self.tables;
        let id = t.authors.id_map.iter().find(|(_, n)| n.as_str() == name)?.0;
        t.authors.link_map.get(id).cloned()
    }

    /// A book's author *names* in real link order (unlike
    /// `field_for("authors")`, which returns them pre-joined into one
    /// `" & "`-separated string) -- needed to look each one up in
    /// `asort_map` while preserving the book's own author ordering.
    pub fn author_names_for_book(&self, book_id: i32) -> Vec<String> {
        let t = &self.tables;
        t.authors.book_col_map.get(&book_id).map(|ids| ids.iter().filter_map(|id| t.authors.id_map.get(id).cloned()).collect()).unwrap_or_default()
    }

    /// `(author_name, link_url)` for every author with a real
    /// (non-empty) stored link -- backs `author_links()`.
    pub fn all_author_links(&self) -> Vec<(String, String)> {
        let t = &self.tables;
        t.authors.link_map.iter().filter(|(_, link)| !link.is_empty()).filter_map(|(id, link)| t.authors.id_map.get(id).map(|name| (name.clone(), link.clone()))).collect()
    }

    /// `(format, stored_filename, size)` for every one of a book's
    /// formats, in `book_col_map`'s own sorted-by-format order --
    /// backs `formats_sizes`/`formats_paths`/`formats_modtimes`/
    /// `formats_path_segments` (issue #524). `stored_filename` is the
    /// real `data.name` column value (no extension), matching
    /// upstream's own `format_abspath` -- NOT a re-derivation from the
    /// book's current title, which could drift from the real on-disk
    /// name if the title changed after the format was added.
    pub fn formats_for_book(&self, book_id: i32) -> Vec<(String, String, i64)> {
        let t = &self.tables;
        let Some(fmts) = t.formats.book_col_map.get(&book_id) else { return Vec::new() };
        fmts.iter()
            .filter_map(|fmt| {
                let fname = t.formats.fname_map.get(&book_id)?.get(fmt)?.clone();
                let size = *t.formats.size_map.get(&book_id)?.get(fmt)?;
                Some((fmt.clone(), fname, size))
            })
            .collect()
    }
}

fn many_to_one(table: &crate::tables::ManyToOneTable, book_id: i32) -> Option<String> {
    let item_id = table.book_col_map.get(&book_id)?;
    table.id_map.get(item_id).cloned()
}

fn many_to_many(table: &crate::tables::ManyToManyTable, book_id: i32, sep: &str) -> Option<String> {
    let ids = table.book_col_map.get(&book_id)?;
    let names: Vec<&str> = ids
        .iter()
        .filter_map(|id| table.id_map.get(id).map(|s| s.as_str()))
        .collect();
    if names.is_empty() {
        None
    } else {
        Some(names.join(sep))
    }
}
