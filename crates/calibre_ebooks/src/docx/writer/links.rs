//! Bookmarks and hyperlinks: port of `docx/writer/links.py`'s
//! `start_text`, `sanitize_bookmark_name`, [`TocItem`] (Python
//! `TOCItem`), and all of [`LinksManager`] (`__init__`,
//! `bookmark_for_anchor`, `bookmark_id`, `serialize_hyperlink`,
//! `process_toc_node`, `process_toc_links`, `serialize_toc`) --
//! `LinksManager` is now fully ported.
//!
//! `serialize_toc` needed `<w:body>` to support prepending a child
//! (Python's `body.insert(0, p)`); [`super::xml::Element`] gained
//! `insert`/`find_descendant_mut` for this.
//!
//! **Not yet wired up**: `LinksManager`'s only caller,
//! `Convert.write` (`from_html.py`'s `Convert` orchestrator, issue
//! #132), isn't ported yet -- there is no real call site to exercise
//! this against beyond the tests here.
//!
//! Python's `urlparse` is not ported in general; [`parse_link_url`]
//! is a narrow stand-in covering exactly what `serialize_hyperlink`
//! needs (scheme detection plus the pre-fragment path for a relative
//! href) -- not a general RFC 3986 parser (no query string, userinfo,
//! or port handling).

use std::collections::{HashMap, HashSet};

use crate::docx::names::DocxNamespace;
use crate::docx::writer::container::DocumentRelationships;
use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::polish::check::parsing::urlquote;
use crate::oeb::toc::{TOCNode, TOC};
use crate::oeb::transforms::filenames::abshref;

use super::xml::Element;

/// Preorder-concatenates every `Text` descendant of `node`, in
/// document order. **Deliberately not [`Dom::text_content`]** -- that
/// helper currently returns text in *reverse* document order for any
/// node with more than one text-bearing descendant (issue #296, a
/// real pre-existing bug found while writing this function's tests,
/// left unfixed here since fixing it needs its own PR: ~40 call sites
/// across unrelated subsystems, most of which happen not to notice
/// because they only check emptiness or read a single-text-node
/// element). `start_text` routinely walks multi-node headings (e.g.
/// `<h1>Chapter <i>One</i></h1>`), where the bug would matter, so it
/// gets its own small correct walk instead of depending on the
/// broken shared one.
fn text_content_in_order(dom: &Dom, node: NodeId, out: &mut String) {
    if let NodeKind::Text(t) = &dom.node(node).kind {
        out.push_str(t);
    }
    for &child in &dom.node(node).children {
        text_content_in_order(dom, child, out);
    }
}

/// Port of `start_text`: the first ~50 characters of `node`'s
/// flattened text content, truncated with a trailing `...` if there
/// was more. Python builds this by manually walking `tag.text` plus
/// each child's own (recursively truncated) text and `tail`, stopping
/// early once the 50-character budget is spent -- a performance
/// optimization only. The two produce the same *result* (early
/// stopping never changes which characters end up in the prefix), so
/// this builds the full flattened text once and slices it instead of
/// reproducing the recursive early-stop walk.
pub fn start_text(dom: &Dom, node: NodeId) -> String {
    const LIMIT: usize = 50;
    let mut full = String::new();
    text_content_in_order(dom, node, &mut full);
    if full.chars().count() > LIMIT {
        let mut truncated: String = full.chars().take(LIMIT).collect();
        truncated.push_str("...");
        truncated
    } else {
        full
    }
}

/// Port of `sanitize_bookmark_name`: transliterates to ASCII, replaces
/// every non-alphanumeric character with `_`, caps the length at 32
/// (Word's real limit is 40; 32 leaves room for a uniquifying
/// `_<n>` suffix), and strips trailing underscores.
pub fn sanitize_bookmark_name(base: &str) -> String {
    let ascii = calibre_utils::filenames::ascii_text(base);
    let mut out = String::with_capacity(ascii.len().min(32));
    for ch in ascii.chars() {
        if out.chars().count() >= 32 {
            break;
        }
        out.push(if ch.is_ascii_alphanumeric() { ch } else { '_' });
    }
    out.trim_end_matches('_').to_string()
}

fn basename(href: &str) -> &str {
    match href.rfind('/') {
        Some(idx) => &href[idx + 1..],
        None => href,
    }
}

