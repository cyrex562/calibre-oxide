//! OEB/XHTML -> plain text (the default TXT output flavor).
//!
//! Port of `old_src/src/calibre/ebooks/txt/txtml.py`'s `TXTMLizer`.
//! Unlike `markdownml.rs`/`textileml.rs`, the Python does not subclass
//! `OEB2HTML` at all -- it is a standalone class, closer in shape to
//! `crate::rb::rbml::RbMlizer`.
//!
//! # A real, distinctive design choice: flatten whitespace before
//! walking the tree
//!
//! `mlize_spine` re-serializes each spine file's whole XML tree to a
//! string, runs [`TxtMlizer::remove_newlines`] over *all of it*
//! (collapsing the source markup's own indentation/pretty-printing
//! newlines to spaces), and only then re-parses and walks it. This is
//! different from `markdownml.rs`/`textileml.rs`, which run their own
//! `remove_newlines` per text/tail fragment during the walk. Ported as
//! written: this crate's `OEBBook` stores each spine item's content as
//! the raw bytes read from its container path (there is no live
//! in-memory tree the way Python's `item.data` is), so the port reads
//! the raw string directly, applies the same XML-comment
//! `--`-to-`__` substitution the Python does before its own
//! re-serialize (real XML forbids `--` inside a comment, so this
//! avoids ever producing something that wouldn't reparse), flattens
//! newlines, and reparses -- functionally the same pipeline, adapted to
//! not needing a round-trip through a tree that already exists as text
//! here.
//!
//! # Preserved upstream quirk
//!
//! **`cleanup_text` "collapses runs of tabs/vertical-tabs/form-feeds"
//! with a plain string replace, not a regex.** The Python is
//! `text.replace('\t+', ' ')` (and the same for `\v+`/`\f+`) -- `str.replace`,
//! not `re.sub`. `'\t+'` is the two-character string *tab-then-plus-sign*,
//! not "one or more tabs"; a call that was clearly meant to be
//! `re.sub(r'\t+', ' ', text)`. Since a literal tab immediately followed
//! by a literal `+` essentially never occurs in real text, this line is
//! a near-total no-op in practice. Ported via the identical literal
//! (non-regex) replacement, not "fixed" to the evidently-intended
//! `re.sub`.

use std::sync::OnceLock;

