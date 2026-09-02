//! Port of `old_src/src/calibre/library/field_metadata.py` (issue
//! #421, first slice) -- a per-field descriptor registry (`datatype`/
//! `is_category`/`display`/etc., keyed by field name) covering every
//! standard column plus whatever custom columns exist in a given
//! library.
//!
//! # Scope: read-only, standard + custom columns only
//!
//! This first slice builds a real, populated [`FieldMetadata`] via
//! [`FieldMetadata::builtin`] (every standard column, ported verbatim
//! from upstream's `_builtin_field_metadata()`) and
//! [`FieldMetadata::from_cache`] (the same, plus one [`FieldInfo`]
//! per row of [`crate::cache::Cache::custom_column_label_map`] --
//! already-real prior art this reuses rather than re-deriving from
//! `custom_columns`/`custom_column_N` tables directly). Not ported in
//! this slice (each its own follow-up once a real consumer needs it,
//! per the issue's own text):
//! - **Mutation**: `add_user_category`/`add_search_category`/
//!   `add_grouped_search_terms`/`remove_dynamic_categories`/
//!   `remove_user_categories` -- upstream methods for building up tag
//!   -browser categories interactively; nothing in this crate
//!   currently drives them.
//! - **Hierarchical/composite columns**: `is_category` is populated
//!   accurately (mirrors each custom column's `normalized` flag, same
//!   as upstream's own `initialize_custom_columns`), but the
//!   *behavior* consumers like `calibre_srv::categories`/`notes` would
//!   need to actually walk a hierarchy isn't part of this slice --
//!   widening those consumers is explicitly a separate follow-up.
//! - **i18n**: upstream's `name` fields go through `_()`/`ngettext()`
//!   for UI translation; this port uses the plain English source
//!   strings directly, matching this crate's existing no-i18n stance
//!   elsewhere.
//! - **`tweaks`**: upstream's `timestamp`/`pubdate`/`last_modified`
//!   `display.date_format` values come from a user-configurable
//!   `tweaks` system this crate hasn't ported; the compiled-in
//!   calibre defaults (`dd MMM yyyy` / `MMM yyyy` / `dd MMM yyyy`) are
//!   used directly instead.

use std::collections::HashMap;

use anyhow::Result;
use indexmap::IndexMap;

use crate::cache::Cache;

/// Upstream's per-field `is_multiple` separator dict -- `None` means
/// the field holds a single term; `Some` means it holds a
/// delimiter-joined list, with separate separators for the three
/// contexts upstream distinguishes (the in-DB cached representation,
/// what a user types, and what's shown back to them).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IsMultiple {
    pub cache_to_list: Option<String>,
    pub ui_to_list: Option<String>,
    pub list_to_ui: Option<String>,
}

impl IsMultiple {
    fn seps(cache_to_list: &str, ui_to_list: &str, list_to_ui: &str) -> Option<Self> {
        Some(Self { cache_to_list: Some(cache_to_list.to_string()), ui_to_list: Some(ui_to_list.to_string()), list_to_ui: Some(list_to_ui.to_string()) })
    }
}

/// What kind of tag-browser entry a field is -- upstream's `kind`
/// (only `Field`/`Category` are reachable from this read-only slice;
/// `User`/`Search` exist for `add_user_category`/`add_search_category`,
/// not ported here, but are kept as real variants so a later slice
/// doesn't need to change this enum's shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    Field,
    Category,
    User,
    Search,
}

/// One field's full descriptor -- port of upstream's per-key `dict`
/// value in `FieldMetadata._tb_cats`.
#[derive(Debug, Clone)]
pub struct FieldInfo {
    /// The registry key: the bare field name for standard fields, or
    /// `#label` for custom fields (upstream's two-namespace scheme so
    /// a custom column can't collide with a standard one).
    pub key: String,
    /// The unprefixed column label.
    pub label: String,
    /// Display name (column heading), when the field has one.
    pub name: Option<String>,
    pub datatype: Option<String>,
    pub kind: FieldKind,
    pub table: Option<String>,
    pub column: Option<String>,
    pub link_column: Option<String>,
    pub category_sort: Option<String>,
    pub is_multiple: Option<IsMultiple>,
    pub is_category: bool,
    pub is_custom: bool,
    pub is_csp: bool,
    pub is_editable: bool,
    pub search_terms: Vec<String>,
    pub display: serde_json::Value,
}

