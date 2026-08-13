//! OEB → a single HTML file.
//!
//! Port of `old_src/src/calibre/ebooks/htmlz/oeb2html.py`.
//!
//! HTMLZ is a book as one HTML file plus its images in a zip, so the
//! conversion has to flatten a whole spine into one document: every
//! file's `<body>` becomes a `<div>`, every id is renumbered so the
//! merged document has no collisions, and every link is rewritten to
//! point at the renumbered id or at the flattened image name.
//!
//! Three flavours differ only in what they do with CSS, and the Python
//! expresses that as three subclasses overriding `dump_text`. Here it
//! is [`CssMode`], since the rest of the class is identical between
//! them.
//!
//! # Two structural differences
//!
//! **Rewriting happens during emission.** calibre mutates the parsed
//! tree first — `rewrite_ids`, then `rewrite_links` — and serializes
//! afterwards. `roxmltree` is read-only, so this port rewrites ids and
//! link attributes as it writes them out. The output is the same; the
//! tree is not touched.
//!
//! **The stylizer is a trait.** As in the FB2 writer, resolved CSS
//! comes from [`StyleProvider`] rather than from a cascade this crate
//! does not have yet. [`CssMode::Inline`] is the one flavour that
//! suffers for it: it writes each element's computed style into a
//! `style` attribute, and a provider that cannot compute one emits
//! nothing. See the note on [`CssMode::Inline`].

use std::collections::BTreeMap;
use std::sync::OnceLock;

use regex::Regex;
use roxmltree::{Document, Node};

use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::StyleProvider;
use crate::xml_util::prepare_string_for_xml;

/// Tags written as `<tag />` rather than with a closing tag.
///
/// Port of the Python `SELF_CLOSING_TAGS`.
pub const SELF_CLOSING_TAGS: [&str; 9] = [
    "area", "base", "basefont", "br", "hr", "input", "img", "link", "meta",
];

/// Attributes that hold a link, and so need rewriting.
///
/// `html.defs.link_attrs` plus `xlink:href`, which is what the Python
/// adds to it.
const LINK_ATTRS: [&str; 11] = [
    "action",
    "archive",
    "background",
    "cite",
    "classid",
    "codebase",
    "data",
    "href",
    "longdesc",
    "profile",
    "src",
];

/// Media types treated as images worth extracting.
const IMAGE_TYPES: [&str; 7] = [
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/svg+xml",
    "image/bmp",
    "image/tiff",
    "image/webp",
];

/// Media types treated as embeddable fonts.
const FONT_TYPES: [&str; 6] = [
    "application/x-font-ttf",
    "application/x-font-truetype",
    "application/x-font-opentype",
    "application/font-sfnt",
    "application/vnd.ms-opentype",
    "font/woff",
];

/// How the conversion handles CSS.
///
/// Port of the three `OEB2HTML` subclasses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CssMode {
    /// Drop the CSS, remapping the handful of properties that have an
    /// HTML equivalent onto `<b>`, `<i>`, `<u>` and `<s>`.
    ///
    /// Port of `OEB2HTMLNoCSSizer`.
    #[default]
    None,
    /// Write each element's computed style into a `style` attribute.
    ///
    /// Port of `OEB2HTMLInlineCSSizer`. This is the flavour that most
    /// wants a real cascade: with a [`StyleProvider`] that cannot
    /// compute one, elements get whatever style they declared for
    /// themselves and nothing more.
    Inline,
    /// Keep the classes and emit the stylesheets, either in a `<style>`
    /// element or as a link to `style.css`.
    ///
    /// Port of `OEB2HTMLClassCSSizer`.
    Class {
        /// True to link `style.css` instead of inlining a `<style>`.
        external: bool,
    },
}

/// What a conversion produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    /// The single HTML document.
    pub html: String,
    /// OEB image href → the flat name it is written under, inside
    /// `images/`.
    pub images: BTreeMap<String, String>,
    /// OEB font href → flat name, inside `fonts/`.
    pub fonts: BTreeMap<String, String>,
    /// The stylesheet text, for [`CssMode::Class`] with `external`.
    pub css: String,
}

fn page_break_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"page-break-[^:]+:[^;]+;?").expect("valid regex"))
}

fn whitespace_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s{2,}").expect("valid regex"))
}

