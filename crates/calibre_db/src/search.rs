//! Port of `old_src/src/calibre/db/search.py` (issue #201 follow-up).
//!
//! # Scope of this pass
//!
//! Upstream's `search.py` is calibre's full search-query evaluator:
//! `DateSearch`, `NumericSearch`, `BooleanSearch`, `KeyPairSearch` (the
//! four typed matchers), `Parser` (a `SearchQueryParser` subclass that
//! dispatches a parsed query tree to the right matcher per field,
//! driven by `field_metadata`), `SavedSearchQueries`, an `LRUCache`,
//! and the top-level `Search` class that adds query caching and
//! virtual-library-aware restriction handling.
//!
//! This crate has no `field_metadata` system (`Cache::field_for`
//! covers a fixed, hardcoded set of standard fields -- see #204's
//! `cache.rs` docs), no custom columns, no virtual libraries, no user
//! categories, no saved searches, and no search-template/formatter
//! support. Porting `Parser.get_matches`'s real field-metadata-driven
//! dispatch verbatim is therefore not possible without first building
//! that whole system (a separate, large follow-up). What this pass
//! ports for real, faithfully transcribed and tested against the
//! Python source:
//!
//! - [`_matchkind`]/[`text_match`]: the `CONTAINS`/`EQUALS`/`REGEXP`/
//!   `ACCENT` match-kind parsing and text matching logic (`_matchkind`
//!   and `_match` in `search.py`), including the `..`-prefixed
//!   "internal match" and `.`-prefixed dotted-prefix EQUALS forms.
//! - [`DateSearch`]: the `>`/`<`/`>=`/`<=`/`=`/`!=` date-comparison
//!   operators, `today`/`yesterday`/`thismonth`/`NdaysAgo` relative
//!   dates, and year/month/day granularity-aware comparison --
//!   English-only (no gettext locale forms; upstream's own English
//!   defaults are what's ported).
//! - [`NumericSearch`]: numeric operators, `k`/`m`/`g` multiplier
//!   suffixes, `true`/`false` presence queries, and the ratings
//!   `//2` storage-to-stars adjustment.
//! - [`BooleanSearch`]: the full `yes`/`no`/`checked`/`unchecked`/
//!   `empty`/`blank`/tristate matrix -- English-only, same reason.
//! - [`KeyPairSearch`]: `key:value` matching for colon-separated
//!   fields (`identifiers`/`isbn`).
//! - A real `And`/`Or`/`Not`/`Token` tree evaluator ([`evaluate`])
//!   matching upstream's `SearchQueryParser.evaluate_and/or/not`
//!   *exactly*, including its candidate-narrowing semantics (AND's
//!   RHS only searches LHS's matches; OR's RHS only searches what LHS
//!   didn't match) -- this is real optimization behavior, not just a
//!   naive tree walk, and matters for correctness with `NOT` inside
//!   `AND`/`OR`.
//! - A fixed location-alias table ([`resolve_location`]) covering
//!   every field `Cache::field_for` knows about (see its match arms),
//!   standing in for the real `field_metadata.search_term_to_field_key`
//!   lookup.
//!
//! # Disclosed simplifications
//!
//! - **`ACCENT_MATCH`/primary-collation `CONTAINS_MATCH`**: upstream's
//!   `primary_contains`/`primary_no_punc_contains` are real ICU
//!   primary-collation-strength substring searches (accent- and
//!   case-insensitive, and additionally punctuation-insensitive for
//!   the `no_punc` variant) implemented in C. This crate has no ICU
//!   binding (`calibre_utils::icu` is a plain-Rust stub -- see its own
//!   doc comment), so both are approximated here as NFD-decompose +
//!   strip combining marks + lowercase (+ strip non-alphanumeric for
//!   the `no_punc` variant) then plain substring containment. Correct
//!   direction for common Latin-script text, not a byte-exact match
//!   for what real ICU collation would accept.
//! - **`"all"` location**: upstream's `all` sweeps every field with
//!   `search_terms` set in `field_metadata`, type-dispatching per
//!   field along the way (numbers/ratings/dates get their own
//!   fast-path even under `all`). Without `field_metadata` this pass
//!   hardcodes [`ALL_TEXT_LOCATIONS`] (title/authors/tags/series/
//!   publisher/comments/languages/formats) and always does a plain
//!   text match -- `identifiers`/dates/numbers are not swept by a bare
//!   `all` query, only by their own explicit location prefix.
//! - **Multi-value fields** (`authors`/`tags`/`languages`/`formats`):
//!   `Cache::field_for` already joins these into one string (#204's
//!   own disclosed simplification). This pass splits them back on the
//!   known join separator before matching, to get real per-item
//!   `CONTAINS`/`EQUALS` semantics back -- but an individual item
//!   containing the separator itself (e.g. a tag literally named
//!   `"a, b"`) would still split wrong, same root cause as #204.
//! - **Not ported at all**: `template:` search (no formatter/template
//!   engine in this crate), `@usercategory` search, `vl:` virtual
//!   library search, `search:savedname` saved-search references
//!   (`SavedSearchQueries`), the `#=N` is_multiple count operator, the
//!   grouped-search-terms (`@location`) expansion, and the top-level
//!   `Search` class's query result cache (`LRUCache`) -- every call to
//!   [`search`] re-evaluates the query tree from scratch. Each is its
//!   own follow-up.
//!
//! None of these change what a *supported* query means; they narrow
//! which queries are supported at all, same as every other disclosed
//! gap in this crate.
//!
//! Every field access here (`fetch_grouped`/`fetch_identifiers`, and
//! every matcher indirectly through them) already goes through
//! [`Cache::field_for`] rather than running its own SQL -- so issue
//! #222's cutover of `field_for` onto an in-memory model applies here
//! automatically, with no changes needed in this file.

use crate::cache::Cache;
use calibre_utils::date::parse_date;
use calibre_utils::icu::lower as icu_lower;
use calibre_utils::search_query_parser::{Parser as QueryParser, SearchNode};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("could not parse search query: {0}")]
    Parse(String),
    #[error("invalid regular expression {0:?}: {1}")]
    InvalidRegex(String, String),
    #[error("non-numeric value in query: {0:?}")]
    NonNumericQuery(String),
    #[error("non-numeric value in column {0}: {1:?}")]
    NonNumericValue(String, String),
    #[error("invalid boolean query {0:?}")]
    InvalidBoolean(String),
    #[error("date conversion error: {0:?}")]
    InvalidDate(String),
    #[error("number conversion error: {0:?}")]
    NumberConversion(String),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    #[error("unknown saved search: {0:?}")]
    UnknownSavedSearch(String),
    #[error("recursive saved search: {0:?}")]
    RecursiveSavedSearch(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

// --- _matchkind / _match {{{

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    Contains,
    Equals,
    Regexp,
    Accent,
}

