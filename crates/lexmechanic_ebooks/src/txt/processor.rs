//! Port of `old_src/src/calibre/ebooks/txt/processor.py`.
//!
//! # Reuse
//!
//! - `clean_ascii_chars` is `crate::rtf2xml::replace_illegals::clean_ascii_chars`
//!   (issue #?? -- ported for `replace_illegals.py`), which is a byte-for-byte
//!   match for `calibre.utils.cleantext.clean_ascii_chars`: it strips
//!   ASCII control characters 0x00-0x1F except tab/LF/CR, plus DEL
//!   (0x7F). Verified against `calibre/utils/cleantext.py`'s
//!   `ascii_pat`, not re-derived from scratch.
//! - `convert_textile` calls `crate::textile::functions::textile`
//!   (issue #53) directly, rather than reimplementing PyTextile.
//! - `unit_convert` (needed by `txt::textileml`, not this file) is
//!   `crate::oeb::transforms::flatcss::unit_convert`, already a
//!   byte-for-byte port of `calibre.ebooks.unit_convert`.
//!
//! # `DocAnalysis`/`line_histogram`
//!
//! `detect_paragraph_type` depends on
//! `calibre.ebooks.conversion.preprocess.DocAnalysis`, which is not
//! part of this issue -- it lives in `conversion/preprocess.py` (629
//! lines, tracked separately in `docs/modules_to_port.md`). Rather than
//! gap the dependency out, [`DocAnalysis`] here is a narrow,
//! purpose-built slice of it: just the `format='txt'` line-extraction
//! mode and the `line_histogram` statistic, which is the *only*
//! `DocAnalysis(...)` construction/method this file actually calls
//! (`DocAnalysis('txt', txt).line_histogram(.55)`, the sole call site).
//! When `conversion::preprocess` is fully ported, its `DocAnalysis`
//! should supersede this one rather than live alongside it.
//!
//! # `fancy_regex` vs. `regex`
//!
//! A handful of patterns here use real lookaround
//! (`(?<=...)`/`(?=...)`), which the plain `regex` crate cannot express
//! at all; those use `fancy_regex` instead, following the precedent set
//! by `crate::textile::functions`. Everything else uses plain `regex`.
//!
//! # Preserved upstream quirks
//!
//! - **`detect_formatting_type`'s markdown "links" pattern inflates the
//!   count by one per line.** The Python regex is
//!   `r'(?mu)^|[^!]\[.*?\](\[|\()'` -- due to `|`'s low precedence, this
//!   parses as "start-of-line (zero-width) OR an actual link pattern",
//!   so `findall` returns one empty match per line *in addition to* any
//!   real links. Verified interactively against CPython's `re`: a
//!   4-line, 1-link document returns 5 matches, not 1. This inflates
//!   `markdown_count` by roughly the document's line count on every
//!   call, which in practice pushes almost any multi-line document over
//!   the `> 5` markdown/textile threshold. Ported byte-for-byte via the
//!   same pattern string, not "fixed".
//! - **`split_string_separator` duplicates the split-point `.`.** Python's
//!   `part[:idx+1] + b'\n\n' + part[idx:]` includes the byte at `idx`
//!   (the period) on *both* sides of the inserted `\n\n`. Ported as-is.
//! - **`separate_hard_scene_breaks`'s character class is a huge ASCII
//!   range, not the five punctuation marks it looks like.** The Python
//!   pattern is `r'(?miu)^[ \t-=~\/_]+$'`; inside `[...]`, `\t-=` is a
//!   *range* (tab through `=`), not three separate characters, so the
//!   class covers most ASCII control chars and punctuation between
//!   `\t` (0x09) and `=` (0x3D), plus `~`, `/`, `_`, and space -- not
//!   the presumably-intended `{space, tab, -, =, ~, /, _}`. Verified
//!   interactively against CPython's `re`. Ported via the identical
//!   pattern string (Rust's `regex` crate parses bracket-class ranges
//!   the same way), not "fixed" to the presumably-intended set.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use fancy_regex::Regex as FancyRegex;
use lazy_static::lazy_static;
use pulldown_cmark::{html, Options, Parser};
use regex::Regex;

use crate::metadata::MetaInformation;
use crate::rtf2xml::replace_illegals::clean_ascii_chars;
use crate::xml_util::prepare_string_for_xml;

/// Port of `HTML_TEMPLATE`.
fn html_template(title: &str, body: &str) -> String {
    format!(
        "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=utf-8\"/><title>{title} </title></head><body>\n{body}\n</body></html>"
    )
}