/// Narrow stand-in for `urllib.parse.urlparse`, covering only what
/// [`LinksManager::serialize_hyperlink`] needs: the scheme (per RFC
/// 3986's `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )` grammar, so a
/// Windows-style `C:\...` path or a `12:30` string doesn't get
/// mistaken for a scheme), and the path portion before any `#`
/// fragment (with a `?` query, if present, also stripped -- this
/// module never needs one). Returns `(scheme, path, fragment)`;
/// `scheme` is lowercased, `fragment` is `""` when absent.
fn parse_link_url(url: &str) -> (Option<String>, String, String) {
    let (before_fragment, fragment) = match url.split_once('#') {
        Some((p, f)) => (p, f.to_string()),
        None => (url, String::new()),
    };
    let scheme = url_scheme(before_fragment);
    let rest = match &scheme {
        Some(s) => &before_fragment[s.len() + 1..],
        None => before_fragment,
    };
    let path = if scheme.is_some() {
        let after_authority = match rest.strip_prefix("//") {
            Some(r) => r.find('/').map(|i| &r[i..]).unwrap_or(""),
            None => rest,
        };
        after_authority.split('?').next().unwrap_or("").to_string()
    } else {
        rest.split('?').next().unwrap_or("").to_string()
    };
    (scheme, path, fragment)
}

fn url_scheme(url: &str) -> Option<String> {
    let colon = url.find(':')?;
    let candidate = &url[..colon];
    let mut chars = candidate.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        return None;
    }
    Some(candidate.to_ascii_lowercase())
}

/// Port of `TOCItem`: one Word-native "Table of Contents" entry
/// (Python's `field code` TOC, not a real navigable OOXML TOC part).
#[derive(Debug, Clone)]
pub struct TocItem {
    pub title: String,
    pub bmark: String,
    pub level: u32,
    pub is_first: bool,
    pub is_last: bool,
}

impl TocItem {
    pub fn new(title: impl Into<String>, bmark: impl Into<String>, level: u32) -> Self {
        TocItem {
            title: title.into(),
            bmark: bmark.into(),
            level,
            is_first: false,
            is_last: false,
        }
    }

    /// Port of `TOCItem.serialize`. Python appends the returned `<w:p>`
    /// at position 0 of `<w:body>`; that insertion (and the resulting
    /// document-order-via-repeated-prepend it relies on) is
    /// [`LinksManager::serialize_toc`]'s job, not this method's.
    pub fn serialize(&self) -> Element {
        let mut ppr = Element::new("w:pPr")
            .with(Element::new("w:pStyle").attr("w:val", "Normal"))
            .with(
                Element::new("w:ind")
                    .attr("w:left", "0")
                    .attr("w:firstLineChars", "0")
                    .attr("w:firstLine", "0")
                    .attr("w:leftChars", (200 * self.level).to_string()),
            );
        let mut p = Element::new("w:p");
        if self.is_first {
            ppr.append(Element::new("w:pageBreakBefore").attr("w:val", "off"));
        }
        p.append(ppr);
        if self.is_first {
            p.append(
                Element::new("w:r").with(Element::new("w:fldChar").attr("w:fldCharType", "begin")),
            );
            p.append(
                Element::new("w:r").with(
                    Element::new("w:instrText")
                        .attr("xml:space", "preserve")
                        .with_text(r" TOC \h "),
                ),
            );
            p.append(
                Element::new("w:r")
                    .with(Element::new("w:fldChar").attr("w:fldCharType", "separate")),
            );
        }
        let mut hl = Element::new("w:hyperlink").attr("w:anchor", &self.bmark);
        let rpr = Element::new("w:rPr")
            .with(
                Element::new("w:color")
                    .attr("w:val", "0000FF")
                    .attr("w:themeColor", "hyperlink"),
            )
            .with(Element::new("w:u").attr("w:val", "single"));
        hl.append(
            Element::new("w:r")
                .with(rpr)
                .with(Element::new("w:t").with_text(self.title.clone())),
        );
        p.append(hl);
        if self.is_last {
            p.append(
                Element::new("w:r").with(Element::new("w:fldChar").attr("w:fldCharType", "end")),
            );
        }
        p
    }
}

enum HyperlinkTarget<'a> {
    Anchor(&'a str),
    RelId(&'a str),
}

fn make_hyperlink<'p>(
    parent: &'p mut Element,
    target: HyperlinkTarget<'_>,
    tooltip: Option<&str>,
) -> &'p mut Element {
    let mut el = match target {
        HyperlinkTarget::Anchor(a) => Element::new("w:hyperlink").attr("w:anchor", a),
        HyperlinkTarget::RelId(id) => Element::new("w:hyperlink").attr("r:id", id),
    };
    if let Some(t) = tooltip {
        if !t.is_empty() {
            el = el.attr("w:tooltip", t);
        }
    }
    parent.append(el)
}

