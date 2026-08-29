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
//!   `fetch_books_to_catalog` (not yet ported -- see below) applies while
//!   building its output. Ported ahead of `fetch_books_to_catalog` itself
//!   since it needs substantially more new infrastructure
//!   (`comments_to_html`, an HTML-paragraph-extraction step) this narrower
//!   slice doesn't.
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

use calibre_utils::icu::capitalize;
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
}
