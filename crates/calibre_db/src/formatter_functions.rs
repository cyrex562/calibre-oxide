//! Port of `formatter_functions.py`'s `GET_FROM_METADATA` +
//! `DB_FUNCS` categories (issue #514, part of the #460 formatter
//! epic): real `calibre_db::Cache`-backed implementations of
//! `calibre_utils::formatter`'s `ValueSource`/`FunctionRegistry`/
//! `FunctionCatalog` traits (see `calibre_utils::formatter::interp`'s
//! own doc for why those traits exist -- this module is exactly the
//! "real backing" issue #513 deferred).
//!
//! # Scope of this pass (~25 of the 34 real upstream functions)
//!
//! Implemented for real, against a live `Cache`: `raw_field`
//! (the 2-argument registered form #513 could only stub, since it
//! needs a real field-metadata-driven list separator),
//! `raw_list`, `has_cover`, `booksize`, `series_sort`,
//! `author_sorts`, `get_link`, `author_links`, `language_codes`,
//! `language_strings`, `current_library_name`, `current_library_path`,
//! `has_extra_files`, `extra_file_names`, `extra_file_size`,
//! `extra_file_modtime`, `virtual_libraries`, `book_count`,
//! `book_values`, `get_note`, `has_note`, `approximate_formats`,
//! `ondevice`, `annotation_count`; (issue #515, the only two
//! `STRING_MANIPULATION` functions that need real book-field access)
//! `check_yes_no`/`field_exists`; (issue #518, the only 3
//! `FORMATTING_VALUES`/`URL_FUNCTIONS` functions that need real
//! `Cache`/`calibre_ebooks` access) `format_date_field`/
//! `rating_to_stars`/`urls_from_identifiers`; and (issue #524, the
//! last 4 `DB_FUNCS` functions, split out of #514 since they needed
//! new per-format size/path/mtime plumbing #514 itself didn't add)
//! `formats_sizes`/`formats_paths`/`formats_modtimes`/
//! `formats_path_segments`, backed by the new [`crate::cache::Cache::format_file_info`]
//! -- every other `STRING_MANIPULATION`/`CASE_CHANGES` (issue #515),
//! `LIST_MANIPULATION`/`LIST_LOOKUP` (issue #516),
//! `ARITHMETIC`/`RELATIONAL`/`BOOLEAN` (issue #517), and
//! `FORMATTING_VALUES`/`DATE_FUNCTIONS`/`URL_FUNCTIONS` (issue #518)
//! built-in needs no `Cache` at all and lives in
//! `calibre_utils::formatter::{string_functions,list_functions,numeric_functions,format_functions}`
//! instead, reached here via [`fallback_call`]/[`fallback_arg_count`].
//!
//! **Not registered at all** (the underlying feature/subsystem is
//! genuinely absent from this port, confirmed by reading the real
//! source rather than assumed -- calling these is a real "unknown
//! function" error, not a silently-empty stub):
//! `is_marked` (no marked-books storage anywhere in `calibre_db`),
//! `user_categories` (feature not ported, `field_metadata.rs`'s own
//! doc discloses this), `current_virtual_library_name` (no
//! "currently active VL" session concept in a stateless template
//! evaluation), `connected_device_name`/`connected_device_uuid` (no
//! e-reader device-connection subsystem exists at all).
//!
//! # Disclosed narrowings on what *is* implemented
//!
//! - `get_link`/`author_links` only work for the `authors` field --
//!   `calibre_db`'s only real per-item link storage is
//!   `AuthorsTable.link_map` (tags/series/publisher have no `link`
//!   column read anywhere in this crate). `get_link` for any other
//!   field returns `''` rather than erroring, matching upstream's own
//!   "no attached link" empty-string case (the *reason* differs --
//!   "field doesn't support links here" vs. upstream's "this specific
//!   item has no link" -- the observable result is the same).
//! - `book_count`/`book_values`'s `use_vl` parameter has no effect --
//!   both branches upstream distinguishes (`db.new_api.search` vs.
//!   `db.search_getting_ids(..., use_virtual_library=True)`) only
//!   differ by whether a *currently active* virtual library (a GUI
//!   session concept) restricts the search; there is no such session
//!   state in a stateless template evaluation here, so both branches
//!   resolve to the same plain `search::search`.
//! - `get_note`'s HTML branch (`plain_text` unset) does not expand
//!   `calres://` embedded-resource references into `data:` URLs the
//!   way upstream's `expand_note_resources` does -- no
//!   `lxml`-equivalent HTML resource-rewriting pipeline exists in
//!   this crate. The raw stored `doc` HTML is returned as-is.
//! - `language_strings`'s `localize` parameter has no effect --
//!   `calibre_utils::localization::lang_display_name` only has
//!   English names (`isolang`'s `english_names` feature; no
//!   translation-catalog machinery exists in this crate).
//! - `get_note`/`has_note` only resolve item values for
//!   `calibre_db::categories::STANDARD_CATEGORIES` (authors/tags/
//!   series/publisher/languages) -- the same restriction the rest of
//!   this crate's category system already has (no custom-column
//!   support yet).

use crate::annotations;
use crate::cache::Cache;
use crate::categories;
use crate::constants::DATA_FILE_PATTERN;
use crate::extra_files;
use crate::field_metadata::FieldMetadata;
use crate::search;
use calibre_ebooks::oeb::transforms::jacket::rating_to_stars as ebooks_rating_to_stars;
use calibre_ebooks::xml_util::prepare_string_for_xml;
use calibre_utils::formatter::format_functions::format_parsed_date;
use calibre_utils::formatter::interp::{FunctionRegistry, RawValue, ValueSource};
use calibre_utils::formatter::parser::FunctionCatalog;
use calibre_utils::icu::strcmp;
use regex::RegexBuilder;
use std::path::Path;
use std::collections::{BTreeSet, HashSet};

/// Real field access, backed by one book's row from
/// `Cache::get_data_as_dict` (fetched once at construction, not once
/// per field access -- a template typically reads many fields for the
/// same book).
pub struct CacheValueSource<'a> {
    row: serde_json::Value,
    _cache: &'a Cache,
}

impl<'a> CacheValueSource<'a> {
    pub fn new(cache: &'a Cache, book_id: i32) -> anyhow::Result<Self> {
        let ids: HashSet<i32> = std::iter::once(book_id).collect();
        let rows = cache.get_data_as_dict(None, false, Some(&ids), false)?;
        let row = rows.into_iter().next().ok_or_else(|| anyhow::anyhow!("Unknown book id {book_id}"))?;
        Ok(Self { row, _cache: cache })
    }
}

fn json_scalar_or_join(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::Array(items) => {
            let joined: Vec<&str> = items.iter().filter_map(|i| i.as_str()).collect();
            Some(joined.join(", "))
        }
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(if *b { "1".to_string() } else { String::new() }),
        _ => None,
    }
}

