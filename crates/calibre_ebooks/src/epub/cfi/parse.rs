//! EPUB CFI parsing, sorting and resolution.
//!
//! Port of `old_src/src/calibre/ebooks/epub/cfi/parse.py`.
//!
//! The Python is itself a hand-written parser — its docstring explains
//! that it follows `epubcfi.ebnf` (reproduced in [`super`]) but avoids
//! depending on grako. It leans on the third-party `regex` module for
//! two features the standard library lacks: character-class
//! subtraction, and repeated-group captures. Neither has a counterpart
//! in Rust's `regex` crate either, so this port scans characters
//! directly. The grammar is small enough that the scanner is shorter
//! than the patterns it replaces.
//!
//! Everything here is total: no input can panic, and an unparseable
//! CFI yields `None` plus the untouched input, exactly as the Python
//! returns its `null` tuple.

use indexmap::IndexMap;
use roxmltree::Node;

/// One `/N[id]` step of a CFI path, with any offset that terminates it.
///
/// Port of the step dicts the Python builds. An offset only ever
/// appears on the last step of a path, since matching one ends the
/// path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Step {
    /// The child index. CFI numbers element children 2, 4, 6, ...,
    /// leaving the odd numbers for text nodes.
    pub num: u32,
    /// An id assertion, `/4[body01]`. Takes precedence over `num` when
    /// resolving — see [`decode_cfi`].
    pub id: Option<String>,
    /// A character offset into the step's text, `:34`.
    pub text_offset: Option<i64>,
    /// A time offset in seconds, `~12.5`.
    pub temporal_offset: Option<f64>,
    /// An `(x, y)` position, `@23:34.1`.
    pub spatial_offset: Option<(f64, f64)>,
    /// The bracketed assertion after a text offset.
    pub text_assertion: Option<TextAssertion>,
}

/// The text surrounding a character offset, used to re-find a position
/// after the document has changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextAssertion {
    pub before: Option<String>,
    pub after: Option<String>,
    /// `;name=value,value` parameters, in the order written.
    pub params: IndexMap<String, Vec<String>>,
}

impl TextAssertion {
    fn is_empty(&self) -> bool {
        self.before.is_none() && self.after.is_none() && self.params.is_empty()
    }
}

/// A CFI path: a run of steps, optionally continuing into another
/// document through a `!` redirect.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path {
    pub steps: Vec<Step>,
    /// The path after a `!`, which crosses into the document the last
    /// step referenced.
    pub redirect: Option<Box<Path>>,
}

impl Path {
    /// Every step of this path and of anything it redirects into, in
    /// order.
    ///
    /// Port of the Python `get_steps`.
    pub fn all_steps(&self) -> Vec<&Step> {
        let mut ans: Vec<&Step> = self.steps.iter().collect();
        if let Some(redirect) = &self.redirect {
            ans.extend(redirect.all_steps());
        }
        ans
    }
}

/// A complete `epubcfi(...)` fragment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cfi {
    pub parent: Path,
    /// The start of a range, when the fragment has one.
    pub start: Option<Path>,
    /// The end of a range.
    pub end: Option<Path>,
}

// -- character classes -----------------------------------------------

/// The characters that must be escaped with `^`.
fn is_special(c: char) -> bool {
    matches!(c, '[' | ']' | '(' | ')' | ',' | ';' | '=' | '^')
}

/// What may follow a `^`. calibre also escaped hyphens historically, so
/// `^-` is accepted even though the specification does not list it.
fn is_escapable(c: char) -> bool {
    is_special(c) || c == '-'
}

/// A character that may appear unescaped.
///
/// The permitted set from the grammar, minus the special characters:
/// tab, newline, carriage return, and everything from space up, with
/// the surrogate and non-character ranges excluded.
fn is_unescaped(c: char, allow_space: bool) -> bool {
    if is_special(c) {
        return false;
    }
    if matches!(c, '\t' | '\n' | '\r') {
        return true;
    }
    let lowest = if allow_space { 0x20 } else { 0x21 };
    let v = c as u32;
    (lowest..=0xD7FF).contains(&v) || (0xE000..=0xFFFD).contains(&v) || v >= 0x10000
}

