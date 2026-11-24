//! OEB/XHTML -> Markdown-formatted plain text.
//!
//! Port of `old_src/src/calibre/ebooks/txt/markdownml.py`'s
//! `MarkdownMLizer`. Same "walk a spine's XHTML trees with a
//! [`StyleProvider`], maintain a per-element tag stack, emit markup
//! strings" shape as `crate::rb::rbml::RbMlizer`/`crate::pml::pmlml::PmlMlizer`
//! -- this module mirrors those closely.
//!
//! # Why this isn't built on `crate::htmlz::oeb2html::Oeb2Html`
//!
//! `MarkdownMLizer` subclasses `calibre.ebooks.htmlz.oeb2html.OEB2HTML`
//! in Python, inheriting `rewrite_ids`/`map_resources`/`rewrite_link`/
//! `get_link_id` -- the machinery that flattens a multi-file spine into
//! one document with cross-file ids renumbered and links repointed.
//! `Oeb2Html` here (issue #28) is a concrete struct built for exactly
//! that flattening job, not designed for subclass-style hook overriding,
//! so a literal inheritance translation doesn't fit.
//!
//! More importantly, *none of that machinery is observable in this
//! converter's actual output*: `dump_text`'s `<a>` handling only ever
//! emits a link when `'://' in attribs['href']` (an absolute, external
//! URL) -- internal links (the only ones `rewrite_link` would ever
//! change) are dropped unconditionally, tag and all. Whether an
//! internal href was rewritten to `#calibre_link-N` or left as
//! `chapter2.html#foo`, it contains no `://`, so the observable output
//! is identical either way. `<img>` likewise uses `attribs['src']`
//! directly, never consulting an image-name map. And nothing in this
//! file's `dump_text` ever reads an `id` attribute at all, so
//! `rewrite_ids` has no observable effect on markdown output either.
//! Given that, reimplementing the flatten/renumber machinery just to
//! leave it unused would be pure ceremony; this port reads `href`
//! attributes directly instead, which is behaviorally identical to
//! calling through `rewrite_link` first for every case that survives to
//! the output.
//!
//! What *is* reused: the "walk a stylizer'd tree, maintain a tag
//! stack" architecture common to every mlizer this crate has, and
//! `crate::oeb::stylizer::StyleProvider`/`ResolvedStyle` as the style
//! seam (not `Oeb2Html`'s dump_text, which serializes HTML, not
//! markdown -- there is no text to reuse there, only shape, and the
//! shape is already shared via the `StyleProvider` trait).

use std::sync::OnceLock;

use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use crate::oeb::transforms::flatcss::unit_convert;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Options `extract_content` reads. Port of the subset of `opts`
/// `markdownml.py` actually touches.
#[derive(Debug, Clone, Default)]
pub struct MarkdownMlOptions {
    /// Only absolute (external, `://`-containing) links are ever
    /// emitted, and only when this is set.
    pub keep_links: bool,
    /// Emit `<img>` as `![alt](src)` when set; drop image references
    /// entirely otherwise.
    pub keep_image_references: bool,
}

#[derive(Debug, Clone)]
struct ListState {
    name: String,
    num: u32,
}

/// Port of `calibre.ebooks.txt.markdownml.MarkdownMLizer`.
#[derive(Debug, Default)]
pub struct MarkdownMlizer {
    in_code: bool,
    in_pre: bool,
    list: Vec<ListState>,
    blockquotes: u32,
    remove_space_after_newline: bool,
    style_bold: bool,
    style_italic: bool,
}