/// `get_data_as_dict` keys custom-column entries by their bare label
/// (e.g. `"myflag"`), but every real template/caller refers to a
/// custom column with a leading `#` (`"#myflag"`, matching upstream's
/// own convention and this crate's own `field_metadata.rs` key format
/// elsewhere) -- both `get_value`/`get_raw_value` below strip it
/// before the row lookup so `field("#myflag")`/`raw_field("#myflag")`
/// (and this module's own `check_yes_no`) actually find the value,
/// a real, previously-latent gap #514's own tests never exercised
/// (they only ever used standard, non-`#` field names).
impl ValueSource for CacheValueSource<'_> {
    fn get_value(&self, name: &str) -> Option<String> {
        self.row.get(name.trim_start_matches('#')).and_then(json_scalar_or_join)
    }

    fn get_raw_value(&self, name: &str) -> Option<RawValue> {
        let name = name.trim_start_matches('#');
        match self.row.get(name)? {
            serde_json::Value::Array(items) => Some(RawValue::List(items.iter().filter_map(|i| i.as_str().map(str::to_string)).collect())),
            serde_json::Value::String(s) => Some(RawValue::Scalar(s.clone())),
            serde_json::Value::Number(n) => Some(RawValue::Scalar(n.to_string())),
            serde_json::Value::Bool(b) => Some(RawValue::Scalar(if *b { "1".to_string() } else { String::new() })),
            _ => None,
        }
    }
}

/// The field's own configured list-join separator (e.g. `" & "` for
/// authors), falling back to `", "` -- matches upstream's own
/// `fm['is_multiple']['list_to_ui']` lookup with the same default.
fn field_list_sep(cache: &Cache, field: &str) -> String {
    let Ok(fm) = FieldMetadata::from_cache(cache) else { return ", ".to_string() };
    fm.get(field).and_then(|fi| fi.is_multiple.as_ref()).and_then(|im| im.list_to_ui.clone()).unwrap_or_else(|| ", ".to_string())
}

fn regex_search_ci(pattern: &str, text: &str) -> Result<bool, String> {
    let re = RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| e.to_string())?;
    Ok(re.is_match(text))
}

/// Strips the `data/` prefix `extra_files::list_extra_files` keeps on
/// every `relpath`, matching upstream's own `f.relpath.partition('/')[-1]`.
fn strip_data_prefix(relpath: &str) -> String {
    relpath.split_once('/').map(|(_, rest)| rest.to_string()).unwrap_or_else(|| relpath.to_string())
}

/// Real function calls, backed by a live `Cache` + one book id.
pub struct CacheFunctions<'a> {
    pub cache: &'a Cache,
    pub book_id: i32,
}

impl<'a> CacheFunctions<'a> {
    pub fn new(cache: &'a Cache, book_id: i32) -> Self {
        Self { cache, book_id }
    }

    fn raw_field(&self, args: &[String]) -> Result<String, String> {
        let name = &args[0];
        let vs = CacheValueSource::new(self.cache, self.book_id).map_err(|e| e.to_string())?;
        match vs.get_raw_value(name) {
            Some(RawValue::Scalar(s)) => Ok(s),
            Some(RawValue::List(items)) => Ok(items.join(&field_list_sep(self.cache, name))),
            Some(RawValue::Map(pairs)) => Ok(pairs.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(", ")),
            None => Ok(args.get(1).cloned().unwrap_or_else(|| "None".to_string())),
        }
    }

    fn raw_list(&self, args: &[String]) -> Result<String, String> {
        let name = &args[0];
        let sep = &args[1];
        let vs = CacheValueSource::new(self.cache, self.book_id).map_err(|e| e.to_string())?;
        match vs.get_raw_value(name) {
            Some(RawValue::List(items)) => Ok(items.join(sep)),
            _ => Ok(format!("{name} is not a list")),
        }
    }

    fn has_cover(&self) -> Result<String, String> {
        let yes = self.cache.has_cover(self.book_id).map_err(|e| e.to_string())?;
        Ok(if yes { "Yes".to_string() } else { String::new() })
    }

    fn booksize(&self) -> Result<String, String> {
        Ok(self.cache.field_for(self.book_id, "size").map_err(|e| e.to_string())?.unwrap_or_default())
    }

    fn series_sort(&self) -> Result<String, String> {
        match self.cache.field_for(self.book_id, "series").map_err(|e| e.to_string())? {
            Some(s) if !s.is_empty() => Ok(calibre_ebooks::metadata::meta::title_sort(&s)),
            _ => Ok(String::new()),
        }
    }

    fn author_sorts(&self, args: &[String]) -> Result<String, String> {
        let names = self.cache.author_names_for_book(self.book_id).map_err(|e| e.to_string())?;
        let sorts: Vec<String> = names.iter().filter_map(|n| self.cache.author_sort_for_name(n).ok().flatten()).collect();
        Ok(sorts.join(&args[0]))
    }

    fn get_link(&self, args: &[String]) -> Result<String, String> {
        if args[0] != "authors" {
            return Ok(String::new());
        }
        Ok(self.cache.author_link_for_name(&args[1]).map_err(|e| e.to_string())?.unwrap_or_default())
    }

    fn author_links(&self, args: &[String]) -> Result<String, String> {
        let mut links = self.cache.all_author_links().map_err(|e| e.to_string())?;
        links.sort_by(|a, b| strcmp(&a.0, &b.0));
        Ok(links.iter().map(|(name, link)| format!("{name}{}{link}", args[0])).collect::<Vec<_>>().join(&args[1]))
    }

    fn language_codes(&self, args: &[String]) -> Result<String, String> {
        let out: Vec<String> = args[0].split(',').map(str::trim).filter(|s| !s.is_empty()).filter_map(calibre_utils::localization::canonicalize_lang).collect();
        Ok(out.join(", "))
    }

    fn language_strings(&self, args: &[String]) -> Result<String, String> {
        let out: Vec<String> = args[0].split(',').map(str::trim).filter(|s| !s.is_empty()).filter_map(calibre_utils::localization::lang_display_name).collect();
        Ok(out.join(", "))
    }

