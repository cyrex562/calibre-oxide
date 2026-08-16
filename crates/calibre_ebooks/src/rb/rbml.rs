//! OEB/XHTML -> RB (RocketBook) markup.
//!
//! Port of `old_src/src/calibre/ebooks/rb/rbml.py`'s `RBMLizer`.
//! Architecturally identical to [`crate::pml::pmlml::PmlMlizer`] (issue
//! #47) and [`crate::fb2::fb2ml::Fb2Mlizer`] (issue #26): walk a
//! spine's XHTML trees with a [`StyleProvider`], maintain a per-element
//! tag list, emit markup strings. This module mirrors `pmlml.rs`'s
//! shape closely -- same crate, same session, no reason to reinvent the
//! tree-walk architecture.
//!
//! Unlike `pmlml.py`, `dump_text` here never checks "is this tag
//! already open on an ancestor" before opening a fresh one -- every one
//! of `TAGS`/`LINK_TAGS`/`STYLES` is opened unconditionally each time it
//! matches, so [`RbMlizer::dump_text`] does not need to thread an
//! "ancestor tags" view down through the recursion the way `pmlml.rs`
//! does; a plain per-call `Vec<String>` of this element's own opened
//! tags, reversed and closed at the end, is enough.
//!
//! # Preserved upstream quirks
//!
//! - **A stray `</A>` on external links.** `dump_text`'s link handling
//!   only emits the opening `<A HREF="#...">` for *internal*
//!   (`'#'`-resolvable, no `://`) links, but pushes a closing tag onto
//!   the tag stack whenever `href` is present at all -- regardless of
//!   whether it was internal or external. An external link therefore
//!   gets a `</A>` in the output with no matching opening tag. Verified
//!   against the Python's indentation (`tag_count += 1` /
//!   `tag_stack.append('A')` sit at the same indent as `if href:`, one
//!   level *outside* `if '://' not in href:`). Ported as-is.
//! - **In-body images not already in the OEB manifest get a
//!   nameless file.** When `dump_text` meets an `<img>` whose resolved
//!   `src` isn't already a key of `name_map` (i.e. `RBWriter::_images`
//!   didn't already assign it a `N.png` name from the manifest), it
//!   calls `unique_name(f'{len(self.name_map)}', self.name_map.keys())`
//!   -- a bare number with **no extension**, deduped against
//!   `name_map`'s *hrefs* rather than its already-assigned names (the
//!   same href-vs-values mix-up `pmlml.rs` documents for its own
//!   `image_name` call). Ported as-is: such an image is written into
//!   the markup as `<IMG SRC="3">`, extension-less.
//! - **`get_toc`'s "not found" branch is dead-on-arrival upstream, not
//!   ported as a crash.** Python's `get_toc` calls `self.oeb.warn(...)`
//!   when a TOC item's href has no matching anchor -- but `RBMLizer`
//!   never sets `self.oeb` (only `self.oeb_book`, in `extract_content`),
//!   so that call would raise `AttributeError` in real Python the
//!   instant it runs. Rather than reproduce a guaranteed crash, this
//!   port does the evidently-intended thing: skip the item silently.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::OnceLock;

use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use crate::rb::unique_name;
use crate::xml_util::prepare_string_for_xml;

const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Port of `TAGS`. RB markup uppercases these tag names directly
/// (`<B>`, `<I>`, ...) -- note this list does *not* match `pmlml.py`'s
/// `TAG_MAP` (no `u`/underline, no `del`/strikethrough), but does
/// include several block-ish tags PML maps differently (`div`, `p`,
/// `h1`..`h6`, `ol`/`ul`/`li`, `blockquote`, `pre`, `code`, `hr`,
/// `br`, `center`, `big`, `small`, `sub`, `sup`).
const TAGS: &[&str] = &[
    "b",
    "big",
    "blockquote",
    "br",
    "center",
    "code",
    "div",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "li",
    "ol",
    "p",
    "pre",
    "small",
    "sub",
    "sup",
    "ul",
];

const LINK_TAGS: &[&str] = &["a"];
const IMAGE_TAGS: &[&str] = &["img"];

/// Port of `STYLES`, in the Python's declared order.
const STYLE_PROPS: &[&str] = &["font-weight", "font-style", "text-align"];

