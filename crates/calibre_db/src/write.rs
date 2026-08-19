//! Port of `old_src/src/calibre/db/write.py` (issue #225, a #201
//! follow-up): the real, per-datatype-adapted field-write path every
//! `Cache.set_field` call goes through in upstream.
//!
//! # Scope of this pass
//!
//! Upstream's `write.py` is written against the real `Field`/`Table`
//! in-memory object model (`fields.py`/`tables.py`) -- a `Writer` per
//! field, dispatching to `one_one_in_books`/`one_one_in_other`/
//! `many_one`/`many_many`/`identifiers`/`custom_series_index` based on
//! the field's real `datatype`/`is_multiple` metadata, each of which
//! also keeps that in-memory table in sync. Issue #222 (that object
//! model) is only through its read-side phase 1
//! (`crate::tables::StandardTables`) -- not wired into `Cache` at all
//! yet -- so this pass, same disclosed-scope choice #214/#216 made for
//! reads, is built directly on the simplified per-call-SQL strategy
//! [`crate::cache::Cache::set_field`] (#223) already provides, with
//! real field-name-dispatched adapters standing in for the real
//! datatype-driven ones (no `field_metadata` subsystem exists to
//! drive that dispatch generically -- the recurring #201 gap).
//!
//! What's real, ported from `get_adapter`/the `Writer` classes:
//!
//! - **Value adaptation** before every write: [`adapt_single_text`]
//!   (trim, empty -> `None`), [`adapt_multiple_text`] (split/trim/
//!   collapse-whitespace/dedupe, preserving order -- `multiple_text`/
//!   `uniq`), [`adapt_bool`], [`adapt_rating`] (`0` means "no
//!   rating", clamped to `0..=10`), [`adapt_series_index`],
//!   [`adapt_languages`] (dedupes, drops the `und`/`zxx`/`mis`/`mul`
//!   placeholders upstream also drops), and identifier cleaning
//!   ([`clean_identifier`]). `title`/`author_sort` get upstream's
//!   `"Unknown"`/`""` fallbacks for an empty value; `uuid`/`timestamp`/
//!   `sort`/`pubdate` reject an empty value outright rather than
//!   clearing it (`Writer.accept_vals`).
//! - **"Ignore items whose value is the same as the current value"**:
//!   every writer function in upstream skips books whose desired value
//!   already matches what's stored. [`set_field`] does the same (a
//!   real gap before this pass -- `Cache::set_field` unconditionally
//!   wrote/relinked every time it was called) and returns whether the
//!   book was actually changed, upstream's per-book "dirtied" concept.
//! - **Batch writes**: [`set_field_many`] is upstream's
//!   `Writer.set_books(book_id_val_map, ...)` shape -- one field
//!   across many books at once -- returning the real dirtied-id set.
//! - **`series`'s combined `"Name [3]"` syntax**: routes through
//!   [`calibre_utils::series::get_series_values`] (already real,
//!   #218) and sets both `series` and `series_index` together, wiring
//!   up a helper that existed but had no real caller yet.
//! - **`title`'s `sort` recompute** happens for free: the real
//!   schema's `books_update_trg` already recomputes `sort` via
//!   `title_sort()` whenever `title` changes, so [`set_field`] doesn't
//!   need to replicate `set_title`'s explicit `title_sort_field.writer`
//!   call.
//!
//! # Not ported (disclosed)
//!
//! - Real per-field `datatype`/`is_multiple` dispatch (needs #222's
//!   later phases) -- this dispatches by hardcoded field name instead,
//!   the same simplification `Cache::set_field` (#223) already made.
//! - Custom-column writes (no per-column datatype adapter dispatch
//!   exists here; `Cache::set_custom_column_value` from #214 is
//!   unaffected and still the way to write those).
//! - Enumeration-field allowed-value filtering (`set_books_for_enum`)
//!   -- no enum custom-column type is supported.
//! - `allow_case_change`/case-difference-only rename detection,
//!   dirtying/notification beyond the returned dirtied-id set,
//!   language canonicalization against a real locale database
//!   (`adapt_languages` here only lowercases/trims).

