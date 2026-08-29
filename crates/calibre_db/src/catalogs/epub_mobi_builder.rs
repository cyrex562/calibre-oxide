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
//! - `detect_author_sort_mismatches`, `fetch_books_by_title`,
//!   `fetch_books_by_author`, and `generate_format_args` -- close out
//!   cluster A (the data-preparation layer). `get_output_profile` remains
//!   deferred (needs `calibre.customize.ui.output_profiles()`, a whole
//!   device-profile registry this crate has no port of), and
//!   `fetch_bookmarks` is intentionally skipped -- upstream's own
//!   docstring says it's been turned off since calibre 0.8.70.
//! - `filter_genre_tags` and `establish_equivalencies` -- prerequisites
//!   for cluster C's genre/alphabetical-section HTML generators, ported
//!   ahead of that cluster since they're small and self-contained.
//!   `filter_genre_tags` needed a new [`crate::cache::Cache::all_tags`]
//!   (a small, genuinely reusable primitive, added alongside this) and is
//!   narrowed to upstream's `"Tags"` `genre_source_field` case only (a
//!   custom `#field` genre source needs `Cache::all_custom`-style
//!   distinct-value querying this crate doesn't have). `establish_
//!   equivalencies` approximates real ICU `collation_order` with plain
//!   per-character Unicode uppercasing, the same "no real ICU collation"
//!   gap as the sort-key simplification below.
//!   `dump_custom_fields` is a `self.opts.verbose`-gated debug-log dumper
//!   with no return value and no effect on any generated output --
//!   omitted entirely (not even a no-op stub), matching this crate's
//!   standing convention of dropping debug-print-only code paths.
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
//! - **No real ICU collation.** Upstream sorts by `sort_key(...)`
//!   (`calibre.utils.icu`'s locale-aware collation key) in
//!   `fetch_books_by_author`/`fetch_books_by_title`; this crate has no
//!   `sort_key`/`collation_order` port (`icu.rs`'s own doc already frames
//!   it as "for now we use Rust Standard Library unicode methods" for
//!   every function it *does* have), so these sort by the computed key
//!   string's plain `Ord` instead -- correct for ASCII-range titles/author
//!   names, an approximation for accented/non-Latin ones.

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

/// Port of `detect_author_sort_mismatches`. Returns one warning string per
/// non-fatal mismatch (upstream's `self.error.append(...)` accumulation)
/// for MOBI-format is instead a hard error on the *first* mismatch
/// ([`CatalogError::AuthorSortMismatch`]), matching upstream's own
/// `raise AuthorSortMismatchException` for that format.
pub fn detect_author_sort_mismatches(books_to_test: &[Value], fmt: &str) -> crate::catalogs::Result<Vec<String>> {
    if books_to_test.is_empty() {
        return Ok(Vec::new());
    }
    let mut books_by_author = books_to_test.to_vec();
    books_by_author.sort_by(|a, b| kf_books_by_author_sorter_author(a).cmp(&kf_books_by_author_sorter_author(b)));

    let authors: Vec<(String, String)> =
        books_by_author.iter().map(|r| (book_str(r, "author").to_string(), book_str(r, "author_sort").to_string())).collect();

    let mut warnings = Vec::new();
    let mut current_author = authors[0].clone();
    for (i, author) in authors.iter().enumerate() {
        if *author != current_author && i > 0 {
            if author.0 == current_author.0 {
                if fmt == "mobi" {
                    return Err(CatalogError::AuthorSortMismatch(format!(
                        "Inconsistent author sort values for author '{}': {} != {}",
                        author.0, author.1, current_author.1
                    )));
                }
                warnings.push(format!(
                    "Warning: Inconsistent author sort values for author '{}':\n {} != {}\n",
                    author.0, author.1, current_author.1
                ));
                continue;
            }
            current_author = author.clone();
        }
    }
    Ok(warnings)
}

/// Port of `fetch_books_by_title`.
pub fn fetch_books_by_title(books_to_catalog: &[Value]) -> crate::catalogs::Result<Vec<Value>> {
    if books_to_catalog.is_empty() {
        return Err(CatalogError::EmptyCatalog);
    }
    let mut books = books_to_catalog.to_vec();
    books.sort_by(|a, b| book_str(a, "title_sort").to_uppercase().cmp(&book_str(b, "title_sort").to_uppercase()));
    Ok(books)
}

/// Shared run-length group-by used by both `fetch_books_by_author` and
/// `generate_html_by_genres`'s own near-identical (and, in upstream, even
/// more severely bugged -- see that function's own doc) unique-authors
/// tally: `pairs` must already be grouped so identical `(friendly, sort)`
/// pairs are consecutive (both callers' inputs already are, by
/// construction). Returns `(friendly, sort, count)` triples, one per
/// distinct run.
fn group_consecutive_authors(pairs: &[(String, String)]) -> Vec<(String, String, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pairs.len() {
        let current = &pairs[i];
        let mut count = 1;
        while i + count < pairs.len() && &pairs[i + count] == current {
            count += 1;
        }
        out.push((current.0.clone(), current.1.clone(), count));
        i += count;
    }
    out
}

/// Port of `fetch_books_by_author`'s output: `self.books_by_author`,
/// `self.books_by_description`, `self.authors` (unique authors as
/// `(friendly, title-cased sort, book count)`), and
/// `self.individual_authors`.
#[derive(Debug, Clone, Default)]
pub struct FetchBooksByAuthorResult {
    pub books_by_author: Vec<Value>,
    pub books_by_description: Option<Vec<Value>>,
    pub authors: Vec<(String, String, usize)>,
    pub individual_authors: Vec<String>,
    /// Non-fatal author_sort-mismatch warnings from
    /// [`detect_author_sort_mismatches`] (dropped for MOBI, which errors
    /// out of this function entirely on the first mismatch instead).
    pub warnings: Vec<String>,
}

/// Port of `fetch_books_by_author`.
///
/// The unique-authors grouping loop is a straightforward run-length
/// group-by over the (already author-sorted) book list rather than a
/// literal translation of upstream's branch structure -- upstream's
/// version has a genuine bug for a catalog containing *exactly one book
/// total*: the single-book path takes a loop branch that never
/// increments the book-count accumulator, then the post-loop "final
/// author" check fires *again* despite the single-book branch already
/// having appended an entry, leaving `unique_authors` with two duplicate
/// `(author, title, 0)` entries instead of one `(author, title, 1)`. Not
/// a stable, intentional-looking result (a visible double-counted-zero
/// glitch, only for the rarest possible catalog size) -- fixed here
/// rather than preserved, same bar as `process_exclusions`'s
/// duplicate-survivor fix.
pub fn fetch_books_by_author(
    books_to_catalog: &[Value],
    books_by_title: Option<&[Value]>,
    cross_reference_authors: bool,
    generate_descriptions: bool,
    sort_descriptions_by_author: bool,
    fmt: &str,
) -> crate::catalogs::Result<FetchBooksByAuthorResult> {
    let mut books_by_author = books_to_catalog.to_vec();
    let warnings = detect_author_sort_mismatches(&books_by_author, fmt)?;

    let mut books_by_description = if generate_descriptions {
        Some(if sort_descriptions_by_author {
            books_by_author.clone()
        } else {
            books_by_title.map(|t| t.to_vec()).unwrap_or_default()
        })
    } else {
        None
    };

    if cross_reference_authors {
        books_by_author = relist_multiple_authors(&books_by_author);
    }

    let longest_author_sort = books_by_author.iter().map(|b| book_str(b, "author_sort").chars().count()).max().unwrap_or(0);
    let sort_key_of = |b: &Value| kf_books_by_author_sorter_author_sort(b, longest_author_sort);

    if let Some(v) = &mut books_by_description {
        v.sort_by(|a, b| sort_key_of(a).cmp(&sort_key_of(b)));
    }
    books_by_author.sort_by(|a, b| sort_key_of(a).cmp(&sort_key_of(b)));

    let authors: Vec<(String, String)> =
        books_by_author.iter().map(|r| (book_str(r, "author").to_string(), capitalize(book_str(r, "author_sort")))).collect();

    let unique_authors: Vec<(String, String, usize)> =
        group_consecutive_authors(&authors).into_iter().map(|(a, s, c)| (a, calibre_utils::icu::title_case(&s), c)).collect();

    let mut individual_authors: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (friendly, _, _) in &unique_authors {
        for a in friendly.replace(" &amp; ", " & ").split(" & ") {
            individual_authors.insert(a.to_string());
        }
    }

    Ok(FetchBooksByAuthorResult {
        books_by_author,
        books_by_description,
        authors: unique_authors,
        individual_authors: individual_authors.into_iter().collect(),
        warnings,
    })
}

/// Port of `generate_format_args`'s output shape (`format`-template
/// substitution args for the `by_*_template.py` section templates --
/// those templates are cluster C's concern, not yet ported).
#[derive(Debug, Clone, Default)]
pub struct FormatArgs {
    pub title: String,
    pub series: Option<String>,
    pub series_index: String,
    pub rating: String,
    pub rating_parens: String,
    pub pubyear: String,
    pub pubyear_parens: String,
}