/// Port of Python's `str.splitlines(keepends=False)`: splits on `\n`,
/// `\r`, `\r\n` (as one boundary), `\x0b`, `\x0c`, `\x1c`-`\x1e`, `\x85`,
/// `\u{2028}`, `\u{2029}`. No trailing empty entry for a string that
/// ends with a boundary.
pub(crate) fn python_splitlines(s: &str) -> Vec<&str> {
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i < chars.len() {
        let (byte_idx, ch) = chars[i];
        let is_boundary = matches!(
            ch,
            '\n' | '\r'
                | '\u{0B}'
                | '\u{0C}'
                | '\u{1C}'
                | '\u{1D}'
                | '\u{1E}'
                | '\u{85}'
                | '\u{2028}'
                | '\u{2029}'
        );
        if is_boundary {
            lines.push(&s[start..byte_idx]);
            if ch == '\r' && chars.get(i + 1).map(|(_, c)| *c) == Some('\n') {
                i += 1;
            }
            start = chars.get(i + 1).map(|(b, _)| *b).unwrap_or(s.len());
        }
        i += 1;
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Port of `clean_txt`.
pub fn clean_txt(txt: &str) -> String {
    lazy_static! {
        static ref LEADING_LINE_WS: FancyRegex =
            FancyRegex::new(r"(?m)^([ ]{2,}|\t+)(?=.)").expect("static regex");
        static ref REDUNDANT_SPACES: Regex = Regex::new(r"[ ]{2,}").expect("static regex");
        static ref LEADING_DOC_WS: FancyRegex =
            FancyRegex::new(r"^\s+(?=.)").expect("static regex");
        static ref TRAILING_DOC_WS: FancyRegex =
            FancyRegex::new(r"(?<=.)\s+$").expect("static regex");
        static ref EXCESS_BLANK_LINES: Regex = Regex::new(r"\n{5,}").expect("static regex");
    }

    let txt = python_splitlines(txt)
        .iter()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n");

    let txt = LEADING_LINE_WS
        .replace_all(&txt, "&nbsp;&nbsp;&nbsp;&nbsp;")
        .into_owned();
    let txt = REDUNDANT_SPACES.replace_all(&txt, " ").into_owned();
    let txt = LEADING_DOC_WS.replace_all(&txt, "").into_owned();
    let txt = TRAILING_DOC_WS.replace_all(&txt, "").into_owned();
    let txt = EXCESS_BLANK_LINES
        .replace_all(&txt, "\n\n\n\n")
        .into_owned();
    clean_ascii_chars(&txt)
}

/// Port of `split_utf8`. Python raises `ValueError` for `n < 3`; this
/// returns `Err` instead of panicking.
///
/// # Diverges from upstream: no infinite loop on an oversized character
///
/// Upstream's backward scan (`while (s[k] & 0xc0) == 0x80: k -= 1`) has
/// no lower bound at all -- for a 4-byte character sitting at the very
/// start of the remaining slice with `n == 3`, the scan backs off
/// through all 3 continuation bytes and stops at the *lead* byte
/// (index 0), which fails the continuation check for the ordinary
/// reason (`k == 0`), not because Python special-cased it. That leaves
/// `k == 0`: `s[:0]` yields an empty chunk and `s = s[0:]` is
/// unchanged, so `len(s) > n` stays true forever -- upstream hangs
/// forever on this input too, it is not a porting-introduced bug. A
/// Rust `Vec` growing unbounded in that same loop turns "hangs" into
/// "exhausts memory and gets OOM-killed" (confirmed: this is what
/// crashed the session that had been running this test), which is
/// worse than a faithful port's hang, so this is fixed rather than
/// carried forward. When the backward scan can't find a boundary
/// within `n` bytes (`k == 0`), the character straddling the window
/// can't be represented in `n` bytes at all -- so instead of splitting
/// it, this includes it whole by scanning *forward* to its end. That
/// guarantees `k >= 1` on every iteration, so `rest` always shrinks
/// and the loop always terminates; the one resulting chunk is a few
/// bytes over `n` in this rare case rather than corrupt or absent.
fn split_utf8(s: &[u8], n: usize) -> Result<Vec<Vec<u8>>> {
    if n < 3 {
        bail!("Cannot split into chunks of less than {n} < 4 bytes");
    }
    let mut chunks = Vec::new();
    let mut rest = s;
    while rest.len() > n {
        let mut k = n;
        // Back off any UTF-8 continuation byte (`10xxxxxx`) so no chunk
        // splits a multi-byte character. `k > 0` guards against
        // underflow.
        while k > 0 && (rest[k] & 0xc0) == 0x80 {
            k -= 1;
        }
        if k == 0 {
            // The character covering this boundary doesn't fit in `n`
            // bytes at all -- scan forward to its end instead of
            // splitting it (or looping forever trying to).
            k = 1;
            while k < rest.len() && (rest[k] & 0xc0) == 0x80 {
                k += 1;
            }
        }
        chunks.push(rest[..k].to_vec());
        rest = &rest[k..];
    }
    chunks.push(rest.to_vec());
    Ok(chunks)
}

/// Port of `split_string_separator`. See the module docs for the
/// preserved duplicate-`.`  quirk.
fn split_string_separator(txt: &[u8], size: usize) -> Vec<u8> {
    if txt.len() > size && size > 3 {
        let size = size - 2;
        let Ok(parts) = split_utf8(txt, size) else {
            // Degenerate `size` (< 3 after the -2 above): Python would
            // raise here. Rather than propagate a panic for an edge
            // case no exposed option can reach in practice (chunk sizes
            // come from KB-scale split settings), leave the text alone.
            return txt.to_vec();
        };
        let mut ans = Vec::new();
        for part in parts {
            match part.iter().rposition(|&b| b == b'.') {
                None => {
                    ans.extend_from_slice(&part);
                    ans.extend_from_slice(b"\n\n");
                }
                Some(idx) => {
                    ans.extend_from_slice(&part[..=idx]);
                    ans.extend_from_slice(b"\n\n");
                    ans.extend_from_slice(&part[idx..]);
                }
            }
        }
        ans
    } else {
        txt.to_vec()
    }
}

fn split_on_bytes<'a>(data: &'a [u8], sep: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut i = 0usize;
    while i + sep.len() <= data.len() {
        if &data[i..i + sep.len()] == sep {
            parts.push(&data[start..i]);
            i += sep.len();
            start = i;
        } else {
            i += 1;
        }
    }
    parts.push(&data[start..]);
    parts
}

/// Port of `split_txt`.
pub fn split_txt(txt: &str, epub_split_size_kb: usize) -> String {
    if epub_split_size_kb == 0 {
        return txt.to_string();
    }
    let bytes = txt.as_bytes();
    if bytes.len() > epub_split_size_kb * 1024 {
        // `epub_split_size_kb - 32` can go negative in Python; `max(16,
        // ...)` always wins in that case, so a saturating subtraction
        // (floor of 0) is equivalent.
        let chunk_size = std::cmp::max(16, epub_split_size_kb.saturating_sub(32)) * 1024;
        let parts = split_on_bytes(bytes, b"\n\n");
        let max_len = parts.iter().map(|p| p.len()).max().unwrap_or(0);
        if max_len > chunk_size {
            let mut joined: Vec<u8> = Vec::new();
            for (i, part) in parts.iter().enumerate() {
                if i > 0 {
                    joined.extend_from_slice(b"\n\n");
                }
                joined.extend_from_slice(&split_string_separator(part, chunk_size));
            }
            return String::from_utf8_lossy(&joined).into_owned();
        }
    }
    txt.to_string()
}

/// Port of `convert_basic`.
pub fn convert_basic(txt: &str, title: &str, epub_split_size_kb: usize) -> String {
    let txt = clean_txt(txt);
    let txt = split_txt(&txt, epub_split_size_kb);

    let mut lines = Vec::new();
    let mut blank_count = 0u32;
    for line in txt.split('\n') {
        if !line.trim().is_empty() {
            blank_count = 0;
            let escaped = prepare_string_for_xml(&line.replace('\n', " "), false);
            lines.push(format!("<p>{escaped}</p>"));
        } else {
            blank_count += 1;
            if blank_count == 2 {
                lines.push("<p>&nbsp;</p>".to_string());
            }
        }
    }

    html_template(title, &lines.join("\n"))
}

/// Port of `DEFAULT_MD_EXTENSIONS`.
pub const DEFAULT_MD_EXTENSIONS: &[&str] = &["footnotes", "tables", "toc"];

/// Port of `create_markdown_object` + `Markdown.convert`, as far as
/// `pulldown_cmark` covers it.
///
/// # `pulldown_cmark` vs. Python's `markdown`
///
/// These are different engines, not just different bindings to the
/// same one, so this is a best-effort mapping rather than a literal
/// port:
/// - `'tables'` -> `Options::ENABLE_TABLES`, `'footnotes'` ->
///   `Options::ENABLE_FOOTNOTES`: real equivalents, both exercised by
///   the default extension list.
/// - `'toc'` has no `pulldown_cmark` equivalent and is silently
///   ignored (no automatic table-of-contents markup is generated).
/// - `'meta'` is not handled here at all -- `convert_markdown_with_metadata`
///   strips the front-matter block itself, via [`parse_markdown_meta`],
///   before this function ever sees the body text.
/// - Any other extension name (`'abbr'`, `'admonition'`, `'attr_list'`,
///   ...) is silently ignored; `pulldown_cmark` has no plugin
///   architecture to hang them off of.
fn markdown_options(extensions: &[&str]) -> Options {
    let mut options = Options::empty();
    for ext in extensions {
        match ext.to_lowercase().as_str() {
            "tables" => options.insert(Options::ENABLE_TABLES),
            "footnotes" => options.insert(Options::ENABLE_FOOTNOTES),
            _ => {}
        }
    }
    options
}

fn markdown_to_html(txt: &str, extensions: &[&str]) -> String {
    let options = markdown_options(extensions);
    let parser = Parser::new_ext(txt, options);
    let mut html_output = String::new();
    html::push_html(&mut html_output, parser);
    html_output
}

/// Port of `convert_markdown`.
pub fn convert_markdown(txt: &str, title: &str, extensions: &[&str]) -> String {
    html_template(title, &markdown_to_html(txt, extensions))
}

/// Port of the simple `Key: value` front-matter grammar implemented by
/// Python's `markdown.extensions.meta` (there is no vendored copy of it
/// in this tree to port literally from, so this follows the
/// well-documented, stable behavior of that extension: `Key: value`
/// pairs; a line indented 4+ spaces with no `key:` prefix extends the
/// *previous* key with an additional list entry, not concatenated
/// text; a blank line, a `---`/`...` marker line, or the first
/// non-matching line ends the block). Keys are lower-cased. Returns the
/// parsed metadata and the remaining body text (the front matter
/// consumed, everything after it untouched).
fn parse_markdown_meta(txt: &str) -> (HashMap<String, Vec<String>>, String) {
    lazy_static! {
        static ref META_RE: Regex =
            Regex::new(r"^[ ]{0,3}(?P<key>[A-Za-z0-9_-]+):\s*(?P<value>.*)$").expect("regex");
        static ref META_MORE_RE: Regex = Regex::new(r"^[ ]{4,}(?P<value>.*)$").expect("regex");
        static ref BEGIN_RE: Regex = Regex::new(r"^-{3}(\s.*)?$").expect("regex");
        static ref END_RE: Regex = Regex::new(r"^(-{3}|\.{3})(\s.*)?$").expect("regex");
    }

    let mut lines: Vec<&str> = txt.lines().collect();
    let mut meta: HashMap<String, Vec<String>> = HashMap::new();
    let mut key: Option<String> = None;

    if let Some(first) = lines.first() {
        if BEGIN_RE.is_match(first) {
            lines.remove(0);
        }
    }

    while !lines.is_empty() {
        let line = lines.remove(0);
        if line.trim().is_empty() || END_RE.is_match(line) {
            break;
        }
        if let Some(caps) = META_RE.captures(line) {
            let k = caps["key"].to_lowercase();
            let v = caps["value"].trim().to_string();
            meta.entry(k.clone()).or_default().push(v);
            key = Some(k);
        } else if let Some(caps) = META_MORE_RE.captures(line) {
            if let Some(k) = &key {
                meta.entry(k.clone())
                    .or_default()
                    .push(caps["value"].trim().to_string());
            } else {
                lines.insert(0, line);
                break;
            }
        } else {
            lines.insert(0, line);
            break;
        }
    }

    (meta, lines.join("\n"))
}

/// Port of `calibre.db.write.get_series_values`.
fn get_series_values(val: &str) -> (String, Option<f64>) {
    lazy_static! {
        static ref SERIES_INDEX_PAT: Regex = Regex::new(r"^(.*)\s+\[([.0-9]+)\]$").expect("regex");
    }
    let trimmed = val.trim();
    if let Some(caps) = SERIES_INDEX_PAT.captures(trimmed) {
        if let Ok(idx) = caps[2].parse::<f64>() {
            return (caps[1].trim().to_string(), Some(idx));
        }
    }
    (val.to_string(), None)
}

/// Best-effort stand-in for `calibre.utils.date.parse_only_date`, which
/// is not part of this issue's scope. Tries a handful of common
/// formats and a bare 4-digit year; anything else is treated the way
/// Python's `except Exception: continue` treats a parse failure --
/// silently skipped, leaving the field unset.
fn parse_meta_date(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    const FORMATS: &[&str] = &["%Y-%m-%d", "%Y/%m/%d", "%d %B %Y", "%B %d, %Y", "%B %d %Y"];
    for fmt in FORMATS {
        if let Ok(d) = NaiveDate::parse_from_str(raw, fmt) {
            if let Some(dt) = d.and_hms_opt(0, 0, 0) {
                return Some(Utc.from_utc_datetime(&dt));
            }
        }
    }
    if let Ok(year) = raw.parse::<i32>() {
        if (1..=9999).contains(&year) {
            if let chrono::LocalResult::Single(dt) = Utc.with_ymd_and_hms(year, 1, 1, 0, 0, 0) {
                return Some(dt);
            }
        }
    }
    None
}

/// Port of `convert_markdown_with_metadata`.
///
/// Field handling matches the Python for every field it actually reads
/// (`title authors series tags pubdate comments publisher rating`):
/// `authors`/`tags` are multi-valued (`mi.metadata_for_field(k)['is_multiple']`
/// is true only for those two among this set), everything else takes
/// the first front-matter value. `series` splits a trailing `[N]` via
/// [`get_series_values`]; `rating` is clamped to `0..=10` and truncated
/// via `int(float(val))`; `pubdate` uses [`parse_meta_date`] (see its
/// docs for how it differs from the real `parse_only_date`).
pub fn convert_markdown_with_metadata(
    txt: &str,
    title: &str,
    extensions: &[&str],
) -> (MetaInformation, String) {
    let mut extensions: Vec<&str> = extensions.to_vec();
    if !extensions.iter().any(|e| e.eq_ignore_ascii_case("meta")) {
        extensions.push("meta");
    }

    let (mut meta, body) = parse_markdown_meta(txt);
    let html_body = markdown_to_html(&body, &extensions);

    let mut mi = MetaInformation {
        title: if title.is_empty() {
            "Unknown".to_string()
        } else {
            title.to_string()
        },
        ..MetaInformation::default()
    };

    for (src, dst) in [("date", "pubdate"), ("summary", "comments")] {
        if !meta.contains_key(dst) {
            if let Some(v) = meta.remove(src) {
                meta.insert(dst.to_string(), v);
            }
        }
    }

    for key in [
        "title",
        "authors",
        "series",
        "tags",
        "pubdate",
        "comments",
        "publisher",
        "rating",
    ] {
        let Some(values) = meta.get(key) else {
            continue;
        };
        if values.is_empty() {
            continue;
        }
        match key {
            "authors" => mi.authors = values.clone(),
            "tags" => mi.tags = values.clone(),
            "title" => mi.title = values[0].clone(),
            "series" => {
                let (name, idx) = get_series_values(&values[0]);
                mi.series = Some(name);
                mi.series_index = idx.unwrap_or(1.0);
            }
            "publisher" => mi.publisher = Some(values[0].clone()),
            "comments" => mi.comments = Some(values[0].clone()),
            "rating" => {
                if let Ok(f) = values[0].parse::<f64>() {
                    mi.rating = Some(f.trunc().clamp(0.0, 10.0));
                }
            }
            "pubdate" => {
                if let Some(dt) = parse_meta_date(&values[0]) {
                    mi.pubdate = Some(dt);
                }
            }
            _ => {}
        }
    }

    let html = html_template(&mi.title, &html_body);
    (mi, html)
}

/// Port of `convert_textile`: wired directly to the real ported
/// PyTextile converter (issue #53) rather than reimplementing it.
pub fn convert_textile(txt: &str, title: &str) -> String {
    let html = crate::textile::functions::textile(txt, 0, "xhtml");
    html_template(title, &html)
}

/// Port of `normalize_line_endings`.
pub fn normalize_line_endings(txt: &str) -> String {
    txt.replace("\r\n", "\n").replace('\r', "\n")
}

/// Port of `separate_paragraphs_single_line`.
pub fn separate_paragraphs_single_line(txt: &str) -> String {
    txt.replace('\n', "\n\n")
}

/// Port of `separate_paragraphs_print_formatted`.
pub fn separate_paragraphs_print_formatted(txt: &str) -> String {
    lazy_static! {
        static ref RE: FancyRegex =
            FancyRegex::new(r"(?m)^(?P<indent>\t+|[ ]{2,})(?=.)").expect("regex");
    }
    RE.replace_all(txt, |caps: &fancy_regex::Captures<'_, str>| {
        format!("\n{}", &caps["indent"])
    })
    .into_owned()
}

