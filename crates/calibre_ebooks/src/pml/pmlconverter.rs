//! PML markup -> HTML.
//!
//! Port of `old_src/src/calibre/ebooks/pml/pmlconverter.py`'s
//! `PML_HTMLizer`: a small state machine that walks PML text one line
//! and one character at a time, tracking which inline/block "codes"
//! (`\b`, `\i`, `\x`, ...) are currently open and emitting the
//! corresponding HTML.
//!
//! # Preserved upstream quirks
//!
//! A few things below look like bugs and are ported as bugs, because
//! this needs to match calibre's actual observable output, not a
//! "corrected" reading of it:
//!
//! - `\Sp`/`\Sb` (superscript/subscript) are defined in `CODE_STATES`
//!   and `STATES_TAGS` but [`parse_pml`](PmlHtmlizer::parse_pml)'s
//!   character dispatcher never routes to them -- only `Fn`, `FN`,
//!   `SB` and `Sd` are recognized after a leading `F`/`S`. So `\Sp`/
//!   `\Sb` are silently swallowed with no effect, matching the Python.
//! - [`cleanup_html_remove_redundant`](PmlHtmlizer::cleanup_html_remove_redundant)'s
//!   redundant-empty-tag removal is built from `STATES_TAGS` templates
//!   that still contain a literal `%s` for `ra`'s close tag and for
//!   `FN`/`SB`'s close tags (the Python only substitutes the *open*
//!   template's placeholder, via `open % '.*?'`, and never touches
//!   `close`). The resulting regex looks for a literal `%s` in the
//!   generated HTML, which never occurs, so empty `<ra>`/`<FN>`/`<SB>`
//!   pairs are never actually cleaned up. Ported as-is.
//!
//! A handful of the Python's `if x in Y:` branches inside
//! `process_code_div`/`process_code_span`/`process_code_block` are
//! unreachable for every actual caller (e.g. `SPAN_STATES` and
//! `STATES_VALUE_REQ`/`STATES_VALUE_REQ_2` are disjoint sets, so
//! "reopen a span with its value" never fires). Those are *not*
//! preserved as dead branches here -- they are collapsed to the one
//! branch that is actually reachable, with a comment at each site
//! explaining why the collapse is behavior-preserving.

use std::collections::HashMap;

use regex::Regex;

use crate::metadata::toc::{TOCNode, TOC};
use crate::xml_util::prepare_string_for_xml;

// -- the STATES tables (ported verbatim from `pmlconverter.py`) --------

/// Port of `PML_HTMLizer.STATES`. `'s'` is listed by the Python but
/// never assigned to by any `CODE_STATES` entry or dispatch branch --
/// dead, but kept here (as an always-closed, never-rendered state) for
/// fidelity with `self.state`'s key set.
const STATES: &[&str] = &[
    "i", "u", "d", "b", "sp", "sb", "h1", "h1c", "h2", "h3", "h4", "h5", "h6", "a", "ra", "c", "r",
    "s", "l", "k", "FN", "SB",
];

const STATES_VALUE_REQ: &[&str] = &["a", "FN", "SB"];
const STATES_VALUE_REQ_2: &[&str] = &["ra"];
const STATES_CLOSE_VALUE_REQ: &[&str] = &["FN", "SB"];
const LINK_STATES: &[&str] = &["a", "ra"];
const BLOCK_STATES: &[&str] = &["a", "ra", "h1", "h2", "h3", "h4", "h5", "h6", "sb", "sp"];
const DIV_STATES: &[&str] = &["c", "r", "FN", "SB"];
const SPAN_STATES: &[&str] = &["l", "k", "i", "u", "d", "b"];

/// The `STATES_TAGS` dict's insertion order, needed for
/// [`PmlHtmlizer::cleanup_html_remove_redundant`], which relies on it.
const STATES_TAGS_ORDER: &[&str] = &[
    "h1", "h1c", "h2", "h3", "h4", "h5", "h6", "sp", "sb", "a", "ra", "c", "r", "t", "T", "i", "u",
    "d", "b", "l", "k", "FN", "SB",
];

/// Port of `STATES_TAGS`. `%s` marks a `str %`-style substitution
/// placeholder, exactly as in the Python (including the ones the
/// Python's own code never fills in -- see the module docs).
fn states_tags(key: &str) -> (&'static str, &'static str) {
    match key {
        "h1" => ("<h1 style=\"page-break-before: always;\">", "</h1>"),
        "h1c" => ("<h1>", "</h1>"),
        "h2" => ("<h2>", "</h2>"),
        "h3" => ("<h3>", "</h3>"),
        "h4" => ("<h4>", "</h4>"),
        "h5" => ("<h5>", "</h5>"),
        "h6" => ("<h6>", "</h6>"),
        "sp" => ("<sup>", "</sup>"),
        "sb" => ("<sub>", "</sub>"),
        "a" => ("<a href=\"#%s\">", "</a>"),
        "ra" => ("<span id=\"r%s\"></span><a href=\"#%s\">", "</a>"),
        "c" => (
            "<div style=\"text-align: center; margin: auto;\">",
            "</div>",
        ),
        "r" => ("<div style=\"text-align: right;\">", "</div>"),
        "t" => ("<div style=\"margin-left: 5%;\">", "</div>"),
        "T" => ("<div style=\"text-indent: %s;\">", "</div>"),
        "i" => ("<span style=\"font-style: italic;\">", "</span>"),
        "u" => ("<span style=\"text-decoration: underline;\">", "</span>"),
        "d" => ("<span style=\"text-decoration: line-through;\">", "</span>"),
        "b" => ("<span style=\"font-weight: bold;\">", "</span>"),
        "l" => ("<span style=\"font-size: 150%;\">", "</span>"),
        "k" => (
            "<span style=\"font-size: 75%; font-variant: small-caps;\">",
            "</span>",
        ),
        "FN" => (
            "<br /><br style=\"page-break-after: always;\" /><div id=\"fn-%s\"><p>",
            "</p><small><a href=\"#rfn-%s\">return</a></small></div>",
        ),
        "SB" => (
            "<br /><br style=\"page-break-after: always;\" /><div id=\"sb-%s\"><p>",
            "</p><small><a href=\"#rsb-%s\">return</a></small></div>",
        ),
        other => unreachable!("unknown PML state tag key: {other}"),
    }
}

