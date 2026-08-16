//! OEB/XHTML -> SNBC markup.
//!
//! Port of `old_src/src/calibre/ebooks/snb/snbml.py`'s `SNBMLizer` and
//! `ProcessFileName`. Architecturally related to the other
//! tree-walk-and-emit-markup converters ported this session
//! ([`crate::pml::pmlml`], [`crate::rb::rbml`], [`crate::rtf::rtfml`]),
//! but with a real structural difference: instead of one flat markup
//! string, `SNBMLizer` splits a single spine item's content into
//! multiple **SNBC sub-documents** (one per "subitem" -- a
//! chapter/bookmark anchor id, supplied by the caller as an
//! `(anchor_id_or_empty, title)` list).
//!
//! It does this in two passes, both ported here exactly as structured
//! in Python rather than restructured into something cleaner (subtle
//! reordering would silently move lines into the wrong chapter):
//!
//! 1. [`SnbMlizer::dump_text`] walks the XHTML tree once, producing one
//!    big text blob, embedding three sentinel markers
//!    (`CALIBRE_SNB_BM_TAG`/`CALIBRE_SNB_IMG_TAG`/`CALIBRE_SNB_PRE_TAG`)
//!    at bookmark/image/preformatted-text boundaries.
//! 2. `mlize`'s second pass scans that blob line by line, and every
//!    time it sees a `CALIBRE_SNB_BM_TAG`-prefixed line it redirects
//!    all *subsequent* output into a different sub-document, until the
//!    next such line.
//!
//! # `dump_text`'s `(text, li)` return shape, traced
//!
//! Python's `dump_text` is declared to return a `(text, li)` tuple,
//! and both of its call sites (the recursive call in its own children
//! loop, and `mlize`'s top-level call) read only `[0]` -- so `li` is
//! *never* read by any caller anywhere in this file. This port
//! therefore threads `li` as ordinary internal recursion state (it
//! still affects *this* call's own output -- see the `tag == "li"` and
//! own-text/tail handling below) but does not return it at all.
//!
//! There's a sharper subtlety in that same tuple, though: two of
//! `dump_text`'s early-return branches (non-XHTML-namespace content,
//! and hidden/`display:none` content) don't return the `(text, li)`
//! tuple at all -- they return a bare list, `[elem.tail]` or `['']`.
//! Calling `[0]` on *that* yields the tail **string** itself (or `''`),
//! not a list. The caller then does `text += t`: with `t` a list this
//! is a normal extend, but with `t` a bare string, Python's `list +=
//! string` iterates the string and appends it **one character per list
//! entry**. That changes `en = text[-1][-2:]` (the last-two-characters
//! lookback used to decide whether a following block tag needs a
//! leading blank line) for whatever comes after: exploded into single
//! characters, `text[-1]` is at most 1 char long, so `en` can now never
//! equal `"\n\n"`.
//!
//! This port's roxmltree adaptation (`own_text`/`tail_text` helpers,
//! matching `crate::rb::rbml`/`crate::pml::pmlml`'s convention) already
//! iterates *all* child nodes including bare text nodes, filtering them
//! out via `!elem.is_element()` -- unlike lxml, which never hands
//! `dump_text` a text node at all (`for item in elem:` only yields
//! child *elements*). Under this adaptation the "non-XHTML-namespace"
//! branch is hit routinely, for every plain text node -- but that
//! content is *already* fully captured once via the neighboring
//! element's own `own_text`/`tail_text`, so replaying it here (exploded
//! or not) would double it. That branch is therefore a genuine no-op
//! (`vec![]`) in this port, matching `crate::rb::rbml`'s and
//! `crate::pml::pmlml`'s established handling of the same roxmltree
//! quirk -- not an attempt to replay Python's char-exploding.
//!
//! The **hidden/`display:none`** branch is different: it fires on a
//! genuine XHTML *element* with the same 1:1 meaning as the Python, so
//! this port *does* reproduce the character-exploding there (see
//! [`SnbMlizer::dump_text`]'s hidden-element branch) -- it's the one
//! place in this file where the quirk has an observable effect on real
//! output.

use std::sync::OnceLock;

use anyhow::{bail, Result};
use indexmap::IndexMap;
use regex::Regex;
use roxmltree::Node;

pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider};
use crate::xml_util::prepare_string_for_xml;

const CALIBRE_SNB_IMG_TAG: &str = "<$$calibre_snb_temp_img$$>";
const CALIBRE_SNB_BM_TAG: &str = "<$$calibre_snb_bm_tag$$>";
const CALIBRE_SNB_PRE_TAG: &str = "<$$calibre_snb_pre_tag$$>";

const BLOCK_TAGS: &[&str] = &["div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "li", "tr"];
const SPACE_TAGS: &[&str] = &["td"];

/// Whitespace `mlize`'s second pass strips from each line: space, tab,
/// newline, CR, and U+3000 (ideographic space). Port of the literal
/// character set in `line.strip(' \t\n\r　')`.
const STRIP_CHARS: &[char] = &[' ', '\t', '\n', '\r', '\u{3000}'];

/// Port of `ProcessFileName`. Flattens path separators to `_`, strips
/// `#` (HTML bookmark characters), lowercases, and rewrites known
/// raster-image extensions to `.jpg` **regardless of the file's actual
/// encoded content** -- SNB readers apparently only support JPEG, and
/// this function only renames the *reference*; it does not re-encode
/// any bytes. See `crate::output::snb_output` for why real re-encoding
/// is out of scope here (it lives in the separate, unported
/// `conversion/plugins/snb_output.py`'s `HandleImage`, not in this
/// file).
// Two separate `.replace()` calls, matching Python's
// `.replace('/', '_').replace(os.sep, '_')` line-for-line (see
// `crate::pml::pmlml::remove_newlines` for the same precedent in this
// crate: collapsing them into one multi-pattern replace would obscure
// that these are two distinct source statements, not a single
// character-class replace).
#[allow(clippy::collapsible_str_replace)]
pub fn process_file_name(file_name: &str) -> String {
    let mut s = file_name
        .replace('/', "_")
        .replace(std::path::MAIN_SEPARATOR, "_");
    s = s.replace('#', "_");
    s = s.to_lowercase();
    if let Some(dot) = s.rfind('.') {
        // Mirror `os.path.splitext`: a dot that is the very first
        // character of the name (a "hidden file" like `.htaccess`) is
        // not treated as an extension separator.
        if dot > 0 {
            let ext = &s[dot..];
            if matches!(ext, ".jpeg" | ".jpg" | ".gif" | ".svg" | ".png") {
                s = format!("{}.jpg", &s[..dot]);
            }
        }
    }
    s
}

/// Options `cleanup_text`/`mlize` read. Port of the subset of `opts`
/// `snbml.py` actually touches. Defaults match the `snb_output`
/// conversion plugin's `OptionRecommendation` defaults
/// (`old_src/.../conversion/plugins/snb_output.py`) and the generic
/// `max_line_length` option's default
/// (`old_src/.../conversion/plugins/txt_output.py`).
///
/// `force_max_line_length` is deliberately *not* modeled: Python reads
/// it only inside `elif False and self.opts.force_max_line_length:`,
/// whose leading `False and` short-circuits before
/// `force_max_line_length` is ever evaluated, so that branch is
/// permanently dead code. See [`wrap_lines`].
///
/// All fields default to `false`/`0`, which matches every one of the
/// upstream defaults cited above (`bool::default()` is `false`,
/// `usize::default()` is `0`), so this is `#[derive(Default)]` rather
/// than a hand-written impl.
#[derive(Debug, Clone, Default)]
pub struct SnbOptions {
    pub snb_hide_chapter_name: bool,
    pub snb_dont_indent_first_line: bool,
    pub snb_insert_empty_line: bool,
    /// `0` disables line wrapping entirely.
    pub snb_max_line_length: usize,
    /// A *different*, more generic option than `snb_max_line_length`
    /// above -- only used to decide whether to clamp the wrap width up
    /// to a minimum of 25.
    pub max_line_length: usize,
    pub remove_paragraph_spacing: bool,
}

/// One `<text>` or `<img>` element inside a `<snbc><body>`. Port of
/// what `mlize`'s second pass appends via `etree.SubElement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnbcBodyItem {
    /// Rendered as `<text><![CDATA[...]]></text>` -- port of
    /// `etree.SubElement(bodyTree, 'text').text = etree.CDATA(...)`.
    Text(String),
    /// Rendered as `<img>...</img>` (plain, XML-escaped text content,
    /// *not* CDATA) -- port of the `img` element built without
    /// `etree.CDATA`.
    Img(String),
}