/// Port of `_matchkind`.
fn matchkind(query: &str, case_sensitive: bool) -> (MatchKind, String) {
    let mut kind = MatchKind::Contains;
    let mut q = query.to_string();
    if query.chars().count() > 1 {
        if let Some(rest) = query.strip_prefix('\\') {
            q = rest.to_string();
        } else if let Some(rest) = query.strip_prefix('=') {
            kind = MatchKind::Equals;
            q = rest.to_string();
        } else if let Some(rest) = query.strip_prefix('~') {
            kind = MatchKind::Regexp;
            q = rest.to_string();
        } else if let Some(rest) = query.strip_prefix('^') {
            kind = MatchKind::Accent;
            q = rest.to_string();
        }
    }
    if !case_sensitive && kind != MatchKind::Regexp {
        q = icu_lower(&q);
    }
    (kind, q)
}

fn is_combining_mark(c: char) -> bool {
    matches!(c as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Approximation of ICU primary-strength collation folding: strip
/// accents (NFD decompose + drop combining marks) then lowercase. See
/// the module docs' "Disclosed simplifications" section.
fn primary_form(s: &str) -> String {
    icu_lower(
        &s.nfd()
            .filter(|c| !is_combining_mark(*c))
            .collect::<String>(),
    )
}

fn primary_contains(query: &str, text: &str) -> bool {
    primary_form(text).contains(&primary_form(query))
}

fn primary_no_punc_contains(query: &str, text: &str) -> bool {
    let strip_punc = |s: &str| -> String {
        primary_form(s)
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect()
    };
    strip_punc(text).contains(&strip_punc(query))
}

/// Port of `_match`: does `query` match any of `values` under `kind`?
fn text_match(
    query: &str,
    values: &[String],
    kind: MatchKind,
    use_primary_find_in_search: bool,
    case_sensitive: bool,
) -> Result<bool, SearchError> {
    let chars: Vec<char> = query.chars().collect();
    let internal_match_ok = chars.len() >= 2 && chars[0] == '.' && chars[1] == '.';
    // Mirrors `query = query[1:]; sq = query[1:]` (each `[1:]` drops
    // one more leading char from the *already-shortened* string).
    let (query, sq) = if internal_match_ok {
        let q: String = chars[1..].iter().collect();
        let sq: String = chars[2..].iter().collect();
        (q, sq)
    } else {
        (query.to_string(), String::new())
    };

    for raw in values {
        let t = if case_sensitive {
            raw.clone()
        } else {
            icu_lower(raw)
        };
        let hit = match kind {
            MatchKind::Equals => {
                if internal_match_ok {
                    if query == t {
                        true
                    } else {
                        t.split('.')
                            .map(|c| c.trim())
                            .filter(|c| !c.is_empty())
                            .any(|c| c == sq)
                    }
                } else if query.starts_with('.') {
                    let rest = &query[1..];
                    if t.starts_with(rest) {
                        let ql = query.chars().count() - 1;
                        let tlen = t.chars().count();
                        tlen == ql || t.chars().nth(ql) == Some('.')
                    } else {
                        false
                    }
                } else {
                    query == t
                }
            }
            MatchKind::Regexp => {
                let pattern = if case_sensitive {
                    query.clone()
                } else {
                    format!("(?i){query}")
                };
                let re = Regex::new(&pattern)
                    .map_err(|e| SearchError::InvalidRegex(query.clone(), e.to_string()))?;
                re.is_match(&t)
            }
            MatchKind::Accent => primary_contains(&query, &t),
            MatchKind::Contains => {
                if !case_sensitive && use_primary_find_in_search {
                    primary_no_punc_contains(&query, &t)
                } else {
                    t.contains(query.as_str())
                }
            }
        };
        if hit {
            return Ok(true);
        }
    }
    Ok(false)
}
// }}}

// --- DateSearch {{{

#[derive(Clone, Copy)]
enum DateRelOp {
    Ne,
    Ge,
    Le,
    Eq,
    Gt,
    Lt,
}

const UNDEFINED_DATE_YEAR: i32 = 101;

fn undefined_date() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(UNDEFINED_DATE_YEAR, 1, 1, 0, 0, 0)
        .unwrap()
}

fn dt_as_local(dt: DateTime<Utc>) -> DateTime<Local> {
    dt.with_timezone(&Local)
}

/// Port of `DateSearch`. English-only relative-date forms (`today`,
/// `yesterday`, `thismonth`, `Ndaysago`) -- see module docs.
pub struct DateSearch;

impl DateSearch {
    fn eq(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        if dbdate.year() != query.year() {
            return false;
        }
        if field_count == 1 {
            return true;
        }
        if dbdate.month() != query.month() {
            return false;
        }
        if field_count == 2 {
            return true;
        }
        dbdate.day() == query.day()
    }

    fn ne(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        !Self::eq(dbdate, query, field_count)
    }

    fn gt(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        if dbdate.year() > query.year() {
            return true;
        }
        if field_count > 1 && dbdate.year() == query.year() {
            if dbdate.month() > query.month() {
                return true;
            }
            return field_count == 3
                && dbdate.month() == query.month()
                && dbdate.day() > query.day();
        }
        false
    }

    fn le(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        !Self::gt(dbdate, query, field_count)
    }

    fn lt(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        if dbdate.year() < query.year() {
            return true;
        }
        if field_count > 1 && dbdate.year() == query.year() {
            if dbdate.month() < query.month() {
                return true;
            }
            return field_count == 3
                && dbdate.month() == query.month()
                && dbdate.day() < query.day();
        }
        false
    }

    fn ge(dbdate: DateTime<Local>, query: DateTime<Local>, field_count: u8) -> bool {
        !Self::lt(dbdate, query, field_count)
    }

    fn apply(
        op: DateRelOp,
        dbdate: DateTime<Local>,
        query: DateTime<Local>,
        field_count: u8,
    ) -> bool {
        match op {
            DateRelOp::Eq => Self::eq(dbdate, query, field_count),
            DateRelOp::Ne => Self::ne(dbdate, query, field_count),
            DateRelOp::Ge => Self::ge(dbdate, query, field_count),
            DateRelOp::Le => Self::le(dbdate, query, field_count),
            DateRelOp::Gt => Self::gt(dbdate, query, field_count),
            DateRelOp::Lt => Self::lt(dbdate, query, field_count),
        }
    }

    /// `query` must already be lowercased by the caller (matches
    /// `Parser.get_matches`'s `icu_lower(query)` call site).
    pub fn call(
        query: &str,
        field_iter: &[(Option<String>, HashSet<i32>)],
    ) -> Result<HashSet<i32>, SearchError> {
        let mut matches = HashSet::new();
        if query.chars().count() < 2 {
            return Ok(matches);
        }

        if query == "false" {
            for (v, book_ids) in field_iter {
                let d = v.as_deref().and_then(|s| parse_date(s, true));
                if d.is_none() || d.unwrap() <= undefined_date() {
                    matches.extend(book_ids.iter().copied());
                }
            }
            return Ok(matches);
        }
        if query == "true" {
            for (v, book_ids) in field_iter {
                let d = v.as_deref().and_then(|s| parse_date(s, true));
                if let Some(d) = d {
                    if d > undefined_date() {
                        matches.extend(book_ids.iter().copied());
                    }
                }
            }
            return Ok(matches);
        }

        let mut q = query;
        let mut op = DateRelOp::Eq;
        for (sym, o) in [
            ("!=", DateRelOp::Ne),
            (">=", DateRelOp::Ge),
            ("<=", DateRelOp::Le),
            ("=", DateRelOp::Eq),
            (">", DateRelOp::Gt),
            ("<", DateRelOp::Lt),
        ] {
            if let Some(rest) = q.strip_prefix(sym) {
                q = rest;
                op = o;
                break;
            }
        }

        let (qd, field_count): (DateTime<Local>, u8) = if q == "_today" || q == "today" {
            (dt_as_local(Utc::now()), 3)
        } else if q == "_yesterday" || q == "yesterday" {
            (dt_as_local(Utc::now() - Duration::days(1)), 3)
        } else if q == "_thismonth" || q == "thismonth" {
            (dt_as_local(Utc::now()), 2)
        } else if let Some(num) = q
            .strip_suffix("daysago")
            .or_else(|| q.strip_suffix("_daysago"))
        {
            let n: i64 = num
                .parse()
                .map_err(|_| SearchError::NumberConversion(num.to_string()))?;
            (dt_as_local(Utc::now() - Duration::days(n)), 3)
        } else {
            let parsed =
                parse_date(q, false).ok_or_else(|| SearchError::InvalidDate(q.to_string()))?;
            let fc = if q.contains('-') {
                q.matches('-').count() + 1
            } else {
                q.matches('/').count() + 1
            };
            (dt_as_local(parsed), fc.min(3) as u8)
        };

        for (v, book_ids) in field_iter {
            let d = v.as_deref().and_then(|s| parse_date(s, true));
            if let Some(d) = d {
                if Self::apply(op, dt_as_local(d), qd, field_count) {
                    matches.extend(book_ids.iter().copied());
                }
            }
        }
        Ok(matches)
    }
}
// }}}

// --- NumericSearch {{{

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NumDatatype {
    Int,
    Float,
    Rating,
}

#[derive(Clone, Copy)]
enum NumRelOp {
    Eq,
    Ne,
    Ge,
    Le,
    Gt,
    Lt,
}

fn apply_num_relop(op: NumRelOp, v: f64, q: f64) -> bool {
    match op {
        NumRelOp::Eq => v == q,
        NumRelOp::Ne => v != q,
        NumRelOp::Ge => v >= q,
        NumRelOp::Le => v <= q,
        NumRelOp::Gt => v > q,
        NumRelOp::Lt => v < q,
    }
}

enum NumMode {
    True,
    False,
    Op(NumRelOp, f64),
}

/// Port of `NumericSearch`. `query` must already be lowercased.
pub struct NumericSearch;

impl NumericSearch {
    pub fn call(
        query: &str,
        field_iter: &[(Option<String>, HashSet<i32>)],
        location: &str,
        dt: NumDatatype,
        candidates: &HashSet<i32>,
        is_many: bool,
    ) -> Result<HashSet<i32>, SearchError> {
        let mut matches = HashSet::new();
        if query.is_empty() {
            return Ok(matches);
        }

        if is_many && (query == "true" || query == "false") {
            let mut found = HashSet::new();
            for (val, ids) in field_iter {
                let ok = if dt == NumDatatype::Rating {
                    val.as_deref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|n| n > 0.0)
                        .unwrap_or(false)
                } else {
                    true
                };
                if ok {
                    found.extend(ids.iter().copied());
                }
            }
            return Ok(if query == "true" {
                found
            } else {
                candidates.difference(&found).copied().collect()
            });
        }

        let mode = if query == "false" {
            NumMode::False
        } else if query == "true" {
            NumMode::True
        } else {
            let mut q = query;
            let mut op = NumRelOp::Eq;
            for (sym, o) in [
                ("!=", NumRelOp::Ne),
                (">=", NumRelOp::Ge),
                ("<=", NumRelOp::Le),
                ("=", NumRelOp::Eq),
                (">", NumRelOp::Gt),
                ("<", NumRelOp::Lt),
            ] {
                if let Some(rest) = q.strip_prefix(sym) {
                    q = rest;
                    op = o;
                    break;
                }
            }

            let mut qs = q.to_string();
            let mut mult = 1.0f64;
            if qs.chars().count() > 1 {
                if let Some(last) = qs.chars().last() {
                    let m = match last.to_ascii_lowercase() {
                        'k' => Some(1024f64),
                        'm' => Some(1024f64.powi(2)),
                        'g' => Some(1024f64.powi(3)),
                        _ => None,
                    };
                    if let Some(m) = m {
                        mult = m;
                        qs.pop();
                    }
                }
            }

            let base: f64 = match dt {
                NumDatatype::Float => qs
                    .parse()
                    .map_err(|_| SearchError::NonNumericQuery(query.to_string()))?,
                NumDatatype::Int | NumDatatype::Rating => qs
                    .parse::<i64>()
                    .map(|n| n as f64)
                    .map_err(|_| SearchError::NonNumericQuery(query.to_string()))?,
            };
            NumMode::Op(op, base * mult)
        };

        let qfalse = matches!(mode, NumMode::False);

        for (val, ids) in field_iter {
            let raw = match val {
                None => {
                    if qfalse {
                        matches.extend(ids.iter().copied());
                    }
                    continue;
                }
                Some(r) => r,
            };
            let v: f64 = match dt {
                NumDatatype::Float => raw
                    .parse()
                    .map_err(|_| SearchError::NonNumericValue(location.to_string(), raw.clone()))?,
                NumDatatype::Int | NumDatatype::Rating => raw
                    .parse::<i64>()
                    .map(|n| n as f64)
                    .map_err(|_| SearchError::NonNumericValue(location.to_string(), raw.clone()))?,
            };
            let v = if v != 0.0 && dt == NumDatatype::Rating {
                (v as i64 / 2) as f64
            } else {
                v
            };
            let hit = match &mode {
                NumMode::True => true,
                NumMode::False => false,
                NumMode::Op(op, q) => apply_num_relop(*op, v, *q),
            };
            if hit {
                matches.extend(ids.iter().copied());
            }
        }
        Ok(matches)
    }
}
// }}}