/// Port of `generate_format_args`. `full_char`/`empty_char` are
/// [`generate_rating_string`]'s own deferred device-profile parameters
/// (see this module's doc). `rating_parens` is non-empty whenever `book`
/// has a `"rating"` key at all (even a zero rating) -- since
/// [`populate_title`]'s output always sets one, every book this is
/// realistically called on gets a `rating_parens` value, even if
/// `rating` itself is `""` for an unrated book (matching upstream's own
/// `'rating' in book` key-existence check, not a truthiness check).
pub fn generate_format_args(book: &Value, full_char: &str, empty_char: &str) -> FormatArgs {
    let series_index_raw = book.get("series_index").map(|v| v.to_string()).unwrap_or_default();
    let series_index = series_index_raw.strip_suffix(".0").unwrap_or(&series_index_raw).to_string();

    let rating = generate_rating_string(book.get("rating").and_then(|v| v.as_f64()), full_char, empty_char);
    let rating_parens = if book.get("rating").is_some() { format!("({rating})") } else { String::new() };

    let pubyear =
        book.get("date").and_then(|v| v.as_str()).and_then(|d| d.split_whitespace().nth(1)).unwrap_or("").to_string();
    let pubyear_parens = if !pubyear.is_empty() { format!("({pubyear})") } else { String::new() };

    FormatArgs {
        title: book.get("title").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        series: book.get("series").and_then(|v| v.as_str()).map(|s| s.to_string()),
        series_index,
        rating,
        rating_parens,
        pubyear,
        pubyear_parens,
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn normalize_tag(tag: &str, max_len: usize) -> String {
    let massaged: String =
        calibre_utils::filenames::ascii_text(tag).to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let normalized = if massaged.chars().any(|c| !is_word_char(c)) {
        massaged.chars().map(|c| if is_word_char(c) { c.to_string() } else { generate_unicode_name(&c.to_string()) }).collect()
    } else {
        massaged
    };
    calibre_utils::filenames::limit_component(&normalized, max_len)
}

/// Port of `filter_genre_tags`, narrowed to upstream's `"Tags"`
/// `genre_source_field` case (see this module's doc for why a custom
/// `#field` genre source isn't supported here). Drops upstream's
/// verbose-only "multiple tags resolving to the same normalized genre"
/// warning log -- a diagnostic side effect, not part of the returned
/// dict.
pub fn filter_genre_tags(
    db: &Cache,
    max_len: usize,
    excluded_tags: &[String],
    exclude_genre: &str,
) -> crate::catalogs::Result<indexmap::IndexMap<String, String>> {
    let mut all_genre_tags = db.all_tags().map_err(CatalogError::Sqlite)?;
    all_genre_tags.sort();

    let excluded_set: std::collections::HashSet<&str> = excluded_tags.iter().map(|s| s.as_str()).collect();
    let re = Regex::new(exclude_genre).ok();

    let mut genre_tags_dict = indexmap::IndexMap::new();
    for tag in &all_genre_tags {
        if excluded_set.contains(tag.as_str()) {
            continue;
        }
        if re.as_ref().is_some_and(|re| re.is_match(tag)) {
            continue;
        }
        if tag == " " {
            continue;
        }
        genre_tags_dict.insert(tag.clone(), normalize_tag(tag, max_len));
    }
    Ok(genre_tags_dict)
}

/// Port of `establish_equivalencies`'s `key=None` (plain string list)
/// case -- the only shape `fetch_books_by_author`/`fetch_books_by_title`
/// actually call it with (`key=sort_field` is unused in the real
/// pipeline). Approximated via per-item Unicode uppercasing of just the
/// leading character (with the same `Ä`/`Ö`/`Ü` -> `A`/`O`/`U`
/// exceptions upstream hardcodes) rather than real ICU
/// `collation_order`, which this crate has no port of (same "no real
/// ICU collation" gap as the sort-key simplification in this module's
/// doc). Real ICU `collation_order` can group multi-character collation
/// units (e.g. Spanish "ch") under one heading and generally strips
/// accents more broadly than upstream's narrow three-letter exception
/// list would suggest; this simplified version does neither -- it's a
/// per-character approximation, not a locale-correct one.
pub fn establish_equivalencies(items: &[String]) -> Vec<String> {
    let exceptions: std::collections::HashMap<char, char> = [('Ä', 'A'), ('Ö', 'O'), ('Ü', 'U')].into_iter().collect();
    items
        .iter()
        .map(|item| {
            let first = item.chars().next().unwrap_or(' ');
            let upper_char = first.to_uppercase().next().unwrap_or(first);
            exceptions.get(&upper_char).copied().unwrap_or(upper_char).to_string()
        })
        .collect()
}

// ===================================================================
// Cluster C: HTML section generators
// ===================================================================
//
// Shared plumbing for every `generate_html_by_*` function: an
// `empty_html_document` skeleton (`generate_html_empty_header`), a
// prefix-glyph snippet builder (`insert_prefix`), and a narrow
// `{name}`-token template substitution (`safe_format`) -- the same
// grammar `calibre_ebooks::oeb::transforms::jacket`'s own private
// `safe_format` already implements for a different issue, reimplemented
// here (rather than made `pub` and shared) since it's a handful of lines
// and this crate's established convention favors small per-file
// duplicates over forcing a shared abstraction across crates for
// something this size. Every default section template string below is
// copied verbatim from `old_src/resources/catalog/section_list_templates
// .conf` (`load_section_templates`'s real source) -- upstream lets users
// override these with local file copies; this crate always uses the
// shipped defaults (same "CLI/GUI dual-typing plumbing dropped"
// precedent as everywhere else in this file).

const NBSP: &str = "\u{a0}";

pub const BY_AUTHORS_NORMAL_TITLE_TEMPLATE: &str = "{title} {pubyear_parens}";
pub const BY_AUTHORS_SERIES_TITLE_TEMPLATE: &str = "[{series_index}] {title} {pubyear_parens}";

fn set_attr(dom: &mut calibre_ebooks::dom::Dom, id: calibre_ebooks::dom::NodeId, name: &str, value: impl Into<String>) {
    dom.node_mut(id).attrs.insert(name.to_string(), value.into());
}

fn append_text(dom: &mut calibre_ebooks::dom::Dom, parent: calibre_ebooks::dom::NodeId, text: &str) {
    let t = dom.new_text(text);
    dom.append_child(parent, t);
}

/// Port of `generate_html_empty_header`: a boilerplate XHTML skeleton
/// with `title` as the document title. The DOCTYPE line is prepended as
/// a literal string rather than round-tripped through
/// [`calibre_ebooks::dom::Dom`]'s parser/serializer -- `Dom`'s `NodeKind`
/// has no `Doctype` variant (it's built for HTML5-tag-soup element
/// trees, not preserving arbitrary XML prolog declarations), so a
/// parse-then-serialize round trip would silently drop it.
fn empty_html_document(title: &str) -> (calibre_ebooks::dom::Dom, calibre_ebooks::dom::NodeId, calibre_ebooks::dom::NodeId) {
    let mut dom = calibre_ebooks::dom::Dom::empty();
    let root = dom.root;
    let html = dom.new_element("html");
    set_attr(&mut dom, html, "xmlns", "http://www.w3.org/1999/xhtml");
    set_attr(&mut dom, html, "xmlns:calibre", "http://calibre.kovidgoyal.net/2009/metadata");
    dom.append_child(root, html);

    let head = dom.new_element("head");
    dom.append_child(html, head);
    let meta = dom.new_element("meta");
    set_attr(&mut dom, meta, "http-equiv", "Content-Type");
    set_attr(&mut dom, meta, "content", "text/html; charset=UTF-8");
    dom.append_child(head, meta);
    let link = dom.new_element("link");
    set_attr(&mut dom, link, "rel", "stylesheet");
    set_attr(&mut dom, link, "type", "text/css");
    set_attr(&mut dom, link, "href", "stylesheet.css");
    set_attr(&mut dom, link, "media", "screen");
    dom.append_child(head, link);
    let title_tag = dom.new_element("title");
    append_text(&mut dom, title_tag, title);
    dom.append_child(head, title_tag);

    let body = dom.new_element("body");
    dom.append_child(html, body);

    (dom, root, body)
}

/// Serializes an [`empty_html_document`]-built tree with the DOCTYPE
/// line upstream's literal header string carries, prepended back on.
fn serialize_html_document(dom: &calibre_ebooks::dom::Dom, root: calibre_ebooks::dom::NodeId) -> String {
    format!(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n{}",
        dom.serialize(root)
    )
}

/// Port of `insert_prefix`.
fn insert_prefix(dom: &mut calibre_ebooks::dom::Dom, parent: calibre_ebooks::dom::NodeId, fmt: &str, prefix_char: Option<&str>) {
    let tag = dom.new_element(if fmt == "mobi" { "code" } else { "span" });
    if fmt != "mobi" {
        set_attr(dom, tag, "class", "prefix");
    }
    let content = prefix_char.filter(|s| !s.is_empty()).unwrap_or(NBSP);
    append_text(dom, tag, content);
    dom.append_child(parent, tag);
}

fn template_token_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap())
}