use fancy_regex::Regex as FancyRegex;
use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use crate::oeb::toc::TOCNode;
use crate::txt::processor::python_splitlines;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Port of `BLOCK_TAGS`.
const BLOCK_TAGS: &[&str] = &["div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "tr"];
/// Port of `BLOCK_STYLES`.
const BLOCK_STYLES: &[&str] = &["block"];
/// Port of `HEADING_TAGS`.
const HEADING_TAGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6"];
/// Port of `SPACE_TAGS`.
const SPACE_TAGS: &[&str] = &["td", "br"];

/// Options `extract_content`/`cleanup_text` read. Port of the subset of
/// `opts` `txtml.py` actually touches.
#[derive(Debug, Clone, Default)]
pub struct TxtMlOptions {
    pub inline_toc: bool,
    pub remove_paragraph_spacing: bool,
    /// `0` disables line wrapping (Python's `if self.opts.max_line_length`
    /// falsy check).
    pub max_line_length: usize,
    pub force_max_line_length: bool,
}

/// Port of `calibre.ebooks.txt.txtml.TXTMLizer`.
#[derive(Debug, Default)]
pub struct TxtMlizer {
    toc_titles: Vec<String>,
    toc_ids: Vec<String>,
    last_was_heading: bool,
}

impl TxtMlizer {
    /// See `MarkdownMlizer::MAX_EM` in [`crate::txt::markdownml`] for
    /// why this clamp exists despite upstream `txtml.py` having none:
    /// an unclamped `margin_top / font_size` can hit `inf`/`NaN` (e.g.
    /// `font-size: 0`) and, unlike Python's `ZeroDivisionError`, Rust's
    /// saturating float-to-int cast turns that into a `.repeat()` call
    /// that tries to allocate an unbounded string.
    const MAX_EM: i64 = 10;

    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `extract_content`.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &TxtMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        self.toc_titles.clear();
        self.toc_ids.clear();
        self.last_was_heading = false;
        let children: Vec<TOCNode> = oeb.toc.root.children.clone();
        self.create_flat_toc(&children);
        self.mlize_spine(oeb, opts, stylizer)
    }

    /// Port of `create_flat_toc`.
    fn create_flat_toc(&mut self, nodes: &[TOCNode]) {
        for item in nodes {
            self.toc_titles.push(item.title.clone().unwrap_or_default());
            self.toc_ids.push(item.href.clone().unwrap_or_default());
            self.create_flat_toc(&item.children);
        }
    }

    /// Port of `mlize_spine`.
    fn mlize_spine(
        &mut self,
        oeb: &OEBBook,
        opts: &TxtMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        let mut output = self.get_toc(opts);
        for spine_item in &oeb.spine.items {
            let Some(item) = oeb.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = oeb.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let content = escape_comment_double_dashes(&content);
            let content = Self::remove_newlines(&content);
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            if let Some(body) = find_body(&doc) {
                let mut out = Vec::new();
                self.dump_text(body, opts, stylizer, &item.href, &mut out);
                output.push_str(&out.concat());
            }
            output.push_str("\n\n\n\n\n\n");
        }
        let output = python_splitlines(&output)
            .iter()
            .map(|l| l.trim_end())
            .collect::<Vec<_>>()
            .join("\n");
        self.cleanup_text(&output, opts)
    }

    /// Port of `remove_newlines`.
    fn remove_newlines(text: &str) -> String {
        static MULTI_SPACE: OnceLock<Regex> = OnceLock::new();
        let multi_space = MULTI_SPACE.get_or_init(|| Regex::new(r"[ ]{2,}").expect("regex"));
        let text = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
        multi_space.replace_all(&text, " ").into_owned()
    }

    /// Port of `get_toc`.
    fn get_toc(&self, opts: &TxtMlOptions) -> String {
        let mut toc = String::new();
        if opts.inline_toc {
            toc.push_str("Table of Contents:\n\n");
            for title in &self.toc_titles {
                toc.push_str(&format!("* {title}\n\n"));
            }
        }
        toc
    }

    /// Port of `cleanup_text`.
    fn cleanup_text(&self, text: &str, opts: &TxtMlOptions) -> String {
        static SINGLE_LINE: OnceLock<FancyRegex> = OnceLock::new();
        static MULTI_SPACE: OnceLock<Regex> = OnceLock::new();
        static BLANK_WITH_SPACES: OnceLock<Regex> = OnceLock::new();
        static COLLAPSE_BLANKS_SPACED: OnceLock<Regex> = OnceLock::new();
        static LINE_TO_PARA: OnceLock<Regex> = OnceLock::new();
        static PARA_GAP: OnceLock<FancyRegex> = OnceLock::new();
        static EXCESS_BLANK: OnceLock<Regex> = OnceLock::new();
        static LEADING_SPACES: OnceLock<Regex> = OnceLock::new();
        static TRAILING_SPACES: OnceLock<Regex> = OnceLock::new();
        static LEADING_DOC_WS: OnceLock<Regex> = OnceLock::new();

        let single_line =
            SINGLE_LINE.get_or_init(|| FancyRegex::new(r"(?<=.)\n(?=.)").expect("regex"));
        let multi_space = MULTI_SPACE.get_or_init(|| Regex::new(r"[ ]{2,}").expect("regex"));
        let blank_with_spaces =
            BLANK_WITH_SPACES.get_or_init(|| Regex::new(r"\n[ ]+\n").expect("regex"));
        let collapse_2_plus =
            COLLAPSE_BLANKS_SPACED.get_or_init(|| Regex::new(r"\n{2,}").expect("regex"));
        let line_to_para =
            LINE_TO_PARA.get_or_init(|| Regex::new(r"(?msu)^(?P<t>[^\t\n]+?)$").expect("regex"));
        let para_gap = PARA_GAP.get_or_init(|| {
            FancyRegex::new(r"(?msu)(?P<b>[^\n])\n+(?P<t>[^\t\n]+?)(?=\n)").expect("regex")
        });
        let excess_blank = EXCESS_BLANK.get_or_init(|| Regex::new(r"\n{7,}").expect("regex"));
        let leading_spaces =
            LEADING_SPACES.get_or_init(|| Regex::new(r"(?imu)^[ ]+").expect("regex"));
        let trailing_spaces =
            TRAILING_SPACES.get_or_init(|| Regex::new(r"(?imu)[ ]+$").expect("regex"));
        let leading_doc_ws =
            LEADING_DOC_WS.get_or_init(|| Regex::new(r"(?u)^[ \n]+").expect("regex"));

        // Preserved quirk: literal string replacement, not a regex --
        // see the module docs.
        let text = text
            .replace('\u{a0}', " ")
            .replace("\t+", " ")
            .replace("\u{0B}+", " ")
            .replace("\u{0C}+", " ");

        let text = single_line.replace_all(&text, " ").into_owned();
        let text = multi_space.replace_all(&text, " ").into_owned();
        let mut text = blank_with_spaces.replace_all(&text, "\n\n").into_owned();

        if opts.remove_paragraph_spacing {
            text = collapse_2_plus.replace_all(&text, "\n").into_owned();
            text = line_to_para
                .replace_all(&text, |caps: &regex::Captures| {
                    format!("{}\n\n", &caps["t"])
                })
                .into_owned();
            text = para_gap
                .replace_all(&text, |caps: &fancy_regex::Captures<'_, str>| {
                    format!("{}\n\n\n\n\n\n{}", &caps["b"], &caps["t"])
                })
                .into_owned();
        } else {
            text = excess_blank.replace_all(&text, "\n\n\n\n\n\n").into_owned();
        }

        let text = leading_spaces.replace_all(&text, "").into_owned();
        let text = trailing_spaces.replace_all(&text, "").into_owned();
        let mut text = leading_doc_ws.replace_all(&text, "").into_owned();

        if opts.max_line_length > 0 {
            let mut max_length = opts.max_line_length;
            if max_length < 25 && !opts.force_max_line_length {
                max_length = 25;
            }
            let mut short_lines = Vec::new();
            for line in python_splitlines(&text) {
                short_lines.extend(wrap_line(line, max_length, opts.force_max_line_length));
            }
            text = short_lines.join("\n");
        }

        text
    }

    /// Port of `dump_text`.
    fn dump_text(
        &mut self,
        elem: Node,
        opts: &TxtMlOptions,
        stylizer: &dyn StyleProvider,
        page_href: &str,
        out: &mut Vec<String>,
    ) {
        if !elem.is_element() {
            return;
        }
        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        let style = stylizer.style(elem);
        if matches!(
            style.display.as_str(),
            "none" | "oeb-page-head" | "oeb-page-foot"
        ) || style.visibility == "hidden"
        {
            if let Some(tail) = tail_text(elem) {
                out.push(tail);
            }
            return;
        }

        let tag = elem.tag_name().name();
        // Python formats a missing id as the literal string `"None"`
        // (an f-string with `tag_id=None`); replicated so the toc-id
        // membership check is byte-for-byte the same (and just as
        // unlikely to spuriously match).
        let tag_id = elem.attribute("id").unwrap_or("None");
        let toc_key = format!("{page_href}#{tag_id}");

        let mut in_block = false;
        let mut in_heading = false;

        if HEADING_TAGS.contains(&tag) || self.toc_ids.iter().any(|id| id == &toc_key) {
            in_heading = true;
            if !self.last_was_heading {
                out.push("\n\n\n\n\n\n".to_string());
            }
        }

        if BLOCK_TAGS.contains(&tag) || BLOCK_STYLES.contains(&style.display.as_str()) {
            if opts.remove_paragraph_spacing && !in_heading {
                out.push("\t".to_string());
            }
            in_block = true;
        }

        if SPACE_TAGS.contains(&tag) {
            out.push(" ".to_string());
        }

        if tag == "hr" {
            out.push("\n\n* * *\n\n".to_string());
        }

        let ems = ((style.margin_top / style.font_size).round() as i64 - 1).min(Self::MAX_EM);
        if ems >= 1 {
            out.push("\n".repeat(ems as usize));
        }

        if let Some(text) = own_text(elem) {
            out.push(text);
        }

        for child in elem.children() {
            self.dump_text(child, opts, stylizer, page_href, out);
        }

        if in_block {
            out.push("\n\n".to_string());
        }
        if in_heading {
            out.push("\n".to_string());
            self.last_was_heading = true;
        } else {
            self.last_was_heading = false;
        }

        if let Some(tail) = tail_text(elem) {
            out.push(tail);
        }
    }
}

