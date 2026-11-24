//! OEB/XHTML -> PML markup.
//!
//! Port of `old_src/src/calibre/ebooks/pml/pmlml.py`'s `PMLMLizer`: the
//! reverse of [`crate::pml::pmlconverter`], walking a spine's XHTML
//! trees and emitting PML markup. Same "walk a tag stack, consult a
//! stylizer per element, emit markup strings" shape as
//! `crate::fb2::fb2ml::Fb2Mlizer` (issue #26) -- this module mirrors
//! that one's structure, including reusing
//! [`crate::oeb::stylizer::StyleProvider`] as the style seam rather
//! than inventing a second one.
//!
//! # Where the trait's shape falls short
//!
//! `dump_text` also wants two things `ResolvedStyle` does not carry:
//!
//! - `text-align` (for the `\c`/`\r` center/right-align codes): pulled
//!   out of `ResolvedStyle::css_text` by hand ([`inline_style_prop`]),
//!   since that field already carries the raw inline `style="..."`
//!   text for exactly this kind of one-off property a converter needs
//!   but the shared trait doesn't promote to a first-class field.
//! - `margin-left`, converted into a `\T="N%"` percent-of-page-height
//!   indent by the Python (`style['margin-left'] * 100 / style.height`,
//!   where `style.height` is the *page* height, not the element's).
//!   `ResolvedStyle` has no notion of page height at all (neither does
//!   `TagStylizer`, nor the general-purpose `Stylizer` -- both work
//!   per-element, not per-page-layout), so this is not implemented;
//!   text that would have gotten a `\T` indent from its margin is left
//!   unindented. This mirrors the FB2 port's own documented choice not
//!   to invent answers the shared stylizer cannot honestly give.
//!
//! # Preserved upstream quirk
//!
//! [`PmlMlizer::dump_text`]'s image-naming call passes
//! `self.image_hrefs.keys()` (already-seen *hrefs*) as the
//! `taken_names` argument to `image_name`, not
//! `self.image_hrefs.values()` (the already-*assigned PML names*, e.g.
//! `"3.png"`). Since hrefs (`images/foo.png`) essentially never equal a
//! generated numeric name (`"3.png"`), the dedup loop inside
//! `image_name` almost never actually triggers. Ported as-is.

use std::collections::HashMap;

use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use crate::pdb::ereader::image_name;
use crate::pml::unipmlcode;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Port of `TAG_MAP`.
fn tag_map(tag: &str) -> Option<&'static str> {
    Some(match tag {
        "b" | "strong" => "B",
        "i" => "i",
        "small" => "k",
        "sub" => "Sb",
        "sup" => "Sp",
        "big" => "l",
        "del" => "o",
        "h1" => "x",
        "h2" => "X0",
        "h3" => "X1",
        "h4" => "X2",
        "h5" => "X3",
        "h6" => "X4",
        _ => return None,
    })
}

/// Port of `STYLES`, in the Python's declared order (checked in that
/// order, and skipped if the resulting code is already open).
const STYLE_PROPS: &[&str] = &["font-weight", "font-style", "text-decoration", "text-align"];

fn style_tag_for(prop: &str, val: &str) -> Option<&'static str> {
    match (prop, val) {
        ("font-weight", "bold") | ("font-weight", "bolder") => Some("B"),
        ("font-style", "italic") => Some("i"),
        ("text-decoration", "underline") => Some("u"),
        ("text-align", "right") => Some("r"),
        ("text-align", "center") => Some("c"),
        _ => None,
    }
}

const BLOCK_TAGS: &[&str] = &["p", "div"];
const LINK_TAGS: &[&str] = &["a"];
const IMAGE_TAGS: &[&str] = &["img"];

/// Options `dump_text`/`clean_text` read. Port of the subset of `opts`
/// `pmlml.py` actually touches.
#[derive(Debug, Clone, Default)]
pub struct PmlOptions {
    pub remove_paragraph_spacing: bool,
}

