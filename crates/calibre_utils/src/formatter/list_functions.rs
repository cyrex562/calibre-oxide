//! Port of `formatter_functions.py`'s `LIST_MANIPULATION` +
//! `LIST_LOOKUP` categories (issue #516, part of the #460 formatter
//! epic): built-ins that treat a field's separator-delimited string
//! (tags, authors, identifiers, ...) as a manipulable list. Like
//! [`super::string_functions`], none of these touch a book/`Cache`,
//! so they live here in `calibre_utils` and are reusable by any
//! [`super::interp::ValueSource`] context; `calibre_db::formatter_functions`
//! falls back to [`call`]/[`arg_count`] here (after its own functions
//! and [`super::string_functions`]'s) for everything else.
//!
//! Of the 21 functions in these two upstream categories, 2 are NOT
//! here: `list_count_field` is already one of #513's own inlined
//! `ExprKind::ListCountField` shortcuts, and `list_split` is a new
//! inlined `ExprKind::ListSplit` shortcut added by this issue (it
//! assigns to interpreter locals -- `id_prefix_N` variables -- so,
//! like `assign`, it needs direct mutable access to the interpreter's
//! own locals map and can't be a plain [`super::interp::FunctionRegistry`]
//! call). See `ast.rs`/`parser.rs`/`interp.rs`'s own `ListSplit`
//! additions.
//!
//! # A real, deliberately preserved case-folding inconsistency
//!
//! `list_join`'s own dedup key uses plain `str.lower()`, while every
//! other list-dedup function here (`list_union`/`list_remove_duplicates`/
//! `list_difference`/`list_intersection`/`list_equals`/`str_in_list`)
//! uses `icu_lower`/`strcmp` (real Unicode collation). This is a
//! genuine discrepancy in the real upstream source (confirmed by
//! reading `list_join`'s own `evaluate` body), not a typo introduced
//! by this port -- [`list_join`] uses Rust's plain
//! [`str::to_lowercase`] to match, while everything else uses
//! [`crate::icu::lower`]/[`crate::icu::strcmp`].

use super::interp::FunctionRegistry;
use super::parser::FunctionCatalog;
use crate::icu;
use fancy_regex::Regex as FancyRegex;
use indexmap::IndexMap;
use regex::RegexBuilder;
use std::cmp::Ordering;

pub struct ListFunctions;

impl FunctionRegistry for ListFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        call(name, args)
    }
}

pub struct ListCatalog;

impl FunctionCatalog for ListCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        arg_count(name)
    }
}

/// Parse-time arity/existence for every function [`call`] handles,
/// including aliases -- see [`super::string_functions::arg_count`]'s
/// own doc for the outer/inner `Option` convention.
pub fn arg_count(name: &str) -> Option<Option<usize>> {
    match name {
        "list_count" | "count" => Some(Some(2)),
        "list_count_matching" | "count_matching" => Some(Some(3)),
        "sublist" => Some(Some(4)),
        "subitems" => Some(Some(3)),
        "list_join" => Some(None),
        "list_union" | "merge_lists" => Some(Some(3)),
        "range" => Some(None),
        "list_remove_duplicates" => Some(Some(2)),
        "list_difference" => Some(Some(3)),
        "list_intersection" => Some(Some(3)),
        "list_sort" => Some(Some(3)),
        "list_equals" => Some(Some(6)),
        "list_re" => Some(Some(4)),
        "list_re_group" => Some(None),
        "list_contains" | "in_list" => Some(None),
        "str_in_list" => Some(None),
        "identifier_in_list" => Some(None),
        "list_item" => Some(Some(3)),
        "select" => Some(Some(2)),
        _ => None,
    }
}