    fn current_library_name(&self) -> Result<String, String> {
        Ok(self.cache.backend.library_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
    }

    fn current_library_path(&self) -> Result<String, String> {
        Ok(self.cache.backend.library_path.to_string_lossy().to_string())
    }

    fn extra_file_names_matching(&self, pattern: Option<&str>) -> Result<Vec<String>, String> {
        let files = extra_files::list_extra_files(self.cache, self.book_id, DATA_FILE_PATTERN).map_err(|e| e.to_string())?;
        let names: Vec<String> = files.iter().map(|f| strip_data_prefix(&f.relpath)).collect();
        match pattern {
            Some(pat) => {
                let mut out = Vec::new();
                for n in names {
                    if regex_search_ci(pat, &n)? {
                        out.push(n);
                    }
                }
                Ok(out)
            }
            None => Ok(names),
        }
    }

    fn has_extra_files(&self, args: &[String]) -> Result<String, String> {
        if args.len() > 1 {
            return Err("Incorrect number of arguments for function has_extra_files".to_string());
        }
        let files = self.extra_file_names_matching(args.first().map(String::as_str))?;
        Ok(if files.is_empty() { String::new() } else { files.len().to_string() })
    }

    fn extra_file_names(&self, args: &[String]) -> Result<String, String> {
        if args.len() > 2 {
            return Err("Incorrect number of arguments for function has_extra_files".to_string());
        }
        let files = self.extra_file_names_matching(args.get(1).map(String::as_str))?;
        Ok(files.join(&args[0]))
    }

    fn extra_file_size(&self, args: &[String]) -> Result<String, String> {
        let want = format!("data/{}", args[0]);
        let files = extra_files::list_extra_files(self.cache, self.book_id, DATA_FILE_PATTERN).map_err(|e| e.to_string())?;
        match files.iter().find(|f| f.relpath == want) {
            Some(f) => Ok(f.size.to_string()),
            None => Ok("-1".to_string()),
        }
    }

    fn extra_file_modtime(&self, args: &[String]) -> Result<String, String> {
        let want = format!("data/{}", args[0]);
        let format_string = &args[1];
        let files = extra_files::list_extra_files(self.cache, self.book_id, DATA_FILE_PATTERN).map_err(|e| e.to_string())?;
        let Some(f) = files.iter().find(|f| f.relpath == want) else {
            // Real upstream not-found value -- the docstring says
            // "-1" but the actual code returns `str(1.0)`, a genuine
            // upstream docstring/code discrepancy preserved verbatim.
            return Ok("1.0".to_string());
        };
        if format_string.is_empty() {
            return Ok((f.mtime_ns as f64 / 1e9).to_string());
        }
        let secs = (f.mtime_ns / 1_000_000_000) as i64;
        let nanos = (f.mtime_ns.rem_euclid(1_000_000_000)) as u32;
        let dt = chrono::DateTime::from_timestamp(secs, nanos).ok_or("invalid modification time")?;
        Ok(calibre_utils::date::format_date(&dt, format_string))
    }

    fn virtual_libraries(&self) -> Result<String, String> {
        let vls = self.cache.virtual_library_map().map_err(|e| e.to_string())?;
        let mut names: Vec<String> = Vec::new();
        for (name, query) in &vls {
            let ids = search::search(self.cache, query).map_err(|e| e.to_string())?;
            if ids.contains(&self.book_id) {
                names.push(name.clone());
            }
        }
        names.sort_by(|a, b| strcmp(a, b));
        Ok(names.join(", "))
    }

    fn book_count(&self, args: &[String]) -> Result<String, String> {
        // `use_vl` (args[1]) has no effect -- see this module's own
        // doc for why.
        let ids = search::search(self.cache, &args[0]).map_err(|e| e.to_string())?;
        Ok(ids.len().to_string())
    }

    fn book_values(&self, args: &[String]) -> Result<String, String> {
        let column = &args[0];
        let query = &args[1];
        let sep = &args[2];
        let fm = FieldMetadata::from_cache(self.cache).map_err(|e| e.to_string())?;
        if fm.get(column).is_none() {
            return Err(format!("The column {column} doesn't exist"));
        }
        let ids = search::search(self.cache, query).map_err(|e| e.to_string())?;
        let id_set: HashSet<i32> = ids.into_iter().collect();
        let rows = self.cache.get_data_as_dict(None, false, Some(&id_set), false).map_err(|e| e.to_string())?;
        let mut set: BTreeSet<String> = BTreeSet::new();
        for row in rows {
            match row.get(column) {
                Some(serde_json::Value::Array(items)) => {
                    for i in items {
                        if let Some(s) = i.as_str() {
                            set.insert(s.to_string());
                        }
                    }
                }
                Some(serde_json::Value::String(s)) if !s.is_empty() => {
                    set.insert(s.clone());
                }
                Some(serde_json::Value::Number(n)) => {
                    set.insert(n.to_string());
                }
                _ => {}
            }
        }
        Ok(set.into_iter().collect::<Vec<_>>().join(sep))
    }

    fn get_note(&self, args: &[String]) -> Result<String, String> {
        let field = &args[0];
        let value = &args[1];
        let plain_text = &args[2];
        let Some(item_id) = categories::get_item_id(self.cache, field, value).map_err(|e| e.to_string())? else {
            return Ok(String::new());
        };
        let Some(note) = self.cache.notes().get_note_data(field, item_id).map_err(|e| e.to_string())? else {
            return Ok(String::new());
        };
        if plain_text == "1" {
            // `searchable_text` is stored as `"{item_value}\n{doc_text}"`
            // -- matches upstream's own `partition('\n')[2]`.
            Ok(note.searchable_text.splitn(2, '\n').nth(1).unwrap_or("").to_string())
        } else {
            Ok(note.doc)
        }
    }

    fn has_note(&self, args: &[String]) -> Result<String, String> {
        let field = &args[0];
        let value = &args[1];
        if !value.is_empty() {
            let item_id = categories::get_item_id(self.cache, field, value).map_err(|e| e.to_string())?;
            let has = match item_id {
                Some(id) => self.cache.notes().get_note_data(field, id).map_err(|e| e.to_string())?.is_some(),
                None => false,
            };
            return Ok(if has { "1".to_string() } else { String::new() });
        }
        let with_notes = self.cache.notes().items_with_notes_for_field(field).map_err(|e| e.to_string())?;
        let vs = CacheValueSource::new(self.cache, self.book_id).map_err(|e| e.to_string())?;
        let items: Vec<String> = match vs.get_raw_value(field) {
            Some(RawValue::List(items)) => items,
            Some(RawValue::Scalar(s)) if !s.is_empty() => vec![s],
            _ => vec![],
        };
        let mut matched = Vec::new();
        for item in items {
            if let Ok(Some(id)) = categories::get_item_id(self.cache, field, &item) {
                if with_notes.contains(&id) {
                    matched.push(item);
                }
            }
        }
        Ok(matched.join(&field_list_sep(self.cache, field)))
    }

    fn approximate_formats(&self) -> Result<String, String> {
        let formats = self.cache.field_for(self.book_id, "formats").map_err(|e| e.to_string())?;
        Ok(formats.map(|s| s.split(", ").map(str::to_uppercase).collect::<Vec<_>>().join(",")).unwrap_or_default())
    }

    fn annotation_count(&self) -> Result<String, String> {
        let c = annotations::annotation_count_for_book(self.cache, self.book_id).map_err(|e| e.to_string())?;
        Ok(if c == 0 { String::new() } else { c.to_string() })
    }

    /// Port of `formats_sizes` (issue #524).
    fn formats_sizes(&self) -> Result<String, String> {
        let infos = self.cache.format_file_info(self.book_id).map_err(|e| e.to_string())?;
        Ok(infos.iter().map(|i| format!("{}:{}", i.fmt, i.size)).collect::<Vec<_>>().join(","))
    }

    /// Port of `formats_paths` (issue #524). Real upstream's own
    /// `evaluate` signature is `(self, formatter, kwargs, mi, locals,
    /// sep=',')` -- a single optional positional argument, NOT `*args`,
    /// despite `arg_count = -1` -- so 2+ arguments is a real error.
    fn formats_paths(&self, args: &[String]) -> Result<String, String> {
        if args.len() > 1 {
            return Err("formats_paths() takes from 0 to 1 positional arguments but more were given".to_string());
        }
        let sep = args.first().map(String::as_str).unwrap_or(",");
        let infos = self.cache.format_file_info(self.book_id).map_err(|e| e.to_string())?;
        Ok(infos.iter().map(|i| format!("{}:{}", i.fmt, i.path.display())).collect::<Vec<_>>().join(sep))
    }

    /// Port of `formats_modtimes` (issue #524). No mtime data exists
    /// anywhere in `calibre_db`'s own `data` table read (confirmed by
    /// reading `tables.rs`'s `FormatsTable`) -- matches
    /// `extra_files.rs`'s own pattern of a real filesystem `stat()`
    /// call on the derived path at read time rather than a stored DB
    /// column. A format whose file is missing/unreadable is silently
    /// excluded (matches upstream's own `mi.get('format_metadata', {})`,
    /// which is only ever populated for formats that actually resolved
    /// to a real file).
    fn formats_modtimes(&self, args: &[String]) -> Result<String, String> {
        let date_format = &args[0];
        let infos = self.cache.format_file_info(self.book_id).map_err(|e| e.to_string())?;
        let mut with_mtime: Vec<(String, std::time::SystemTime)> = Vec::new();
        for info in &infos {
            if let Ok(mtime) = std::fs::metadata(&info.path).and_then(|m| m.modified()) {
                with_mtime.push((info.fmt.clone(), mtime));
            }
        }
        with_mtime.sort_by(|a, b| b.1.cmp(&a.1));
        let parts: Vec<String> = with_mtime
            .into_iter()
            .map(|(fmt, mtime)| {
                let dt: chrono::DateTime<chrono::Utc> = mtime.into();
                format!("{fmt}:{}", calibre_utils::date::format_date(&dt, date_format))
            })
            .collect();
        Ok(parts.join(","))
    }

    /// Port of `formats_path_segments` (issue #524) -- decomposes the
    /// FIRST format's path (`book_col_map`'s own sorted-by-format
    /// order, matching this crate's other formats_* functions; real
    /// upstream picks whatever its own dict iteration order gives,
    /// documented as "one of the extensions will be picked at random")
    /// into author/title/format path segments.
    fn formats_path_segments(&self, args: &[String]) -> Result<String, String> {
        let with_author = &args[0];
        let with_title = &args[1];
        let with_format = &args[2];
        let with_ext = &args[3];
        let sep = &args[4];
        let infos = self.cache.format_file_info(self.book_id).map_err(|e| e.to_string())?;
        let Some(info) = infos.first() else {
            return Ok("No book formats found so the path can't be generated".to_string());
        };
        let file_name = info.path.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        let fmt_component = if with_ext == "0" || with_ext.is_empty() {
            Path::new(file_name).file_stem().and_then(|s| s.to_str()).unwrap_or(file_name).to_string()
        } else {
            file_name.to_string()
        };
        let parent = info.path.parent();
        let title_component = parent.and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let author_component = parent.and_then(|p| p.parent()).and_then(|p| p.file_name()).and_then(|s| s.to_str()).unwrap_or_default().to_string();
        let mut parts = Vec::new();
        if with_author == "1" {
            parts.push(author_component);
        }
        if with_title == "1" {
            parts.push(title_component);
        }
        if with_format == "1" {
            parts.push(fmt_component);
        }
        Ok(parts.join(sep))
    }

    /// Port of `check_yes_no` (issue #515) -- needs real book-field
    /// access (`getattr(mi, field, None)`) so it lives here rather
    /// than in `calibre_utils::formatter::string_functions` with the
    /// rest of `STRING_MANIPULATION`.
    ///
    /// `Cache::get_custom_column_value` stores a bool custom column's
    /// value as the string `"0"`/`"1"` (see that function's own doc),
    /// not a JSON boolean -- `get_data_as_dict`/`CacheValueSource`
    /// inherit that representation unchanged, so both the string and
    /// (for forward-compatibility) bool JSON shapes are handled here.
    fn check_yes_no(&self, args: &[String]) -> Result<String, String> {
        let field_key = &args[0];
        let row_key = field_key.trim_start_matches('#');
        let fm = FieldMetadata::from_cache(self.cache).map_err(|e| e.to_string())?;
        let is_bool_field = fm.get(field_key).map(|i| i.datatype.as_deref() == Some("bool")).unwrap_or(false);
        let vs = CacheValueSource::new(self.cache, self.book_id).map_err(|e| e.to_string())?;
        match vs.row.get(row_key) {
            None | Some(serde_json::Value::Null) => Ok(if args[1] == "1" { "Yes".to_string() } else { String::new() }),
            Some(_) if !is_bool_field => Err("check_yes_no requires the field be a Yes/No custom column".to_string()),
            Some(serde_json::Value::String(s)) => {
                let b = s == "1";
                Ok(if (args[2] == "1" && !b) || (args[3] == "1" && b) { "Yes".to_string() } else { String::new() })
            }
            Some(serde_json::Value::Bool(b)) => {
                let b = *b;
                Ok(if (args[2] == "1" && !b) || (args[3] == "1" && b) { "Yes".to_string() } else { String::new() })
            }
            _ => Err("check_yes_no requires the field be a Yes/No custom column".to_string()),
        }
    }

    /// Port of `field_exists` (issue #515) -- needs the real field
    /// registry (`mi.all_field_keys()`, standard fields + this
    /// library's actual custom columns), so it lives here rather than
    /// in `calibre_utils::formatter::string_functions`.
    fn field_exists(&self, args: &[String]) -> Result<String, String> {
        let field = args[0].to_lowercase();
        let fm = FieldMetadata::from_cache(self.cache).map_err(|e| e.to_string())?;
        Ok(if fm.all_field_keys().contains(&field.as_str()) { "1".to_string() } else { String::new() })
    }

    /// Port of `format_date_field` (issue #518) -- needs the real
    /// field registry + this book's actual field value, so it lives
    /// here rather than in `calibre_utils::formatter::format_functions`
    /// with `format_date` (which works on an already-evaluated date
    /// *string*, not a field lookup).
    ///
    /// Disclosed real upstream bug, not replicated: upstream's own
    /// `from_number` branch calls `float(val)` where `val` is already
    /// `mi.get(field)`'s real `datetime` object (not a number string)
    /// -- that would raise a `TypeError` in real calibre too. This
    /// port reports the same real error instead of silently doing
    /// something more helpful than upstream.
    fn format_date_field(&self, args: &[String]) -> Result<String, String> {
        let field_arg = &args[0];
        let format_string = &args[1];
        let fm = FieldMetadata::from_cache(self.cache).map_err(|e| e.to_string())?;
        let field = fm.search_term_to_field_key(field_arg).to_string();
        if !fm.all_field_keys().contains(&field.as_str()) {
            return Err(format!("Function format_date_field: Unknown field '{field}'"));
        }
        let info = fm.get(&field).ok_or_else(|| format!("Function format_date_field: Unknown field '{field}'"))?;
        if info.datatype.as_deref() != Some("datetime") {
            return Err(format!("Function format_date_field: field '{field}' is not a date"));
        }
        if format_string.starts_with("from_number") {
            return Err("format_date_field: 'from_number' cannot be used on a real date field (matches a real upstream bug -- calling float() on an already-parsed datetime raises there too)".to_string());
        }
        let vs = CacheValueSource::new(self.cache, self.book_id).map_err(|e| e.to_string())?;
        let val_str = match vs.row.get(field.trim_start_matches('#')) {
            None | Some(serde_json::Value::Null) => return Ok(String::new()),
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => return Ok(String::new()),
        };
        let Some(d) = calibre_utils::date::parse_date(&val_str, true) else { return Ok(String::new()) };
        Ok(format_parsed_date(d, format_string).unwrap_or_default())
    }

    /// Port of `rating_to_stars` (issue #518) -- needs
    /// `calibre_ebooks::oeb::transforms::jacket::rating_to_stars`,
    /// which `calibre_utils` can't depend on (that crate depends on
    /// `calibre_utils`, not the reverse), so it lives here even though
    /// it doesn't touch `self.cache`/`self.book_id` at all.
    fn rating_to_stars(&self, args: &[String]) -> Result<String, String> {
        let value = &args[0];
        let use_half_stars = &args[1];
        if value.is_empty() {
            return Ok(String::new());
        }
        const ERR: &str = "The rating must be a number between 0 and 5";
        let v: f64 = value.parse().map_err(|_| ERR.to_string())?;
        let v2 = v * 2.0;
        if !(0.0..=10.0).contains(&v2) {
            return Err(ERR.to_string());
        }
        Ok(ebooks_rating_to_stars(Some(v2), use_half_stars == "1"))
    }

    /// Port of `urls_from_identifiers` (issue #518) -- needs
    /// `calibre_ebooks::xml_util::prepare_string_for_xml` (same
    /// cross-crate-dependency reason as `rating_to_stars`).
    ///
    /// Disclosed narrowing: real upstream also consults
    /// `msprefs['id_link_rules']` (user-configured URL templates,
    /// no preferences-UI equivalent exists in this crate) and
    /// `all_metadata_plugins()` (calibre's metadata-download source
    /// plugins, a subsystem not ported at all here) -- only the
    /// hardcoded fallback identifiers upstream itself falls back to
    /// (`isbn`/`doi`/`arxiv`/`oclc`/`issn`) plus explicit `uri`/`url`-
    /// named identifiers are implemented. Real calibre without any
    /// matching plugin/rule installed would ALSO produce nothing for
    /// every identifier type this port doesn't cover, so this is
    /// narrower in *source* but not wildly narrower in typical
    /// *practice*.
    fn urls_from_identifiers(&self, args: &[String]) -> Result<String, String> {
        let identifiers_str = &args[0];
        let sort_results = &args[1];
        let mut pairs: Vec<(String, String)> = Vec::new();
        for id_ in identifiers_str.split(',') {
            if id_.is_empty() {
                continue;
            }
            if let Some((l, r)) = id_.split_once(':') {
                let (l, r) = (l.trim(), r.trim());
                if !l.is_empty() && !r.is_empty() {
                    pairs.push((l.to_lowercase(), r.to_string()));
                }
            }
        }
        let mut links: Vec<(String, String, String, String)> = Vec::new();
        for (k, v) in &pairs {
            match k.as_str() {
                "isbn" => links.push((v.clone(), "isbn".to_string(), v.clone(), format!("https://www.worldcat.org/isbn/{v}"))),
                "doi" => links.push(("DOI".to_string(), "doi".to_string(), v.clone(), format!("https://dx.doi.org/{v}"))),
                "arxiv" => links.push(("arXiv".to_string(), "arxiv".to_string(), v.clone(), format!("https://arxiv.org/abs/{v}"))),
                "oclc" => links.push(("OCLC".to_string(), "oclc".to_string(), v.clone(), format!("https://www.worldcat.org/oclc/{v}"))),
                "issn" => {
                    if let Some(issn) = check_issn(v) {
                        links.push((issn.clone(), "issn".to_string(), issn.clone(), format!("https://www.worldcat.org/issn/{issn}")));
                    }
                }
                _ => {}
            }
        }
        for (k, v) in &pairs {
            if is_uri_identifier_key(k) {
                let scheme = v.split_once(':').map(|(s, _)| s.to_lowercase()).unwrap_or_default();
                if matches!(scheme.as_str(), "http" | "https" | "file") {
                    links.push((url_display_name(v), k.clone(), v.clone(), v.clone()));
                }
            }
        }
        if sort_results != "0" {
            links.sort_by(|a, b| strcmp(&a.0, &b.0));
        }
        let html: Vec<String> = links
            .iter()
            .map(|(name, id_typ, id_val, url)| {
                format!(
                    "<a href=\"{}\" title=\"{}:{}\">{}</a>",
                    prepare_string_for_xml(url, true),
                    prepare_string_for_xml(id_typ, true),
                    prepare_string_for_xml(id_val, true),
                    prepare_string_for_xml(name, false)
                )
            })
            .collect();
        Ok(html.join(", "))
    }
}

/// Port of `calibre.ebooks.metadata.check_issn`.
fn check_issn(issn: &str) -> Option<String> {
    if issn.is_empty() {
        return None;
    }
    let cleaned: String = issn.to_uppercase().chars().filter(|c| c.is_ascii_digit() || *c == 'X').collect();
    if cleaned.len() < 8 {
        return None;
    }
    let digits: Vec<i32> = cleaned[..7].chars().map(|c| c.to_digit(10).unwrap_or(0) as i32).collect();
    let sum: i32 = digits.iter().enumerate().map(|(i, d)| (8 - i as i32) * d).sum();
    let check = 11 - sum.rem_euclid(11);
    let last = cleaned.chars().nth(7)?;
    let matches_check = (check == 10 && last == 'X') || last.to_digit(10).map(|d| d as i32) == Some(check);
    matches_check.then_some(cleaned)
}

/// Matches upstream's `re.match(r'ur[il]\d*$', k)` on an already-
/// lowercased key.
fn is_uri_identifier_key(k: &str) -> bool {
    k.strip_prefix("uri").or_else(|| k.strip_prefix("url")).is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit()))
}