/// Port of `separate_hard_scene_breaks`. See the module docs for the
/// preserved "huge ASCII range" character-class quirk.
pub fn separate_hard_scene_breaks(txt: &str) -> String {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(?m)^[ \t-=~/_]+$").expect("regex");
    }
    RE.replace_all(txt, |caps: &regex::Captures| {
        let line = &caps[0];
        if !line.trim().is_empty() {
            format!("\n{line}\n")
        } else {
            line.to_string()
        }
    })
    .into_owned()
}

/// Port of `block_to_single_line`.
pub fn block_to_single_line(txt: &str) -> String {
    lazy_static! {
        static ref RE: FancyRegex = FancyRegex::new(r"(?<=.)\n(?=.)").expect("regex");
    }
    RE.replace_all(txt, " ").into_owned()
}

/// Port of `preserve_spaces`.
pub fn preserve_spaces(txt: &str) -> String {
    lazy_static! {
        static ref SPACE_RE: Regex = Regex::new(r"[ ]{2,}").expect("regex");
    }
    let txt = SPACE_RE
        .replace_all(txt, |caps: &regex::Captures| {
            let len = caps[0].chars().count();
            format!(" {}", "&nbsp;".repeat(len - 1))
        })
        .into_owned();
    txt.replace('\t', "&nbsp;&nbsp;&nbsp;&nbsp;")
}