/// Split `line` (already known to exceed `max_length` characters --
/// Python counts codepoints via `len(str)`, so this indexes by `char`,
/// not byte) into wrapped pieces, breaking at the last space at or
/// before `max_length`, then (with `force`) at exactly `max_length`,
/// then at the first space after it, then not at all. Port of the
/// `while len(line) > max_length` loop body in `cleanup_text`.
fn wrap_line(line: &str, max_length: usize, force: bool) -> Vec<String> {
    let mut chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    while chars.len() > max_length {
        if let Some(idx) = chars[..max_length].iter().rposition(|&c| c == ' ') {
            out.push(chars[..idx].iter().collect());
            chars = chars[idx + 1..].to_vec();
        } else if force {
            out.push(chars[..max_length].iter().collect());
            chars = chars[max_length..].to_vec();
        } else if let Some(rel) = chars[max_length..].iter().position(|&c| c == ' ') {
            let idx = max_length + rel;
            out.push(chars[..idx].iter().collect());
            chars = chars[idx + 1..].to_vec();
        } else {
            out.push(chars.iter().collect());
            chars.clear();
        }
    }
    out.push(chars.iter().collect());
    out
}

/// Port of the `--` -> `__` XML-comment substitution `mlize_spine`
/// applies before re-serializing (real XML disallows `--` inside a
/// comment). No lookaround needed: `(?s)` (DOTALL) plus a non-greedy
/// body handles multi-line comments.
fn escape_comment_double_dashes(content: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)<!--(.*?)-->").expect("regex"));
    re.replace_all(content, |caps: &regex::Captures| {
        format!("<!--{}-->", caps[1].replace("--", "__"))
    })
    .into_owned()
}