fn css_url_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"url\(\s*['"]?([^'")]+)['"]?\s*\)"#).expect("valid regex"))
}

/// Flattens an OEB book into one HTML document.
///
/// Port of the Python `OEB2HTML` and its three subclasses.
#[derive(Debug, Default)]
pub struct Oeb2Html {
    /// OEB href (with optional `#id`) → the `#calibre_link-N` anchor it
    /// was renumbered to.
    links: BTreeMap<String, String>,
    /// Insertion order for `links`, since the numbering depends on it.
    link_order: Vec<String>,
    images: BTreeMap<String, String>,
    fonts: BTreeMap<String, String>,
    base_hrefs: Vec<String>,
    book_title: String,
}

impl Oeb2Html {
    pub fn new() -> Self {
        Self::default()
    }

    /// Convert a book.
    ///
    /// Port of the Python `oeb2html` plus `mlize_spine`.
    pub fn convert(
        &mut self,
        oeb: &OEBBook,
        mode: CssMode,
        styles: &dyn StyleProvider,
    ) -> Converted {
        self.links.clear();
        self.link_order.clear();
        self.images.clear();
        self.fonts.clear();
        self.book_title = oeb
            .metadata
            .get("title")
            .first()
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        self.base_hrefs = self.spine_hrefs(oeb);

        self.map_resources(oeb);

        let mut body = String::new();
        for href in self.spine_hrefs(oeb) {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            let root = doc
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "body")
                .unwrap_or(doc.root_element());
            self.dump_text(root, styles, &href, mode, &mut body);
            body.push_str("\n\n");
        }

        let css = self.get_css(oeb);
        let title = prepare_string_for_xml(&self.book_title, false);
        let head = match mode {
            CssMode::Class { external: true } => {
                r#"<link href="style.css" rel="stylesheet" type="text/css" />"#.to_string()
            }
            CssMode::Class { external: false } => {
                format!(r#"<style type="text/css">{css}</style>"#)
            }
            _ => String::new(),
        };
        let html = match mode {
            CssMode::Class { .. } => format!(
                "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\" />\
                 {head}<title>{title}</title></head><body>{body}</body></html>"
            ),
            _ => format!(
                "<html><head><meta http-equiv=\"Content-Type\" content=\"text/html;charset=utf-8\" />\
                 <title>{title}</title></head><body>{body}</body></html>"
            ),
        };

        Converted {
            html,
            images: self.images.clone(),
            fonts: self.fonts.clone(),
            css,
        }
    }

    fn spine_hrefs(&self, oeb: &OEBBook) -> Vec<String> {
        oeb.spine
            .items
            .iter()
            .filter_map(|s| oeb.manifest.items.get(&s.idref))
            .map(|i| i.href.clone())
            .collect()
    }

    /// The anchor a href (optionally with an id) was renumbered to,
    /// assigning one if this is the first time it has been seen.
    ///
    /// Port of the Python `get_link_id`.
    pub fn get_link_id(&mut self, href: &str, id: &str) -> String {
        let key = if id.is_empty() {
            href.to_string()
        } else {
            format!("{href}#{id}")
        };
        if let Some(existing) = self.links.get(&key) {
            return existing.clone();
        }
        let anchor = format!("#calibre_link-{}", self.link_order.len());
        self.links.insert(key.clone(), anchor.clone());
        self.link_order.push(key);
        anchor
    }

    /// Give every image and font a flat name, and every spine file and
    /// linked-to id an anchor.
    ///
    /// Port of the Python `map_resources`.
    fn map_resources(&mut self, oeb: &OEBBook) {
        let mut by_type = |types: &[&str]| -> Vec<String> {
            let mut hrefs: Vec<String> = oeb
                .manifest
                .items
                .values()
                .filter(|i| types.contains(&i.media_type.as_str()))
                .map(|i| i.href.clone())
                .collect();
            // Sorted by href, as the Python's `sorted(..., key=attrgetter('href'))`
            // does, so the numbering is stable.
            hrefs.sort();
            hrefs
        };
        for (index, href) in by_type(&IMAGE_TYPES).into_iter().enumerate() {
            let ext = extension(&href);
            self.images.insert(href, format!("{index:06}{ext}"));
        }
        for (index, href) in by_type(&FONT_TYPES).into_iter().enumerate() {
            let ext = extension(&href);
            self.fonts.insert(href, format!("{index:06}{ext}"));
        }

        for href in self.spine_hrefs(oeb) {
            self.get_link_id(&href, "");
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            let root = doc
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "body")
                .unwrap_or(doc.root_element());
            // Every link into another spine file gets an anchor, so the
            // merged document can resolve it.
            let mut targets: Vec<(String, String)> = Vec::new();
            for el in root.descendants().filter(|n| n.is_element()) {
                for attr in el.attributes() {
                    if !LINK_ATTRS.contains(&attr.name()) {
                        continue;
                    }
                    let resolved = abshref(&href, attr.value());
                    let (target, id) = match resolved.split_once('#') {
                        Some((t, i)) => (t.to_string(), i.to_string()),
                        None => (resolved, String::new()),
                    };
                    if self.base_hrefs.contains(&target) {
                        targets.push((target, id));
                    }
                }
            }
            for (target, id) in targets {
                self.get_link_id(&target, &id);
            }
        }
    }

    /// Rewrite a link to point at its flattened destination.
    ///
    /// Port of the Python `rewrite_link`.
    pub fn rewrite_link(&self, url: &str, page_href: &str) -> String {
        let abs = abshref(page_href, url);
        if let Some(name) = self.images.get(&abs) {
            return format!("images/{name}");
        }
        if let Some(anchor) = self.links.get(&abs) {
            return anchor.clone();
        }
        if let Some(name) = self.fonts.get(&abs) {
            return format!("fonts/{name}");
        }
        url.to_string()
    }

    /// The stylesheets, concatenated, with their URLs rewritten.
    ///
    /// Port of the Python `get_css`. calibre rewrites the URLs with
    /// css_parser's `replaceUrls`; with no CSS object model here, the
    /// `url(...)` occurrences are rewritten textually, which reaches the
    /// same declarations without understanding the rest of the sheet.
    fn get_css(&self, oeb: &OEBBook) -> String {
        let mut out = String::new();
        let mut sheets: Vec<&crate::oeb::manifest::ManifestItem> = oeb
            .manifest
            .items
            .values()
            .filter(|i| i.media_type == "text/css")
            .collect();
        sheets.sort_by(|a, b| a.href.cmp(&b.href));
        for item in sheets {
            let Ok(raw) = oeb.container.read(&item.href) else {
                continue;
            };
            let text = String::from_utf8_lossy(&raw).into_owned();
            let rewritten = css_url_pat().replace_all(&text, |caps: &regex::Captures| {
                format!("url({})", self.rewrite_link(&caps[1], &item.href))
            });
            out.push_str(&rewritten);
            out.push_str("\n\n");
        }
        out
    }

    /// Escape text for HTML, spelling out the entities calibre does.
    ///
    /// Port of the Python `prepare_string_for_html`.
    pub fn prepare_string_for_html(raw: &str) -> String {
        prepare_string_for_xml(raw, false)
            .replace('\u{00ad}', "&shy;")
            .replace('\u{2014}', "&mdash;")
            .replace('\u{2013}', "&ndash;")
            .replace('\u{00a0}', "&nbsp;")
    }

    /// Walk an element, writing HTML.
    ///
    /// Port of the three `dump_text` implementations, which differ only
    /// in how they treat styles.
    fn dump_text(
        &mut self,
        elem: Node,
        styles: &dyn StyleProvider,
        page_href: &str,
        mode: CssMode,
        out: &mut String,
    ) {
        if !elem.is_element() {
            return;
        }
        let ns = elem.tag_name().namespace();
        // Only XHTML and SVG content is converted; text after anything
        // else still is.
        if !(ns.is_none() || ns == Some(XHTML_NS) || ns == Some(SVG_NS)) {
            if let Some(tail) = tail_text(elem) {
                out.push_str(&Self::prepare_string_for_html(&tail));
            }
            return;
        }

        let style = styles.style(elem);
        // The class flavour ignores styles entirely, so a hidden
        // element survives it.
        if mode != (CssMode::Class { external: true })
            && mode != (CssMode::Class { external: false })
            && (matches!(
                style.display.as_str(),
                "none" | "oeb-page-head" | "oeb-page-foot"
            ) || style.visibility == "hidden")
        {
            return;
        }

        let mut tag = elem.tag_name().name().to_string();
        // Each file's body becomes a div so the files can be merged.
        let is_body = tag == "body";
        if is_body {
            tag = "div".to_string();
        }
        let mut tags = vec![tag.clone()];

        let mut attrs = String::new();
        for attr in elem.attributes() {
            let name = attr.name();
            // `class` goes only where classes are kept; `style` is
            // always dropped, since it is re-emitted below if wanted.
            if name == "style" {
                continue;
            }
            if name == "class" && !matches!(mode, CssMode::Class { .. }) {
                continue;
            }
            let value = if name == "id" {
                // Renumbered so the merged document has no collisions.
                let anchor = self.get_link_id(page_href, attr.value());
                anchor.trim_start_matches('#').to_string()
            } else if LINK_ATTRS.contains(&name) {
                self.rewrite_link(attr.value(), page_href)
            } else {
                attr.value().to_string()
            };
            attrs.push_str(&format!(
                " {name}=\"{}\"",
                prepare_string_for_xml(&value, true)
            ));
        }
        // A body carries the anchor for its whole file.
        if is_body {
            let anchor = self.get_link_id(page_href, "");
            attrs.push_str(&format!(" id=\"{}\"", anchor.trim_start_matches('#')));
        }

        let style_attr = if mode == CssMode::Inline {
            inline_style_attribute(&style.css_text, is_body)
        } else {
            String::new()
        };

        out.push('<');
        out.push_str(&tag);
        out.push_str(&attrs);
        out.push_str(&style_attr);
        if SELF_CLOSING_TAGS.contains(&tag.as_str()) {
            out.push_str(" />");
        } else {
            out.push('>');
        }

        // The no-CSS flavour turns a few styles back into tags.
        if mode == CssMode::None {
            for (condition, html_tag) in [
                (matches!(style.font_weight.as_str(), "bold" | "bolder"), "b"),
                (style.font_style == "italic", "i"),
                (style.text_decoration == "underline", "u"),
                (style.text_decoration == "line-through", "s"),
            ] {
                if condition {
                    out.push_str(&format!("<{html_tag}>"));
                    tags.push(html_tag.to_string());
                }
            }
        }

        if let Some(text) = own_text(elem) {
            out.push_str(&Self::prepare_string_for_html(&text));
        }
        for child in elem.children().filter(Node::is_element) {
            self.dump_text(child, styles, page_href, mode, out);
        }

        for t in tags.iter().rev() {
            if !SELF_CLOSING_TAGS.contains(&t.as_str()) {
                out.push_str(&format!("</{t}>"));
            }
        }
        if let Some(tail) = tail_text(elem) {
            out.push_str(&Self::prepare_string_for_html(&tail));
        }
    }
}