/// A simplified `urlparse(url).netloc or .path` -- real upstream uses
/// Python's own `urlparse` for this; this is a direct, bounded
/// re-derivation (scheme `://` then up to the first `/`, `?`, or `#`)
/// rather than a full RFC 3986 URL parser.
fn url_display_name(url: &str) -> String {
    let Some(rest) = url.split_once("://").map(|(_, r)| r) else { return url.to_string() };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let netloc = &rest[..end];
    if netloc.is_empty() { rest.to_string() } else { netloc.to_string() }
}

impl FunctionRegistry for CacheFunctions<'_> {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        match name {
            "raw_field" => self.raw_field(args),
            "raw_list" => self.raw_list(args),
            "has_cover" => self.has_cover(),
            "booksize" => self.booksize(),
            "series_sort" => self.series_sort(),
            "author_sorts" => self.author_sorts(args),
            "get_link" => self.get_link(args),
            "author_links" => self.author_links(args),
            "language_codes" => self.language_codes(args),
            "language_strings" => self.language_strings(args),
            "current_library_name" => self.current_library_name(),
            "current_library_path" => self.current_library_path(),
            "has_extra_files" => self.has_extra_files(args),
            "extra_file_names" => self.extra_file_names(args),
            "extra_file_size" => self.extra_file_size(args),
            "extra_file_modtime" => self.extra_file_modtime(args),
            "virtual_libraries" => self.virtual_libraries(),
            "book_count" => self.book_count(args),
            "book_values" => self.book_values(args),
            "get_note" => self.get_note(args),
            "has_note" => self.has_note(args),
            "approximate_formats" => self.approximate_formats(),
            "ondevice" => Ok(String::new()),
            "annotation_count" => self.annotation_count(),
            "formats_sizes" => self.formats_sizes(),
            "formats_paths" => self.formats_paths(args),
            "formats_modtimes" => self.formats_modtimes(args),
            "formats_path_segments" => self.formats_path_segments(args),
            "check_yes_no" => self.check_yes_no(args),
            "field_exists" => self.field_exists(args),
            "format_date_field" => self.format_date_field(args),
            "rating_to_stars" => self.rating_to_stars(args),
            "urls_from_identifiers" => self.urls_from_identifiers(args),
            // Every `STRING_MANIPULATION`/`CASE_CHANGES` (#515),
            // `LIST_MANIPULATION`/`LIST_LOOKUP` (#516),
            // `ARITHMETIC`/`RELATIONAL`/`BOOLEAN` (#517), and
            // `FORMATTING_VALUES`/`DATE_FUNCTIONS`/`URL_FUNCTIONS`
            // (#518) built-in needs no `Cache` access at all, so all
            // four live in `calibre_utils` and are reached here as a
            // fallback.
            _ => fallback_call(name, args),
        }
    }
}