// --- BooleanSearch {{{

/// Port of `BooleanSearch`. English-only value forms (`yes`/`no`/
/// `checked`/`unchecked`/`empty`/`blank`/`true`/`false`) -- see module
/// docs. Reuses [`crate::utils::force_to_bool`] for value coercion.
pub struct BooleanSearch;

impl BooleanSearch {
    const VALID: &'static [&'static str] = &[
        "_no",
        "false",
        "no",
        "unchecked",
        "_unchecked",
        "checked",
        "_checked",
        "_yes",
        "true",
        "yes",
        "blank",
        "_blank",
        "_empty",
        "empty",
    ];
    const NO_AND_UNCHECKED: &'static [&'static str] =
        &["unchecked", "_unchecked", "no", "_no", "false"];
    const NO_AND_UNCHECKED_WITH_TRUE: &'static [&'static str] =
        &["unchecked", "_unchecked", "no", "_no", "true"];
    const YES_AND_CHECKED: &'static [&'static str] =
        &["checked", "_checked", "yes", "_yes", "true"];
    const EMPTY_AND_BLANK: &'static [&'static str] =
        &["blank", "_blank", "empty", "_empty", "false"];

    pub fn call(
        query: &str,
        field_iter: &[(Option<String>, HashSet<i32>)],
        bools_are_tristate: bool,
    ) -> Result<HashSet<i32>, SearchError> {
        if !Self::VALID.contains(&query) {
            return Err(SearchError::InvalidBoolean(query.to_string()));
        }
        let mut matches = HashSet::new();
        for (val, ids) in field_iter {
            let b = val.as_deref().and_then(crate::utils::force_to_bool);
            let hit = if !bools_are_tristate {
                match b {
                    None | Some(false) => Self::NO_AND_UNCHECKED.contains(&query),
                    Some(true) => Self::YES_AND_CHECKED.contains(&query),
                }
            } else {
                match b {
                    None => Self::EMPTY_AND_BLANK.contains(&query),
                    Some(false) => Self::NO_AND_UNCHECKED_WITH_TRUE.contains(&query),
                    Some(true) => Self::YES_AND_CHECKED.contains(&query),
                }
            };
            if hit {
                matches.extend(ids.iter().copied());
            }
        }
        Ok(matches)
    }
}
// }}}