/// Port of `CODE_STATES`: PML escape code -> `self.state` key.
fn code_states(code: &str) -> Option<&'static str> {
    Some(match code {
        "q" => "a",
        "x" => "h1",
        "X0" => "h2",
        "X1" => "h3",
        "X2" => "h4",
        "X3" => "h5",
        "X4" => "h6",
        "Sp" => "sp",
        "Sb" => "sb",
        "c" => "c",
        "r" => "r",
        "i" => "i",
        "I" => "i",
        "u" => "u",
        "o" => "d",
        "b" => "b",
        "B" => "b",
        "l" => "l",
        "k" => "k",
        "Fn" => "ra",
        "Sd" => "ra",
        "FN" => "FN",
        "SB" => "SB",
        _ => return None,
    })
}

fn new_line_exchange(key: &str) -> Option<&'static str> {
    match key {
        "h1" => Some("h1c"),
        _ => None,
    }
}

/// Fill the first `%s` placeholder in a `STATES_TAGS` template.
fn fill1(template: &str, val: &str) -> String {
    template.replacen("%s", val, 1)
}

/// Fill both `%s` placeholders in a `STATES_TAGS` template with the
/// same value, port of Python's `template % (val, val)`.
fn fill2(template: &str, val: &str) -> String {
    template.replacen("%s", val, 2)
}

// -- a StringIO-alike, since `code_value` needs seek/tell -------------

/// A character-addressed cursor over one line of PML, standing in for
/// Python's `io.StringIO(line)`. Character-addressed (not byte-
/// addressed) to match Python string indexing, which is what
/// `code_value`'s `stream.seek(loc)` backtrack relies on.
struct CharStream {
    chars: Vec<char>,
    pos: usize,
}

impl CharStream {
    fn new(line: &str) -> Self {
        CharStream {
            chars: line.chars().collect(),
            pos: 0,
        }
    }

