//! Port of `old_src/src/calibre/library/catalogs/epub_mobi_builder.py`'s
//! `CatalogBuilder` class -- the engine behind `EPUB_MOBI.run()`
//! (`epub_mobi.py`) that generates a browsable EPUB/MOBI/AZW3 catalog's
//! source HTML/OPF/NCX files from a library's book list.
//!
//! `CatalogBuilder` is ~65 methods on one class (4337 lines) -- far larger
//! than the rest of issue #57 combined -- so this module is being ported
//! incrementally, cluster by cluster (see the persisted project memory for
//! the full plan). **What's here so far**:
//! - The sort/key helper cluster (`_kf_author_to_author_sort`,
//!   `_kf_books_by_author_sorter_author`,
//!   `_kf_books_by_author_sorter_author_sort`, `_kf_books_by_series_sorter`,
//!   `generate_sort_title`, `letter_or_symbol`, `generate_unicode_name`,
//!   `convert_html_entities`, `generate_author_anchor`,
//!   `generate_series_anchor`, `get_friendly_genre_tag`,
//!   `generate_rating_string`) -- the pure string transforms every later
//!   HTML/NCX-generating cluster calls into.
//! - The exclusion/prefix-rules sub-cluster of the data-preparation layer
//!   (`get_prefix_rules`, `discover_prefix`, `get_excluded_tags`,
//!   `filter_excluded_genres`, `process_exclusions`,
//!   `relist_multiple_authors`) -- filters and per-book prefix annotations
//!   [`fetch_books_to_catalog`] applies while building its output.
//! - `fetch_books_to_catalog` itself (`generate_short_description`,
//!   `merge_comments`, and its own inner `_populate_title` closure, ported
//!   as [`populate_title`]) -- the entry point that turns a raw
//!   [`crate::cache::Cache::get_data_as_dict`] row into the enriched
//!   `this_title` shape every later HTML/NCX cluster consumes. Needed
//!   `comments_to_html` (a whole separate module upstream,
//!   `calibre.library.comments`) as a prerequisite -- already ported at
//!   [`calibre_ebooks::oeb::transforms::jacket::comments_to_html`] for a
//!   different issue (jacket-page generation); this port found and fixed a
//!   real gap in it (a missing `<script>`/`<table>`/etc. sanitize branch)
//!   before relying on it here.
//!
//! # Disclosed simplifications
//!
//! - **`book` records are `&serde_json::Value` objects, not a `CatalogBuilder`
//!   instance's rich state.** Upstream's key functions are bound methods
//!   reading `self.output_profile`/`self.generate_for_kindle_mobi`/etc.
//!   alongside the `book` dict argument. This module has no `CatalogBuilder`
//!   struct yet (that's cluster F, ported last, once every cluster it calls
//!   into exists) -- functions here take exactly the fields they need as
//!   explicit parameters instead, expecting `book["author"]` (a `" & "`-
//!   joined string of every author, matching upstream's own
//!   `fetch_books_to_catalog`-computed field, NOT the `"authors"` array
//!   [`crate::cache::Cache::get_data_as_dict`] itself produces),
//!   `book["title_sort"]`, `book["series"]`, `book["series_index"]`, and
//!   `book["author_sort"]`.
//! - **`generate_rating_string` takes its two rating glyphs as parameters**
//!   (`full_char`/`empty_char`) rather than reading `self.SYMBOL_FULL_RATING`/
//!   `self.SYMBOL_EMPTY_RATING`, which upstream derives from a per-device
//!   `output_profile` object (`calibre.customize.ui.output_profiles`) this
//!   crate has no port of yet -- deferred to whichever cluster ports
//!   `get_output_profile`.
//! - **Prefix/exclusion rules arrive already-typed**, not as raw
//!   `opts.prefix_rules`/`opts.exclusion_rules` strings `eval()`'d into
//!   tuples -- matches this module's own established "CLI/GUI dual-typing
//!   plumbing is dropped" precedent (see `bibtex.rs`'s module doc).
//!   [`PrefixRule`] is `get_prefix_rules`'s one real reshape (a 4-tuple ->
//!   named-field struct); `get_excluded_tags`/`process_exclusions` take
//!   `&[(String, String, String)]` triples directly.
//! - **No `bools_are_tristate` preference.** `discover_prefix`/
//!   `process_exclusions`'s custom-bool-field handling (substituting a
//!   locale "False" string for a `None` bool value only when that
//!   preference is off) has no preference to check here -- this crate
//!   always takes the "preference is on" branch (no substitution), the
//!   simpler of upstream's two paths.
//! - **`process_exclusions`'s duplicate-survivor bug is fixed, not
//!   preserved.** Upstream accumulates a record into `filtered_data_set`
//!   on every exclusion-pair miss and removes it (via `list.remove`,
//!   which only strips the *first* occurrence) on a later hit -- with 2+
//!   exclusion pairs, a record that missed earlier pairs before finally
//!   matching one can survive as a literal duplicate in the output. That's
//!   not a stable, easily-replicated wrong result the way this crate's
//!   other preserved bugs are (see `rtf2xml`'s fix-vs-preserve bar) -- it's
//!   an accidental consequence of Python list-mutation order that would
//!   read as a visible defect (duplicate catalog rows) rather than
//!   intentional behavior, so this port excludes a record if *any*
//!   exclusion pair matches it, with no duplicates, matching the code's
//!   evident intent.
//! - **`genre_source_field`/`header_note_source_field` support only
//!   `"Tags"` or a `#`-prefixed custom column**, not upstream's fully
//!   general `db.get_field`/`db.metadata_for_field` (which can target any
//!   standard or custom field by name). Every real-world use of these two
//!   options is a custom column in practice (the whole point of
//!   `genre_source_field` is picking an *alternate* genre source, and
//!   `Tags` is the field it's an alternative to); a field name that's
//!   neither is treated as producing no genres/no note, not an error.
//! - **A custom `header_note_source_field` doesn't get upstream's
//!   datatype-aware reformatting** (`datetime`-typed columns re-rendered
//!   via `format_date`, `text`-typed list values joined with `" · "`) --
//!   this crate's custom columns are always scalar strings already (see
//!   the exclusion/prefix-rules simplification above), so the raw stored
//!   string is used directly.
//! - **`discover_prefix` reads the *original*, un-filtered `record`'s
//!   tags**, not the [`filter_excluded_genres`]-filtered tag list
//!   [`populate_title`] stores on its own output -- matching upstream
//!   exactly (`self.discover_prefix(record)`, not
//!   `self.discover_prefix(this_title)`).
//! - **No local-timezone conversion for the `date` display field.**
//!   Upstream renders `pubdate` via `as_local_time(...)` before
//!   formatting; this crate has no established "local timezone" concept
//!   (`calibre_utils::date` is UTC-only throughout), so the month/year
//!   string is formatted directly from the UTC value.
//! - **Short-description text extraction is simplified.** Upstream walks
//!   `comments_to_html`'s output with BeautifulSoup, joining only the
//!   *direct* string-valued children of each top-level `<p>` (a shallow,
//!   one-level walk -- `token.string` is `None`, and thus skipped, for any
//!   child tag with more than one grandchild). This port instead takes
//!   each `<p>`'s full recursive text content
//!   ([`calibre_ebooks::dom::Dom::text_content`]) -- close enough for text
//!   that's immediately truncated to a short preview via
//!   [`generate_short_description`] regardless.
//! - **`fetch_books_to_catalog`'s tag-exclusion narrows the fetched rows
//!   by post-filtering, not a search-query string.** Upstream builds a
//!   `not (tags:"=x" or tags:"=y" ...)` search phrase and folds it into
//!   `opts.search_text` before calling `search_sort_db` -- this crate's
//!   generators expect the caller to have already resolved any search to
//!   an explicit id list (see `catalogs/mod.rs`'s own doc), so tag
//!   exclusion happens as a post-fetch filter here instead. Both produce
//!   the same final row set.

