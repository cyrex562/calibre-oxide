//! Port of `calibre.utils.matcher` (issue #462): a fuzzy subsequence-
//! matching/scoring algorithm (a "quick open" style filter) backing
//! the GUI Preferences search box.
//!
//! # Disclosed narrowings
//!
//! - **`matcher.c`** (the C-accelerated `CScorer`) is not ported
//!   separately. It's a pure performance reimplementation of the exact
//!   same algorithm as `matcher.py`'s own `PyScorer`/`process_item`
//!   (flat pre-allocated arrays instead of a Python dict for
//!   memoization) -- not a different algorithm. This port implements
//!   the algorithm once, in Rust, which is already native-speed.
//! - **The `Worker`/`Queue`-based thread pool** (`matcher.py`'s
//!   `Worker`/`workers`/`split`) exists only to route CPU-bound
//!   scoring work around Python's GIL. Rust has no GIL, so this port
//!   scores sequentially -- the real, correct equivalent, not a
//!   narrowing. A `rayon`-based parallel scan would be a legitimate
//!   future optimization once a real consumer with an actual
//!   performance need exists (this issue's own text: "backs a
//!   specific GUI screen that doesn't appear to exist yet in this
//!   port either").
//! - **`primary_find`** (upstream: ICU4C's `usearch.h`-based
//!   collation-strength substring search, via `calibre.utils.icu`) is
//!   approximated the same documented way
//!   [`crate::icu`]'s own module doc already discloses for
//!   `calibre_db::search`'s `primary_contains`: NFD-decompose + strip
//!   combining marks + lowercase, then compare. Folded once per
//!   character (not re-normalized on every comparison), for both
//!   correctness-parity with that existing precedent and performance.
//! - **Empty query**: real upstream's `max_score_per_char = (1.0/len(item)
//!   + 1.0/len(needle)) / 2.0` divides by zero (`len(needle) == 0`)
//!   when queried with an empty string, which Python raises
//!   `ZeroDivisionError` for. Rust's float division would instead
//!   silently produce `inf`. Neither is a good outcome for an
//!   interactive filter's "nothing typed yet" state -- this port
//!   explicitly treats an empty query as matching nothing (empty
//!   results), a disclosed behavior choice rather than a crash or an
//!   `inf`-score leak.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use unicode_normalization::UnicodeNormalization;

use crate::icu::{lower, upper};

pub const DEFAULT_LEVEL1: &str = "/";
pub const DEFAULT_LEVEL2: &str = "-_ 0123456789";
pub const DEFAULT_LEVEL3: &str = ".";

fn is_combining_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

/// Primary-strength collation folding for a single character: NFD
/// decompose, drop combining marks, lowercase. See the module doc's
/// disclosed narrowing on `primary_find`.
fn primary_fold(c: char) -> String {
    c.to_string().nfd().filter(|ch| !is_combining_mark(*ch)).collect::<String>().to_lowercase()
}

struct ScoreContext<'a> {
    level1: &'a HashSet<char>,
    level2: &'a HashSet<char>,
    level3: &'a HashSet<char>,
    max_score_per_char: f64,
    memory: HashMap<(usize, usize, usize), (f64, Vec<i64>)>,
}

/// Port of `calc_score_for_char`: the per-matched-character score
/// factor -- higher for matches right after a `level1` separator
/// (e.g. `/`), a `level2` separator or a lower-to-upper case
/// transition (camelCase word boundaries), a `level3` separator, or
/// simply closer together in the haystack.
fn calc_score_for_char(ctx: &ScoreContext, prev: char, current: char, distance: i64) -> f64 {
    let factor = if ctx.level1.contains(&prev) {
        0.9
    } else if ctx.level2.contains(&prev) || (lower(&prev.to_string()) == prev.to_string() && upper(&current.to_string()) == current.to_string()) {
        0.8
    } else if ctx.level3.contains(&prev) {
        0.7
    } else {
        (1.0 / distance as f64) * 0.75
    };
    ctx.max_score_per_char * factor
}