/// Port of `remove_indents`.
pub fn remove_indents(txt: &str) -> String {
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(?m)^[\r\t\x0c\x0b ]+").expect("regex");
    }
    RE.replace_all(txt, "").into_owned()
}

/// Narrow port of `calibre.ebooks.conversion.preprocess.DocAnalysis`
/// for `format='txt'` only. See the module docs.
struct DocAnalysis {
    lines: Vec<String>,
}

impl DocAnalysis {
    /// Port of `DocAnalysis.__init__` for `format='txt'`: `raw.replace('&nbsp;',
    /// ' ')`, then every match of `.*?\n` -- each line up to and
    /// including its trailing `\n`. A final line with no trailing
    /// newline is dropped, exactly as the Python regex would drop it.
    fn new_txt(raw: &str) -> Self {
        let raw = raw.replace("&nbsp;", " ");
        let mut lines = Vec::new();
        let mut current = String::new();
        for ch in raw.chars() {
            current.push(ch);
            if ch == '\n' {
                lines.push(std::mem::take(&mut current));
            }
        }
        DocAnalysis { lines }
    }

    /// Port of `DocAnalysis.line_histogram`.
    fn line_histogram(&self, percent: f64) -> bool {
        const MIN_LINE_LENGTH: usize = 20;
        const MAX_LINE_LENGTH: usize = 1900;
        const BUCKETS: usize = 20;

        let mut buckets = [0u32; BUCKETS];
        for line in &self.lines {
            let l = line.chars().count();
            if l > MIN_LINE_LENGTH && l < MAX_LINE_LENGTH {
                let bucket = l / 100;
                if bucket < BUCKETS {
                    buckets[bucket] += 1;
                }
            }
        }

        let total_lines = self.lines.len();
        if total_lines == 0 {
            return false;
        }
        let max_value = buckets
            .iter()
            .map(|&c| c as f64 / total_lines as f64)
            .fold(0.0_f64, f64::max);

        max_value >= percent
    }
}

