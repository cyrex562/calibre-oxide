//! Port of `old_src/src/calibre/library/catalogs/bibtex.py`'s `BIBTEX`
//! catalog generator -- renders a book list to a `.bib` file.
//!
//! See `crates/calibre_db/src/catalogs/mod.rs`'s module doc for the
//! disclosed simplifications shared by every generator in this module. This
//! file additionally simplifies:
//!
//! - **`calibre.utils.bibtex.BibTeX` is ported narrowly, not fully.**
//!   Upstream's `BibTeX` class lives in `calibre/utils/bibtex.py`, a
//!   *different* module with no tracked port issue of its own -- ~2470 of
//!   its 2632 lines are `utf8enc2latex_mapping`, a Unicode-code-point ->
//!   LaTeX-escape-sequence table, used only by `resolveUnicode`. Tracing
//!   `BIBTEX.run`'s actual call path: `resolveUnicode` only runs when
//!   `ascii_bibtex` is true, which upstream only sets when the user
//!   explicitly picks `--choose-encoding ascii` (the CLI default is
//!   `utf8`, where `ascii_bibtex` stays false and `resolveUnicode` never
//!   runs at all). This port implements every other `BibTeX` method fully
//!   (`resolveEntities`, `escapeSpecialCharacters`, `ValidateCitationKey`,
//!   `stripUnmatchedSyntax`, `bibtex_author_format`, `utf8ToBibtex`) but
//!   [`BibtexEscaper::resolve_unicode`] is a documented pass-through --
//!   the `ascii`-encoding path emits UTF-8 text un-LaTeX-escaped rather
//!   than pulling in the full mapping table. If a caller needs the real
//!   ASCII/LaTeX-escaped path, porting `utf8enc2latex_mapping` is its own
//!   scoped follow-up (codegen from the live Python dict, per
//!   `[[large-data-table-port-technique]]`), not part of issue #57.
//! - **Output is always written as UTF-8.** Upstream's `bibfile_enc`
//!   option (`utf8`/`cp1252`/`ascii`) additionally selects the *byte*
//!   encoding `codecs.open` writes with (plus an error-handling tag:
//!   `strict`/`replace`/`ignore`/`backslashreplace`). This crate has no
//!   `cp1252` (or general non-UTF-8) encoder dependency; `run` always
//!   writes UTF-8 bytes regardless of `ascii_bibtex`. Combined with the
//!   point above, choosing "ascii" output here means UTF-8-encoded text
//!   that hasn't been LaTeX-escaped, not genuine 7-bit-clean ASCII bytes.
//! - **CLI widget/string dual-typing is dropped.** Upstream's `run`
//!   handles `opts.bibfile_enc`/`bibfile_enctag`/`bib_entry` arriving as
//!   either a raw string (CLI) or a GUI combobox integer index, with a
//!   fallback-to-default on either failing -- that's CLI/GUI plumbing, not
//!   core logic. [`BibtexOptions`] takes already-resolved, strongly-typed
//!   values instead ([`BibEntryMode`], `ascii_bibtex: bool`).
//! - **No wall-clock reads inside library code.** The file header embeds
//!   `strftime('%A, %d. %B %Y %H:%M')` (upstream's local "now"). `run`
//!   takes `generated_at: DateTime<Utc>` as an explicit parameter instead
//!   of calling `Utc::now()` internally, keeping this module pure/testable
//!   (this crate's established "pure-core, caller supplies impure inputs"
//!   convention).

use std::path::Path;

use chrono::{DateTime, Datelike, Utc};
use regex::Regex;
use serde_json::Value;

use crate::cache::Cache;
use crate::catalogs::{get_output_fields, CatalogError, Result, TEMPLATE_ALLOWED_FIELDS};

/// Port of `calibre.utils.bibtex.BibTeX`, narrowed per this module's own
/// doc (`resolve_unicode` is a pass-through, not the full LaTeX-escape
/// table).
#[derive(Debug, Clone)]
pub struct BibtexEscaper {
    pub ascii_bibtex: bool,
}