/// Port of `process_item`'s real non-recursive stack-based search:
/// finds the highest-scoring way to match every character of `needle`
/// (in order, as a subsequence) somewhere in `haystack`, backtracking
/// to try later occurrences of each needle character when a later
/// occurrence would score higher overall. Memoized by `(hidx, nidx,
/// last_idx)` exactly as upstream is, to avoid recomputing the same
/// sub-search twice.
fn process_item(ctx: &mut ScoreContext, haystack: &[char], haystack_folded: &[String], needle_folded: &[String]) -> (f64, Vec<i64>) {
    let needle_len = needle_folded.len();
    let mut stack: Vec<(usize, usize, usize, f64, Vec<i64>)> = vec![(0, 0, 0, 0.0, vec![-1i64; needle_len])];
    let mut final_score = stack[0].3;
    let mut final_positions = stack[0].4.clone();

    while let Some((mut hidx, nidx, mut last_idx, mut score, mut positions)) = stack.pop() {
        let key = (hidx, nidx, last_idx);
        let (out_score, out_positions) = if let Some((s, p)) = ctx.memory.get(&key) {
            (*s, p.clone())
        } else {
            'inner: for i in nidx..needle_len {
                if haystack.len() - hidx < needle_len - i {
                    score = 0.0;
                    break 'inner;
                }
                let found = haystack_folded[hidx..].iter().position(|f| f == &needle_folded[i]);
                let pos = match found {
                    Some(p) => p + hidx,
                    None => {
                        score = 0.0;
                        break 'inner;
                    }
                };

                let distance = pos as i64 - last_idx as i64;
                let score_for_char = if distance <= 1 { ctx.max_score_per_char } else { calc_score_for_char(ctx, haystack[pos - 1], haystack[pos], distance) };
                hidx = pos + 1;
                // Backtrack state: retry needle char `i` starting from
                // a later haystack position, using an independent copy
                // of `positions` from before this attempt committed.
                stack.push((hidx, i, last_idx, score, positions.clone()));
                last_idx = pos;
                positions[i] = pos as i64;
                score += score_for_char;
            }
            ctx.memory.insert(key, (score, positions.clone()));
            (score, positions)
        };
        if out_score > final_score {
            final_score = out_score;
            final_positions = out_positions;
        }
    }
    (final_score, final_positions)
}

/// Port of `PyScorer.__call__`'s per-item setup (fresh `max_score_per_char`
/// and memoization table per item) plus `process_item` itself.
fn score_one(haystack: &[char], needle: &[char], level1: &HashSet<char>, level2: &HashSet<char>, level3: &HashSet<char>) -> (f64, Vec<i64>) {
    if needle.is_empty() || haystack.is_empty() {
        return (0.0, vec![-1i64; needle.len()]);
    }
    let haystack_folded: Vec<String> = haystack.iter().map(|&c| primary_fold(c)).collect();
    let needle_folded: Vec<String> = needle.iter().map(|&c| primary_fold(c)).collect();
    let max_score_per_char = (1.0 / haystack.len() as f64 + 1.0 / needle.len() as f64) / 2.0;
    let mut ctx = ScoreContext { level1, level2, level3, max_score_per_char, memory: HashMap::new() };
    process_item(&mut ctx, haystack, &haystack_folded, &needle_folded)
}

/// Port of `Matcher`: scores and ranks `items` against a query.
pub struct Matcher {
    items: Vec<String>,
    level1: HashSet<char>,
    level2: HashSet<char>,
    level3: HashSet<char>,
}

impl Matcher {
    /// Port of `Matcher.__init__` with the real default level
    /// separators.
    pub fn new(items: impl IntoIterator<Item = String>) -> Self {
        Self::with_levels(items, DEFAULT_LEVEL1, DEFAULT_LEVEL2, DEFAULT_LEVEL3)
    }

    /// Port of `Matcher.__init__` with explicit level separators.
    /// Empty items are dropped (`filter(None, items)`), and every
    /// item is NFC-normalized, matching upstream exactly.
    pub fn with_levels(items: impl IntoIterator<Item = String>, level1: &str, level2: &str, level3: &str) -> Self {
        let items: Vec<String> = items.into_iter().filter(|s| !s.is_empty()).map(|s| s.nfc().collect()).collect();
        Matcher {
            items,
            level1: level1.chars().collect(),
            level2: level2.chars().collect(),
            level3: level3.chars().collect(),
        }
    }