/// Real dispatch for every function this module implements -- a free
/// function (not just [`ListFunctions::call`]) so
/// `calibre_db::formatter_functions::CacheFunctions::call` can fall
/// back to it directly.
pub fn call(name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "list_count" | "count" => Ok(list_count(&args[0], &args[1])),
        "list_count_matching" | "count_matching" => list_count_matching(&args[0], &args[1], &args[2]),
        "sublist" => sublist(&args[0], &args[1], &args[2], &args[3]),
        "subitems" => subitems(&args[0], &args[1], &args[2]),
        "list_join" => list_join(args),
        "list_union" | "merge_lists" => Ok(list_union(&args[0], &args[1], &args[2])),
        "range" => range_fn(args),
        "list_remove_duplicates" => Ok(list_remove_duplicates(&args[0], &args[1])),
        "list_difference" => Ok(list_difference(&args[0], &args[1], &args[2])),
        "list_intersection" => Ok(list_intersection(&args[0], &args[1], &args[2])),
        "list_sort" => Ok(list_sort(&args[0], &args[1], &args[2])),
        "list_equals" => Ok(list_equals(&args[0], &args[1], &args[2], &args[3], &args[4], &args[5])),
        "list_re" => list_re(&args[0], &args[1], &args[2], &args[3]),
        "list_re_group" => list_re_group(&args[0], &args[1], &args[2], &args[3], &args[4..]),
        "list_contains" | "in_list" => list_contains(args),
        "str_in_list" => str_in_list(args),
        "identifier_in_list" => identifier_in_list(args),
        "list_item" => list_item(&args[0], &args[1], &args[2]),
        "select" => Ok(select(&args[0], &args[1])),
        _ => Err(format!("No function named {name:?} exists")),
    }
}

/// Port of `list_count`/`count`: number of non-empty (untrimmed)
/// pieces after splitting on `sep`.
fn list_count(val: &str, sep: &str) -> String {
    val.split(sep).filter(|v| !v.is_empty()).count().to_string()
}

/// Port of `list_count_matching`/`count_matching`.
fn list_count_matching(value: &str, pattern: &str, sep: &str) -> Result<String, String> {
    let re = RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let n = value.split(sep).map(str::trim).filter(|v| !v.is_empty()).filter(|v| re.is_match(v)).count();
    Ok(n.to_string())
}

/// Python `seq[start:end]` index normalization (negative wraps from
/// the end, clamped to bounds) -- shared by [`sublist`]/[`subitems`].
fn normalize_slice(len: usize, start: i64, end: i64) -> (usize, usize) {
    let len = len as i64;
    let norm = |i: i64| -> i64 { if i < 0 { (len + i).max(0) } else { i.min(len) } };
    let s = norm(start);
    let e = norm(end);
    if s >= e { (0, 0) } else { (s as usize, e as usize) }
}

/// Port of `sublist`.
fn sublist(val: &str, start_index: &str, end_index: &str, sep: &str) -> Result<String, String> {
    if val.is_empty() {
        return Ok(String::new());
    }
    let si: i64 = start_index.parse().map_err(|_| format!("sublist: invalid start_index {start_index:?}"))?;
    let ei_raw: i64 = end_index.parse().map_err(|_| format!("sublist: invalid end_index {end_index:?}"))?;
    let items: Vec<String> = val.split(sep).map(|v| v.trim().to_string()).collect();
    let ei = if ei_raw == 0 { items.len() as i64 } else { ei_raw };
    let (s, e) = normalize_slice(items.len(), si, ei);
    let join_sep = if sep == "," { ", " } else { sep };
    Ok(items[s..e].join(join_sep))
}

/// Port of `subitems`'s `period_pattern`: split on a `.` only when
/// it's flanked by non-period, non-whitespace characters on both
/// sides (so `"A..B"` or `"A. B"` don't split there).
fn split_periods(item: &str) -> Vec<String> {
    let re = FancyRegex::new(r"(?<=[^.\s])\.(?=[^.\s])").unwrap();
    let mut parts = Vec::new();
    let mut last = 0;
    for m in re.find_iter(item).flatten() {
        parts.push(item[last..m.start()].to_string());
        last = m.end();
    }
    parts.push(item[last..].to_string());
    parts
}

/// Port of `subitems`.
fn subitems(val: &str, start_index: &str, end_index: &str) -> Result<String, String> {
    if val.is_empty() {
        return Ok(String::new());
    }
    let si: i64 = start_index.parse().map_err(|_| format!("subitems: invalid start_index {start_index:?}"))?;
    let ei_raw: i64 = end_index.parse().map_err(|_| format!("subitems: invalid end_index {end_index:?}"))?;
    let has_periods = val.contains('.');
    let mut rv: Vec<String> = Vec::new();
    for item in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let components = if has_periods && item.contains('.') { split_periods(item) } else { vec![item.to_string()] };
        let ei = if ei_raw == 0 { components.len() as i64 } else { ei_raw };
        let (s, e) = normalize_slice(components.len(), si, ei);
        let t = components[s..e].join(".").trim().to_string();
        if !t.is_empty() && !rv.contains(&t) {
            rv.push(t);
        }
    }
    rv.sort_by(|a, b| icu::strcmp(a, b));
    Ok(rv.join(", "))
}