/// Consume a `characters` token.
///
/// Returns the unescaped value, the text exactly as written, and the
/// rest of the input. Both forms are needed because calibre unescapes
/// ids, assertion text and parameter values but leaves parameter
/// *names* as written — see [`parse_text_assertion`].
///
/// Yields `None` when no character matches, matching the `+`
/// quantifier on the Python patterns.
fn take_chars(raw: &str, allow_space: bool) -> Option<(String, &str, &str)> {
    let mut out = String::new();
    let mut rest = raw;
    loop {
        let mut it = rest.chars();
        let Some(c) = it.next() else { break };
        if c == '^' {
            // An escape only counts if something escapable follows;
            // otherwise the `^` is just a special char and stops the
            // token.
            match it.next() {
                Some(next) if is_escapable(next) => {
                    out.push(next);
                    rest = &rest[c.len_utf8() + next.len_utf8()..];
                }
                _ => break,
            }
        } else if is_unescaped(c, allow_space) {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        } else {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        let consumed = raw.len() - rest.len();
        Some((out, &raw[..consumed], rest))
    }
}

/// Consume an integer: `0`, or a non-zero digit followed by digits.
fn take_integer(raw: &str) -> Option<(i64, &str)> {
    let mut end = 0;
    for (i, c) in raw.char_indices() {
        if !c.is_ascii_digit() {
            break;
        }
        // A leading zero terminates the token immediately, so `0` is
        // valid but `01` parses as `0` with `1` left over.
        if i == 0 && c == '0' {
            end = 1;
            break;
        }
        end = i + 1;
    }
    if end == 0 {
        return None;
    }
    raw[..end].parse().ok().map(|v| (v, &raw[end..]))
}

/// Consume a number: an integer with an optional fractional part, or
/// `0.` followed by digits.
fn take_number(raw: &str) -> Option<(f64, &str)> {
    let bytes = raw.as_bytes();
    let mut i = 0;
    if bytes.first().is_some_and(|b| b.is_ascii_digit()) {
        if bytes[0] == b'0' {
            i = 1;
        } else {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
    } else {
        return None;
    }
    // A fractional part needs at least one digit after the point.
    if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
    }
    raw[..i].parse().ok().map(|v| (v, &raw[i..]))
}

// -- the parser ------------------------------------------------------

/// Parse a full `epubcfi(path[,start,end])` fragment.
///
/// Returns the fragment and whatever follows it. On any failure the
/// input is returned untouched, as the Python's `null` tuple does.
///
/// Port of the Python `parse_epubcfi`.
pub fn parse_epubcfi(raw: &str) -> (Option<Cfi>, &str) {
    let original = raw;
    let Some(rest) = raw.strip_prefix("epubcfi(") else {
        return (None, original);
    };
    let (Some(parent), rest) = parse_path(rest) else {
        return (None, original);
    };

    let (mut start, mut end) = (None, None);
    let mut rest = rest;
    if let Some(after_comma) = rest.strip_prefix(',') {
        let (s, r) = parse_path(after_comma);
        start = s;
        rest = r;
        if let Some(after_comma) = rest.strip_prefix(',') {
            let (e, r) = parse_path(after_comma);
            end = e;
            rest = r;
        }
        // A range needs both halves.
        if start.is_none() || end.is_none() {
            return (None, original);
        }
    }
    match rest.strip_prefix(')') {
        Some(rest) => (Some(Cfi { parent, start, end }), rest),
        None => (None, original),
    }
}

/// Parse the path component of a CFI, `/2/4[id]:3`.
///
/// Returns `None` when the input begins with no step at all.
///
/// Port of the Python `parse_path`.
pub fn parse_path(raw: &str) -> (Option<Path>, &str) {
    let mut path = Path::default();
    let rest = parse_path_into(raw, &mut path);
    if path.steps.is_empty() {
        (None, rest)
    } else {
        (Some(path), rest)
    }
}

/// Port of the Python `_parse_path`, which recurses per step.
fn parse_path_into<'a>(raw: &'a str, ans: &mut Path) -> &'a str {
    let mut raw = raw;
    loop {
        let Some(rest) = raw.strip_prefix('/') else {
            return raw;
        };
        let Some((num, rest)) = take_integer(rest) else {
            return raw;
        };
        let mut step = Step {
            num: num.max(0) as u32,
            ..Default::default()
        };
        let mut rest = rest;

        // An optional id assertion.
        if let Some(after) = rest.strip_prefix('[') {
            if let Some((id, _, r)) = take_chars(after, true) {
                if let Some(r) = r.strip_prefix(']') {
                    step.id = Some(id);
                    rest = r;
                }
            }
        }

        if let Some(after_bang) = rest.strip_prefix('!') {
            ans.steps.push(step);
            let mut redirect = Path::default();
            let remaining = parse_path_into(after_bang, &mut redirect);
            ans.redirect = Some(Box::new(redirect));
            return remaining;
        }

        // An offset, if there is one, ends the path.
        match parse_offset(rest, &mut step) {
            Some(remaining) => {
                ans.steps.push(step);
                return remaining;
            }
            None => {
                ans.steps.push(step);
                raw = rest;
            }
        }
    }
}