/// Port of `Formatter.safe_format`, narrowed to the bare-`{name}` grammar
/// every section template in `section_list_templates.conf` actually
/// uses (no `{name.attr}`/function-call/conditional syntax the real
/// `TemplateFormatter` supports for computed-column templates elsewhere
/// in calibre) -- same narrowing `jacket.rs`'s own `safe_format` already
/// applies for a different template set, for the same reason (see this
/// section's own doc).
fn safe_format(template: &str, args: &FormatArgs) -> String {
    template_token_re()
        .replace_all(template, |caps: &regex::Captures| match &caps[1] {
            "title" => args.title.clone(),
            "series" => args.series.clone().unwrap_or_default(),
            "series_index" => args.series_index.clone(),
            "rating" => args.rating.clone(),
            "rating_parens" => args.rating_parens.clone(),
            "pubyear" => args.pubyear.clone(),
            "pubyear_parens" => args.pubyear_parens.clone(),
            _ => String::new(),
        })
        .into_owned()
}

/// Port of `letter_or_symbol`'s actual return value (as opposed to
/// [`letter_or_symbol`]'s boolean-only test, used by the two callers
/// that only need the test): the input string if it's alphabetic
/// (ASCII-ized), or [`SYMBOLS`] if not.
pub fn letter_or_symbol_str(text: &str) -> String {
    if letter_or_symbol(text) {
        text.to_string()
    } else {
        SYMBOLS.to_string()
    }
}

/// Port of `generate_html_by_author`: renders `content/ByAlphaAuthor.html`'s
/// body. Returns the finished HTML document string -- writing it to
/// `content_dir` and tracking it in `html_filelist_1` is the caller's job
/// (cluster F, not yet ported), matching this crate's "pure function,
/// caller does I/O" convention used throughout this module.
///
/// `rating_full_char`/`rating_empty_char` thread through to
/// [`generate_format_args`] (itself threading to
/// [`generate_rating_string`]) -- deferred device-profile parameters
/// that don't actually affect this particular template's output (neither
/// `BY_AUTHORS_NORMAL_TITLE_TEMPLATE` nor `BY_AUTHORS_SERIES_TITLE_TEMPLATE`
/// reference `{rating}`/`{rating_parens}`), but are still required
/// arguments since `generate_format_args` computes them unconditionally.
pub fn generate_html_by_author(
    books_by_author: &[Value],
    fmt: &str,
    generate_for_kindle_mobi: bool,
    generate_series: bool,
    generate_descriptions: bool,
    rating_full_char: &str,
    rating_empty_char: &str,
) -> String {
    let friendly_name = "Authors";
    let (mut dom, root, body) = empty_html_document(friendly_name);

    let div_tag = dom.new_element("div");
    let mut div_opening_tag: Option<calibre_ebooks::dom::NodeId> = None;
    let mut div_running_tag: Option<calibre_ebooks::dom::NodeId> = None;

    let mut author_count = 0usize;
    let mut current_author = String::new();
    let mut current_letter = String::new();
    let mut current_series: Option<String> = None;

    let author_sorts: Vec<String> = books_by_author.iter().map(|b| book_str(b, "author_sort").to_string()).collect();
    let sort_equivalents = establish_equivalencies(&author_sorts);

    for (idx, book) in books_by_author.iter().enumerate() {
        let letter_candidate = letter_or_symbol_str(&sort_equivalents[idx]);
        if letter_candidate != current_letter {
            if let Some(opening) = div_opening_tag.take() {
                dom.append_child(div_tag, opening);
            }
            if let Some(running) = div_running_tag.take() {
                dom.append_child(div_tag, running);
            }
            author_count = 0;

            let opening = dom.new_element("div");
            if !dom.children(div_tag).is_empty() {
                set_attr(&mut dom, opening, "class", "initial_letter");
            }

            let p_index = dom.new_element("p");
            set_attr(&mut dom, p_index, "class", "author_title_letter_index");
            let a_tag = dom.new_element("a");
            current_letter = letter_candidate;
            if current_letter == SYMBOLS {
                set_attr(&mut dom, a_tag, "id", format!("{SYMBOLS}_authors"));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, SYMBOLS);
            } else {
                set_attr(&mut dom, a_tag, "id", format!("{}_authors", generate_unicode_name(&current_letter)));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, &sort_equivalents[idx]);
            }
            dom.append_child(opening, p_index);
            div_opening_tag = Some(opening);
        }

        let author = book_str(book, "author").to_string();
        if author != current_author {
            current_author = author.clone();
            author_count += 1;
            if author_count >= 2 {
                if let Some(opening) = div_opening_tag.take() {
                    dom.append_child(div_tag, opening);
                }
                if author_count > 2 {
                    if let Some(running) = div_running_tag.take() {
                        dom.append_child(div_tag, running);
                    }
                }
                let running = dom.new_element("div");
                set_attr(&mut dom, running, "class", "author_logical_group");
                div_running_tag = Some(running);
            }

            current_series = None;
            let p_author = dom.new_element("p");
            set_attr(&mut dom, p_author, "class", "author_index");
            let a_tag = dom.new_element("a");
            set_attr(&mut dom, a_tag, "id", generate_author_anchor(&current_author));
            append_text(&mut dom, a_tag, &current_author);
            dom.append_child(p_author, a_tag);
            let target = if author_count == 1 { div_opening_tag.unwrap() } else { div_running_tag.unwrap() };
            dom.append_child(target, p_author);
        }

        match book.get("series").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(series) if Some(series.to_string()) != current_series => {
                current_series = Some(series.to_string());
                let p_series = dom.new_element("p");
                set_attr(&mut dom, p_series, "class", if fmt == "mobi" { "series_mobi" } else { "series" });
                if generate_series {
                    let a_tag = dom.new_element("a");
                    set_attr(&mut dom, a_tag, "href", format!("BySeries.html#{}", generate_series_anchor(series)));
                    append_text(&mut dom, a_tag, series);
                    dom.append_child(p_series, a_tag);
                } else {
                    append_text(&mut dom, p_series, series);
                }
                if author_count == 1 {
                    dom.append_child(div_opening_tag.unwrap(), p_series);
                } else if let Some(running) = div_running_tag {
                    dom.append_child(running, p_series);
                }
            }
            None => current_series = None,
            _ => {}
        }

        let p_book = dom.new_element("p");
        set_attr(&mut dom, p_book, "class", "line_item");
        insert_prefix(&mut dom, p_book, fmt, book.get("prefix").and_then(|v| v.as_str()));

        let span_tag = dom.new_element("span");
        set_attr(&mut dom, span_tag, "class", "entry");
        let a_tag = dom.new_element("a");
        if generate_descriptions {
            let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            set_attr(&mut dom, a_tag, "href", format!("book_{book_id}.html"));
        }
        let args = generate_format_args(book, rating_full_char, rating_empty_char);
        let template = if current_series.is_some() { BY_AUTHORS_SERIES_TITLE_TEMPLATE } else { BY_AUTHORS_NORMAL_TITLE_TEMPLATE };
        append_text(&mut dom, a_tag, &safe_format(template, &args));
        dom.append_child(span_tag, a_tag);
        dom.append_child(p_book, span_tag);

        let target = if author_count == 1 { div_opening_tag.unwrap() } else { div_running_tag.unwrap() };
        dom.append_child(target, p_book);
    }

    let p_title = dom.new_element("p");
    set_attr(&mut dom, p_title, "class", "title");
    let a_section_start = dom.new_element("a");
    set_attr(&mut dom, a_section_start, "id", "section_start");
    dom.append_child(p_title, a_section_start);
    if !generate_for_kindle_mobi {
        let a_tag = dom.new_element("a");
        set_attr(&mut dom, a_tag, "id", friendly_name.to_lowercase().replace(' ', ""));
        dom.append_child(p_title, a_tag);
        append_text(&mut dom, p_title, friendly_name);
    }
    dom.insert_child(body, 0, p_title);

    if let Some(opening) = div_opening_tag.take() {
        dom.append_child(div_tag, opening);
    } else if let Some(running) = div_running_tag.take() {
        dom.append_child(div_tag, running);
    }
    dom.append_child(body, div_tag);

    serialize_html_document(&dom, root)
}

pub const BY_TITLES_NORMAL_TITLE_TEMPLATE: &str = "{title}";
pub const BY_TITLES_SERIES_TITLE_TEMPLATE: &str = "{title} ({series} [{series_index}])";