/// Port of `list_join`: real Python-dict-comprehension-`.update()`
/// order semantics (an existing key's *value* is replaced in place,
/// keeping its original position; a new key is appended at the end)
/// via [`IndexMap::insert`], which has that exact behavior.
fn list_join(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Err("list_join requires at least 1 argument".to_string());
    }
    let with_separator = &args[0];
    let pairs = &args[1..];
    if pairs.len() % 2 != 0 {
        return Err("Invalid 'List, separator' pairs. Every list must have one associated separator".to_string());
    }
    let mut result: IndexMap<String, String> = IndexMap::new();
    let mut i = 0;
    while i < pairs.len() {
        let list = &pairs[i];
        let sep = &pairs[i + 1];
        for item in list.split(sep.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            result.insert(item.to_lowercase(), item.to_string());
        }
        i += 2;
    }
    Ok(result.into_values().collect::<Vec<_>>().join(with_separator))
}

/// Port of `list_union`/`merge_lists`.
fn list_union(list1: &str, list2: &str, separator: &str) -> String {
    let mut res: IndexMap<String, String> = IndexMap::new();
    for l in list2.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
        res.insert(icu::lower(l), l.to_string());
    }
    for l in list1.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
        res.insert(icu::lower(l), l.to_string());
    }
    let sep = if separator == "," { ", " } else { separator };
    res.into_values().collect::<Vec<_>>().join(sep)
}

/// Port of `range`: Python `range(start, stop, step)` length, computed
/// without materializing the sequence, so a huge (but within `limit`)
/// range doesn't need `limit`-sized intermediate storage either.
fn range_fn(args: &[String]) -> Result<String, String> {
    if args.is_empty() {
        return Err("range: requires at least 1 argument".to_string());
    }
    let parse = |s: &str| -> Result<i64, String> { if s.is_empty() || s == "None" { Ok(0) } else { s.parse::<i64>().map_err(|_| format!("range: invalid integer {s:?}")) } };
    let mut start = 0i64;
    let stop;
    let mut step = 1i64;
    let mut limit = 1000i64;
    if args.len() == 1 {
        stop = parse(&args[0])?;
    } else if args.len() == 2 {
        start = parse(&args[0])?;
        stop = parse(&args[1])?;
    } else {
        start = parse(&args[0])?;
        stop = parse(&args[1])?;
        step = parse(&args[2])?;
        if args.len() > 3 {
            limit = parse(&args[3])?;
        }
    }
    if step == 0 {
        return Err("range() arg 3 must not be zero".to_string());
    }
    let len: i64 = if step > 0 {
        if start >= stop { 0 } else { (stop - start - 1) / step + 1 }
    } else if start <= stop {
        0
    } else {
        (start - stop - 1) / (-step) + 1
    };
    if len > limit {
        return Err(format!("range: length ({len}) longer than limit ({limit})"));
    }
    let mut items = Vec::with_capacity(len.max(0) as usize);
    let mut v = start;
    for _ in 0..len {
        items.push(v.to_string());
        v += step;
    }
    Ok(items.join(", "))
}

/// Port of `list_remove_duplicates`.
fn list_remove_duplicates(list: &str, separator: &str) -> String {
    let mut res: IndexMap<String, String> = IndexMap::new();
    for l in list.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
        res.insert(icu::lower(l), l.to_string());
    }
    let sep = if separator == "," { ", " } else { separator };
    res.into_values().collect::<Vec<_>>().join(sep)
}

/// Port of `list_difference`.
fn list_difference(list1: &str, list2: &str, separator: &str) -> String {
    let l1: Vec<&str> = list1.split(separator).map(str::trim).filter(|s| !s.is_empty()).collect();
    let l2: std::collections::HashSet<String> = list2.split(separator).map(str::trim).filter(|s| !s.is_empty()).map(icu::lower).collect();
    let mut res: Vec<&str> = Vec::new();
    for i in &l1 {
        if !l2.contains(&icu::lower(i)) && !res.contains(i) {
            res.push(i);
        }
    }
    let sep = if separator == "," { ", " } else { separator };
    res.join(sep)
}