fn find_body<'a, 'input>(doc: &'a Document<'input>) -> Option<Node<'a, 'input>> {
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "body")
        .or_else(|| Some(doc.root_element()))
}

fn own_text(elem: Node) -> Option<String> {
    let first = elem.first_child()?;
    if first.is_text() {
        first.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

fn tail_text(elem: Node) -> Option<String> {
    let next = elem.next_sibling()?;
    if next.is_text() {
        next.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use crate::oeb::stylizer::Stylizer as ConcreteStylizer;
    use anyhow::Result;
    use std::collections::HashMap as Map;

    #[derive(Default)]
    struct MemContainer(Map<String, Vec<u8>>);

    impl Container for MemContainer {
        fn read(&self, path: &str) -> Result<Vec<u8>> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such part: {path}"))
        }
        fn write(&mut self, path: &str, data: &[u8]) -> Result<()> {
            self.0.insert(path.to_string(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn namelist(&self) -> Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    fn book(html: &str) -> OEBBook {
        let mut container = MemContainer::default();
        container
            .0
            .insert("index.html".to_string(), html.as_bytes().to_vec());
        let mut oeb = OEBBook::new(Box::new(container));
        oeb.manifest.items.insert(
            "item0".to_string(),
            ManifestItem::new("item0", "index.html", "application/xhtml+xml"),
        );
        oeb.spine.items.push(SpineItem::new("item0", true));
        oeb
    }

    fn convert(html: &str, opts: &TxtMlOptions) -> String {
        let oeb = book(html);
        TxtMlizer::new().extract_content(&oeb, opts, &TagStylizer)
    }

    #[test]
    fn paragraphs_are_separated_by_blank_lines() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>one</p><p>two</p></body></html>"#;
        let out = convert(html, &TxtMlOptions::default());
        assert!(out.contains("one"), "{out}");
        assert!(out.contains("two"), "{out}");
        assert!(out.contains("\n\n"), "{out}");
    }

    #[test]
    fn hr_produces_a_scene_break() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>a</p><hr/><p>b</p></body></html>"#;
        let out = convert(html, &TxtMlOptions::default());
        assert!(out.contains("* * *"), "{out}");
    }

    #[test]
    fn hidden_content_is_dropped() {
        // `TagStylizer` (what `convert()` uses) only knows tag-name
        // defaults, not inline `style="..."` -- it can't see
        // `display: none` here, so this needs the real `Stylizer`.
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>keep</p><p style="display: none">drop</p></body></html>"#;
        let oeb = book(html);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out = TxtMlizer::new().extract_content(&oeb, &TxtMlOptions::default(), &stylizer);
        assert!(out.contains("keep"), "{out}");
        assert!(!out.contains("drop"), "{out}");
    }

    #[test]
    fn inline_toc_only_emitted_when_requested() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>x</p></body></html>"#;
        let out_off = convert(html, &TxtMlOptions::default());
        assert!(!out_off.contains("Table of Contents"), "{out_off}");
        let opts = TxtMlOptions {
            inline_toc: true,
            ..Default::default()
        };
        let out_on = convert(html, &opts);
        assert!(out_on.contains("Table of Contents:"), "{out_on}");
    }

    #[test]
    fn max_line_length_wraps_long_lines_at_a_space() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>a text line that is definitely much longer than the configured maximum length setting here</p></body></html>"#;
        let opts = TxtMlOptions {
            max_line_length: 30,
            ..Default::default()
        };
        let out = convert(html, &opts);
        for line in out.lines() {
            assert!(line.chars().count() <= 30, "{line:?} in {out:?}");
        }
    }

    #[test]
    fn wrap_line_breaks_at_the_last_space_within_bounds() {
        // `max_length` 8 would put "world foo" (9 chars) back over the
        // limit after the first break, triggering a second break at
        // its own last space -- correct per the port's (and upstream
        // `txtml.py`'s) `while len(line) > max_length` loop, just not
        // what this test means to exercise. 11 lets "hello" break off
        // at the last space within bounds while leaving "world foo"
        // (9 chars) under the limit, so it isn't broken again.
        let wrapped = wrap_line("hello world foo", 11, false);
        assert_eq!(wrapped, vec!["hello".to_string(), "world foo".to_string()]);
    }

    #[test]
    fn wrap_line_force_breaks_when_no_space_and_forced() {
        let wrapped = wrap_line("abcdefghij", 4, true);
        assert_eq!(
            wrapped,
            vec!["abcd".to_string(), "efgh".to_string(), "ij".to_string()]
        );
    }

    #[test]
    fn comment_double_dashes_are_escaped() {
        let out = escape_comment_double_dashes("<!-- a -- b -->text");
        assert_eq!(out, "<!-- a __ b -->text");
    }
}