/// One SNBC sub-document. Port of one `etree.Element('snbc')` tree
/// from `mlize`'s `trees` dict.
#[derive(Debug, Clone, Default)]
pub struct SnbcDoc {
    pub title: String,
    pub hide_title: bool,
    pub body: Vec<SnbcBodyItem>,
}

impl SnbcDoc {
    /// Serialize to the `<snbc><head>...</head><body>...</body></snbc>`
    /// XML the conversion plugin writes to a `.snbc` file. Not a literal
    /// port of anything in `snbml.py` itself (Python returns live
    /// `etree.Element` trees and leaves serialization to the caller,
    /// `etree.tostring(..., pretty_print=True, encoding='utf-8')`), but
    /// a real, parseable equivalent needed since this crate has no
    /// `lxml`-equivalent tree type to hand back instead.
    pub fn to_xml(&self) -> String {
        let mut s = String::from("<?xml version='1.0' encoding='utf-8'?>\n<snbc>\n  <head>\n");
        s.push_str(&format!(
            "    <title>{}</title>\n",
            prepare_string_for_xml(&self.title, false)
        ));
        if self.hide_title {
            s.push_str("    <hidetitle>true</hidetitle>\n");
        }
        s.push_str("  </head>\n  <body>\n");
        for item in &self.body {
            match item {
                SnbcBodyItem::Text(t) => {
                    s.push_str("    <text><![CDATA[");
                    s.push_str(&escape_cdata(t));
                    s.push_str("]]></text>\n");
                }
                SnbcBodyItem::Img(src) => {
                    s.push_str("    <img>");
                    s.push_str(&prepare_string_for_xml(src, false));
                    s.push_str("</img>\n");
                }
            }
        }
        s.push_str("  </body>\n</snbc>\n");
        s
    }
}

/// Split a `]]>` (which cannot appear literally inside CDATA) into two
/// adjacent CDATA sections. `lxml`'s `etree.CDATA` does not do this
/// escaping either, but this port aims for `to_xml()` to produce
/// genuinely valid, re-parseable XML.
fn escape_cdata(text: &str) -> String {
    text.replace("]]>", "]]]]><![CDATA[>")
}

/// Port of `calibre.ebooks.snb.snbml.SNBMLizer`.
#[derive(Debug, Default)]
pub struct SnbMlizer {
    /// Port of `self.curSubItem`. Python declares this as a class
    /// attribute defaulting to `''` and never assigns it `None`
    /// anywhere in this file, so the `if self.curSubItem is not None`
    /// guard in `dump_text` is always true in practice; this port drops
    /// that always-true guard and keeps a plain `String`.
    cur_sub_item: String,
}

