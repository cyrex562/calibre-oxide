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

use crate::tables::{StandardTables, TableSet};
use rusqlite::{Connection, Result};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Source of [`FieldStore::version`]: unique across every store and every
/// change to one, so a result cached against a version is never served for
/// different data.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

/// Real in-memory field-access layer: owns a [`StandardTables`]
/// snapshot and answers `field_for`-shaped queries against it,
/// matching [`crate::cache::Cache::field_for`]'s field set and value
/// contract (`Option<String>`, multi-value fields joined the same way
/// -- `" & "` for authors, `", "` for tags/languages/formats,
/// `"type:val,type:val"` for identifiers) exactly, so callers can't
/// tell the difference except by speed.
#[derive(Clone)]
pub struct FieldStore {
    tables: StandardTables,
    version: u64,
    /// When each table last changed, as a [`FieldStore::version`] value
    /// (see [`FieldStore::fields_version`]).
    table_versions: [u64; TableSet::COUNT],
    derived: Derived,
}

/// Text folded for `contains` searches, one entry per field, built the first
/// time a search needs it and dropped whenever the store changes.
#[derive(Default)]
struct Derived {
    folded: Mutex<HashMap<&'static str, Arc<Folded>>>,
}

impl Clone for Derived {
    fn clone(&self) -> Self {
        Derived { folded: Mutex::new(self.folded.lock().unwrap().clone()) }
    }
}

/// The pre-folded text of one searchable field (see `search::fold_primary`).
enum Folded {
    /// One value per book.
    Single(HashMap<i32, String>),
    /// One value per item (series, publisher), looked up through the book's item.
    OneItem(HashMap<i32, String>),
    /// The `sep`-separated parts of each item (tags, authors, languages), or
    /// `None` when the items cannot be told apart from the joined string the
    /// old per-book search split (an item that contains the separator).
    ManyItems(Option<HashMap<i32, Vec<String>>>),
    /// Same for formats, keyed by format.
    Formats(Option<HashMap<String, Vec<String>>>),
}

/// The folded parts of one stored value: `sep`-split when it is a multi-value
/// field. The value is lower-cased first, as `text_match` does.
fn fold_parts(raw: &str, sep: Option<&str>) -> Vec<String> {
    let lowered = lexmechanic_utils::icu::lower(raw);
    match sep {
        Some(sep) => lowered.split(sep).map(crate::search::fold_primary).collect(),
        None => vec![crate::search::fold_primary(&lowered)],
    }
}

/// Whether joining `items` with `sep` and splitting again gives back exactly
/// the items' own splits. It does unless an item holds the separator or ends
/// or starts with part of it (which would make the join ambiguous).
fn join_is_unambiguous<'a>(items: impl Iterator<Item = &'a String>, sep: &str) -> bool {
    let prefixes: Vec<&str> = (1..sep.len()).filter(|i| sep.is_char_boundary(*i)).map(|i| &sep[..i]).collect();
    let suffixes: Vec<&str> = (1..sep.len()).filter(|i| sep.is_char_boundary(*i)).map(|i| &sep[i..]).collect();
    let mut items = items;
    items.all(|name| !name.contains(sep) && !prefixes.iter().any(|p| name.ends_with(p)) && !suffixes.iter().any(|s| name.starts_with(s)))
}

/// The tables the sort key of `field` is read from (see
/// `ajax::sort_row`: `title` sorts on `sort`, `authors` on `author_sort`).
/// `None` for a field not known here.
fn sort_field_tables(field: &str) -> Option<TableSet> {
    Some(match field {
        "title" | "sort" => TableSet::SORT,
        "authors" | "author_sort" => TableSet::AUTHOR_SORT,
        "publisher" => TableSet::PUBLISHER,
        "series" => TableSet::SERIES,
        "uuid" => TableSet::UUID,
        "comments" => TableSet::COMMENTS,
        "isbn" => TableSet::ISBN,
        "identifiers" => TableSet::IDENTIFIERS,
        "rating" => TableSet::RATING,
        "size" => TableSet::SIZE,
        "series_index" => TableSet::SERIES_INDEX,
        "tags" => TableSet::TAGS,
        "languages" => TableSet::LANGUAGES,
        "timestamp" => TableSet::TIMESTAMP,
        "pubdate" => TableSet::PUBDATE,
        "last_modified" => TableSet::LAST_MODIFIED,
        "id" => TableSet::default(),
        "folder" => TableSet::PATH,
        "file_name" => TableSet::FORMATS,
        _ => return None,
    })
}

/// The tables the folded text of a searchable field is derived from.
fn folded_field_tables(field: &str) -> TableSet {
    match field {
        "title" => TableSet::TITLE,
        "sort" => TableSet::SORT,
        "author_sort" => TableSet::AUTHOR_SORT,
        "comments" => TableSet::COMMENTS,
        "path" => TableSet::PATH,
        "series" => TableSet::SERIES,
        "publisher" => TableSet::PUBLISHER,
        "tags" => TableSet::TAGS,
        "languages" => TableSet::LANGUAGES,
        "authors" => TableSet::AUTHORS,
        "formats" => TableSet::FORMATS,
        // Not one of the known fields: drop it with anything.
        _ => TableSet::ALL,
    }
}