impl Default for BibtexEscaper {
    fn default() -> Self {
        BibtexEscaper { ascii_bibtex: false }
    }
}

fn invalid_citation_char(c: char) -> bool {
    matches!(c, ' ' | '"' | '@' | '\'' | ',' | '#' | '}' | '{' | '~' | '%' | '&' | '$' | '^')
}

fn escape_char_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[#&%_]").unwrap())
}

impl BibtexEscaper {
    /// Port of `ValidateCitationKey`.
    pub fn validate_citation_key(&self, text: &str) -> String {
        text.chars().filter(|c| !invalid_citation_char(*c)).collect()
    }

    /// Port of `resolveEntities`.
    fn resolve_entities(&self, text: &str) -> String {
        text.replace("&mdash;", "{---}").replace("&ndash;", "{--}").replace('"', "{\"}")
    }

    /// Port of `escapeSpecialCharacters`.
    fn escape_special_characters(&self, text: &str) -> String {
        let text = text.replace('\\', "\\\\").replace('~', "{\\char`\\~}");
        escape_char_re().replace_all(&text, |caps: &regex::Captures| format!("\\{}", &caps[0])).into_owned()
    }

    /// Port of `resolveUnicode` -- see this module's doc for why this is a
    /// documented pass-through rather than the full LaTeX-escape table.
    fn resolve_unicode(&self, text: &str) -> String {
        text.replace("$}{$", "")
    }

    /// Port of `utf8ToBibtex`.
    pub fn utf8_to_bibtex(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let text = self.resolve_entities(text);
        let text = self.escape_special_characters(&text);
        if self.ascii_bibtex {
            self.resolve_unicode(&text)
        } else {
            text
        }
    }

    /// Port of `bibtex_author_format`.
    pub fn bibtex_author_format(&self, authors: &[String]) -> String {
        self.utf8_to_bibtex(&authors.join(" and "))
    }

    /// Port of `stripUnmatchedSyntax`.
    pub fn strip_unmatched_syntax(&self, text: &str, open_char: char, close_char: char) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut stack: Vec<usize> = Vec::new();
        let mut remove: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for (i, &ch) in chars.iter().enumerate() {
            if ch == open_char {
                stack.push(i);
            } else if ch == close_char {
                if stack.pop().is_none() {
                    remove.insert(i);
                }
            }
        }
        remove.extend(stack);
        if remove.is_empty() {
            text.to_string()
        } else {
            chars.iter().enumerate().filter(|(i, _)| !remove.contains(i)).map(|(_, c)| *c).collect()
        }
    }
}

/// Port of `bibtex.py`'s local `format_isbn` usage
/// (`calibre.ebooks.metadata.format_isbn`): hyphenate a validated ISBN by
/// digit count, or return the input unchanged if it doesn't check out.
fn format_isbn(isbn: &str) -> String {
    let Some(clean) = calibre_ebooks::metadata::check_isbn(isbn) else {
        return isbn.to_string();
    };
    let i = &clean;
    if i.len() == 10 {
        format!("{}-{}-{}-{}", &i[0..2], &i[2..6], &i[6..9], &i[9..10])
    } else {
        format!("{}-{}-{}-{}-{}", &i[0..3], &i[3..5], &i[5..9], &i[9..12], &i[12..13])
    }
}

/// Port of `calibre.library.save_to_disk.preprocess_template`.
fn preprocess_template(template: &str) -> String {
    template.replace("//", "/").replace("{author}", "{authors}").replace("{tag}", "{tags}")
}

fn date_only(s: &str) -> &str {
    s.split('T').next().unwrap_or(s)
}

