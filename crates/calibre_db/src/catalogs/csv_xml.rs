//! Port of `old_src/src/calibre/library/catalogs/csv_xml.py`'s `CSV_XML`
//! catalog generator -- renders a book list to a flat CSV file or a small
//! `<calibredb><record>...</record></calibredb>` XML document.
//!
//! See `crates/calibre_db/src/catalogs/mod.rs`'s module doc for the
//! disclosed simplifications shared by every generator in this module
//! (no live search-view state, no `field_metadata` subsystem, no
//! on-device tracking). This file additionally simplifies:
//!
//! - **XML building**: upstream uses `lxml.builder.E` + `lxml.etree`.
//!   This crate has no dependency on an XML-writing library from
//!   `calibre_db` (only `calibre_ebooks`, a dependency of a dependency,
//!   has one, and it's shaped for namespaced OEB documents, not this
//!   flat un-namespaced record format), so this file has its own small
//!   private element tree + pretty-printer -- not shared with any other
//!   module, matching this crate's established "duplicate a small
//!   per-file helper rather than force a shared abstraction" convention.
//! - **Custom-column values are always scalar strings.** Upstream's
//!   `#`-prefixed fields can hold a list (joined with `" & "` or `", "`
//!   depending on `field_metadata[field]['display']['is_names']`) since
//!   Calibre supports multi-valued custom columns. This crate's custom
//!   columns (`Cache::get_custom_column_value`) store one value per book
//!   per column -- there is no multi-valued custom-column storage to
//!   join here, so the `is_names`/list-join branch is dead code in this
//!   port and isn't reproduced.
//! - **`isoformat` isn't re-applied to already-fetched date strings.**
//!   `Cache::get_data_as_dict`'s `timestamp`/`pubdate` values are already
//!   ISO-8601-ish strings (or, with `convert_to_local_tz`, RFC3339); this
//!   port uses that string directly rather than re-parsing and
//!   re-formatting it a second time.

use std::path::Path;

use regex::Regex;
use serde_json::Value;

use crate::cache::Cache;
use crate::catalogs::{get_output_fields, CatalogError, Result};

/// Port of `CSV_XML.cli_options` + the subset of `opts` its `run` method
/// reads. `sort_by`/`ids`/`search_text` resolution to a final `ids` list
/// is the caller's job (see the module doc) -- by the time this reaches
/// [`run`], any search has already been turned into an explicit id list.
#[derive(Debug, Clone)]
pub struct CsvXmlOptions {
    /// `--fields`: `"all"` or a comma-separated field list.
    pub fields: String,
    /// `--sort-by`: a field to sort ascending by, or `None` for
    /// whatever order [`Cache::get_data_as_dict`] returns.
    pub sort_by: Option<String>,
    /// The specific book ids to export, or `None` for every book.
    pub ids: Option<Vec<i32>>,
    /// `opts.catalog_title` -- only meaningful for the XML root element.
    pub catalog_title: Option<String>,
    /// Whether a device is currently connected -- see the module doc's
    /// "No on-device tracking" note.
    pub is_device_connected: bool,
    /// Upstream's `current_library_name()` -- this crate has no global
    /// "current library" singleton, so the caller resolves and passes
    /// it explicitly (typically `library_path.file_name()`).
    pub current_library: String,
}

impl Default for CsvXmlOptions {
    fn default() -> Self {
        CsvXmlOptions {
            fields: "all".to_string(),
            sort_by: None,
            ids: None,
            catalog_title: None,
            is_device_connected: false,
            current_library: String::new(),
        }
    }
}

fn fetch_rows(db: &Cache, opts: &CsvXmlOptions) -> Result<Vec<Value>> {
    let ids: Option<std::collections::HashSet<i32>> =
        opts.ids.as_ref().map(|v| v.iter().copied().collect());
    let mut rows = db
        .get_data_as_dict(None, false, ids.as_ref(), true)
        .map_err(CatalogError::Db)?;
    if let Some(sort_by) = &opts.sort_by {
        rows.sort_by(|a, b| compare_by_field(a, b, sort_by));
    }
    Ok(rows)
}