/// Port of `LinksManager`'s bookmark/hyperlink half -- see the module
/// docs for what's not ported (the TOC-serialization half).
#[derive(Debug)]
pub struct LinksManager {
    document_relationships: DocumentRelationships,
    top_anchor: String,
    anchor_map: HashMap<(String, String), String>,
    used_bookmark_names: HashSet<String>,
    bmark_id: u64,
    document_hrefs: HashSet<String>,
    external_links: HashMap<String, String>,
    toc: Vec<TocItem>,
}

impl LinksManager {
    /// Port of `LinksManager.__init__`. `namespace`/`log` aren't
    /// stored -- `namespace` was only ever used for
    /// `Namespace.makeelement`, which [`Element`] doesn't need, and
    /// nothing here logs yet (matching [`super::styles::StylesManager`]).
    pub fn new(document_relationships: DocumentRelationships) -> Self {
        LinksManager {
            document_relationships,
            top_anchor: uuid::Uuid::new_v4().simple().to_string(),
            anchor_map: HashMap::new(),
            used_bookmark_names: HashSet::new(),
            bmark_id: 0,
            document_hrefs: HashSet::new(),
            external_links: HashMap::new(),
            toc: Vec::new(),
        }
    }

    pub fn top_anchor(&self) -> &str {
        &self.top_anchor
    }

    /// Whether [`Self::process_toc_links`] found a real TOC to
    /// serialize -- port of `Convert.write`'s own `if
    /// self.links_manager.toc:` guard, needed because
    /// [`Self::serialize_toc`] unconditionally adds a "Table of
    /// Contents" heading even with zero entries, so a caller must
    /// check first rather than always calling it.
    pub fn has_toc(&self) -> bool {
        !self.toc.is_empty()
    }

    pub fn document_relationships(&self) -> &DocumentRelationships {
        &self.document_relationships
    }

    /// Port of the `bookmark_id` property: a mutating counter, not a
    /// plain getter (each call hands out the next id).
    pub fn bookmark_id(&mut self) -> u64 {
        self.bmark_id += 1;
        self.bmark_id
    }

    /// Port of `LinksManager.bookmark_for_anchor`.
    pub fn bookmark_for_anchor(
        &mut self,
        anchor: &str,
        current_item_href: &str,
        dom: &Dom,
        html_tag: NodeId,
    ) -> String {
        let key = (current_item_href.to_string(), anchor.to_string());
        if let Some(existing) = self.anchor_map.get(&key) {
            return existing.clone();
        }
        let name = if anchor == self.top_anchor {
            self.document_hrefs.insert(current_item_href.to_string());
            format!("Top of {}", basename(current_item_href))
        } else {
            let text = start_text(dom, html_tag).trim().to_string();
            if text.is_empty() {
                anchor.to_string()
            } else {
                text
            }
        };
        let base = sanitize_bookmark_name(&name);
        let mut name = base.clone();
        let mut i = 0u32;
        while self.used_bookmark_names.contains(&name) {
            i += 1;
            name = format!("{base}_{i}");
        }
        self.anchor_map.insert(key, name.clone());
        self.used_bookmark_names.insert(name.clone());
        name
    }