    fn read1(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn tell(&self) -> usize {
        self.pos
    }

    fn seek(&mut self, pos: usize) {
        self.pos = pos;
    }
}

/// Port of `code_value`: reads a `="value"` sequence off `stream`,
/// starting right after a code letter. Backtracks (leaves `stream`'s
/// position unchanged and returns `""`) if the sequence is malformed --
/// e.g. missing `=`, missing opening or closing `"`.
fn code_value(stream: &mut CharStream) -> String {
    let mut value = String::new();
    // 0: before `=`. 1: before the first `"`. 2: before the second `"`.
    // 3: after the second `"`.
    let mut state = 0u8;
    let loc = stream.tell();

    while let Some(c) = stream.read1() {
        match state {
            0 => {
                if c == '=' {
                    state = 1;
                } else if c != ' ' {
                    break;
                }
            }
            1 => {
                if c == '"' {
                    state = 2;
                } else if c != ' ' {
                    break;
                }
            }
            2 => {
                if c == '"' {
                    state = 3;
                    break;
                } else {
                    value.push(c);
                }
            }
            _ => unreachable!(),
        }
    }

    if state != 3 {
        stream.seek(loc);
        value.clear();
    }
    value.trim().to_string()
}

/// A pragmatic port of Python's `str.splitlines()`: splits on `\r\n`,
/// `\r` and `\n` (not producing a trailing empty entry for a string
/// ending in a line break). Python's version also splits on `\v`,
/// `\f`, `\x1c`-`\x1e`, `\x85`, `\u2028` and `\u2029`; those do not
/// occur in real PML input, so they are not replicated.
fn splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let bytes = s.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines.push(&s[start..i]);
                i += 1;
                start = i;
            }
            b'\r' => {
                lines.push(&s[start..i]);
                i += 1;
                if i < bytes.len() && bytes[i] == b'\n' {
                    i += 1;
                }
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

fn join_href(href: &str, frag: &str) -> String {
    if frag.is_empty() {
        href.to_string()
    } else {
        format!("{href}#{frag}")
    }
}

// -- prepare_pml / strip_pml -------------------------------------------

/// Wrap `\x...\x` or `\X[0-4]...\X[0-4]` chapter markers as
/// `\x="stripped text"original text\x` (used later by `parse_pml` to
/// register a TOC entry), pairing each opening marker with the *next*
/// occurrence of the identical marker text.
///
/// Port of the `(?P<c>\\x)(?P<text>.*?)(?P=c)`/
/// `(?P<c>\\X[0-4])(?P<text>.*?)(?P=c)` substitutions in `prepare_pml`.
/// Rust's `regex` crate has no backreference support (`(?P=c)`), so
/// this walks the string by hand instead of using a single regex; the
/// pairing behavior it implements (lazy match, so consecutive markers
/// pair up 1st-with-2nd, 3rd-with-4th, ...; an unmatched trailing
/// marker is left untouched) was verified by tracing Python's
/// leftmost-lazy-match semantics by hand.
fn wrap_markers(pml: &str, find_marker: impl Fn(&str) -> Option<(usize, usize)>) -> String {
    let mut out = String::new();
    let mut rest = pml;
    loop {
        match find_marker(rest) {
            None => {
                out.push_str(rest);
                break;
            }
            Some((start, marker_len)) => {
                let marker = &rest[start..start + marker_len];
                let after = &rest[start + marker_len..];
                match after.find(marker) {
                    None => {
                        // No closing marker anywhere ahead: this
                        // opening marker is left untouched, and
                        // scanning resumes right after it.
                        out.push_str(&rest[..start + marker_len]);
                        rest = after;
                    }
                    Some(rel_end) => {
                        let text = &after[..rel_end];
                        out.push_str(&rest[..start]);
                        let stripped = strip_pml(text);
                        out.push_str(marker);
                        out.push_str("=\"");
                        out.push_str(&stripped);
                        out.push('"');
                        out.push_str(text);
                        out.push_str(marker);
                        rest = &after[rel_end + marker.len()..];
                    }
                }
            }
        }
    }
    out
}

fn wrap_x_markers(pml: &str) -> String {
    wrap_markers(pml, |s| s.find("\\x").map(|pos| (pos, 2)))
}

fn wrap_capital_x_markers(pml: &str) -> String {
    wrap_markers(pml, |s| {
        let mut idx = 0;
        while let Some(rel) = s[idx..].find("\\X") {
            let pos = idx + rel;
            if let Some(d) = s[pos + 2..].chars().next() {
                if ('0'..='4').contains(&d) {
                    return Some((pos, 2 + d.len_utf8()));
                }
            }
            idx = pos + 2;
        }
        None
    })
}

/// Port of `PML_HTMLizer.prepare_pml`.
pub fn prepare_pml(pml: &str) -> String {
    let mut pml = wrap_x_markers(pml);
    pml = wrap_capital_x_markers(&pml);

    static COMMENT: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let comment = COMMENT.get_or_init(|| Regex::new(r"(?s)\\v.*?\\v").unwrap());
    pml = comment.replace_all(&pml, "").into_owned();

    // Collapse runs of 2+ spaces to one, then trim leading/trailing
    // spaces per line, then (redundantly, since the trim already
    // empties them) blank out whitespace-only lines. Python spells
    // this as four separate regexes, two of which use a lookahead/
    // lookbehind Rust's `regex` crate cannot express
    // (`^[ ]*(?=.)`/`(?<=.)[ ]*$`). Tracing their backtracking shows
    // the lookaround only matters when a line is made up *entirely* of
    // spaces sitting at the very start/end of the whole document, and
    // even then the leftover space(s) are removed by the final
    // `^[ ]*$` -> '' pass regardless -- so plain per-line
    // leading/trailing trim produces an identical result.
    static MULTI_SPACE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let multi_space = MULTI_SPACE.get_or_init(|| Regex::new(r"[ ]{2,}").unwrap());
    let collapsed = multi_space.replace_all(&pml, " ").into_owned();
    pml = collapsed
        .split('\n')
        .map(|line| line.trim_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");

    static FOOTNOTE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let footnote = FOOTNOTE.get_or_init(|| {
        Regex::new(r#"(?s)<footnote\s+id="(?P<target>.+?)">\s*(?P<text>.*?)\s*</footnote>"#)
            .unwrap()
    });
    pml = footnote
        .replace_all(&pml, |caps: &regex::Captures| {
            let text = &caps["text"];
            if text.is_empty() {
                String::new()
            } else {
                format!("\\FN=\"{}\"{}\\FN", &caps["target"], text)
            }
        })
        .into_owned();

    static SIDEBAR: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let sidebar = SIDEBAR.get_or_init(|| {
        Regex::new(r#"(?s)<sidebar\s+id="(?P<target>.+?)">\s*(?P<text>.*?)\s*</sidebar>"#).unwrap()
    });
    pml = sidebar
        .replace_all(&pml, |caps: &regex::Captures| {
            let text = &caps["text"];
            if text.is_empty() {
                String::new()
            } else {
                format!("\\SB=\"{}\"{}\\SB", &caps["target"], text)
            }
        })
        .into_owned();

    pml = pml.replace('&', "&amp;");

    static A_CODE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let a_code = A_CODE.get_or_init(|| Regex::new(r"\\a(?P<num>\d{3})").unwrap());
    pml = a_code
        .replace_all(&pml, |caps: &regex::Captures| {
            format!("&#{};", &caps["num"])
        })
        .into_owned();

    static U_CODE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let u_code = U_CODE.get_or_init(|| Regex::new(r"\\U(?P<num>[0-9a-f]{4})").unwrap());
    pml = u_code
        .replace_all(&pml, |caps: &regex::Captures| {
            let n = u32::from_str_radix(&caps["num"], 16).unwrap_or(0);
            // Port of `my_unichr`: `chr(n)`, or `'?'` if `n` is not a
            // valid code point.
            char::from_u32(n)
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".to_string())
        })
        .into_owned();

    prepare_string_for_xml(&pml, false)
}

/// Port of `PML_HTMLizer.strip_pml`: strip every PML code, leaving
/// plain text (used to build a TOC entry's display text from a
/// chapter-marker span that may itself contain further markup).
pub fn strip_pml(pml: &str) -> String {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| {
        [
            r#"\\C\d=".*""#,
            r#"\\Fn=".*""#,
            r#"\\Sd=".*""#,
            r#"\\.=".*""#,
            r"\\X\d",
            r"\\S[pbd]",
            r"\\Fn",
            r"\\a\d\d\d",
            r"\\U\d\d\d\d",
            r"\\.",
        ]
        .iter()
        .map(|p| Regex::new(p).unwrap())
        .collect()
    });

    let mut pml = pml.to_string();
    for re in patterns.iter() {
        pml = re.replace_all(&pml, "").into_owned();
    }
    // NOT `.replace(['\r', '\n'], " ")`: replacing the two-char `\r\n`
    // sequence first (as one space) before any lone survivor is what
    // keeps a Windows line ending from becoming *two* spaces, matching
    // Python's three separate `.replace()` calls in the same order.
    #[allow(clippy::collapsible_str_replace)]
    {
        pml = pml
            .replace("\r\n", " ")
            .replace('\n', " ")
            .replace('\r', " ");
    }
    pml.trim().to_string()
}

// -- the state machine ---------------------------------------------------

/// Port of `calibre.ebooks.pml.pmlconverter.PML_HTMLizer`.
#[derive(Debug, Default)]
pub struct PmlHtmlizer {
    state: HashMap<&'static str, (bool, String)>,
    /// `(level, href, id, text)`. `level` is `None` for a bare `\x`
    /// marker (Python stores the literal int `0` there, which then
    /// never string-equals `'0'`/`'1'`/... in `get_toc` -- see that
    /// method's docs) and `Some(digit)` for a `\X[0-4]`/`\C[0-4]`
    /// marker's level character.
    toc: Vec<(Option<char>, String, String, String)>,
    file_name: String,
}

impl PmlHtmlizer {
    pub fn new() -> Self {
        PmlHtmlizer {
            state: HashMap::new(),
            toc: Vec::new(),
            file_name: String::new(),
        }
    }

    fn is_open(&self, key: &str) -> bool {
        self.state[key].0
    }

    fn close_state_tag(&self, key: &str) -> String {
        let (_, close) = states_tags(key);
        if STATES_CLOSE_VALUE_REQ.contains(&key) {
            fill1(close, &self.state[key].1)
        } else {
            close.to_string()
        }
    }

    fn open_state_tag_plain(&self, key: &str) -> String {
        states_tags(key).0.to_string()
    }

    /// Port of `start_line`.
    fn start_line(&self) -> String {
        // A local copy: the h1 -> h1c exchange (page-break-before is
        // only wanted where the `\x` code actually opened, not on
        // every continuation line of a multi-line `\x` span) must not
        // mutate `self.state` itself.
        let mut state = self.state.clone();
        for &key in STATES {
            if let Some(exchange) = new_line_exchange(key) {
                if state.get(key).is_some_and(|v| v.0) {
                    let val = state[key].clone();
                    state.insert(exchange, val);
                    state.insert(key, (false, String::new()));
                }
            }
        }

        let mut div = Vec::new();
        let mut span = Vec::new();
        let mut other = Vec::new();
        for &key in STATES {
            if let Some((open, val)) = state.get(key) {
                if *open {
                    if DIV_STATES.contains(&key) {
                        div.push((key, val.clone()));
                    } else if SPAN_STATES.contains(&key) {
                        span.push((key, val.clone()));
                    } else {
                        other.push((key, val.clone()));
                    }
                }
            }
        }

        let mut start = String::new();
        for (key, val) in other.into_iter().chain(div).chain(span) {
            let (open, _) = states_tags(key);
            if STATES_VALUE_REQ.contains(&key) {
                start.push_str(&fill1(open, &val));
            } else if STATES_VALUE_REQ_2.contains(&key) {
                start.push_str(&fill2(open, &val));
            } else {
                start.push_str(open);
            }
        }
        format!("<p>{start}")
    }

    /// Port of `end_line`.
    fn end_line(&self) -> String {
        let mut div = Vec::new();
        let mut span = Vec::new();
        let mut other = Vec::new();
        for &key in STATES {
            if self.is_open(key) {
                if DIV_STATES.contains(&key) {
                    div.push(key);
                } else if SPAN_STATES.contains(&key) {
                    span.push(key);
                } else {
                    other.push(key);
                }
            }
        }
        let mut end = String::new();
        for key in span.into_iter().chain(div).chain(other) {
            end.push_str(&self.close_state_tag(key));
        }
        format!("{end}</p>")
    }

    /// Port of `process_code`. `code` is a raw PML escape code (as
    /// looked up in [`code_states`]), not yet resolved to a
    /// `self.state` key.
    fn process_code(&mut self, code: &str, stream: &mut CharStream, pre: &str) -> String {
        let Some(state_key) = code_states(code) else {
            return String::new();
        };

        let text = if DIV_STATES.contains(&state_key) {
            self.process_code_div(state_key, stream)
        } else if SPAN_STATES.contains(&state_key) {
            self.process_code_span(state_key, stream)
        } else if BLOCK_STATES.contains(&state_key) {
            self.process_code_block(state_key, stream, pre)
        } else {
            self.process_code_simple(state_key, stream)
        };

        let entry = self.state.get_mut(state_key).expect("state pre-populated");
        entry.0 = !entry.0;
        text
    }

    /// Port of `process_code_simple`. Unreachable from
    /// [`process_code`] given the current `CODE_STATES` table (every
    /// mapped state is a member of `DIV_STATES`, `SPAN_STATES` or
    /// `BLOCK_STATES`), ported anyway for fidelity with the Python's
    /// public method surface.
    fn process_code_simple(&mut self, code: &str, stream: &mut CharStream) -> String {
        let mut text = String::new();
        if self.is_open(code) {
            text.push_str(&self.close_state_tag(code));
        } else if STATES_VALUE_REQ.contains(&code) || STATES_VALUE_REQ_2.contains(&code) {
            let val = code_value(stream);
            let (open, _) = states_tags(code);
            if STATES_VALUE_REQ.contains(&code) {
                text.push_str(&fill1(open, &val));
            } else {
                text.push_str(&fill2(open, &val));
            }
            self.state.get_mut(code).unwrap().1 = val;
        } else {
            text.push_str(&self.open_state_tag_plain(code));
        }
        text
    }

    /// Port of `process_code_div`.
    fn process_code_div(&mut self, code: &str, stream: &mut CharStream) -> String {
        let mut text = String::new();
        if self.is_open(code) {
            for &c in SPAN_STATES.iter().chain(DIV_STATES.iter()) {
                if self.is_open(c) {
                    text.push_str(&self.close_state_tag(c));
                }
            }
            for &c in DIV_STATES.iter().chain(SPAN_STATES.iter()) {
                if c == code {
                    continue;
                }
                if self.is_open(c) {
                    if STATES_VALUE_REQ.contains(&c) {
                        let (open, _) = states_tags(c);
                        text.push_str(&fill1(open, &self.state[c].1));
                    } else {
                        text.push_str(&self.open_state_tag_plain(c));
                    }
                }
            }
        } else {
            for &c in SPAN_STATES {
                if self.is_open(c) {
                    text.push_str(&self.close_state_tag(c));
                }
            }
            if STATES_VALUE_REQ.contains(&code) {
                let val = code_value(stream);
                let (open, _) = states_tags(code);
                text.push_str(&fill1(open, &val));
                self.state.get_mut(code).unwrap().1 = val;
            } else {
                text.push_str(&self.open_state_tag_plain(code));
            }
            for &c in SPAN_STATES {
                if self.is_open(c) {
                    text.push_str(&self.open_state_tag_plain(c));
                }
            }
        }
        text
    }

    /// Port of `process_code_span`.
    ///
    /// The Python's reopen loops branch on
    /// `c in STATES_VALUE_REQ`/`STATES_VALUE_REQ_2` for `c` ranging
    /// over `SPAN_STATES`; since `SPAN_STATES` (`l`,`k`,`i`,`u`,`d`,`b`)
    /// and `STATES_VALUE_REQ ∪ STATES_VALUE_REQ_2` (`a`,`FN`,`SB`,`ra`)
    /// are disjoint, those branches never fire and only the plain-open
    /// `else` is reachable -- so that's the only one implemented here.
    fn process_code_span(&mut self, code: &str, _stream: &mut CharStream) -> String {
        let mut text = String::new();
        if self.is_open(code) {
            for &c in SPAN_STATES {
                if self.is_open(c) {
                    text.push_str(&self.close_state_tag(c));
                }
            }
            for &c in SPAN_STATES {
                if c == code {
                    continue;
                }
                if self.is_open(c) {
                    text.push_str(&self.open_state_tag_plain(c));
                }
            }
        } else {
            // `code` is always a SPAN_STATES member here, and none of
            // those are in STATES_VALUE_REQ/STATES_VALUE_REQ_2, so the
            // Python's value-required opening branch never fires; the
            // stream is correspondingly never read, as in Python.
            text.push_str(&self.open_state_tag_plain(code));
        }
        text
    }

    /// Port of `process_code_block`.
    fn process_code_block(&mut self, code: &str, stream: &mut CharStream, pre: &str) -> String {
        let mut text = String::new();
        for &c in SPAN_STATES {
            if self.is_open(c) {
                text.push_str(&self.close_state_tag(c));
            }
        }
        if self.is_open(code) {
            // None of BLOCK_STATES are in STATES_CLOSE_VALUE_REQ (only
            // FN/SB are, and those are DIV_STATES), so this is always
            // a plain close.
            let (_, close) = states_tags(code);
            text.push_str(close);
        } else if STATES_VALUE_REQ.contains(&code) || STATES_VALUE_REQ_2.contains(&code) {
            let mut val = code_value(stream);
            if LINK_STATES.contains(&code) {
                val = val.trim_start_matches('#').to_string();
            }
            if !pre.is_empty() {
                val = format!("{pre}-{val}");
            }
            let (open, _) = states_tags(code);
            if STATES_VALUE_REQ.contains(&code) {
                text.push_str(&fill1(open, &val));
            } else {
                text.push_str(&fill2(open, &val));
            }
            self.state.get_mut(code).unwrap().1 = val;
        } else {
            text.push_str(&self.open_state_tag_plain(code));
        }
        for &c in SPAN_STATES {
            if self.is_open(c) {
                text.push_str(&self.open_state_tag_plain(c));
            }
        }
        text
    }

    fn basename(&self) -> String {
        std::path::Path::new(&self.file_name)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Port of `parse_pml`.
    // `indent_capital_t`/`adv_indent_val`'s resets at the end of a line
    // (mirroring `indent_state['T'] = False` in Python) are true
    // dead stores -- the next line unconditionally overwrites both
    // before reading them -- but are kept for fidelity with the
    // Python's own (equally redundant) resets.
    #[allow(unused_assignments)]
    pub fn parse_pml(&mut self, pml: &str, file_name: &str) -> String {
        let pml = prepare_pml(pml);
        let mut output: Vec<String> = Vec::new();

        self.state = HashMap::new();
        self.toc.clear();
        self.file_name = file_name.to_string();

        // `indent_t` alone persists meaningfully across lines (an open
        // `\t` block carries over); the rest are unconditionally
        // recomputed at the top of every line before they're read, so
        // they don't need a pre-loop initial value.
        let mut indent_t = false;
        let mut indent_capital_t;
        let mut indent_st;
        let mut indent_s_capital_t;
        let mut indent_et;
        let mut basic_indent;
        let mut adv_indent_val = String::new();
        let mut empty_count = 0u32;

        for &s in STATES {
            self.state.insert(s, (false, String::new()));
        }

        for line in splitlines(&pml) {
            let mut parsed: Vec<String> = Vec::new();
            let mut empty = true;

            basic_indent = indent_t;
            indent_capital_t = false;
            if line.trim_start().starts_with("\\t") || basic_indent {
                basic_indent = true;
                indent_st = true;
            } else {
                indent_st = false;
            }
            indent_s_capital_t = line.trim_start().starts_with("\\T");
            indent_et = line.trim_end().ends_with("\\t");

            let mut stream = CharStream::new(line);
            parsed.push(self.start_line());

            while let Some(c) = stream.read1() {
                let mut text = String::new();

                if c == '\\' {
                    let Some(code_char) = stream.read1() else {
                        parsed.push(text);
                        break;
                    };

                    if "qcriIuobBlk".contains(code_char) {
                        text = self.process_code(&code_char.to_string(), &mut stream, "");
                    } else if code_char == 'F' || code_char == 'S' {
                        if let Some(l) = stream.read1() {
                            let two = format!("{code_char}{l}");
                            match two.as_str() {
                                "Fn" => text = self.process_code("Fn", &mut stream, "fn"),
                                "FN" => text = self.process_code("FN", &mut stream, ""),
                                "SB" => text = self.process_code("SB", &mut stream, ""),
                                "Sd" => text = self.process_code("Sd", &mut stream, "sb"),
                                // `\Sp`/`\Sb` (superscript/subscript)
                                // fall here too, and are silently
                                // dropped -- see the module docs.
                                _ => {}
                            }
                        }
                    } else if code_char == 'x' || code_char == 'X' || code_char == 'C' {
                        empty = false;
                        let mut t = String::new();
                        let level: Option<char> = if code_char == 'X' || code_char == 'C' {
                            stream.read1()
                        } else {
                            None
                        };
                        let id = format!("pml_toc-{}", self.toc.len());
                        let value = code_value(&mut stream);
                        if code_char == 'x' {
                            t = self.process_code("x", &mut stream, "");
                        } else if code_char == 'X' {
                            let code = format!("X{}", level.unwrap_or(' '));
                            t = self.process_code(&code, &mut stream, "");
                        }
                        if value.is_empty() {
                            text = t;
                        } else {
                            self.toc.push((level, self.basename(), id.clone(), value));
                            text = format!("{t}<span id=\"{id}\"></span>");
                        }
                    } else if code_char == 'm' {
                        empty = false;
                        let src = code_value(&mut stream);
                        text = format!("<img src=\"images/{src}\" />");
                    } else if code_char == 'Q' {
                        empty = false;
                        let id = code_value(&mut stream);
                        text = format!("<span id=\"{id}\"></span>");
                    } else if code_char == 'p' {
                        empty = false;
                        text = "<br /><br style=\"page-break-after: always;\" />".to_string();
                    } else if code_char == 'n' {
                        // No-op.
                    } else if code_char == 'w' {
                        empty = false;
                        text = format!("<hr style=\"width: {}\" />", code_value(&mut stream));
                    } else if code_char == 't' {
                        indent_t = !indent_t;
                    } else if code_char == 'T' {
                        if !indent_capital_t {
                            adv_indent_val = code_value(&mut stream);
                        } else {
                            code_value(&mut stream);
                        }
                        indent_capital_t = true;
                    } else if code_char == '-' {
                        empty = false;
                        text = "&shy;".to_string();
                    } else if code_char == '\\' {
                        empty = false;
                        text = "\\".to_string();
                    }
                    // Any other escape code is silently dropped,
                    // matching the Python's lack of a final `else`.
                } else {
                    if c != ' ' {
                        empty = false;
                    }
                    text = c.to_string();
                }
                parsed.push(text);
            }

            if empty {
                empty_count += 1;
                if empty_count == 2 {
                    output.push("<p>&nbsp;</p>".to_string());
                }
            } else {
                empty_count = 0;
                parsed.push(self.end_line());

                if basic_indent {
                    if indent_st && (indent_et || indent_t) {
                        parsed.insert(0, states_tags("t").0.to_string());
                        parsed.push(states_tags("t").1.to_string());
                    } else {
                        parsed.insert(0, fill1(states_tags("T").0, "5%"));
                        parsed.push(states_tags("T").1.to_string());
                    }
                } else if indent_capital_t && indent_s_capital_t {
                    parsed.insert(0, fill1(states_tags("T").0, &adv_indent_val));
                    parsed.push(states_tags("T").1.to_string());
                    indent_capital_t = false;
                    adv_indent_val = String::new();
                }

                output.push(parsed.concat());
            }
        }

        cleanup_html(&output.join("\n"))
    }

    /// Port of `get_toc`.
    ///
    /// TOC can have up to 5 levels, 0-4 inclusive. This adds items to
    /// their appropriate depth in the TOC tree; an item whose specified
    /// depth would leave it without a valid parent is attached to the
    /// nearest valid level above it instead. `level == None` (a bare
    /// `\x` marker; see this struct's `toc` field docs) never matches
    /// `'0'..='3'` and always falls into the level-4 bucket, matching
    /// Python's `0 == '0'` being `False`.
    pub fn get_toc(&self) -> TOC {
        let mut root_children: Vec<TOCNode> = Vec::new();
        let mut t_l0: Option<Vec<usize>> = None;
        let mut t_l1: Option<Vec<usize>> = None;
        let mut t_l2: Option<Vec<usize>> = None;
        let mut t_l3: Option<Vec<usize>> = None;

        fn get_mut<'a>(root: &'a mut Vec<TOCNode>, path: &[usize]) -> &'a mut Vec<TOCNode> {
            let mut cur = root;
            for &idx in path {
                cur = &mut cur[idx].children;
            }
            cur
        }

        fn add_item(
            root: &mut Vec<TOCNode>,
            path: &[usize],
            href: &str,
            id: &str,
            text: &str,
        ) -> Vec<usize> {
            let siblings = get_mut(root, path);
            siblings.push(TOCNode {
                title: text.to_string(),
                src: join_href(href, id),
                children: Vec::new(),
            });
            let mut new_path = path.to_vec();
            new_path.push(siblings.len() - 1);
            new_path
        }

        for (level, href, id, text) in &self.toc {
            match level {
                Some('0') => {
                    t_l0 = Some(add_item(&mut root_children, &[], href, id, text));
                    t_l1 = None;
                    t_l2 = None;
                    t_l3 = None;
                }
                Some('1') => {
                    if t_l0.is_none() {
                        t_l0 = Some(vec![]);
                    }
                    let parent = t_l0.clone().unwrap();
                    t_l1 = Some(add_item(&mut root_children, &parent, href, id, text));
                    t_l2 = None;
                    t_l3 = None;
                }
                Some('2') => {
                    if t_l1.is_none() {
                        t_l1 = Some(t_l0.clone().unwrap_or_default());
                    }
                    let parent = t_l1.clone().unwrap();
                    t_l2 = Some(add_item(&mut root_children, &parent, href, id, text));
                    t_l3 = None;
                }
                Some('3') => {
                    if t_l2.is_none() {
                        t_l2 = Some(if t_l1.is_none() {
                            t_l0.clone().unwrap_or_default()
                        } else {
                            t_l1.clone().unwrap()
                        });
                    }
                    let parent = t_l2.clone().unwrap();
                    t_l3 = Some(add_item(&mut root_children, &parent, href, id, text));
                }
                _ => {
                    if t_l3.is_none() {
                        t_l3 = Some(if t_l2.is_none() {
                            if t_l1.is_none() {
                                t_l0.clone().unwrap_or_default()
                            } else {
                                t_l1.clone().unwrap()
                            }
                        } else {
                            t_l2.clone().unwrap()
                        });
                    }
                    let parent = t_l3.clone().unwrap();
                    add_item(&mut root_children, &parent, href, id, text);
                }
            }
        }

        TOC {
            nodes: root_children,
        }
    }
}

/// Port of `cleanup_html`: repeatedly strip empty/redundant tag pairs
/// until a fixed point, then strip leading whitespace throughout.
fn cleanup_html(html: &str) -> String {
    let mut html = html.to_string();
    loop {
        let next = cleanup_html_remove_redundant(&html);
        if next == html {
            break;
        }
        html = next;
    }
    static LEADING_WS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = LEADING_WS.get_or_init(|| Regex::new(r"(?m)^\s*").unwrap());
    re.replace_all(&html, "").into_owned()
}

/// Port of `cleanup_html_remove_redundant`. See the module docs for why
/// `ra`/`FN`/`SB`'s removal never actually matches anything -- that is
/// a faithful reproduction of the Python, not a bug in this port.
fn cleanup_html_remove_redundant(html: &str) -> String {
    let mut html = html.to_string();
    for &key in STATES_TAGS_ORDER {
        let (open, close) = states_tags(key);
        let open_pattern = if STATES_VALUE_REQ.contains(&key) {
            // Python's `.*?` here has no `(?s)`/DOTALL either (the
            // substitution is done under just `(?u)`), so `.` must not
            // match newlines -- matches this regex's default.
            fill1(open, ".*?")
        } else {
            open.to_string()
        };
        let pattern = format!(r"(?u){open_pattern}\s*{close}");
        if let Ok(re) = Regex::new(&pattern) {
            html = re.replace_all(&html, "").into_owned();
        }
    }
    static EMPTY_P: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = EMPTY_P.get_or_init(|| Regex::new(r"(?im)<p>\s*</p>").unwrap());
    re.replace_all(&html, "").into_owned()
}

/// Port of `pml_to_html`.
pub fn pml_to_html(pml: &str) -> String {
    let mut hizer = PmlHtmlizer::new();
    hizer.parse_pml(pml, "")
}

/// Port of `footnote_sidebar_to_html`.
pub fn footnote_sidebar_to_html(pre_id: &str, id: &str, pml: &str) -> String {
    let trimmed_id = id.trim_matches('\u{1}');
    if !trimmed_id.trim().is_empty() {
        format!(
            "<br /><br style=\"page-break-after: always;\" /><div id=\"{pre_id}-{trimmed_id}\">{}<small><a href=\"#r{pre_id}-{trimmed_id}\">return</a></small></div>",
            pml_to_html(pml)
        )
    } else {
        format!(
            "<br /><br style=\"page-break-after: always;\" /><div>{}</div>",
            pml_to_html(pml)
        )
    }
}

/// Port of `footnote_to_html`.
pub fn footnote_to_html(id: &str, pml: &str) -> String {
    footnote_sidebar_to_html("fn", id, pml)
}

/// Port of `sidebar_to_html`.
pub fn sidebar_to_html(id: &str, pml: &str) -> String {
    footnote_sidebar_to_html("sb", id, pml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_italic_underline_strikethrough_toggle() {
        assert_eq!(
            pml_to_html(r"\bbold\b"),
            "<p><span style=\"font-weight: bold;\">bold</span></p>"
        );
        assert_eq!(
            pml_to_html(r"\iitalic\i"),
            "<p><span style=\"font-style: italic;\">italic</span></p>"
        );
        assert_eq!(
            pml_to_html(r"\uunderline\u"),
            "<p><span style=\"text-decoration: underline;\">underline</span></p>"
        );
        assert_eq!(
            pml_to_html(r"\ostrike\o"),
            "<p><span style=\"text-decoration: line-through;\">strike</span></p>"
        );
    }

    #[test]
    fn nested_spans_reopen_correctly_when_one_closes_early() {
        // \b opens bold, \i opens italic (nested), \b closes bold --
        // italic must be closed, bold closed, then italic reopened.
        let html = pml_to_html(r"\bA\iB\bC\i");
        assert_eq!(
            html,
            "<p><span style=\"font-weight: bold;\">A<span style=\"font-style: italic;\">B</span></span><span style=\"font-style: italic;\">C</span></p>"
        );
    }

    #[test]
    fn headings_produce_a_page_break_and_a_toc_entry() {
        // Raw PML never spells out the `="..."` title itself -- that's
        // what `prepare_pml`'s marker-wrapping derives from the text
        // between the pair of `\x` codes.
        let mut hizer = PmlHtmlizer::new();
        let html = hizer.parse_pml(r"\xChapter One\x", "index.html");
        // The TOC-marker anchor span is emitted right after the
        // opening tag, ahead of the heading text itself.
        assert!(
            html.contains(
                "<h1 style=\"page-break-before: always;\"><span id=\"pml_toc-0\"></span>Chapter One</h1>"
            ),
            "{html}"
        );
        let toc = hizer.get_toc();
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "Chapter One");
        assert_eq!(toc.nodes[0].src, "index.html#pml_toc-0");
    }

    #[test]
    fn nested_toc_levels_build_a_tree() {
        // `\X0`..`\X4` store their TOC level as the literal digit
        // character, so (unlike a bare `\x` -- see the quirk test
        // below) they nest exactly as their digits suggest.
        let mut hizer = PmlHtmlizer::new();
        let pml = concat!(
            "\\X0Book\\X0\n",
            "\\X1Chapter 1\\X1\n",
            "\\X2Section 1.1\\X2\n",
        );
        hizer.parse_pml(pml, "index.html");
        let toc = hizer.get_toc();
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "Book");
        assert_eq!(toc.nodes[0].children.len(), 1);
        assert_eq!(toc.nodes[0].children[0].title, "Chapter 1");
        assert_eq!(toc.nodes[0].children[0].children[0].title, "Section 1.1");
    }

    #[test]
    fn bare_x_markers_never_become_a_top_level_toc_entry() {
        // A quirk in the Python: `\x` markers store their TOC "level"
        // as the literal *int* `0`, while `\X0`..`\X4` store it as the
        // digit *character* read from the source. `get_toc` compares
        // levels with `level == '0'` (a string), which is never true
        // for the int -- so a bare `\x` entry always falls through to
        // the deepest ("level 4") fallback bucket, regardless of `\x`
        // visually mapping to `<h1>`, the topmost heading level.
        let mut hizer = PmlHtmlizer::new();
        let pml = concat!("\\X0Chapter\\X0\n", "\\xOrphan\\x\n");
        hizer.parse_pml(pml, "index.html");
        let toc = hizer.get_toc();
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "Chapter");
        // "Orphan" ends up nested *under* "Chapter", not beside it.
        assert_eq!(toc.nodes[0].children.len(), 1);
        assert_eq!(toc.nodes[0].children[0].title, "Orphan");
    }

    #[test]
    fn a_toc_level_with_no_valid_parent_attaches_to_the_root() {
        // A level-2 entry with no preceding level-0/1 entry attaches
        // directly under the (synthetic) root.
        let mut hizer = PmlHtmlizer::new();
        hizer.parse_pml(r"\X0Deep\X0", "index.html");
        let toc = hizer.get_toc();
        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].title, "Deep");
    }

    #[test]
    fn footnotes_and_sidebars_become_divs() {
        let html = footnote_to_html("1", "footnote text");
        assert!(html.contains("id=\"fn-1\""), "{html}");
        assert!(html.contains("footnote text"), "{html}");
        assert!(html.contains("#rfn-1"), "{html}");

        let html = sidebar_to_html("2", "sidebar text");
        assert!(html.contains("id=\"sb-2\""), "{html}");
        assert!(html.contains("#rsb-2"), "{html}");
    }

    #[test]
    fn footnote_with_control_char_only_id_omits_the_anchor() {
        let html = footnote_to_html("\u{1}", "text");
        assert!(!html.contains("id=\"fn-"), "{html}");
        assert!(html.contains("text"), "{html}");
    }

    #[test]
    fn basic_indent_wraps_the_line_in_a_left_margin_div() {
        let html = pml_to_html("\\ttext\\t");
        assert!(html.contains("<div style=\"margin-left: 5%;\">"), "{html}");
    }

    #[test]
    fn advanced_indent_uses_the_given_percentage() {
        let html = pml_to_html("\\T=\"10%\"text");
        assert!(html.contains("<div style=\"text-indent: 10%;\">"), "{html}");
    }

    #[test]
    fn a_unicode_escape_becomes_the_actual_character() {
        // \U0041 = 'A'
        let html = pml_to_html("\\U0041");
        assert!(html.contains('A'), "{html}");
    }

    #[test]
    fn an_a_escape_becomes_a_numeric_entity() {
        // \a160 -- prepare_pml turns it into `&#160;`, and
        // prepare_string_for_xml's entity-resolution pass then decodes
        // that right back into the literal non-breaking space
        // character (see `prepare_pml`'s docs on this round trip).
        // Surrounded by real text: a *lone* nbsp forms a whitespace-only
        // `<p>`, which `cleanup_html` legitimately strips (`\s` matches
        // U+00A0 in both Rust's and Python's regex engines), matching
        // Python -- so this checks the round trip with visible text on
        // both sides instead of relying on nothing else being present.
        let html = pml_to_html("before\\a160after");
        assert!(html.contains('\u{a0}'), "{html:?}");
        assert!(html.contains("before\u{a0}after"), "{html:?}");
    }

    #[test]
    fn two_consecutive_blank_lines_become_a_single_nbsp_paragraph() {
        let html = pml_to_html("line one\n\n\nline two");
        assert!(html.contains("<p>&nbsp;</p>"), "{html}");
        // Exactly one -- the counter only triggers *at* 2, not on
        // every subsequent blank line in the run.
        assert_eq!(html.matches("&nbsp;").count(), 1, "{html}");
    }

    #[test]
    fn links_and_anchors_round_trip_through_code_value() {
        // Double-hash delimiter: the content contains a `"#` sequence
        // (`\q="#target"`), which would close a single-hash raw string
        // early -- the exact bug this crate has hit before.
        let html = pml_to_html(r##"\q="#target"link text\q"##);
        assert!(html.contains("<a href=\"#target\">link text</a>"), "{html}");
    }

    // -- code_value ----------------------------------------------------

    fn value_after(line: &str) -> (String, usize) {
        let mut stream = CharStream::new(line);
        let v = code_value(&mut stream);
        (v, stream.tell())
    }

    #[test]
    fn code_value_reads_a_well_formed_quoted_value() {
        let (v, pos) = value_after(r#"="hello"rest"#);
        assert_eq!(v, "hello");
        assert_eq!(&r#"="hello"rest"#[..pos], r#"="hello""#);
    }

    #[test]
    fn code_value_allows_spaces_around_the_equals_and_quote() {
        let (v, _) = value_after(r#"  =  "hello""#);
        assert_eq!(v, "hello");
    }

    #[test]
    fn code_value_trims_the_captured_value() {
        let (v, _) = value_after(r#"=" hello "rest"#);
        assert_eq!(v, "hello");
    }

    #[test]
    fn code_value_backtracks_on_a_missing_equals() {
        let (v, pos) = value_after(r#"garbage"#);
        assert_eq!(v, "");
        assert_eq!(pos, 0, "stream position must be reset");
    }

    #[test]
    fn code_value_backtracks_on_a_missing_opening_quote() {
        let (v, pos) = value_after(r#"=garbage"#);
        assert_eq!(v, "");
        assert_eq!(pos, 0);
    }

    #[test]
    fn code_value_backtracks_on_a_missing_closing_quote() {
        let (v, pos) = value_after(r#"="unterminated"#);
        assert_eq!(v, "");
        assert_eq!(pos, 0);
    }

    #[test]
    fn code_value_backtracks_on_an_empty_stream() {
        let (v, pos) = value_after("");
        assert_eq!(v, "");
        assert_eq!(pos, 0);
    }

    // -- strip_pml / prepare_pml ----------------------------------------

    #[test]
    fn strip_pml_removes_every_code_leaving_plain_text() {
        assert_eq!(strip_pml(r"\bBold\b text"), "Bold text");
        assert_eq!(strip_pml("line1\nline2"), "line1 line2");
    }

    #[test]
    fn prepare_pml_escapes_ampersands_and_angle_brackets() {
        let out = prepare_pml("Smith & Co <tag>");
        assert!(out.contains("Smith &amp; Co"), "{out}");
        assert!(out.contains("&lt;tag&gt;"), "{out}");
    }
}