fn compare_by_field(a: &Value, b: &Value, field: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a.get(field), b.get(field)) {
        (Some(Value::Number(x)), Some(Value::Number(y))) => {
            x.as_f64().unwrap_or(0.0).partial_cmp(&y.as_f64().unwrap_or(0.0)).unwrap_or(Ordering::Equal)
        }
        (Some(Value::String(x)), Some(Value::String(y))) => x.cmp(y),
        (Some(x), None) | (Some(x), Some(Value::Null)) if !x.is_null() => Ordering::Less,
        (None, Some(y)) | (Some(Value::Null), Some(y)) if !y.is_null() => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

/// Port of `CSV_XML.run`: dispatch to [`generate_csv`] or [`generate_xml`]
/// based on `path_to_output`'s extension, matching upstream's own
/// `path_to_output.rpartition('.')[2]` dispatch.
pub fn run(db: &Cache, path_to_output: &Path, opts: &CsvXmlOptions) -> Result<()> {
    let ext = path_to_output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "xml" => generate_xml(db, path_to_output, opts),
        _ => generate_csv(db, path_to_output, opts),
    }
}

fn custom_label(field: &str) -> Option<&str> {
    field.strip_prefix('#')
}

fn is_rating_field(db: &Cache, field: &str) -> Result<bool> {
    if field == "rating" {
        return Ok(true);
    }
    if let Some(label) = custom_label(field) {
        let map = db.custom_column_label_map().map_err(CatalogError::Sqlite)?;
        if let Some(meta) = map.get(label) {
            return Ok(meta.get("datatype").and_then(|d| d.as_str()) == Some("rating"));
        }
    }
    Ok(false)
}

/// Narrow approximation of Python's `f'{value:.2g}'` (2 significant
/// digits, trailing zeros trimmed), scoped to the realistic rating
/// domain this is actually used for (a book rating divided by 2, so
/// `0.0..=5.0` in half-star steps) rather than a general-purpose `%g`
/// formatter.
fn format_2sig(v: f64) -> String {
    if v == 0.0 {
        return "0".to_string();
    }
    let magnitude = v.abs().log10().floor() as i32;
    let decimals = (1 - magnitude).max(0) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

fn clean_isbn(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == '-').collect()
}

fn opening_tag_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<(\w+)( |>)").unwrap())
}

/// Port of the CSV branch's HTML-to-markdown heuristic: if `item` opens
/// with an HTML tag *and* closes with that same tag at the very end of
/// the string, run it through `html2text`; otherwise leave it alone.
fn markdown_if_wrapped_in_html(item: &str) -> String {
    let Some(caps) = opening_tag_re().captures(item) else {
        return item.to_string();
    };
    let tag = &caps[1];
    let closing = format!("</{tag}>");
    if item.ends_with(&closing) {
        calibre_utils::html2text::html2text(item)
    } else {
        item.to_string()
    }
}

fn csv_field_value(
    db: &Cache,
    entry: &Value,
    field: &str,
    current_library: &str,
) -> Result<Option<String>> {
    let raw: Option<String> = if let Some(label) = custom_label(field) {
        let id = entry["id"].as_i64().unwrap_or_default() as i32;
        db.get_custom_column_value(id, label).map_err(CatalogError::Sqlite)?
    } else if field == "library_name" {
        Some(current_library.to_string())
    } else if field == "title_sort" {
        entry.get("sort").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        json_field_as_display_string(entry.get(field))
    };

    let Some(mut item) = raw else {
        return Ok(None);
    };

    if field == "formats" {
        if let Some(Value::Array(list)) = entry.get("formats") {
            let exts: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.rsplit('.').next().unwrap_or(s).to_ascii_lowercase())
                .collect();
            item = exts.join(", ");
        }
    } else if field == "authors" {
        if let Some(Value::Array(list)) = entry.get("authors") {
            let names: Vec<String> = list.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
            item = calibre_ebooks::metadata::authors_to_string(&names);
        }
    } else if field == "tags" {
        if let Some(Value::Array(list)) = entry.get("tags") {
            let tags: Vec<&str> = list.iter().filter_map(|v| v.as_str()).collect();
            item = tags.join(", ");
        }
    } else if field == "isbn" {
        item = clean_isbn(&item);
    } else if field == "comments" {
        item = item.replace("\r\n", " ").replace('\n', " ");
    } else if is_rating_field(db, field)? {
        if let Some(n) = entry.get(field).and_then(|v| v.as_f64()) {
            if n != 0.0 {
                item = format_2sig(n / 2.0);
            }
        }
    }

    Ok(Some(markdown_if_wrapped_in_html(&item)))
}