    /// Port of `LinksManager.serialize_hyperlink`. Appends a
    /// `<w:hyperlink>` into `parent` and returns it when `url`
    /// resolves to a known internal anchor or an external
    /// `http`/`https`/`ftp` link; otherwise returns `parent`
    /// unchanged (Python's `return parent`), meaning the caller
    /// should keep using `parent` directly -- matching
    /// `TextRun.serialize`'s `parent = ... links_manager.serialize_hyperlink(p, self.link)`,
    /// not yet ported (needs `TextRun` itself, `from_html.py`).
    ///
    /// An internal href that isn't in `document_hrefs` logs a warning
    /// in Python (`self.log.warn(...)`) and falls through -- not
    /// reproduced, matching this crate's general absence of a log
    /// sink for these writer managers so far.
    pub fn serialize_hyperlink<'p>(
        &mut self,
        parent: &'p mut Element,
        names: &DocxNamespace,
        current_item_href: &str,
        url: &str,
        tooltip: Option<&str>,
    ) -> &'p mut Element {
        let (scheme, path, fragment) = parse_link_url(url);
        if scheme.is_none() {
            let mut href = abshref(current_item_href, &path);
            if !self.document_hrefs.contains(&href) {
                href = urlquote(&href);
            }
            if self.document_hrefs.contains(&href) {
                let frag_key = if fragment.is_empty() {
                    self.top_anchor.clone()
                } else {
                    fragment
                };
                let key = (href.clone(), frag_key);
                let bmark = self.anchor_map.get(&key).cloned().unwrap_or_else(|| {
                    self.anchor_map
                        .get(&(href.clone(), self.top_anchor.clone()))
                        .cloned()
                        .expect(
                            "a document href only ever enters document_hrefs after \
                             bookmark_for_anchor(top_anchor, ...) already recorded its \
                             (href, top_anchor) anchor_map entry",
                        )
                });
                return make_hyperlink(parent, HyperlinkTarget::Anchor(&bmark), tooltip);
            }
        }
        if matches!(
            scheme.as_deref(),
            Some("http") | Some("https") | Some("ftp")
        ) {
            let rel_id = if let Some(id) = self.external_links.get(url) {
                id.clone()
            } else {
                let rtype = names.name("LINKS").unwrap_or_default().to_string();
                let id = self
                    .document_relationships
                    .add(url, &rtype, Some("External"));
                self.external_links.insert(url.to_string(), id.clone());
                id
            };
            return make_hyperlink(parent, HyperlinkTarget::RelId(&rel_id), tooltip);
        }
        parent
    }

    /// Port of `LinksManager.process_toc_node`. `toc`'s href is
    /// looked up verbatim (no `abshref`) -- matching Python, since OEB
    /// TOC entries already carry canonical, item-absolute hrefs, unlike
    /// the raw in-document hrefs [`Self::serialize_hyperlink`] has to
    /// resolve.
    pub fn process_toc_node(&mut self, toc: &TOCNode, level: u32) {
        if let Some(href) = &toc.href {
            let (_, path, fragment) = parse_link_url(href);
            if self.document_hrefs.contains(&path) {
                let frag_key = if fragment.is_empty() {
                    self.top_anchor.clone()
                } else {
                    fragment
                };
                let key = (path.clone(), frag_key);
                let bmark = self.anchor_map.get(&key).cloned().unwrap_or_else(|| {
                    self.anchor_map
                        .get(&(path.clone(), self.top_anchor.clone()))
                        .cloned()
                        .expect(
                            "a document href only ever enters document_hrefs after \
                             bookmark_for_anchor(top_anchor, ...) already recorded its \
                             (href, top_anchor) anchor_map entry",
                        )
                });
                self.toc.push(TocItem::new(
                    toc.title.clone().unwrap_or_default(),
                    bmark,
                    level,
                ));
            }
        }
        for child in &toc.children {
            self.process_toc_node(child, level + 1);
        }
    }

    /// Port of `LinksManager.process_toc_links`. Python's `oeb.toc and
    /// oeb.toc.count() > 1` guard collapses to just the count check --
    /// [`TOC`] always exists here, there's no `None` case to short
    /// on.
    pub fn process_toc_links(&mut self, toc: &TOC) {
        self.toc.clear();
        if toc.count() <= 1 {
            return;
        }
        for child in &toc.root.children {
            self.process_toc_node(child, 0);
        }
        if let Some(first) = self.toc.first_mut() {
            first.is_first = true;
        }
        if let Some(last) = self.toc.last_mut() {
            last.is_last = true;
        }
    }

    /// Port of `LinksManager.serialize_toc`. Python finds the
    /// `pageBreakBefore` element to flip on via
    /// `body[0].xpath('//*[local-name()="pageBreakBefore"]')[0]` --
    /// an *absolute* xpath (`//` searches from the document root, not
    /// from `body[0]`), but since nothing outside `body`'s own subtree
    /// carries that tag in this writer, searching `body`'s descendants
    /// finds the same element. `__('Table of Contents')` is ported as
    /// the literal English string, matching this crate's established
    /// stand-in for calibre's gettext calls (see
    /// `crate::pdf::render::links`).
    pub fn serialize_toc(&mut self, body: &mut Element, primary_heading_style: Option<&str>) {
        if let Some(pbb) = body.find_descendant_mut("w:pageBreakBefore") {
            pbb.set("w:val", "on");
        }
        for item in self.toc.iter().rev() {
            body.insert(0, item.serialize());
        }
        let mut ppr = Element::new("w:pPr");
        if let Some(style_id) = primary_heading_style {
            ppr.append(Element::new("w:pStyle").attr("w:val", style_id));
        }
        ppr.append(Element::new("w:pageBreakBefore").attr("w:val", "off"));
        let mut p = Element::new("w:p");
        p.append(ppr);
        p.append(Element::new("w:r").with(Element::new("w:t").with_text("Table of Contents")));
        body.insert(0, p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::writer::xml::Child;
    use crate::dom::Dom;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn find(dom: &Dom, tag: &str) -> NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    #[test]
    fn start_text_returns_flattened_text_content() {
        let dom = make("<html><body><p>hello <b>world</b></p></body></html>");
        let p = find(&dom, "p");
        assert_eq!(start_text(&dom, p), "hello world");
    }

    #[test]
    fn start_text_truncates_past_fifty_chars_with_an_ellipsis() {
        let long = "x".repeat(60);
        let html = format!("<html><body><p>{long}</p></body></html>");
        let dom = make(&html);
        let p = find(&dom, "p");
        let result = start_text(&dom, p);
        assert_eq!(result.chars().count(), 53);
        assert!(result.ends_with("..."));
        assert_eq!(&result[..50], &long[..50]);
    }

    #[test]
    fn start_text_exactly_at_the_limit_has_no_ellipsis() {
        let exact = "y".repeat(50);
        let html = format!("<html><body><p>{exact}</p></body></html>");
        let dom = make(&html);
        let p = find(&dom, "p");
        assert_eq!(start_text(&dom, p), exact);
    }

    #[test]
    fn sanitize_bookmark_name_replaces_non_alphanumerics() {
        assert_eq!(
            sanitize_bookmark_name("Chapter One: The Beginning!"),
            "Chapter_One__The_Beginning"
        );
    }

    #[test]
    fn sanitize_bookmark_name_strips_trailing_underscores() {
        assert_eq!(sanitize_bookmark_name("hello!!!"), "hello");
    }

    #[test]
    fn sanitize_bookmark_name_caps_at_32_chars() {
        let long = "a".repeat(50);
        assert_eq!(sanitize_bookmark_name(&long).len(), 32);
    }

    #[test]
    fn sanitize_bookmark_name_transliterates_non_ascii() {
        // ascii_text turns accented Latin letters into their plain
        // ASCII equivalents rather than replacing them with `_`.
        assert_eq!(sanitize_bookmark_name("caf\u{e9}"), "cafe");
    }

    #[test]
    fn parse_link_url_detects_http_scheme_and_strips_query_and_fragment() {
        let (scheme, path, fragment) = parse_link_url("https://example.com/a/b.html?x=1#frag");
        assert_eq!(scheme.as_deref(), Some("https"));
        assert_eq!(path, "/a/b.html");
        assert_eq!(fragment, "frag");
    }

    #[test]
    fn parse_link_url_relative_href_has_no_scheme() {
        let (scheme, path, fragment) = parse_link_url("chapter2.html#top");
        assert_eq!(scheme, None);
        assert_eq!(path, "chapter2.html");
        assert_eq!(fragment, "top");
    }

    #[test]
    fn parse_link_url_fragment_only_has_empty_path() {
        let (scheme, path, fragment) = parse_link_url("#top");
        assert_eq!(scheme, None);
        assert_eq!(path, "");
        assert_eq!(fragment, "top");
    }

    #[test]
    fn toc_item_serialize_wraps_title_in_a_hyperlink() {
        let item = TocItem::new("Chapter One", "bmark1", 1);
        let p = item.serialize();
        assert_eq!(p.name, "w:p");
        let ppr = p.children_named("w:pPr").next().unwrap();
        let ind = ppr.children_named("w:ind").next().unwrap();
        assert_eq!(ind.get("w:leftChars"), Some("200"));
        assert!(ppr.children_named("w:pageBreakBefore").next().is_none());
        let hl = p.children_named("w:hyperlink").next().unwrap();
        assert_eq!(hl.get("w:anchor"), Some("bmark1"));
        let r = hl.children_named("w:r").next().unwrap();
        let t = r.children_named("w:t").next().unwrap();
        assert_eq!(
            t.children.first(),
            Some(&Child::Text("Chapter One".to_string()))
        );
    }

    #[test]
    fn toc_item_first_emits_the_field_begin_run_and_page_break_off() {
        let mut item = TocItem::new("Ch1", "b1", 0);
        item.is_first = true;
        let p = item.serialize();
        let ppr = p.children_named("w:pPr").next().unwrap();
        assert!(ppr.children_named("w:pageBreakBefore").next().is_some());
        let runs: Vec<_> = p.children_named("w:r").collect();
        assert_eq!(
            runs.len(),
            3,
            "field-begin run, instrText run, and field-separate run, before the hyperlink's own run"
        );
        assert!(runs[0]
            .children_named("w:fldChar")
            .next()
            .map(|e| e.get("w:fldCharType") == Some("begin"))
            .unwrap_or(false));
        assert_eq!(
            runs[1]
                .children_named("w:instrText")
                .next()
                .and_then(|e| e.get("xml:space")),
            Some("preserve")
        );
        assert!(runs[2]
            .children_named("w:fldChar")
            .next()
            .map(|e| e.get("w:fldCharType") == Some("separate"))
            .unwrap_or(false));
    }

    #[test]
    fn toc_item_last_emits_the_field_end_run() {
        let mut item = TocItem::new("Ch1", "b1", 0);
        item.is_last = true;
        let p = item.serialize();
        let runs: Vec<_> = p.children_named("w:r").collect();
        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0]
                .children_named("w:fldChar")
                .next()
                .unwrap()
                .get("w:fldCharType"),
            Some("end")
        );
    }

    fn ns() -> DocxNamespace {
        DocxNamespace::new(true)
    }

    fn relationships() -> DocumentRelationships {
        DocumentRelationships::new(&ns())
    }

    #[test]
    fn bookmark_for_anchor_uses_the_element_text_when_no_top_anchor() {
        let dom = make("<html><body><h1>My Heading</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let name = mgr.bookmark_for_anchor("some-id", "chap1.html", &dom, h1);
        assert_eq!(name, "My_Heading");
    }

    #[test]
    fn bookmark_for_anchor_falls_back_to_the_anchor_when_text_is_empty() {
        let dom = make("<html><body><a id=\"x\"></a></body></html>");
        let a = find(&dom, "a");
        let mut mgr = LinksManager::new(relationships());
        let name = mgr.bookmark_for_anchor("x", "chap1.html", &dom, a);
        assert_eq!(name, "x");
    }

    #[test]
    fn bookmark_for_anchor_uniquifies_repeated_names() {
        let dom = make("<html><body><h1>Same</h1><h2>Same</h2></body></html>");
        let h1 = find(&dom, "h1");
        let h2 = find(&dom, "h2");
        let mut mgr = LinksManager::new(relationships());
        let a = mgr.bookmark_for_anchor("id1", "chap1.html", &dom, h1);
        let b = mgr.bookmark_for_anchor("id2", "chap1.html", &dom, h2);
        assert_eq!(a, "Same");
        assert_eq!(b, "Same_1");
    }

    #[test]
    fn bookmark_for_anchor_reuses_the_same_name_for_the_same_key() {
        let dom = make("<html><body><h1>Same</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let a = mgr.bookmark_for_anchor("id1", "chap1.html", &dom, h1);
        let b = mgr.bookmark_for_anchor("id1", "chap1.html", &dom, h1);
        assert_eq!(a, b);
    }

    #[test]
    fn bookmark_for_anchor_top_anchor_names_from_the_item_basename_and_marks_document_href() {
        let dom = make("<html><body><h1>x</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        let name = mgr.bookmark_for_anchor(&top, "text/chap1.html", &dom, h1);
        assert_eq!(name, "Top_of_chap1_html");
        assert!(mgr.document_hrefs.contains("text/chap1.html"));
    }

    #[test]
    fn bookmark_id_increments_from_one() {
        let mut mgr = LinksManager::new(relationships());
        assert_eq!(mgr.bookmark_id(), 1);
        assert_eq!(mgr.bookmark_id(), 2);
        assert_eq!(mgr.bookmark_id(), 3);
    }

    #[test]
    fn serialize_hyperlink_internal_anchor_wraps_in_a_hyperlink_element() {
        let dom = make("<html><body><h1>Target</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        let names = ns();
        let mut p = Element::new("w:p");
        let hl = mgr.serialize_hyperlink(&mut p, &names, "chap1.html", "chap1.html", None);
        assert_eq!(hl.name, "w:hyperlink");
        assert_eq!(hl.get("w:anchor"), Some("Top_of_chap1_html"));
    }

    #[test]
    fn serialize_hyperlink_with_a_fragment_resolves_to_that_anchor() {
        let dom = make("<html><body><h1 id=\"sec\">Target</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        mgr.bookmark_for_anchor("sec", "chap1.html", &dom, h1);
        let names = ns();
        let mut p = Element::new("w:p");
        let hl = mgr.serialize_hyperlink(&mut p, &names, "chap1.html", "chap1.html#sec", None);
        assert_eq!(hl.get("w:anchor"), Some("Target"));
    }

    #[test]
    fn serialize_hyperlink_unknown_internal_href_leaves_parent_unchanged() {
        let mut mgr = LinksManager::new(relationships());
        let names = ns();
        let mut p = Element::new("w:p");
        let out = mgr.serialize_hyperlink(&mut p, &names, "chap1.html", "unknown.html", None);
        assert_eq!(out.name, "w:p");
        assert_eq!(out.child_count(), 0);
    }

    #[test]
    fn serialize_hyperlink_external_http_link_adds_a_relationship() {
        let mut mgr = LinksManager::new(relationships());
        let names = ns();
        let mut p = Element::new("w:p");
        let hl = mgr.serialize_hyperlink(
            &mut p,
            &names,
            "chap1.html",
            "https://example.com/",
            Some("a tip"),
        );
        assert_eq!(hl.name, "w:hyperlink");
        assert!(hl.get("r:id").is_some());
        assert_eq!(hl.get("w:tooltip"), Some("a tip"));
    }

    #[test]
    fn serialize_hyperlink_reuses_the_same_relationship_for_a_repeated_url() {
        let mut mgr = LinksManager::new(relationships());
        let names = ns();
        let mut p1 = Element::new("w:p");
        let id1 = mgr
            .serialize_hyperlink(&mut p1, &names, "chap1.html", "https://example.com/", None)
            .get("r:id")
            .unwrap()
            .to_string();
        let mut p2 = Element::new("w:p");
        let id2 = mgr
            .serialize_hyperlink(&mut p2, &names, "chap1.html", "https://example.com/", None)
            .get("r:id")
            .unwrap()
            .to_string();
        assert_eq!(id1, id2);
    }

    #[test]
    fn serialize_hyperlink_unsupported_scheme_leaves_parent_unchanged() {
        let mut mgr = LinksManager::new(relationships());
        let names = ns();
        let mut p = Element::new("w:p");
        let out = mgr.serialize_hyperlink(&mut p, &names, "chap1.html", "mailto:a@b.com", None);
        assert_eq!(out.child_count(), 0);
    }

    #[test]
    fn process_toc_node_registers_a_toc_item_for_a_known_document_href() {
        let dom = make("<html><body><h1>x</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        let node = TOCNode::new(
            Some("Chapter 1".to_string()),
            Some("chap1.html".to_string()),
        );
        mgr.process_toc_node(&node, 0);
        assert_eq!(mgr.toc.len(), 1);
        assert_eq!(mgr.toc[0].title, "Chapter 1");
        assert_eq!(mgr.toc[0].level, 0);
        assert_eq!(mgr.toc[0].bmark, "Top_of_chap1_html");
    }

    #[test]
    fn process_toc_node_skips_hrefs_outside_the_document() {
        let mut mgr = LinksManager::new(relationships());
        let node = TOCNode::new(Some("Nope".to_string()), Some("unknown.html".to_string()));
        mgr.process_toc_node(&node, 0);
        assert!(mgr.toc.is_empty());
    }

    #[test]
    fn process_toc_node_recurses_into_children_with_increasing_level() {
        let dom = make("<html><body><h1>x</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        let mut root = TOCNode::new(Some("Root".to_string()), Some("chap1.html".to_string()));
        root.add(TOCNode::new(
            Some("Child".to_string()),
            Some("chap1.html".to_string()),
        ));
        mgr.process_toc_node(&root, 0);
        assert_eq!(mgr.toc.len(), 2);
        assert_eq!(mgr.toc[0].level, 0);
        assert_eq!(mgr.toc[1].level, 1);
    }

    #[test]
    fn process_toc_links_returns_early_when_the_toc_has_one_or_fewer_entries() {
        let mut mgr = LinksManager::new(relationships());
        let mut toc = TOC::new();
        toc.root.add(TOCNode::new(
            Some("Only".to_string()),
            Some("chap1.html".to_string()),
        ));
        mgr.process_toc_links(&toc);
        assert!(mgr.toc.is_empty());
        assert!(!mgr.has_toc());
    }

    #[test]
    fn has_toc_is_true_once_process_toc_links_finds_real_entries() {
        let dom = make("<html><body><h1>x</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        mgr.bookmark_for_anchor(&top, "chap2.html", &dom, h1);
        let mut toc = TOC::new();
        toc.root.add(TOCNode::new(
            Some("One".to_string()),
            Some("chap1.html".to_string()),
        ));
        toc.root.add(TOCNode::new(
            Some("Two".to_string()),
            Some("chap2.html".to_string()),
        ));
        assert!(!mgr.has_toc(), "nothing processed yet");
        mgr.process_toc_links(&toc);
        assert!(mgr.has_toc());
    }

    #[test]
    fn process_toc_links_marks_first_and_last_entries() {
        let dom = make("<html><body><h1>x</h1></body></html>");
        let h1 = find(&dom, "h1");
        let mut mgr = LinksManager::new(relationships());
        let top = mgr.top_anchor().to_string();
        mgr.bookmark_for_anchor(&top, "chap1.html", &dom, h1);
        mgr.bookmark_for_anchor(&top, "chap2.html", &dom, h1);
        mgr.bookmark_for_anchor(&top, "chap3.html", &dom, h1);
        let mut toc = TOC::new();
        toc.root.add(TOCNode::new(
            Some("One".to_string()),
            Some("chap1.html".to_string()),
        ));
        toc.root.add(TOCNode::new(
            Some("Two".to_string()),
            Some("chap2.html".to_string()),
        ));
        toc.root.add(TOCNode::new(
            Some("Three".to_string()),
            Some("chap3.html".to_string()),
        ));
        mgr.process_toc_links(&toc);
        assert_eq!(mgr.toc.len(), 3);
        assert!(mgr.toc[0].is_first);
        assert!(!mgr.toc[1].is_first && !mgr.toc[1].is_last);
        assert!(mgr.toc[2].is_last);
    }

    #[test]
    fn serialize_toc_prepends_title_then_items_and_flips_the_first_pagebreak() {
        let mut mgr = LinksManager::new(relationships());
        mgr.toc = vec![TocItem::new("One", "b1", 0), TocItem::new("Two", "b2", 0)];
        mgr.toc[0].is_first = true;
        mgr.toc[1].is_last = true;
        let mut body = Element::new("w:body").with(Element::new("w:p").with(
            Element::new("w:pPr").with(Element::new("w:pageBreakBefore").attr("w:val", "off")),
        ));
        mgr.serialize_toc(&mut body, Some("Heading1"));

        let ps: Vec<&Element> = body.children_named("w:p").collect();
        assert_eq!(ps.len(), 4, "title + 2 toc items + the original paragraph");

        let title_ppr = ps[0].children_named("w:pPr").next().unwrap();
        assert_eq!(
            title_ppr
                .children_named("w:pStyle")
                .next()
                .unwrap()
                .get("w:val"),
            Some("Heading1")
        );
        let title_t = ps[0]
            .children_named("w:r")
            .next()
            .unwrap()
            .children_named("w:t")
            .next()
            .unwrap();
        assert_eq!(
            title_t.children.first(),
            Some(&Child::Text("Table of Contents".to_string()))
        );

        let one_hyperlink = ps[1].children_named("w:hyperlink").next().unwrap();
        assert_eq!(one_hyperlink.get("w:anchor"), Some("b1"));
        let two_hyperlink = ps[2].children_named("w:hyperlink").next().unwrap();
        assert_eq!(two_hyperlink.get("w:anchor"), Some("b2"));

        let orig_pbb = ps[3]
            .children_named("w:pPr")
            .next()
            .unwrap()
            .children_named("w:pageBreakBefore")
            .next()
            .unwrap();
        assert_eq!(orig_pbb.get("w:val"), Some("on"));
    }

    #[test]
    fn serialize_toc_with_no_toc_items_still_prepends_the_title() {
        let mut mgr = LinksManager::new(relationships());
        let mut body = Element::new("w:body").with(Element::new("w:p").with(
            Element::new("w:pPr").with(Element::new("w:pageBreakBefore").attr("w:val", "off")),
        ));
        mgr.serialize_toc(&mut body, None);
        let ps: Vec<&Element> = body.children_named("w:p").collect();
        assert_eq!(ps.len(), 2);
        let title_ppr = ps[0].children_named("w:pPr").next().unwrap();
        assert!(title_ppr.children_named("w:pStyle").next().is_none());
    }
}
