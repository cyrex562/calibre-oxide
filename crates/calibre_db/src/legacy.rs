//! Port of `old_src/src/calibre/db/legacy.py` (issue #223, a #201
//! follow-up): `LibraryDatabase`, a compatibility-emulation layer
//! re-exposing the old, pre-2013 `library/database2.py` API surface
//! (hundreds of methods) on top of the modern `db.cache.Cache`, so old
//! plugins/scripts written against the legacy API keep working
//! unmodified.
//!
//! # Scope of this pass
//!
//! Upstream's `legacy.py` is ~1050 lines defining `LibraryDatabase`
//! (a class body plus large `setattr(LibraryDatabase, name, ...)`
//! loops that mechanically generate hundreds of thin
//! `self.new_api.<method>` delegating wrappers). This crate now has a
//! real, unified [`crate::cache::Cache`] (post #212/#214/#216) to
//! delegate to, and a real [`crate::view::View`] that plays the role
//! of upstream's `self.data` (the row-order/id-lookup view an
//! index-based, `index_is_id=False` call resolves through) -- so
//! [`LegacyDb`] ports the delegation pattern faithfully for every
//! method whose real implementation already exists somewhere in this
//! crate, rather than re-deriving a parallel implementation.
//!
//! What's real: id/index resolution (`id`/`index`/`has_id`/`all_ids`/
//! `is_empty`/`refresh`), the full metaprogrammed legacy getter API
//! (`title`, `authors`, `comment(s)`, `publisher`, `rating`, `series`,
//! `series_index`, `tags`, `title_sort`, `timestamp`, `uuid`,
//! `pubdate`, `languages`, `max_size`), the legacy setter API (backed
//! by the new [`crate::cache::Cache::set_field`]), format/cover/
//! identifier accessors, `get_categories`, `find_identical_books`,
//! `get_data_as_dict`, the `all_*_names`/`get_*_with_ids`/`*_name`/
//! `delete_*_using_id`/`rename_*` item-management families, directory
//! import (`find_books_in_directory`/`import_book_directory[_multiple]`/
//! `recursive_import`, delegating to `adding.rs` from #219), and a few
//! standalone helpers (`get_next_series_num_for`, `has_book`,
//! `author_sort_from_authors`).
//!
//! # Not ported (disclosed)
//!
//! - **`index_is_id=false` addressing** resolves through `self.data`
//!   (a [`View`] over every book, unrestricted/unsorted) exactly like
//!   upstream's fresh-`LibraryDatabase` state -- but upstream's
//!   `self.data` is also the live view a GUI's search/sort actions
//!   mutate; this port's `refresh()` rebuilds an unrestricted view
//!   rather than tracking a persistent restriction.
//! - **`field_metadata`/composite fields**: no such subsystem exists
//!   in this crate (the single most recurring disclosed gap across
//!   the #201 follow-up chain) -- `standard_field_keys`/
//!   `searchable_fields`/`sortable_field_keys`/`all_field_keys` return
//!   a hardcoded list matching [`crate::cache::Cache::field_for`]'s
//!   supported fields, not real per-library metadata.
//! - **Saved searches**: no such subsystem exists in this crate at
//!   all (tracked separately, e.g. issue #226 for FTS); the
//!   `saved_search_*` family is not implemented.
//! - **`allow_case_change`/dirtying/notification/conversion options/
//!   custom book data/on-device state**: none of these subsystems
//!   exist in this crate; the corresponding legacy methods are not
//!   implemented.
//! - **`create_book_entry`/`add_books`/`import_book`**: upstream's
//!   real versions handle format-file copying, duplicate detection,
//!   and metadata plugins together; this crate's building blocks
//!   ([`crate::adding`], [`crate::copy_to_library`]) already have
//!   their own narrower, disclosed shapes from #219/#221 -- this pass
//!   exposes those as-is rather than writing a third variant.