    /// Port of `Matcher.__call__`: scores every item against `query`,
    /// returns `(item, positions)` pairs -- `positions` are the
    /// per-`query`-character match locations (character indices into
    /// `item`), in descending-score order. Items that don't match at
    /// all (score `0`) are excluded. When `limit` is given, real
    /// upstream applies it to the SORTED (not-yet-filtered) list
    /// before dropping zero-score items -- reproduced exactly, so a
    /// small `limit` can legitimately return fewer than `limit`
    /// results if some of the top-`limit`-by-score items didn't
    /// actually match.
    pub fn call(&self, query: &str, limit: Option<usize>) -> Vec<(String, Vec<i64>)> {
        let query: String = query.nfc().collect();
        let needle: Vec<char> = query.chars().collect();

        let mut scored: Vec<(f64, &String, Vec<i64>)> = self
            .items
            .iter()
            .map(|item| {
                let haystack: Vec<char> = item.chars().collect();
                let (score, positions) = score_one(&haystack, &needle, &self.level1, &self.level2, &self.level3);
                (score, item, positions)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(limit) = limit {
            scored.truncate(limit);
        }
        scored.into_iter().filter(|&(score, _, _)| score != 0.0).map(|(_, item, positions)| (item.clone(), positions)).collect()
    }
}

/// Port of `get_items_from_dir`: every file under `basedir` (recursive),
/// as a `/`-separated path relative to `basedir`, for which `accept`
/// returns `true`.
pub fn get_items_from_dir(basedir: &Path, accept: impl Fn(&Path) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    walk_dir(basedir, basedir, &accept, &mut out);
    out
}

fn walk_dir(dir: &Path, basedir: &Path, accept: &dyn Fn(&Path) -> bool, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            walk_dir(&path, basedir, accept, out);
        } else if accept(&path) {
            if let Ok(rel) = path.strip_prefix(basedir) {
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
}

/// Port of `FilesystemMatcher`: a [`Matcher`] over every file under a
/// directory tree.
pub struct FilesystemMatcher(Matcher);

impl FilesystemMatcher {
    pub fn new(basedir: &Path) -> Self {
        Self::with_accept(basedir, |_| true)
    }

    pub fn with_accept(basedir: &Path, accept: impl Fn(&Path) -> bool) -> Self {
        FilesystemMatcher(Matcher::new(get_items_from_dir(basedir, accept)))
    }

    pub fn call(&self, query: &str, limit: Option<usize>) -> Vec<(String, Vec<i64>)> {
        self.0.call(query, limit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_a_simple_subsequence_with_correct_positions() {
        let m = Matcher::new(["hello world".to_string()]);
        let results = m.call("hw", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "hello world");
        assert_eq!(results[0].1, vec![0, 6], "'h' at index 0, 'w' at index 6");
    }

    #[test]
    fn no_match_is_excluded_from_results() {
        let m = Matcher::new(["hello".to_string(), "world".to_string()]);
        let results = m.call("xyz", None);
        assert!(results.is_empty());
    }

    #[test]
    fn word_boundary_matches_score_higher_than_mid_word_matches() {
        // "ab" appears mid-word in "xxabxx" but right after a level2
        // separator ('_') in "xx_abxx" -- the latter should score
        // higher and sort first.
        let m = Matcher::new(["xxabxx".to_string(), "xx_abxx".to_string()]);
        let results = m.call("ab", None);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "xx_abxx", "a match right after a level2 separator should outscore a mid-word match");
    }

    #[test]
    fn camel_case_boundary_scores_like_a_separator() {
        let m = Matcher::new(["xxabxx".to_string(), "xxAbxx".to_string()]);
        let results = m.call("ab", None);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "xxAbxx", "a lower-to-upper transition should score like a real word boundary");
    }

    #[test]
    fn matching_is_case_and_accent_insensitive() {
        let m = Matcher::new(["café".to_string()]);
        let results = m.call("CAFE", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, vec![0, 1, 2, 3]);
    }

    #[test]
    fn limit_is_applied_before_the_zero_score_filter() {
        // Matches upstream exactly: limit truncates the sorted (not
        // yet filtered) list, so if the best `limit` items by score
        // include some non-matches, fewer than `limit` results come
        // back.
        let m = Matcher::new(["ab".to_string(), "zzzzz".to_string()]);
        let results = m.call("ab", Some(1));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "ab");
    }

    #[test]
    fn empty_query_matches_nothing() {
        let m = Matcher::new(["hello".to_string()]);
        assert!(m.call("", None).is_empty());
    }

    #[test]
    fn empty_items_are_dropped_at_construction() {
        let m = Matcher::new(["".to_string(), "hello".to_string()]);
        // An empty haystack could never legitimately match anything
        // anyway, but confirm it doesn't even appear as a 0-length
        // false match for an empty query (which itself always yields
        // no results, see `empty_query_matches_nothing`).
        let results = m.call("h", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "hello");
    }

    #[test]
    fn non_bmp_characters_match_themselves_exactly() {
        // Real upstream's own `test_non_bmp` cross-check (matcher.py's
        // `test()`): an underscore, a non-BMP emoji, and a hyphen,
        // queried with the exact same string, should match every
        // character in order.
        let raw = "_\u{1f431}-";
        let m = Matcher::new([raw.to_string()]);
        let results = m.call(raw, None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].1, vec![0, 1, 2]);
    }

    #[test]
    fn results_are_sorted_by_descending_score() {
        // A match starting immediately (distance <= 1 from the start,
        // real upstream's own shortcut that skips the boundary-score
        // check entirely) beats one with a real gap before it, even
        // a separator-adjacent gap -- confirmed directly against
        // `word_boundary_matches_score_higher_than_mid_word_matches`'s
        // own case, which controls for this by putting BOTH matches
        // an equal distance from the start.
        let m = Matcher::new(["ab_zzzzzz".to_string(), "zzzzzz_ab".to_string()]);
        let results = m.call("ab", None);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "ab_zzzzzz", "an immediate match at the very start should sort first");
        assert_eq!(results[1].0, "zzzzzz_ab");
    }

    #[test]
    fn get_items_from_dir_walks_a_real_directory_recursively() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub/b.txt"), b"x").unwrap();

        let mut items = get_items_from_dir(dir.path(), |_| true);
        items.sort();
        assert_eq!(items, vec!["a.txt".to_string(), "sub/b.txt".to_string()]);
    }

    #[test]
    fn filesystem_matcher_finds_files_by_fuzzy_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img/one.gif"), b"x").unwrap();
        std::fs::write(dir.path().join("readme.txt"), b"x").unwrap();

        let m = FilesystemMatcher::new(dir.path());
        let results = m.call("one", None);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "img/one.gif");
    }
}