/// Parse whichever offset follows a step, if any.
///
/// Port of the Python `parse_offset`, including the order it tries the
/// forms in: a text offset, then spatio-temporal, then temporal, then
/// spatial.
fn parse_offset<'a>(raw: &'a str, step: &mut Step) -> Option<&'a str> {
    if let Some(rest) = raw.strip_prefix(':') {
        if let Some((offset, rest)) = take_integer(rest) {
            step.text_offset = Some(offset);
            return Some(parse_text_assertion(rest, step));
        }
    }
    if let Some(rest) = raw.strip_prefix('~') {
        if let Some((t, rest)) = take_number(rest) {
            // `~t@x:y` is one offset, not two.
            if let Some(after_at) = rest.strip_prefix('@') {
                if let Some((x, r)) = take_number(after_at) {
                    if let Some(r) = r.strip_prefix(':') {
                        if let Some((y, r)) = take_number(r) {
                            step.temporal_offset = Some(t);
                            step.spatial_offset = Some((x, y));
                            return Some(r);
                        }
                    }
                }
            }
            step.temporal_offset = Some(t);
            return Some(rest);
        }
    }
    if let Some(rest) = raw.strip_prefix('@') {
        if let Some((x, rest)) = take_number(rest) {
            if let Some(rest) = rest.strip_prefix(':') {
                if let Some((y, rest)) = take_number(rest) {
                    step.spatial_offset = Some((x, y));
                    return Some(rest);
                }
            }
        }
    }
    None
}

/// Parse the `[before,after;name=value]` assertion after a text offset.
///
/// A malformed assertion consumes nothing — the original input is
/// returned and the step keeps no assertion.
///
/// Port of the Python `parse_text_assertion`.
fn parse_text_assertion<'a>(raw: &'a str, step: &mut Step) -> &'a str {
    let Some(rest) = raw.strip_prefix('[') else {
        return raw;
    };
    let mut ta = TextAssertion::default();
    let mut rest = rest;

    if let Some((before, _, r)) = take_chars(rest, true) {
        ta.before = Some(before);
        rest = r;
        if let Some(after_comma) = rest.strip_prefix(',') {
            if let Some((after, _, r)) = take_chars(after_comma, true) {
                ta.after = Some(after);
                rest = r;
            }
        }
    } else if let Some(after_comma) = rest.strip_prefix(',') {
        if let Some((after, _, r)) = take_chars(after_comma, true) {
            ta.after = Some(after);
            rest = r;
        }
    }

    // `;name=value,value` parameters, repeated.
    loop {
        let Some(after_semi) = rest.strip_prefix(';') else {
            break;
        };
        // calibre stores the parameter name exactly as written — only
        // the values go through unescape — so `;^-=x` yields the name
        // `^-`, carets and all.
        let Some((_, name_raw, r)) = take_chars(after_semi, false) else {
            break;
        };
        let Some(r) = r.strip_prefix('=') else { break };
        let mut values = Vec::new();
        let mut vr = r;
        loop {
            let Some((value, _, next)) = take_chars(vr, true) else {
                break;
            };
            values.push(value);
            vr = next;
            match vr.strip_prefix(',') {
                Some(next) => vr = next,
                None => break,
            }
        }
        if values.is_empty() {
            break;
        }
        ta.params.insert(name_raw.to_string(), values);
        rest = vr;
    }

    // Without a closing bracket the assertion is not an assertion, and
    // nothing at all is consumed.
    let Some(rest) = rest.strip_prefix(']') else {
        return raw;
    };
    if !ta.is_empty() {
        step.text_assertion = Some(ta);
    }
    rest
}