use calibre_utils::icu::capitalize;
use chrono::Datelike;
use regex::Regex;
use serde_json::Value;

use crate::cache::Cache;
use crate::catalogs::CatalogError;

/// Port of `CatalogBuilder.SYMBOLS` -- upstream is `_('Symbols')` (a
/// gettext-localized string); this crate has no i18n subsystem to port
/// against (matches every other `_()` call site so far), so this is the
/// literal English default.
pub const SYMBOLS: &str = "Symbols";

/// Port of `_kf_author_to_author_sort`: `"John Smith"` -> `"Smith, john"`.
pub fn kf_author_to_author_sort(author: &str) -> String {
    let mut tokens: Vec<&str> = author.split_whitespace().collect();
    if tokens.is_empty() {
        return String::new();
    }
    let last = tokens.pop().unwrap();
    let mut rotated: Vec<String> = vec![last.to_string()];
    rotated.extend(tokens.iter().map(|s| s.to_string()));
    if rotated.len() > 1 {
        rotated[0].push(',');
    }
    capitalize(&rotated.join(" "))
}

fn series_index_suffix(index: f64) -> String {
    let integer = index.trunc();
    let fraction = index - integer;
    let frac_str = format!("{fraction:0.4}");
    let frac_trimmed = frac_str.trim_start_matches('0');
    format!("{:04}{}", integer as i64, frac_trimmed)
}

fn book_str<'a>(book: &'a Value, field: &str) -> &'a str {
    book.get(field).and_then(|v| v.as_str()).unwrap_or("")
}

fn book_f64(book: &Value, field: &str) -> f64 {
    book.get(field).and_then(|v| v.as_f64()).unwrap_or(0.0)
}

fn has_series(book: &Value) -> bool {
    !book_str(book, "series").is_empty()
}

/// Port of `_kf_books_by_author_sorter_author`: a sort key of computed
/// `author_sort` + title (series-aware -- series sort after non-series).
pub fn kf_books_by_author_sorter_author(book: &Value) -> String {
    if !has_series(book) {
        format!(
            "{} {}",
            kf_author_to_author_sort(book_str(book, "author")),
            capitalize(book_str(book, "title_sort"))
        )
    } else {
        format!(
            "{} ~{} {}",
            kf_author_to_author_sort(book_str(book, "author")),
            generate_sort_title(book_str(book, "series")),
            series_index_suffix(book_f64(book, "series_index"))
        )
    }
}

/// Port of `_kf_books_by_author_sorter_author_sort`: same shape as
/// [`kf_books_by_author_sorter_author`] but using the book's own stored
/// `author_sort` instead of a freshly-computed one, left-padded to
/// `longest_author_sort` so every key sorts by that column first.
pub fn kf_books_by_author_sorter_author_sort(book: &Value, longest_author_sort: usize) -> String {
    if !has_series(book) {
        format!(
            "{:<longest_author_sort$}!{}",
            capitalize(book_str(book, "author_sort")),
            capitalize(book_str(book, "title_sort"))
        )
    } else {
        format!(
            "{:<longest_author_sort$}~{}{}",
            capitalize(book_str(book, "author_sort")),
            generate_sort_title(book_str(book, "series")),
            series_index_suffix(book_f64(book, "series_index"))
        )
    }
}

/// Port of `_kf_books_by_series_sorter`.
pub fn kf_books_by_series_sorter(book: &Value) -> String {
    format!(
        "{} {}",
        generate_sort_title(book_str(book, "series")),
        series_index_suffix(book_f64(book, "series_index"))
    )
}

/// Port of `letter_or_symbol`: is `text` alphabetic (once transliterated
/// to ASCII)? If not, the caller should bucket it under [`SYMBOLS`]
/// instead of its own leading letter.
///
/// Upstream's docstring calls the parameter a single character, but the
/// real implementation (`re.search`, not an anchored match) just tests
/// whether *any* A-Za-z character appears anywhere in the (ASCII-ized)
/// input -- `generate_series_anchor` below relies on this by passing a
/// whole series name, not one character.
pub fn letter_or_symbol(text: &str) -> bool {
    calibre_utils::filenames::ascii_text(text).chars().any(|c| c.is_ascii_alphabetic())
}

/// Port of `generate_unicode_name`: a legal XHTML anchor built from a
/// unicode character's Unicode name (e.g. `'A'` -> `"LATIN_CAPITAL_LETTER_A"`).
/// Characters with no Unicode name (upstream would raise inside
/// `unicodedata.name`) are skipped rather than aborting the whole anchor,
/// matching this crate's "malformed input degrades gracefully" convention.
pub fn generate_unicode_name(c: &str) -> String {
    let terms: Vec<String> = c
        .chars()
        .filter_map(unicode_names2::name)
        .map(|n| n.to_string().replace(' ', "_"))
        .collect();
    terms.join("_")
}

/// Port of `convert_html_entities`.
pub fn convert_html_entities(s: &str) -> String {
    calibre_ebooks::html_entities::decode_entities(s)
}

fn strip_non_word(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect()
}

/// Port of `generate_author_anchor`.
pub fn generate_author_anchor(author: &str) -> String {
    strip_non_word(&calibre_utils::filenames::ascii_text(author))
}

/// Port of `generate_series_anchor`.
pub fn generate_series_anchor(series: &str) -> String {
    if !letter_or_symbol(series) {
        format!("symbol_{}_series", strip_non_word(series).to_lowercase())
    } else {
        format!("{}_series", strip_non_word(&calibre_utils::filenames::ascii_text(series)).to_lowercase())
    }
}

