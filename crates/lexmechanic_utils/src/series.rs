//! Port of `old_src/src/calibre/db/__init__.py`'s series-index helpers
//! (issue #218, a #201 follow-up): `_get_next_series_num_for_list` and
//! `_get_series_values`.
//!
//! # Disclosed simplification
//!
//! Upstream's `_get_next_series_num_for_list(series_indices, unwrap=True)`
//! accepts either a flat list of numbers or a list of `(index, ...)`
//! tuples (unwrapped via `[x[0] for x in series_indices]` when
//! `unwrap` is true) -- a Python-ism for a specific caller shape, not
//! a real behavioral choice. [`get_next_series_num_for_list`] just
//! takes `&[f64]`; callers with tuples extract the field themselves.

use crate::config::CONFIG;
use lazy_static::lazy_static;
use regex::Regex;

/// Port of `_get_next_series_num_for_list`. Reads the
/// `series_index_auto_increment` tweak (`GlobalPrefs`) to decide the
/// strategy: `next` (first integer past the largest existing index,
/// the default), `first_free`/`next_free`/`last_free` (search for a
/// gap), or a fixed number. An empty `series_indices` always returns
/// the configured fixed number (or `1.0` if the tweak isn't numeric).
///
/// The `first_free`/`next_free`/`last_free` loops search integers
/// `1..10000` (matching upstream's own range, comment and all: "really
/// shouldn't get here"). Upstream's `first_free`/`next_free` fall off
/// the end of that loop with no `return` at all (implicitly `None`,
/// a Python type mismatch against every other branch's `float`) if
/// no free slot exists in that range -- practically unreachable, and
/// preserved here as a defensible fallback (one past the search
/// range) rather than an `Option`, since propagating `None` through
/// every caller for a case that can't happen with real book counts
/// isn't worth the API cost.
pub fn get_next_series_num_for_list(series_indices: &[f64]) -> f64 {
    let increment = CONFIG.get_prefs().series_index_auto_increment;

    if series_indices.is_empty() {
        return increment.parse::<f64>().unwrap_or(1.0);
    }

    match increment.as_str() {
        "next" => series_indices.last().unwrap().floor() + 1.0,
        "first_free" => {
            for i in 1..10000 {
                if !series_indices.contains(&(i as f64)) {
                    return i as f64;
                }
            }
            10000.0
        }
        "next_free" => {
            let start = series_indices[0].ceil() as i64;
            for i in start..10000 {
                if !series_indices.contains(&(i as f64)) {
                    return i as f64;
                }
            }
            10000.0
        }
        "last_free" => {
            let start = series_indices.last().unwrap().ceil() as i64;
            for i in (1..=start).rev() {
                if !series_indices.contains(&(i as f64)) {
                    return i as f64;
                }
            }
            series_indices.last().unwrap() + 1.0
        }
        other => other.parse::<f64>().unwrap_or(1.0),
    }
}

lazy_static! {
    static ref SERIES_INDEX_PAT: Regex = Regex::new(r"^(.*)\s+\[([.0-9]+)\]$").unwrap();
}

/// Port of `_get_series_values`: splits a `"Series Name [1.5]"`-style
/// trailing bracketed index off a plain string. Returns `(val, None)`
/// unchanged if there's no bracket suffix, or the suffix isn't a
/// valid number.
pub fn get_series_values(val: &str) -> (String, Option<f64>) {
    if val.is_empty() {
        return (val.to_string(), None);
    }
    let trimmed = val.trim();
    if let Some(caps) = SERIES_INDEX_PAT.captures(trimmed) {
        if let Ok(idx) = caps[2].parse::<f64>() {
            return (caps[1].trim().to_string(), Some(idx));
        }
    }
    (val.to_string(), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_series_values_splits_a_bracketed_index() {
        assert_eq!(
            get_series_values("The Foundation Series [3]"),
            ("The Foundation Series".to_string(), Some(3.0))
        );
        assert_eq!(
            get_series_values("Some Series [1.5]"),
            ("Some Series".to_string(), Some(1.5))
        );
    }

    #[test]
    fn get_series_values_leaves_a_plain_string_unchanged() {
        assert_eq!(
            get_series_values("Just A Title"),
            ("Just A Title".to_string(), None)
        );
    }

    #[test]
    fn get_series_values_handles_empty_input() {
        assert_eq!(get_series_values(""), ("".to_string(), None));
    }

    #[test]
    fn get_series_values_ignores_a_non_numeric_bracket() {
        // Not `[.0-9]+`, so the pattern doesn't match at all -- the
        // whole string (brackets included) passes through unchanged.
        assert_eq!(
            get_series_values("A Title [draft]"),
            ("A Title [draft]".to_string(), None)
        );
    }

    #[test]
    fn get_next_series_num_for_an_empty_list_is_one_by_default() {
        assert_eq!(get_next_series_num_for_list(&[]), 1.0);
    }

    #[test]
    fn get_next_series_num_next_strategy_is_floor_of_last_plus_one() {
        // Reads the real process-global `CONFIG` singleton (this
        // module doesn't have a way to inject a fake one) -- relies on
        // "next" being the tweak's default value, not on nothing else
        // in the process having overridden it. Fine today since
        // nothing in this crate writes `series_index_auto_increment`
        // anywhere yet, but worth knowing if this starts flaking.
        assert_eq!(get_next_series_num_for_list(&[1.0, 2.5, 4.0]), 5.0);
    }
}