use crate::cache::Cache;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// Port of `single_text`: trims whitespace; an all-whitespace/empty
/// result becomes `None` (upstream: `x if x else None`).
pub fn adapt_single_text(x: &str) -> Option<String> {
    let t = x.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// Port of `multiple_text`/`uniq`: splits on `sep`, trims each part
/// and collapses internal whitespace, drops empties, and
/// de-duplicates case-insensitively while preserving first-seen order.
pub fn adapt_multiple_text(sep: char, x: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for part in x.split(sep) {
        let cleaned: String = part.split_whitespace().collect::<Vec<_>>().join(" ");
        if cleaned.is_empty() {
            continue;
        }
        if seen.insert(cleaned.to_lowercase()) {
            out.push(cleaned);
        }
    }
    out
}

/// Port of `adapt_bool`: `"true"`/`"false"` (case-insensitive),
/// `"none"`/`""` -> `None`, otherwise parsed as an integer and
/// coerced to `bool` (matching upstream's `bool(int(x))` fallback).
pub fn adapt_bool(x: &str) -> Option<bool> {
    match x.trim().to_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        "none" | "" => None,
        other => other.parse::<i64>().ok().map(|n| n != 0),
    }
}

/// Port of the `rating` adapter: `0` (or unparseable) means "no
/// rating" (`None`), otherwise clamped to `0..=10`.
pub fn adapt_rating(x: &str) -> Option<i32> {
    let n: i32 = x.trim().parse().ok()?;
    if n == 0 {
        None
    } else {
        Some(n.clamp(0, 10))
    }
}

/// Port of `adapt_series_index`: parses as `f64`, defaulting to `1.0`
/// on a missing/unparseable value.
pub fn adapt_series_index(x: &str) -> f64 {
    x.trim().parse().unwrap_or(1.0)
}

/// Port of `adapt_languages`: dedupes (order-preserving) and drops
/// the placeholder codes upstream also drops (`und`/`zxx`/`mis`/
/// `mul`). Unlike upstream's `canonicalize_lang`, this only lowercases
/// and trims -- no real locale/ISO-639 database is available in this
/// crate to canonicalize against.
pub fn adapt_languages(x: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for part in x.split(',') {
        let lc = part.trim().to_lowercase();
        if lc.is_empty() || matches!(lc.as_str(), "und" | "zxx" | "mis" | "mul") {
            continue;
        }
        if seen.insert(lc.clone()) {
            out.push(lc);
        }
    }
    out
}

/// Port of `clean_identifier`: lowercases/strips `:`/`,` from the
/// type, and strips the value, replacing `,` with `|` (identifiers are
/// themselves comma-joined by this crate's `field_for`/`set_field`
/// `"type:val,type:val"` contract, so a literal `,` in a value would
/// corrupt that join).
pub fn clean_identifier(typ: &str, val: &str) -> (String, String) {
    let t = typ.trim().to_lowercase().replace([':', ','], "");
    let v = val.trim().replace(',', "|");
    (t, v)
}