/// Port of `calibre.ebooks.pml.pmlml.PMLMLizer`.
#[derive(Debug, Default)]
pub struct PmlMlizer {
    /// href -> assigned PML image name (`"cover.png"`, `"3.png"`, ...).
    pub image_hrefs: HashMap<String, String>,
    /// `"href#fragment"` -> `"calibre_link-N"`.
    link_hrefs: HashMap<String, String>,
    /// href -> fragment (`""` for none) -> (title, depth).
    toc: HashMap<String, HashMap<String, (String, usize)>>,
}

impl PmlMlizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `extract_content` + `pmlmlize_spine`.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &PmlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        self.toc.clear();
        self.create_flat_toc(&oeb.toc.root.children, 0);

        self.image_hrefs.clear();
        self.link_hrefs.clear();

        let mut output = String::new();
        output.push_str(&self.get_cover_page(oeb, stylizer));
        output.push_str(&self.get_text(oeb, opts, stylizer));
        clean_text(&output, opts.remove_paragraph_spacing)
    }

    /// Port of `create_flat_toc`.
    fn create_flat_toc(&mut self, nodes: &[crate::oeb::toc::TOCNode], level: usize) {
        for item in nodes {
            let Some(full) = item.href.as_deref() else {
                continue;
            };
            let (href, id) = match full.split_once('#') {
                Some((h, id)) => (h.to_string(), id.to_string()),
                None => (full.to_string(), String::new()),
            };
            self.get_anchor_id(&href, &id);
            self.toc
                .entry(href)
                .or_default()
                .insert(id, (item.title.clone().unwrap_or_default(), level));
            self.create_flat_toc(&item.children, level + 1);
        }
    }

    /// Port of `get_cover_page`.
    fn get_cover_page(&mut self, oeb: &OEBBook, stylizer: &dyn StyleProvider) -> String {
        let mut output = String::new();
        if let Some(cover) = oeb.guide.get("cover") {
            output.push_str("\\m=\"cover.png\"\n");
            self.image_hrefs
                .insert(cover.href.clone(), "cover.png".to_string());
        }
        if let Some(titlepage) = oeb.guide.get("titlepage") {
            let href = titlepage.href.clone();
            if let Some(item) = oeb.manifest.get_by_href(&href) {
                let in_spine = oeb.spine.items.iter().any(|si| si.idref == item.id);
                if !in_spine {
                    if let Ok(raw) = oeb.container.read(&item.href) {
                        let content = String::from_utf8_lossy(&raw).into_owned();
                        if let Ok(doc) = Document::parse(&content) {
                            if let Some(body) = find_body(&doc) {
                                let mut out = Vec::new();
                                self.dump_text(body, stylizer, &item.href, &[], &mut out);
                                output.push_str(&out.concat());
                            }
                        }
                    }
                }
            }
        }
        output
    }

    /// Port of `get_text`.
    fn get_text(
        &mut self,
        oeb: &OEBBook,
        _opts: &PmlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        let mut text = String::new();
        for spine_item in &oeb.spine.items {
            let Some(item) = oeb.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = oeb.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let content = prepare_text(&content);
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            text.push_str(&self.add_page_anchor(&item.href));
            if let Some(body) = find_body(&doc) {
                let mut out = Vec::new();
                self.dump_text(body, stylizer, &item.href, &[], &mut out);
                text.push_str(&out.concat());
            }
        }
        text
    }

    /// Port of `add_page_anchor`.
    fn add_page_anchor(&mut self, page_href: &str) -> String {
        self.get_anchor(page_href, "")
    }

    /// Port of `get_anchor_id`.
    fn get_anchor_id(&mut self, href: &str, aid: &str) -> String {
        let key = format!("{href}#{aid}");
        if !self.link_hrefs.contains_key(&key) {
            let n = self.link_hrefs.len();
            self.link_hrefs
                .insert(key.clone(), format!("calibre_link-{n}"));
        }
        self.link_hrefs[&key].clone()
    }

    /// Port of `get_anchor`.
    fn get_anchor(&mut self, page_href: &str, aid: &str) -> String {
        let id = self.get_anchor_id(page_href, aid);
        format!("\\Q=\"{id}\"")
    }

    /// Port of `dump_text`. Unlike `Fb2Mlizer::dump_text`, this takes
    /// no `oeb: &OEBBook` -- Python's version doesn't either (`page`
    /// alone, via `page.abshref`, is enough; image `src`s are recorded
    /// as seen, with no manifest-membership check).
    fn dump_text(
        &mut self,
        elem: Node,
        stylizer: &dyn StyleProvider,
        page_href: &str,
        tag_stack: &[String],
        out: &mut Vec<String>,
    ) {
        if !elem.is_element() {
            return;
        }
        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            if let Some(tail) = tail_text(elem) {
                out.push(prepare_string_for_pml(&tail));
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
                out.push(prepare_string_for_pml(&tail));
            }
            return;
        }

        let mut tags: Vec<String> = Vec::new();
        let tag = elem.tag_name().name().to_string();
        let open_tags: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();

        if BLOCK_TAGS.contains(&tag.as_str()) || style.display == "block" {
            tags.push("block".to_string());
        }

        if IMAGE_TAGS.contains(&tag.as_str()) {
            if let Some(src) = elem.attribute("src").filter(|s| !s.is_empty()) {
                let href = resolve_href(page_href, src);
                if !self.image_hrefs.contains_key(&href) {
                    if self.image_hrefs.is_empty() {
                        self.image_hrefs
                            .insert(href.clone(), "cover.png".to_string());
                    } else {
                        let taken: Vec<String> = self.image_hrefs.keys().cloned().collect();
                        let candidate = format!("{}.png", self.image_hrefs.len());
                        let field = image_name(&candidate, &taken);
                        let name: String = field
                            .iter()
                            .take_while(|&&b| b != 0)
                            .map(|&b| b as char)
                            .collect();
                        self.image_hrefs.insert(href.clone(), name);
                    }
                }
                out.push(format!("\\m=\"{}\"", self.image_hrefs[&href]));
            }
        } else if tag == "hr" {
            let width = elem.attribute("width");
            let w = match width {
                Some(w) if w.ends_with('%') => w.to_string(),
                Some(w) => format!("{w}%"),
                None => "50%".to_string(),
            };
            out.push(format!("\\w=\"{w}\""));
        } else if tag == "br" {
            out.push("\n\\c \n\\c\n".to_string());
        }

        // TOC markers: only if the tag isn't a heading and we aren't
        // already inside a `\x`/`\X[0-4]` span.
        let toc_name = elem.attribute("name").map(str::to_string);
        let toc_id = elem.attribute("id").map(str::to_string);
        let in_heading = open_tags
            .iter()
            .chain(tags.iter())
            .any(|t| matches!(t.as_str(), "x" | "X0" | "X1" | "X2" | "X3" | "X4"));
        let has_marker = toc_name.as_deref().is_some_and(|s| !s.is_empty())
            || toc_id.as_deref().is_some_and(|s| !s.is_empty());
        if has_marker
            && !matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6")
            && !in_heading
        {
            if let Some(entries) = self.toc.get(page_href) {
                for probe in [&toc_name, &toc_id].into_iter().flatten() {
                    if let Some((title, depth)) = entries.get(probe) {
                        if !title.is_empty() {
                            let depth = (*depth).min(4);
                            out.push(format!("\\C{depth}=\"{title}\""));
                        }
                    }
                }
            }
        }

        if style.css_text.contains("page-break-before")
            && inline_style_prop(&style.css_text, "page-break-before").as_deref() == Some("always")
        {
            out.push("\\p".to_string());
        }

        if let Some(pml_tag) = tag_map(&tag) {
            if !open_tags.iter().chain(tags.iter()).any(|t| t == pml_tag) {
                out.push(format!("\\{pml_tag}"));
                tags.push(pml_tag.to_string());
            }
        }

        if LINK_TAGS.contains(&tag.as_str())
            && !open_tags.iter().chain(tags.iter()).any(|t| t == "q")
        {
            if let Some(href) = elem.attribute("href") {
                let mut href = resolve_href(page_href, href);
                if !href.contains("://") {
                    if !href.contains('#') {
                        href.push('#');
                    }
                    if !self.link_hrefs.contains_key(&href) {
                        let n = self.link_hrefs.len();
                        self.link_hrefs
                            .insert(href.clone(), format!("calibre_link-{n}"));
                    }
                    let target = format!("#{}", self.link_hrefs[&href]);
                    out.push(format!("\\q=\"{target}\""));
                    tags.push("q".to_string());
                }
            }
        }

        for name_x in [elem.attribute("id"), elem.attribute("name")]
            .into_iter()
            .flatten()
        {
            out.push(self.get_anchor(page_href, name_x));
        }

        let combined: Vec<String> = open_tags.iter().chain(tags.iter()).cloned().collect();
        for prop in STYLE_PROPS {
            let val = match *prop {
                "font-weight" => style.font_weight.clone(),
                "font-style" => style.font_style.clone(),
                "text-decoration" => style.text_decoration.clone(),
                "text-align" => {
                    inline_style_prop(&style.css_text, "text-align").unwrap_or_default()
                }
                _ => unreachable!(),
            };
            if let Some(style_tag) = style_tag_for(prop, &val) {
                if !combined.iter().any(|t| t == style_tag) {
                    out.push(format!("\\{style_tag}"));
                    tags.push(style_tag.to_string());
                }
            }
        }

        // Soft scene breaks from the element's own top margin. Margin-
        // left indentation (`\T="N%"`) is not implemented -- see the
        // module docs.
        if style.font_size > 0.0 {
            let ems = ((style.margin_top / style.font_size) - 1.0).round();
            if ems >= 1.0 {
                out.push("\n\\c \n\\c\n".to_string());
            }
        }

        if let Some(text) = own_text(elem) {
            out.push(prepare_string_for_pml(&text));
        }

        let child_stack: Vec<String> = tag_stack.iter().chain(tags.iter()).cloned().collect();
        for child in elem.children() {
            self.dump_text(child, stylizer, page_href, &child_stack, out);
        }

        tags.reverse();
        out.extend(close_tags(&tags));

        if inline_style_prop(&style.css_text, "page-break-after").as_deref() == Some("always") {
            out.push("\\p".to_string());
        }

        if let Some(tail) = tail_text(elem) {
            out.push(prepare_string_for_pml(&tail));
        }
    }
}