/// Port of `get_friendly_genre_tag`: the first key in `genre_tags_dict`
/// (populated by `filter_genre_tags`, a later cluster) whose value equals
/// `genre`. `genre_tags_dict` preserves insertion order (an
/// [`indexmap::IndexMap`]) since upstream relies on dict-iteration order
/// picking the *first* match deterministically.
pub fn get_friendly_genre_tag<'a>(genre_tags_dict: &'a indexmap::IndexMap<String, String>, genre: &str) -> Option<&'a str> {
    genre_tags_dict.iter().find(|(_, v)| v.as_str() == genre).map(|(k, _)| k.as_str())
}

/// Port of `generate_rating_string` -- see this module's doc for why the
/// rating glyphs are parameters rather than device-profile state.
pub fn generate_rating_string(rating: Option<f64>, full_char: &str, empty_char: &str) -> String {
    let Some(rating) = rating else { return String::new() };
    let stars = (rating as i64) / 2;
    if stars <= 0 {
        return String::new();
    }
    let stars = stars.min(5) as usize;
    format!("{}{}", full_char.repeat(stars), empty_char.repeat(5 - stars))
}

fn leading_number_to_fixed_width(word: &str) -> String {
    let cleaned = word.replace(',', "");
    match cleaned.find(|c: char| !c.is_ascii_digit()) {
        Some(suffix_start) => {
            let (num_part, suffix) = cleaned.split_at(suffix_start);
            match num_part.parse::<f64>() {
                Ok(n) => format!("{n:10.0}{suffix}"),
                Err(_) => cleaned,
            }
        }
        None => match cleaned.parse::<f64>() {
            Ok(n) => format!("{n:10.0}"),
            Err(_) => cleaned,
        },
    }
}

/// Port of `generate_sort_title`. Strips stop words via
/// [`calibre_ebooks::metadata::title_sort`], then fixed-width-pads any
/// leading numeric word in each token so purely-numeric titles sort
/// numerically rather than lexically (`"2001"` before `"10"`).
///
/// The upstream `numbers_as_text` branch (translating leading numbers to
/// English words via `NumberToText`) is dead code even in the real
/// Python source -- guarded by a literal `if False:` -- so it's not
/// reproduced here either; see [`crate::catalogs::utils`]'s own doc.
pub fn generate_sort_title(title: &str) -> String {
    let sort_title = calibre_ebooks::metadata::title_sort(title);
    let words: Vec<&str> = sort_title.split_whitespace().collect();
    let mut translated: Vec<String> = Vec::with_capacity(words.len());

    for (i, word) in words.iter().enumerate() {
        let starts_with_digit = word.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
        if i == 0 {
            let mut word = word.to_string();
            if starts_with_digit {
                word = leading_number_to_fixed_width(&word);
            }
            let first = word.chars().next().unwrap_or(' ');
            if !letter_or_symbol(&first.to_string()) {
                if first > 'A' || (('9' as u32) < (first as u32) && (first as u32) < ('A' as u32)) {
                    translated.push("/".to_string());
                }
            }
            translated.push(capitalize(&word));
        } else if starts_with_digit {
            translated.push(leading_number_to_fixed_width(word));
        } else {
            translated.push(word.to_string());
        }
    }

    translated.join(" ")
}

/// Port of the model described in `get_prefix_rules`'s docstring:
/// `('<rule name>', '<#source_field_lookup>', '<pattern>', '<prefix>')`.
#[derive(Debug, Clone)]
pub struct PrefixRule {
    pub name: String,
    pub field: String,
    pub pattern: String,
    pub prefix: String,
}

/// Port of `get_prefix_rules` -- a pure reshape now that `opts.prefix_rules`
/// arrives already-typed (see this module's doc).
pub fn get_prefix_rules(rules: &[(String, String, String, String)]) -> Vec<PrefixRule> {
    rules
        .iter()
        .map(|(name, field, pattern, prefix)| PrefixRule {
            name: name.clone(),
            field: field.clone(),
            pattern: pattern.clone(),
            prefix: prefix.clone(),
        })
        .collect()
}

fn book_id_of(book: &Value) -> i32 {
    book.get("id").and_then(|v| v.as_i64()).unwrap_or_default() as i32
}

/// Port of `discover_prefix`: the first [`PrefixRule`] whose pattern
/// matches `book`, or `None`.
pub fn discover_prefix(db: &Cache, book: &Value, prefix_rules: &[PrefixRule]) -> crate::catalogs::Result<Option<String>> {
    let tags: Vec<String> = book
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_ascii_lowercase())).collect())
        .unwrap_or_default();
    let book_id = book_id_of(book);

    for rule in prefix_rules {
        if rule.field.eq_ignore_ascii_case("tags") {
            if tags.iter().any(|t| t == &rule.pattern.to_ascii_lowercase()) {
                return Ok(Some(rule.prefix.clone()));
            }
        } else if let Some(label) = rule.field.strip_prefix('#') {
            let field_contents = db.get_custom_column_value(book_id, label).map_err(CatalogError::Sqlite)?;
            let field_contents = match field_contents.as_deref() {
                Some("") | None => None,
                Some(s) => Some(s.to_string()),
            };
            match field_contents {
                Some(contents) => {
                    if let Ok(re) = Regex::new(&format!("(?i){}", rule.pattern)) {
                        if re.is_match(&contents) {
                            return Ok(Some(rule.prefix.clone()));
                        }
                    }
                }
                None if rule.pattern == "None" => return Ok(Some(rule.prefix.clone())),
                None => {}
            }
        }
    }
    Ok(None)
}

/// Port of `get_excluded_tags`: every tag named by a `"Tags"`-field
/// exclusion rule, deduplicated. Drops upstream's console logging of
/// which books get excluded by tag -- a side effect, not part of the
/// return value.
pub fn get_excluded_tags(exclusion_rules: &[(String, String, String)]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    for (_, field, pattern) in exclusion_rules {
        if field == "Tags" {
            for tag in pattern.split(',') {
                seen.insert(tag.to_string());
            }
        }
    }
    seen.into_iter().collect()
}

/// Port of `filter_excluded_genres`: drop any tag matching `exclude_genre`
/// (after HTML-entity decoding). On a malformed regex, returns `tags`
/// unchanged, matching upstream's `except Exception: return tags`.
pub fn filter_excluded_genres(tags: &[String], exclude_genre: &str) -> Vec<String> {
    let Ok(re) = Regex::new(exclude_genre) else {
        return tags.to_vec();
    };
    tags.iter()
        .map(|t| convert_html_entities(t))
        .filter(|t| !re.is_match(t))
        .collect()
}