fn json_field_as_display_string(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(items)) => Some(
            items
                .iter()
                .map(|x| json_field_as_display_string(Some(x)).unwrap_or_default())
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Some(other) => Some(other.to_string()),
    }
}

/// Port of `CSV_XML.run`'s `self.fmt == 'csv'` branch.
pub fn generate_csv(db: &Cache, path_to_output: &Path, opts: &CsvXmlOptions) -> Result<()> {
    let fields = get_output_fields(db, &opts.fields, opts.is_device_connected)?;
    let rows = fetch_rows(db, opts)?;

    let mut out = String::new();
    out.push('\u{feff}');
    out.push_str(&fields.join(","));
    out.push('\n');

    for entry in &rows {
        let mut cells = Vec::with_capacity(fields.len());
        for field in &fields {
            let value = csv_field_value(db, entry, field, &opts.current_library)?;
            let text = value.unwrap_or_default();
            cells.push(format!("\"{}\"", text.replace('"', "\"\"")));
        }
        out.push_str(&cells.join(","));
        out.push('\n');
    }

    std::fs::write(path_to_output, out).map_err(|e| CatalogError::Db(e.into()))?;
    Ok(())
}

#[derive(Debug, Clone)]
struct XmlNode {
    name: String,
    attrs: Vec<(String, String)>,
    text: Option<String>,
    children: Vec<XmlNode>,
}

impl XmlNode {
    fn el(name: impl Into<String>) -> Self {
        XmlNode { name: name.into(), attrs: Vec::new(), text: None, children: Vec::new() }
    }

    fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    fn with_attr(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.attrs.push((name.into(), value.into()));
        self
    }

    fn push_child(&mut self, child: XmlNode) {
        self.children.push(child);
    }

    fn write(&self, out: &mut String, depth: usize) {
        let XmlNode { name, attrs, text, children } = self;
        let indent = "  ".repeat(depth);
        out.push_str(&indent);
        out.push('<');
        out.push_str(name);
        for (k, v) in attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(&escape_attr(v));
            out.push('"');
        }
        if text.is_none() && children.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push('>');
        if let Some(t) = text {
            out.push_str(&escape_text(t));
        }
        if !children.is_empty() {
            out.push('\n');
            for child in children {
                child.write(out, depth + 1);
            }
            out.push_str(&indent);
        }
        out.push_str("</");
        out.push_str(name);
        out.push_str(">\n");
    }
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn escape_attr(s: &str) -> String {
    escape_text(s).replace('"', "&quot;")
}