impl FieldStore {
    pub fn load(conn: &Connection) -> Result<Self> {
        Ok(Self::from_tables(StandardTables::read(conn)?))
    }

    /// A store holding `tables`, every table as new as the store.
    pub fn from_tables(tables: StandardTables) -> Self {
        let version = next_version();
        Self { tables, version, table_versions: [version; TableSet::COUNT], derived: Derived::default() }
    }

    /// Identifies this exact content: a different value after any load or
    /// patch. Results derived from the store (a sorted order, say) can be
    /// cached against it.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The tables, to see which of them two snapshots share.
    #[cfg(test)]
    pub(crate) fn tables(&self) -> &StandardTables {
        &self.tables
    }

    /// How many books have a title (the books `get_data_as_dict` lists).
    pub fn book_count(&self) -> usize {
        self.tables.title.book_col_map.len()
    }

    /// Notes that `tables` changed: a new store version, the tables' own
    /// versions, and the folded text derived from them dropped (the rest of
    /// it is still good).
    fn changed(&mut self, tables: TableSet) {
        if tables.is_empty() {
            return;
        }
        self.version = next_version();
        for i in tables.indices() {
            self.table_versions[i] = self.version;
        }
        let mut folded = self.derived.folded.lock().unwrap();
        folded.retain(|field, _| !folded_field_tables(field).indices().any(|i| tables.indices().any(|j| i == j)));
    }

    /// Re-reads one book (see [`StandardTables::reload_book`]). The store's
    /// version moves only if something in the book's rows changed.
    pub fn reload_book(&mut self, conn: &Connection, book_id: i32) -> Result<()> {
        let changed = self.tables.reload_book(conn, book_id)?;
        self.changed(changed);
        Ok(())
    }

    /// [`FieldStore::reload_book`] for a book already read into `fresh` (see
    /// [`StandardTables::read_book_rows`]): no database access, so it can be
    /// done without holding the connection.
    pub fn adopt_book(&mut self, fresh: &StandardTables, book_id: i32) {
        let changed = self.tables.adopt_book(fresh, book_id);
        self.changed(changed);
    }

    /// Re-reads one whole item table (see [`StandardTables::reload_item_table`]).
    pub fn reload_item_table(&mut self, conn: &Connection, which: crate::tables::ItemTable) -> Result<()> {
        let changed = self.tables.reload_item_table(conn, which)?;
        self.changed(changed);
        Ok(())
    }

    /// Replaces one item table with the one in `fresh`, read earlier.
    pub fn adopt_item_table(&mut self, fresh: &StandardTables, which: crate::tables::ItemTable) {
        use crate::tables::ItemTable;
        let mut changed = TableSet::default();
        macro_rules! take {
            ($field:ident, $bit:expr) => {
                if *self.tables.$field != *fresh.$field {
                    self.tables.$field = fresh.$field.clone();
                    changed.insert($bit);
                }
            };
        }
        match which {
            ItemTable::Series => take!(series, TableSet::SERIES),
            ItemTable::Publishers => take!(publisher, TableSet::PUBLISHER),
            ItemTable::Ratings => take!(rating, TableSet::RATING),
            ItemTable::Tags => take!(tags, TableSet::TAGS),
            ItemTable::Languages => take!(languages, TableSet::LANGUAGES),
            ItemTable::Authors => take!(authors, TableSet::AUTHORS),
        }
        self.changed(changed);
    }

    /// Identifies this content **as far as `fields` are concerned**: it
    /// changes only when a table those fields are read from does, where
    /// [`FieldStore::version`] changes with every edit to every field. A sort
    /// order built from `fields` can be kept for as long as this stays the
    /// same, through edits to other fields (a rating, a comment). Which books are
    /// listed (those with a title) counts too: a book appearing or going moves
    /// every sort, a title being edited does not.
    ///
    /// Falls back to the store's whole version for a field this does not know
    /// the tables of.
    pub fn fields_version(&self, fields: &[&str]) -> u64 {
        let mut tables = TableSet::MEMBERS;
        for field in fields {
            match sort_field_tables(field) {
                Some(t) => tables.insert(t),
                None => return self.version,
            }
        }
        tables.indices().map(|i| self.table_versions[i]).max().unwrap_or(self.version)
    }