/// Port of `detect_paragraph_type`. Returns one of `"block"`,
/// `"single"`, `"print"`, `"unformatted"`.
pub fn detect_paragraph_type(txt: &str) -> &'static str {
    lazy_static! {
        static ref NON_EMPTY_LINE: Regex = Regex::new(r"(?m)^\s*.+$").expect("regex");
        static ref TAB_LINE: Regex = Regex::new(r"(?m)^(\t|\s{2,}).+$").expect("regex");
        static ref EMPTY_LINE: Regex = Regex::new(r"(?m)^\s*$").expect("regex");
    }

    let txt = txt.replace("\r\n", "\n").replace('\r', "\n");
    let txt_line_count = NON_EMPTY_LINE.find_iter(&txt).count() as f64;

    let hardbreaks = DocAnalysis::new_txt(&txt).line_histogram(0.55);

    if hardbreaks {
        let tab_line_count = TAB_LINE.find_iter(&txt).count() as f64;
        let print_percent = tab_line_count / txt_line_count;

        let empty_line_count = EMPTY_LINE.find_iter(&txt).count() as f64;
        let block_percent = empty_line_count / txt_line_count;

        if print_percent >= block_percent {
            if (0.15..=0.75).contains(&print_percent) {
                return "print";
            }
        } else if (0.15..=0.75).contains(&block_percent) {
            return "block";
        }

        return "unformatted";
    }

    "single"
}