use crate::cache::Cache;
use crate::categories::{self, Tag};
use crate::copy_to_library::duplicate_detection_maps;
use crate::utils::find_identical_books;
use crate::view::View;
use anyhow::Result;
use indexmap::IndexMap;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// This crate's own pre-existing (non-upstream) legacy-database-format
/// compatibility checker -- distinct from [`LegacyDb`] below, which is
/// the actual port of `legacy.py`'s `LibraryDatabase`.
pub struct LegacyDB;

impl Default for LegacyDB {
    fn default() -> Self {
        Self::new()
    }
}

impl LegacyDB {
    pub fn new() -> Self {
        LegacyDB
    }

    pub fn check_compatibility(&self, db_path: &Path) -> Result<bool> {
        if !db_path.exists() {
            return Ok(true);
        }
        Ok(true)
    }

    pub fn migrate(&self, _db_path: &Path) -> Result<()> {
        Err(anyhow::anyhow!(
            "Legacy database migration is not supported in this version."
        ))
    }
}

/// The standard field names [`Cache::field_for`] resolves -- used
/// here in place of upstream's real `field_metadata`-driven field
/// list, same disclosed simplification as elsewhere in this crate.
const STANDARD_FIELD_KEYS: &[&str] = &[
    "title",
    "sort",
    "author_sort",
    "authors",
    "comments",
    "isbn",
    "path",
    "uuid",
    "series_index",
    "timestamp",
    "pubdate",
    "last_modified",
    "series",
    "publisher",
    "rating",
    "tags",
    "languages",
    "formats",
    "identifiers",
    "size",
];

/// Port of `legacy.py`'s `LibraryDatabase` -- a compatibility wrapper
/// around [`Cache`] (upstream's `self.new_api`) and a [`View`]
/// (upstream's `self.data`). See the module doc comment for scope.
pub struct LegacyDb {
    pub new_api: Arc<Mutex<Cache>>,
    data: Mutex<View>,
    library_path: PathBuf,
}

impl LegacyDb {
    pub fn new<P: AsRef<Path>>(library_path: P) -> Result<Self> {
        let library_path = library_path.as_ref().to_path_buf();
        let cache = Arc::new(Mutex::new(Cache::new(&library_path)?));
        let data = Mutex::new(View::new(cache.clone()));
        Ok(LegacyDb {
            new_api: cache,
            data,
            library_path,
        })
    }

    pub fn library_path(&self) -> &Path {
        &self.library_path
    }

    pub fn dbpath(&self) -> PathBuf {
        self.library_path.join("metadata.db")
    }

    /// A no-op: this port has no open file handles or background
    /// threads to close beyond what `Cache`'s `Drop` already handles.
    pub fn close(&self) {}

    /// Upstream checks the on-disk file's mtime/size against what was
    /// last seen, to detect another process (or a device sync)
    /// touching the file underneath a long-lived `LibraryDatabase`.
    /// This port has no such out-of-band change tracking, so this
    /// always reports "not modified".
    pub fn check_if_modified(&self) -> bool {
        false
    }

    // --- id/index resolution (mirrors upstream's `self.data`) {{{

    /// Rebuilds `self.data` as an unrestricted, unsorted view over
    /// every book -- see the module doc comment's disclosed gap
    /// around persistent view state.
    pub fn refresh(&self) {
        *self.data.lock().unwrap() = View::new(self.new_api.clone());
    }

    pub fn all_ids(&self) -> Vec<i32> {
        self.data.lock().unwrap().get_ids().to_vec()
    }

    pub fn is_empty(&self) -> bool {
        self.data.lock().unwrap().count() == 0
    }

    /// `self.data.index_to_id(index)`.
    pub fn id(&self, index: usize) -> Option<i32> {
        self.data.lock().unwrap().get_ids().get(index).copied()
    }

    /// `self.data.id_to_index(book_id)`.
    pub fn index(&self, book_id: i32) -> Option<usize> {
        self.data
            .lock()
            .unwrap()
            .get_ids()
            .iter()
            .position(|&x| x == book_id)
    }