impl SnbMlizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `extract_content` + `mlize`.
    ///
    /// `body` is the spine item's `<body>` element. `item_href` is that
    /// item's own href (used to resolve in-body image paths the same
    /// way `ProcessFileName(os.path.dirname(self.item.href))` does).
    /// `subitems` is the `(anchor_id, title)` bookmark list; it *must*
    /// contain an entry whose id is the empty string (the "preface"
    /// section text falls into before the first real bookmark) --
    /// every real caller in the Python (the `snb_output` conversion
    /// plugin) always supplies one, so this is an enforced-but-real
    /// caller contract rather than a made-up restriction, and mlize()
    /// returns an error rather than silently dropping content if it's
    /// missing.
    pub fn extract_content(
        &mut self,
        body: Node,
        item_href: &str,
        subitems: &[(String, String)],
        stylizer: &dyn StyleProvider,
        opts: &SnbOptions,
    ) -> Result<IndexMap<String, SnbcDoc>> {
        self.cur_sub_item = String::new();
        self.mlize(body, item_href, subitems, stylizer, opts)
    }

    fn mlize(
        &mut self,
        body: Node,
        item_href: &str,
        subitems: &[(String, String)],
        stylizer: &dyn StyleProvider,
        opts: &SnbOptions,
    ) -> Result<IndexMap<String, SnbcDoc>> {
        let mut trees: IndexMap<String, SnbcDoc> = IndexMap::new();
        for (href, title) in subitems {
            trees.insert(
                href.clone(),
                SnbcDoc {
                    title: title.clone(),
                    hide_title: opts.snb_hide_chapter_name,
                    body: Vec::new(),
                },
            );
        }

        let mut output: Vec<String> = vec![String::new()];
        output.push(format!("{CALIBRE_SNB_BM_TAG}\n\n"));
        output.extend(self.dump_text(subitems, body, stylizer, "", false, ""));
        let output = cleanup_text(&output.concat(), opts);

        let mut subitem = String::new();
        if !trees.contains_key(&subitem) {
            bail!(
                "SNB subitems must include an entry with an empty-string href \
                 (the preface section); got: {:?}",
                subitems
            );
        }

        for line in output.lines() {
            if let Some(pos) = line.find(CALIBRE_SNB_PRE_TAG) {
                let content = &line[pos + CALIBRE_SNB_PRE_TAG.len()..];
                body_of(&mut trees, &subitem)?.push(SnbcBodyItem::Text(content.to_string()));
                continue;
            }
            let trimmed = line.trim_matches(STRIP_CHARS);
            if trimmed.is_empty() {
                continue;
            }
            if let Some(rest) = trimmed.strip_prefix(CALIBRE_SNB_IMG_TAG) {
                let dir = item_href.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                let prefix = process_file_name(dir);
                let value = if !prefix.is_empty() {
                    format!("{prefix}_{rest}")
                } else {
                    rest.to_string()
                };
                body_of(&mut trees, &subitem)?.push(SnbcBodyItem::Img(value));
            } else if let Some(rest) = trimmed.strip_prefix(CALIBRE_SNB_BM_TAG) {
                subitem = rest.to_string();
                if !trees.contains_key(&subitem) {
                    bail!("SNB bookmark referenced unknown subitem {subitem:?}");
                }
            } else {
                let prefix = if !opts.snb_dont_indent_first_line {
                    "\u{3000}\u{3000}"
                } else {
                    ""
                };
                body_of(&mut trees, &subitem)?
                    .push(SnbcBodyItem::Text(format!("{prefix}{trimmed}")));
            }
            if opts.snb_insert_empty_line {
                body_of(&mut trees, &subitem)?.push(SnbcBodyItem::Text(String::new()));
            }
        }

        Ok(trees)
    }

    /// Port of `dump_text`. See the module docs for the `(text, li)`
    /// return-shape quirk and how this port's roxmltree adaptation
    /// relates to it.
    fn dump_text(
        &mut self,
        subitems: &[(String, String)],
        elem: Node,
        stylizer: &dyn StyleProvider,
        end: &str,
        pre: bool,
        li: &str,
    ) -> Vec<String> {
        if !elem.is_element() {
            // A bare text/comment/PI node visited directly via the
            // parent's child loop -- already fully captured via
            // `own_text`/`tail_text` on the neighboring element. See
            // the module docs for why this is a true no-op here.
            return Vec::new();
        }

        let mut text: Vec<String> = vec![String::new()];
        let style = stylizer.style(elem);

        if let Some(id) = elem.attribute("id") {
            if subitems.iter().any(|(href, _)| href == id) && self.cur_sub_item != id {
                self.cur_sub_item = id.to_string();
                text.push(format!("\n\n{CALIBRE_SNB_BM_TAG}{}\n\n", self.cur_sub_item));
            }
        }

        if matches!(
            style.display.as_str(),
            "none" | "oeb-page-head" | "oeb-page-foot"
        ) || style.visibility == "hidden"
        {
            // Port of `return [elem.tail]` / `return ['']`: see the
            // module docs -- this branch's tail (if any) is exploded
            // into one fragment per character, exactly reproducing
            // Python's `list += string` behavior for the bare-list
            // return this Python branch takes (unlike the normal
            // `(text, li)` tuple path).
            return match tail_text(elem) {
                Some(tail) if !tail.is_empty() => tail.chars().map(|c| c.to_string()).collect(),
                _ => Vec::new(),
            };
        }

        let tag = elem.tag_name().name();
        let mut in_block = false;
        let mut li = li.to_string();

        if BLOCK_TAGS.contains(&tag) || style.display == "block" {
            in_block = true;
            if !end.ends_with("\n\n") && own_text(elem).is_some() {
                text.push("\n\n".to_string());
            }
        }

        if SPACE_TAGS.contains(&tag) {
            // Port of `if not end.endswith('u ') and ...`. `'u '` (not
            // a single space) is what the Python literally checks --
            // almost never true in real content. Preserved as-is
            // rather than "corrected" to a single-space check.
            if !end.ends_with("u ") && own_text(elem).is_some() {
                text.push(" ".to_string());
            }
        }

        if tag == "img" {
            if let Some(src) = elem.attribute("src") {
                text.push(format!(
                    "\n\n{CALIBRE_SNB_IMG_TAG}{}\n\n",
                    process_file_name(src)
                ));
            }
        }

        if tag == "br" {
            text.push("\n\n".to_string());
        }

        if tag == "li" {
            li = "- ".to_string();
        }

        let pre = tag == "pre" || pre;

        if let Some(own) = own_text(elem) {
            let content = format!("{li}{own}");
            text.push(if pre {
                join_with_pre_tag(&content)
            } else {
                content
            });
            li = String::new();
        }

        for child in elem.children() {
            let en = if text.len() >= 2 {
                last_n_chars(text.last().expect("just checked len >= 2"), 2)
            } else {
                String::new()
            };
            let t = self.dump_text(subitems, child, stylizer, &en, pre, &li);
            text.extend(t);
        }

        if in_block {
            text.push("\n\n".to_string());
        }

        if let Some(tail) = tail_text(elem) {
            text.push(if pre {
                join_with_pre_tag(&tail)
            } else {
                format!("{li}{tail}")
            });
        }

        text
    }
}