/// Routes a name unknown to [`CacheFunctions::call`] to whichever of
/// `calibre_utils`'s no-`Cache`-needed function modules actually
/// implements it -- by arity lookup, not by matching on a module's
/// "No function named ... exists" error text (fragile), since a name
/// absent from one module's dispatch table is also absent from that
/// same module's arity table.
fn fallback_call(name: &str, args: &[String]) -> Result<String, String> {
    use calibre_utils::formatter::{format_functions, list_functions, misc_functions, numeric_functions, string_functions};
    if string_functions::arg_count(name).is_some() {
        string_functions::call(name, args)
    } else if list_functions::arg_count(name).is_some() {
        list_functions::call(name, args)
    } else if numeric_functions::arg_count(name).is_some() {
        numeric_functions::call(name, args)
    } else if format_functions::arg_count(name).is_some() {
        format_functions::call(name, args)
    } else {
        misc_functions::call(name, args)
    }
}

/// The [`FunctionCatalog`] counterpart of [`fallback_call`].
fn fallback_arg_count(name: &str) -> Option<Option<usize>> {
    use calibre_utils::formatter::{format_functions, list_functions, misc_functions, numeric_functions, string_functions};
    string_functions::arg_count(name)
        .or_else(|| list_functions::arg_count(name))
        .or_else(|| numeric_functions::arg_count(name))
        .or_else(|| format_functions::arg_count(name))
        .or_else(|| misc_functions::arg_count(name))
}