impl MarkdownMlizer {
    /// Upstream `markdownml.py` has no such bound (unlike
    /// `textileml.py`'s `MAX_EM`, see [`crate::txt::textileml`]) and
    /// divides `margin_top` by `font_size` unclamped. In Python that
    /// division raises `ZeroDivisionError` for a zero `font_size`
    /// (achievable via ordinary CSS like `font-size: 0`); in Rust,
    /// float division by zero silently yields `inf`/`NaN`, and the
    /// subsequent `as i64` cast *saturates* rather than panicking,
    /// producing an enormous `ems` value that a `.repeat()` call then
    /// tries to allocate, exhausting memory. Clamping here (matching
    /// `textileml.rs`'s established bound) turns that into a bounded,
    /// harmless string instead of an OOM.
    const MAX_EM: i64 = 10;

    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `extract_content` + `mlize_spine` + `tidy_up`.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &MarkdownMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        *self = MarkdownMlizer::default();
        let txt = self.mlize_spine(oeb, opts, stylizer);
        Self::tidy_up(&txt)
    }

    fn mlize_spine(
        &mut self,
        oeb: &OEBBook,
        opts: &MarkdownMlOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        let mut output = String::new();
        for spine_item in &oeb.spine.items {
            let Some(item) = oeb.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = oeb.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            if let Some(body) = find_body(&doc) {
                let mut out = Vec::new();
                self.dump_text(body, opts, stylizer, &mut out);
                output.push_str(&out.concat());
            }
            output.push_str("\n\n");
        }
        output
    }

    /// Port of `tidy_up`.
    fn tidy_up(text: &str) -> String {
        static LEADING_1_3: OnceLock<Regex> = OnceLock::new();
        static LEADING_1: OnceLock<Regex> = OnceLock::new();
        static BLANK_SPACE_LINE: OnceLock<Regex> = OnceLock::new();
        static EXCESS_BLANK: OnceLock<Regex> = OnceLock::new();
        let leading_1_3 = LEADING_1_3.get_or_init(|| Regex::new(r"(?ms)^[ ]{1,3}").expect("regex"));
        let leading_1 = LEADING_1.get_or_init(|| Regex::new(r"(?ms)^[ ]").expect("regex"));
        let blank_space_line =
            BLANK_SPACE_LINE.get_or_init(|| Regex::new(r"(?ms)^[ ]+$").expect("regex"));
        let excess_blank = EXCESS_BLANK.get_or_init(|| Regex::new(r"(?ms)\n{7,}").expect("regex"));

        let text = leading_1_3.replace_all(text, "").into_owned();
        // A `pre` block indents by 4 spaces; 3 were just trimmed, so
        // anything with a space left is a `pre` line.
        let text = leading_1.replace_all(&text, "    ").into_owned();

        // Remove tabs that aren't at the beginning of a line.
        let mut new_lines = Vec::new();
        for l in text.split('\n') {
            let leading_tabs: String = l.chars().take_while(|&c| c == '\t').collect();
            let rest: String = l.chars().filter(|&c| c != '\t').collect();
            new_lines.push(format!("{leading_tabs}{rest}"));
        }
        let text = new_lines.join("\n");

        let text = blank_space_line.replace_all(&text, "").into_owned();
        let text = excess_blank.replace_all(&text, "\n\n\n\n\n\n").into_owned();

        format!("{}\n\n", text.trim_start().trim_end())
    }

    /// Port of `remove_newlines`.
    fn remove_newlines(&mut self, text: &str) -> String {
        static MULTI_SPACE: OnceLock<Regex> = OnceLock::new();
        static TABS: OnceLock<Regex> = OnceLock::new();
        static LEADING_SPACES: OnceLock<Regex> = OnceLock::new();
        let multi_space = MULTI_SPACE.get_or_init(|| Regex::new(r"[ ]{2,}").expect("regex"));
        let tabs = TABS.get_or_init(|| Regex::new(r"\t+").expect("regex"));
        let leading_spaces = LEADING_SPACES.get_or_init(|| Regex::new(r"^ +").expect("regex"));

        let mut text = text.replace("\r\n", " ").replace(['\n', '\r'], " ");
        text = multi_space.replace_all(&text, " ").into_owned();
        text = tabs.replace_all(&text, "").into_owned();
        if self.remove_space_after_newline {
            text = leading_spaces.replace_all(&text, "").into_owned();
            self.remove_space_after_newline = false;
        }
        text
    }

    /// Port of `prepare_string_for_markdown`.
    fn prepare_string_for_markdown(txt: &str) -> String {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r"([\\`*_{}\[\]()#+!])").expect("regex"));
        re.replace_all(txt, r"\$1").into_owned()
    }

    /// Port of `prepare_string_for_pre`.
    fn prepare_string_for_pre(txt: &str) -> String {
        txt.split('\n')
            .map(|l| format!("    {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Port of `dump_text`.
    fn dump_text(
        &mut self,
        elem: Node,
        opts: &MarkdownMlOptions,
        stylizer: &dyn StyleProvider,
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
        let tag = elem.tag_name().name().to_string();
        let mut tags: Vec<String> = Vec::new();

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

        // Soft scene breaks (top).
        let top_ems = ((style.margin_top / style.font_size).round() as i64 - 1).min(Self::MAX_EM);
        if top_ems >= 1 {
            out.push("\n\n".repeat(top_ems as usize));
        }

        let bq = "> ".repeat(self.blockquotes as usize);
        let is_heading = matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        if is_heading || tag == "p" || tag == "div" {
            let h_tag = if is_heading {
                let level: usize = tag[1..].parse().unwrap_or(1);
                format!("{} ", "#".repeat(level))
            } else {
                String::new()
            };
            out.push(format!("\n{bq}{h_tag}"));
            tags.push("\n".to_string());
            self.remove_space_after_newline = true;
        }

        let not_heading_or_cite = !matches!(
            tag.as_str(),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "cite"
        );
        if (style.font_style == "italic" || matches!(tag.as_str(), "i" | "em"))
            && not_heading_or_cite
            && !self.style_italic
        {
            out.push("*".to_string());
            tags.push("*".to_string());
            self.style_italic = true;
        }
        let not_heading_or_th =
            !matches!(tag.as_str(), "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th");
        if (matches!(style.font_weight.as_str(), "bold" | "bolder")
            || matches!(tag.as_str(), "b" | "strong"))
            && not_heading_or_th
            && !self.style_bold
        {
            out.push("**".to_string());
            tags.push("**".to_string());
            self.style_bold = true;
        }

        if tag == "br" {
            out.push("  \n".to_string());
            self.remove_space_after_newline = true;
        }
        match tag.as_str() {
            "blockquote" => {
                self.blockquotes += 1;
                tags.push(">".to_string());
                out.push("> ".repeat(self.blockquotes as usize));
            }
            "code" => {
                if !self.in_pre && !self.in_code {
                    out.push("`".to_string());
                    tags.push("`".to_string());
                    self.in_code = true;
                }
            }
            "pre" => {
                if !self.in_pre {
                    out.push("\n".to_string());
                    tags.push("pre".to_string());
                    self.in_pre = true;
                }
            }
            "hr" => {
                out.push("\n* * *".to_string());
                tags.push("\n".to_string());
            }
            "a" => {
                if opts.keep_links {
                    if let Some(href) = elem.attribute("href").filter(|h| h.contains("://")) {
                        let mut title = String::new();
                        if let Some(t) = elem.attribute("title") {
                            let remove_space = self.remove_space_after_newline;
                            title = format!(" \"{}\"", self.remove_newlines(t));
                            self.remove_space_after_newline = remove_space;
                        }
                        out.push("[".to_string());
                        tags.push(format!("]({href}{title})"));
                    }
                }
            }
            "img" => {
                if opts.keep_image_references {
                    if let Some(src) = elem.attribute("src") {
                        let mut txt = "!".to_string();
                        if let Some(alt) = elem.attribute("alt") {
                            let remove_space = self.remove_space_after_newline;
                            txt.push_str(&format!("[{}]", self.remove_newlines(alt)));
                            self.remove_space_after_newline = remove_space;
                        }
                        txt.push_str(&format!("({src})"));
                        out.push(txt);
                    }
                }
            }
            "ol" | "ul" => {
                tags.push(tag.clone());
                self.list.push(ListState {
                    name: tag.clone(),
                    num: 0,
                });
            }
            "li" => {
                let list_count = self.list.len();
                let name = self
                    .list
                    .last()
                    .map(|l| l.name.clone())
                    .unwrap_or_else(|| "ul".to_string());
                out.push("\n".to_string());
                if list_count.saturating_sub(1) > 0 {
                    out.push("\t".repeat(list_count - 1));
                }
                out.push(bq.clone());
                if name == "ul" {
                    out.push("+ ".to_string());
                } else if name == "ol" {
                    if let Some(li) = self.list.last_mut() {
                        li.num += 1;
                        out.push(format!("{}. ", li.num));
                    }
                }
            }
            _ => {}
        }

        if let Some(text) = own_text(elem) {
            let converted = if self.in_pre {
                Self::prepare_string_for_pre(&text)
            } else if self.in_code {
                self.remove_newlines(&text)
            } else {
                let removed = self.remove_newlines(&text);
                Self::prepare_string_for_markdown(&removed)
            };
            out.push(converted);
        }

        for child in elem.children() {
            self.dump_text(child, opts, stylizer, out);
        }

        for t in tags.into_iter().rev() {
            match t.as_str() {
                "pre" => {
                    self.in_pre = false;
                    out.push("\n".to_string());
                }
                "ul" | "ol" => {
                    self.list.pop();
                    out.push("\n".to_string());
                }
                ">" => {
                    self.blockquotes = self.blockquotes.saturating_sub(1);
                }
                _ => {
                    if t == "**" {
                        self.style_bold = false;
                    } else if t == "*" {
                        self.style_italic = false;
                    } else if t == "`" {
                        self.in_code = false;
                    }
                    out.push(t);
                }
            }
        }

        // Soft scene breaks (bottom): `ResolvedStyle` has no
        // `margin-bottom` field (it doesn't inherit and isn't otherwise
        // needed by the trait's other consumers), so it's pulled from
        // the element's own declared `style="..."` text and converted
        // the same way `crate::oeb::transforms::flatcss::unit_convert`
        // (the real port of `calibre.ebooks.unit_convert`) converts any
        // other CSS length -- with a fixed 96dpi/12pt-body assumption,
        // since there is no output-profile system here yet.
        if let Some(mb) = inline_style_prop(&style.css_text, "margin-bottom")
            .and_then(|v| unit_convert(&v, 0.0, style.font_size, 96.0, 12.0))
        {
            let ems = ((mb / style.font_size).round() as i64 - 1).min(Self::MAX_EM);
            if ems >= 1 {
                out.push("\n\n".repeat(ems as usize));
            }
        }

        if let Some(tail) = tail_text(elem) {
            let converted = if self.in_pre {
                Self::prepare_string_for_pre(&tail)
            } else if self.in_code {
                self.remove_newlines(&tail)
            } else {
                let removed = self.remove_newlines(&tail);
                Self::prepare_string_for_markdown(&removed)
            };
            out.push(converted);
        }
    }
}

/// Extract one declaration's value out of an inline `style="..."` CSS
/// text. Mirrors `pml/pmlml.rs`'s helper of the same name.
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

    fn convert(html: &str, opts: &MarkdownMlOptions) -> String {
        let oeb = book(html);
        MarkdownMlizer::new().extract_content(&oeb, opts, &TagStylizer)
    }

    #[test]
    fn headings_become_hash_prefixed_lines() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1><h2>Sub</h2></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("# Title"), "{out}");
        assert!(out.contains("## Sub"), "{out}");
    }

    #[test]
    fn bold_and_italic_are_escaped_with_markdown_markers() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>plain <b>bold</b> and <i>italic</i> text</p></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("**bold**"), "{out}");
        assert!(out.contains("*italic*"), "{out}");
    }

    #[test]
    fn external_links_are_emitted_only_when_keep_links_is_set() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="http://example.com/">link</a></p></body></html>"#;
        let out_off = convert(html, &MarkdownMlOptions::default());
        assert!(!out_off.contains("]("), "{out_off}");

        let opts = MarkdownMlOptions {
            keep_links: true,
            ..Default::default()
        };
        let out_on = convert(html, &opts);
        assert!(out_on.contains("[link](http://example.com/)"), "{out_on}");
    }

    #[test]
    fn internal_links_are_never_emitted_even_with_keep_links() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="chapter2.html">next</a></p></body></html>"#;
        let opts = MarkdownMlOptions {
            keep_links: true,
            ..Default::default()
        };
        let out = convert(html, &opts);
        assert!(!out.contains("]("), "{out}");
        assert!(out.contains("next"), "{out}");
    }

    #[test]
    fn images_only_emitted_when_keep_image_references_is_set() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><img src="a.png" alt="pic"/></p></body></html>"#;
        let out_off = convert(html, &MarkdownMlOptions::default());
        assert!(!out_off.contains("!["), "{out_off}");

        let opts = MarkdownMlOptions {
            keep_image_references: true,
            ..Default::default()
        };
        let out_on = convert(html, &opts);
        assert!(out_on.contains("![pic](a.png)"), "{out_on}");
    }

    #[test]
    fn unordered_lists_use_plus_markers() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><ul><li>one</li><li>two</li></ul></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("+ one"), "{out}");
        assert!(out.contains("+ two"), "{out}");
    }

    #[test]
    fn ordered_lists_use_incrementing_numbers() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><ol><li>one</li><li>two</li></ol></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("1. one"), "{out}");
        assert!(out.contains("2. two"), "{out}");
    }

    #[test]
    fn blockquotes_get_a_greater_than_prefix() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><blockquote><p>quoted</p></blockquote></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("> "), "{out}");
        assert!(out.contains("quoted"), "{out}");
    }

    #[test]
    fn pre_and_code_blocks_are_marked() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><pre>code here</pre><p><code>inline</code></p></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("code here"), "{out}");
        assert!(out.contains("`inline`"), "{out}");
    }

    #[test]
    fn hidden_content_is_dropped() {
        // `TagStylizer` (what `convert()` uses) only knows tag-name
        // defaults, not inline `style="..."` -- it can't see
        // `display: none` here, so this needs the real `Stylizer`
        // (see `a_real_stylizer_still_produces_bold_from_font_weight`
        // for the same pattern).
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>keep</p><p style="display: none">drop</p></body></html>"#;
        let oeb = book(html);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out =
            MarkdownMlizer::new().extract_content(&oeb, &MarkdownMlOptions::default(), &stylizer);
        assert!(out.contains("keep"), "{out}");
        assert!(!out.contains("drop"), "{out}");
    }

    #[test]
    fn special_markdown_characters_are_escaped() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>1 * 2 [x] (y) #z</p></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains(r"1 \* 2 \[x\] \(y\) \#z"), "{out}");
    }

    #[test]
    fn hr_produces_a_scene_break() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>a</p><hr/><p>b</p></body></html>"#;
        let out = convert(html, &MarkdownMlOptions::default());
        assert!(out.contains("* * *"), "{out}");
    }

    #[test]
    fn a_real_stylizer_still_produces_bold_from_font_weight() {
        let html = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p style="font-weight: bold">bold text</p></body></html>"#;
        let oeb = book(html);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out =
            MarkdownMlizer::new().extract_content(&oeb, &MarkdownMlOptions::default(), &stylizer);
        assert!(out.contains("**bold text**"), "{out}");
    }
}