// -- sorting ---------------------------------------------------------

/// The offsets a CFI sort key compares, after its step numbers.
///
/// The spatial pair is stored `(y, x)` — reversed from how it is
/// written — because that is the order the Python's
/// `tuple(reversed(...))` produces, and vertical position dominates.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Offsets {
    pub temporal: f64,
    pub spatial_y: f64,
    pub spatial_x: f64,
    pub text: i64,
}

/// A CFI's position in reading order.
///
/// Port of what the Python `cfi_sort_key` returns. Comparison is by
/// step numbers first, then offsets — which is why this type exists
/// rather than a bare tuple: the offsets are floats, and `f64` is not
/// `Ord`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CfiSortKey {
    pub step_nums: Vec<u32>,
    pub offsets: Offsets,
}

impl Eq for CfiSortKey {}

impl PartialOrd for CfiSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CfiSortKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.step_nums
            .cmp(&other.step_nums)
            .then_with(|| self.offsets.temporal.total_cmp(&other.offsets.temporal))
            .then_with(|| self.offsets.spatial_y.total_cmp(&other.offsets.spatial_y))
            .then_with(|| self.offsets.spatial_x.total_cmp(&other.offsets.spatial_x))
            .then_with(|| self.offsets.text.cmp(&other.offsets.text))
    }
}

/// The sort key for a CFI.
///
/// With `only_path` the input is a bare path; otherwise it is a full
/// `epubcfi(...)` fragment, and a range sorts by its start.
///
/// An unparseable CFI sorts before everything, as in the Python, which
/// returns an empty step tuple and zero offsets.
///
/// Port of the Python `cfi_sort_key`.
pub fn cfi_sort_key(cfi: &str, only_path: bool) -> CfiSortKey {
    let path = if only_path {
        parse_path(cfi).0
    } else {
        parse_epubcfi(cfi).0.map(|c| c.start.unwrap_or(c.parent))
    };
    let Some(path) = path else {
        return CfiSortKey::default();
    };

    let steps = path.all_steps();
    let step_nums = steps.iter().map(|s| s.num).collect();
    let last = steps.last();
    let offsets = match last {
        Some(step) => {
            let (x, y) = step.spatial_offset.unwrap_or((0.0, 0.0));
            Offsets {
                temporal: step.temporal_offset.unwrap_or(0.0),
                spatial_y: y,
                spatial_x: x,
                text: step.text_offset.unwrap_or(0),
            }
        }
        None => Offsets::default(),
    };
    CfiSortKey { step_nums, offsets }
}

// -- resolution ------------------------------------------------------