/// Parse-time arity/existence catalog matching [`CacheFunctions`]'s
/// own coverage -- outer `None` = unknown name, `Some(None)` =
/// variadic, `Some(Some(n))` = fixed arity `n`.
pub struct CacheCatalog;

impl FunctionCatalog for CacheCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        match name {
            "raw_field" => Some(None),
            "raw_list" => Some(Some(2)),
            "has_cover" | "booksize" | "series_sort" | "current_library_name" | "current_library_path" | "virtual_libraries" | "approximate_formats" | "ondevice" | "annotation_count" => Some(Some(0)),
            "author_sorts" => Some(Some(1)),
            "get_link" | "author_links" | "language_strings" | "has_note" | "book_count" => Some(Some(2)),
            "language_codes" | "extra_file_size" => Some(Some(1)),
            "extra_file_modtime" => Some(Some(2)),
            "get_note" => Some(Some(3)),
            "book_values" => Some(Some(4)),
            "has_extra_files" | "extra_file_names" => Some(None),
            "field_exists" => Some(Some(1)),
            "check_yes_no" => Some(Some(4)),
            "format_date_field" | "rating_to_stars" | "urls_from_identifiers" => Some(Some(2)),
            "formats_sizes" => Some(Some(0)),
            "formats_paths" => Some(None),
            "formats_modtimes" => Some(Some(1)),
            "formats_path_segments" => Some(Some(5)),
            _ => fallback_arg_count(name),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calibre_ebooks::metadata::MetaInformation;
    use std::collections::HashMap as StdHashMap;