/// Port of `list_intersection`.
fn list_intersection(list1: &str, list2: &str, separator: &str) -> String {
    let l1: Vec<&str> = list1.split(separator).map(str::trim).filter(|s| !s.is_empty()).collect();
    let l2: std::collections::HashSet<String> = list2.split(separator).map(str::trim).filter(|s| !s.is_empty()).map(icu::lower).collect();
    let mut res: Vec<&str> = Vec::new();
    for i in &l1 {
        if l2.contains(&icu::lower(i)) && !res.contains(i) {
            res.push(i);
        }
    }
    let sep = if separator == "," { ", " } else { separator };
    res.join(sep)
}

/// Port of `list_sort`: real Unicode-collation-aware sort (matching
/// `icu::strcmp`, the same comparator this crate already uses for
/// other locale-aware sorts -- see e.g. `virtual_libraries`/
/// `author_links` in `calibre_db::formatter_functions`).
fn list_sort(value: &str, direction: &str, separator: &str) -> String {
    let mut res: Vec<&str> = value.split(separator).map(str::trim).filter(|s| !s.is_empty()).collect();
    res.sort_by(|a, b| icu::strcmp(a, b));
    if direction != "0" {
        res.reverse();
    }
    let sep = if separator == "," { ", " } else { separator };
    res.join(sep)
}

/// Port of `list_equals`.
fn list_equals(list1: &str, sep1: &str, list2: &str, sep2: &str, yes_val: &str, no_val: &str) -> String {
    let s1: std::collections::HashSet<String> = list1.split(sep1).map(str::trim).filter(|s| !s.is_empty()).map(icu::lower).collect();
    let s2: std::collections::HashSet<String> = list2.split(sep2).map(str::trim).filter(|s| !s.is_empty()).map(icu::lower).collect();
    if s1 == s2 { yes_val.to_string() } else { no_val.to_string() }
}

/// Port of `list_re`.
fn list_re(src_list: &str, separator: &str, include_re: &str, opt_replace: &str) -> Result<String, String> {
    let re = RegexBuilder::new(include_re).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let mut res: Vec<String> = Vec::new();
    for item in src_list.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
        if re.is_match(item) {
            let replaced = if !opt_replace.is_empty() {
                let rust_repl = super::string_functions::translate_python_replacement(opt_replace);
                re.replace_all(item, rust_repl.as_str()).into_owned()
            } else {
                item.to_string()
            };
            for piece in replaced.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
                if !res.iter().any(|r| r == piece) {
                    res.push(piece.to_string());
                }
            }
        }
    }
    let sep = if separator == "," { ", " } else { separator };
    Ok(res.join(sep))
}

/// Port of `list_re_group`: like [`list_re`] but the per-matched-item
/// transform is a real [`super::string_functions::re_group`] call
/// (unconditional replacement via group templates) rather than a
/// plain optional string replacement.
fn list_re_group(src_list: &str, separator: &str, include_re: &str, search_re: &str, templates: &[String]) -> Result<String, String> {
    let filter_re = RegexBuilder::new(include_re).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let mut res: Vec<String> = Vec::new();
    for item in src_list.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
        if filter_re.is_match(item) {
            let replaced = super::string_functions::re_group(item, search_re, templates)?;
            for piece in replaced.split(separator).map(str::trim).filter(|s| !s.is_empty()) {
                if !res.iter().any(|r| r == piece) {
                    res.push(piece.to_string());
                }
            }
        }
    }
    let sep = if separator == "," { ", " } else { separator };
    Ok(res.join(sep))
}

/// Port of `list_contains`/`in_list`.
fn list_contains(args: &[String]) -> Result<String, String> {
    if args.len() < 3 {
        return Err("in_list requires at least 3 arguments".to_string());
    }
    let val = &args[0];
    let sep = &args[1];
    let rest = &args[2..];
    if rest.len() % 2 != 1 {
        return Err("in_list requires an odd number of arguments".to_string());
    }
    let items: Vec<&str> = val.split(sep.as_str()).map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut i = 0;
    while i < rest.len() {
        if i + 1 >= rest.len() {
            return Ok(rest[i].clone());
        }
        let pattern = &rest[i];
        let found_val = &rest[i + 1];
        let re = RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| e.to_string())?;
        if items.iter().any(|v| re.is_match(v)) {
            return Ok(found_val.clone());
        }
        i += 2;
    }
    unreachable!("rest.len() is odd, so the loop always returns via the trailing not_found_val")
}