/// Port of `generate_html_by_title`: renders `content/ByAlphaTitle.html`'s
/// body. Unlike `generate_html_by_author`, there's no per-author
/// sub-grouping -- each letter section is one flat `<div>` of book lines,
/// each showing the title followed by " · " and a link to the author.
///
/// Upstream re-derives a `books_by_title_no_series_prefix` list (a
/// `deepcopy` of `books_to_catalog` re-sorted the same way) gated behind
/// `self.use_series_prefix_in_titles_section`, which `__init__` hardcodes
/// to `False` and nothing else in the class ever sets -- meaning that
/// branch is always taken, and the re-derived list is sorted identically
/// to (and therefore content-equivalent to) `books_by_title` itself (same
/// source list, same `sort_key(title_sort.upper())` key). This port just
/// takes `books_by_title` directly rather than reproducing the always-
/// taken, always-equivalent re-derivation.
pub fn generate_html_by_title(
    books_by_title: &[Value],
    fmt: &str,
    generate_for_kindle_mobi: bool,
    generate_descriptions: bool,
    generate_authors: bool,
    rating_full_char: &str,
    rating_empty_char: &str,
) -> String {
    let (mut dom, root, body) = empty_html_document("Books By Alpha Title");

    let p_title = dom.new_element("p");
    set_attr(&mut dom, p_title, "class", "title");
    let a_section_start = dom.new_element("a");
    set_attr(&mut dom, a_section_start, "id", "section_start");
    dom.append_child(p_title, a_section_start);
    if !generate_for_kindle_mobi {
        let a_tag = dom.new_element("a");
        set_attr(&mut dom, a_tag, "id", "bytitle");
        dom.append_child(p_title, a_tag);
        append_text(&mut dom, p_title, "Titles");
    }
    dom.append_child(body, p_title);

    let div_tag = dom.new_element("div");
    let mut current_letter = String::new();
    let mut div_running_tag: Option<calibre_ebooks::dom::NodeId> = None;

    let title_sorts: Vec<String> = books_by_title.iter().map(|b| book_str(b, "title_sort").to_string()).collect();
    let sort_equivalents = establish_equivalencies(&title_sorts);

    for (idx, book) in books_by_title.iter().enumerate() {
        let letter_candidate = letter_or_symbol_str(&sort_equivalents[idx]);
        if letter_candidate != current_letter {
            if let Some(running) = div_running_tag.take() {
                dom.append_child(div_tag, running);
            }
            let running = dom.new_element("div");
            if !dom.children(div_tag).is_empty() {
                set_attr(&mut dom, running, "class", "initial_letter");
            }
            current_letter = letter_candidate;

            let p_index = dom.new_element("p");
            set_attr(&mut dom, p_index, "class", "author_title_letter_index");
            let a_tag = dom.new_element("a");
            if current_letter == SYMBOLS {
                set_attr(&mut dom, a_tag, "id", format!("{SYMBOLS}_titles"));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, SYMBOLS);
            } else {
                set_attr(&mut dom, a_tag, "id", format!("{}_titles", generate_unicode_name(&current_letter)));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, &sort_equivalents[idx]);
            }
            dom.append_child(running, p_index);
            div_running_tag = Some(running);
        }

        let p_book = dom.new_element("p");
        set_attr(&mut dom, p_book, "class", "line_item");
        insert_prefix(&mut dom, p_book, fmt, book.get("prefix").and_then(|v| v.as_str()));

        let span_tag = dom.new_element("span");
        set_attr(&mut dom, span_tag, "class", "entry");

        let title_a = dom.new_element("a");
        if generate_descriptions {
            let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            set_attr(&mut dom, title_a, "href", format!("book_{book_id}.html"));
        }
        let args = generate_format_args(book, rating_full_char, rating_empty_char);
        let has_series = book.get("series").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).is_some();
        let template = if has_series { BY_TITLES_SERIES_TITLE_TEMPLATE } else { BY_TITLES_NORMAL_TITLE_TEMPLATE };
        append_text(&mut dom, title_a, &safe_format(template, &args));
        dom.append_child(span_tag, title_a);

        append_text(&mut dom, span_tag, " \u{b7} ");

        let em_tag = dom.new_element("em");
        let author_a = dom.new_element("a");
        let author = book_str(book, "author");
        if generate_authors {
            set_attr(&mut dom, author_a, "href", format!("ByAlphaAuthor.html#{}", generate_author_anchor(author)));
        }
        append_text(&mut dom, author_a, author);
        dom.append_child(em_tag, author_a);
        dom.append_child(span_tag, em_tag);

        dom.append_child(p_book, span_tag);

        if let Some(running) = div_running_tag {
            dom.append_child(running, p_book);
        }
    }

    if let Some(running) = div_running_tag.take() {
        dom.append_child(div_tag, running);
    }
    dom.append_child(body, div_tag);

    serialize_html_document(&dom, root)
}

pub const BY_SERIES_TITLE_TEMPLATE: &str = "[{series_index}] {title} {pubyear_parens}";

/// Port of `generate_html_by_series`: renders `content/BySeries.html`'s
/// body, or `None` if no book in `books_to_catalog` has a series (matching
/// upstream's own early return -- upstream additionally sets
/// `self.opts.generate_series = False` as a side effect so later
/// generators skip series links entirely; since every generator in this
/// module already takes `generate_series` as an explicit caller-supplied
/// parameter rather than reading shared mutable state, propagating that
/// `None` into `false` for subsequent calls is the caller's job, not
/// this function's).
///
/// Recomputes each book's `prefix` via [`discover_prefix`] against the
/// book record *as already enriched by [`populate_title`]* (i.e. against
/// its `filter_excluded_genres`-filtered `tags`, not the original raw
/// tags [`populate_title`]'s own internal `discover_prefix` call used) --
/// matching upstream's real `book['prefix'] = self.discover_prefix(book)`
/// line here precisely, a real (if easy to miss) discrepancy from every
/// other generator, which just uses [`populate_title`]'s already-computed
/// `prefix` field unchanged.
pub fn generate_html_by_series(
    db: &Cache,
    books_to_catalog: &[Value],
    prefix_rules: &[PrefixRule],
    fmt: &str,
    generate_for_kindle_mobi: bool,
    generate_descriptions: bool,
    generate_authors: bool,
    rating_full_char: &str,
    rating_empty_char: &str,
) -> crate::catalogs::Result<Option<String>> {
    let friendly_name = "Series";

    let mut books_by_series: Vec<Value> =
        books_to_catalog.iter().filter(|b| !book_str(b, "series").is_empty()).cloned().collect();
    if books_by_series.is_empty() {
        return Ok(None);
    }
    books_by_series.sort_by(|a, b| kf_books_by_series_sorter(a).cmp(&kf_books_by_series_sorter(b)));

    let series_sorts: Vec<String> = books_by_series.iter().map(|b| generate_sort_title(book_str(b, "series"))).collect();
    let sort_equivalents = establish_equivalencies(&series_sorts);

    let (mut dom, root, body) = empty_html_document(friendly_name);
    let div_tag = dom.new_element("div");
    let mut current_letter = String::new();
    let mut current_series: Option<String> = None;

    for (idx, book) in books_by_series.iter_mut().enumerate() {
        let letter_candidate = letter_or_symbol_str(&sort_equivalents[idx]);
        if letter_candidate != current_letter {
            current_letter = letter_candidate;
            let p_index = dom.new_element("p");
            set_attr(&mut dom, p_index, "class", "series_letter_index");
            let a_tag = dom.new_element("a");
            if current_letter == SYMBOLS {
                set_attr(&mut dom, a_tag, "id", format!("{SYMBOLS}_series"));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, SYMBOLS);
            } else {
                set_attr(&mut dom, a_tag, "id", format!("{}_series", generate_unicode_name(&current_letter)));
                dom.append_child(p_index, a_tag);
                append_text(&mut dom, p_index, &sort_equivalents[idx]);
            }
            dom.append_child(div_tag, p_index);
        }

        let series = book_str(book, "series").to_string();
        if Some(&series) != current_series.as_ref() {
            current_series = Some(series.clone());
            let p_series = dom.new_element("p");
            set_attr(&mut dom, p_series, "class", if fmt == "mobi" { "series_mobi" } else { "series" });
            let a_tag = dom.new_element("a");
            set_attr(&mut dom, a_tag, "id", generate_series_anchor(&series));
            dom.append_child(p_series, a_tag);
            append_text(&mut dom, p_series, &series);
            dom.append_child(div_tag, p_series);
        }

        let recomputed_prefix = discover_prefix(db, book, prefix_rules)?;
        if let Value::Object(map) = book {
            map.insert("prefix".to_string(), recomputed_prefix.map(Value::String).unwrap_or(Value::Null));
        }

        let p_book = dom.new_element("p");
        set_attr(&mut dom, p_book, "class", "line_item");
        insert_prefix(&mut dom, p_book, fmt, book.get("prefix").and_then(|v| v.as_str()));

        let span_tag = dom.new_element("span");
        set_attr(&mut dom, span_tag, "class", "entry");

        let title_a = dom.new_element("a");
        if generate_descriptions {
            let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            set_attr(&mut dom, title_a, "href", format!("book_{book_id}.html"));
        }
        let args = generate_format_args(book, rating_full_char, rating_empty_char);
        append_text(&mut dom, title_a, &safe_format(BY_SERIES_TITLE_TEMPLATE, &args));
        dom.append_child(span_tag, title_a);

        append_text(&mut dom, span_tag, " \u{b7} ");

        let author = book_str(book, "author").to_string();
        let author_a = dom.new_element("a");
        if generate_authors {
            set_attr(&mut dom, author_a, "href", format!("ByAlphaAuthor.html#{}", generate_author_anchor(&author)));
        }
        append_text(&mut dom, author_a, &author);
        dom.append_child(span_tag, author_a);

        dom.append_child(p_book, span_tag);
        dom.append_child(div_tag, p_book);
    }

    let p_title = dom.new_element("p");
    set_attr(&mut dom, p_title, "class", "title");
    let a_section_start = dom.new_element("a");
    set_attr(&mut dom, a_section_start, "id", "section_start");
    dom.append_child(p_title, a_section_start);
    if !generate_for_kindle_mobi {
        let a_tag = dom.new_element("a");
        set_attr(&mut dom, a_tag, "id", friendly_name.to_lowercase().replace(' ', ""));
        dom.append_child(p_title, a_tag);
        append_text(&mut dom, p_title, friendly_name);
    }
    dom.append_child(body, p_title);
    dom.append_child(body, div_tag);

    Ok(Some(serialize_html_document(&dom, root)))
}