/// Port of `remove_newlines`.
// NOT `.replace(['\r', '\n'], " ")`: replacing the two-char `\r\n`
// sequence first (as one space) before any lone survivor is what keeps
// a Windows line ending from becoming *two* spaces, matching Python's
// three separate `.replace()` calls in the same order.
#[allow(clippy::collapsible_str_replace)]
fn remove_newlines(text: &str) -> String {
    text.replace("\r\n", " ")
        .replace('\n', " ")
        .replace('\r', " ")
}

/// Port of `prepare_string_for_pml`.
fn prepare_string_for_pml(text: &str) -> String {
    let mut text = remove_newlines(text);
    text = text.replace('\\', "\\\\");
    text = text.replace("\\\\c \\\\c", "\\c \n\\c\n");
    text
}

/// Port of `prepare_text`. Python spells the "must directly follow a
/// closed paragraph" test as a fixed-width lookbehind
/// (`(?<=</p>)\s*<p[^>]*>...`), which Rust's `regex` crate cannot
/// express. This walks matches of the un-lookbehind-ed pattern by hand
/// and checks the four bytes before each candidate match against the
/// *original* string -- exactly what a zero-width lookbehind does, and
/// in particular still true for a `</p>` that was itself part of an
/// earlier (already-replaced) match, since matching always looks at the
/// untouched input, matching Python's `re.sub` semantics.
fn prepare_text(text: &str) -> String {
    static TARGET: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = TARGET.get_or_init(|| Regex::new("(?s)\\s*<p[^>]*>[\u{c2}\u{a0}\\s]*</p>").unwrap());

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    loop {
        let Some(m) = re.find_at(text, cursor) else {
            out.push_str(&text[cursor..]);
            break;
        };
        let preceded_by_close_p = m.start() >= 4 && &text[m.start() - 4..m.start()] == "</p>";
        out.push_str(&text[cursor..m.start()]);
        if preceded_by_close_p {
            out.push_str("\\c\n\\c");
        } else {
            out.push_str(&text[m.start()..m.end()]);
        }
        cursor = m.end();
    }
    out
}