/// Port of `str_in_list`. Disclosed docstring/code discrepancy: the
/// real docstring claims the comparison is case-insensitive, but the
/// real code compares with `strcmp` (full Unicode collation, which
/// distinguishes case as a real, if low-priority, ordering/equality
/// factor -- confirmed by `icu::strcmp`'s own test suite, e.g.
/// `strcmp("a", "A") != Equal`), not `icu_lower`/`primary_strcmp`
/// (which would actually fold case). This port matches the real code,
/// not the docstring's claim.
fn str_in_list(args: &[String]) -> Result<String, String> {
    if args.len() < 3 {
        return Err("str_in_list requires at least 3 arguments".to_string());
    }
    let val = &args[0];
    let sep = &args[1];
    let rest = &args[2..];
    if rest.len() % 2 != 1 {
        return Err("str_in_list requires an odd number of arguments".to_string());
    }
    let items: Vec<String> = val.split(sep.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
    let mut i = 0;
    while i < rest.len() {
        if i + 1 >= rest.len() {
            return Ok(rest[i].clone());
        }
        let candidates: Vec<String> = rest[i].split(sep.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect();
        let found_val = &rest[i + 1];
        if items.iter().any(|v| candidates.iter().any(|t| icu::strcmp(t, v) == Ordering::Equal)) {
            return Ok(found_val.clone());
        }
        i += 2;
    }
    unreachable!("rest.len() is odd, so the loop always returns via the trailing not_found_val")
}

/// Port of `identifier_in_list`.
fn identifier_in_list(args: &[String]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("identifier_in_list requires 2 or 4 arguments".to_string());
    }
    let val = &args[0];
    let ident = &args[1];
    let extra = &args[2..];
    let (fv_is_id, fv, nfv) = match extra.len() {
        0 => (true, String::new(), String::new()),
        2 => (false, extra[0].clone(), extra[1].clone()),
        _ => return Err("identifier_in_list requires 2 or 4 arguments".to_string()),
    };
    let (id_, regexp) = match ident.split_once(':') {
        Some((a, b)) => (a, b),
        None => (ident.as_str(), ""),
    };
    if id_.is_empty() {
        return Ok(nfv);
    }
    for candidate in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let Some((i, v)) = candidate.split_once(':') else { continue };
        if v.is_empty() || i != id_ {
            continue;
        }
        let matched = if regexp.is_empty() {
            true
        } else {
            RegexBuilder::new(regexp).case_insensitive(true).build().map_err(|e| e.to_string())?.is_match(v)
        };
        if matched {
            return Ok(if fv_is_id { candidate.to_string() } else { fv });
        }
    }
    Ok(nfv)
}

/// Port of `list_item`.
fn list_item(val: &str, index: &str, sep: &str) -> Result<String, String> {
    if val.is_empty() {
        return Ok(String::new());
    }
    let idx: i64 = index.parse().map_err(|_| format!("list_item: invalid index {index:?}"))?;
    let items: Vec<&str> = val.split(sep).collect();
    let n = items.len() as i64;
    let real_idx = if idx < 0 { n + idx } else { idx };
    if real_idx < 0 || real_idx >= n {
        return Ok(String::new());
    }
    Ok(items[real_idx as usize].trim().to_string())
}