fn str_field(entry: &Value, field: &str) -> Option<String> {
    match entry.get(field) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

/// Port of `CSV_XML.run`'s `self.fmt == 'xml'` branch.
pub fn generate_xml(db: &Cache, path_to_output: &Path, opts: &CsvXmlOptions) -> Result<()> {
    let fields = get_output_fields(db, &opts.fields, opts.is_device_connected)?;
    let rows = fetch_rows(db, opts)?;
    let field_set: std::collections::HashSet<&str> = fields.iter().map(|s| s.as_str()).collect();

    let mut root = XmlNode::el("calibredb");
    if let Some(title) = &opts.catalog_title {
        root = root.with_attr("title", title.clone());
    }

    for entry in &rows {
        let mut record = XmlNode::el("record");
        let book_id = entry["id"].as_i64().unwrap_or_default() as i32;

        for field in &fields {
            if let Some(label) = custom_label(field) {
                if let Some(val) = db.get_custom_column_value(book_id, label).map_err(CatalogError::Sqlite)? {
                    let tag = field.replace('#', "_");
                    record.push_child(XmlNode::el(tag).with_text(val));
                }
            }
        }

        for field in ["id", "uuid", "publisher", "rating", "size", "isbn", "ondevice", "identifiers"] {
            if !field_set.contains(field) {
                continue;
            }
            let Some(mut val) = str_field(entry, field) else { continue };
            if field == "rating" {
                if let Some(n) = entry.get(field).and_then(|v| v.as_f64()) {
                    if n != 0.0 {
                        val = format_2sig(n / 2.0);
                    }
                }
            }
            record.push_child(XmlNode::el(field).with_text(val));
        }

        if field_set.contains("title") {
            let title = entry.get("title").and_then(|v| v.as_str()).unwrap_or_default();
            let sort = entry.get("sort").and_then(|v| v.as_str()).unwrap_or_default();
            record.push_child(XmlNode::el("title").with_attr("sort", sort).with_text(title));
        }

        if field_set.contains("authors") {
            let sort = entry.get("author_sort").and_then(|v| v.as_str()).unwrap_or_default();
            let mut aus = XmlNode::el("authors").with_attr("sort", sort);
            if let Some(Value::Array(names)) = entry.get("authors") {
                for name in names.iter().filter_map(|v| v.as_str()) {
                    aus.push_child(XmlNode::el("author").with_text(name));
                }
            }
            record.push_child(aus);
        }

        for field in ["timestamp", "pubdate"] {
            if !field_set.contains(field) {
                continue;
            }
            if let Some(val) = entry.get(field).and_then(|v| v.as_str()) {
                record.push_child(XmlNode::el(field).with_text(val));
            }
        }

        if field_set.contains("tags") {
            if let Some(Value::Array(tags)) = entry.get("tags") {
                if !tags.is_empty() {
                    let mut node = XmlNode::el("tags");
                    for tag in tags.iter().filter_map(|v| v.as_str()) {
                        node.push_child(XmlNode::el("tag").with_text(tag));
                    }
                    record.push_child(node);
                }
            }
        }

        if field_set.contains("comments") {
            if let Some(c) = entry.get("comments").and_then(|v| v.as_str()) {
                if !c.is_empty() {
                    record.push_child(XmlNode::el("comments").with_text(c));
                }
            }
        }

        if field_set.contains("series") {
            if let Some(series) = entry.get("series").and_then(|v| v.as_str()) {
                if !series.is_empty() {
                    let index = entry.get("series_index").map(|v| v.to_string()).unwrap_or_default();
                    record.push_child(XmlNode::el("series").with_attr("index", index).with_text(series));
                }
            }
        }

        if field_set.contains("languages") {
            if let Some(Value::Array(langs)) = entry.get("languages") {
                if !langs.is_empty() {
                    let joined =
                        langs.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ");
                    record.push_child(XmlNode::el("languages").with_text(joined));
                }
            }
        }

        if field_set.contains("cover") {
            if let Some(cover) = entry.get("cover").and_then(|v| v.as_str()) {
                record.push_child(XmlNode::el("cover").with_text(cover.replace(std::path::MAIN_SEPARATOR, "/")));
            }
        }

        if field_set.contains("formats") {
            if let Some(Value::Array(formats)) = entry.get("formats") {
                if !formats.is_empty() {
                    let mut node = XmlNode::el("formats");
                    for f in formats.iter().filter_map(|v| v.as_str()) {
                        node.push_child(XmlNode::el("format").with_text(f.replace(std::path::MAIN_SEPARATOR, "/")));
                    }
                    record.push_child(node);
                }
            }
        }

        if field_set.contains("library_name") {
            record.push_child(XmlNode::el("library_name").with_text(opts.current_library.clone()));
        }

        root.push_child(record);
    }

    let mut out = String::from("<?xml version='1.0' encoding='utf-8'?>\n");
    root.write(&mut out, 0);
    std::fs::write(path_to_output, out).map_err(|e| CatalogError::Db(e.into()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;
    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    fn write_temp_file(dir: &Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn add_test_book(dir: &Path, cache: &Cache, title: &str, authors: &[&str]) -> i32 {
        let source = write_temp_file(dir, &format!("{title}.epub"), b"x");
        let mut meta = MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = authors.iter().map(|s| s.to_string()).collect();
        cache.add_book(&source, &meta).unwrap()
    }

    /// `Cache::set_field` doesn't support `"isbn"` (not one of the
    /// upstream-mirrored writable fields it lists), so tests that need
    /// one write it directly, matching the same direct-`conn` pattern
    /// `cache.rs`'s own test module uses for columns without a setter.
    fn set_isbn(cache: &Cache, book_id: i32, isbn: &str) {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("UPDATE books SET isbn = ?1 WHERE id = ?2", (isbn, book_id)).unwrap();
    }

    #[test]
    fn csv_output_has_a_bom_header_row_and_one_quoted_row_per_book() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "My Book", &["Alice", "Bob"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions {
            fields: "title,authors".to_string(),
            current_library: "MyLib".to_string(),
            ..Default::default()
        };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.starts_with('\u{feff}'));
        let mut lines = text.trim_start_matches('\u{feff}').lines();
        assert_eq!(lines.next().unwrap(), "title,authors");
        assert_eq!(lines.next().unwrap(), "\"My Book\",\"Alice & Bob\"");
    }

    #[test]
    fn csv_isbn_is_stripped_to_digits_x_and_hyphens() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T", &["A"]);
        set_isbn(&cache, id, "ISBN: 978-0-13X (paperback)");

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions { fields: "isbn".to_string(), ..Default::default() };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert_eq!(row, "\"978-0-13X\"");
    }

    #[test]
    fn csv_missing_field_renders_as_an_empty_quoted_cell() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "T", &["A"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions { fields: "series".to_string(), ..Default::default() };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert_eq!(text.lines().nth(1).unwrap(), "\"\"");
    }

    #[test]
    fn xml_output_wraps_records_in_a_calibredb_root_with_title_and_authors() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "My Book", &["Alice", "Bob"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.xml");
        let opts = CsvXmlOptions {
            fields: "title,authors".to_string(),
            catalog_title: Some("Test Catalog".to_string()),
            ..Default::default()
        };
        generate_xml(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.contains("<calibredb title=\"Test Catalog\">"));
        assert!(text.contains("<title sort="));
        assert!(text.contains(">My Book</title>"));
        assert!(text.contains("<author>Alice</author>"));
        assert!(text.contains("<author>Bob</author>"));
    }

    #[test]
    fn xml_omits_fields_not_in_the_requested_field_list() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "T", &["A"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.xml");
        let opts = CsvXmlOptions { fields: "title".to_string(), ..Default::default() };
        generate_xml(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(!text.contains("<authors"));
    }

    #[test]
    fn run_dispatches_on_the_output_extension() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "T", &["A"]);
        let out_dir = tempdir().unwrap();

        let csv_path = out_dir.path().join("out.csv");
        run(&cache, &csv_path, &CsvXmlOptions { fields: "title".to_string(), ..Default::default() }).unwrap();
        assert!(std::fs::read_to_string(&csv_path).unwrap().starts_with('\u{feff}'));

        let xml_path = out_dir.path().join("out.xml");
        run(&cache, &xml_path, &CsvXmlOptions { fields: "title".to_string(), ..Default::default() }).unwrap();
        assert!(std::fs::read_to_string(&xml_path).unwrap().starts_with("<?xml"));
    }

    #[test]
    fn sort_by_orders_rows_ascending() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "Zebra", &["A"]);
        add_test_book(dir.path(), &cache, "Apple", &["A"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions {
            fields: "title".to_string(),
            sort_by: Some("title".to_string()),
            ..Default::default()
        };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        let rows: Vec<&str> = text.trim_start_matches('\u{feff}').lines().skip(1).collect();
        assert_eq!(rows, vec!["\"Apple\"", "\"Zebra\""]);
    }

    #[test]
    fn custom_field_appears_as_a_hash_prefixed_column() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T", &["A"]);
        cache.add_custom_column("genre", "Genre", "text", false).unwrap();
        cache.set_custom_column_value(id, "genre", "Sci-Fi").unwrap();

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions { fields: "#genre".to_string(), ..Default::default() };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        let mut lines = text.trim_start_matches('\u{feff}').lines();
        assert_eq!(lines.next().unwrap(), "#genre");
        assert_eq!(lines.next().unwrap(), "\"Sci-Fi\"");
    }

    #[test]
    fn html_wrapped_comment_is_converted_to_markdown_in_csv() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T", &["A"]);
        cache.set_field(id, "comments", "<p>Hello <b>world</b></p>").unwrap();

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.csv");
        let opts = CsvXmlOptions { fields: "comments".to_string(), ..Default::default() };
        generate_csv(&cache, &out_path, &opts).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        let row = text.lines().nth(1).unwrap();
        assert!(!row.contains("<p>"), "expected HTML to be converted, got: {row}");
    }
}