    pub fn has_id(&self, book_id: i32) -> bool {
        self.new_api
            .lock()
            .unwrap()
            .field_for(book_id, "id")
            .ok()
            .flatten()
            .is_some()
    }

    fn resolve(&self, index: i32, index_is_id: bool) -> Option<i32> {
        if index_is_id {
            Some(index)
        } else {
            self.id(index as usize)
        }
    }

    // }}}

    // --- Legacy getter API {{{

    pub fn get_property(&self, index: i32, index_is_id: bool, field: &str) -> Option<String> {
        let book_id = self.resolve(index, index_is_id)?;
        self.new_api
            .lock()
            .unwrap()
            .field_for(book_id, field)
            .ok()
            .flatten()
    }

    pub fn title(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "title")
    }
    pub fn title_sort(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "sort")
    }
    pub fn author_sort(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "author_sort")
    }
    pub fn authors(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "authors")
    }
    pub fn comment(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "comments")
    }
    pub fn comments(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "comments")
    }
    pub fn publisher(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "publisher")
    }
    pub fn max_size(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "size")
    }
    pub fn rating(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "rating")
    }
    pub fn series(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "series")
    }
    pub fn series_index(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "series_index")
    }
    pub fn tags(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "tags")
    }
    pub fn timestamp(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "timestamp")
    }
    pub fn uuid(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "uuid")
    }
    pub fn pubdate(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "pubdate")
    }
    pub fn languages(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "languages")
    }
    pub fn metadata_last_modified(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_property(index, index_is_id, "last_modified")
    }
    /// No on-device subsystem exists in this crate; always `None`.
    pub fn ondevice(&self, _index: i32, _index_is_id: bool) -> Option<String> {
        None
    }

    fn field_id_for(&self, book_id: i32, link_table: &str, link_col: &str) -> Option<i32> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {link_col} FROM {link_table} WHERE book = ?1"),
            [book_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn series_id(&self, index: i32, index_is_id: bool) -> Option<i32> {
        let book_id = self.resolve(index, index_is_id)?;
        self.field_id_for(book_id, "books_series_link", "series")
    }
    pub fn publisher_id(&self, index: i32, index_is_id: bool) -> Option<i32> {
        let book_id = self.resolve(index, index_is_id)?;
        self.field_id_for(book_id, "books_publishers_link", "publisher")
    }

    pub fn has_cover(&self, book_id: i32) -> bool {
        self.new_api
            .lock()
            .unwrap()
            .has_cover(book_id)
            .unwrap_or(false)
    }

    pub fn get_tags(&self, book_id: i32) -> HashSet<String> {
        self.get_property(book_id, true, "tags")
            .map(|s| s.split(", ").map(|t| t.to_string()).collect())
            .unwrap_or_default()
    }

    pub fn get_identifiers(&self, index: i32, index_is_id: bool) -> HashMap<String, String> {
        self.get_property(index, index_is_id, "identifiers")
            .map(|s| {
                s.split(',')
                    .filter_map(|pair| pair.split_once(':'))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn isbn(&self, index: i32, index_is_id: bool) -> Option<String> {
        self.get_identifiers(index, index_is_id)
            .get("isbn")
            .cloned()
    }

    // }}}

    // --- Legacy setter API (backed by `Cache::set_field`) {{{

    fn set_property(&self, book_id: i32, field: &str, value: &str) -> Result<()> {
        self.new_api
            .lock()
            .unwrap()
            .set_field(book_id, field, value)
    }

    pub fn set_title(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "title", value)
    }
    pub fn set_title_sort(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "sort", value)
    }
    pub fn set_author_sort(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "author_sort", value)
    }
    /// `value` is `&`-joined (`"Author One & Author Two"`), matching
    /// how [`Cache::field_for`] returns `authors`.
    pub fn set_authors(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "authors", value)
    }
    pub fn set_comment(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "comments", value)
    }
    pub fn set_has_cover(&self, book_id: i32, value: bool) -> Result<()> {
        self.set_property(book_id, "has_cover", if value { "1" } else { "0" })
    }
    /// `value` is `"type:val,type:val"`, matching how
    /// [`Cache::field_for`] returns `identifiers`.
    pub fn set_identifiers(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "identifiers", value)
    }
    /// `value` is `", "`-joined lang codes, matching how
    /// [`Cache::field_for`] returns `languages`.
    pub fn set_languages(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "languages", value)
    }
    pub fn set_pubdate(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "pubdate", value)
    }
    pub fn set_publisher(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "publisher", value)
    }
    pub fn set_rating(&self, book_id: i32, value: i32) -> Result<()> {
        self.set_property(book_id, "rating", &value.to_string())
    }
    pub fn set_series(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "series", value)
    }
    pub fn set_series_index(&self, book_id: i32, value: f64) -> Result<()> {
        self.set_property(book_id, "series_index", &value.to_string())
    }
    pub fn set_timestamp(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "timestamp", value)
    }
    pub fn set_uuid(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "uuid", value)
    }
    /// `value` is `", "`-joined tag names, matching how
    /// [`Cache::field_for`] returns `tags`.
    pub fn set_tags(&self, book_id: i32, value: &str) -> Result<()> {
        self.set_property(book_id, "tags", value)
    }

    // }}}

    // --- Categories / duplicate detection / bulk export {{{

    pub fn get_categories(
        &self,
        sort: &str,
        ids: Option<&HashSet<i32>>,
    ) -> Result<IndexMap<String, Vec<Tag>>> {
        categories::get_categories(&self.new_api, sort, ids)
    }

    /// `mi_title`/`mi_authors` stand in for upstream's `Metadata`
    /// object argument -- this port takes the two fields
    /// [`find_identical_books`] actually needs directly.
    pub fn find_identical_books(
        &self,
        mi_title: &str,
        mi_authors: &[String],
    ) -> Result<HashSet<i32>> {
        let cache = self.new_api.lock().unwrap();
        let (author_map, aid_to_bids, title_map) = duplicate_detection_maps(&cache)?;
        Ok(find_identical_books(
            mi_title,
            mi_authors,
            &author_map,
            &aid_to_bids,
            &title_map,
        ))
    }

    pub fn get_data_as_dict(&self) -> Result<Vec<serde_json::Value>> {
        self.new_api
            .lock()
            .unwrap()
            .get_data_as_dict(None, false, None, false)
    }

    // }}}

    // --- Many-(one, many) field item management {{{

    fn all_names(&self, table: &str, name_col: &str) -> Vec<String> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!(
            "SELECT {name_col} FROM {table} ORDER BY {name_col}"
        )) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let rows = stmt.query_map([], |row| row.get::<_, String>(0));
        match rows {
            Ok(rows) => rows.filter_map(|r| r.ok()).collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn all_author_names(&self) -> Vec<String> {
        self.all_names("authors", "name")
    }
    pub fn all_tag_names(&self) -> Vec<String> {
        self.all_names("tags", "name")
    }
    pub fn all_series_names(&self) -> Vec<String> {
        self.all_names("series", "name")
    }
    pub fn all_publisher_names(&self) -> Vec<String> {
        self.all_names("publishers", "name")
    }
    pub fn all_formats(&self) -> Vec<String> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT DISTINCT format FROM data ORDER BY format") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| row.get::<_, String>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    fn id_map(&self, table: &str, name_col: &str) -> Vec<(i32, String)> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare(&format!("SELECT id, {name_col} FROM {table}")) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    pub fn get_tags_with_ids(&self) -> Vec<(i32, String)> {
        self.id_map("tags", "name")
    }
    pub fn get_series_with_ids(&self) -> Vec<(i32, String)> {
        self.id_map("series", "name")
    }
    pub fn get_publishers_with_ids(&self) -> Vec<(i32, String)> {
        self.id_map("publishers", "name")
    }
    pub fn get_languages_with_ids(&self) -> Vec<(i32, String)> {
        self.id_map("languages", "lang_code")
    }
    pub fn get_ratings_with_ids(&self) -> Vec<(i32, i32)> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT id, rating FROM ratings") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?)))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
    }

    /// `[[author_id, name, sort, link], ...]` for every author,
    /// matching upstream's `get_authors_with_ids` shape.
    pub fn get_authors_with_ids(&self) -> Vec<(i32, String, String, String)> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare("SELECT id, name, sort, link FROM authors") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        stmt.query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default()
    }

    /// Case-insensitive lookup, matching upstream's `icu_lower`-keyed
    /// dict (approximated the same way as elsewhere in this crate:
    /// ASCII-aware `to_lowercase`, not real ICU casefolding).
    pub fn get_author_id(&self, author: &str) -> Option<i32> {
        let target = author.to_lowercase();
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, name FROM authors").ok()?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
            })
            .ok()?;
        for row in rows.flatten() {
            if row.1.to_lowercase() == target {
                return Some(row.0);
            }
        }
        None
    }

    fn item_name(&self, table: &str, name_col: &str, item_id: i32) -> Option<String> {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        conn.query_row(
            &format!("SELECT {name_col} FROM {table} WHERE id = ?1"),
            [item_id],
            |row| row.get(0),
        )
        .ok()
    }

    pub fn author_name(&self, item_id: i32) -> Option<String> {
        self.item_name("authors", "name", item_id)
    }
    pub fn tag_name(&self, item_id: i32) -> Option<String> {
        self.item_name("tags", "name", item_id)
    }
    pub fn series_name(&self, item_id: i32) -> Option<String> {
        self.item_name("series", "name", item_id)
    }

    fn delete_item_using_id(&self, table: &str, link_table: &str, link_col: &str, item_id: i32) {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let _ = conn.execute(
            &format!("DELETE FROM {link_table} WHERE {link_col} = ?1"),
            [item_id],
        );
        let _ = conn.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [item_id]);
    }

    pub fn delete_tag_using_id(&self, item_id: i32) {
        self.delete_item_using_id("tags", "books_tags_link", "tag", item_id)
    }
    pub fn delete_series_using_id(&self, item_id: i32) {
        self.delete_item_using_id("series", "books_series_link", "series", item_id)
    }
    pub fn delete_publisher_using_id(&self, item_id: i32) {
        self.delete_item_using_id("publishers", "books_publishers_link", "publisher", item_id)
    }

    /// Renames the item and, if that collides with an existing name
    /// (the `name` column's `UNIQUE` constraint), merges by
    /// re-pointing the old item's links at the existing one and
    /// deleting the now-orphaned old row -- a real but simplified
    /// stand-in for upstream's full `rename_items` (which also
    /// updates every affected book's composite/sort fields).
    fn rename_item(
        &self,
        table: &str,
        link_table: &str,
        link_col: &str,
        old_id: i32,
        new_name: &str,
    ) {
        let cache = self.new_api.lock().unwrap();
        let mut conn = cache.backend.conn.lock().unwrap();
        let tx = match conn.transaction() {
            Ok(tx) => tx,
            Err(_) => return,
        };
        let existing: Option<i32> = tx
            .query_row(
                &format!("SELECT id FROM {table} WHERE name = ?1 AND id != ?2"),
                (new_name, old_id),
                |row| row.get(0),
            )
            .ok();
        match existing {
            Some(target_id) => {
                let _ = tx.execute(
                    &format!(
                        "UPDATE OR IGNORE {link_table} SET {link_col} = ?1 WHERE {link_col} = ?2"
                    ),
                    (target_id, old_id),
                );
                let _ = tx.execute(
                    &format!("DELETE FROM {link_table} WHERE {link_col} = ?1"),
                    [old_id],
                );
                let _ = tx.execute(&format!("DELETE FROM {table} WHERE id = ?1"), [old_id]);
            }
            None => {
                let _ = tx.execute(
                    &format!("UPDATE {table} SET name = ?1 WHERE id = ?2"),
                    (new_name, old_id),
                );
            }
        }
        let _ = tx.commit();
    }

    pub fn rename_author(&self, old_id: i32, new_name: &str) {
        self.rename_item("authors", "books_authors_link", "author", old_id, new_name)
    }
    pub fn rename_tag(&self, old_id: i32, new_name: &str) {
        self.rename_item("tags", "books_tags_link", "tag", old_id, new_name)
    }
    pub fn rename_publisher(&self, old_id: i32, new_name: &str) {
        self.rename_item(
            "publishers",
            "books_publishers_link",
            "publisher",
            old_id,
            new_name,
        )
    }

    // }}}

    // --- Field key introspection (no field_metadata subsystem, see module doc) {{{

    pub fn standard_field_keys(&self) -> Vec<&'static str> {
        STANDARD_FIELD_KEYS.to_vec()
    }
    pub fn searchable_fields(&self) -> Vec<&'static str> {
        STANDARD_FIELD_KEYS.to_vec()
    }
    pub fn sortable_field_keys(&self) -> Vec<&'static str> {
        STANDARD_FIELD_KEYS.to_vec()
    }
    pub fn custom_field_keys(&self) -> Vec<String> {
        self.new_api
            .lock()
            .unwrap()
            .custom_column_label_map()
            .map(|m| m.into_keys().collect())
            .unwrap_or_default()
    }
    pub fn all_field_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = STANDARD_FIELD_KEYS.iter().map(|s| s.to_string()).collect();
        keys.extend(self.custom_field_keys());
        keys
    }

    // }}}

    // --- Directory import (delegates to `adding.rs`, #219) {{{

    pub fn find_books_in_directory(
        &self,
        dirpath: &Path,
        single_book_per_directory: bool,
    ) -> Vec<Vec<PathBuf>> {
        crate::adding::find_books_in_directory(dirpath, single_book_per_directory, &[])
    }
    pub fn import_book_directory(&self, dirpath: &Path) -> Result<Option<i32>> {
        crate::adding::import_book_directory(&self.new_api, dirpath, &[])
    }
    pub fn import_book_directory_multiple(&self, dirpath: &Path) -> Result<Vec<i32>> {
        crate::adding::import_book_directory_multiple(&self.new_api, dirpath, &[])
    }
    pub fn recursive_import(
        &self,
        root: &Path,
        single_book_per_directory: bool,
    ) -> Result<Vec<i32>> {
        crate::adding::recursive_import(&self.new_api, root, single_book_per_directory, &[])
    }

    // }}}

    // --- Miscellaneous {{{

    pub fn has_book(&self, title: &str) -> bool {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let target = title.trim().to_lowercase();
        let mut stmt = match conn.prepare("SELECT title FROM books") {
            Ok(s) => s,
            Err(_) => return false,
        };
        let rows = match stmt.query_map([], |row| row.get::<_, String>(0)) {
            Ok(r) => r,
            Err(_) => return false,
        };
        let found = rows.flatten().any(|t| t.trim().to_lowercase() == target);
        found
    }

    /// The next free/incremented index for `series` across every book
    /// currently in it -- delegates to
    /// [`calibre_utils::series::get_next_series_num_for_list`] (#218)
    /// with the real per-book `series_index` values, matching
    /// upstream's `new_api.get_next_series_num_for`.
    pub fn get_next_series_num_for(&self, series: &str) -> f64 {
        let cache = self.new_api.lock().unwrap();
        let conn = cache.backend.conn.lock().unwrap();
        let mut stmt = match conn.prepare(
            "SELECT books.series_index FROM books \
             JOIN books_series_link ON books_series_link.book = books.id \
             JOIN series ON series.id = books_series_link.series \
             WHERE series.name = ?1",
        ) {
            Ok(s) => s,
            Err(_) => return 1.0,
        };
        let indices: Vec<f64> = stmt
            .query_map([series], |row| row.get::<_, f64>(0))
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default();
        calibre_utils::series::get_next_series_num_for_list(&indices)
    }

    pub fn author_sort_from_authors(&self, authors: &[String]) -> String {
        calibre_ebooks::metadata::authors::authors_to_sort_string(authors)
    }

    // }}}
}