/// Port of `BIBTEX.cli_options` + the subset of `opts` `run` reads --
/// see this module's doc for what's dropped (CLI/GUI dual-typing,
/// non-UTF-8 output encoding).
#[derive(Debug, Clone)]
pub struct BibtexOptions {
    pub fields: String,
    pub sort_by: Option<String>,
    pub ids: Option<Vec<i32>>,
    pub is_device_connected: bool,
    pub current_library: String,
    /// `--create-citation`, default `true`.
    pub create_citation: bool,
    /// `--add-files-path`, default `true`.
    pub add_files: bool,
    /// `--citation-template`, default `"{authors}{id}"`.
    pub citation_template: String,
    /// `--choose-encoding ascii` maps to `true`; anything else (the
    /// default is `utf8`) maps to `false`.
    pub ascii_bibtex: bool,
    /// `--entry-type`, default [`BibEntryMode::Book`] (the CLI option's
    /// own documented default).
    pub entry_mode: BibEntryMode,
}

impl Default for BibtexOptions {
    fn default() -> Self {
        BibtexOptions {
            fields: "all".to_string(),
            sort_by: None,
            ids: None,
            is_device_connected: false,
            current_library: String::new(),
            create_citation: true,
            add_files: true,
            citation_template: "{authors}{id}".to_string(),
            ascii_bibtex: false,
            entry_mode: BibEntryMode::Book,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BibEntryMode {
    Book,
    Misc,
    Mixed,
}

fn fetch_rows(db: &Cache, opts: &BibtexOptions) -> Result<Vec<Value>> {
    let ids: Option<std::collections::HashSet<i32>> = opts.ids.as_ref().map(|v| v.iter().copied().collect());
    let mut rows = db.get_data_as_dict(None, false, ids.as_ref(), true).map_err(CatalogError::Db)?;
    if let Some(sort_by) = &opts.sort_by {
        rows.sort_by(|a, b| match (a.get(sort_by.as_str()), b.get(sort_by.as_str())) {
            (Some(Value::Number(x)), Some(Value::Number(y))) => x
                .as_f64()
                .unwrap_or(0.0)
                .partial_cmp(&y.as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal),
            (Some(Value::String(x)), Some(Value::String(y))) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        });
    }
    Ok(rows)
}

/// Port of `check_entry_book_valid`.
fn check_entry_book_valid(entry: &Value) -> bool {
    for field in ["title", "authors", "publisher"] {
        match entry.get(field) {
            Some(Value::String(s)) if !s.is_empty() => {}
            Some(Value::Array(a)) if !a.is_empty() => {}
            _ => return false,
        }
    }
    matches!(entry.get("pubdate"), Some(Value::String(s)) if !s.is_empty())
}

fn template_field_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{[^{}]*\}").unwrap())
}

fn brace_strip_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[{}]").unwrap())
}

/// Port of `make_bibtex_citation`/its nested `tpl_replace`.
fn make_bibtex_citation(entry: &Value, template_citation: &str, esc: &BibtexEscaper) -> String {
    let resolve_one = |field: &str| -> String {
        if !TEMPLATE_ALLOWED_FIELDS.contains(&field) {
            return String::new();
        }
        let text = match field {
            "pubdate" | "timestamp" => entry
                .get(field)
                .and_then(|v| v.as_str())
                .map(|s| date_only(s).to_string())
                .unwrap_or_default(),
            "tags" | "authors" => entry
                .get(field)
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            "id" | "series_index" => entry.get(field).map(|v| v.to_string()).unwrap_or_default(),
            _ => entry.get(field).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        };
        calibre_utils::filenames::ascii_text(&text)
    };

    if !template_citation.is_empty() {
        let substituted = template_field_re().replace_all(template_citation, |caps: &regex::Captures| {
            let field = brace_strip_re().replace_all(&caps[0], "").into_owned();
            resolve_one(&field)
        });
        let tpl_citation = esc.utf8_to_bibtex(&esc.validate_citation_key(&substituted));
        if !tpl_citation.is_empty() {
            return tpl_citation;
        }
    }

    let fallback = match entry.get("isbn").and_then(|v| v.as_str()) {
        Some(isbn) if !isbn.is_empty() => isbn.chars().filter(|c| c.is_ascii_digit()).collect(),
        _ => entry.get("id").map(|v| v.to_string()).unwrap_or_default(),
    };
    esc.validate_citation_key(&fallback)
}

fn is_effectively_empty(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Port of `create_bibtex_entry`.
fn create_bibtex_entry(
    db: &Cache,
    entry: &Value,
    fields: &[String],
    mode: BibEntryMode,
    template_citation: &str,
    esc: &BibtexEscaper,
    citation_bibtex: bool,
    calibre_files: bool,
    current_library: &str,
) -> Result<String> {
    let mut lines: Vec<String> = Vec::new();
    let is_valid_book = check_entry_book_valid(entry);
    if mode != BibEntryMode::Misc && is_valid_book {
        lines.push("@book{".to_string());
    } else if mode != BibEntryMode::Book {
        lines.push("@misc{".to_string());
    } else {
        return Ok(String::new());
    }

    if citation_bibtex {
        lines = vec![format!("{} {}", lines[0], make_bibtex_citation(entry, template_citation, esc))];
    }

    let book_id = entry["id"].as_i64().unwrap_or_default() as i32;

    for field in fields {
        let item: Value = if let Some(label) = field.strip_prefix('#') {
            match db.get_custom_column_value(book_id, label).map_err(CatalogError::Sqlite)? {
                Some(s) => Value::String(s),
                None => Value::Null,
            }
        } else if field == "title_sort" {
            entry.get("sort").cloned().unwrap_or(Value::Null)
        } else if field == "library_name" {
            Value::String(current_library.to_string())
        } else {
            entry.get(field.as_str()).cloned().unwrap_or(Value::Null)
        };

        if is_effectively_empty(&item) {
            continue;
        }

        match field.as_str() {
            "authors" => {
                if let Some(names) = item.as_array() {
                    let names: Vec<String> = names.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
                    lines.push(format!("author = \"{}\"", esc.bibtex_author_format(&names)));
                }
            }
            "languages" => {
                if let Value::Array(langs) = &item {
                    let names: Vec<String> = langs.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
                    lines.push(format!("language = \"{}\"", esc.bibtex_author_format(&names)));
                } else if let Value::String(s) = &item {
                    lines.push(format!("language = \"{s}\""));
                }
            }
            "id" => {
                if let Some(n) = item.as_i64() {
                    lines.push(format!("calibreid = \"{n}\""));
                }
            }
            "rating" => {
                if let Some(n) = item.as_f64() {
                    lines.push(format!("rating = \"{}\"", n as i64));
                }
            }
            "size" => {
                if let Some(n) = item.as_i64() {
                    lines.push(format!("size = \"{n} octets\""));
                }
            }
            "tags" => {
                if let Some(tags) = item.as_array() {
                    let tags: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
                    lines.push(format!("tags = \"{}\"", esc.utf8_to_bibtex(&tags.join(", "))));
                }
            }
            "comments" => {
                if let Some(text) = item.as_str() {
                    let text = text.replace("\r\n", " ").replace('\n', " ");
                    let text = esc.strip_unmatched_syntax(&text, '{', '}');
                    let text = calibre_utils::html2text::html2text(&text);
                    lines.push(format!("note = \"{}\"", esc.utf8_to_bibtex(&text)));
                }
            }
            "isbn" => {
                if let Some(isbn) = item.as_str() {
                    lines.push(format!("isbn = \"{}\"", format_isbn(isbn)));
                }
            }
            "formats" => {
                if let Some(paths) = item.as_array() {
                    let exts: Vec<String> = paths
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(|s| s.rsplit('.').next().unwrap_or(s).to_ascii_lowercase())
                        .collect();
                    lines.push(format!("formats = \"{}\"", exts.join(", ")));
                    if calibre_files {
                        let files: Vec<String> = paths
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| {
                                let ext = s.rsplit('.').next().unwrap_or(s);
                                format!(":{ext}:{}", ext.to_ascii_uppercase())
                            })
                            .collect();
                        lines.push(format!("file = \"{}\"", files.join(", ")));
                    }
                }
            }
            "series_index" => {
                if let Some(n) = item.as_f64() {
                    lines.push(format!("volume = \"{}\"", n as i64));
                }
            }
            "timestamp" => {
                if let Some(s) = item.as_str() {
                    lines.push(format!("timestamp = \"{}\"", date_only(s)));
                }
            }
            "pubdate" => {
                if let Some(s) = item.as_str() {
                    if let Some(dt) = calibre_utils::date::parse_date(s, true) {
                        lines.push(format!("year = \"{}\"", dt.year()));
                        lines.push(format!("month = \"{}\"", esc.utf8_to_bibtex(&dt.format("%b").to_string())));
                    }
                }
            }
            _ if field.starts_with('#') => {
                if let Some(s) = item.as_str() {
                    lines.push(format!("custom_{} = \"{}\"", &field[1..], esc.utf8_to_bibtex(s)));
                }
            }
            _ => {
                if let Some(s) = item.as_str() {
                    lines.push(format!("{field} = \"{}\"", esc.utf8_to_bibtex(s)));
                }
            }
        }
    }

    let mut out = lines.join(",\n    ");
    out.push_str(" }\n\n");
    Ok(out)
}

/// Port of `BIBTEX.run`.
#[allow(clippy::too_many_arguments)]
pub fn run(db: &Cache, path_to_output: &Path, opts: &BibtexOptions, generated_at: DateTime<Utc>) -> Result<()> {
    let fields = get_output_fields(db, &opts.fields, opts.is_device_connected)?;
    let rows = fetch_rows(db, opts)?;

    let esc = BibtexEscaper { ascii_bibtex: opts.ascii_bibtex };
    let template_citation = preprocess_template(&opts.citation_template);

    let mut nb_entries = rows.len();
    if opts.entry_mode == BibEntryMode::Book {
        let nb_books = rows.iter().filter(|e| check_entry_book_valid(e)).count();
        if nb_books < nb_entries {
            nb_entries = nb_books;
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "@preamble{{\"This catalog of {} entries was generated by calibre on {}\"}}\n\n",
        nb_entries,
        generated_at.format("%A, %d. %B %Y %H:%M")
    ));

    for entry in &rows {
        out.push_str(&create_bibtex_entry(
            db,
            entry,
            &fields,
            opts.entry_mode,
            &template_citation,
            &esc,
            opts.create_citation,
            opts.add_files,
            &opts.current_library,
        )?);
    }

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

    fn a_generated_at() -> DateTime<Utc> {
        calibre_utils::date::parse_date("2024-03-15T10:30:00Z", true).unwrap()
    }

    // --- BibtexEscaper ---

    #[test]
    fn validate_citation_key_strips_disallowed_characters() {
        let esc = BibtexEscaper::default();
        assert_eq!(esc.validate_citation_key("a b@c'd,e#f{g}h~i%j&k$l^m"), "abcdefghijklm");
    }

    #[test]
    fn utf8_to_bibtex_escapes_entities_and_special_chars() {
        let esc = BibtexEscaper::default();
        assert_eq!(esc.utf8_to_bibtex("100% done & happy"), "100\\% done \\& happy");
        assert_eq!(esc.utf8_to_bibtex("say \"hi\""), "say {\"}hi{\"}");
    }

    #[test]
    fn utf8_to_bibtex_of_empty_string_is_empty() {
        let esc = BibtexEscaper::default();
        assert_eq!(esc.utf8_to_bibtex(""), "");
    }

    #[test]
    fn strip_unmatched_syntax_removes_only_unbalanced_braces() {
        let esc = BibtexEscaper::default();
        assert_eq!(esc.strip_unmatched_syntax("a {b} c} {d", '{', '}'), "a {b} c d");
    }

    #[test]
    fn bibtex_author_format_joins_with_and() {
        let esc = BibtexEscaper::default();
        assert_eq!(esc.bibtex_author_format(&["Alice".to_string(), "Bob".to_string()]), "Alice and Bob");
    }

    // --- format_isbn / preprocess_template ---

    #[test]
    fn format_isbn_hyphenates_a_valid_isbn13() {
        // Upstream's split is a naive fixed-width [:3]/[3:5]/[5:9]/[9:12]/[12]
        // slice, not real variable-length ISBN group hyphenation -- traced
        // against a live run of the actual Python function.
        assert_eq!(format_isbn("9780306406157"), "978-03-0640-615-7");
    }

    #[test]
    fn format_isbn_returns_input_unchanged_when_invalid() {
        assert_eq!(format_isbn("not an isbn"), "not an isbn");
    }

    #[test]
    fn preprocess_template_normalizes_double_slashes_and_aliases() {
        assert_eq!(preprocess_template("a//b/{author}/{tag}"), "a/b/{authors}/{tags}");
    }

    // --- create_bibtex_entry / run ---

    #[test]
    fn a_complete_book_produces_a_book_entry() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "My Book", &["Alice"]);
        cache.set_field(id, "publisher", "Acme").unwrap();
        cache.set_field(id, "pubdate", "2020-01-01T00:00:00+00:00").unwrap();

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.bib");
        let opts = BibtexOptions { fields: "title,authors,publisher,pubdate".to_string(), ..Default::default() };
        run(&cache, &out_path, &opts, a_generated_at()).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.starts_with("@preamble{\"This catalog of 1 entries"), "{text}");
        assert!(text.contains("@book{"), "{text}");
        assert!(text.contains("author = \"Alice\""), "{text}");
        assert!(text.contains("title = \"My Book\""), "{text}");
        assert!(text.contains("year = \"2020\""), "{text}");
        assert!(text.contains("month = \"Jan\""), "{text}");
    }

    #[test]
    fn an_incomplete_book_in_strict_book_mode_produces_no_entry() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "No Publisher", &["Alice"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.bib");
        let opts = BibtexOptions {
            fields: "title,authors".to_string(),
            entry_mode: BibEntryMode::Book,
            ..Default::default()
        };
        run(&cache, &out_path, &opts, a_generated_at()).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(!text.contains("@book{") && !text.contains("@misc{"), "{text}");
    }

    #[test]
    fn misc_mode_always_produces_an_entry() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "No Publisher", &["Alice"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.bib");
        let opts = BibtexOptions {
            fields: "title,authors".to_string(),
            entry_mode: BibEntryMode::Misc,
            ..Default::default()
        };
        run(&cache, &out_path, &opts, a_generated_at()).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.contains("@misc{"), "{text}");
    }

    #[test]
    fn citation_key_falls_back_to_isbn_digits_when_the_template_yields_nothing() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T", &["A"]);
        {
            let conn = cache.backend.conn.lock().unwrap();
            conn.execute("UPDATE books SET isbn = ?1 WHERE id = ?2", ("978-0-13-1", id)).unwrap();
        }

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.bib");
        let opts = BibtexOptions {
            fields: "title".to_string(),
            citation_template: "{nonexistent_field}".to_string(),
            entry_mode: BibEntryMode::Misc,
            ..Default::default()
        };
        run(&cache, &out_path, &opts, a_generated_at()).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.contains("@misc{ 97801"), "{text}");
    }

    #[test]
    fn formats_field_adds_a_file_entry_when_calibre_files_is_enabled() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "T", &["A"]);

        let out_dir = tempdir().unwrap();
        let out_path = out_dir.path().join("catalog.bib");
        let opts = BibtexOptions {
            fields: "title,formats".to_string(),
            entry_mode: BibEntryMode::Misc,
            ..Default::default()
        };
        run(&cache, &out_path, &opts, a_generated_at()).unwrap();

        let text = std::fs::read_to_string(&out_path).unwrap();
        assert!(text.contains("formats = \"epub\""), "{text}");
        assert!(text.contains("file = \":epub:EPUB\""), "{text}");
    }
}
