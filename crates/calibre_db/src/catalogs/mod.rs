//! Port of `old_src/src/calibre/library/catalogs/` (issue #57) -- calibre's
//! catalog generators, which render a library's book list out to BibTeX,
//! CSV/XML, or an EPUB/MOBI/AZW3 "browsable catalog" ebook.
//!
//! # What's here
//!
//! - [`utils`]: `NumberToText`, a small recursive number-to-English-words
//!   converter used by the EPUB/MOBI builder's series/genre sort text.
//! - [`csv_xml`]: the `CSV_XML` generator (`csv_xml.py`).
//! - The constants and shared logic below, ported from `__init__.py`
//!   (`FIELDS`/`TEMPLATE_ALLOWED_FIELDS`/three exception variants) and
//!   from `calibre.customize.CatalogPlugin`'s own base-class methods
//!   (`get_output_fields`), which every generator subclass inherits
//!   unchanged rather than overriding.
//!
//! The remaining files (`bibtex.py`, `epub_mobi.py`, `epub_mobi_builder.py`)
//! are follow-up work; each generator subclasses `calibre.customize.
//! CatalogPlugin`, itself a subclass of the generic `calibre.customize.
//! Plugin`. This crate doesn't yet have any plugin *registration/discovery*
//! system consuming a `CatalogPlugin`-shaped trait object
//! (`crates/calibre_db/src/cli/cmd_catalog.rs`, the one existing catalog
//! entry point, dispatches on file extension directly, not through a
//! plugin registry), so each generator is ported as a plain struct with a
//! `run`-style function rather than a trait implementation.
//!
//! # Disclosed simplifications shared by every generator
//!
//! - **No live "current search" state.** Upstream's `CatalogPlugin.
//!   search_sort_db` calls `db.search(opts.search_text)` before fetching
//!   data, narrowing a *stateful* search view on the legacy `db` object
//!   that `get_data_as_dict(ids=None)` then implicitly reads back. This
//!   crate's [`crate::cache::Cache`] has no such view state --
//!   [`crate::cache::Cache::get_data_as_dict`] takes an explicit `ids`
//!   set or exports every book. Generators here therefore expect the
//!   caller to resolve any search query to an explicit id list first
//!   (via [`crate::search::search`]) and pass it in, rather than
//!   re-implementing search resolution in this module.
//! - **No `field_metadata` subsystem.** Upstream's `db.field_metadata`
//!   dict-of-dicts (per-field `datatype`, `display` sub-keys) doesn't
//!   exist in this crate (`legacy.rs`/`search.rs` both document this).
//!   Standard fields' shapes are instead hardcoded here from
//!   [`crate::cache::Cache::get_data_as_dict`]'s own known, documented
//!   output shape; custom (`#`-prefixed) fields' `datatype` comes from
//!   [`crate::cache::Cache::custom_column_label_map`] instead.
//! - **No on-device tracking.** Upstream adds an `ondevice` value per
//!   book from `db.catalog_plugin_on_device_temp_mapping`, populated by
//!   a prior device-sync step this crate has no equivalent of yet.
//!   `is_device_connected` is accepted (it still gates whether
//!   `"ondevice"` appears in the output field list at all, matching
//!   [`get_output_fields`]'s own logic), but no generator here can
//!   populate a real device-presence value.

use std::collections::BTreeSet;

use thiserror::Error;

use crate::cache::Cache;

pub mod bibtex;
pub mod csv_xml;
pub mod epub_mobi_builder;
pub mod ncx;
pub mod opf;
pub mod output_profiles;
pub mod utils;

/// Port of `catalogs/__init__.py`'s `FIELDS` list -- the full set of
/// per-book fields a catalog can be asked to include via `--fields`.
pub const FIELDS: &[&str] = &[
    "all",
    "title",
    "title_sort",
    "author_sort",
    "authors",
    "comments",
    "cover",
    "formats",
    "id",
    "isbn",
    "library_name",
    "ondevice",
    "pubdate",
    "publisher",
    "rating",
    "series_index",
    "series",
    "size",
    "tags",
    "timestamp",
    "uuid",
    "languages",
    "identifiers",
];

/// Port of `catalogs/__init__.py`'s `TEMPLATE_ALLOWED_FIELDS` list -- the
/// fields usable in a BibTeX/EPUB template field-reference.
pub const TEMPLATE_ALLOWED_FIELDS: &[&str] = &[
    "author_sort",
    "authors",
    "id",
    "isbn",
    "pubdate",
    "title_sort",
    "publisher",
    "series_index",
    "series",
    "tags",
    "timestamp",
    "title",
    "uuid",
];