/// Port of `detect_formatting_type`. Returns `"markdown"`, `"textile"`,
/// or `"heuristic"`. See the module docs for the preserved
/// line-count-inflation quirk in the markdown "links" count.
pub fn detect_formatting_type(txt: &str) -> &'static str {
    lazy_static! {
        static ref MD_HEADING_HASH: Regex = Regex::new(r"(?m)^#+").expect("regex");
        static ref MD_HEADING_EQ: Regex = Regex::new(r"(?m)^=+$").expect("regex");
        static ref MD_HEADING_DASH: Regex = Regex::new(r"(?m)^-+$").expect("regex");
        static ref MD_IMAGE: Regex = Regex::new(r"!\[.*?\](\[|\()").expect("regex");
        // Preserved quirk: `^` here always matches at every line start
        // in addition to the intended link pattern -- see module docs.
        static ref MD_LINK: Regex = Regex::new(r"(?m)^|[^!]\[.*?\](\[|\()").expect("regex");
        static ref TX_HEADING: Regex = Regex::new(r"(?m)^h[1-6]\.").expect("regex");
        static ref TX_BLOCKQUOTE: Regex = Regex::new(r"(?m)^bq\.").expect("regex");
        static ref TX_IMAGE: FancyRegex = FancyRegex::new(r"(?<=\!)\S+(?=\!)").expect("regex");
        static ref TX_LINK: Regex = Regex::new("\"[^\"]*\":\\S+").expect("regex");
        static ref TX_PARA: Regex = Regex::new(r"(?m)^p(<|<>|=|>)?\. ").expect("regex");
    }

    let mut markdown_count = 0usize;
    markdown_count += MD_HEADING_HASH.find_iter(txt).count();
    markdown_count += MD_HEADING_EQ.find_iter(txt).count();
    markdown_count += MD_HEADING_DASH.find_iter(txt).count();
    markdown_count += MD_IMAGE.find_iter(txt).count();
    markdown_count += MD_LINK.find_iter(txt).count();

    let mut textile_count = 0usize;
    textile_count += TX_HEADING.find_iter(txt).count();
    textile_count += TX_BLOCKQUOTE.find_iter(txt).count();
    textile_count += TX_IMAGE.find_iter(txt).flatten().count();
    textile_count += TX_LINK.find_iter(txt).count();
    textile_count += TX_PARA.find_iter(txt).count();

    if markdown_count > 5 || textile_count > 5 {
        if markdown_count > textile_count {
            "markdown"
        } else {
            "textile"
        }
    } else {
        "heuristic"
    }
}

const OEB_IMAGE_MIME_TYPES: [&str; 4] = ["image/gif", "image/jpeg", "image/png", "image/svg+xml"];

fn is_oeb_image(path: &str) -> bool {
    mime_guess::from_path(path)
        .first()
        .map(|m| OEB_IMAGE_MIME_TYPES.contains(&m.essence_str()))
        .unwrap_or(false)
}