// --- KeyPairSearch {{{

/// Port of `KeyPairSearch`, for colon-separated fields (`identifiers`).
pub struct KeyPairSearch;

impl KeyPairSearch {
    pub fn call(
        query: &str,
        field_iter: &[(Vec<(String, String)>, HashSet<i32>)],
        candidates: &HashSet<i32>,
        use_primary_find: bool,
    ) -> Result<HashSet<i32>, SearchError> {
        let (keyq, keyq_kind, valq, valq_kind) = if let Some(idx) = query.find(':') {
            let (k, v) = (&query[..idx], &query[idx + 1..]);
            let (km, kq) = matchkind(k.trim(), false);
            let (vm, vq) = matchkind(v.trim(), false);
            (kq, km, vq, vm)
        } else {
            let (vm, vq) = matchkind(query, false);
            (String::new(), MatchKind::Contains, vq, vm)
        };

        if valq == "true" || valq == "false" {
            let mut found = HashSet::new();
            for (pairs, ids) in field_iter {
                let has = if !keyq.is_empty() {
                    pairs.iter().any(|(k, _)| *k == keyq)
                } else {
                    !pairs.is_empty()
                };
                if has {
                    found.extend(ids.iter().copied());
                }
            }
            return Ok(if valq == "true" {
                found
            } else {
                candidates.difference(&found).copied().collect()
            });
        }

        let mut matches = HashSet::new();
        for (pairs, ids) in field_iter {
            for (k, v) in pairs {
                if !keyq.is_empty()
                    && !text_match(&keyq, &[k.clone()], keyq_kind, use_primary_find, false)?
                {
                    continue;
                }
                if !valq.is_empty()
                    && !text_match(&valq, &[v.clone()], valq_kind, use_primary_find, false)?
                {
                    continue;
                }
                matches.extend(ids.iter().copied());
                break;
            }
        }
        Ok(matches)
    }
}
// }}}

// --- Location dispatch table {{{