fn style_tag_for(prop: &str, val: &str) -> Option<&'static str> {
    match (prop, val) {
        ("font-weight", "bold") | ("font-weight", "bolder") => Some("B"),
        ("font-style", "italic") => Some("I"),
        ("text-align", "center") => Some("CENTER"),
        _ => None,
    }
}

/// The exact sentinel Python inserts and then substitutes the real TOC
/// markup into -- deliberately unlikely to collide with real content,
/// not a design to "clean up".
const TOC_PLACEHOLDER: &str =
    "ghji87yhjko0Caliblre-toc-placeholder-for-insertion-later8ujko0987yjk";

/// Options `extract_content`/`get_toc` read. Port of the subset of
/// `opts` `rbml.py` actually touches.
#[derive(Debug, Clone, Default)]
pub struct RbOptions {
    /// `get_toc` only emits inline TOC HTML when this is set.
    pub inline_toc: bool,
}

/// Port of `calibre.ebooks.rb.rbml.RBMLizer`.
#[derive(Debug, Default)]
pub struct RbMlizer {
    /// href -> assigned RB image name (`"0.png"`, ...), threaded in
    /// from `RbWriter::images` and extended in place for images
    /// `dump_text` discovers that weren't already in the manifest (see
    /// the module docs' quirk).
    pub name_map: HashMap<String, String>,
    /// `"href#fragment"` -> `"calibre_link-N"`.
    link_hrefs: HashMap<String, String>,
}