/// Port of `process_exclusions`: drop every book matched by a
/// custom-field (`#`-prefixed) exclusion rule. Tag-based exclusion rules
/// are handled earlier, via [`get_excluded_tags`] narrowing the search
/// query before books are ever fetched -- this only re-examines
/// already-fetched books against the *custom-field* rules. See this
/// module's doc for why the upstream duplicate-survivor behavior is fixed
/// here rather than preserved.
pub fn process_exclusions(
    db: &Cache,
    data_set: &[Value],
    exclusion_rules: &[(String, String, String)],
) -> crate::catalogs::Result<Vec<Value>> {
    let pairs: Vec<(&str, &str)> = exclusion_rules
        .iter()
        .filter(|(_, field, pat)| field.starts_with('#') && !pat.is_empty())
        .map(|(_, field, pat)| (field.as_str(), pat.as_str()))
        .collect();

    if pairs.is_empty() {
        return Ok(data_set.to_vec());
    }

    let mut filtered = Vec::with_capacity(data_set.len());
    for record in data_set {
        let book_id = book_id_of(record);
        let mut excluded = false;
        for (field, pattern) in &pairs {
            let label = field.strip_prefix('#').unwrap_or(field);
            let field_contents = db.get_custom_column_value(book_id, label).map_err(CatalogError::Sqlite)?;
            let field_contents = match field_contents.as_deref() {
                Some("") | None => None,
                Some(s) => Some(s.to_string()),
            };
            match field_contents {
                Some(contents) => {
                    if let Ok(re) = Regex::new(&format!("(?i){pattern}")) {
                        if re.is_match(&contents) {
                            excluded = true;
                            break;
                        }
                    }
                }
                None if *pattern == "None" => {
                    excluded = true;
                    break;
                }
                None => {}
            }
        }
        if !excluded {
            filtered.push(record.clone());
        }
    }
    Ok(filtered)
}

/// Port of `relist_multiple_authors`: for every book with 2+ authors, add
/// one cloned entry per additional author (each clone's `author`/
/// `author_sort`/`authors` rotated so that author leads).
pub fn relist_multiple_authors(books_by_author: &[Value]) -> Vec<Value> {
    let mut result = books_by_author.to_vec();
    for book in books_by_author {
        let Some(authors) = book.get("authors").and_then(|v| v.as_array()) else { continue };
        let authors: Vec<String> = authors.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
        if authors.len() <= 1 {
            continue;
        }
        let mut rotated = authors.clone();
        for _ in 1..authors.len() {
            let first = rotated.remove(0);
            rotated.push(first);
            let mut new_book = book.clone();
            if let Value::Object(map) = &mut new_book {
                map.insert("author".to_string(), Value::String(rotated.join(" & ")));
                map.insert("authors".to_string(), Value::from(rotated.clone()));
                let asl: Vec<String> = rotated
                    .iter()
                    .map(|a| calibre_ebooks::metadata::author_to_author_sort(a, None, None, None, None, None, None))
                    .collect();
                map.insert("author_sort".to_string(), Value::String(asl.join(" & ")));
            }
            result.push(new_book);
        }
    }
    result
}

/// Port of `generate_short_description`'s `dest` parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortDescriptionDest {
    Title,
    Author,
    Description,
}

fn short_description(description: &str, limit: usize) -> Option<String> {
    let mut short = String::new();
    for word in description.split_whitespace() {
        short.push_str(word);
        short.push(' ');
        if short.chars().count() > limit {
            short.push_str("...");
            return Some(short);
        }
    }
    // Matches upstream: if the whole description is consumed without ever
    // exceeding `limit`, `_short_description` falls off the end of its
    // `for` loop with no explicit `return`, which in Python means it
    // returns `None`. Callers only reach this function when the input is
    // already at least `limit` characters long, so this is effectively
    // unreachable in practice, not a deliberately useful `None` path.
    None
}

/// Port of `generate_short_description`.
pub fn generate_short_description(
    description: Option<&str>,
    dest: ShortDescriptionDest,
    author_clip: usize,
    description_clip: usize,
) -> Option<String> {
    let description = description.filter(|d| !d.is_empty())?;
    match dest {
        ShortDescriptionDest::Title => Some(description.to_string()),
        ShortDescriptionDest::Author => {
            if author_clip > 0 && description.chars().count() < author_clip {
                Some(description.to_string())
            } else {
                short_description(description, author_clip)
            }
        }
        ShortDescriptionDest::Description => {
            if description_clip > 0 && description.chars().count() < description_clip {
                Some(description.to_string())
            } else {
                short_description(description, description_clip)
            }
        }
    }
}

/// Port of `merge_comments_rule['position']`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergePosition {
    Before,
    After,
}

/// Port of `merge_comments`. Only ever called when a merge field is
/// actually configured -- matches upstream's own
/// `if self.merge_comments_rule['field']: ...` guard at the one call
/// site, so `field` here is required rather than `Option`.
pub fn merge_comments(
    db: &Cache,
    book_id: i32,
    description: Option<&str>,
    field: &str,
    position: MergePosition,
    hr: bool,
) -> crate::catalogs::Result<Option<String>> {
    match description {
        Some(desc) if !desc.is_empty() => {
            let addendum = db.get_custom_column_value(book_id, field).map_err(CatalogError::Sqlite)?.unwrap_or_default();
            let sep = if hr { "<hr class=\"merged_comments_divider\"/>" } else { "\n" };
            let merged = match position {
                MergePosition::Before => format!("{addendum}{sep}{desc}"),
                MergePosition::After => format!("{desc}{sep}{addendum}"),
            };
            Ok(Some(merged))
        }
        _ => Ok(db.get_custom_column_value(book_id, field).map_err(CatalogError::Sqlite)?),
    }
}

/// Port of `calibre.utils.date.is_date_undefined`: true for `None`, or a
/// date at or before calibre's `UNDEFINED_DATE` sentinel (`0101-01-01`).
pub fn is_date_undefined(dt: Option<&chrono::DateTime<chrono::Utc>>) -> bool {
    match dt {
        None => true,
        Some(d) => d.year() < 101 || (d.year() == 101 && d.month() == 1 && d.day() == 1),
    }
}