#[derive(Clone)]
enum LocationKind {
    All,
    Date(&'static str),
    Numeric(&'static str, NumDatatype),
    KeyPair(&'static str, Option<&'static str>),
    Text(&'static str),
    MultiText(&'static str, &'static str),
}

/// Fixed field-name -> location table standing in for
/// `field_metadata.search_term_to_field_key`, covering every field
/// [`Cache::field_for`] knows about (see its own match arms) plus
/// common singular/plural aliases. See module docs.
fn resolve_location(loc: &str) -> Option<LocationKind> {
    Some(match loc {
        "all" => LocationKind::All,
        "title" => LocationKind::Text("title"),
        "sort" | "title_sort" => LocationKind::Text("sort"),
        "author_sort" => LocationKind::Text("author_sort"),
        "authors" | "author" => LocationKind::MultiText("authors", " & "),
        "tags" | "tag" => LocationKind::MultiText("tags", ", "),
        "languages" | "language" | "lang" => LocationKind::MultiText("languages", ", "),
        "formats" | "format" => LocationKind::MultiText("formats", ", "),
        "series" => LocationKind::Text("series"),
        "publisher" | "publishers" => LocationKind::Text("publisher"),
        "comments" | "comment" => LocationKind::Text("comments"),
        "uuid" => LocationKind::Text("uuid"),
        "path" => LocationKind::Text("path"),
        "id" => LocationKind::Numeric("id", NumDatatype::Int),
        "series_index" => LocationKind::Numeric("series_index", NumDatatype::Float),
        "rating" => LocationKind::Numeric("rating", NumDatatype::Rating),
        "size" => LocationKind::Numeric("size", NumDatatype::Int),
        "timestamp" | "date" => LocationKind::Date("timestamp"),
        "pubdate" => LocationKind::Date("pubdate"),
        "last_modified" | "modified" => LocationKind::Date("last_modified"),
        "identifiers" | "identifier" => LocationKind::KeyPair("identifiers", None),
        "isbn" => LocationKind::KeyPair("identifiers", Some("isbn")),
        _ => return None,
    })
}

/// Every location string [`resolve_location`] recognizes -- passed to
/// [`QueryParser::new`] so `location:query` prefixes tokenize
/// correctly.
fn search_locations() -> Vec<String> {
    [
        "all",
        // Not resolved by `resolve_location`/`get_matches` -- handled
        // specially in `evaluate` as a saved-search expansion. Still
        // needs to be in this list so the tokenizer recognizes
        // `search:name` as a location-prefixed token at all.
        "search",
        "title",
        "sort",
        "title_sort",
        "author_sort",
        "authors",
        "author",
        "tags",
        "tag",
        "languages",
        "language",
        "lang",
        "formats",
        "format",
        "series",
        "publisher",
        "publishers",
        "comments",
        "comment",
        "uuid",
        "path",
        "id",
        "series_index",
        "rating",
        "size",
        "timestamp",
        "date",
        "pubdate",
        "last_modified",
        "modified",
        "identifiers",
        "identifier",
        "isbn",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Text-searchable fields swept by a bare `all` (or no-location) query.
/// See module docs' "all location" disclosed simplification.
const ALL_TEXT_LOCATIONS: &[&str] = &[
    "title",
    "authors",
    "tags",
    "series",
    "publisher",
    "comments",
    "languages",
    "formats",
];

fn split_multi(sep: &str, joined: &str) -> Vec<String> {
    joined.split(sep).map(|s| s.to_string()).collect()
}

fn fetch_grouped(
    cache: &Cache,
    field: &str,
    candidates: &HashSet<i32>,
) -> Result<Vec<(Option<String>, HashSet<i32>)>, SearchError> {
    let mut groups: HashMap<Option<String>, HashSet<i32>> = HashMap::new();
    for &book_id in candidates {
        let val = cache.field_for(book_id, field)?;
        groups.entry(val).or_default().insert(book_id);
    }
    Ok(groups.into_iter().collect())
}

fn fetch_identifiers(
    cache: &Cache,
    field: &str,
    candidates: &HashSet<i32>,
) -> Result<Vec<(Vec<(String, String)>, HashSet<i32>)>, SearchError> {
    let mut out = Vec::new();
    for &book_id in candidates {
        let joined = cache.field_for(book_id, field)?;
        let pairs = joined.as_deref().map(parse_identifiers).unwrap_or_default();
        let mut ids = HashSet::new();
        ids.insert(book_id);
        out.push((pairs, ids));
    }
    Ok(out)
}

fn parse_identifiers(joined: &str) -> Vec<(String, String)> {
    joined
        .split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            pair.split_once(':')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// Port of `Parser.get_matches`, narrowed to the fixed location table
/// above. Unknown locations match nothing (same as upstream: `if
/// location not in self.all_search_locations: return matches`).
fn get_matches(
    cache: &Cache,
    location: &str,
    query: &str,
    candidates: &HashSet<i32>,
) -> Result<HashSet<i32>, SearchError> {
    if candidates.is_empty() || query.trim().is_empty() {
        return Ok(HashSet::new());
    }
    let loc = icu_lower(location);
    let Some(kind) = resolve_location(&loc) else {
        return Ok(HashSet::new());
    };

    match kind {
        LocationKind::All => {
            let (mk, q) = matchkind(query, false);
            let mut matches = HashSet::new();
            for &field in ALL_TEXT_LOCATIONS {
                let current: HashSet<i32> = candidates.difference(&matches).copied().collect();
                if current.is_empty() {
                    break;
                }
                let sep = resolve_location(field).and_then(|k| match k {
                    LocationKind::MultiText(_, sep) => Some(sep),
                    _ => None,
                });
                for (val, ids) in fetch_grouped(cache, field, &current)? {
                    if let Some(v) = val {
                        let parts = match sep {
                            Some(sep) => split_multi(sep, &v),
                            None => vec![v],
                        };
                        if text_match(&q, &parts, mk, true, false)? {
                            matches.extend(ids);
                        }
                    }
                }
            }
            Ok(matches)
        }
        LocationKind::Date(field) => {
            let q = icu_lower(query);
            let grouped = fetch_grouped(cache, field, candidates)?;
            DateSearch::call(&q, &grouped)
        }
        LocationKind::Numeric(field, dt) if field == "id" => {
            let q = icu_lower(query);
            let grouped: Vec<(Option<String>, HashSet<i32>)> = candidates
                .iter()
                .map(|&id| {
                    let mut s = HashSet::new();
                    s.insert(id);
                    (Some(id.to_string()), s)
                })
                .collect();
            NumericSearch::call(&q, &grouped, field, dt, candidates, false)
        }
        LocationKind::Numeric(field, dt) => {
            let q = icu_lower(query);
            let grouped = fetch_grouped(cache, field, candidates)?;
            NumericSearch::call(&q, &grouped, field, dt, candidates, false)
        }
        LocationKind::KeyPair(field, forced_key) => {
            let effective_query = match forced_key {
                Some(k) => format!("={k}:{query}"),
                None => query.to_string(),
            };
            let grouped = fetch_identifiers(cache, field, candidates)?;
            KeyPairSearch::call(&effective_query, &grouped, candidates, true)
        }
        LocationKind::Text(field) => {
            let (mk, q) = matchkind(query, false);
            let mut matches = HashSet::new();
            for (val, ids) in fetch_grouped(cache, field, candidates)? {
                if let Some(v) = val {
                    if text_match(&q, &[v], mk, true, false)? {
                        matches.extend(ids);
                    }
                }
            }
            Ok(matches)
        }
        LocationKind::MultiText(field, sep) => {
            let (mk, q) = matchkind(query, false);
            let mut matches = HashSet::new();
            for (val, ids) in fetch_grouped(cache, field, candidates)? {
                if let Some(v) = val {
                    let parts = split_multi(sep, &v);
                    if text_match(&q, &parts, mk, true, false)? {
                        matches.extend(ids);
                    }
                }
            }
            Ok(matches)
        }
    }
}

/// Port of `SearchQueryParser.evaluate_and/or/not/token` -- see module
/// docs for why the candidate-narrowing here is real behavior, not
/// just a tree walk.
///
/// `seen` is upstream's own `self.searches_seen`: the set of saved-
/// search names currently being expanded on this call stack, so a
/// `search:` token can detect (and reject) a cycle instead of
/// recursing forever -- see [`evaluate_saved_search`].
fn evaluate(
    cache: &Cache,
    node: &SearchNode,
    candidates: &HashSet<i32>,
    seen: &mut HashSet<String>,
) -> Result<HashSet<i32>, SearchError> {
    match node {
        SearchNode::And(l, r) => {
            let lm = evaluate(cache, l, candidates, seen)?;
            let rm = evaluate(cache, r, &lm, seen)?;
            Ok(lm.intersection(&rm).copied().collect())
        }
        SearchNode::Or(l, r) => {
            let lm = evaluate(cache, l, candidates, seen)?;
            let remaining: HashSet<i32> = candidates.difference(&lm).copied().collect();
            let rm = evaluate(cache, r, &remaining, seen)?;
            Ok(lm.union(&rm).copied().collect())
        }
        SearchNode::Not(inner) => {
            let m = evaluate(cache, inner, candidates, seen)?;
            Ok(candidates.difference(&m).copied().collect())
        }
        SearchNode::Token { location, query } => {
            if location.eq_ignore_ascii_case("search") {
                evaluate_saved_search(cache, query, candidates, seen)
            } else {
                get_matches(cache, location, query, candidates)
            }
        }
    }
}

/// Port of `evaluate_token`'s `location.lower() == 'search'` branch
/// (`_check_saved_search_recursion` + `_get_saved_search_text`):
/// looks `query` (the saved search's name) up, recursively parses and
/// evaluates its stored query text against `candidates`, and rejects
/// a search that (directly or indirectly) references itself.
fn evaluate_saved_search(
    cache: &Cache,
    query: &str,
    candidates: &HashSet<i32>,
    seen: &mut HashSet<String>,
) -> Result<HashSet<i32>, SearchError> {
    let name = query.strip_prefix('=').unwrap_or(query);
    let name_lower = name.to_lowercase();
    if seen.contains(&name_lower) {
        return Err(SearchError::RecursiveSavedSearch(name.to_string()));
    }
    let saved_query = cache.saved_search_lookup(name)?.ok_or_else(|| SearchError::UnknownSavedSearch(name.to_string()))?;

    seen.insert(name_lower.clone());
    let result = (|| -> Result<HashSet<i32>, SearchError> {
        let mut parser = QueryParser::new(search_locations());
        let tree = parser.parse(&saved_query).map_err(|e| SearchError::Parse(e.to_string()))?;
        evaluate(cache, &tree, candidates, seen)
    })();
    seen.remove(&name_lower);
    result
}
// }}}

/// Port of the top-level search entry point (a narrowed
/// `Search.__call__`/`_do_search`, without the `LRUCache` query cache
/// or virtual-library restriction handling -- see module docs).
pub fn search(cache: &Cache, query: &str) -> anyhow::Result<Vec<i32>> {
    let query = query.trim();
    let all_ids: HashSet<i32> = cache.all_book_ids()?.into_iter().collect();
    if query.is_empty() {
        let mut v: Vec<i32> = all_ids.into_iter().collect();
        v.sort_unstable();
        return Ok(v);
    }
    let mut parser = QueryParser::new(search_locations());
    let tree = parser
        .parse(query)
        .map_err(|e| SearchError::Parse(e.to_string()))?;
    let mut seen = HashSet::new();
    let matches = evaluate(cache, &tree, &all_ids, &mut seen)?;
    let mut v: Vec<i32> = matches.into_iter().collect();
    v.sort_unstable();
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(ids: &[i32]) -> HashSet<i32> {
        ids.iter().copied().collect()
    }

    // --- matchkind / text_match ---

    #[test]
    fn matchkind_recognizes_all_four_prefixes_and_strips_them() {
        assert_eq!(
            matchkind("hello", false),
            (MatchKind::Contains, "hello".to_string())
        );
        assert_eq!(
            matchkind("=Hello", false),
            (MatchKind::Equals, "hello".to_string())
        );
        assert_eq!(
            matchkind("~Hel.o", false),
            (MatchKind::Regexp, "Hel.o".to_string())
        );
        assert_eq!(
            matchkind("^Hello", false),
            (MatchKind::Accent, "hello".to_string())
        );
        // Leading backslash forces plain CONTAINS even though the next
        // char looks like an operator prefix.
        assert_eq!(
            matchkind("\\=x", false),
            (MatchKind::Contains, "=x".to_string())
        );
    }

    #[test]
    fn matchkind_regexp_preserves_case_even_when_case_insensitive() {
        // "leave case in regexps because it can be significant e.g. \S \W \D"
        assert_eq!(
            matchkind("~Foo", false),
            (MatchKind::Regexp, "Foo".to_string())
        );
    }

    #[test]
    fn text_match_contains_is_case_insensitive_by_default() {
        assert!(text_match(
            "asimov",
            &["Isaac Asimov".to_string()],
            MatchKind::Contains,
            true,
            false
        )
        .unwrap());
    }

    #[test]
    fn text_match_equals_requires_exact_match() {
        assert!(text_match(
            "fantasy",
            &["fantasy".to_string()],
            MatchKind::Equals,
            true,
            false
        )
        .unwrap());
        assert!(!text_match(
            "fantasy",
            &["high fantasy".to_string()],
            MatchKind::Equals,
            true,
            false
        )
        .unwrap());
    }

    #[test]
    fn text_match_equals_dot_prefix_matches_hierarchical_prefix() {
        // "=.Fiction" should match "Fiction.Mystery" (a dot-boundary
        // prefix) but not "FictionOther".
        assert!(text_match(
            ".fiction",
            &["fiction.mystery".to_string()],
            MatchKind::Equals,
            true,
            false
        )
        .unwrap());
        assert!(!text_match(
            ".fiction",
            &["fictionother".to_string()],
            MatchKind::Equals,
            true,
            false
        )
        .unwrap());
        assert!(text_match(
            ".fiction",
            &["fiction".to_string()],
            MatchKind::Equals,
            true,
            false
        )
        .unwrap());
    }

    #[test]
    fn text_match_regexp_matches_and_reports_invalid_patterns() {
        assert!(text_match(
            "as.mov",
            &["Asimov".to_string()],
            MatchKind::Regexp,
            true,
            false
        )
        .unwrap());
        assert!(text_match(
            "^Isaac",
            &["Isaac Asimov".to_string()],
            MatchKind::Regexp,
            true,
            false
        )
        .unwrap());
        assert!(text_match(
            "[".into(),
            &["x".to_string()],
            MatchKind::Regexp,
            true,
            false
        )
        .is_err());
    }

    #[test]
    fn text_match_accent_ignores_diacritics() {
        assert!(text_match(
            "cafe",
            &["Café".to_string()],
            MatchKind::Accent,
            true,
            false
        )
        .unwrap());
    }

    // --- DateSearch ---

    fn date_field(pairs: &[(&str, &[i32])]) -> Vec<(Option<String>, HashSet<i32>)> {
        pairs
            .iter()
            .map(|(v, ids)| (Some(v.to_string()), hs(ids)))
            .collect()
    }

    #[test]
    fn date_search_year_only_query_matches_whole_year() {
        let field = date_field(&[
            ("2020-05-01T00:00:00+00:00", &[1]),
            ("2021-01-01T00:00:00+00:00", &[2]),
        ]);
        let m = DateSearch::call("2020", &field).unwrap();
        assert_eq!(m, hs(&[1]));
    }

    #[test]
    fn date_search_greater_than_respects_granularity() {
        let field = date_field(&[
            ("2020-06-01T00:00:00+00:00", &[1]),
            ("2019-06-01T00:00:00+00:00", &[2]),
        ]);
        let m = DateSearch::call(">2019-12-31", &field).unwrap();
        assert_eq!(m, hs(&[1]));
    }

    #[test]
    fn date_search_false_matches_undefined_or_missing_dates() {
        let field = date_field(&[
            ("0101-01-01T00:00:00+00:00", &[1]),
            ("2020-01-01T00:00:00+00:00", &[2]),
        ]);
        let m = DateSearch::call("false", &field).unwrap();
        assert!(m.contains(&1));
        assert!(!m.contains(&2));
    }

    // --- NumericSearch ---

    fn num_field(pairs: &[(Option<&str>, &[i32])]) -> Vec<(Option<String>, HashSet<i32>)> {
        pairs
            .iter()
            .map(|(v, ids)| (v.map(|s| s.to_string()), hs(ids)))
            .collect()
    }

    #[test]
    fn numeric_search_operators_and_multiplier_suffix() {
        let field = num_field(&[
            (Some("1024"), &[1]),
            (Some("2048"), &[2]),
            (Some("512"), &[3]),
        ]);
        let candidates = hs(&[1, 2, 3]);
        let m = NumericSearch::call(">=1k", &field, "size", NumDatatype::Int, &candidates, false)
            .unwrap();
        assert_eq!(m, hs(&[1, 2]));
    }

    #[test]
    fn numeric_search_rating_divides_stored_value_by_two() {
        // Ratings are stored *2 internally (half-star granularity);
        // a plain "3" query should match a stored value of 6.
        let field = num_field(&[(Some("6"), &[1]), (Some("4"), &[2])]);
        let candidates = hs(&[1, 2]);
        let m = NumericSearch::call(
            "3",
            &field,
            "rating",
            NumDatatype::Rating,
            &candidates,
            false,
        )
        .unwrap();
        assert_eq!(m, hs(&[1]));
    }

    #[test]
    fn numeric_search_false_matches_none_values() {
        let field = num_field(&[(None, &[1]), (Some("5"), &[2])]);
        let candidates = hs(&[1, 2]);
        let m = NumericSearch::call(
            "false",
            &field,
            "series_index",
            NumDatatype::Float,
            &candidates,
            false,
        )
        .unwrap();
        assert_eq!(m, hs(&[1]));
    }

    #[test]
    fn numeric_search_rejects_non_numeric_query() {
        let field = num_field(&[(Some("5"), &[1])]);
        let candidates = hs(&[1]);
        assert!(
            NumericSearch::call("abc", &field, "size", NumDatatype::Int, &candidates, false)
                .is_err()
        );
    }

    // --- BooleanSearch ---

    fn bool_field(pairs: &[(Option<&str>, &[i32])]) -> Vec<(Option<String>, HashSet<i32>)> {
        pairs
            .iter()
            .map(|(v, ids)| (v.map(|s| s.to_string()), hs(ids)))
            .collect()
    }

    #[test]
    fn boolean_search_non_tristate_treats_none_as_no() {
        let field = bool_field(&[(None, &[1]), (Some("true"), &[2]), (Some("false"), &[3])]);
        let m = BooleanSearch::call("no", &field, false).unwrap();
        assert_eq!(m, hs(&[1, 3]));
        let m = BooleanSearch::call("yes", &field, false).unwrap();
        assert_eq!(m, hs(&[2]));
    }

    #[test]
    fn boolean_search_tristate_distinguishes_empty_from_no() {
        let field = bool_field(&[(None, &[1]), (Some("false"), &[2]), (Some("true"), &[3])]);
        let m = BooleanSearch::call("empty", &field, true).unwrap();
        assert_eq!(m, hs(&[1]));
        let m = BooleanSearch::call("no", &field, true).unwrap();
        assert_eq!(m, hs(&[2]));
    }

    #[test]
    fn boolean_search_rejects_invalid_query() {
        let field = bool_field(&[(Some("true"), &[1])]);
        assert!(BooleanSearch::call("maybe", &field, false).is_err());
    }

    // --- KeyPairSearch ---

    fn kp_field(pairs: &[(&[(&str, &str)], &[i32])]) -> Vec<(Vec<(String, String)>, HashSet<i32>)> {
        pairs
            .iter()
            .map(|(kv, ids)| {
                (
                    kv.iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    hs(ids),
                )
            })
            .collect()
    }

    #[test]
    fn keypair_search_matches_on_key_and_value() {
        let field = kp_field(&[
            (&[("isbn", "1234567890")][..], &[1]),
            (&[("asin", "B000ABC")][..], &[2]),
        ]);
        let candidates = hs(&[1, 2]);
        let m = KeyPairSearch::call("isbn:1234567890", &field, &candidates, true).unwrap();
        assert_eq!(m, hs(&[1]));
    }

    #[test]
    fn keypair_search_true_false_check_key_presence() {
        let field = kp_field(&[(&[("isbn", "x")][..], &[1]), (&[][..], &[2])]);
        let candidates = hs(&[1, 2]);
        let m = KeyPairSearch::call("isbn:true", &field, &candidates, true).unwrap();
        assert_eq!(m, hs(&[1]));
        let m = KeyPairSearch::call("true", &field, &candidates, true).unwrap();
        assert_eq!(m, hs(&[1]));
    }

    // --- end-to-end against a real Cache ---

    use crate::backend::Backend;
    use tempfile::tempdir;

    fn make_cache() -> (tempfile::TempDir, Cache) {
        let dir = tempdir().unwrap();
        let backend = Backend::new(dir.path()).unwrap();
        (dir, Cache::from_backend(backend))
    }

    fn insert_book(cache: &Cache, title: &str, authors: &[&str], tags: &[&str]) -> i32 {
        let conn = cache.backend.conn.lock().unwrap();
        conn.execute("INSERT INTO books (title) VALUES (?)", [title])
            .unwrap();
        let book_id = conn.last_insert_rowid() as i32;
        for a in authors {
            conn.execute(
                "INSERT OR IGNORE INTO authors (name, sort) VALUES (?, ?)",
                [*a, *a],
            )
            .unwrap();
            let aid: i64 = conn
                .query_row("SELECT id FROM authors WHERE name = ?", [*a], |r| r.get(0))
                .unwrap();
            conn.execute(
                "INSERT INTO books_authors_link (book, author) VALUES (?, ?)",
                rusqlite::params![book_id, aid],
            )
            .unwrap();
        }
        for t in tags {
            conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?)", [*t])
                .unwrap();
            let tid: i64 = conn
                .query_row("SELECT id FROM tags WHERE name = ?", [*t], |r| r.get(0))
                .unwrap();
            conn.execute(
                "INSERT INTO books_tags_link (book, tag) VALUES (?, ?)",
                rusqlite::params![book_id, tid],
            )
            .unwrap();
        }
        book_id
    }

    #[test]
    fn search_plain_word_matches_title_via_all_location() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &["scifi", "classic"]);
        let ids = search(&cache, "foundation").unwrap();
        assert_eq!(ids, vec![1]);
    }

    #[test]
    fn search_location_prefix_matches_specific_field() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &["scifi", "classic"]);
        let ids = search(&cache, "author:herbert").unwrap();
        assert_eq!(ids, vec![2]);
    }

    #[test]
    fn search_and_or_not_combine_with_real_set_semantics() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &["scifi", "classic"]);
        insert_book(&cache, "Emma", &["Jane Austen"], &["classic"]);