/// Resolve a CFI path against a parsed document, returning the element
/// it points at.
///
/// Port of the Python `decode_cfi`. Two things are worth knowing:
///
/// - An id assertion wins over the step number. `/2[body01]` finds the
///   element with that id even though step 2 would be the first child.
/// - CFI numbers element children 2, 4, 6, ..., reserving odd numbers
///   for text nodes, so an odd step number matches no element and the
///   resolution fails unless an id saves it.
///
/// One deviation: calibre interpolates the id into an XPath expression
/// even when the step has no id, so it searches for `@id="None"` and
/// would match an element actually carrying that id. This port skips
/// the search instead.
pub fn decode_cfi<'a, 'i>(root: Node<'a, 'i>, cfi: &str) -> Option<Node<'a, 'i>> {
    let path = parse_path(cfi).0?;
    let mut ans = root;
    for step in path.all_steps() {
        if let Some(id) = &step.id {
            if let Some(found) = ans
                .descendants()
                .skip(1)
                .find(|n| n.is_element() && n.attribute("id") == Some(id.as_str()))
            {
                ans = found;
                continue;
            }
        }
        let mut index = 0u32;
        let mut matched = None;
        for child in ans.children().filter(|c| c.is_element()) {
            // Advance to the next even number: odd indices belong to
            // text nodes.
            index |= 1;
            index += 1;
            if index == step.num {
                matched = Some(child);
                break;
            }
        }
        ans = matched?;
    }
    Some(ans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a path of plain numbered steps.
    fn s(nums: &[u32]) -> Path {
        Path {
            steps: nums
                .iter()
                .map(|n| Step {
                    num: *n,
                    ..Default::default()
                })
                .collect(),
            redirect: None,
        }
    }

    fn step_with_id(num: u32, id: &str) -> Step {
        Step {
            num,
            id: Some(id.to_string()),
            ..Default::default()
        }
    }

    // The cases below are calibre's own, ported from
    // old_src/src/calibre/ebooks/epub/cfi/tests.py.

    #[test]
    fn parses_steps() {
        assert_eq!(parse_path("/2"), (Some(s(&[2])), ""));
        assert_eq!(parse_path("/2/3/4"), (Some(s(&[2, 3, 4])), ""));

        let expected = Path {
            steps: vec![
                Step {
                    num: 1,
                    ..Default::default()
                },
                step_with_id(2, "some,^id"),
                Step {
                    num: 3,
                    ..Default::default()
                },
            ],
            redirect: None,
        };
        assert_eq!(parse_path("/1/2[some^,^^id]/3"), (Some(expected), ""));
    }

    #[test]
    fn parses_redirects() {
        let mut expected = s(&[1, 2]);
        expected.redirect = Some(Box::new(s(&[3, 4])));
        assert_eq!(parse_path("/1/2!/3/4"), (Some(expected), ""));

        let mut expected = Path {
            steps: vec![
                Step {
                    num: 1,
                    ..Default::default()
                },
                step_with_id(2, "id"),
            ],
            redirect: None,
        };
        expected.redirect = Some(Box::new(s(&[3, 4])));
        assert_eq!(parse_path("/1/2[id]!/3/4"), (Some(expected), ""));

        let mut expected = s(&[1]);
        expected.redirect = Some(Box::new(Path {
            steps: vec![
                step_with_id(2, "id"),
                Step {
                    num: 3,
                    ..Default::default()
                },
                Step {
                    num: 4,
                    ..Default::default()
                },
            ],
            redirect: None,
        }));
        assert_eq!(parse_path("/1!/2[id]/3/4"), (Some(expected), ""));
    }

    /// A path of one step carrying the given offsets.
    fn offset_path(f: impl Fn(&mut Step)) -> Path {
        let mut step = Step {
            num: 1,
            ..Default::default()
        };
        f(&mut step);
        Path {
            steps: vec![step],
            redirect: None,
        }
    }

    #[test]
    fn parses_offsets() {
        for (raw, temporal) in [
            ("/1~0", 0.0),
            ("/1~7", 7.0),
            ("/1~43.1", 43.1),
            ("/1~0.01", 0.01),
            ("/1~1.301", 1.301),
        ] {
            assert_eq!(
                parse_path(raw),
                (
                    Some(offset_path(|s| s.temporal_offset = Some(temporal))),
                    ""
                ),
                "parsing {raw}"
            );
        }

        for raw in ["/1@23:34.1", "/1@23:34.10"] {
            assert_eq!(
                parse_path(raw),
                (
                    Some(offset_path(|s| s.spatial_offset = Some((23.0, 34.1)))),
                    ""
                ),
                "parsing {raw}"
            );
        }

        assert_eq!(
            parse_path("/1~3@3.1:2.3"),
            (
                Some(offset_path(|s| {
                    s.temporal_offset = Some(3.0);
                    s.spatial_offset = Some((3.1, 2.3));
                })),
                ""
            )
        );

        for (raw, text) in [("/1:0", 0), ("/1:3", 3)] {
            assert_eq!(
                parse_path(raw),
                (Some(offset_path(|s| s.text_offset = Some(text))), ""),
                "parsing {raw}"
            );
        }
    }

    /// A `/1:3` path carrying a text assertion.
    fn assertion_path(
        before: Option<&str>,
        after: Option<&str>,
        params: &[(&str, &[&str])],
    ) -> Path {
        offset_path(|s| {
            s.text_offset = Some(3);
            let mut ta = TextAssertion {
                before: before.map(str::to_string),
                after: after.map(str::to_string),
                params: IndexMap::new(),
            };
            for (name, values) in params {
                ta.params.insert(
                    (*name).to_string(),
                    values.iter().map(|v| (*v).to_string()).collect(),
                );
            }
            if !ta.is_empty() {
                s.text_assertion = Some(ta);
            }
        })
    }

    #[test]
    fn parses_text_assertions() {
        for (raw, expected) in [
            ("/1:3[aa^,b]", assertion_path(Some("aa,b"), None, &[])),
            // A bare hyphen needs no escape, but calibre used to write
            // one, so both spellings must parse to the same thing.
            ("/1:3[aa-b]", assertion_path(Some("aa-b"), None, &[])),
            ("/1:3[aa^-b]", assertion_path(Some("aa-b"), None, &[])),
            ("/1:3[aa-^--b]", assertion_path(Some("aa---b"), None, &[])),
            (
                "/1:3[aa^,b,c1]",
                assertion_path(Some("aa,b"), Some("c1"), &[]),
            ),
            ("/1:3[,aa^,b]", assertion_path(None, Some("aa,b"), &[])),
            ("/1:3[;s=a]", assertion_path(None, None, &[("s", &["a"])])),
            (
                "/1:3[a;s=a]",
                assertion_path(Some("a"), None, &[("s", &["a"])]),
            ),
            (
                "/1:3[a;s=a^,b,c^;d;x=y]",
                assertion_path(Some("a"), None, &[("s", &["a,b", "c;d"]), ("x", &["y"])]),
            ),
        ] {
            assert_eq!(parse_path(raw), (Some(expected), ""), "parsing {raw}");
        }
    }

    #[test]
    fn parameter_names_keep_their_escapes_but_values_do_not() {
        // calibre unescapes ids, assertion text and parameter values,
        // but stores the parameter name exactly as written. Reproduced;
        // found by the differential corpus in tests/epub_cfi_test.rs.
        let path = parse_path("/1:3[;^-=a^,b]").0.expect("parses");
        let ta = path.steps[0].text_assertion.as_ref().expect("an assertion");
        assert_eq!(ta.params.keys().collect::<Vec<_>>(), vec!["^-"]);
        assert_eq!(ta.params["^-"], vec!["a,b"]);
    }

    #[test]
    fn sort_keys_order_by_steps_then_offsets() {
        let key = |nums: &[u32], t: f64, y: f64, x: f64, text: i64| CfiSortKey {
            step_nums: nums.to_vec(),
            offsets: Offsets {
                temporal: t,
                spatial_y: y,
                spatial_x: x,
                text,
            },
        };
        assert_eq!(
            cfi_sort_key("/1/2/3", true),
            key(&[1, 2, 3], 0.0, 0.0, 0.0, 0)
        );
        assert_eq!(
            cfi_sort_key("/1[id]:34[yyyy]", true),
            key(&[1], 0.0, 0.0, 0.0, 34)
        );
        // The spatial pair sorts y before x.
        assert_eq!(cfi_sort_key("/1@1:2", true), key(&[1], 0.0, 2.0, 1.0, 0));
        assert_eq!(cfi_sort_key("/1~1.2", true), key(&[1], 1.2, 0.0, 0.0, 0));
    }

    #[test]
    fn sort_keys_actually_sort() {
        let mut cfis = vec!["/2/4", "/2/2", "/2/4:10", "/2/4:2", "/1", "/2/4~1.5"];
        cfis.sort_by_key(|c| cfi_sort_key(c, true));
        assert_eq!(
            cfis,
            vec!["/1", "/2/2", "/2/4", "/2/4:2", "/2/4:10", "/2/4~1.5"]
        );
    }

    #[test]
    fn an_unparseable_cfi_sorts_first() {
        let null = CfiSortKey::default();
        assert_eq!(cfi_sort_key("", true), null);
        assert_eq!(cfi_sort_key("nonsense", true), null);
        assert_eq!(cfi_sort_key("2/4", true), null, "a path must start with /");
        assert!(cfi_sort_key("junk", true) < cfi_sort_key("/1", true));
    }

    #[test]
    fn full_fragments_sort_by_their_start() {
        let ranged = cfi_sort_key("epubcfi(/2/4,/2:1,/4:5)", false);
        assert_eq!(ranged.step_nums, vec![2]);
        assert_eq!(ranged.offsets.text, 1, "the start path, not the parent");

        let plain = cfi_sort_key("epubcfi(/2/4/6)", false);
        assert_eq!(plain.step_nums, vec![2, 4, 6]);
    }

    #[test]
    fn parses_whole_fragments() {
        let (cfi, rest) = parse_epubcfi("epubcfi(/2/4)");
        assert_eq!(rest, "");
        let cfi = cfi.expect("parses");
        assert_eq!(cfi.parent, s(&[2, 4]));
        assert!(cfi.start.is_none() && cfi.end.is_none());

        let (cfi, rest) = parse_epubcfi("epubcfi(/2,/4:1,/6:9)trailing");
        assert_eq!(rest, "trailing");
        let cfi = cfi.expect("parses");
        assert_eq!(cfi.parent, s(&[2]));
        assert_eq!(cfi.start.unwrap().steps[0].text_offset, Some(1));
        assert_eq!(cfi.end.unwrap().steps[0].text_offset, Some(9));
    }

    #[test]
    fn malformed_fragments_consume_nothing() {
        for raw in [
            "",
            "nonsense",
            "epubcfi(",
            "epubcfi()",
            "epubcfi(/2",
            // A range needs both halves.
            "epubcfi(/2,/4:1)",
            "epubcfi(/2,,)",
        ] {
            let (cfi, rest) = parse_epubcfi(raw);
            assert!(cfi.is_none(), "{raw:?} should not parse");
            assert_eq!(rest, raw, "{raw:?} should consume nothing");
        }
    }

    #[test]
    fn leading_zeros_are_not_part_of_a_number() {
        // The grammar forbids them, so `/01` parses as step 0 with `1`
        // left over rather than as step 1.
        let (path, rest) = parse_path("/01");
        assert_eq!(path.unwrap().steps[0].num, 0);
        assert_eq!(rest, "1");
    }

    #[test]
    fn an_unterminated_assertion_is_not_consumed() {
        // No closing bracket: the offset stands, the assertion does not.
        let (path, rest) = parse_path("/1:3[unterminated");
        let path = path.expect("the path still parses");
        assert_eq!(path.steps[0].text_offset, Some(3));
        assert!(path.steps[0].text_assertion.is_none());
        assert_eq!(rest, "[unterminated");
    }

    #[test]
    fn an_id_assertion_without_a_closing_bracket_is_ignored() {
        let (path, rest) = parse_path("/2[unterminated");
        let path = path.expect("the path still parses");
        assert_eq!(path.steps[0].num, 2);
        assert!(path.steps[0].id.is_none());
        assert_eq!(rest, "[unterminated");
    }

    #[test]
    fn a_caret_without_an_escapable_character_ends_the_token() {
        // `^z` is not an escape, so the id assertion never closes and
        // is discarded.
        let (path, _) = parse_path("/2[a^zb]");
        assert!(path.unwrap().steps[0].id.is_none());
    }

    #[test]
    fn all_steps_walks_through_redirects() {
        let path = parse_path("/1/2!/3/4").0.unwrap();
        let nums: Vec<u32> = path.all_steps().iter().map(|s| s.num).collect();
        assert_eq!(nums, vec![1, 2, 3, 4]);
    }

    // -- decode_cfi, calibre's own test document ---------------------

    const DOC: &str = r#"<html>
<head></head>
<body id="body01">
        <p>a</p>
        <p>b</p>
        <p>c</p>
        <p>d</p>
        <p id="para05">xxx<em>yyy</em>0123456789</p>
        <p>e</p>
        <p>f</p>
        <img id="svgimg" src="foo.svg" alt="g"/>
        <p>h</p>
        <p><span>hello</span><span>goodbye</span>text here<em>adieu</em>text there</p>
    </body>
</html>"#;

    fn doc() -> roxmltree::Document<'static> {
        roxmltree::Document::parse(DOC).expect("the test document parses")
    }

    #[test]
    fn decode_resolves_to_the_body_by_number_or_id() {
        let doc = doc();
        let root = doc.root_element();
        let body = root
            .children()
            .filter(|c| c.is_element())
            .next_back()
            .expect("a body");

        // The number, the number plus a matching id, a wrong number
        // rescued by the id, and a number that would resolve elsewhere
        // but is overridden by the id.
        for cfi in ["/4", "/4[body01]", "/900[body01]", "/2[body01]"] {
            assert_eq!(decode_cfi(root, cfi), Some(body), "resolving {cfi}");
        }
    }

    #[test]
    fn decode_numbers_element_children_by_twos() {
        let doc = doc();
        let root = doc.root_element();
        let body = root
            .children()
            .filter(|c| c.is_element())
            .next_back()
            .unwrap();
        let children: Vec<_> = body.children().filter(|c| c.is_element()).collect();
        assert_eq!(children.len(), 10);
        for (i, child) in children.iter().enumerate() {
            let cfi = format!("/4/{}", (i + 1) * 2);
            assert_eq!(decode_cfi(root, &cfi), Some(*child), "resolving {cfi}");
        }
    }

    #[test]
    fn decode_follows_an_id_then_keeps_counting() {
        let doc = doc();
        let root = doc.root_element();
        let body = root
            .children()
            .filter(|c| c.is_element())
            .next_back()
            .unwrap();
        let p = body.children().filter(|c| c.is_element()).nth(4).unwrap();
        assert_eq!(p.attribute("id"), Some("para05"));

        assert_eq!(decode_cfi(root, "/4/999[para05]"), Some(p));
        // And the step after the id assertion resolves normally.
        let em = p.children().find(|c| c.is_element()).unwrap();
        assert_eq!(decode_cfi(root, "/4/999[para05]/2"), Some(em));
    }

    #[test]
    fn decode_gives_up_rather_than_guessing() {
        let doc = doc();
        let root = doc.root_element();
        // An odd step number addresses a text node, never an element.
        assert_eq!(decode_cfi(root, "/3"), None);
        // Past the end of the children.
        assert_eq!(decode_cfi(root, "/400"), None);
        // An id that is not in the document falls back to the number,
        // which here is out of range.
        assert_eq!(decode_cfi(root, "/400[nosuchid]"), None);
        // Not a path at all.
        assert_eq!(decode_cfi(root, ""), None);
        assert_eq!(decode_cfi(root, "rubbish"), None);
    }

    #[test]
    fn decode_ignores_offsets_which_address_text_not_elements() {
        let doc = doc();
        let root = doc.root_element();
        let body = root
            .children()
            .filter(|c| c.is_element())
            .next_back()
            .unwrap();
        let p = body.children().filter(|c| c.is_element()).nth(4).unwrap();
        // The character offset picks a position inside the element; the
        // element it resolves to is the same either way.
        assert_eq!(decode_cfi(root, "/4/10:3"), Some(p));
        assert_eq!(decode_cfi(root, "/4/10:3[xxx]"), Some(p));
    }
}