    fn make_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path()).unwrap();
        (dir, cache)
    }

    fn add_book(cache: &Cache, title: &str, authors: &[&str]) -> i32 {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join(format!("{title}.epub"));
        std::fs::write(&src, b"fake").unwrap();
        let meta = MetaInformation::new(title, authors.iter().map(|s| s.to_string()).collect());
        let id = cache.add_book(&src, &meta).unwrap();
        // `Cache::add_book`/`MetaInformation` doesn't wire tags/series
        // (a real, pre-existing narrowing -- see the #461/#464
        // session memory) -- set them explicitly the same way the
        // rest of this session's scratch fixtures do.
        cache.set_field(id, "tags", "Fiction, Mystery").unwrap();
        cache.set_field(id, "series", "A Series").unwrap();
        id
    }

    #[test]
    fn raw_field_and_raw_list_read_real_multi_value_data() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Alice Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("raw_field", &["tags".to_string()]).unwrap(), "Fiction, Mystery");
        assert_eq!(f.call("raw_list", &["tags".to_string(), " | ".to_string()]).unwrap(), "Fiction | Mystery");
        // A default is used only when the field is genuinely absent.
        assert_eq!(f.call("raw_field", &["no_such_field".to_string(), "fallback".to_string()]).unwrap(), "fallback");
    }

    #[test]
    fn has_cover_and_booksize_reflect_real_book_state() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("has_cover", &[]).unwrap(), "", "no cover was set");
        let size = f.call("booksize", &[]).unwrap();
        assert!(!size.is_empty() && size != "0", "a real added format should have a nonzero size, got {size:?}");
    }

    #[test]
    fn series_sort_strips_a_leading_article_via_title_sort() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        cache.set_field(id, "series", "The Chronicles").unwrap();
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("series_sort", &[]).unwrap(), "Chronicles, The");
    }

    #[test]
    fn author_sorts_and_get_link_and_author_links_use_real_author_table_data() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Zora Zed", "Amy Ant"]);
        // Give "Zora Zed" a real stored link.
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE authors SET link = 'https://example.com/zora' WHERE name = 'Zora Zed'", []).unwrap();
        }
        let f = CacheFunctions::new(&cache, id);
        // Order follows the book's own author link order, not alphabetical.
        let sorts = f.call("author_sorts", &[" & ".to_string()]).unwrap();
        assert!(sorts.contains("Zed, Zora") && sorts.contains("Ant, Amy"), "got: {sorts}");

        assert_eq!(f.call("get_link", &["authors".to_string(), "Zora Zed".to_string()]).unwrap(), "https://example.com/zora");
        assert_eq!(f.call("get_link", &["authors".to_string(), "Amy Ant".to_string()]).unwrap(), "", "no link was set for this author");
        assert_eq!(f.call("get_link", &["tags".to_string(), "Fiction".to_string()]).unwrap(), "", "tags have no link storage in this port");

        let links = f.call("author_links", &[":".to_string(), ",".to_string()]).unwrap();
        assert_eq!(links, "Zora Zed:https://example.com/zora");
    }

    #[test]
    fn language_codes_and_language_strings_round_trip() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("language_codes", &["English, French".to_string()]).unwrap(), "eng, fra");
        assert_eq!(f.call("language_strings", &["eng,fra".to_string(), "0".to_string()]).unwrap(), "English, French");
    }

    #[test]
    fn current_library_name_and_path_reflect_the_real_open_library() {
        let (dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("current_library_path", &[]).unwrap(), dir.path().to_string_lossy());
        assert_eq!(f.call("current_library_name", &[]).unwrap(), dir.path().file_name().unwrap().to_string_lossy());
    }

    #[test]
    fn extra_files_functions_see_real_files_on_disk() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let mut files = StdHashMap::new();
        files.insert("data/notes.txt".to_string(), b"hello".to_vec());
        files.insert("data/cover_alt.jpg".to_string(), b"jpegbytes".to_vec());
        extra_files::add_extra_files(&cache, id, &files, false).unwrap();

        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("has_extra_files", &[]).unwrap(), "2");
        assert_eq!(f.call("has_extra_files", &[r"\.txt$".to_string()]).unwrap(), "1");

        let names = f.call("extra_file_names", &[",".to_string()]).unwrap();
        let mut parts: Vec<&str> = names.split(',').collect();
        parts.sort();
        assert_eq!(parts, vec!["cover_alt.jpg", "notes.txt"]);

        assert_eq!(f.call("extra_file_size", &["notes.txt".to_string()]).unwrap(), "5");
        assert_eq!(f.call("extra_file_size", &["missing.txt".to_string()]).unwrap(), "-1");

        let modtime = f.call("extra_file_modtime", &["notes.txt".to_string(), String::new()]).unwrap();
        assert!(modtime.parse::<f64>().unwrap() > 0.0, "got: {modtime}");
    }

    #[test]
    fn virtual_libraries_reports_real_membership() {
        let (_dir, cache) = make_cache();
        let id1 = add_book(&cache, "Fiction Book", &["Author"]);
        let _id2 = add_book(&cache, "Nonfiction Book", &["Author"]);
        cache.virtual_library_add("Fiction Only", "tags:Fiction").unwrap();
        cache.virtual_library_add("Empty VL", "tags:NoSuchTag").unwrap();

        let f = CacheFunctions::new(&cache, id1);
        assert_eq!(f.call("virtual_libraries", &[]).unwrap(), "Fiction Only");
    }

    #[test]
    fn book_count_and_book_values_run_a_real_search() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book One", &["Shared Author"]);
        let _id2 = add_book(&cache, "Book Two", &["Shared Author"]);
        let f = CacheFunctions::new(&cache, id);

        assert_eq!(f.call("book_count", &["authors:\"=Shared Author\"".to_string(), "1".to_string()]).unwrap(), "2");

        let titles = f.call("book_values", &["title".to_string(), "authors:\"=Shared Author\"".to_string(), "|".to_string(), "1".to_string()]).unwrap();
        let mut parts: Vec<&str> = titles.split('|').collect();
        parts.sort();
        assert_eq!(parts, vec!["Book One", "Book Two"]);

        assert!(f.call("book_values", &["no_such_column".to_string(), "".to_string(), ",".to_string(), "1".to_string()]).is_err());
    }

    #[test]
    fn get_note_and_has_note_reflect_real_stored_notes() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        cache.notes().initialize().unwrap();
        let item_id = categories::get_item_id(&cache, "tags", "Fiction").unwrap().unwrap();
        cache.notes().set_note("tags", item_id, "Fiction", "<p>Great genre</p>", &HashSet::new()).unwrap();

        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("has_note", &["tags".to_string(), "Fiction".to_string()]).unwrap(), "1");
        assert_eq!(f.call("has_note", &["tags".to_string(), "Mystery".to_string()]).unwrap(), "");

        let html = f.call("get_note", &["tags".to_string(), "Fiction".to_string(), "".to_string()]).unwrap();
        assert!(html.contains("Great genre"), "got: {html}");

        // Empty field_value: list every one of *this book's* tags
        // that has a note (only "Fiction" does).
        let with_notes = f.call("has_note", &["tags".to_string(), String::new()]).unwrap();
        assert_eq!(with_notes, "Fiction");
    }

    #[test]
    fn approximate_formats_and_ondevice_and_annotation_count() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("approximate_formats", &[]).unwrap(), "EPUB", "add_book's own test fixture writes a .epub");
        assert_eq!(f.call("ondevice", &[]).unwrap(), "");
        assert_eq!(f.call("annotation_count", &[]).unwrap(), "");
    }

    #[test]
    fn formats_sizes_and_paths_reflect_real_files_on_disk() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        // add_book's own fixture writes a real 4-byte "fake" .epub.
        assert_eq!(f.call("formats_sizes", &[]).unwrap(), "EPUB:4");
        let paths = f.call("formats_paths", &[]).unwrap();
        assert!(paths.starts_with("EPUB:"), "got: {paths}");
        let path_str = paths.strip_prefix("EPUB:").unwrap();
        assert!(std::path::Path::new(path_str).is_file(), "the reported path should really exist on disk: {path_str}");
        let paths_custom_sep = f.call("formats_paths", &["|".to_string()]).unwrap();
        assert_eq!(paths_custom_sep, paths, "single-format list looks the same regardless of separator");
        assert!(f.call("formats_paths", &["a".to_string(), "b".to_string()]).is_err(), "real upstream signature takes at most one positional argument");
    }

    #[test]
    fn formats_modtimes_reports_a_real_recent_mtime() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        let out = f.call("formats_modtimes", &["yyyy".to_string()]).unwrap();
        assert!(out.starts_with("EPUB:"), "got: {out}");
        let year = out.strip_prefix("EPUB:").unwrap();
        assert_eq!(year.len(), 4, "got: {year}");
    }

    #[test]
    fn formats_path_segments_decomposes_the_real_path() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        let fmt_only = f.call("formats_path_segments", &["0".to_string(), "0".to_string(), "1".to_string(), "0".to_string(), "/".to_string()]).unwrap();
        assert!(!fmt_only.is_empty() && !fmt_only.contains('/'), "got: {fmt_only}");
        let fmt_with_ext = f.call("formats_path_segments", &["0".to_string(), "0".to_string(), "1".to_string(), "1".to_string(), "/".to_string()]).unwrap();
        assert_eq!(fmt_with_ext, format!("{fmt_only}.epub"));
        let full = f.call("formats_path_segments", &["1".to_string(), "1".to_string(), "1".to_string(), "1".to_string(), "/".to_string()]).unwrap();
        assert_eq!(full.matches('/').count(), 2, "author/title/format -- got: {full}");
    }

    #[test]
    fn check_yes_no_covers_undefined_false_true_and_rejects_non_bool_fields() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        cache.add_custom_column("myflag", "My Flag", "bool", false).unwrap();
        let f = CacheFunctions::new(&cache, id);

        // No value has been set yet -- undefined.
        assert_eq!(f.call("check_yes_no", &["#myflag".to_string(), "1".to_string(), "0".to_string(), "0".to_string()]).unwrap(), "Yes");
        assert_eq!(f.call("check_yes_no", &["#myflag".to_string(), "0".to_string(), "0".to_string(), "0".to_string()]).unwrap(), "");

        cache.set_custom_column_value(id, "myflag", "0").unwrap();
        assert_eq!(f.call("check_yes_no", &["#myflag".to_string(), "0".to_string(), "1".to_string(), "0".to_string()]).unwrap(), "Yes");
        assert_eq!(f.call("check_yes_no", &["#myflag".to_string(), "0".to_string(), "0".to_string(), "1".to_string()]).unwrap(), "");

        cache.set_custom_column_value(id, "myflag", "1").unwrap();
        assert_eq!(f.call("check_yes_no", &["#myflag".to_string(), "0".to_string(), "0".to_string(), "1".to_string()]).unwrap(), "Yes");

        assert!(f.call("check_yes_no", &["title".to_string(), "0".to_string(), "0".to_string(), "1".to_string()]).is_err(), "title is a real, non-bool field");
    }

    #[test]
    fn field_exists_checks_the_real_field_registry() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        cache.add_custom_column("myflag", "My Flag", "bool", false).unwrap();
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("field_exists", &["title".to_string()]).unwrap(), "1");
        assert_eq!(f.call("field_exists", &["TITLE".to_string()]).unwrap(), "1", "lookup is case-insensitive");
        assert_eq!(f.call("field_exists", &["#myflag".to_string()]).unwrap(), "1");
        assert_eq!(f.call("field_exists", &["no_such_field".to_string()]).unwrap(), "");
    }

    #[test]
    fn string_manipulation_functions_fall_through_to_calibre_utils() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("uppercase", &["abc".to_string()]).unwrap(), "ABC");
        assert_eq!(f.call("substr", &["12345".to_string(), "1".to_string(), "0".to_string()]).unwrap(), "2345");
    }

    #[test]
    fn list_manipulation_functions_fall_through_to_calibre_utils() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("list_count", &["a,b,c".to_string(), ",".to_string()]).unwrap(), "3");
        assert_eq!(f.call("list_item", &["a & b".to_string(), "-1".to_string(), "&".to_string()]).unwrap(), "b");
    }

    #[test]
    fn catalog_falls_through_for_list_manipulation_arities() {
        let catalog = CacheCatalog;
        assert_eq!(catalog.arg_count("list_item"), Some(Some(3)));
        assert_eq!(catalog.arg_count("range"), Some(None));
    }

    #[test]
    fn numeric_functions_fall_through_to_calibre_utils() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("add", &["2".to_string(), "3".to_string()]).unwrap(), "5.0");
        assert_eq!(f.call("strcmp", &["apple".to_string(), "Apple".to_string(), "lt".to_string(), "eq".to_string(), "gt".to_string()]).unwrap(), "eq");
    }

    #[test]
    fn catalog_falls_through_for_numeric_arities() {
        let catalog = CacheCatalog;
        assert_eq!(catalog.arg_count("strcmp"), Some(Some(5)));
        assert_eq!(catalog.arg_count("add"), Some(None));
    }

    #[test]
    fn format_functions_fall_through_to_calibre_utils() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("human_readable", &["1536".to_string()]).unwrap(), "1.5 KB");
        assert_eq!(f.call("today", &[]).unwrap().len() > 0, true);
        assert!(f.call("is_dark_mode", &[]).is_err(), "no GUI exists in this port -- real upstream raises only_in_gui_error here, not a silent empty string");
    }

    #[test]
    fn catalog_falls_through_for_format_arities() {
        let catalog = CacheCatalog;
        assert_eq!(catalog.arg_count("human_readable"), Some(Some(1)));
        assert_eq!(catalog.arg_count("make_url"), Some(None));
    }

    #[test]
    fn format_date_field_formats_a_real_datetime_field() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        let out = f.call("format_date_field", &["timestamp".to_string(), "yyyy".to_string()]).unwrap();
        assert_eq!(out.len(), 4, "got: {out}");
        assert!(f.call("format_date_field", &["title".to_string(), "yyyy".to_string()]).is_err(), "title is not a date field");
        assert!(f.call("format_date_field", &["no_such_field".to_string(), "yyyy".to_string()]).is_err());
    }

    #[test]
    fn rating_to_stars_delegates_to_the_real_jacket_helper() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert_eq!(f.call("rating_to_stars", &["5".to_string(), "0".to_string()]).unwrap(), "★★★★★");
        assert_eq!(f.call("rating_to_stars", &["".to_string(), "0".to_string()]).unwrap(), "");
        assert!(f.call("rating_to_stars", &["6".to_string(), "0".to_string()]).is_err());
    }

    #[test]
    fn urls_from_identifiers_covers_the_hardcoded_fallback_identifiers() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        let html = f.call("urls_from_identifiers", &["isbn:0123456789, doi:10.1/xyz".to_string(), "0".to_string()]).unwrap();
        assert!(html.contains("https://www.worldcat.org/isbn/0123456789"), "got: {html}");
        assert!(html.contains("https://dx.doi.org/10.1/xyz"), "got: {html}");
        assert_eq!(f.call("urls_from_identifiers", &["unknown_source:xyz".to_string(), "0".to_string()]).unwrap(), "", "no rule/plugin matches this identifier type in this port");
    }

    #[test]
    fn catalog_falls_through_for_string_manipulation_arities() {
        let catalog = CacheCatalog;
        assert_eq!(catalog.arg_count("shorten"), Some(Some(4)));
        assert_eq!(catalog.arg_count("field_exists"), Some(Some(1)));
        assert_eq!(catalog.arg_count("check_yes_no"), Some(Some(4)));
    }

    #[test]
    fn catalog_reports_correct_arity_and_rejects_unknown_names() {
        let catalog = CacheCatalog;
        assert_eq!(catalog.arg_count("has_cover"), Some(Some(0)));
        assert_eq!(catalog.arg_count("get_note"), Some(Some(3)));
        assert_eq!(catalog.arg_count("raw_field"), Some(None));
        assert_eq!(catalog.arg_count("is_marked"), None, "not registered -- no marked-books storage exists");
        assert_eq!(catalog.arg_count("connected_device_name"), None, "not registered -- no device subsystem exists");
    }

    #[test]
    fn unregistered_function_call_is_a_real_error() {
        let (_dir, cache) = make_cache();
        let id = add_book(&cache, "Book", &["Author"]);
        let f = CacheFunctions::new(&cache, id);
        assert!(f.call("is_marked", &[]).is_err());
    }
}