        assert_eq!(
            search(&cache, "tag:scifi and tag:classic").unwrap(),
            vec![2]
        );
        assert_eq!(
            search(&cache, "tag:scifi or tag:classic").unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            search(&cache, "tag:classic and not tag:scifi").unwrap(),
            vec![3]
        );
    }

    #[test]
    fn search_expands_a_saved_search_by_name() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Emma", &["Jane Austen"], &["classic"]);
        cache.saved_search_add("scifi books", "tag:scifi").unwrap();

        assert_eq!(search(&cache, "search:\"scifi books\"").unwrap(), vec![1]);
        // Case-insensitive name match, matching saved_search_lookup.
        assert_eq!(search(&cache, "search:\"SCIFI BOOKS\"").unwrap(), vec![1]);
    }

    #[test]
    fn search_combines_a_saved_search_with_other_terms() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &["scifi", "classic"]);
        cache.saved_search_add("scifi", "tag:scifi").unwrap();

        assert_eq!(search(&cache, "search:scifi and tag:classic").unwrap(), vec![2]);
    }

    #[test]
    fn search_of_an_unknown_saved_search_is_a_real_error() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        let err = search(&cache, "search:nonexistent").unwrap_err();
        assert!(err.to_string().contains("nonexistent"), "{err}");
    }

    #[test]
    fn search_rejects_a_directly_recursive_saved_search() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        cache.saved_search_add("loopy", "search:loopy").unwrap();
        let err = search(&cache, "search:loopy").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("recursive"), "{err}");
    }

    #[test]
    fn search_rejects_an_indirectly_recursive_saved_search() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        cache.saved_search_add("a", "search:b").unwrap();
        cache.saved_search_add("b", "search:a").unwrap();
        let err = search(&cache, "search:a").unwrap_err();
        assert!(err.to_string().to_lowercase().contains("recursive"), "{err}");
    }

    #[test]
    fn search_allows_the_same_saved_search_twice_when_not_actually_cyclic() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &["scifi"]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &["scifi", "classic"]);
        cache.saved_search_add("scifi", "tag:scifi").unwrap();
        // Two independent references to the same saved search in one
        // query (not nested inside each other) must not trip the
        // recursion guard -- `seen` is cleared after each expansion
        // completes.
        assert_eq!(search(&cache, "search:scifi or search:scifi").unwrap(), vec![1, 2]);
    }

    #[test]
    fn search_multi_value_field_matches_individual_items_not_the_joined_string() {
        let (_dir, cache) = make_cache();
        // Joined form would be "Isaac Asimov & Robert Silverberg" --
        // a query for "Asimov & Robert" must NOT match: the split-back
        // logic should see these as two separate author names.
        insert_book(
            &cache,
            "Nightfall",
            &["Isaac Asimov", "Robert Silverberg"],
            &[],
        );
        assert!(search(&cache, "author:\"Asimov & Robert\"")
            .unwrap()
            .is_empty());
        assert_eq!(search(&cache, "author:Silverberg").unwrap(), vec![1]);
    }

    #[test]
    fn search_unknown_location_matches_nothing_without_erroring() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &[]);
        assert!(search(&cache, "nosuchfield:foo").unwrap().is_empty());
    }

    #[test]
    fn search_empty_query_returns_every_book() {
        let (_dir, cache) = make_cache();
        insert_book(&cache, "Foundation", &["Isaac Asimov"], &[]);
        insert_book(&cache, "Dune", &["Frank Herbert"], &[]);
        assert_eq!(search(&cache, "").unwrap(), vec![1, 2]);
    }
}