fn plain_text_paragraphs(html: &str) -> String {
    let dom = calibre_ebooks::dom::Dom::parse(html);
    dom.find_all_tag_global("p")
        .iter()
        .map(|&id| dom.text_content(id))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The subset of `opts`/`self.merge_comments_rule` [`populate_title`]
/// reads -- see this module's doc for what's narrowed relative to
/// upstream's fully general field lookups.
#[derive(Debug, Clone, Default)]
pub struct PopulateTitleOptions {
    pub exclude_genre: String,
    /// `"Tags"` or a `#`-prefixed custom column label.
    pub genre_source_field: String,
    /// A `#`-prefixed custom column label, or `None` to disable header
    /// notes entirely.
    pub header_note_source_field: Option<String>,
    /// A `#`-prefixed custom column label, or `None` to disable comment
    /// merging entirely (matches upstream's `if
    /// self.merge_comments_rule['field']:` guard).
    pub merge_comments_field: Option<String>,
    pub merge_comments_position: MergePosition,
    pub merge_comments_hr: bool,
    pub description_clip: usize,
    pub author_clip: usize,
}

impl Default for MergePosition {
    fn default() -> Self {
        MergePosition::After
    }
}

/// Port of `fetch_books_to_catalog`'s inner `_populate_title` closure:
/// turn one raw [`crate::cache::Cache::get_data_as_dict`] row into the
/// enriched `this_title` shape.
pub fn populate_title(
    db: &Cache,
    record: &Value,
    opts: &PopulateTitleOptions,
    prefix_rules: &[PrefixRule],
) -> crate::catalogs::Result<Value> {
    let mut this_title = serde_json::Map::new();
    let book_id = record.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
    let book_id_i32 = book_id as i32;
    this_title.insert("id".to_string(), Value::from(book_id));
    this_title.insert("uuid".to_string(), record.get("uuid").cloned().unwrap_or(Value::Null));

    let title = convert_html_entities(record.get("title").and_then(|v| v.as_str()).unwrap_or_default());
    this_title.insert("title".to_string(), Value::String(title.clone()));

    match record.get("series").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        Some(series) => {
            this_title.insert("series".to_string(), Value::String(series.to_string()));
            let series_index = record.get("series_index").and_then(|v| v.as_f64()).unwrap_or(0.0);
            this_title.insert("series_index".to_string(), Value::from(series_index));
        }
        None => {
            this_title.insert("series".to_string(), Value::Null);
            this_title.insert("series_index".to_string(), Value::from(0.0));
        }
    }

    this_title.insert("title_sort".to_string(), Value::String(generate_sort_title(&title)));

    let record_authors: Vec<String> = record
        .get("authors")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let author = if !record_authors.is_empty() { record_authors.join(" & ") } else { "Unknown".to_string() };
    let authors = if record_authors.is_empty() { vec![author.clone()] } else { record_authors };
    this_title.insert("authors".to_string(), Value::from(authors));
    this_title.insert("author".to_string(), Value::String(author.clone()));

    let author_sort = match record.get("author_sort").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        Some(s) => s.to_string(),
        None => kf_author_to_author_sort(&author),
    };
    this_title.insert("author_sort".to_string(), Value::String(author_sort));

    if let Some(publisher) = record.get("publisher").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        this_title.insert("publisher".to_string(), Value::String(publisher.to_string()));
    }

    let rating = record.get("rating").and_then(|v| v.as_f64()).filter(|r| *r != 0.0).unwrap_or(0.0);
    this_title.insert("rating".to_string(), Value::from(rating));

    let pubdate = record.get("pubdate").and_then(|v| v.as_str()).and_then(|s| calibre_utils::date::parse_date(s, true));
    if is_date_undefined(pubdate.as_ref()) {
        this_title.insert("date".to_string(), Value::Null);
    } else {
        let dt = pubdate.unwrap();
        this_title.insert("date".to_string(), Value::String(dt.format("%B %Y").to_string()));
    }

    this_title.insert("timestamp".to_string(), record.get("timestamp").cloned().unwrap_or(Value::Null));

    let raw_comments = record.get("comments").and_then(|v| v.as_str()).filter(|c| !c.is_empty());
    let (description, short_description_val) = match raw_comments {
        Some(comments) => {
            let mut comments = comments.to_string();
            if let Some(pos) = comments.find("<div class=\"user_annotations\">") {
                comments.truncate(pos);
            }
            if let Some(pos) = comments.find("<hr class=\"annotations_divider\" />") {
                comments.truncate(pos);
            }
            let html = calibre_ebooks::oeb::transforms::jacket::comments_to_html(&comments);
            let plain = plain_text_paragraphs(&html);
            let short = generate_short_description(
                Some(&plain),
                ShortDescriptionDest::Description,
                opts.author_clip,
                opts.description_clip,
            );
            (Some(html), short)
        }
        None => (None, None),
    };
    let description = match &opts.merge_comments_field {
        Some(field) => merge_comments(
            db,
            book_id_i32,
            description.as_deref(),
            field,
            opts.merge_comments_position,
            opts.merge_comments_hr,
        )?,
        None => description,
    };
    this_title.insert("description".to_string(), description.map(Value::String).unwrap_or(Value::Null));
    this_title.insert(
        "short_description".to_string(),
        short_description_val.map(Value::String).unwrap_or(Value::Null),
    );

    if let Some(cover) = record.get("cover").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        this_title.insert("cover".to_string(), Value::String(cover.to_string()));
    }

    let raw_tags: Vec<String> = record
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .unwrap_or_default();
    let tags = if raw_tags.is_empty() { Vec::new() } else { filter_excluded_genres(&raw_tags, &opts.exclude_genre) };
    this_title.insert("tags".to_string(), Value::from(tags.clone()));

    let genres: Vec<String> = if opts.genre_source_field == "Tags" {
        tags
    } else if let Some(label) = opts.genre_source_field.strip_prefix('#') {
        match db.get_custom_column_value(book_id_i32, label).map_err(CatalogError::Sqlite)? {
            Some(v) if !v.is_empty() => filter_excluded_genres(&[v], &opts.exclude_genre),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    this_title.insert("genres".to_string(), Value::from(genres));

    if let Some(formats) = record.get("formats").and_then(|v| v.as_array()).filter(|a| !a.is_empty()) {
        let converted: Vec<String> = formats.iter().filter_map(|v| v.as_str()).map(convert_html_entities).collect();
        this_title.insert("formats".to_string(), Value::from(converted));
    }

    if let Some(label) = &opts.header_note_source_field {
        if let Some(content) = db.get_custom_column_value(book_id_i32, label).map_err(CatalogError::Sqlite)?.filter(|c| !c.is_empty()) {
            let mut note = serde_json::Map::new();
            note.insert("source".to_string(), Value::String(label.clone()));
            note.insert("content".to_string(), Value::String(content));
            this_title.insert("notes".to_string(), Value::Object(note));
        }
    }

    // Reads the ORIGINAL record's tags, not this_title's
    // filter_excluded_genres-filtered copy -- see this module's doc.
    let prefix = discover_prefix(db, record, prefix_rules)?;
    this_title.insert("prefix".to_string(), prefix.map(Value::String).unwrap_or(Value::Null));

    Ok(Value::Object(this_title))
}

/// Port of `fetch_books_to_catalog`'s entry point (its inner
/// `_populate_title` closure is [`populate_title`], ported separately).
/// `ids` is `None` for every book, matching upstream's `opts.ids` being
/// unset.
pub fn fetch_books_to_catalog(
    db: &Cache,
    ids: Option<&[i32]>,
    exclusion_rules: &[(String, String, String)],
    prefix_rules: &[PrefixRule],
    opts: &PopulateTitleOptions,
) -> crate::catalogs::Result<Vec<Value>> {
    let ids_set: Option<std::collections::HashSet<i32>> = ids.map(|v| v.iter().copied().collect());
    let rows = db.get_data_as_dict(None, false, ids_set.as_ref(), true).map_err(CatalogError::Db)?;

    let excluded_tags: std::collections::HashSet<String> = get_excluded_tags(exclusion_rules).into_iter().collect();
    let rows: Vec<Value> = if excluded_tags.is_empty() {
        rows
    } else {
        rows.into_iter()
            .filter(|r| {
                !r.get("tags")
                    .and_then(|v| v.as_array())
                    .map(|tags| tags.iter().any(|t| t.as_str().map(|s| excluded_tags.contains(s)).unwrap_or(false)))
                    .unwrap_or(false)
            })
            .collect()
    };

    let rows = process_exclusions(db, &rows, exclusion_rules)?;

    rows.iter().map(|record| populate_title(db, record, opts, prefix_rules)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(fields: &[(&str, Value)]) -> Value {
        Value::Object(fields.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
    }

    // --- author-sort key ---

    #[test]
    fn author_to_author_sort_moves_last_name_first() {
        assert_eq!(kf_author_to_author_sort("John Smith"), "Smith, john");
    }

    #[test]
    fn author_to_author_sort_of_a_single_token_is_unchanged_shape() {
        assert_eq!(kf_author_to_author_sort("Prince"), "Prince");
    }

    #[test]
    fn author_to_author_sort_of_three_tokens_moves_only_the_last() {
        assert_eq!(kf_author_to_author_sort("John Q Smith"), "Smith, john q");
    }

    // --- series index formatting (shared by the _kf_* sorters) ---

    #[test]
    fn books_by_series_sorter_pads_the_index_to_four_digits() {
        // Even a whole-number index gets a ".0000" fraction suffix --
        // Python's `str(f'{0.0:0.4f}').lstrip('0')` strips only the
        // single leading '0' before the decimal point, not every zero.
        let b = book(&[("series", Value::String("Foundation".into())), ("series_index", Value::from(3.0))]);
        assert_eq!(kf_books_by_series_sorter(&b), "Foundation 0003.0000");
    }

    #[test]
    fn books_by_series_sorter_appends_a_stripped_fraction() {
        let b = book(&[("series", Value::String("Foundation".into())), ("series_index", Value::from(3.5))]);
        assert_eq!(kf_books_by_series_sorter(&b), "Foundation 0003.5000");
    }

    #[test]
    fn books_by_author_sorter_author_is_series_aware() {
        let standalone = book(&[
            ("author", Value::String("John Smith".into())),
            ("title_sort", Value::String("great book, the".into())),
            ("series", Value::String("".into())),
        ]);
        assert_eq!(kf_books_by_author_sorter_author(&standalone), "Smith, john Great book, the");

        let in_series = book(&[
            ("author", Value::String("John Smith".into())),
            ("series", Value::String("Foundation".into())),
            ("series_index", Value::from(2.0)),
        ]);
        assert_eq!(kf_books_by_author_sorter_author(&in_series), "Smith, john ~Foundation 0002.0000");
    }

    #[test]
    fn books_by_author_sorter_author_sort_left_pads_to_the_given_width() {
        let b = book(&[
            ("author_sort", Value::String("Smith, John".into())),
            ("title_sort", Value::String("book".into())),
            ("series", Value::String("".into())),
        ]);
        let key = kf_books_by_author_sorter_author_sort(&b, 20);
        assert_eq!(key, "Smith, john         !Book");
    }

    // --- letter_or_symbol / anchors ---

    #[test]
    fn letter_or_symbol_true_for_ascii_alphabetic_content() {
        assert!(letter_or_symbol("Café"));
        assert!(letter_or_symbol("étude")); // ascii_text transliterates é -> e
    }

    #[test]
    fn letter_or_symbol_false_for_pure_punctuation_or_digits() {
        assert!(!letter_or_symbol("123"));
        assert!(!letter_or_symbol("@#$"));
    }

    #[test]
    fn generate_author_anchor_strips_non_word_chars() {
        // `\W` strips the space too -- there's no separator left at all.
        assert_eq!(generate_author_anchor("O'Brien, Jr."), "OBrienJr");
    }

    #[test]
    fn generate_series_anchor_symbol_prefixed_for_non_alphabetic_series() {
        assert_eq!(generate_series_anchor("123"), "symbol_123_series");
    }

    #[test]
    fn generate_series_anchor_lowercases_alphabetic_series() {
        assert_eq!(generate_series_anchor("Foundation"), "foundation_series");
    }

    #[test]
    fn generate_unicode_name_builds_an_underscore_joined_name() {
        assert_eq!(generate_unicode_name("A"), "LATIN_CAPITAL_LETTER_A");
    }

    #[test]
    fn convert_html_entities_decodes_named_and_numeric_entities() {
        assert_eq!(convert_html_entities("AT&amp;T"), "AT&T");
    }

    // --- genre lookup / rating string ---

    #[test]
    fn get_friendly_genre_tag_returns_the_first_matching_key() {
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Sci-Fi".to_string(), "Science Fiction".to_string());
        dict.insert("SF".to_string(), "Science Fiction".to_string());
        assert_eq!(get_friendly_genre_tag(&dict, "Science Fiction"), Some("Sci-Fi"));
        assert_eq!(get_friendly_genre_tag(&dict, "Nonexistent"), None);
    }

    #[test]
    fn generate_rating_string_fills_stars_by_half_the_rating() {
        assert_eq!(generate_rating_string(Some(8.0), "*", "-"), "****-");
        assert_eq!(generate_rating_string(Some(10.0), "*", "-"), "*****");
        assert_eq!(generate_rating_string(None, "*", "-"), "");
        assert_eq!(generate_rating_string(Some(0.0), "*", "-"), "");
    }

    // --- generate_sort_title ---

    #[test]
    fn generate_sort_title_moves_leading_article_to_the_end() {
        // Only the FIRST word gets `capitalize()` applied (a no-op here,
        // since `title_sort` already capitalizes "Great") -- every other
        // word keeps whatever casing `title_sort` gave it, including the
        // relocated "The".
        assert_eq!(generate_sort_title("The Great Book"), "Great Book, The");
    }

    #[test]
    fn generate_sort_title_zero_pads_a_leading_number_for_numeric_sorting() {
        let sorted = generate_sort_title("2001 A Space Odyssey");
        assert!(sorted.starts_with("      2001"), "{sorted:?}");
    }

    // --- exclusion/prefix rules ---

    use tempfile::tempdir;

    fn open_test_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let cache = Cache::new(dir.path()).expect("Cache::new should succeed");
        (dir, cache)
    }

    fn write_temp_file(dir: &std::path::Path, name: &str, content: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    fn add_test_book(dir: &std::path::Path, cache: &Cache, title: &str) -> i32 {
        let source = write_temp_file(dir, &format!("{title}.epub"), b"x");
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = title.to_string();
        meta.authors = vec!["A".to_string()];
        cache.add_book(&source, &meta).unwrap()
    }

    #[test]
    fn get_prefix_rules_reshapes_tuples_into_named_fields() {
        let rules = vec![("Read".to_string(), "tags".to_string(), "+".to_string(), "\u{2713}".to_string())];
        let out = get_prefix_rules(&rules);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "Read");
        assert_eq!(out[0].field, "tags");
        assert_eq!(out[0].pattern, "+");
        assert_eq!(out[0].prefix, "\u{2713}");
    }

    #[test]
    fn discover_prefix_matches_a_tags_rule_case_insensitively() {
        let (_dir, cache) = open_test_cache();
        let rules = get_prefix_rules(&[("Read".to_string(), "tags".to_string(), "wishlist".to_string(), "W".to_string())]);
        let b = book(&[("id", Value::from(1)), ("tags", Value::from(vec!["Wishlist".to_string()]))]);
        assert_eq!(discover_prefix(&cache, &b, &rules).unwrap(), Some("W".to_string()));
    }

    #[test]
    fn discover_prefix_returns_none_when_nothing_matches() {
        let (_dir, cache) = open_test_cache();
        let rules = get_prefix_rules(&[("Read".to_string(), "tags".to_string(), "wishlist".to_string(), "W".to_string())]);
        let b = book(&[("id", Value::from(1)), ("tags", Value::from(vec!["Fiction".to_string()]))]);
        assert_eq!(discover_prefix(&cache, &b, &rules).unwrap(), None);
    }

    #[test]
    fn discover_prefix_matches_a_custom_field_regex() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.add_custom_column("status", "Status", "text", false).unwrap();
        cache.set_custom_column_value(id, "status", "Archived").unwrap();

        let rules = get_prefix_rules(&[("Arch".to_string(), "#status".to_string(), "archiv".to_string(), "A".to_string())]);
        let b = book(&[("id", Value::from(id))]);
        assert_eq!(discover_prefix(&cache, &b, &rules).unwrap(), Some("A".to_string()));
    }

    #[test]
    fn get_excluded_tags_collects_and_dedupes_tags_rule_values() {
        let rules = vec![
            ("Skip".to_string(), "Tags".to_string(), "Catalog,Archived".to_string()),
            ("Skip2".to_string(), "Tags".to_string(), "Archived".to_string()),
            ("Other".to_string(), "#status".to_string(), "x".to_string()),
        ];
        let mut tags = get_excluded_tags(&rules);
        tags.sort();
        assert_eq!(tags, vec!["Archived".to_string(), "Catalog".to_string()]);
    }

    #[test]
    fn filter_excluded_genres_drops_matching_tags_and_decodes_entities() {
        let tags = vec!["[Project Gutenberg]".to_string(), "AT&amp;T".to_string(), "Fiction".to_string()];
        let filtered = filter_excluded_genres(&tags, r"\[.+\]|^\+$");
        assert_eq!(filtered, vec!["AT&T".to_string(), "Fiction".to_string()]);
    }

    #[test]
    fn filter_excluded_genres_returns_input_unchanged_on_malformed_regex() {
        let tags = vec!["Fiction".to_string()];
        assert_eq!(filter_excluded_genres(&tags, "[unclosed"), tags);
    }

    #[test]
    fn process_exclusions_drops_books_matching_a_custom_field_rule() {
        let (dir, cache) = open_test_cache();
        let keep_id = add_test_book(dir.path(), &cache, "Keep Me");
        let drop_id = add_test_book(dir.path(), &cache, "Drop Me");
        cache.add_custom_column("status", "Status", "text", false).unwrap();
        cache.set_custom_column_value(drop_id, "status", "Archived").unwrap();

        let data = vec![book(&[("id", Value::from(keep_id))]), book(&[("id", Value::from(drop_id))])];
        let rules = vec![("Arch".to_string(), "#status".to_string(), "archiv".to_string())];
        let out = process_exclusions(&cache, &data, &rules).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["id"], Value::from(keep_id));
    }

    #[test]
    fn process_exclusions_is_a_no_op_with_no_custom_field_rules() {
        let (_dir, cache) = open_test_cache();
        let data = vec![book(&[("id", Value::from(1))])];
        let rules = vec![("Skip".to_string(), "Tags".to_string(), "Archived".to_string())];
        let out = process_exclusions(&cache, &data, &rules).unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn relist_multiple_authors_adds_one_rotated_clone_per_extra_author() {
        let books = vec![book(&[
            ("author", Value::String("Alice & Bob & Carol".to_string())),
            ("authors", Value::from(vec!["Alice".to_string(), "Bob".to_string(), "Carol".to_string()])),
        ])];
        let out = relist_multiple_authors(&books);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0]["author"], "Alice & Bob & Carol");
        assert_eq!(out[1]["author"], "Bob & Carol & Alice");
        assert_eq!(out[2]["author"], "Carol & Alice & Bob");
    }

    #[test]
    fn relist_multiple_authors_leaves_single_author_books_alone() {
        let books = vec![book(&[
            ("author", Value::String("Alice".to_string())),
            ("authors", Value::from(vec!["Alice".to_string()])),
        ])];
        let out = relist_multiple_authors(&books);
        assert_eq!(out.len(), 1);
    }

    // --- generate_short_description ---

    #[test]
    fn short_description_none_for_empty_input() {
        assert_eq!(generate_short_description(None, ShortDescriptionDest::Description, 100, 100), None);
        assert_eq!(generate_short_description(Some(""), ShortDescriptionDest::Description, 100, 100), None);
    }

    #[test]
    fn short_description_title_is_never_truncated() {
        let long = "a".repeat(500);
        assert_eq!(generate_short_description(Some(&long), ShortDescriptionDest::Title, 10, 10), Some(long));
    }

    #[test]
    fn short_description_under_the_clip_length_passes_through() {
        assert_eq!(
            generate_short_description(Some("short"), ShortDescriptionDest::Description, 100, 100),
            Some("short".to_string())
        );
    }

    #[test]
    fn short_description_over_the_clip_length_is_truncated_with_an_ellipsis() {
        let long = "one two three four five six seven eight nine ten";
        let short = generate_short_description(Some(long), ShortDescriptionDest::Description, 100, 10).unwrap();
        assert!(short.ends_with("..."), "{short:?}");
        assert!(short.len() < long.len(), "{short:?}");
    }

    // --- merge_comments ---

    #[test]
    fn merge_comments_places_the_custom_field_before_the_description() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.add_custom_column("notes", "Notes", "text", false).unwrap();
        cache.set_custom_column_value(id, "notes", "NOTE").unwrap();

        let merged = merge_comments(&cache, id, Some("DESC"), "notes", MergePosition::Before, false).unwrap();
        assert_eq!(merged, Some("NOTE\nDESC".to_string()));
    }

    #[test]
    fn merge_comments_places_the_custom_field_after_with_an_hr() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.add_custom_column("notes", "Notes", "text", false).unwrap();
        cache.set_custom_column_value(id, "notes", "NOTE").unwrap();

        let merged = merge_comments(&cache, id, Some("DESC"), "notes", MergePosition::After, true).unwrap();
        assert_eq!(merged, Some("DESC<hr class=\"merged_comments_divider\"/>NOTE".to_string()));
    }

    #[test]
    fn merge_comments_with_no_description_returns_just_the_custom_field() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.add_custom_column("notes", "Notes", "text", false).unwrap();
        cache.set_custom_column_value(id, "notes", "NOTE").unwrap();

        let merged = merge_comments(&cache, id, None, "notes", MergePosition::Before, false).unwrap();
        assert_eq!(merged, Some("NOTE".to_string()));
    }

    // --- is_date_undefined ---

    #[test]
    fn is_date_undefined_true_for_none_and_the_sentinel_date() {
        assert!(is_date_undefined(None));
        let sentinel = calibre_utils::date::parse_date("0101-01-01T00:00:00Z", true).unwrap();
        assert!(is_date_undefined(Some(&sentinel)));
    }

    #[test]
    fn is_date_undefined_false_for_a_real_date() {
        let real = calibre_utils::date::parse_date("2020-06-15T00:00:00Z", true).unwrap();
        assert!(!is_date_undefined(Some(&real)));
    }

    // --- populate_title / fetch_books_to_catalog ---

    fn default_populate_opts() -> PopulateTitleOptions {
        PopulateTitleOptions {
            exclude_genre: r"\[.+\]|^\+$".to_string(),
            genre_source_field: "Tags".to_string(),
            description_clip: 380,
            author_clip: 100,
            ..Default::default()
        }
    }

    #[test]
    fn populate_title_fills_defaults_for_a_minimal_book() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "My Book");
        let rows = cache.get_data_as_dict(None, false, None, true).unwrap();
        let record = rows.iter().find(|r| r["id"] == Value::from(id)).unwrap();

        let title = populate_title(&cache, record, &default_populate_opts(), &[]).unwrap();
        assert_eq!(title["title"], "My Book");
        assert_eq!(title["author"], "A");
        assert_eq!(title["author_sort"], "A");
        assert_eq!(title["series"], Value::Null);
        assert_eq!(title["series_index"], Value::from(0.0));
        assert_eq!(title["prefix"], Value::Null);
        assert_eq!(title["description"], Value::Null);
    }

    #[test]
    fn populate_title_computes_author_sort_when_missing() {
        let (dir, cache) = open_test_cache();
        let source = write_temp_file(dir.path(), "book.epub", b"x");
        let mut meta = calibre_ebooks::metadata::MetaInformation::default();
        meta.title = "T".to_string();
        meta.authors = vec!["John Smith".to_string()];
        let id = cache.add_book(&source, &meta).unwrap();
        // Cache::add_book defaults author_sort to the plain author name;
        // force it blank to actually exercise the "compute it" branch,
        // matching Python's `record['author_sort'].strip()` falsy check.
        cache.set_field(id, "author_sort", "").unwrap();

        let rows = cache.get_data_as_dict(None, false, None, true).unwrap();
        let record = rows.iter().find(|r| r["id"] == Value::from(id)).unwrap();
        let title = populate_title(&cache, record, &default_populate_opts(), &[]).unwrap();
        assert_eq!(title["author_sort"], "Smith, john");
    }

    #[test]
    fn populate_title_converts_comments_to_html_and_a_short_description() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.set_field(id, "comments", "Hello world").unwrap();

        let rows = cache.get_data_as_dict(None, false, None, true).unwrap();
        let record = rows.iter().find(|r| r["id"] == Value::from(id)).unwrap();
        let title = populate_title(&cache, record, &default_populate_opts(), &[]).unwrap();
        assert_eq!(title["description"], "<p class=\"description\">Hello world</p>");
        assert_eq!(title["short_description"], "Hello world");
    }

    #[test]
    fn populate_title_filters_excluded_genre_tags() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.set_field(id, "tags", "Fiction, [Project Gutenberg]").unwrap();

        let rows = cache.get_data_as_dict(None, false, None, true).unwrap();
        let record = rows.iter().find(|r| r["id"] == Value::from(id)).unwrap();
        let title = populate_title(&cache, record, &default_populate_opts(), &[]).unwrap();
        assert_eq!(title["tags"], Value::from(vec!["Fiction".to_string()]));
        assert_eq!(title["genres"], Value::from(vec!["Fiction".to_string()]));
    }

    #[test]
    fn populate_title_applies_a_matching_prefix_rule() {
        let (dir, cache) = open_test_cache();
        let id = add_test_book(dir.path(), &cache, "T");
        cache.set_field(id, "tags", "Wishlist").unwrap();

        let rows = cache.get_data_as_dict(None, false, None, true).unwrap();
        let record = rows.iter().find(|r| r["id"] == Value::from(id)).unwrap();
        let rules = get_prefix_rules(&[("W".to_string(), "tags".to_string(), "wishlist".to_string(), "\u{d7}".to_string())]);
        let title = populate_title(&cache, record, &default_populate_opts(), &rules).unwrap();
        assert_eq!(title["prefix"], "\u{d7}");
    }

    #[test]
    fn fetch_books_to_catalog_excludes_books_by_tag() {
        let (dir, cache) = open_test_cache();
        let keep = add_test_book(dir.path(), &cache, "Keep");
        let drop = add_test_book(dir.path(), &cache, "Drop");
        cache.set_field(drop, "tags", "Catalog").unwrap();

        let exclusion_rules = vec![("Skip".to_string(), "Tags".to_string(), "Catalog".to_string())];
        let titles = fetch_books_to_catalog(&cache, None, &exclusion_rules, &[], &default_populate_opts()).unwrap();
        let ids: Vec<i64> = titles.iter().map(|t| t["id"].as_i64().unwrap()).collect();
        assert!(ids.contains(&(keep as i64)));
        assert!(!ids.contains(&(drop as i64)));
    }

    #[test]
    fn fetch_books_to_catalog_excludes_books_by_custom_field() {
        let (dir, cache) = open_test_cache();
        let keep = add_test_book(dir.path(), &cache, "Keep");
        let drop = add_test_book(dir.path(), &cache, "Drop");
        cache.add_custom_column("status", "Status", "text", false).unwrap();
        cache.set_custom_column_value(drop, "status", "Archived").unwrap();

        let exclusion_rules = vec![("Arch".to_string(), "#status".to_string(), "archiv".to_string())];
        let titles = fetch_books_to_catalog(&cache, None, &exclusion_rules, &[], &default_populate_opts()).unwrap();
        let ids: Vec<i64> = titles.iter().map(|t| t["id"].as_i64().unwrap()).collect();
        assert!(ids.contains(&(keep as i64)));
        assert!(!ids.contains(&(drop as i64)));
    }
}