/// Port of `catalogs/__init__.py`'s three catalog-generator exceptions,
/// plus a wrapped [`crate::cache::Cache`] error variant every generator
/// needs (upstream just lets a database error propagate as whatever
/// exception the DB layer itself raises).
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("author_sort mismatch: {0}")]
    AuthorSortMismatch(String),
    #[error("empty catalog")]
    EmptyCatalog,
    #[error("invalid --fields specified: {0}")]
    InvalidGenresSourceField(String),
    #[error(transparent)]
    Db(#[from] anyhow::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

pub type Result<T> = std::result::Result<T, CatalogError>;

fn field_sorter(key: &str) -> String {
    match key.strip_prefix('#') {
        Some(rest) => format!("~{rest}"),
        None => key.to_string(),
    }
}

/// The standard (non-custom) fields every generator can emit, mirroring
/// `CatalogPlugin.get_output_fields`'s own hardcoded `all_std_fields` set.
const STANDARD_FIELDS: &[&str] = &[
    "author_sort",
    "authors",
    "comments",
    "cover",
    "formats",
    "id",
    "isbn",
    "library_name",
    "ondevice",
    "pubdate",
    "publisher",
    "rating",
    "series_index",
    "series",
    "size",
    "tags",
    "timestamp",
    "title_sort",
    "title",
    "uuid",
    "languages",
    "identifiers",
];

/// Port of `CatalogPlugin.get_output_fields`: resolve the `--fields`
/// option (or, absent one, every standard + custom field) into the
/// actual ordered list of fields a catalog should emit.
///
/// `fields_arg` is `"all"` or a comma-separated field list (upstream's
/// `opts.fields`). Custom fields are read from `db`'s own custom-column
/// table and rendered `#`-prefixed, matching upstream's field-naming
/// convention (`crate::cache::Cache::custom_column_label_map`'s own keys
/// are the bare DB `label` column, without the `#`).
pub fn get_output_fields(db: &Cache, fields_arg: &str, is_device_connected: bool) -> Result<Vec<String>> {
    let custom_columns = db.custom_column_label_map()?;

    let mut all_fields: BTreeSet<String> = STANDARD_FIELDS.iter().map(|s| s.to_string()).collect();
    for (label, meta) in &custom_columns {
        let field = format!("#{label}");
        all_fields.insert(field.clone());
        if meta.get("datatype").and_then(|d| d.as_str()) == Some("series") {
            all_fields.insert(format!("{field}_index"));
        }
    }

    let mut fields: Vec<String> = if fields_arg != "all" {
        let requested: BTreeSet<String> = fields_arg.split(',').map(|s| s.trim().to_string()).collect();
        let invalid: Vec<&String> = requested.difference(&all_fields).collect();
        if !invalid.is_empty() {
            let mut invalid_sorted: Vec<String> = invalid.into_iter().cloned().collect();
            invalid_sorted.sort();
            return Err(CatalogError::InvalidGenresSourceField(invalid_sorted.join(", ")));
        }
        fields_arg
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|f| all_fields.contains(f))
            .collect()
    } else {
        let mut sorted: Vec<String> = all_fields.iter().cloned().collect();
        sorted.sort_by_key(|k| field_sorter(k));
        sorted
    };

    if !is_device_connected {
        fields.retain(|f| f != "ondevice");
    }

    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    #[test]
    fn all_returns_every_standard_field_sorted_alphabetically_without_ondevice() {
        let (_dir, cache) = open_test_cache();
        let fields = get_output_fields(&cache, "all", false).unwrap();
        assert!(!fields.contains(&"ondevice".to_string()));
        assert!(fields.contains(&"title".to_string()));
        let mut sorted = fields.clone();
        sorted.sort();
        assert_eq!(fields, sorted);
    }

    #[test]
    fn all_includes_ondevice_only_when_a_device_is_connected() {
        let (_dir, cache) = open_test_cache();
        let fields = get_output_fields(&cache, "all", true).unwrap();
        assert!(fields.contains(&"ondevice".to_string()));
    }

    #[test]
    fn explicit_field_list_is_returned_in_the_requested_order() {
        let (_dir, cache) = open_test_cache();
        let fields = get_output_fields(&cache, "authors,title", false).unwrap();
        assert_eq!(fields, vec!["authors".to_string(), "title".to_string()]);
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        let (_dir, cache) = open_test_cache();
        let err = get_output_fields(&cache, "title,not_a_real_field", false).unwrap_err();
        assert!(matches!(err, CatalogError::InvalidGenresSourceField(_)));
    }

    #[test]
    fn custom_columns_are_included_hash_prefixed_when_requesting_all() {
        let (_dir, cache) = open_test_cache();
        cache.add_custom_column("genre", "Genre", "text", false).unwrap();
        let fields = get_output_fields(&cache, "all", false).unwrap();
        assert!(fields.contains(&"#genre".to_string()));
    }

    #[test]
    fn series_typed_custom_columns_gain_a_synthetic_index_field() {
        let (_dir, cache) = open_test_cache();
        cache.add_custom_column("myseries", "My Series", "series", false).unwrap();
        let fields = get_output_fields(&cache, "all", false).unwrap();
        assert!(fields.contains(&"#myseries".to_string()));
        assert!(fields.contains(&"#myseries_index".to_string()));
    }
}