/// Build the `style` attribute the inline-CSS flavour writes.
///
/// Port of the style handling in `OEB2HTMLInlineCSSizer.dump_text`: a
/// body gains an unconditional page break and loses any page-break
/// declaration of its own, runs of whitespace collapse, and the result
/// is quoted with single quotes so it can sit inside a double-quoted
/// attribute.
pub fn inline_style_attribute(declared: &str, is_body: bool) -> String {
    let mut css = declared.to_string();
    if is_body {
        // A merged file is a page break, and any other page-break
        // declaration would fight it.
        css = format!(
            "page-break-before: always; {}",
            page_break_pat().replace_all(&css, "")
        );
    }
    let css = whitespace_pat().replace_all(&css, " ").trim().to_string();
    if css.is_empty() {
        return String::new();
    }
    format!(" style=\"{}\"", css.replace('"', "'"))
}

/// The XHTML namespace.
pub const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";
/// The SVG namespace.
pub const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// Resolve `url` against the page that carried it.
///
/// Stands in for the OEB `Item.abshref`.
fn abshref(page_href: &str, url: &str) -> String {
    if url.starts_with('#') {
        return format!("{page_href}{url}");
    }
    if url.contains("://") || url.starts_with('/') {
        return url.to_string();
    }
    let base = match page_href.rfind('/') {
        Some(i) => &page_href[..i + 1],
        None => "",
    };
    let (path, fragment) = match url.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (url, None),
    };
    let combined = format!("{base}{path}");
    let mut parts: Vec<&str> = Vec::new();
    for part in combined.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let joined = parts.join("/");
    match fragment {
        Some(f) => format!("{joined}#{f}"),
        None => joined,
    }
}