/// Extract one declaration's value out of an inline `style="..."`-style
/// CSS text (as carried by `ResolvedStyle::css_text`).
fn inline_style_prop(css_text: &str, prop: &str) -> Option<String> {
    for decl in css_text.split(';') {
        let mut parts = decl.splitn(2, ':');
        let p = parts.next()?.trim();
        let v = parts.next()?.trim();
        if p.eq_ignore_ascii_case(prop) {
            return Some(v.to_string());
        }
    }
    None
}

/// Port of `close_tags`.
fn close_tags(tags: &[String]) -> Vec<String> {
    let mut text = Vec::new();
    for tag in tags {
        if tag == "block" {
            text.push("\n\n".to_string());
        } else if tag == "c" || tag == "r" {
            text.push(format!("\n\\{tag}"));
        } else {
            text.push(format!("\\{tag}"));
        }
    }
    text
}

/// Port of `PMLMLizer.clean_text`.
pub fn clean_text(text: &str, remove_paragraph_spacing: bool) -> String {
    static DOUBLE_P: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let mut text = DOUBLE_P
        .get_or_init(|| Regex::new(r"\\p\s*\\p").unwrap())
        .replace_all(text, "")
        .into_owned();

    static ANCHOR: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let anchor_re = ANCHOR.get_or_init(|| Regex::new(r#"\\Q="(?P<id>.+?)""#).unwrap());
    static LINK: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    // NOT `r#"...#(...)..."#` -- the pattern itself contains a `"#`
    // sequence (`\q="#...`), which would close a single-hash raw
    // string early (the exact bug this crate has hit before). A plain
    // escaped string literal sidesteps the ambiguity entirely.
    let link_re = LINK.get_or_init(|| Regex::new("\\\\q=\"#(?P<id>.+?)\"").unwrap());

    let anchors: std::collections::HashSet<String> = anchor_re
        .captures_iter(&text)
        .map(|c| c["id"].to_string())
        .collect();
    let links: std::collections::HashSet<String> = link_re
        .captures_iter(&text)
        .map(|c| c["id"].to_string())
        .collect();
    for unused in anchors.difference(&links) {
        text = text.replace(&format!("\\Q=\"{unused}\""), "");
    }

    text = strip_nested_toc_markers(&text);

    text = text.replace('\u{c2}', "");
    text = text.replace('\u{a0}', " ");

    text = text
        .chars()
        .map(|c| {
            if c.is_ascii() {
                c.to_string()
            } else {
                unipmlcode(c)
            }
        })
        .collect();

    static LEADING_SPACES: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    text = LEADING_SPACES
        .get_or_init(|| Regex::new(r"(?m)^[ ]+").unwrap())
        .replace_all(&text, "")
        .into_owned();
    static TRAILING_SPACES: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    text = TRAILING_SPACES
        .get_or_init(|| Regex::new(r"(?m)[ ]+$").unwrap())
        .replace_all(&text, "")
        .into_owned();
    static MULTI_SPACE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    text = MULTI_SPACE
        .get_or_init(|| Regex::new(r"[ ]{2,}").unwrap())
        .replace_all(&text, " ")
        .into_owned();

    static SCENE_BREAK_RUN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    text = SCENE_BREAK_RUN
        .get_or_init(|| Regex::new(r"(\\c\s*\\c\s*){2,}").unwrap())
        .replace_all(&text, "\\c \n\\c\n")
        .into_owned();

    static BLANK_LINE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    text = BLANK_LINE
        .get_or_init(|| Regex::new(r"\n[ ]+\n").unwrap())
        .replace_all(&text, "\n\n")
        .into_owned();

    if remove_paragraph_spacing {
        static MULTI_NEWLINE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        text = MULTI_NEWLINE
            .get_or_init(|| Regex::new(r"\n{2,}").unwrap())
            .replace_all(&text, "\n")
            .into_owned();

        static HAS_CODE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let has_code = HAS_CODE.get_or_init(|| Regex::new(r"\\[XxCmrctTp]").unwrap());
        text = text
            .split('\n')
            .map(|line| {
                if line.is_empty() || has_code.is_match(line) {
                    line.to_string()
                } else {
                    format!("        {line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    } else {
        static TRIPLE_NEWLINE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        text = TRIPLE_NEWLINE
            .get_or_init(|| Regex::new(r"\n{3,}").unwrap())
            .replace_all(&text, "\n\n")
            .into_owned();
    }

    text
}

/// Remove a `\C[0-4]="..."` tag nested directly inside a `\x`/`\X[0-4]`
/// marker pair. Port of `clean_text`'s
/// `(?P<t>\\(x|X[0-4]))(?P<a>.*?)(?P<c>\\C[0-4]...)(?P<b>.*?)(?P=t)`
/// substitution, which needs a backreference Rust's `regex` crate does
/// not support.
///
/// This only looks for the `\C` tag between an opening marker and the
/// *immediately next* occurrence of the identical marker text (rather
/// than Python's full backtracking, which could in principle skip past
/// several marker occurrences to find one with a `\C` tag inside it).
/// `dump_text`'s own TOC-marker logic never actually emits a `\C` tag
/// while already inside a `\x`/`\X[0-4]` span (see the "TOC markers"
/// section above, which explicitly excludes headings and anything
/// already inside one), so in the output this function actually runs
/// against, a `\C` tag nested inside a marker pair -- if it occurs at
/// all -- is always the immediate one.
fn strip_nested_toc_markers(text: &str) -> String {
    static CN: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let cn_re = CN.get_or_init(|| Regex::new(r#"(?s)\\C[0-4]\s*=\s*"[^"]*""#).unwrap());

    fn find_marker(s: &str) -> Option<(usize, usize)> {
        let lower_x = s.find("\\x");
        let mut idx = 0;
        let upper_x = loop {
            match s[idx..].find("\\X") {
                None => break None,
                Some(rel) => {
                    let pos = idx + rel;
                    if let Some(d) = s[pos + 2..].chars().next() {
                        if ('0'..='4').contains(&d) {
                            break Some((pos, 3));
                        }
                    }
                    idx = pos + 2;
                }
            }
        };
        match (lower_x, upper_x) {
            (Some(xp), Some((bp, bl))) => {
                if xp <= bp {
                    Some((xp, 2))
                } else {
                    Some((bp, bl))
                }
            }
            (Some(xp), None) => Some((xp, 2)),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    let mut out = String::new();
    let mut rest = text;
    loop {
        match find_marker(rest) {
            None => {
                out.push_str(rest);
                break;
            }
            Some((start, len)) => {
                let marker = rest[start..start + len].to_string();
                let after = &rest[start + len..];
                match after.find(&marker) {
                    None => {
                        out.push_str(&rest[..start + len]);
                        rest = after;
                    }
                    Some(rel_end) => {
                        let inner = &after[..rel_end];
                        out.push_str(&rest[..start]);
                        out.push_str(&marker);
                        if let Some(m) = cn_re.find(inner) {
                            out.push_str(&inner[..m.start()]);
                            out.push_str(&inner[m.end()..]);
                        } else {
                            out.push_str(inner);
                        }
                        out.push_str(&marker);
                        rest = &after[rel_end + marker.len()..];
                    }
                }
            }
        }
    }
    out
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

/// Resolve an `src`/`href` against the page that carried it. Stands in
/// for the Python's `page.abshref`.
fn resolve_href(page_href: &str, src: &str) -> String {
    if src.starts_with('/') || src.contains("://") {
        return src.to_string();
    }
    let base = match page_href.rfind('/') {
        Some(i) => &page_href[..i + 1],
        None => "",
    };
    let joined = format!("{base}{src}");
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
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

    fn book(parts: &[(&str, &str)]) -> OEBBook {
        let mut container = MemContainer::default();
        for (name, content) in parts {
            container
                .0
                .insert((*name).to_string(), content.as_bytes().to_vec());
        }
        let mut oeb = OEBBook::new(Box::new(container));
        for (i, (name, _)) in parts.iter().enumerate() {
            let id = format!("item{i}");
            oeb.manifest.items.insert(
                id.clone(),
                ManifestItem::new(&id, name, "application/xhtml+xml"),
            );
            oeb.manifest.hrefs.insert((*name).to_string(), id.clone());
            oeb.spine.items.push(SpineItem::new(&id, true));
        }
        oeb
    }

    fn convert(oeb: &OEBBook) -> String {
        PmlMlizer::new().extract_content(oeb, &PmlOptions::default(), &TagStylizer)
    }

    const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p>Hello <b>bold</b> and <i>italic</i>.</p>
</body></html>"#;

    #[test]
    fn inline_tags_map_to_pml_codes() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb);
        assert!(out.contains("\\Bbold\\B"), "{out}");
        assert!(out.contains("\\iitalic\\i"), "{out}");
    }

    #[test]
    fn headings_map_to_x_codes() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1><h2>Sub</h2></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        // `TagStylizer` gives headings `font-weight: bold` per the
        // HTML default stylesheet, same as a real browser -- so the
        // STYLES pass also wraps them in `\B`, on top of the TAG_MAP
        // code. Both codes are real PML output, not redundant/wrong.
        assert!(out.contains("\\x\\BTitle\\B\\x"), "{out}");
        assert!(out.contains("\\X0\\BSub\\B\\X0"), "{out}");
    }

    #[test]
    fn external_links_are_dropped_only_internal_ones_get_a_q_code() {
        // Port of `dump_text`'s `if '://' not in href:` guard: pmlml
        // only emits `\q=` for internal ('#'-resolvable) links --
        // unlike FB2 (issue #26), which keeps only *external* links.
        // An external href is simply not turned into any PML code.
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="http://example.com/">link</a></p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(!out.contains("\\q="), "{out}");
        assert!(out.contains("link"), "{out}");
    }

    #[test]
    fn internal_links_resolve_to_a_calibre_link_anchor() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="chapter2.html">next</a></p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("\\q=\"#calibre_link-"), "{out}");
    }

    #[test]
    fn images_get_assigned_sequential_names() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="a.png"/><img src="b.png"/></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let mut mlizer = PmlMlizer::new();
        let out = mlizer.extract_content(&oeb, &PmlOptions::default(), &TagStylizer);
        // First image seen becomes cover.png (no guide cover present).
        assert!(out.contains("\\m=\"cover.png\""), "{out}");
        assert!(out.contains("\\m=\"1.png\""), "{out}");
        assert_eq!(mlizer.image_hrefs.len(), 2);
    }

    #[test]
    fn a_guide_cover_is_assigned_cover_png_up_front() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.png"/></body></html>"#;
        let mut oeb = book(&[("index.html", page)]);
        oeb.guide.add("cover", None, "cover.jpg");
        let out = convert(&oeb);
        assert!(out.contains("\\m=\"cover.png\"\n"), "{out}");
        // The in-body image is a *different* href, so it gets its own
        // sequential name rather than reusing "cover.png".
        assert!(out.contains("\\m=\"1.png\""), "{out}");
    }

    #[test]
    fn horizontal_rules_carry_a_width_code() {
        let page =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><hr width="30%"/></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("\\w=\"30%\""), "{out}");
    }

    #[test]
    fn anchors_without_a_referencing_link_are_dropped_by_clean_text() {
        // Double-hash delimiter: the content itself contains a `"#`
        // sequence (`\q="#used"`), which would close a single-hash raw
        // string early.
        let text = r##"\Q="unused"text\Q="used"more\q="#used"link"##;
        let out = clean_text(text, false);
        assert!(!out.contains("\\Q=\"unused\""), "{out}");
        assert!(out.contains("\\Q=\"used\""), "{out}");
    }

    #[test]
    fn clean_text_condenses_scene_break_runs() {
        let text = "\\c \n\\c\n\\c \n\\c\n\\c \n\\c\n";
        let out = clean_text(text, false);
        assert_eq!(out.matches("\\c").count(), 2, "{out}");
    }

    #[test]
    fn clean_text_converts_non_ascii_via_unipmlcode() {
        let text = "caf\u{e9}";
        let out = clean_text(text, false);
        // e9 (Latin Small Letter E with Acute) is a cp1252 byte < 256,
        // so it should become an `\a` code, not a literal char.
        assert!(out.starts_with("caf\\a"), "{out}");
    }

    #[test]
    fn remove_paragraph_spacing_indents_plain_lines_only() {
        let text = "one\n\n\ntwo\n\\x=\"H\"three\\x";
        let out = clean_text(text, true);
        assert!(out.contains("        one"), "{out}");
        assert!(out.contains("        two"), "{out}");
        // A line carrying a PML code is left unindented.
        assert!(out.contains("\\x=\"H\"three\\x"), "{out}");
        assert!(!out.contains("        \\x"), "{out}");
    }

    #[test]
    fn prepare_text_marks_empty_paragraphs_following_a_closed_one() {
        let html = "<p>a</p><p> </p><p>b</p>";
        let out = prepare_text(html);
        assert!(out.contains("\\c\n\\c"), "{out}");
        // The very first paragraph of the document has no preceding
        // `</p>`, so a leading empty one is left untouched.
        let leading = "<p></p><p>a</p>";
        let out2 = prepare_text(leading);
        assert_eq!(out2, leading);
    }

    #[test]
    fn resolve_href_matches_the_fb2_helper_semantics() {
        assert_eq!(
            resolve_href("text/index.html", "images/a.png"),
            "text/images/a.png"
        );
        assert_eq!(
            resolve_href("text/index.html", "../images/a.png"),
            "images/a.png"
        );
        assert_eq!(
            resolve_href("index.html", "http://x.com/a.png"),
            "http://x.com/a.png"
        );
    }

    // -- round trip: PML -> HTML -> PML ---------------------------------

    #[test]
    fn round_trip_pml_to_html_to_pml_keeps_the_semantic_content() {
        // Not byte-for-byte (PML and HTML aren't a lossless pair --
        // `pml_to_html` turns `\b`/`\i` into `<span style="...">`, not
        // `<b>`/`<i>`, so this needs the *concrete* `Stylizer` (which
        // reads inline `style=` attributes) rather than `TagStylizer`
        // (which only knows tag-name defaults) to see the formatting
        // again on the way back.
        use crate::oeb::stylizer::Stylizer as ConcreteStylizer;
        use crate::pml::pmlconverter::pml_to_html;

        let original = r"before \bbold\b and \iitalic\i after";
        let html = pml_to_html(original);
        let full_html =
            format!("<html xmlns=\"http://www.w3.org/1999/xhtml\"><body>{html}</body></html>");
        let oeb = book(&[("index.html", &full_html)]);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out = PmlMlizer::new().extract_content(&oeb, &PmlOptions::default(), &stylizer);

        assert!(out.contains("before"), "{out}");
        assert!(out.contains("bold"), "{out}");
        assert!(out.contains("\\B"), "{out}");
        assert!(out.contains("italic"), "{out}");
        assert!(out.contains("\\i"), "{out}");
        assert!(out.contains("after"), "{out}");
    }
}
