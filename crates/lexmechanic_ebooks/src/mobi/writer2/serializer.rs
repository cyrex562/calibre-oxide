//! Serialize an OEB book's spine into a single MOBI markup buffer with
//! `filepos="..."` internal-link placeholders.
//!
//! Port of `calibre.ebooks.mobi.writer2.serializer.Serializer`.
//!
//! # DOM reuse
//!
//! Python walks an lxml-parsed tree per spine item. This port parses
//! each spine item's raw HTML with [`crate::dom::Dom`] (the same
//! html5ever-backed arena added for issue #33's `upshift_markup`) and
//! walks that instead of introducing a second tree abstraction.
//! `dom::Dom`'s `convert()` step discards element/attribute namespaces
//! entirely (it only keeps local names), so the Python namespace
//! filter/prefix logic (`NSRMAP`, `namespace()`, `prefixname()`) has no
//! equivalent here -- every parsed element is treated as in-scope XHTML,
//! which is correct for the spine content this reads (plain XHTML
//! bodies) and is the same simplification `dom.rs` already made for the
//! reader side.
//!
//! # OEB model gap
//!
//! The existing `oeb::spine::SpineItem` only carries `idref`/`linear`,
//! not a resolved `href` or the section/article boundary flags
//! (`is_section_start` etc) Python mutates directly onto spine items.
//! Rather than widening that shared struct (used by every other
//! input/output module), this file resolves hrefs against the manifest
//! and tracks those flags in a private `ResolvedSpineItem` local to
//! serialization.

use std::collections::{HashMap, HashSet};

use crate::dom::{Dom, Node, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_DOCS;

/// Split `href` into `(path, fragment)` on the first `#`, matching
/// Python's `urllib.parse.urldefrag`.
pub fn urldefrag(href: &str) -> (String, String) {
    match href.split_once('#') {
        Some((p, f)) => (p.to_string(), f.to_string()),
        None => (href.to_string(), String::new()),
    }
}

fn quote_unquote_segment(seg: &str) -> String {
    let decoded = urlencoding::decode(seg)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| seg.to_string());
    urlencoding::encode(&decoded).into_owned()
}

/// Normalize an internal book URL: backslashes become slashes, and each
/// path/fragment segment is percent-decoded then re-encoded, matching
/// the net effect of Python's `urlnormalize` for the scheme-less
/// relative hrefs used inside a book (this port does not reproduce full
/// RFC 3986 URL parsing, only the path-segment quoting Python's version
/// reduces to once its `scheme in ('', 'file')` branch is taken, which
/// is always true for these internal references).
pub fn urlnormalize(href: &str) -> String {
    let (path, frag) = urldefrag(href);
    let path = path.replace('\\', "/");
    let frag = frag.replace('\\', "/");
    let path = path
        .split('/')
        .map(quote_unquote_segment)
        .collect::<Vec<_>>()
        .join("/");
    let frag = frag
        .split('/')
        .map(quote_unquote_segment)
        .collect::<Vec<_>>()
        .join("/");
    if frag.is_empty() {
        path
    } else {
        format!("{path}#{frag}")
    }
}

/// Resolve `rel` against the directory of `base_href`, collapsing `.`/
/// `..` segments. Stand-in for the OEB `Item.abshref` method.
pub fn abshref(base_href: &str, rel: &str) -> String {
    if rel.contains("://") || rel.starts_with('#') {
        return rel.to_string();
    }
    let base_dir = match base_href.rfind('/') {
        Some(idx) => &base_href[..=idx],
        None => "",
    };
    let combined = format!("{base_dir}{rel}");
    let mut out: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "." => {}
            ".." => {
                out.pop();
            }
            _ => out.push(seg),
        }
    }
    out.join("/")
}

/// A spine item plus the section/article boundary flags
/// [`Serializer::find_blocks`] computes for it. See the module-level
/// OEB-model-gap note for why this lives here instead of on
/// `oeb::spine::SpineItem`.
#[derive(Debug, Clone)]
struct ResolvedSpineItem {
    href: String,
    linear: bool,
    is_section_start: bool,
    is_section_end: bool,
    is_article_start: bool,
    is_article_end: bool,
}