fn body_of<'a>(
    trees: &'a mut IndexMap<String, SnbcDoc>,
    key: &str,
) -> Result<&'a mut Vec<SnbcBodyItem>> {
    trees
        .get_mut(key)
        .map(|d| &mut d.body)
        .ok_or_else(|| anyhow::anyhow!("SNB: unknown subitem {key:?}"))
}

/// Port of `(f'\n\n{CALIBRE_SNB_PRE_TAG}').join(text.splitlines())`.
fn join_with_pre_tag(text: &str) -> String {
    let sep = format!("\n\n{CALIBRE_SNB_PRE_TAG}");
    text.lines().collect::<Vec<_>>().join(&sep)
}

/// Last (up to) `n` **characters** (not bytes) of `s`. Port of
/// `s[-n:]`'s Unicode-code-point-based semantics.
fn last_n_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
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

/// Port of `cleanup_text`.
pub fn cleanup_text(text: &str, opts: &SnbOptions) -> String {
    let mut text = text.replace('\u{c2}', "");
    text = text.replace('\u{a0}', " ");
    text = text.replace('\u{a9}', "(C)");

    // Port of `.replace('\t+', ' ')` etc: a plain string `.replace()`,
    // *not* `re.sub()` -- these look for the literal two-character
    // sequence "tab followed by a plus sign" (and vtab/form-feed
    // likewise), which essentially never occurs in real text. Reads
    // like an upstream bug (probably meant `re.sub(r'\t+', ...)` to
    // collapse runs of whitespace), but this is what the Python
    // actually does, so it's ported as the same effectively-dead
    // literal replacement rather than "fixed" into the regex that was
    // probably intended.
    text = text.replace("\t+", " ");
    text = text.replace("\u{b}+", " ");
    text = text.replace("\u{c}+", " ");

    // Port of `re.sub(r'(?<=.)\n(?=.)', ' ', text)` (`os.linesep` is
    // `\n` on the platforms this runs on): join a lone newline into a
    // single space only when it has a non-newline character on *both*
    // sides, leaving blank-line paragraph breaks alone. No lookaround
    // in the `regex` crate, so this is a manual scan (same technique as
    // `crate::pml::pmlml::prepare_text`).
    text = join_single_newlines(&text);

    static BLANK_LINE: OnceLock<Regex> = OnceLock::new();
    text = BLANK_LINE
        .get_or_init(|| Regex::new(r"\n[ ]+\n").unwrap())
        .replace_all(&text, "\n\n")
        .into_owned();

    if opts.remove_paragraph_spacing {
        static MULTI_NEWLINE: OnceLock<Regex> = OnceLock::new();
        text = MULTI_NEWLINE
            .get_or_init(|| Regex::new(r"\n{2,}").unwrap())
            .replace_all(&text, "\n")
            .into_owned();

        // Port of `re.sub(r'(?imu)^(?=.)', '\t', text)`: prefix every
        // *non-empty* line with a tab. No lookaround support, so this
        // splits on the only separator the text can contain at this
        // point (`\n`) and rejoins -- equivalent to the zero-width
        // insert.
        text = text
            .split('\n')
            .map(|line| {
                if line.is_empty() {
                    line.to_string()
                } else {
                    format!("\t{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    } else {
        static TRIPLE_NEWLINE: OnceLock<Regex> = OnceLock::new();
        text = TRIPLE_NEWLINE
            .get_or_init(|| Regex::new(r"\n{3,}").unwrap())
            .replace_all(&text, "\n\n")
            .into_owned();
    }

    static LEADING_SPACES: OnceLock<Regex> = OnceLock::new();
    text = LEADING_SPACES
        .get_or_init(|| Regex::new(r"(?m)^[ ]+").unwrap())
        .replace_all(&text, "")
        .into_owned();
    static TRAILING_SPACES: OnceLock<Regex> = OnceLock::new();
    text = TRAILING_SPACES
        .get_or_init(|| Regex::new(r"(?m)[ ]+$").unwrap())
        .replace_all(&text, "")
        .into_owned();

    if opts.snb_max_line_length > 0 {
        text = wrap_lines(&text, opts);
    }

    text
}

fn join_single_newlines(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            let prev_ok = i > 0 && chars[i - 1] != '\n';
            let next_ok = i + 1 < chars.len() && chars[i + 1] != '\n';
            if prev_ok && next_ok {
                out.push(' ');
                continue;
            }
        }
        out.push(c);
    }
    out
}

/// Port of `cleanup_text`'s `snb_max_line_length` line-wrapping block.
///
/// The Python has an `elif False and self.opts.force_max_line_length:`
/// branch here -- the leading `False and` short-circuits Python's `and`
/// before `force_max_line_length` is ever read, so that branch can
/// never execute. This port omits it entirely (rather than keeping an
/// unreachable `if false` arm) and does not model
/// `force_max_line_length` as an option at all, since nothing can ever
/// observe it; this comment is the trace of that decision. Falls
/// straight through, as the Python does at runtime, to the "find the
/// first space after `max_length`" branch.
///
/// Operates on **characters**, not bytes (Python string indexing is
/// Unicode-code-point-based, and SNB is a CJK-market ebook format where
/// multi-byte content is the common case, not an edge case).
fn wrap_lines(text: &str, opts: &SnbOptions) -> String {
    let mut max_length = opts.snb_max_line_length;
    if opts.max_line_length < 25 {
        max_length = 25;
    }

    let mut short_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let mut chars: Vec<char> = line.chars().collect();
        while chars.len() > max_length {
            let space = chars[..max_length].iter().rposition(|&c| c == ' ');
            if let Some(space) = space {
                short_lines.push(chars[..space].iter().collect());
                chars.drain(..=space);
            } else {
                let space = chars[max_length..]
                    .iter()
                    .position(|&c| c == ' ')
                    .map(|rel| rel + max_length);
                if let Some(space) = space {
                    short_lines.push(chars[..space].iter().collect());
                    chars.drain(..=space);
                } else {
                    short_lines.push(chars.iter().collect());
                    chars.clear();
                }
            }
        }
        short_lines.push(chars.iter().collect());
    }
    short_lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::stylizer::TagStylizer;
    use roxmltree::Document;

    fn find_body<'a, 'input>(doc: &'a Document<'input>) -> Node<'a, 'input> {
        doc.descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "body")
            .unwrap()
    }

    fn convert(
        html: &str,
        item_href: &str,
        subitems: &[(String, String)],
        opts: &SnbOptions,
    ) -> IndexMap<String, SnbcDoc> {
        let doc = Document::parse(html).unwrap();
        let body = find_body(&doc);
        SnbMlizer::new()
            .extract_content(body, item_href, subitems, &TagStylizer, opts)
            .unwrap()
    }

    fn preface() -> Vec<(String, String)> {
        vec![(String::new(), "Chapter".to_string())]
    }

    #[test]
    fn simple_paragraph_lands_in_the_preface_subitem() {
        let html =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello world</p></body></html>"#;
        let trees = convert(html, "index.html", &preface(), &SnbOptions::default());
        let doc = &trees[""];
        assert!(doc
            .body
            .iter()
            .any(|i| matches!(i, SnbcBodyItem::Text(t) if t.contains("Hello world"))));
    }

    #[test]
    fn indents_first_line_by_default() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#;
        let trees = convert(html, "index.html", &preface(), &SnbOptions::default());
        let doc = &trees[""];
        assert!(doc.body.iter().any(
            |i| matches!(i, SnbcBodyItem::Text(t) if t.starts_with('\u{3000}') && t.contains("Hello"))
        ));
    }

    #[test]
    fn snb_dont_indent_first_line_suppresses_the_indent() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#;
        let opts = SnbOptions {
            snb_dont_indent_first_line: true,
            ..Default::default()
        };
        let trees = convert(html, "index.html", &preface(), &opts);
        let doc = &trees[""];
        assert!(
            doc.body
                .iter()
                .any(|i| matches!(i, SnbcBodyItem::Text(t) if t == "Hello")),
            "{:?}",
            doc.body
        );
    }

    #[test]
    fn multiple_subitems_split_content_at_bookmark_ids() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
            <p id="ch1">First chapter text</p>
            <p id="ch2">Second chapter text</p>
        </body></html>"#;
        let subitems = vec![
            (String::new(), "Preface".to_string()),
            ("ch1".to_string(), "Chapter 1".to_string()),
            ("ch2".to_string(), "Chapter 2".to_string()),
        ];
        let trees = convert(html, "index.html", &subitems, &SnbOptions::default());

        assert_eq!(trees.len(), 3);
        let ch1_text: Vec<_> = trees["ch1"]
            .body
            .iter()
            .filter_map(|i| match i {
                SnbcBodyItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            ch1_text.iter().any(|t| t.contains("First chapter")),
            "{ch1_text:?}"
        );
        let ch2_text: Vec<_> = trees["ch2"]
            .body
            .iter()
            .filter_map(|i| match i {
                SnbcBodyItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            ch2_text.iter().any(|t| t.contains("Second chapter")),
            "{ch2_text:?}"
        );
        // Nothing after the bookmarks should have leaked back into the
        // preface subitem.
        let preface_has_chapter_text = trees[""].body.iter().any(
            |i| matches!(i, SnbcBodyItem::Text(t) if t.contains("First chapter") || t.contains("Second chapter")),
        );
        assert!(!preface_has_chapter_text);
    }

    #[test]
    fn images_are_recorded_with_a_dirname_prefix() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.PNG"/></body></html>"#;
        let trees = convert(html, "text/index.html", &preface(), &SnbOptions::default());
        let doc = &trees[""];
        assert!(
            doc.body
                .iter()
                .any(|i| matches!(i, SnbcBodyItem::Img(src) if src == "text_pic.jpg")),
            "{:?}",
            doc.body
        );
    }

    #[test]
    fn images_with_no_directory_component_get_no_prefix() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.png"/></body></html>"#;
        let trees = convert(html, "index.html", &preface(), &SnbOptions::default());
        let doc = &trees[""];
        assert!(
            doc.body
                .iter()
                .any(|i| matches!(i, SnbcBodyItem::Img(src) if src == "pic.jpg")),
            "{:?}",
            doc.body
        );
    }

    #[test]
    fn preformatted_text_is_preserved_line_by_line() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><pre>line one