fn field(
    key: &str,
    table: Option<&str>,
    column: Option<&str>,
    link_column: Option<&str>,
    category_sort: Option<&str>,
    datatype: Option<&str>,
    is_multiple: Option<IsMultiple>,
    kind: FieldKind,
    name: Option<&str>,
    search_terms: &[&str],
    is_category: bool,
    is_csp: bool,
) -> FieldInfo {
    FieldInfo {
        key: key.to_string(),
        label: key.to_string(),
        name: name.map(str::to_string),
        datatype: datatype.map(str::to_string),
        kind,
        table: table.map(str::to_string),
        column: column.map(str::to_string),
        link_column: link_column.map(str::to_string),
        category_sort: category_sort.map(str::to_string),
        is_multiple,
        is_category,
        is_custom: false,
        is_csp,
        is_editable: true,
        search_terms: search_terms.iter().map(|s| s.to_string()).collect(),
        display: serde_json::json!({}),
    }
}

/// Port of `_builtin_field_metadata()` -- every standard column, in
/// upstream's own order (the order they'd appear in a tag browser).
fn builtin_fields() -> Vec<FieldInfo> {
    use FieldKind::{Category, Field};
    vec![
        field("authors", Some("authors"), Some("name"), Some("author"), Some("sort"), Some("text"), IsMultiple::seps(",", "&", " & "), Field, Some("Authors"), &["authors", "author"], true, false),
        field("languages", Some("languages"), Some("lang_code"), Some("lang_code"), Some("lang_code"), Some("text"), IsMultiple::seps(",", ",", ", "), Field, Some("Languages"), &["languages", "language"], true, false),
        field("series", Some("series"), Some("name"), Some("series"), Some("(title_sort(name))"), Some("series"), None, Field, Some("Series"), &["series"], true, false),
        field("formats", None, None, None, None, Some("text"), IsMultiple::seps(",", ",", ", "), Field, Some("Formats"), &["formats", "format"], true, false),
        field("publisher", Some("publishers"), Some("name"), Some("publisher"), Some("name"), Some("text"), None, Field, Some("Publisher"), &["publisher"], true, false),
        field("rating", Some("ratings"), Some("rating"), Some("rating"), Some("rating"), Some("rating"), None, Field, Some("Rating"), &["rating"], true, false),
        field("news", Some("news"), Some("name"), None, Some("name"), None, None, Category, Some("News"), &[], true, false),
        field("tags", Some("tags"), Some("name"), Some("tag"), Some("name"), Some("text"), IsMultiple::seps(",", ",", ", "), Field, Some("Tags"), &["tags", "tag"], true, false),
        field("identifiers", None, None, None, None, Some("text"), IsMultiple::seps(",", ",", ", "), Field, Some("Identifiers"), &["identifiers", "identifier", "isbn"], true, true),
        field("author_sort", None, None, None, None, Some("text"), None, Field, Some("Author sort"), &["author_sort"], false, false),
        field("au_map", None, None, None, None, Some("text"), Some(IsMultiple { cache_to_list: Some(",".to_string()), ui_to_list: None, list_to_ui: None }), Field, None, &[], false, false),
        field("comments", None, None, None, None, Some("text"), None, Field, Some("Comments"), &["comments", "comment"], false, false),
        field("cover", None, None, None, None, Some("int"), None, Field, Some("Cover"), &["cover"], false, false),
        field("id", None, None, None, None, Some("int"), None, Field, Some("Id"), &["id"], false, false),
        field("last_modified", None, None, None, None, Some("datetime"), None, Field, Some("Modified"), &["last_modified"], false, false),
        field("ondevice", None, None, None, None, Some("text"), None, Field, Some("On device"), &["ondevice"], false, false),
        field("path", None, None, None, None, Some("text"), None, Field, Some("Path"), &[], false, false),
        field("pubdate", None, None, None, None, Some("datetime"), None, Field, Some("Published"), &["pubdate"], false, false),
        field("marked", None, None, None, None, Some("text"), None, Field, None, &["marked"], false, false),
        field("in_tag_browser", None, None, None, None, Some("text"), None, Field, None, &["in_tag_browser"], false, false),
        field("series_index", None, None, None, None, Some("float"), None, Field, None, &["series_index"], false, false),
        field("series_sort", None, None, None, None, Some("text"), None, Field, Some("Series sort"), &["series_sort"], false, false),
        field("sort", None, None, None, None, Some("text"), None, Field, Some("Title sort"), &["title_sort"], false, false),
        field("size", None, None, None, None, Some("float"), None, Field, Some("Size"), &["size"], false, false),
        field("timestamp", None, None, None, None, Some("datetime"), None, Field, Some("Date"), &["date"], false, false),
        field("title", None, None, None, None, Some("text"), None, Field, Some("Title"), &["title"], false, false),
        field("uuid", None, None, None, None, Some("text"), None, Field, None, &["uuid"], false, false),
    ]
}