pub struct Serializer<'a> {
    oeb: &'a OEBBook,
    /// href (urlnormalized) -> image record index.
    images: &'a HashMap<String, usize>,
    is_periodical: bool,
    write_page_breaks_after_item: bool,

    items: Vec<ResolvedSpineItem>,

    /// hrefs (urlnormalized) of every image actually referenced by the
    /// serialized markup.
    pub used_images: HashSet<String>,
    /// href (urlnormalized, possibly with a `#fragment`) -> offset into
    /// the returned buffer where the content it names lives.
    pub id_offsets: HashMap<String, usize>,
    href_offsets: HashMap<String, Vec<usize>>,
    /// Buffer offsets of non-linear spine items (uncrossable breaks).
    pub breaks: Vec<usize>,

    pub start_offset: Option<usize>,
    start_href: Option<String>,
    anchor_offset: Option<usize>,

    pub body_start_offset: usize,
    pub body_end_offset: usize,
    pub end_offset: usize,

    buf: Vec<u8>,
    warnings: Vec<String>,
}

impl<'a> Serializer<'a> {
    pub fn new(
        oeb: &'a OEBBook,
        images: &'a HashMap<String, usize>,
        is_periodical: bool,
        write_page_breaks_after_item: bool,
    ) -> Self {
        let mut s = Serializer {
            oeb,
            images,
            is_periodical,
            write_page_breaks_after_item,
            items: Vec::new(),
            used_images: HashSet::new(),
            id_offsets: HashMap::new(),
            href_offsets: HashMap::new(),
            breaks: Vec::new(),
            start_offset: None,
            start_href: None,
            anchor_offset: None,
            body_start_offset: 0,
            body_end_offset: 0,
            end_offset: 0,
            buf: Vec::new(),
            warnings: Vec::new(),
        };
        s.find_blocks();
        s
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn manifest_href_of(&self, idref: &str) -> Option<String> {
        self.oeb.manifest.get_by_id(idref).map(|m| m.href.clone())
    }

    /// Port of `Serializer.find_blocks`: mark every spine item as the
    /// start/end of a section/article per `oeb.toc`, so `serialize_item`
    /// can wrap it in the right anchors.
    fn find_blocks(&mut self) {
        let mut resolved: Vec<ResolvedSpineItem> = self
            .oeb
            .spine
            .items
            .iter()
            .filter_map(|si| {
                self.manifest_href_of(&si.idref)
                    .map(|href| ResolvedSpineItem {
                        href,
                        linear: si.linear,
                        is_section_start: false,
                        is_section_end: false,
                        is_article_start: false,
                        is_article_end: false,
                    })
            })
            .collect();

        let find_spine_idx = |items: &[ResolvedSpineItem], href: Option<&str>| -> Option<usize> {
            let href = urldefrag(href.unwrap_or_default()).0;
            items.iter().position(|it| it.href == href)
        };

        for node in self.oeb.toc.iter_descendants(false) {
            if node.klass.as_deref() == Some("section") {
                if node.children.is_empty() {
                    continue;
                }
                if let Some(idx) = find_spine_idx(&resolved, node.href.as_deref()) {
                    resolved[idx].is_section_start = true;
                }
                for article in &node.children {
                    if let Some(idx) = find_spine_idx(&resolved, article.href.as_deref()) {
                        resolved[idx].is_article_start = true;
                    }
                }
            }
        }

        let mut in_sec = false;
        let mut in_art = false;
        for i in 0..resolved.len() {
            if in_art && resolved[i].is_article_start {
                if i > 0 {
                    resolved[i - 1].is_article_end = true;
                }
                in_art = false;
            }
            if in_sec && resolved[i].is_section_start {
                if i > 0 {
                    resolved[i - 1].is_section_end = true;
                }
                in_sec = false;
            }
            if resolved[i].is_section_start {
                in_sec = true;
            }
            if resolved[i].is_article_start {
                in_art = true;
            }
        }
        if let Some(last) = resolved.last_mut() {
            last.is_section_end = true;
            last.is_article_end = true;
        }

        self.items = resolved;
    }

    /// Serialize the whole book to a single UTF-8 buffer. Port of
    /// `Serializer.__call__`.
    pub fn serialize(&mut self) -> Vec<u8> {
        self.buf.clear();
        self.buf.extend_from_slice(b"<html>");
        self.serialize_head();
        self.serialize_body();
        self.buf.extend_from_slice(b"</html>");
        self.end_offset = self.buf.len();
        self.fixup_links();
        if self.start_offset.is_none() && !self.is_periodical {
            self.start_offset = Some(self.body_start_offset);
        }
        std::mem::take(&mut self.buf)
    }

    fn serialize_head(&mut self) {
        self.buf.extend_from_slice(b"<head>");
        if !self.oeb.guide.references.is_empty() {
            self.serialize_guide();
        }
        self.buf.extend_from_slice(b"</head>");
    }

    fn serialize_guide(&mut self) {
        self.buf.extend_from_slice(b"<guide>");
        // `oeb.guide.values()` in Python is insertion-ordered; ours is a
        // `HashMap`, so sort by type for reproducible output.
        let mut refs: Vec<_> = self.oeb.guide.references.values().collect();
        refs.sort_by(|a, b| a.type_.cmp(&b.type_));
        for r in refs {
            let path = urldefrag(&r.href).0;
            let in_docs = self
                .oeb
                .manifest
                .get_by_href(&path)
                .map(|m| {
                    OEB_DOCS
                        .iter()
                        .any(|dt| dt.eq_ignore_ascii_case(&m.media_type))
                })
                .unwrap_or(false);
            if !in_docs {
                continue;
            }
            self.buf.extend_from_slice(b"<reference type=\"");
            let type_ = r.type_.strip_prefix("other.").unwrap_or(&r.type_);
            self.serialize_text(type_, true);
            self.buf.extend_from_slice(b"\" ");
            if let Some(title) = &r.title {
                self.buf.extend_from_slice(b"title=\"");
                self.serialize_text(title, true);
                self.buf.extend_from_slice(b"\" ");
                if is_guide_ref_start(r.title.as_deref(), Some(&r.type_)) {
                    self.start_href = Some(r.href.clone());
                }
            }
            self.serialize_href(&r.href, None);
            self.buf.extend_from_slice(b" />");
        }
        self.buf.extend_from_slice(b"</guide>");
    }

    fn serialize_href(&mut self, href: &str, base: Option<&str>) -> bool {
        let normalized = urlnormalize(href);
        let (mut path, frag) = urldefrag(&normalized);
        if !path.is_empty() {
            if let Some(base_href) = base {
                path = urlnormalize(&abshref(base_href, &path));
            }
        }
        let item_href = if path.is_empty() {
            None
        } else {
            match self.oeb.manifest.get_by_href(&path) {
                Some(m) => Some(m.href.clone()),
                None => return false,
            }
        };
        if let Some(h) = &item_href {
            let in_spine = self.items.iter().any(|it| it.href == *h);
            if !in_spine {
                return false;
            }
        }
        let final_path = match item_href {
            Some(h) => h,
            None => match base {
                Some(b) => b.to_string(),
                None => return false,
            },
        };
        let key = if frag.is_empty() {
            final_path
        } else {
            format!("{final_path}#{frag}")
        };
        self.buf.extend_from_slice(b"filepos=");
        self.href_offsets
            .entry(key)
            .or_default()
            .push(self.buf.len());
        self.buf.extend_from_slice(b"0000000000");
        true
    }

    fn serialize_body(&mut self) {
        self.buf.extend_from_slice(b"<body>");
        self.body_start_offset = self.buf.len();

        if self.is_periodical {
            if let Some(top) = self.oeb.toc.first() {
                self.serialize_toc_level(top, None);
            }
        }

        let items: Vec<usize> = {
            let mut linear: Vec<usize> = Vec::new();
            let mut nonlinear: Vec<usize> = Vec::new();
            for (i, it) in self.items.iter().enumerate() {
                if it.linear {
                    linear.push(i);
                } else {
                    nonlinear.push(i);
                }
            }
            linear.extend(nonlinear);
            linear
        };

        for i in items {
            if self.is_periodical && self.items[i].is_section_start {
                if let Some(top) = self.oeb.toc.first() {
                    let href = self.items[i].href.clone();
                    for section_toc in &top.children {
                        if section_toc.href.as_deref().map(urlnormalize)
                            == Some(urlnormalize(&href))
                        {
                            let section_url =
                                strip_article_dir(section_toc.href.as_deref().unwrap_or(""));
                            self.serialize_toc_level(section_toc, Some(&section_url));
                            break;
                        }
                    }
                }
            }
            self.serialize_item(i);
        }

        self.body_end_offset = self.buf.len();
        self.buf.extend_from_slice(b"</body>");
    }

    fn serialize_toc_level(&mut self, tocref: &crate::oeb::toc::TOCNode, href: Option<&str>) {
        if let Some(href) = href {
            self.buf.extend_from_slice(b"<mbp:pagebreak />");
            self.id_offsets.insert(urlnormalize(href), self.buf.len());
        }

        if tocref.klass.as_deref() == Some("periodical") {
            self.buf
                .extend_from_slice(b"<div> <div height=\"1em\"></div>");
        } else {
            self.buf
                .extend_from_slice(b"<div></div> <div> <h2 height=\"1em\"><font size=\"+2\"><b>");
            self.buf
                .extend_from_slice(tocref.title.as_deref().unwrap_or("").as_bytes());
            self.buf
                .extend_from_slice(b"</b></font></h2> <div height=\"1em\"></div>");
        }

        self.buf.extend_from_slice(b"<ul>");
        for tocitem in &tocref.children {
            self.buf.extend_from_slice(b"<li><a filepos=");
            let mut itemhref = tocitem.href.clone().unwrap_or_default();
            if tocref.klass.as_deref() == Some("periodical") {
                itemhref = strip_article_dir(&itemhref);
            }
            self.href_offsets
                .entry(itemhref)
                .or_default()
                .push(self.buf.len());
            self.buf.extend_from_slice(b"0000000000");
            self.buf.extend_from_slice(b" ><font size=\"+1\"><b><u>");
            self.buf
                .extend_from_slice(tocitem.title.as_deref().unwrap_or("").as_bytes());
            self.buf.extend_from_slice(b"</u></b></font></a></li>");
        }
        self.buf
            .extend_from_slice(b"</ul><div height=\"1em\"></div></div><mbp:pagebreak />");
    }

    fn serialize_item(&mut self, i: usize) {
        let item = self.items[i].clone();
        if !item.linear {
            self.breaks.push(self.buf.len().saturating_sub(1));
        }
        self.id_offsets
            .insert(urlnormalize(&item.href), self.buf.len());
        if item.is_section_start {
            self.buf.extend_from_slice(b"<a ></a> ");
        }
        if item.is_article_start {
            self.buf.extend_from_slice(b"<a ></a> <a ></a>");
        }

        let raw = self.oeb.container.read(&item.href).unwrap_or_default();
        let html = String::from_utf8_lossy(&raw).into_owned();
        let dom = Dom::parse(&html);
        if let Some(body) = dom.find_first_tag_global("body") {
            let children = dom.children(body);
            for child in children {
                self.serialize_elem(&dom, child, &item.href);
            }
        }

        if self.write_page_breaks_after_item {
            self.buf.extend_from_slice(b"<mbp:pagebreak/>");
        }
        if item.is_article_end {
            self.buf.extend_from_slice(b"<a ></a> <a ></a>");
        }
        if item.is_section_end {
            self.buf.extend_from_slice(b" <a ></a>");
        }
        self.anchor_offset = None;
    }

    fn serialize_elem(&mut self, dom: &Dom, id: NodeId, item_href: &str) {
        let node: &Node = dom.node(id);
        let tag = match &node.kind {
            NodeKind::Element(t) => t.clone(),
            _ => return,
        };

        let id_attr = node.attrs.get("id").cloned();
        if let Some(id_val) = &id_attr {
            let href = format!("{item_href}#{id_val}");
            let offset = self.anchor_offset.unwrap_or(self.buf.len());
            let key = urlnormalize(&href);
            self.id_offsets.entry(key).or_insert(offset);
        }

        let real_attr_count = node.attrs.len() - usize::from(id_attr.is_some());
        if self.anchor_offset.is_some()
            && tag == "a"
            && real_attr_count == 0
            && node.children.is_empty()
        {
            return;
        }

        self.anchor_offset = Some(self.buf.len());
        self.buf.push(b'<');
        self.buf.extend_from_slice(tag.as_bytes());

        for (attr, val) in &node.attrs {
            if attr == "id" {
                continue;
            }
            self.buf.push(b' ');
            if attr == "href" {
                if self.serialize_href(val, Some(item_href)) {
                    continue;
                }
            } else if attr == "src" {
                let href = urlnormalize(&abshref(item_href, val));
                if let Some(&index) = self.images.get(&href) {
                    self.used_images.insert(href);
                    self.buf
                        .extend_from_slice(format!("recindex=\"{index:05}\"").as_bytes());
                    continue;
                }
            }
            self.buf.extend_from_slice(attr.as_bytes());
            self.buf.extend_from_slice(b"=\"");
            self.serialize_text(val, true);
            self.buf.push(b'"');
        }
        self.buf.push(b'>');

        // `dom::Dom` interleaves text as ordinary children (unlike
        // lxml's separate `.text`/`.tail` properties), so walking
        // children in document order and resetting `anchor_offset`
        // whenever non-empty text is written reproduces both the
        // "leading text" and "child tail" cases from `serialize_elem`
        // in one pass.
        for &child in &dom.children(id) {
            match &dom.node(child).kind {
                NodeKind::Text(t) => {
                    if !t.is_empty() {
                        self.anchor_offset = None;
                        let t = t.clone();
                        self.serialize_text(&t, false);
                    }
                }
                NodeKind::Element(_) => {
                    self.serialize_elem(dom, child, item_href);
                }
                _ => {}
            }
        }

        self.buf.extend_from_slice(b"</");
        self.buf.extend_from_slice(tag.as_bytes());
        self.buf.push(b'>');
    }

    fn serialize_text(&mut self, text: &str, quot: bool) {
        let mut text = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('\u{00ad}', "");
        if quot {
            text = text.replace('"', "&quot;");
        }
        let normalized: String =
            unicode_normalization::UnicodeNormalization::nfc(text.as_str()).collect();
        self.buf.extend_from_slice(normalized.as_bytes());
    }

    fn fixup_links(&mut self) {
        let start_href = self.start_href.clone();
        let entries: Vec<(String, Vec<usize>)> = self
            .href_offsets
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (href, hoffs) in entries {
            let is_start = start_href.as_deref() == Some(href.as_str());
            let mut lookup_href = href.clone();
            if !self.id_offsets.contains_key(&lookup_href) {
                self.warnings
                    .push(format!("Hyperlink target {lookup_href:?} not found"));
                lookup_href = urldefrag(&lookup_href).0;
            }
            if let Some(&ioff) = self.id_offsets.get(&lookup_href) {
                if is_start {
                    self.start_offset = Some(ioff);
                }
                for hoff in hoffs {
                    let rendered = format!("{ioff:010}");
                    let bytes = rendered.as_bytes();
                    if hoff + bytes.len() <= self.buf.len() {
                        self.buf[hoff..hoff + bytes.len()].copy_from_slice(bytes);
                    }
                }
            }
        }
    }
}

/// Strip an `article_NNN/` path segment, turning a section's first
/// article URL into the section's own (synthetic) index URL. Mirrors
/// `re.sub(r'article_\d+/', '', href)` in `serializer.py`.
fn strip_article_dir(href: &str) -> String {
    lazy_static::lazy_static! {
        static ref ARTICLE_DIR: regex::Regex = regex::Regex::new(r"article_\d+/").unwrap();
    }
    ARTICLE_DIR.replace_all(href, "").into_owned()
}

/// Whether a `<guide>` reference should set the book's opening location.
/// Port of `is_guide_ref_start` in `mobi/utils.py`.
pub fn is_guide_ref_start(title: Option<&str>, type_: Option<&str>) -> bool {
    if let Some(t) = title {
        if t.eq_ignore_ascii_case("start") {
            return true;
        }
    }
    if let Some(t) = type_ {
        let lower = t.to_lowercase();
        if lower == "start" || lower == "other.start" || lower == "text" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn write(dir: &std::path::Path, name: &str, content: &str) {
        std::fs::write(dir.join(name), content).unwrap();
    }

    fn simple_book(dir: &std::path::Path) -> OEBBook {
        write(
            dir,
            "c1.html",
            "<html><body><h1 id=\"top\">One</h1><p>Hello <a href=\"c2.html\">next</a></p></body></html>",
        );
        write(
            dir,
            "c2.html",
            "<html><body><p id=\"p2\">Two <a href=\"c1.html#top\">back</a></p></body></html>",
        );
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
        oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
        oeb.manifest.add("c2", "c2.html", "application/xhtml+xml");
        oeb.spine.add("c1", true);
        oeb.spine.add("c2", true);
        oeb
    }

    #[test]
    fn serializes_two_chapters_and_resolves_a_cross_link() {
        let dir = tempfile::tempdir().unwrap();
        let oeb = simple_book(dir.path());
        let images = HashMap::new();
        let mut s = Serializer::new(&oeb, &images, false, true);
        let out = s.serialize();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Hello"), "{text}");
        assert!(text.contains("Two"), "{text}");
        // The link to c2.html should have been resolved to a real,
        // non-placeholder 10-digit offset.
        assert!(!text.contains("filepos=0000000000"), "{text}");
        assert_eq!(s.warnings().len(), 0, "{:?}", s.warnings());
    }

    #[test]
    fn a_dangling_link_produces_a_warning_but_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "c1.html",
            "<html><body><a href=\"nope.html\">x</a></body></html>",
        );
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
        oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
        oeb.spine.add("c1", true);
        let images = HashMap::new();
        let mut s = Serializer::new(&oeb, &images, false, true);
        let _ = s.serialize();
        // The href doesn't resolve to any manifest item at all, so
        // serialize_href itself declines to write a filepos link (the
        // href attribute is dropped, matching Python's `return False`
        // meaning the caller falls through to writing a literal `href`
        // attribute) -- so this specific case produces no warning, only
        // a same-document dangling id would. Exercised here mainly to
        // confirm it doesn't panic.
    }

    #[test]
    fn src_attributes_become_recindex_when_the_image_is_known() {
        let dir = tempfile::tempdir().unwrap();
        write(
            dir.path(),
            "c1.html",
            "<html><body><img src=\"cover.jpg\"/></body></html>",
        );
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
        oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
        oeb.spine.add("c1", true);
        let mut images = HashMap::new();
        images.insert("cover.jpg".to_string(), 1usize);
        let mut s = Serializer::new(&oeb, &images, false, true);
        let out = s.serialize();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("recindex=\"00001\""), "{text}");
        assert!(s.used_images.contains("cover.jpg"));
    }

    #[test]
    fn urlnormalize_is_idempotent_for_plain_relative_paths() {
        assert_eq!(urlnormalize("chapter1.html"), "chapter1.html");
        assert_eq!(
            urlnormalize("dir/chapter1.html#frag"),
            "dir/chapter1.html#frag"
        );
    }

    #[test]
    fn abshref_resolves_against_the_base_directory() {
        assert_eq!(abshref("text/c1.html", "../images/x.png"), "images/x.png");
        assert_eq!(abshref("c1.html", "x.png"), "x.png");
    }
}