    /// The pre-folded text of `field`, built on first use.
    fn folded(&self, field: &'static str) -> Option<Arc<Folded>> {
        if let Some(f) = self.derived.folded.lock().unwrap().get(field) {
            return Some(Arc::clone(f));
        }
        let t = &self.tables;
        let fold_one = |m: &HashMap<i32, String>| -> HashMap<i32, String> { m.iter().map(|(id, v)| (*id, fold_parts(v, None).pop().unwrap_or_default())).collect() };
        let many = |names: &HashMap<i32, String>, sep: &str| -> Folded {
            if !join_is_unambiguous(names.values(), sep) {
                return Folded::ManyItems(None);
            }
            Folded::ManyItems(Some(names.iter().map(|(id, name)| (*id, fold_parts(name, Some(sep)))).collect()))
        };
        let built = match field {
            "title" => Folded::Single(fold_one(&t.title.book_col_map)),
            "sort" => Folded::Single(fold_one(&t.sort.book_col_map)),
            "author_sort" => Folded::Single(fold_one(&t.author_sort.book_col_map)),
            "comments" => Folded::Single(fold_one(&t.comments.book_col_map)),
            "path" => Folded::Single(fold_one(&t.path.book_col_map)),
            "series" => Folded::OneItem(fold_one(&t.series.id_map)),
            "publisher" => Folded::OneItem(fold_one(&t.publisher.id_map)),
            "tags" => many(&t.tags.id_map, ", "),
            "languages" => many(&t.languages.id_map, ", "),
            "authors" => many(&t.authors.id_map, " & "),
            "formats" => {
                if !join_is_unambiguous(t.formats.col_book_map.keys(), ", ") {
                    Folded::Formats(None)
                } else {
                    Folded::Formats(Some(t.formats.col_book_map.keys().map(|f| (f.clone(), fold_parts(f, Some(", ")))).collect()))
                }
            }
            _ => return None,
        };
        let built = Arc::new(built);
        self.derived.folded.lock().unwrap().insert(field, Arc::clone(&built));
        Some(built)
    }

    /// The books among `candidates` whose `field` value has a part that
    /// contains `needle` once both are folded: exactly what
    /// `text_match(.., Contains, use_primary = true)` decides per book, but
    /// working on text folded once per store version and, for multi-value
    /// fields, on each distinct item rather than on every book's joined
    /// string. `needle` is `search::fold_primary` of the (lower-cased) query.
    /// `None` when this field or its data cannot be answered that way and the
    /// caller must match book by book.
    pub fn folded_contains(&self, field: &str, needle: &str, candidates: &HashSet<i32>) -> Option<HashSet<i32>> {
        let key: &'static str = ["title", "sort", "author_sort", "comments", "path", "series", "publisher", "tags", "languages", "authors", "formats"].into_iter().find(|f| *f == field)?;
        let folded = self.folded(key)?;
        let t = &self.tables;
        let any_part = |parts: &Vec<String>| parts.iter().any(|p| p.contains(needle));
        // Walking every book of the field costs the same however few are
        // candidates, so a small candidate set is looked up instead.
        let scan = |len: usize| candidates.len() * 2 >= len;
        // Books linked to a hit item: from the link map directly when most of
        // the library is in play, else from each candidate's own links.
        fn linked<V>(links: &HashMap<i32, V>, candidates: &HashSet<i32>, scan: bool, any: impl Fn(&V) -> bool) -> HashSet<i32> {
            if scan {
                links.iter().filter(|(b, v)| any(v) && candidates.contains(b)).map(|(b, _)| *b).collect()
            } else {
                candidates.iter().copied().filter(|b| links.get(b).is_some_and(&any)).collect()
            }
        }
        let out: HashSet<i32> = match (&*folded, key) {
            (Folded::Single(m), _) => {
                if scan(m.len()) {
                    m.iter().filter(|(b, f)| f.contains(needle) && candidates.contains(b)).map(|(b, _)| *b).collect()
                } else {
                    candidates.iter().copied().filter(|b| m.get(b).is_some_and(|f| f.contains(needle))).collect()
                }
            }
            (Folded::OneItem(m), _) => {
                let hits: HashSet<i32> = m.iter().filter(|(_, f)| f.contains(needle)).map(|(i, _)| *i).collect();
                let links = if key == "series" { &t.series.book_col_map } else { &t.publisher.book_col_map };
                linked(links, candidates, scan(links.len()), |i| hits.contains(i))
            }
            (Folded::ManyItems(None), _) | (Folded::Formats(None), _) => return None,
            (Folded::ManyItems(Some(m)), _) => {
                let hits: HashSet<i32> = m.iter().filter(|(_, parts)| any_part(parts)).map(|(i, _)| *i).collect();
                let links = match key {
                    "tags" => &t.tags.book_col_map,
                    "languages" => &t.languages.book_col_map,
                    _ => &t.authors.book_col_map,
                };
                linked(links, candidates, scan(links.len()), |ids: &Vec<i32>| ids.iter().any(|i| hits.contains(i)))
            }
            (Folded::Formats(Some(m)), _) => {
                let hits: HashSet<&String> = m.iter().filter(|(_, parts)| any_part(parts)).map(|(f, _)| f).collect();
                let links = &t.formats.book_col_map;
                linked(links, candidates, scan(links.len()), |fmts: &Vec<String>| fmts.iter().any(|f| hits.contains(f)))
            }
        };
        Some(out)
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