/// Compiled-in defaults for the three date fields' `display.date_format`
/// -- see the module doc's `tweaks` disclosure.
fn default_date_format(key: &str) -> Option<&'static str> {
    match key {
        "timestamp" | "last_modified" => Some("dd MMM yyyy"),
        "pubdate" => Some("MMM yyyy"),
        _ => None,
    }
}

/// Real, tested per-field descriptor registry -- see the module doc.
pub struct FieldMetadata {
    fields: IndexMap<String, FieldInfo>,
    search_term_map: HashMap<String, String>,
    custom_label_to_key_map: HashMap<String, String>,
}

impl FieldMetadata {
    /// Every standard column, no custom columns -- for callers with
    /// no open library (or who only care about standard fields).
    pub fn builtin() -> Self {
        let mut fields = IndexMap::new();
        let mut search_term_map = HashMap::new();
        for mut info in builtin_fields() {
            if let Some(fmt) = default_date_format(&info.key) {
                info.display = serde_json::json!({"date_format": fmt});
            }
            for term in &info.search_terms {
                search_term_map.insert(term.clone(), info.key.clone());
            }
            fields.insert(info.key.clone(), info);
        }
        Self { fields, search_term_map, custom_label_to_key_map: HashMap::new() }
    }

    /// [`FieldMetadata::builtin`] plus one [`FieldInfo`] per custom
    /// column in `cache`'s library (port of upstream's
    /// `initialize_custom_columns`' `add_custom_field` loop, driven
    /// by the same `custom_column_label_map` data this crate's
    /// `cache.rs` already reads for other purposes).
    pub fn from_cache(cache: &Cache) -> Result<Self> {
        let mut fm = Self::builtin();
        let custom_columns = cache.custom_column_label_map()?;
        let mut labels: Vec<&String> = custom_columns.keys().collect();
        labels.sort();
        for label in labels {
            let v = &custom_columns[label];
            let datatype = v["datatype"].as_str().unwrap_or("text").to_string();
            let name = v["name"].as_str().unwrap_or(label).to_string();
            let num = v["num"].as_i64().unwrap_or_default();
            let editable = v["editable"].as_bool().unwrap_or(true);
            let normalized = v["normalized"].as_bool().unwrap_or(false);
            let is_multiple_flag = v["is_multiple"].as_bool().unwrap_or(false);
            let display = v["display"].clone();

            let is_multiple = if !is_multiple_flag {
                None
            } else if display.get("is_names").and_then(|v| v.as_bool()).unwrap_or(false) {
                IsMultiple::seps("|", "&", " & ")
            } else if datatype == "composite" {
                IsMultiple::seps(",", ",", ", ")
            } else {
                IsMultiple::seps("|", ",", ", ")
            };

            let key = format!("#{label}");
            let table_name = format!("custom_column_{num}");
            let info = FieldInfo {
                key: key.clone(),
                label: label.clone(),
                name: Some(name),
                datatype: Some(datatype),
                kind: FieldKind::Field,
                table: Some(table_name),
                column: Some("value".to_string()),
                link_column: Some("value".to_string()),
                category_sort: Some("value".to_string()),
                is_multiple,
                is_category: normalized,
                is_custom: true,
                is_csp: false,
                is_editable: editable,
                search_terms: vec![key.clone()],
                display,
            };
            fm.search_term_map.insert(key.clone(), key.clone());
            fm.custom_label_to_key_map.insert(label.clone(), key.clone());
            fm.fields.insert(key, info);
        }
        Ok(fm)
    }

    pub fn get(&self, key: &str) -> Option<&FieldInfo> {
        if key == "title_sort" {
            return self.fields.get("sort");
        }
        self.fields.get(key)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        key == "title_sort" || self.fields.contains_key(key)
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.fields.keys().map(String::as_str)
    }

    pub fn is_custom_field(key: &str) -> bool {
        key.starts_with('#')
    }

    pub fn standard_field_keys(&self) -> Vec<&str> {
        self.fields.values().filter(|f| f.kind == FieldKind::Field && !f.is_custom).map(|f| f.key.as_str()).collect()
    }

    pub fn custom_field_keys(&self, include_composites: bool) -> Vec<&str> {
        self.fields
            .values()
            .filter(|f| f.kind == FieldKind::Field && f.is_custom && (include_composites || f.datatype.as_deref() != Some("composite")))
            .map(|f| f.key.as_str())
            .collect()
    }

    pub fn all_field_keys(&self) -> Vec<&str> {
        self.fields.values().filter(|f| f.kind == FieldKind::Field).map(|f| f.key.as_str()).collect()
    }

    pub fn label_to_key(&self, label: &str) -> Option<&str> {
        self.custom_label_to_key_map.get(label).map(String::as_str)
    }