impl RbMlizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_name_map(name_map: HashMap<String, String>) -> Self {
        Self {
            name_map,
            link_hrefs: HashMap::new(),
        }
    }

    /// Port of `extract_content` + `mlize_spine`.
    pub fn extract_content(
        &mut self,
        oeb: &OEBBook,
        opts: &RbOptions,
        stylizer: &dyn StyleProvider,
    ) -> String {
        self.link_hrefs.clear();

        let mut output = String::from("<HTML><HEAD><TITLE></TITLE></HEAD><BODY>");
        output.push_str(&self.get_cover_page(oeb, stylizer));
        output.push_str(TOC_PLACEHOLDER);
        output.push_str(&self.get_text(oeb, stylizer));
        output.push_str("</BODY></HTML>");

        let toc_html = self.get_toc(oeb, opts);
        let output = output.replace(TOC_PLACEHOLDER, &toc_html);
        clean_text(&output)
    }

    /// Port of `get_cover_page`.
    fn get_cover_page(&mut self, oeb: &OEBBook, stylizer: &dyn StyleProvider) -> String {
        let mut output = String::new();
        if let Some(cover) = oeb.guide.get("cover") {
            if let Some(name) = self.name_map.get(&cover.href) {
                output.push_str(&format!("<IMG SRC=\"{name}\">"));
            }
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
                                self.dump_text(body, stylizer, &item.href, &mut out);
                                output.push_str(&out.concat());
                            }
                        }
                    }
                }
            }
        }
        output
    }

    /// Port of `get_toc`. See the module docs for the "not found"
    /// branch's deviation from a crashing `self.oeb.warn(...)`.
    fn get_toc(&self, oeb: &OEBBook, opts: &RbOptions) -> String {
        if !opts.inline_toc {
            return String::new();
        }
        let mut toc = String::from("<H1>Table of Contents:</H1><UL>\n");
        for item in &oeb.toc.root.children {
            let Some(href) = item.href.as_deref() else {
                continue;
            };
            if let Some(target) = self.link_hrefs.get(href) {
                let title = item.title.clone().unwrap_or_default();
                toc.push_str(&format!("<LI><A HREF=\"#{target}\">{title}</A></LI>\n"));
            }
        }
        toc.push_str("</UL>");
        toc
    }

    /// Port of `get_text`.
    fn get_text(&mut self, oeb: &OEBBook, stylizer: &dyn StyleProvider) -> String {
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
            output.push_str(&self.add_page_anchor(&item.href));
            if let Some(body) = find_body(&doc) {
                let mut out = Vec::new();
                self.dump_text(body, stylizer, &item.href, &mut out);
                output.push_str(&out.concat());
            }
        }
        output
    }

    /// Port of `add_page_anchor`.
    fn add_page_anchor(&mut self, page_href: &str) -> String {
        self.get_anchor(page_href, "")
    }

    /// Port of `get_anchor`.
    fn get_anchor(&mut self, page_href: &str, aid: &str) -> String {
        let key = format!("{page_href}#{aid}");
        if !self.link_hrefs.contains_key(&key) {
            let n = self.link_hrefs.len();
            self.link_hrefs
                .insert(key.clone(), format!("calibre_link-{n}"));
        }
        format!("<A NAME=\"{}\"></A>", self.link_hrefs[&key])
    }

    /// Port of `dump_text`.
    fn dump_text(
        &mut self,
        elem: Node,
        stylizer: &dyn StyleProvider,
        page_href: &str,
        out: &mut Vec<String>,
    ) {
        if !elem.is_element() {
            return;
        }
        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            // Port of `return [elem.tail]`: raw, NOT XML-escaped --
            // unlike the tail push at the very end of this function.
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

        let tag = elem.tag_name().name().to_string();
        let mut tags: Vec<String> = Vec::new();

        if IMAGE_TAGS.contains(&tag.as_str()) {
            if let Some(src) = elem.attribute("src").filter(|s| !s.is_empty()) {
                let href = resolve_href(page_href, src);
                if !self.name_map.contains_key(&href) {
                    let keys: Vec<String> = self.name_map.keys().cloned().collect();
                    let name = unique_name(&keys.len().to_string(), &keys);
                    self.name_map.insert(href.clone(), name);
                }
                out.push(format!("<IMG SRC=\"{}\">", self.name_map[&href]));
            }
        }

        if TAGS.contains(&tag.as_str()) {
            let rb_tag = tag.to_uppercase();
            out.push(format!("<{rb_tag}>"));
            tags.push(rb_tag);
        }

        if LINK_TAGS.contains(&tag.as_str()) {
            if let Some(href_attr) = elem.attribute("href").filter(|h| !h.is_empty()) {
                let mut href = resolve_href(page_href, href_attr);
                if !href.contains("://") {
                    if !href.contains('#') {
                        href.push('#');
                    }
                    if !self.link_hrefs.contains_key(&href) {
                        let n = self.link_hrefs.len();
                        self.link_hrefs
                            .insert(href.clone(), format!("calibre_link-{n}"));
                    }
                    let target = self.link_hrefs[&href].clone();
                    out.push(format!("<A HREF=\"#{target}\">"));
                }
                // Closing `</A>` is queued regardless of the `://`
                // branch above -- see the module docs' quirk.
                tags.push("A".to_string());
            }
        }

        if let Some(id_name) = elem.attribute("id") {
            out.push(self.get_anchor(page_href, id_name));
        }

        for prop in STYLE_PROPS {
            let val: String = match *prop {
                "font-weight" => style.font_weight.clone(),
                "font-style" => style.font_style.clone(),
                "text-align" => {
                    inline_style_prop(&style.css_text, "text-align").unwrap_or_default()
                }
                _ => unreachable!(),
            };
            if let Some(style_tag) = style_tag_for(prop, &val) {
                out.push(format!("<{style_tag}>"));
                tags.push(style_tag.to_string());
            }
        }

        if let Some(text) = own_text(elem) {
            out.push(prepare_string_for_xml(&text, false));
        }

        for child in elem.children() {
            self.dump_text(child, stylizer, page_href, out);
        }

        tags.reverse();
        for t in tags {
            out.push(format!("</{t}>"));
        }

        if let Some(tail) = tail_text(elem) {
            out.push(prepare_string_for_xml(&tail, false));
        }
    }
}

/// Port of `clean_text`: drops `<A NAME="...">` anchors that no
/// `<A HREF="#...">` ever references.
fn clean_text(text: &str) -> String {
    static ANCHOR: OnceLock<Regex> = OnceLock::new();
    let anchor_re = ANCHOR.get_or_init(|| Regex::new("<A NAME=\"(?P<id>.+?)\"></A>").unwrap());
    static LINK: OnceLock<Regex> = OnceLock::new();
    // NOT a raw string: the pattern itself contains a `"#` sequence,
    // which would close a single-hash raw string early.
    let link_re = LINK.get_or_init(|| Regex::new("<A HREF=\"#(?P<id>.+?)\">").unwrap());

    let anchors: HashSet<String> = anchor_re
        .captures_iter(text)
        .map(|c| c["id"].to_string())
        .collect();
    let links: HashSet<String> = link_re
        .captures_iter(text)
        .map(|c| c["id"].to_string())
        .collect();

    let mut text = text.to_string();
    for unused in anchors.difference(&links) {
        text = text.replace(&format!("<A NAME=\"{unused}\"></A>"), "");
    }
    text
}