pub const BY_GENRES_NORMAL_TITLE_TEMPLATE: &str = "{title} {pubyear_parens}";
pub const BY_GENRES_SERIES_TITLE_TEMPLATE: &str = "{series_index}. {title} {pubyear_parens}";

/// A slim per-book summary for a genre page -- upstream's `this_book`
/// dict (`author`, `title`, `author_sort` (already
/// [`capitalize`]-d), `prefix`, `tags`, `id`, `series`, `series_index`,
/// `date`), deliberately narrower than a full [`populate_title`]
/// output. Notably has no `"rating"` key -- [`generate_format_args`]'s
/// `rating_parens` is empty for every genre-page book, matching upstream
/// exactly (its own `'rating' in book` check is false for this shape).
fn slim_genre_book(book: &Value) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("author".to_string(), Value::String(book_str(book, "author").to_string()));
    m.insert("title".to_string(), Value::String(book_str(book, "title").to_string()));
    m.insert("author_sort".to_string(), Value::String(capitalize(book_str(book, "author_sort"))));
    m.insert("prefix".to_string(), book.get("prefix").cloned().unwrap_or(Value::Null));
    m.insert("tags".to_string(), book.get("tags").cloned().unwrap_or(Value::Null));
    m.insert("id".to_string(), book.get("id").cloned().unwrap_or(Value::Null));
    m.insert("series".to_string(), book.get("series").cloned().unwrap_or(Value::Null));
    m.insert("series_index".to_string(), book.get("series_index").cloned().unwrap_or(Value::Null));
    m.insert("date".to_string(), book.get("date").cloned().unwrap_or(Value::Null));
    Value::Object(m)
}

/// Port of `generate_html_by_genre`: renders one genre's page. Returns
/// the finished HTML plus `titles_spanned` (the first, and if there's
/// more than one book the last, `(author, title)` pair -- matching
/// upstream's own return shape exactly, including that it's *not* a
/// simple 2-tuple when there's only one book).
pub fn generate_html_by_genre(
    genre: &str,
    section_head: bool,
    books: &[Value],
    friendly_genre_tag: &str,
    fmt: &str,
    generate_authors: bool,
    generate_series: bool,
    generate_descriptions: bool,
    rating_full_char: &str,
    rating_empty_char: &str,
) -> (String, Vec<(String, String)>) {
    let (mut dom, root, body) = empty_html_document(genre);

    let anchor_div = dom.new_element("div");
    if section_head {
        let a_tag = dom.new_element("a");
        set_attr(&mut dom, a_tag, "id", "section_start");
        dom.append_child(anchor_div, a_tag);
    }
    let a_genre = dom.new_element("a");
    set_attr(&mut dom, a_genre, "id", format!("Genre_{genre}"));
    dom.append_child(anchor_div, a_genre);
    dom.append_child(body, anchor_div);

    let p_title = dom.new_element("p");
    set_attr(&mut dom, p_title, "class", "title");
    append_text(&mut dom, p_title, friendly_genre_tag);
    dom.append_child(body, p_title);

    let authors_div = dom.new_element("div");
    set_attr(&mut dom, authors_div, "class", "authors");

    let mut current_author = String::new();
    let mut current_series: Option<String> = None;
    for book in books {
        let author = book_str(book, "author").to_string();
        if author != current_author {
            current_author = author.clone();
            current_series = None;
            let p_author = dom.new_element("p");
            set_attr(&mut dom, p_author, "class", "author_index");
            let a_tag = dom.new_element("a");
            if generate_authors {
                set_attr(&mut dom, a_tag, "href", format!("ByAlphaAuthor.html#{}", generate_author_anchor(&author)));
            }
            append_text(&mut dom, a_tag, &author);
            dom.append_child(p_author, a_tag);
            dom.append_child(authors_div, p_author);
        }

        match book.get("series").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            Some(series) if Some(series.to_string()) != current_series => {
                current_series = Some(series.to_string());
                let p_series = dom.new_element("p");
                set_attr(&mut dom, p_series, "class", if fmt == "mobi" { "series_mobi" } else { "series" });
                if generate_series {
                    let a_tag = dom.new_element("a");
                    set_attr(&mut dom, a_tag, "href", format!("BySeries.html#{}", generate_series_anchor(series)));
                    append_text(&mut dom, a_tag, series);
                    dom.append_child(p_series, a_tag);
                } else {
                    append_text(&mut dom, p_series, series);
                }
                dom.append_child(authors_div, p_series);
            }
            None => current_series = None,
            _ => {}
        }

        let p_book = dom.new_element("p");
        set_attr(&mut dom, p_book, "class", "line_item");
        insert_prefix(&mut dom, p_book, fmt, book.get("prefix").and_then(|v| v.as_str()));

        let span_tag = dom.new_element("span");
        set_attr(&mut dom, span_tag, "class", "entry");
        let title_a = dom.new_element("a");
        if generate_descriptions {
            let book_id = book.get("id").and_then(|v| v.as_i64()).unwrap_or_default();
            set_attr(&mut dom, title_a, "href", format!("book_{book_id}.html"));
        }
        let args = generate_format_args(book, rating_full_char, rating_empty_char);
        let template = if current_series.is_some() { BY_GENRES_SERIES_TITLE_TEMPLATE } else { BY_GENRES_NORMAL_TITLE_TEMPLATE };
        append_text(&mut dom, title_a, &safe_format(template, &args));
        dom.append_child(span_tag, title_a);
        dom.append_child(p_book, span_tag);

        dom.append_child(authors_div, p_book);
    }
    dom.append_child(body, authors_div);

    let titles_spanned = if books.len() > 1 {
        vec![
            (book_str(&books[0], "author").to_string(), book_str(&books[0], "title").to_string()),
            (book_str(&books[books.len() - 1], "author").to_string(), book_str(&books[books.len() - 1], "title").to_string()),
        ]
    } else {
        vec![(book_str(&books[0], "author").to_string(), book_str(&books[0], "title").to_string())]
    };

    (serialize_html_document(&dom, root), titles_spanned)
}

/// One genre's summary + rendered page -- upstream's `master_genre_list`
/// entry shape.
#[derive(Debug, Clone)]
pub struct GenrePage {
    /// The normalized tag (upstream's dict key -- what upstream simply
    /// calls `genre`).
    pub tag: String,
    pub file: String,
    pub authors: Vec<(String, String, usize)>,
    pub books: Vec<Value>,
    pub titles_spanned: Vec<(String, String)>,
    pub html: String,
}