/// Port of `get_images_from_polyglot_text`.
///
/// The markdown branch needs a real markdown-to-HTML pass (reused from
/// [`markdown_to_html`]) followed by a real HTML parse to find `<img
/// src>` -- done with `crate::dom::Dom` (`html5ever`-backed),
/// matching this crate's established "parse HTML, don't regex-scrape
/// it" convention for exactly this kind of task.
pub fn get_images_from_polyglot_text(txt: &str, base_dir: &str, file_ext: &str) -> HashSet<String> {
    lazy_static! {
        // Port of the textile image-reference regex. Needs `fancy_regex`
        // for the trailing `(?=\s|$)` lookahead.
        static ref TEXTILE_IMAGE_RE: FancyRegex = FancyRegex::new(
            r"(?m)(?:[\[{])?\!(?:\. )?(?P<path>[^\s(!]+)\s?(?:\(([^)]+)\))?\!(?::(\S+))?(?:[\]}]|(?=\s|$))"
        ).expect("regex");
    }

    let base_dir: PathBuf = if base_dir.is_empty() {
        std::env::current_dir().unwrap_or_default()
    } else {
        PathBuf::from(base_dir)
    };

    let mut images = HashSet::new();
    let mut check_path = |path: &str| {
        if path.is_empty() || Path::new(path).is_absolute() || !is_oeb_image(path) {
            return;
        }
        if base_dir.join(path).exists() {
            images.insert(path.to_string());
        }
    };

    if matches!(file_ext, "txt" | "text" | "textile") {
        for caps in TEXTILE_IMAGE_RE.captures_iter(txt).flatten() {
            if let Some(m) = caps.name("path") {
                check_path(m.as_str());
            }
        }
    }

    if matches!(file_ext, "txt" | "text" | "md" | "markdown") {
        let html = html_template("", &markdown_to_html(txt, &[]));
        let dom = crate::dom::Dom::parse(&html);
        for id in dom.find_all_tag_global("img") {
            if let Some(src) = dom.node(id).attrs.get("src").cloned() {
                check_path(&src);
            }
        }
    }

    images
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_txt_strips_trailing_whitespace_and_condenses_leading() {
        let out = clean_txt("line one   \n\tindented\nnormal  spaced");
        assert!(!out.contains("one   \n"), "{out}");
        assert!(out.contains("&nbsp;&nbsp;&nbsp;&nbsp;indented"), "{out}");
        assert!(out.contains("normal spaced"), "{out}");
    }

    #[test]
    fn clean_txt_condenses_excessive_blank_lines() {
        let out = clean_txt("a\n\n\n\n\n\n\n\nb");
        assert_eq!(out, "a\n\n\n\nb");
    }

    #[test]
    fn clean_txt_strips_illegal_ascii_control_chars() {
        let out = clean_txt("a\u{0}b");
        assert_eq!(out, "ab");
    }

    #[test]
    fn split_utf8_never_splits_a_multibyte_character() {
        // "café" is c-a-f-é, é is 2 bytes (0xc3 0xa9) -> 5 bytes total.
        let s = "café".as_bytes();
        let chunks = split_utf8(s, 4).unwrap();
        for chunk in &chunks {
            assert!(
                std::str::from_utf8(chunk).is_ok(),
                "{chunk:?} not valid utf8"
            );
        }
        let rejoined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(rejoined, s);
    }

    #[test]
    fn split_utf8_handles_a_4_byte_character_at_the_boundary() {
        // U+1F600 GRINNING FACE is 4 bytes.
        let s = "ab\u{1F600}cd".as_bytes();
        for n in 3..s.len() {
            let chunks = split_utf8(s, n).unwrap();
            for chunk in &chunks {
                assert!(std::str::from_utf8(chunk).is_ok(), "n={n} {chunk:?}");
            }
            let rejoined: Vec<u8> = chunks.into_iter().flatten().collect();
            assert_eq!(rejoined, s, "n={n}");
        }
    }

    #[test]
    fn split_utf8_rejects_too_small_a_chunk_size() {
        assert!(split_utf8(b"hello", 2).is_err());
    }

    #[test]
    fn convert_basic_wraps_lines_in_paragraphs() {
        let html = convert_basic("Hello world.\nSecond line.", "T", 0);
        assert!(html.contains("<p>Hello world.</p>"), "{html}");
        assert!(html.contains("<p>Second line.</p>"), "{html}");
        assert!(html.contains("<title>T </title>"), "{html}");
    }

    #[test]
    fn convert_basic_condenses_double_blank_lines_to_nbsp() {
        let html = convert_basic("one\n\n\ntwo", "", 0);
        assert!(html.contains("<p>&nbsp;</p>"), "{html}");
    }

    #[test]
    fn convert_markdown_produces_wrapped_html() {
        let html = convert_markdown("# Heading\n\nSome *text*.", "Doc", DEFAULT_MD_EXTENSIONS);
        assert!(html.contains("<h1>Heading</h1>"), "{html}");
        assert!(html.contains("<em>text</em>"), "{html}");
        assert!(html.starts_with("<html>"), "{html}");
    }

    #[test]
    fn convert_markdown_with_metadata_parses_front_matter() {
        let txt = "Title: My Book\nAuthors: Jane Doe\nAuthors: John Roe\nTags: fiction\nTags: drama\n\n# Chapter One\n\nText here.";
        let (mi, html) = convert_markdown_with_metadata(txt, "", DEFAULT_MD_EXTENSIONS);
        assert_eq!(mi.title, "My Book");
        assert_eq!(
            mi.authors,
            vec!["Jane Doe".to_string(), "John Roe".to_string()]
        );
        assert_eq!(mi.tags, vec!["fiction".to_string(), "drama".to_string()]);
        assert!(html.contains("<h1>Chapter One</h1>"), "{html}");
        assert!(
            !html.contains("Title:"),
            "front matter should be consumed: {html}"
        );
    }

    #[test]
    fn convert_markdown_with_metadata_parses_series_and_rating() {
        let txt = "Series: My Series [3]\nRating: 8.7\n\nBody text.";
        let (mi, _html) = convert_markdown_with_metadata(txt, "", DEFAULT_MD_EXTENSIONS);
        assert_eq!(mi.series.as_deref(), Some("My Series"));
        assert_eq!(mi.series_index, 3.0);
        assert_eq!(mi.rating, Some(8.0));
    }

    #[test]
    fn convert_markdown_with_metadata_continuation_lines_add_list_entries() {
        let txt = "Authors: Jane Doe\n    John Roe\n\nBody.";
        let (mi, _html) = convert_markdown_with_metadata(txt, "", DEFAULT_MD_EXTENSIONS);
        assert_eq!(
            mi.authors,
            vec!["Jane Doe".to_string(), "John Roe".to_string()]
        );
    }

    #[test]
    fn convert_textile_produces_wrapped_html() {
        let html = convert_textile("h1. Header\n\nSome *bold* text.", "T");
        assert!(html.contains("<h1>Header</h1>"), "{html}");
        assert!(html.contains("<strong>bold</strong>"), "{html}");
    }

    #[test]
    fn normalize_line_endings_converts_everything_to_unix() {
        assert_eq!(normalize_line_endings("a\r\nb\rc\nd"), "a\nb\nc\nd");
    }

    #[test]
    fn separate_paragraphs_single_line_doubles_newlines() {
        assert_eq!(separate_paragraphs_single_line("a\nb\nc"), "a\n\nb\n\nc");
    }

    #[test]
    fn separate_paragraphs_print_formatted_inserts_blank_before_indents() {
        let out = separate_paragraphs_print_formatted("normal\n\tindented text\nmore");
        assert!(out.contains("normal\n\n\tindented text"), "{out}");
    }

    #[test]
    fn separate_hard_scene_breaks_wraps_break_lines() {
        let out = separate_hard_scene_breaks("para one\n----\npara two");
        assert!(out.contains("para one\n\n----\n\npara two"), "{out}");
    }

    #[test]
    fn block_to_single_line_joins_wrapped_lines() {
        assert_eq!(block_to_single_line("one\ntwo\nthree"), "one two three");
    }

    #[test]
    fn preserve_spaces_converts_runs_of_spaces_to_nbsp() {
        assert_eq!(preserve_spaces("a   b"), "a &nbsp;&nbsp;b");
        assert_eq!(preserve_spaces("a\tb"), "a&nbsp;&nbsp;&nbsp;&nbsp;b");
    }

    #[test]
    fn remove_indents_strips_leading_whitespace_per_line() {
        assert_eq!(remove_indents("  a\n\tb\nc"), "a\nb\nc");
    }

    #[test]
    fn detect_paragraph_type_single_line_paragraphs() {
        let txt = "Line one.\nLine two.\nLine three.";
        assert_eq!(detect_paragraph_type(txt), "single");
    }

    #[test]
    fn detect_paragraph_type_block_paragraphs() {
        // `line_histogram` needs >=55% of *all* lines -- including the
        // blank-line separators it also counts -- to land in one length
        // bucket. A single long line per paragraph (the original
        // fixture here) can supply at most 50% once the blank
        // separators are counted, so `hardbreaks` never trips and the
        // real algorithm returns "single" for it, not "block": genuine
        // block-formatted text is *hard-wrapped* (several same-length
        // lines per paragraph), which is what actually clears the
        // threshold.
        let wrapped_line =
            |n: u32| format!("Line {n} of this hard-wrapped paragraph stays a consistent length.");
        let mut doc = String::new();
        for _ in 0..10 {
            for i in 0..5 {
                doc.push_str(&wrapped_line(i));
                doc.push('\n');
            }
            doc.push('\n');
        }
        assert_eq!(detect_paragraph_type(&doc), "block");
    }

    #[test]
    fn detect_paragraph_type_print_formatted() {
        // "print" format indents only each paragraph's *first* line;
        // continuation lines aren't indented. The original fixture
        // indented every line, so `print_percent` came out to 1.0 --
        // above the `0.75` ceiling the algorithm checks -- and it fell
        // through to "unformatted" instead.
        let para_start =
            |n: u32| format!("\tParagraph {n} begins here and runs for a while before wrapping.");
        let cont_line =
            |n: u32| format!("continuation line number {n} of the same paragraph keeps going.");
        let mut doc = String::new();
        for p in 0..10 {
            doc.push_str(&para_start(p));
            doc.push('\n');
            for i in 0..4 {
                doc.push_str(&cont_line(i));
                doc.push('\n');
            }
        }
        assert_eq!(detect_paragraph_type(&doc), "print");
    }

    #[test]
    fn detect_formatting_type_recognizes_markdown() {
        let txt = "# Heading one\n## Heading two\n### Heading three\n#### Heading four\n##### Heading five\n###### Heading six\n";
        assert_eq!(detect_formatting_type(txt), "markdown");
    }

    #[test]
    fn detect_formatting_type_recognizes_textile() {
        // The markdown "links" pattern inflates `markdown_count` by one
        // per line regardless of content (see the module docs' preserved
        // quirk); with six lines here that's already 7 (six line starts
        // plus the zero-width match at the very end of the trailing
        // `\n`). A single textile link only earns one point back, which
        // isn't enough to overtake it -- two links on the last line
        // (still one `MD_LINK` line-start hit, but two real `TX_LINK`
        // hits) is what actually tips the count in textile's favor.
        let txt = "h1. One\nh2. Two\nh3. Three\nbq. Quoted\np<>. Justified\n\"a link\":http://example.com \"another link\":http://example.org\n";
        assert_eq!(detect_formatting_type(txt), "textile");
    }

    #[test]
    fn detect_formatting_type_falls_back_to_heuristic() {
        assert_eq!(
            detect_formatting_type("Just some plain prose.\nNo markup here at all."),
            "heuristic"
        );
    }

    #[test]
    fn get_images_from_polyglot_text_finds_markdown_images_that_exist_on_disk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cover.png"), b"png").unwrap();
        let txt = "# Title\n\n![alt](cover.png)\n";
        let images = get_images_from_polyglot_text(txt, tmp.path().to_str().unwrap(), "md");
        assert!(images.contains("cover.png"), "{images:?}");
    }

    #[test]
    fn get_images_from_polyglot_text_ignores_images_that_do_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let txt = "![alt](missing.png)\n";
        let images = get_images_from_polyglot_text(txt, tmp.path().to_str().unwrap(), "md");
        assert!(images.is_empty(), "{images:?}");
    }

    #[test]
    fn get_images_from_polyglot_text_finds_textile_images() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("pic.jpg"), b"jpg").unwrap();
        let txt = "!pic.jpg! some text";
        let images = get_images_from_polyglot_text(txt, tmp.path().to_str().unwrap(), "textile");
        assert!(images.contains("pic.jpg"), "{images:?}");
    }
}