    pub fn search_term_to_field_key<'a>(&'a self, term: &'a str) -> &'a str {
        self.search_term_map.get(term).map(String::as_str).unwrap_or(term)
    }

    pub fn searchable_fields(&self) -> Vec<&str> {
        self.fields.values().filter(|f| f.kind == FieldKind::Field && !f.search_terms.is_empty()).map(|f| f.key.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_covers_every_standard_column_with_accurate_is_category_and_datatype() {
        let fm = FieldMetadata::builtin();
        let authors = fm.get("authors").unwrap();
        assert!(authors.is_category);
        assert_eq!(authors.datatype.as_deref(), Some("text"));
        assert_eq!(authors.is_multiple.as_ref().unwrap().list_to_ui.as_deref(), Some(" & "));

        let title = fm.get("title").unwrap();
        assert!(!title.is_category);
        assert_eq!(title.datatype.as_deref(), Some("text"));

        let news = fm.get("news").unwrap();
        assert_eq!(news.kind, FieldKind::Category);
        assert!(news.is_category);
        assert!(news.datatype.is_none());
    }

    #[test]
    fn title_sort_is_an_alias_for_sort() {
        let fm = FieldMetadata::builtin();
        assert!(fm.contains_key("title_sort"));
        assert_eq!(fm.get("title_sort").unwrap().key, "sort");
    }

    #[test]
    fn date_fields_get_the_compiled_in_default_display_format() {
        let fm = FieldMetadata::builtin();
        assert_eq!(fm.get("timestamp").unwrap().display["date_format"], "dd MMM yyyy");
        assert_eq!(fm.get("pubdate").unwrap().display["date_format"], "MMM yyyy");
        assert_eq!(fm.get("last_modified").unwrap().display["date_format"], "dd MMM yyyy");
    }

    #[test]
    fn standard_field_keys_excludes_no_custom_fields_when_there_are_none() {
        let fm = FieldMetadata::builtin();
        assert_eq!(fm.standard_field_keys().len(), fm.all_field_keys().len());
        assert!(fm.custom_field_keys(true).is_empty());
    }

    #[test]
    fn from_cache_adds_a_real_custom_column_with_accurate_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.add_custom_column("shelf", "Shelf", "text", false).unwrap();

        let fm = FieldMetadata::from_cache(&cache).unwrap();
        let info = fm.get("#shelf").expect("custom column should be registered under its #-prefixed key");
        assert!(info.is_custom);
        assert_eq!(info.label, "shelf");
        assert_eq!(info.name.as_deref(), Some("Shelf"));
        assert_eq!(info.datatype.as_deref(), Some("text"));
        assert!(!info.is_category, "non-normalized column should not be a tag-browser category");
        assert!(info.is_multiple.is_none(), "is_multiple=false column should have no separator dict");
        assert_eq!(FieldMetadata::is_custom_field("#shelf"), true);
        assert_eq!(FieldMetadata::is_custom_field("title"), false);
        assert_eq!(fm.label_to_key("shelf"), Some("#shelf"));
        assert!(fm.custom_field_keys(true).contains(&"#shelf"));
        assert!(!fm.standard_field_keys().contains(&"#shelf"));
    }

    #[test]
    fn from_cache_marks_a_normalized_multi_valued_custom_column_as_a_category() {
        // `Cache::add_custom_column` always inserts `normalized = 0`
        // and rejects `is_multiple=true` text columns outright (it
        // has no normalized/tag-like custom column support yet), so
        // this test sets the row directly to exercise `from_cache`'s
        // own `normalized`/`is_multiple` -> `is_category`/separator-dict
        // mapping against real `custom_columns` table data either way.
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        cache.add_custom_column("mytags", "My Tags", "text", false).unwrap();
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE custom_columns SET normalized = 1, is_multiple = 1 WHERE label = 'mytags'", []).unwrap();
        }

        let fm = FieldMetadata::from_cache(&cache).unwrap();
        let info = fm.get("#mytags").unwrap();
        assert!(info.is_category, "normalized column should be a tag-browser category");
        let seps = info.is_multiple.as_ref().expect("is_multiple=true column should have a separator dict");
        assert_eq!(seps.list_to_ui.as_deref(), Some(", "));
    }

    #[test]
    fn search_term_to_field_key_resolves_builtin_aliases() {
        let fm = FieldMetadata::builtin();
        assert_eq!(fm.search_term_to_field_key("author"), "authors");
        assert_eq!(fm.search_term_to_field_key("isbn"), "identifiers");
        assert_eq!(fm.search_term_to_field_key("nonexistent_term"), "nonexistent_term");
    }
}