/// Port of `generate_html_by_genres`. `genre_tags_dict` is
/// [`filter_genre_tags`]'s output (`friendly tag -> normalized tag`);
/// `books_by_author` must already carry each book's `"genres"` array
/// (matching [`populate_title`]'s own output).
///
/// The grouping loop is a plain `IndexMap`-based group-by (normalized tag
/// -> deduped-by-`(title, author)` book list, in first-seen order) rather
/// than upstream's literal list-of-single-key-dicts-with-nested-linear-
/// scans structure -- that structure has no observable effect on the
/// output beyond its own internal bookkeeping cost, so this port
/// reproduces its *result* (deduped books grouped under whichever
/// normalized tag they share, ordered by the first `friendly_tag` that
/// introduced each group) directly rather than the awkward container
/// shape that produces it.
///
/// **Fixes a real bug, not preserved**: upstream's per-genre
/// `unique_authors` tally only appends an entry when a *different*
/// author is encountered -- meaning the *last* author in every genre
/// with 2+ distinct authors is silently dropped from the list entirely
/// (there's no post-loop "flush the final author" step here, unlike
/// `fetch_books_by_author`'s otherwise-similar loop). Unlike that
/// function's narrow single-book-catalog edge case, this drops real data
/// in the ordinary multi-author case -- a visible defect, not a stable
/// quirk -- so this port reuses the same (already-fixed)
/// `group_consecutive_authors` helper `fetch_books_by_author` uses
/// instead of transliterating the buggy loop.
pub fn generate_html_by_genres(
    genre_tags_dict: &indexmap::IndexMap<String, String>,
    books_by_author: &[Value],
    fmt: &str,
    generate_authors: bool,
    generate_series: bool,
    generate_descriptions: bool,
    rating_full_char: &str,
    rating_empty_char: &str,
) -> Vec<GenrePage> {
    let mut friendly_tags: Vec<&String> = genre_tags_dict.keys().collect();
    friendly_tags.sort();

    let mut order: Vec<String> = Vec::new();
    let mut groups: indexmap::IndexMap<String, Vec<Value>> = indexmap::IndexMap::new();
    let mut seen: std::collections::HashMap<String, std::collections::HashSet<(String, String)>> = std::collections::HashMap::new();

    for friendly_tag in friendly_tags {
        let normalized = genre_tags_dict[friendly_tag].clone();
        for book in books_by_author {
            let has_tag = book
                .get("genres")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().any(|g| g.as_str() == Some(friendly_tag.as_str())))
                .unwrap_or(false);
            if !has_tag {
                continue;
            }

            let key = (book_str(book, "title").to_string(), book_str(book, "author").to_string());
            let seen_for_tag = seen.entry(normalized.clone()).or_default();
            if seen_for_tag.contains(&key) {
                continue;
            }
            seen_for_tag.insert(key);

            if !groups.contains_key(&normalized) {
                order.push(normalized.clone());
            }
            groups.entry(normalized.clone()).or_default().push(slim_genre_book(book));
        }
    }

    let mut pages = Vec::with_capacity(order.len());
    for (index, tag) in order.iter().enumerate() {
        let books_for_genre = &groups[tag];
        let authors: Vec<(String, String)> =
            books_for_genre.iter().map(|b| (book_str(b, "author").to_string(), book_str(b, "author_sort").to_string())).collect();
        let unique_authors = group_consecutive_authors(&authors);

        let friendly = get_friendly_genre_tag(genre_tags_dict, tag).unwrap_or(tag.as_str()).to_string();
        let (html, titles_spanned) = generate_html_by_genre(
            tag,
            index == 0,
            books_for_genre,
            &friendly,
            fmt,
            generate_authors,
            generate_series,
            generate_descriptions,
            rating_full_char,
            rating_empty_char,
        );

        pages.push(GenrePage {
            tag: tag.clone(),
            file: format!("content/Genre_{tag}.html"),
            authors: unique_authors,
            books: books_for_genre.clone(),
            titles_spanned,
            html,
        });
    }
    pages
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

    // --- detect_author_sort_mismatches / fetch_books_by_title / fetch_books_by_author ---

    fn author_book(author: &str, author_sort: &str, title: &str, title_sort: &str) -> Value {
        book(&[
            ("author", Value::String(author.to_string())),
            ("author_sort", Value::String(author_sort.to_string())),
            ("authors", Value::from(vec![author.to_string()])),
            ("title", Value::String(title.to_string())),
            ("title_sort", Value::String(title_sort.to_string())),
            ("series", Value::Null),
        ])
    }

    #[test]
    fn detect_author_sort_mismatches_warns_for_epub_but_errors_for_mobi() {
        let books = vec![
            author_book("Smith, John", "Smith, John", "A", "A"),
            author_book("Smith, John", "Smyth, John", "B", "B"),
        ];
        let warnings = detect_author_sort_mismatches(&books, "epub").unwrap();
        assert_eq!(warnings.len(), 1);

        let err = detect_author_sort_mismatches(&books, "mobi").unwrap_err();
        assert!(matches!(err, CatalogError::AuthorSortMismatch(_)));
    }

    #[test]
    fn detect_author_sort_mismatches_is_fine_with_consistent_sorts() {
        let books = vec![author_book("Alice", "Alice", "A", "A"), author_book("Bob", "Bob", "B", "B")];
        assert_eq!(detect_author_sort_mismatches(&books, "epub").unwrap(), Vec::<String>::new());
    }

    #[test]
    fn fetch_books_by_title_sorts_case_insensitively_by_title_sort() {
        let books = vec![author_book("A", "A", "zebra", "zebra"), author_book("B", "B", "Apple", "apple")];
        let sorted = fetch_books_by_title(&books).unwrap();
        assert_eq!(sorted[0]["title"], "Apple");
        assert_eq!(sorted[1]["title"], "zebra");
    }

    #[test]
    fn fetch_books_by_title_errors_on_an_empty_catalog() {
        let err = fetch_books_by_title(&[]).unwrap_err();
        assert!(matches!(err, CatalogError::EmptyCatalog));
    }

    #[test]
    fn fetch_books_by_author_groups_and_counts_unique_authors() {
        let books = vec![
            author_book("Alice", "Alice", "Book1", "Book1"),
            author_book("Alice", "Alice", "Book2", "Book2"),
            author_book("Bob", "Bob", "Book3", "Book3"),
        ];
        let result = fetch_books_by_author(&books, None, false, false, false, "epub").unwrap();
        let names: Vec<&str> = result.authors.iter().map(|(name, _, _)| name.as_str()).collect();
        assert_eq!(names, vec!["Alice", "Bob"]);
        let alice_count = result.authors.iter().find(|(n, _, _)| n == "Alice").unwrap().2;
        assert_eq!(alice_count, 2);
        let bob_count = result.authors.iter().find(|(n, _, _)| n == "Bob").unwrap().2;
        assert_eq!(bob_count, 1);
    }

    #[test]
    fn fetch_books_by_author_single_book_catalog_reports_count_one() {
        // Regression test for the upstream duplicate-zero-count bug this
        // port deliberately fixes (see fetch_books_by_author's own doc).
        let books = vec![author_book("Alice", "Alice", "Only Book", "Only Book")];
        let result = fetch_books_by_author(&books, None, false, false, false, "epub").unwrap();
        assert_eq!(result.authors, vec![("Alice".to_string(), "Alice".to_string(), 1)]);
    }

    #[test]
    fn fetch_books_by_author_splits_multi_author_strings_into_individual_authors() {
        let books = vec![author_book("Alice & Bob", "Alice & Bob", "Collab", "Collab")];
        let result = fetch_books_by_author(&books, None, false, false, false, "epub").unwrap();
        let mut individuals = result.individual_authors.clone();
        individuals.sort();
        assert_eq!(individuals, vec!["Alice".to_string(), "Bob".to_string()]);
    }

    #[test]
    fn fetch_books_by_author_populates_books_by_description_when_requested() {
        let books = vec![author_book("Alice", "Alice", "Book1", "Book1")];
        let result = fetch_books_by_author(&books, None, false, true, true, "epub").unwrap();
        assert!(result.books_by_description.is_some());
        assert_eq!(result.books_by_description.unwrap().len(), 1);
    }

    // --- generate_format_args ---

    fn format_args_book(fields: &[(&str, Value)]) -> Value {
        book(fields)
    }

    #[test]
    fn generate_format_args_strips_a_whole_number_series_index_suffix() {
        let b = format_args_book(&[("title", Value::from("T")), ("series", Value::Null), ("series_index", Value::from(3.0))]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.series_index, "3");
    }

    #[test]
    fn generate_format_args_keeps_a_fractional_series_index() {
        let b = format_args_book(&[("title", Value::from("T")), ("series", Value::Null), ("series_index", Value::from(3.5))]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.series_index, "3.5");
    }

    #[test]
    fn generate_format_args_extracts_the_pubyear_from_the_date_field() {
        let b = format_args_book(&[
            ("title", Value::from("T")),
            ("series", Value::Null),
            ("series_index", Value::from(0.0)),
            ("date", Value::from("June 2020")),
        ]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.pubyear, "2020");
        assert_eq!(args.pubyear_parens, "(2020)");
    }

    #[test]
    fn generate_format_args_empty_pubyear_when_date_is_absent() {
        let b = format_args_book(&[("title", Value::from("T")), ("series", Value::Null), ("series_index", Value::from(0.0))]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.pubyear, "");
        assert_eq!(args.pubyear_parens, "");
    }

    #[test]
    fn generate_format_args_rating_parens_present_even_for_a_zero_rating() {
        // Matches upstream's `'rating' in book` key-existence check --
        // populate_title always sets a rating key (even 0), so
        // rating_parens is always non-empty for a real populated book,
        // even though the rating glyph string itself is empty.
        let b = format_args_book(&[
            ("title", Value::from("T")),
            ("series", Value::Null),
            ("series_index", Value::from(0.0)),
            ("rating", Value::from(0.0)),
        ]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.rating, "");
        assert_eq!(args.rating_parens, "()");
    }

    #[test]
    fn generate_format_args_rating_parens_empty_when_no_rating_key_at_all() {
        let b = format_args_book(&[("title", Value::from("T")), ("series", Value::Null), ("series_index", Value::from(0.0))]);
        let args = generate_format_args(&b, "*", "-");
        assert_eq!(args.rating_parens, "");
    }

    // --- filter_genre_tags / establish_equivalencies ---

    #[test]
    fn normalize_tag_lowercases_and_strips_whitespace() {
        assert_eq!(normalize_tag("Science Fiction", 245), "sciencefiction");
    }

    #[test]
    fn normalize_tag_substitutes_a_unicode_name_for_symbols() {
        // Cross-checked against a live Python `unicodedata.name('-')`
        // call: "HYPHEN-MINUS" (which itself contains a hyphen -- not
        // further sanitized, matching upstream exactly).
        assert_eq!(normalize_tag("Sci-Fi", 245), "sciHYPHEN-MINUSfi");
    }

    #[test]
    fn filter_genre_tags_excludes_configured_and_pattern_matched_tags() {
        let (dir, cache) = open_test_cache();
        add_test_book(dir.path(), &cache, "T");
        {
            let conn = cache.backend.conn.lock().unwrap();
            for tag in ["Fiction", "[Project Gutenberg]", "Archived"] {
                conn.execute("INSERT INTO tags (name) VALUES (?1)", [tag]).unwrap();
            }
        }

        let excluded_tags = vec!["Archived".to_string()];
        let dict = filter_genre_tags(&cache, 245, &excluded_tags, r"\[.+\]|^\+$").unwrap();
        assert!(dict.contains_key("Fiction"));
        assert!(!dict.contains_key("Archived"));
        assert!(!dict.contains_key("[Project Gutenberg]"));
    }

    #[test]
    fn establish_equivalencies_uppercases_leading_characters() {
        let items = vec!["apple".to_string(), "Banana".to_string()];
        assert_eq!(establish_equivalencies(&items), vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn establish_equivalencies_applies_the_a_o_u_umlaut_exceptions() {
        let items = vec!["Äpple".to_string(), "Öl".to_string(), "Über".to_string()];
        assert_eq!(establish_equivalencies(&items), vec!["A".to_string(), "O".to_string(), "U".to_string()]);
    }

    // --- generate_html_by_author ---

    fn author_html_book(
        author: &str,
        author_sort: &str,
        title: &str,
        title_sort: &str,
        series: Option<&str>,
        prefix: Option<&str>,
    ) -> Value {
        book(&[
            ("id", Value::from(1)),
            ("author", Value::String(author.to_string())),
            ("author_sort", Value::String(author_sort.to_string())),
            ("title", Value::String(title.to_string())),
            ("title_sort", Value::String(title_sort.to_string())),
            ("series", series.map(Value::from).unwrap_or(Value::Null)),
            ("series_index", Value::from(1.0)),
            ("prefix", prefix.map(Value::from).unwrap_or(Value::Null)),
        ])
    }

    #[test]
    fn generate_html_by_author_produces_a_well_formed_document() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", None, None)];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        assert!(html.starts_with("<!DOCTYPE html"), "{html}");
        assert!(html.contains("<title>Authors</title>"), "{html}");
        assert!(html.contains("class=\"author_index\""), "{html}");
        assert!(html.contains(">Alice<"), "{html}");
        assert!(html.contains("class=\"line_item\""), "{html}");
        assert!(html.contains("Book One"), "{html}");
    }

    #[test]
    fn generate_html_by_author_second_author_gets_a_logical_group_div() {
        let books = vec![
            author_html_book("Alice", "Alice", "A Book", "A Book", None, None),
            author_html_book("Adam", "Adam", "B Book", "B Book", None, None),
        ];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        assert!(html.contains("class=\"author_logical_group\""), "{html}");
        // First letter section (the very first in the whole document)
        // never gets the initial_letter class.
        assert!(!html.contains("class=\"initial_letter\""), "{html}");
    }

    #[test]
    fn generate_html_by_author_new_letter_gets_initial_letter_class() {
        let books = vec![
            author_html_book("Alice", "Alice", "A Book", "A Book", None, None),
            author_html_book("Zeb", "Zeb", "Z Book", "Z Book", None, None),
        ];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        assert!(html.contains("class=\"initial_letter\""), "{html}");
        // Different letters means no shared logical-group wrapper.
        assert!(!html.contains("class=\"author_logical_group\""), "{html}");
    }

    #[test]
    fn generate_html_by_author_series_link_uses_the_series_anchor() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", Some("Foundation"), None)];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        assert!(html.contains("href=\"BySeries.html#foundation_series\""), "{html}");
        assert!(html.contains("class=\"series\""), "{html}");
    }

    #[test]
    fn generate_html_by_author_series_class_is_series_mobi_for_mobi_format() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", Some("Foundation"), None)];
        let html = generate_html_by_author(&books, "mobi", false, true, false, "*", "-");
        assert!(html.contains("class=\"series_mobi\""), "{html}");
    }

    #[test]
    fn generate_html_by_author_uses_the_series_title_template_when_in_a_series() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", Some("Foundation"), None)];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        // BY_AUTHORS_SERIES_TITLE_TEMPLATE is "[{series_index}] {title} ...".
        assert!(html.contains("[1] Book One"), "{html}");
    }

    #[test]
    fn generate_html_by_author_inserts_the_prefix_glyph() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", None, Some("\u{2713}"))];
        let html = generate_html_by_author(&books, "epub", false, true, false, "*", "-");
        assert!(html.contains("class=\"prefix\""), "{html}");
        assert!(html.contains('\u{2713}'), "{html}");
    }

    #[test]
    fn generate_html_by_author_prefix_uses_code_tag_for_mobi() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", None, Some("\u{2713}"))];
        let html = generate_html_by_author(&books, "mobi", false, true, false, "*", "-");
        assert!(html.contains("<code>"), "{html}");
    }

    #[test]
    fn generate_html_by_author_adds_a_book_link_when_generating_descriptions() {
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", None, None)];
        let html = generate_html_by_author(&books, "epub", false, true, true, "*", "-");
        assert!(html.contains("href=\"book_1.html\""), "{html}");
    }

    #[test]
    fn generate_html_by_author_omits_the_section_title_text_for_kindle_mobi() {
        // `contains(">Authors<")` alone would also match the ever-present
        // `<title>Authors</title>` document head, so check the
        // section-heading anchor's id specifically.
        let books = vec![author_html_book("Alice", "Alice", "Book One", "Book One", None, None)];
        let with_title = generate_html_by_author(&books, "mobi", false, true, false, "*", "-");
        let without_title = generate_html_by_author(&books, "mobi", true, true, false, "*", "-");
        assert!(with_title.contains("id=\"authors\""), "{with_title}");
        assert!(!without_title.contains("id=\"authors\""), "{without_title}");
    }

    // --- generate_html_by_title ---

    fn title_html_book(title: &str, title_sort: &str, author: &str, series: Option<&str>, prefix: Option<&str>) -> Value {
        book(&[
            ("id", Value::from(1)),
            ("title", Value::String(title.to_string())),
            ("title_sort", Value::String(title_sort.to_string())),
            ("author", Value::String(author.to_string())),
            ("series", series.map(Value::from).unwrap_or(Value::Null)),
            ("series_index", Value::from(2.0)),
            ("prefix", prefix.map(Value::from).unwrap_or(Value::Null)),
        ])
    }

    #[test]
    fn generate_html_by_title_produces_a_well_formed_document() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", None, None)];
        let html = generate_html_by_title(&books, "epub", false, false, true, "*", "-");
        assert!(html.starts_with("<!DOCTYPE html"), "{html}");
        assert!(html.contains("<title>Books By Alpha Title</title>"), "{html}");
        assert!(html.contains("id=\"bytitle\""), "{html}");
        assert!(html.contains("class=\"line_item\""), "{html}");
        assert!(html.contains("Book One"), "{html}");
        assert!(html.contains(" \u{b7} "), "{html}");
        assert!(html.contains(">Alice<"), "{html}");
    }

    #[test]
    fn generate_html_by_title_uses_the_series_template_when_series_is_set() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", Some("Foundation"), None)];
        let html = generate_html_by_title(&books, "epub", false, false, true, "*", "-");
        // BY_TITLES_SERIES_TITLE_TEMPLATE is "{title} ({series} [{series_index}])".
        assert!(html.contains("Book One (Foundation [2])"), "{html}");
    }

    #[test]
    fn generate_html_by_title_links_the_author_only_when_generate_authors_is_set() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", None, None)];
        let with_link = generate_html_by_title(&books, "epub", false, false, true, "*", "-");
        let without_link = generate_html_by_title(&books, "epub", false, false, false, "*", "-");
        assert!(with_link.contains("href=\"ByAlphaAuthor.html#Alice\""), "{with_link}");
        assert!(!without_link.contains("href=\"ByAlphaAuthor.html"), "{without_link}");
    }

    #[test]
    fn generate_html_by_title_new_letter_gets_initial_letter_class_after_the_first() {
        let books =
            vec![title_html_book("Apple", "Apple", "A", None, None), title_html_book("Zebra", "Zebra", "B", None, None)];
        let html = generate_html_by_title(&books, "epub", false, false, true, "*", "-");
        assert!(html.contains("class=\"initial_letter\""), "{html}");
    }

    #[test]
    fn generate_html_by_title_omits_the_section_title_text_for_kindle_mobi() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", None, None)];
        let with_title = generate_html_by_title(&books, "epub", false, false, true, "*", "-");
        let without_title = generate_html_by_title(&books, "epub", true, false, true, "*", "-");
        assert!(with_title.contains("id=\"bytitle\""), "{with_title}");
        assert!(!without_title.contains("id=\"bytitle\""), "{without_title}");
    }

    #[test]
    fn generate_html_by_title_prefix_uses_code_tag_for_mobi() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", None, Some("\u{2713}"))];
        let html = generate_html_by_title(&books, "mobi", false, false, true, "*", "-");
        assert!(html.contains("<code>"), "{html}");
    }

    #[test]
    fn generate_html_by_title_adds_a_book_link_when_generating_descriptions() {
        let books = vec![title_html_book("Book One", "Book One", "Alice", None, None)];
        let html = generate_html_by_title(&books, "epub", false, true, true, "*", "-");
        assert!(html.contains("href=\"book_1.html\""), "{html}");
    }

    // --- generate_html_by_series ---

    fn series_html_book(id: i32, title: &str, author: &str, series: &str, series_index: f64, tags: &[&str]) -> Value {
        book(&[
            ("id", Value::from(id)),
            ("title", Value::String(title.to_string())),
            ("title_sort", Value::String(title.to_string())),
            ("author", Value::String(author.to_string())),
            ("authors", Value::from(vec![author.to_string()])),
            ("series", Value::String(series.to_string())),
            ("series_index", Value::from(series_index)),
            ("tags", Value::from(tags.iter().map(|t| t.to_string()).collect::<Vec<_>>())),
        ])
    }

    #[test]
    fn generate_html_by_series_none_when_no_book_has_a_series() {
        let (_dir, cache) = open_test_cache();
        let books = vec![title_html_book("T", "T", "A", None, None)];
        let result = generate_html_by_series(&cache, &books, &[], "epub", false, false, true, "*", "-").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn generate_html_by_series_produces_a_well_formed_document() {
        let (_dir, cache) = open_test_cache();
        let books = vec![series_html_book(1, "Book One", "Alice", "Foundation", 1.0, &[])];
        let html = generate_html_by_series(&cache, &books, &[], "epub", false, false, true, "*", "-").unwrap().unwrap();
        assert!(html.starts_with("<!DOCTYPE html"), "{html}");
        assert!(html.contains("<title>Series</title>"), "{html}");
        assert!(html.contains("class=\"series\""), "{html}");
        assert!(html.contains("id=\"foundation_series\""), "{html}");
        assert!(html.contains("[1] Book One"), "{html}");
    }

    #[test]
    fn generate_html_by_series_groups_multiple_books_under_one_series_header() {
        let (_dir, cache) = open_test_cache();
        let books = vec![
            series_html_book(1, "Book One", "Alice", "Foundation", 1.0, &[]),
            series_html_book(2, "Book Two", "Alice", "Foundation", 2.0, &[]),
        ];
        let html = generate_html_by_series(&cache, &books, &[], "epub", false, false, true, "*", "-").unwrap().unwrap();
        assert_eq!(html.matches("id=\"foundation_series\"").count(), 1, "{html}");
        assert!(html.contains("[1] Book One"), "{html}");
        assert!(html.contains("[2] Book Two"), "{html}");
    }

    #[test]
    fn generate_html_by_series_class_is_series_mobi_for_mobi_format() {
        let (_dir, cache) = open_test_cache();
        let books = vec![series_html_book(1, "Book One", "Alice", "Foundation", 1.0, &[])];
        let html = generate_html_by_series(&cache, &books, &[], "mobi", false, false, true, "*", "-").unwrap().unwrap();
        assert!(html.contains("class=\"series_mobi\""), "{html}");
    }

    #[test]
    fn generate_html_by_series_recomputes_the_prefix_from_the_populated_tags() {
        let (_dir, cache) = open_test_cache();
        let books = vec![series_html_book(1, "Book One", "Alice", "Foundation", 1.0, &["wishlist"])];
        let rules = get_prefix_rules(&[("W".to_string(), "tags".to_string(), "wishlist".to_string(), "\u{d7}".to_string())]);
        let html = generate_html_by_series(&cache, &books, &rules, "epub", false, false, true, "*", "-").unwrap().unwrap();
        assert!(html.contains('\u{d7}'), "{html}");
    }

    #[test]
    fn generate_html_by_series_omits_the_section_title_text_for_kindle_mobi() {
        let (_dir, cache) = open_test_cache();
        let books = vec![series_html_book(1, "Book One", "Alice", "Foundation", 1.0, &[])];
        let with_title = generate_html_by_series(&cache, &books, &[], "epub", false, false, true, "*", "-").unwrap().unwrap();
        let without_title = generate_html_by_series(&cache, &books, &[], "epub", true, false, true, "*", "-").unwrap().unwrap();
        assert!(with_title.contains("id=\"series\""), "{with_title}");
        assert!(!without_title.contains("id=\"series\""), "{without_title}");
    }

    // --- generate_html_by_genre / generate_html_by_genres ---

    fn genre_book(id: i32, title: &str, author: &str, author_sort: &str, genres: &[&str]) -> Value {
        book(&[
            ("id", Value::from(id)),
            ("title", Value::String(title.to_string())),
            ("author", Value::String(author.to_string())),
            ("author_sort", Value::String(author_sort.to_string())),
            ("genres", Value::from(genres.iter().map(|g| g.to_string()).collect::<Vec<_>>())),
            ("tags", Value::from(genres.iter().map(|g| g.to_string()).collect::<Vec<_>>())),
            ("series", Value::Null),
            ("series_index", Value::from(0.0)),
            ("date", Value::Null),
            ("prefix", Value::Null),
        ])
    }

    #[test]
    fn generate_html_by_genre_produces_a_well_formed_page() {
        let books = vec![slim_genre_book(&genre_book(1, "Book One", "Alice", "Alice", &["Fiction"]))];
        let (html, spanned) = generate_html_by_genre("fiction", true, &books, "Fiction", "epub", true, true, false, "*", "-");
        assert!(html.starts_with("<!DOCTYPE html"), "{html}");
        assert!(html.contains("id=\"section_start\""), "{html}");
        assert!(html.contains("id=\"Genre_fiction\""), "{html}");
        assert!(html.contains("class=\"authors\""), "{html}");
        assert!(html.contains(">Fiction<"), "{html}");
        assert_eq!(spanned, vec![("Alice".to_string(), "Book One".to_string())]);
    }

    #[test]
    fn generate_html_by_genre_titles_spanned_covers_first_and_last_when_multiple_books() {
        let books = vec![
            slim_genre_book(&genre_book(1, "Book A", "Alice", "Alice", &["Fiction"])),
            slim_genre_book(&genre_book(2, "Book Z", "Zeb", "Zeb", &["Fiction"])),
        ];
        let (_html, spanned) = generate_html_by_genre("fiction", false, &books, "Fiction", "epub", true, true, false, "*", "-");
        assert_eq!(spanned, vec![("Alice".to_string(), "Book A".to_string()), ("Zeb".to_string(), "Book Z".to_string())]);
    }

    #[test]
    fn generate_html_by_genres_groups_books_by_normalized_tag() {
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Fiction".to_string(), "fiction".to_string());
        let books = vec![
            genre_book(1, "Book A", "Alice", "Alice", &["Fiction"]),
            genre_book(2, "Book B", "Bob", "Bob", &["Fiction"]),
            genre_book(3, "Book C", "Carol", "Carol", &["Nonfiction"]),
        ];
        let pages = generate_html_by_genres(&dict, &books, "epub", true, true, false, "*", "-");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].tag, "fiction");
        assert_eq!(pages[0].books.len(), 2);
    }

    #[test]
    fn generate_html_by_genres_dedups_synonymous_friendly_tags() {
        // Two friendly tags mapping to the same normalized form should
        // not double-count a book that carries both.
        let mut dict = indexmap::IndexMap::new();
        dict.insert("SciFi".to_string(), "scifi".to_string());
        dict.insert("Sci-Fi".to_string(), "scifi".to_string());
        let books = vec![genre_book(1, "Book A", "Alice", "Alice", &["SciFi", "Sci-Fi"])];
        let pages = generate_html_by_genres(&dict, &books, "epub", true, true, false, "*", "-");
        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].books.len(), 1);
    }

    #[test]
    fn generate_html_by_genres_unique_authors_includes_the_last_author() {
        // Regression test for the upstream bug this port deliberately
        // fixes: the last distinct author in a genre must NOT be dropped.
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Fiction".to_string(), "fiction".to_string());
        let books = vec![
            genre_book(1, "Book A", "Alice", "Alice", &["Fiction"]),
            genre_book(2, "Book B", "Bob", "Bob", &["Fiction"]),
        ];
        let pages = generate_html_by_genres(&dict, &books, "epub", true, true, false, "*", "-");
        let names: Vec<&str> = pages[0].authors.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(names, vec!["Alice", "Bob"]);
    }

    #[test]
    fn generate_html_by_genres_uses_the_friendly_tag_as_the_page_title() {
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Sci-Fi".to_string(), "scifi".to_string());
        let books = vec![genre_book(1, "Book A", "Alice", "Alice", &["Sci-Fi"])];
        let pages = generate_html_by_genres(&dict, &books, "epub", true, true, false, "*", "-");
        assert!(pages[0].html.contains(">Sci-Fi<"), "{}", pages[0].html);
    }

    #[test]
    fn generate_html_by_genres_only_the_first_page_gets_section_start() {
        let mut dict = indexmap::IndexMap::new();
        dict.insert("Fiction".to_string(), "fiction".to_string());
        dict.insert("Nonfiction".to_string(), "nonfiction".to_string());
        let books = vec![
            genre_book(1, "Book A", "Alice", "Alice", &["Fiction"]),
            genre_book(2, "Book B", "Bob", "Bob", &["Nonfiction"]),
        ];
        let pages = generate_html_by_genres(&dict, &books, "epub", true, true, false, "*", "-");
        assert_eq!(pages.len(), 2);
        assert!(pages[0].html.contains("id=\"section_start\""), "{}", pages[0].html);
        assert!(!pages[1].html.contains("id=\"section_start\""), "{}", pages[1].html);
    }
}