/// Port of `select`.
fn select(val: &str, key: &str) -> String {
    if val.is_empty() {
        return String::new();
    }
    let tkey = format!("{key}:");
    for v in val.split(',').map(str::trim) {
        if let Some(rest) = v.strip_prefix(tkey.as_str()) {
            return rest.to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_count_ignores_empty_pieces() {
        assert_eq!(call("list_count", &["a,b,,c".to_string(), ",".to_string()]).unwrap(), "3");
        assert_eq!(call("count", &["a & b".to_string(), "&".to_string()]).unwrap(), "2");
    }

    #[test]
    fn list_count_matching_counts_regex_hits() {
        assert_eq!(call("list_count_matching", &["Fiction, Fantasy, Drama".to_string(), "^F".to_string(), ",".to_string()]).unwrap(), "2");
    }

    #[test]
    fn sublist_matches_documented_examples() {
        assert_eq!(call("sublist", &["A, B, C".to_string(), "0".to_string(), "1".to_string(), ",".to_string()]).unwrap(), "A");
        assert_eq!(call("sublist", &["A, B, C".to_string(), "-1".to_string(), "0".to_string(), ",".to_string()]).unwrap(), "C");
        assert_eq!(call("sublist", &["A, B, C".to_string(), "0".to_string(), "-1".to_string(), ",".to_string()]).unwrap(), "A, B");
    }

    #[test]
    fn subitems_matches_documented_examples() {
        assert_eq!(call("subitems", &["A.B.C".to_string(), "0".to_string(), "1".to_string()]).unwrap(), "A");
        assert_eq!(call("subitems", &["A.B.C".to_string(), "0".to_string(), "2".to_string()]).unwrap(), "A.B");
        assert_eq!(call("subitems", &["A.B.C".to_string(), "1".to_string(), "0".to_string()]).unwrap(), "B.C");
        assert_eq!(call("subitems", &["A.B.C, D.E".to_string(), "0".to_string(), "1".to_string()]).unwrap(), "A, D");
        assert_eq!(call("subitems", &["A.B.C, D.E".to_string(), "0".to_string(), "2".to_string()]).unwrap(), "A.B, D.E");
    }

    #[test]
    fn list_join_preserves_first_insertion_position_but_the_later_pairs_value_wins() {
        // Real Python dict-comprehension-`.update()` semantics: the
        // *value* for an existing key is replaced by whichever pair
        // is processed last, but the key's *position* stays wherever
        // it was first inserted.
        let out = call("list_join", &[";".to_string(), "TAG,other".to_string(), ",".to_string(), "tag".to_string(), ",".to_string()]).unwrap();
        assert_eq!(out, "tag;other");
    }

    #[test]
    fn list_join_requires_even_pairs() {
        assert!(call("list_join", &[";".to_string(), "a,b".to_string()]).is_err());
    }

    #[test]
    fn list_union_prefers_list1_casing() {
        assert_eq!(call("list_union", &["Fiction".to_string(), "fiction, Drama".to_string(), ",".to_string()]).unwrap(), "Fiction, Drama");
    }

    #[test]
    fn range_matches_documented_examples() {
        assert_eq!(call("range", &["5".to_string()]).unwrap(), "0, 1, 2, 3, 4");
        assert_eq!(call("range", &["0".to_string(), "5".to_string()]).unwrap(), "0, 1, 2, 3, 4");
        assert_eq!(call("range", &["-1".to_string(), "5".to_string()]).unwrap(), "-1, 0, 1, 2, 3, 4");
        assert_eq!(call("range", &["1".to_string(), "5".to_string()]).unwrap(), "1, 2, 3, 4");
        assert_eq!(call("range", &["1".to_string(), "5".to_string(), "2".to_string()]).unwrap(), "1, 3");
        assert_eq!(call("range", &["1".to_string(), "5".to_string(), "2".to_string(), "5".to_string()]).unwrap(), "1, 3");
        assert!(call("range", &["1".to_string(), "5".to_string(), "2".to_string(), "1".to_string()]).is_err());
    }

    #[test]
    fn list_remove_duplicates_is_case_insensitive_and_keeps_the_last_casing() {
        assert_eq!(call("list_remove_duplicates", &["a, A, b".to_string(), ",".to_string()]).unwrap(), "A, b");
    }

    #[test]
    fn list_difference_and_intersection_are_case_insensitive() {
        assert_eq!(call("list_difference", &["A, B, C".to_string(), "b".to_string(), ",".to_string()]).unwrap(), "A, C");
        assert_eq!(call("list_intersection", &["A, B, C".to_string(), "b, c".to_string(), ",".to_string()]).unwrap(), "B, C");
    }

    #[test]
    fn list_sort_supports_ascending_and_descending() {
        assert_eq!(call("list_sort", &["c, a, b".to_string(), "0".to_string(), ",".to_string()]).unwrap(), "a, b, c");
        assert_eq!(call("list_sort", &["c, a, b".to_string(), "1".to_string(), ",".to_string()]).unwrap(), "c, b, a");
    }

    #[test]
    fn list_equals_ignores_order_and_case() {
        assert_eq!(call("list_equals", &["A,B".to_string(), ",".to_string(), "b,a".to_string(), ",".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "yes");
        assert_eq!(call("list_equals", &["A,B".to_string(), ",".to_string(), "b,c".to_string(), ",".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "no");
    }

    #[test]
    fn list_re_filters_and_optionally_replaces() {
        assert_eq!(call("list_re", &["Fiction, Drama, Fantasy".to_string(), ",".to_string(), "^F".to_string(), "".to_string()]).unwrap(), "Fiction, Fantasy");
        assert_eq!(call("list_re", &["foo, bar".to_string(), ",".to_string(), "(.*)".to_string(), r"[\1]".to_string()]).unwrap(), "[foo], [bar]");
    }

    #[test]
    fn list_re_group_applies_group_templates_per_item() {
        let out = call(
            "list_re_group",
            &["hello, world".to_string(), ",".to_string(), ".*".to_string(), r"(\S+)".to_string(), "{$:uppercase()}".to_string()],
        )
        .unwrap();
        assert_eq!(out, "HELLO, WORLD");
    }

    #[test]
    fn list_contains_returns_first_match_or_the_trailing_default() {
        assert_eq!(call("list_contains", &["Fiction, Drama".to_string(), ",".to_string(), "^F".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "yes");
        assert_eq!(call("in_list", &["Fiction, Drama".to_string(), ",".to_string(), "^Z".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "no");
        // A single trailing arg (no pattern/found_val pairs) is valid
        // -- it's just an unconditional default.
        assert_eq!(call("list_contains", &["a".to_string(), ",".to_string(), "x".to_string()]).unwrap(), "x");
        assert!(call("list_contains", &["a".to_string(), ",".to_string(), "x".to_string(), "y".to_string()]).is_err(), "an even count after val,sep is a real error");
    }

    #[test]
    fn str_in_list_compares_exact_case_despite_its_own_docstrings_ignoring_case_claim() {
        // See this module's own `str_in_list` doc for the real
        // docstring/code discrepancy this preserves.
        assert_eq!(call("str_in_list", &["Fiction, Drama".to_string(), ",".to_string(), "Fiction".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "yes");
        assert_eq!(call("str_in_list", &["Fiction, Drama".to_string(), ",".to_string(), "fiction".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "no");
    }

    #[test]
    fn identifier_in_list_matches_documented_forms() {
        let ids = "isbn:1234, url:http://example.com".to_string();
        assert_eq!(call("identifier_in_list", &[ids.clone(), "isbn".to_string()]).unwrap(), "isbn:1234");
        assert_eq!(call("identifier_in_list", &[ids.clone(), "doi".to_string()]).unwrap(), "");
        assert_eq!(call("identifier_in_list", &[ids.clone(), "isbn".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "yes");
        assert_eq!(call("identifier_in_list", &[ids, "isbn:^12".to_string(), "yes".to_string(), "no".to_string()]).unwrap(), "yes");
    }

    #[test]
    fn list_item_supports_negative_indices_and_out_of_range() {
        assert_eq!(call("list_item", &["a & b & c".to_string(), "-1".to_string(), "&".to_string()]).unwrap(), "c");
        assert_eq!(call("list_item", &["a & b".to_string(), "5".to_string(), "&".to_string()]).unwrap(), "");
    }

    #[test]
    fn select_finds_the_first_matching_identifier() {
        assert_eq!(call("select", &["isbn:1234, url:foo".to_string(), "url".to_string()]).unwrap(), "foo");
        assert_eq!(call("select", &["isbn:1234".to_string(), "doi".to_string()]).unwrap(), "");
    }

    #[test]
    fn unknown_function_is_a_real_error() {
        assert!(call("no_such_function", &[]).is_err());
    }

    #[test]
    fn catalog_reports_correct_arity_including_aliases() {
        assert_eq!(arg_count("list_count"), Some(Some(2)));
        assert_eq!(arg_count("count"), Some(Some(2)));
        assert_eq!(arg_count("merge_lists"), Some(Some(3)));
        assert_eq!(arg_count("range"), Some(None));
        assert_eq!(arg_count("no_such_function"), None);
    }
}