/// Extract one declaration's value out of an inline `style="..."`-style
/// CSS text (as carried by `ResolvedStyle::css_text`). Mirrors
/// `pml/pmlml.rs`'s helper of the same name.
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
        RbMlizer::new().extract_content(oeb, &RbOptions::default(), &TagStylizer)
    }

    const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p>Hello <b>bold</b> and <i>italic</i>.</p>
</body></html>"#;

    #[test]
    fn wraps_body_in_html_head_title_body_shell() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb);
        assert!(
            out.starts_with("<HTML><HEAD><TITLE></TITLE></HEAD><BODY>"),
            "{out}"
        );
        assert!(out.ends_with("</BODY></HTML>"), "{out}");
    }

    #[test]
    fn inline_tags_map_to_uppercase_rb_tags() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb);
        assert!(out.contains("<B>bold</B>"), "{out}");
        assert!(out.contains("<I>italic</I>"), "{out}");
    }

    #[test]
    fn headings_and_block_tags_map_through_tags_list() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1><div>d</div><ul><li>x</li></ul></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("<H1>"), "{out}");
        assert!(out.contains("</H1>"), "{out}");
        assert!(out.contains("<DIV>d</DIV>"), "{out}");
        assert!(out.contains("<UL><LI>x</LI></UL>"), "{out}");
    }

    #[test]
    fn internal_links_resolve_to_a_calibre_link_anchor() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="chapter2.html">next</a></p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("<A HREF=\"#calibre_link-"), "{out}");
        assert!(out.contains("</A>"), "{out}");
    }

    #[test]
    fn external_links_get_a_stray_closing_tag_with_no_opener() {
        // Preserved upstream quirk -- see the module docs.
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p><a href="http://example.com/">link</a></p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(!out.contains("<A HREF=\"#"), "{out}");
        assert!(!out.contains("HREF=\"http"), "{out}");
        assert!(out.contains("link</A>"), "{out}");
    }

    #[test]
    fn images_already_in_name_map_use_their_assigned_name() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.png"/></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let mut name_map = HashMap::new();
        name_map.insert("pic.png".to_string(), "0.png".to_string());
        let out = RbMlizer::with_name_map(name_map).extract_content(
            &oeb,
            &RbOptions::default(),
            &TagStylizer,
        );
        assert!(out.contains("<IMG SRC=\"0.png\">"), "{out}");
    }

    #[test]
    fn images_not_already_in_name_map_get_an_extension_less_fallback_name() {
        // Preserved upstream quirk -- see the module docs.
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.png"/></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("<IMG SRC=\"0\">"), "{out}");
    }

    #[test]
    fn anchors_without_a_referencing_link_are_dropped_by_clean_text() {
        let text = "<A NAME=\"unused\"></A>text<A NAME=\"used\"></A>more<A HREF=\"#used\">link</A>";
        let out = clean_text(text);
        assert!(!out.contains("NAME=\"unused\""), "{out}");
        assert!(out.contains("NAME=\"used\""), "{out}");
    }

    #[test]
    fn inline_toc_only_emitted_when_requested() {
        let oeb = book(&[("index.html", PAGE)]);
        let opts_off = RbOptions::default();
        let out_off = RbMlizer::new().extract_content(&oeb, &opts_off, &TagStylizer);
        assert!(!out_off.contains("Table of Contents"), "{out_off}");
    }

    #[test]
    fn style_attributes_map_to_style_tags() {
        use crate::oeb::stylizer::Stylizer as ConcreteStylizer;
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p style="font-weight: bold">bold</p><p style="text-align: center">mid</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out = RbMlizer::new().extract_content(&oeb, &RbOptions::default(), &stylizer);
        assert!(out.contains("<B>bold</B>"), "{out}");
        assert!(out.contains("<CENTER>mid</CENTER>"), "{out}");
    }

    #[test]
    fn resolve_href_resolves_relative_and_leaves_absolute_alone() {
        assert_eq!(
            resolve_href("text/index.html", "images/a.png"),
            "text/images/a.png"
        );
        assert_eq!(
            resolve_href("index.html", "http://x.com/a.png"),
            "http://x.com/a.png"
        );
    }
}