line two</pre></body></html>"#;
        let trees = convert(html, "index.html", &preface(), &SnbOptions::default());
        let doc = &trees[""];
        let texts: Vec<&str> = doc
            .body
            .iter()
            .filter_map(|i| match i {
                SnbcBodyItem::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("line one")), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("line two")), "{texts:?}");
    }

    #[test]
    fn snb_insert_empty_line_adds_a_blank_text_after_each_line() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hello</p></body></html>"#;
        let opts = SnbOptions {
            snb_insert_empty_line: true,
            ..Default::default()
        };
        let trees = convert(html, "index.html", &preface(), &opts);
        let doc = &trees[""];
        assert!(
            doc.body
                .iter()
                .any(|i| matches!(i, SnbcBodyItem::Text(t) if t.is_empty())),
            "{:?}",
            doc.body
        );
    }

    #[test]
    fn missing_preface_subitem_is_an_error() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Hi</p></body></html>"#;
        let doc = Document::parse(html).unwrap();
        let body = find_body(&doc);
        let subitems = vec![("ch1".to_string(), "Chapter 1".to_string())];
        let result = SnbMlizer::new().extract_content(
            body,
            "index.html",
            &subitems,
            &TagStylizer,
            &SnbOptions::default(),
        );
        assert!(result.is_err());
    }

    // -- process_file_name -----------------------------------------

    #[test]
    fn process_file_name_flattens_and_lowercases() {
        assert_eq!(
            process_file_name("Text/Chapter1.HTML"),
            "text_chapter1.html"
        );
    }

    #[test]
    fn process_file_name_strips_bookmark_hash() {
        assert_eq!(process_file_name("chapter1.html#top"), "chapter1.html_top");
    }

    #[test]
    fn process_file_name_rewrites_raster_extensions_to_jpg() {
        for (input, expected) in [
            ("a.jpeg", "a.jpg"),
            ("a.JPG", "a.jpg"),
            ("a.gif", "a.jpg"),
            ("a.svg", "a.jpg"),
            ("a.png", "a.jpg"),
        ] {
            assert_eq!(process_file_name(input), expected, "input={input}");
        }
    }

    #[test]
    fn process_file_name_leaves_other_extensions_alone() {
        assert_eq!(process_file_name("book.snbf"), "book.snbf");
    }

    // -- cleanup_text -------------------------------------------------

    #[test]
    fn cleanup_text_joins_single_newlines_but_keeps_blank_lines() {
        let out = cleanup_text("a\nb\n\nc", &SnbOptions::default());
        assert_eq!(out, "a b\n\nc");
    }

    #[test]
    fn cleanup_text_collapses_triple_newlines_by_default() {
        let out = cleanup_text("a \n\n\n\n b", &SnbOptions::default());
        // The leading/trailing space handling also strips per-line
        // spaces; assert on the newline collapsing specifically.
        assert!(!out.contains("\n\n\n"), "{out:?}");
    }

    #[test]
    fn cleanup_text_remove_paragraph_spacing_indents_with_tabs() {
        let opts = SnbOptions {
            remove_paragraph_spacing: true,
            ..Default::default()
        };
        let out = cleanup_text("a\n\n\nb", &opts);
        assert_eq!(out, "\ta\n\tb");
    }

    #[test]
    fn cleanup_text_wraps_long_lines_on_a_space_boundary() {
        let opts = SnbOptions {
            snb_max_line_length: 10,
            max_line_length: 30, // >= 25, so the 25-floor clamp does not kick in
            ..Default::default()
        };
        let out = cleanup_text("hello there long line", &opts);
        for line in out.lines() {
            assert!(line.chars().count() <= 10, "{line:?} in {out:?}");
        }
        assert_eq!(out.replace('\n', " "), "hello there long line");
    }

    #[test]
    fn cleanup_text_wrap_clamps_short_max_line_length_up_to_25() {
        let opts = SnbOptions {
            snb_max_line_length: 5,
            max_line_length: 0, // < 25, so max_length is clamped to 25
            ..Default::default()
        };
        let out = cleanup_text("a short line under 25 chars", &opts);
        // Clamped to 25: the 28-char input isn't split at width 5.
        assert!(out.lines().count() <= 2, "{out:?}");
    }

    #[test]
    fn cleanup_text_disabled_when_snb_max_line_length_is_zero() {
        let long = "a".repeat(200);
        let out = cleanup_text(&long, &SnbOptions::default());
        assert_eq!(out, long);
    }
}