fn extension(href: &str) -> String {
    let name = href.rsplit('/').next().unwrap_or(href);
    match name.rfind('.') {
        Some(i) if i > 0 => name[i..].to_string(),
        _ => String::new(),
    }
}

/// The text directly inside an element, before any child element.
fn own_text(elem: Node) -> Option<String> {
    let first = elem.first_child()?;
    first
        .is_text()
        .then(|| first.text().unwrap_or("").to_string())
}

/// The text following an element, before the next one.
fn tail_text(elem: Node) -> Option<String> {
    let next = elem.next_sibling()?;
    if next.is_text() {
        next.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

/// Convert with the CSS dropped.
///
/// Port of the Python `oeb2html_no_css`.
pub fn oeb2html_no_css(oeb: &OEBBook, styles: &dyn StyleProvider) -> Converted {
    Oeb2Html::new().convert(oeb, CssMode::None, styles)
}

/// Convert with the CSS inlined into `style` attributes.
///
/// Port of the Python `oeb2html_inline_css`.
pub fn oeb2html_inline_css(oeb: &OEBBook, styles: &dyn StyleProvider) -> Converted {
    Oeb2Html::new().convert(oeb, CssMode::Inline, styles)
}

/// Convert keeping the classes and the stylesheets.
///
/// Port of the Python `oeb2html_class_css`, which forces the inline
/// class style — the stylesheet goes in a `<style>` element rather than
/// a separate file.
pub fn oeb2html_class_css(oeb: &OEBBook, styles: &dyn StyleProvider) -> Converted {
    Oeb2Html::new().convert(oeb, CssMode::Class { external: false }, styles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use crate::oeb::stylizer::{Stylizer, TagStylizer};
    use anyhow::Result;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemContainer(HashMap<String, Vec<u8>>);

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

    struct Builder {
        oeb: OEBBook,
        next: usize,
    }

    impl Builder {
        fn new() -> Self {
            let mut oeb = OEBBook::new(Box::new(MemContainer::default()));
            oeb.metadata.add("title", "A Book");
            Self { oeb, next: 0 }
        }

        fn part(mut self, href: &str, media_type: &str, content: &[u8], in_spine: bool) -> Self {
            let id = format!("id{}", self.next);
            self.next += 1;
            self.oeb
                .manifest
                .items
                .insert(id.clone(), ManifestItem::new(&id, href, media_type));
            self.oeb.manifest.hrefs.insert(href.to_string(), id.clone());
            self.oeb.container.write(href, content).unwrap();
            if in_spine {
                self.oeb.spine.items.push(SpineItem::new(&id, true));
            }
            self
        }

        fn page(self, href: &str, body: &str) -> Self {
            let content =
                format!(r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>{body}</body></html>"#);
            self.part(href, "application/xhtml+xml", content.as_bytes(), true)
        }

        fn build(self) -> OEBBook {
            self.oeb
        }
    }

    #[test]
    fn each_file_becomes_a_div_with_its_own_anchor() {
        let oeb = Builder::new()
            .page("a.html", "<p>one</p>")
            .page("b.html", "<p>two</p>")
            .build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert_eq!(out.html.matches("<div").count(), 2, "{}", out.html);
        assert!(out.html.contains(r#"id="calibre_link-0""#), "{}", out.html);
        assert!(out.html.contains(r#"id="calibre_link-1""#), "{}", out.html);
        assert!(!out.html.contains("<body><body"), "{}", out.html);
        assert!(out.html.contains("<title>A Book</title>"));
    }

    #[test]
    fn ids_are_renumbered_and_links_follow_them() {
        let oeb = Builder::new()
            .page(
                "a.html",
                r#"<p id="here">one</p><a href="b.html#there">go</a>"#,
            )
            .page("b.html", r#"<p id="there">two</p>"#)
            .build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        // No original id survives.
        assert!(!out.html.contains(r#"id="here""#), "{}", out.html);
        assert!(!out.html.contains(r#"id="there""#), "{}", out.html);
        // And the link points at a renumbered anchor.
        assert!(
            out.html.contains(r##"href="#calibre_link-"##),
            "{}",
            out.html
        );
    }

    #[test]
    fn images_are_flattened_and_numbered_by_href() {
        let oeb = Builder::new()
            .page(
                "text/a.html",
                r#"<p><img src="../img/z.png"/><img src="../img/a.jpg"/></p>"#,
            )
            .part("img/z.png", "image/png", b"png", false)
            .part("img/a.jpg", "image/jpeg", b"jpg", false)
            .build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        // Sorted by href, so a.jpg is 0 and z.png is 1.
        assert_eq!(
            out.images.get("img/a.jpg").map(String::as_str),
            Some("000000.jpg")
        );
        assert_eq!(
            out.images.get("img/z.png").map(String::as_str),
            Some("000001.png")
        );
        assert!(
            out.html.contains(r#"src="images/000001.png""#),
            "{}",
            out.html
        );
        assert!(
            out.html.contains(r#"src="images/000000.jpg""#),
            "{}",
            out.html
        );
    }

    #[test]
    fn fonts_are_flattened_too() {
        let oeb = Builder::new()
            .page("a.html", "<p>x</p>")
            .part("fonts/x.ttf", "application/x-font-ttf", b"ttf", false)
            .build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert_eq!(
            out.fonts.get("fonts/x.ttf").map(String::as_str),
            Some("000000.ttf")
        );
    }

    #[test]
    fn the_no_css_flavour_turns_styles_back_into_tags() {
        let oeb = Builder::new()
            .page(
                "a.html",
                r#"<p style="font-weight: bold">b</p><p style="font-style: italic">i</p>
                   <p style="text-decoration: underline">u</p><p style="text-decoration: line-through">s</p>"#,
            )
            .build();
        let out = oeb2html_no_css(&oeb, &Stylizer::new(96.0, 12.0));
        assert!(out.html.contains("<b>"), "{}", out.html);
        assert!(out.html.contains("<i>"), "{}", out.html);
        assert!(out.html.contains("<u>"), "{}", out.html);
        assert!(out.html.contains("<s>"), "{}", out.html);
        // And the style attribute itself is gone.
        assert!(!out.html.contains("style="), "{}", out.html);
    }

    #[test]
    fn the_inline_flavour_writes_a_style_attribute() {
        let oeb = Builder::new()
            .page("a.html", r#"<p style="color: red">x</p>"#)
            .build();
        let out = oeb2html_inline_css(&oeb, &Stylizer::new(96.0, 12.0));
        assert!(out.html.contains(r#"style="color: red""#), "{}", out.html);
        // The merged file's div always breaks the page.
        assert!(
            out.html.contains("page-break-before: always"),
            "{}",
            out.html
        );
        // And no <b>/<i> remapping happens in this flavour.
        assert!(!out.html.contains("<b>"), "{}", out.html);
    }

    #[test]
    fn an_inline_body_style_loses_its_own_page_breaks() {
        let content = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body style="page-break-after: avoid; color: blue">
            <p>x</p></body></html>"#;
        let oeb = Builder::new()
            .part("a.html", "application/xhtml+xml", content.as_bytes(), true)
            .build();
        let out = oeb2html_inline_css(&oeb, &Stylizer::new(96.0, 12.0));
        assert!(
            out.html.contains("page-break-before: always"),
            "{}",
            out.html
        );
        assert!(!out.html.contains("page-break-after"), "{}", out.html);
        assert!(out.html.contains("color: blue"), "{}", out.html);
    }

    #[test]
    fn the_class_flavour_keeps_classes_and_emits_the_stylesheet() {
        let oeb = Builder::new()
            .page("a.html", r#"<p class="first">x</p>"#)
            .part("style.css", "text/css", b"p { color: red }", false)
            .build();
        let out = oeb2html_class_css(&oeb, &TagStylizer);
        assert!(out.html.contains(r#"class="first""#), "{}", out.html);
        assert!(
            out.html.contains("<style type=\"text/css\">"),
            "{}",
            out.html
        );
        assert!(out.html.contains("p { color: red }"), "{}", out.html);
    }

    #[test]
    fn the_external_class_flavour_links_a_stylesheet_instead() {
        let oeb = Builder::new()
            .page("a.html", "<p>x</p>")
            .part("style.css", "text/css", b"p { color: red }", false)
            .build();
        let out = Oeb2Html::new().convert(&oeb, CssMode::Class { external: true }, &TagStylizer);
        assert!(
            out.html.contains(r#"<link href="style.css""#),
            "{}",
            out.html
        );
        assert!(!out.html.contains("<style"), "{}", out.html);
        // The stylesheet still comes back, for the caller to write out.
        assert!(out.css.contains("p { color: red }"));
    }

    #[test]
    fn stylesheet_urls_are_rewritten_to_the_flat_names() {
        let oeb = Builder::new()
            .page("a.html", r#"<p><img src="img/a.png"/></p>"#)
            .part("img/a.png", "image/png", b"png", false)
            .part(
                "s.css",
                "text/css",
                b"body { background: url(img/a.png) }",
                false,
            )
            .build();
        let out = oeb2html_class_css(&oeb, &TagStylizer);
        assert!(out.css.contains("url(images/000000.png)"), "{}", out.css);
    }

    #[test]
    fn hidden_content_is_dropped() {
        let oeb = Builder::new()
            .page(
                "a.html",
                r#"<p>keep</p><p style="display: none">drop</p><p style="visibility: hidden">gone</p>"#,
            )
            .build();
        let out = oeb2html_no_css(&oeb, &Stylizer::new(96.0, 12.0));
        assert!(out.html.contains("keep"), "{}", out.html);
        assert!(!out.html.contains("drop"), "{}", out.html);
        assert!(!out.html.contains("gone"), "{}", out.html);
    }

    #[test]
    fn self_closing_tags_are_written_without_a_closing_tag() {
        let oeb = Builder::new().page("a.html", "<p>a<br/>b</p><hr/>").build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert!(out.html.contains("<br />"), "{}", out.html);
        assert!(!out.html.contains("</br>"), "{}", out.html);
        assert!(out.html.contains("<hr />"), "{}", out.html);
    }

    #[test]
    fn text_is_escaped_and_the_named_entities_are_spelled_out() {
        assert_eq!(
            Oeb2Html::prepare_string_for_html("a & b < c"),
            "a &amp; b &lt; c"
        );
        assert_eq!(Oeb2Html::prepare_string_for_html("\u{00ad}"), "&shy;");
        assert_eq!(Oeb2Html::prepare_string_for_html("\u{2014}"), "&mdash;");
        assert_eq!(Oeb2Html::prepare_string_for_html("\u{2013}"), "&ndash;");
        assert_eq!(Oeb2Html::prepare_string_for_html("\u{00a0}"), "&nbsp;");
    }

    #[test]
    fn tail_text_survives_its_element() {
        let oeb = Builder::new()
            .page("a.html", "<p>before<b>bold</b>after</p>")
            .build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert!(out.html.contains("before"), "{}", out.html);
        assert!(out.html.contains("after"), "{}", out.html);
    }

    #[test]
    fn hrefs_resolve_against_the_page() {
        assert_eq!(abshref("text/a.html", "b.html"), "text/b.html");
        assert_eq!(abshref("text/a.html", "../img/x.png"), "img/x.png");
        assert_eq!(abshref("a.html", "#frag"), "a.html#frag");
        assert_eq!(abshref("text/a.html", "b.html#f"), "text/b.html#f");
        assert_eq!(
            abshref("a.html", "https://example.com/x"),
            "https://example.com/x"
        );
    }

    #[test]
    fn a_missing_or_unparseable_spine_item_is_skipped() {
        let mut oeb = Builder::new().page("good.html", "<p>x</p>").build();
        oeb.manifest.items.insert(
            "gone".to_string(),
            ManifestItem::new("gone", "gone.html", "application/xhtml+xml"),
        );
        oeb.spine.items.push(SpineItem::new("gone", true));
        oeb.container.write("bad.html", b"<not <xml").unwrap();
        oeb.manifest.items.insert(
            "bad".to_string(),
            ManifestItem::new("bad", "bad.html", "application/xhtml+xml"),
        );
        oeb.spine.items.push(SpineItem::new("bad", true));

        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert!(out.html.contains("x"), "the readable file still converts");
    }

    #[test]
    fn a_book_with_no_spine_still_produces_a_document() {
        let oeb = Builder::new().build();
        let out = oeb2html_no_css(&oeb, &TagStylizer);
        assert!(out.html.starts_with("<html><head>"));
        assert!(out.html.ends_with("</body></html>"));
    }

    #[test]
    fn link_ids_are_handed_out_in_order_and_reused() {
        let mut izer = Oeb2Html::new();
        assert_eq!(izer.get_link_id("a.html", ""), "#calibre_link-0");
        assert_eq!(izer.get_link_id("a.html", "x"), "#calibre_link-1");
        assert_eq!(izer.get_link_id("a.html", ""), "#calibre_link-0", "reused");
        assert_eq!(izer.get_link_id("b.html", ""), "#calibre_link-2");
    }
}