/// Adapts `value` for `field` the way upstream's per-field `get_adapter`
/// dispatch does. Returns `None` when the field rejects the value
/// outright (upstream's `Writer.accept_vals`) rather than writing it.
fn adapt(field: &str, value: &str) -> Option<String> {
    match field {
        "title" => Some(adapt_single_text(value).unwrap_or_else(|| "Unknown".to_string())),
        "author_sort" => Some(adapt_single_text(value).unwrap_or_default()),
        "authors" => {
            let names = adapt_multiple_text('&', value);
            Some(if names.is_empty() {
                "Unknown".to_string()
            } else {
                names.join(" & ")
            })
        }
        "tags" => Some(adapt_multiple_text(',', value).join(", ")),
        "languages" => Some(adapt_languages(value).join(", ")),
        "series_index" => Some(adapt_series_index(value).to_string()),
        "rating" => Some(
            adapt_rating(value)
                .map(|n| n.to_string())
                .unwrap_or_else(|| "0".to_string()),
        ),
        "comments" | "publisher" => Some(adapt_single_text(value).unwrap_or_default()),
        "uuid" | "timestamp" | "sort" | "pubdate" => {
            let t = value.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        }
        "identifiers" => {
            let mut pairs = Vec::new();
            for pair in value.split(',') {
                if let Some((k, v)) = pair.split_once(':') {
                    let (k, v) = clean_identifier(k, v);
                    if !k.is_empty() && !v.is_empty() {
                        pairs.push(format!("{k}:{v}"));
                    }
                }
            }
            Some(pairs.join(","))
        }
        _ => Some(value.to_string()),
    }
}

/// Writes `adapted` to `field` on `book_id` only if it differs from
/// the current value -- upstream's "ignore items whose value is the
/// same as the current value" optimization. Returns whether it wrote.
fn write_if_changed(
    cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    field: &str,
    adapted: &str,
) -> Result<bool> {
    let guard = cache.lock().unwrap();
    let current = guard.field_for(book_id, field)?;
    if current.as_deref() == Some(adapted) {
        return Ok(false);
    }
    guard.set_field(book_id, field, adapted)?;
    Ok(true)
}

/// `series` supports upstream's combined `"Name [3]"` syntax (via the
/// already-real [`calibre_utils::series::get_series_values`], #218):
/// when a bracketed index is present, both `series` and `series_index`
/// are set together.
fn set_series_field(cache: &Arc<Mutex<Cache>>, book_id: i32, value: &str) -> Result<bool> {
    let (name, idx) = calibre_utils::series::get_series_values(value);
    let name = adapt_single_text(&name).unwrap_or_default();
    let mut changed = write_if_changed(cache, book_id, "series", &name)?;
    if let Some(idx) = idx {
        if write_if_changed(cache, book_id, "series_index", &idx.to_string())? {
            changed = true;
        }
    }
    Ok(changed)
}

/// Adapts and writes `value` to `field` on `book_id`, skipping the
/// write entirely if the adapted value is unchanged or rejected.
/// Returns whether the book was actually changed.
pub fn set_field(
    cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    field: &str,
    value: &str,
) -> Result<bool> {
    if field == "series" {
        return set_series_field(cache, book_id, value);
    }
    let Some(adapted) = adapt(field, value) else {
        return Ok(false);
    };
    write_if_changed(cache, book_id, field, &adapted)
}

/// Port of `Writer.set_books`: sets `field` on every book in
/// `book_id_val_map` at once, returning the set of book ids that were
/// actually changed.
pub fn set_field_many(
    cache: &Arc<Mutex<Cache>>,
    field: &str,
    book_id_val_map: &HashMap<i32, String>,
) -> Result<HashSet<i32>> {
    let mut dirtied = HashSet::new();
    for (&book_id, value) in book_id_val_map {
        if set_field(cache, book_id, field, value)? {
            dirtied.insert(book_id);
        }
    }
    Ok(dirtied)
}

pub fn set_title(cache: &Arc<Mutex<Cache>>, book_id: i32, title: &str) -> Result<()> {
    set_field(cache, book_id, "title", title).map(|_| ())
}

pub fn set_author_sort(cache: &Arc<Mutex<Cache>>, book_id: i32, author_sort: &str) -> Result<()> {
    set_field(cache, book_id, "author_sort", author_sort).map(|_| ())
}

pub fn update_field(
    cache: &Arc<Mutex<Cache>>,
    book_id: i32,
    field: &str,
    value: &str,
) -> Result<()> {
    set_field(cache, book_id, field, value).map(|_| ())
}
