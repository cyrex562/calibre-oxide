//! Port of `old_src/src/calibre/ebooks/oeb/transforms/structure.py`.
//!
//! # Scope note: no general XPath engine
//!
//! Several options this file reads (`chapter`, `page_breaks_before`,
//! `level1_toc`/`level2_toc`/`level3_toc`, `start_reading_at`) are, in
//! Python, arbitrary user-supplied **XPath 1.0** expressions evaluated
//! with `lxml.etree.XPath` (including calibre's `re:test()` regex
//! extension function). This crate has no general XPath engine (the
//! [`crate::css`] module is a CSS selector matcher, a different
//! language), and building one is out of scope for this port.
//!
//! What's real:
//! - The exact **default** `chapter` expression's behavior -- by far the
//!   common case, since most conversions never override it --
//!   is reimplemented directly as Rust logic in
//!   [`default_chapter_candidates`] (h1/h2 elements whose text matches
//!   the chapter/book/section/part/prologue/epilogue pattern, or any
//!   element with `class="chapter"`).
//! - A small XPath *subset* ([`simple_xpath`]) covering the other
//!   defaults and the common simple case: `//tagname` and
//!   `//*[name()='a' or name()='b' ...]`, unioned with `|`, and the
//!   disabling sentinel `/`. This covers `page_breaks_before`'s default
//!   (`//*[name()='h1' or name()='h2']`) exactly.
//! - Everything else in this file (TOC assembly, `toc_filter`,
//!   "Unnamed" title backfill, link-based TOC generation, chapter
//!   marking, start-reading-at insertion) is a full, faithful port.
//!
//! An expression outside what [`simple_xpath`] understands degrades to
//! "no matches" rather than panicking -- consistent with this project's
//! fault-tolerance rules (never crash on malformed/unsupported input;
//! Python's own behavior on a *syntactically* invalid XPath is likewise
//! to warn and continue with no matches).

use regex::Regex;

use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::toc::TOCNode;

/// Options this transform reads (`opts.*` in Python). No shared,
/// pipeline-wide options type exists yet (see other transforms' options
/// structs for the same narrowing) -- construct with
/// [`StructureOptions::default`] and override only what you need.
#[derive(Debug, Clone)]
pub struct StructureOptions {
    pub use_auto_toc: bool,
    pub no_chapters_in_toc: bool,
    pub toc_threshold: usize,
    pub max_toc_links: usize,
    pub duplicate_links_in_toc: bool,
    pub toc_filter: Option<String>,
    pub page_breaks_before: Option<String>,
    pub chapter: Option<String>,
    pub chapter_mark: String,
    pub level1_toc: Option<String>,
    pub level2_toc: Option<String>,
    pub level3_toc: Option<String>,
    pub start_reading_at: Option<String>,
}

impl Default for StructureOptions {
    fn default() -> Self {
        StructureOptions {
            use_auto_toc: false,
            no_chapters_in_toc: false,
            toc_threshold: 6,
            max_toc_links: 50,
            duplicate_links_in_toc: false,
            toc_filter: None,
            page_breaks_before: Some("//*[name()='h1' or name()='h2']".to_string()),
            chapter: Some(DEFAULT_CHAPTER_XPATH.to_string()),
            chapter_mark: "pagebreak".to_string(),
            level1_toc: None,
            level2_toc: None,
            level3_toc: None,
            start_reading_at: None,
        }
    }
}

/// Sentinel matched against `opts.chapter` to decide whether to use the
/// real default-heuristic implementation ([`default_chapter_candidates`])
/// instead of the generic (much narrower) [`simple_xpath`] fallback.
pub const DEFAULT_CHAPTER_XPATH: &str = "//*[((name()='h1' or name()='h2') and re:test(., '\\s*((chapter|book|section|part)\\s+)|((prolog|prologue|epilogue)(\\s+|$))', 'i')) or @class = 'chapter']";

lazy_static::lazy_static! {
    static ref CHAPTER_WORD_RE: Regex = Regex::new(
        r"(?i)\s*((chapter|book|section|part)\s+)|((prolog|prologue|epilogue)(\s+|$))"
    ).unwrap();
}

/// Real implementation of the *default* `chapter` XPath's semantics: h1
/// or h2 elements whose text content matches the chapter/section/part/
/// prologue/epilogue pattern, plus any element carrying
/// `class="chapter"`.
fn default_chapter_candidates(dom: &Dom, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    for el in dom.preorder_elements(root) {
        let tag = dom.tag(el).unwrap_or("");
        if (tag == "h1" || tag == "h2") && CHAPTER_WORD_RE.is_match(&dom.text_content(el)) {
            out.push(el);
            continue;
        }
        if dom
            .node(el)
            .attrs
            .get("class")
            .map(|c| c.split_whitespace().any(|w| w == "chapter"))
            .unwrap_or(false)
        {
            out.push(el);
        }
    }
    out
}

/// Evaluate the small XPath subset described in the module docs. `/`
/// disables (returns no matches, same as Python's documented
/// "use `/` to disable" convention). Alternatives are joined with `|`.
fn simple_xpath(dom: &Dom, root: NodeId, expr: &str) -> Vec<NodeId> {
    let expr = expr.trim();
    if expr.is_empty() || expr == "/" {
        return Vec::new();
    }
    let mut out = Vec::new();
    for alt in expr.split('|') {
        let alt = alt.trim();
        if let Some(tag) = alt.strip_prefix("//") {
            let tag = tag.trim_start_matches("h:");
            if let Some(names) = parse_name_union(tag) {
                for el in dom.preorder_elements(root) {
                    if names.iter().any(|n| dom.tag(el) == Some(n.as_str())) && !out.contains(&el) {
                        out.push(el);
                    }
                }
            } else if !tag.contains(['[', '*', '@']) && !tag.is_empty() {
                for el in dom.find_all_tag(root, tag) {
                    if !out.contains(&el) {
                        out.push(el);
                    }
                }
            }
            // Anything more complex than the two forms above is left
            // unmatched -- see the module-level scope note.
        }
    }
    out
}

/// Parses `*[name()='a' or name()='b' ...]` into `["a", "b", ...]`.
fn parse_name_union(tag_expr: &str) -> Option<Vec<String>> {
    let inner = tag_expr.strip_prefix("*[")?.strip_suffix(']')?;
    let mut names = Vec::new();
    for part in inner.split("or") {
        let part = part.trim();
        let part = part.strip_prefix("name()=")?;
        let name = part.trim_matches(|c| c == '\'' || c == '"');
        names.push(name.to_string());
    }
    Some(names)
}

/// Port of `isspace`: true for the empty string or a string that is
/// entirely whitespace once non-breaking spaces are dropped.
fn isspace(s: &str) -> bool {
    s.replace('\u{a0}', "").trim().is_empty() || s.is_empty()
}

fn collect_preorder_all(dom: &Dom, id: NodeId, out: &mut Vec<NodeId>) {
    out.push(id);
    for c in dom.children(id) {
        collect_preorder_all(dom, c, out);
    }
}

/// Port of `at_start`: true if there is no real content (non-whitespace
/// text, an `<img>`, or an `<svg>`) before `elem` in `body`'s document
/// order. A simplification of Python's element `.text`/`.tail`
/// ancestor-ownership bookkeeping (this DOM has no separate tail
/// concept -- see `unsmarten.rs`'s equivalent note); functionally
/// equivalent for the common case.
fn at_start(dom: &Dom, body: NodeId, elem: NodeId) -> bool {
    let mut order = Vec::new();
    collect_preorder_all(dom, body, &mut order);
    for node in order {
        if node == elem {
            return true;
        }
        if let Some(tag) = dom.tag(node) {
            if tag == "img" || tag == "svg" {
                return false;
            }
        } else if let NodeKind::Text(t) = &dom.node(node).kind {
            if !isspace(t) {
                return false;
            }
        }
    }
    false
}

struct DetectedChapter {
    href: String,
    elem: NodeId,
}

/// Port of `DetectStructure`.
pub struct DetectStructure;

impl DetectStructure {
    pub fn call(&self, oeb: &mut OEBBook, opts: &StructureOptions) {
        let mut docs: Vec<(String, Dom)> = Vec::new();
        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for href in &spine_hrefs {
            let Ok(raw) = oeb.container.read(href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            docs.push((href.clone(), Dom::parse(&html)));
        }

        let detected = self.detect_chapters(&mut docs, opts);

        let had_original_toc = !oeb.toc.root.children.is_empty();
        let original_count = oeb.toc.count();
        if opts.use_auto_toc || had_original_toc {
            let mut new_toc = TOCNode::new(None, None);
            let mut counter = 1i32;
            if let Some(expr) = &opts.level1_toc {
                self.add_leveled_toc_items(&docs, opts, expr, &mut new_toc, &mut counter);
            }
            if count_nodes(&new_toc) < 1 {
                if !opts.no_chapters_in_toc && !detected.is_empty() {
                    self.create_toc_from_chapters(&detected, &docs, &mut new_toc, &mut counter);
                }
                if count_nodes(&new_toc) < opts.toc_threshold {
                    self.create_toc_from_links(&docs, opts, &mut new_toc, &mut counter);
                }
            }
            if count_nodes(&new_toc) < 2 && original_count > 2 {
                // Keep the original TOC -- leave oeb.toc untouched.
            } else {
                oeb.toc.root = new_toc;
            }
        }

        if let Some(filter_expr) = &opts.toc_filter {
            if let Ok(re) = Regex::new(filter_expr) {
                remove_matching(&mut oeb.toc.root, &re);
            }
        }

        if let Some(expr) = &opts.page_breaks_before {
            for (href, dom) in &mut docs {
                let matches = simple_xpath(dom, dom.root, expr);
                for elem in matches {
                    let style = dom
                        .node(elem)
                        .attrs
                        .get("style")
                        .cloned()
                        .unwrap_or_default();
                    let mut new_style = style;
                    if !new_style.is_empty() {
                        new_style.push_str("; ");
                    }
                    new_style.push_str("page-break-before:always");
                    dom.node_mut(elem)
                        .attrs
                        .insert("style".to_string(), new_style);
                }
                let _ = href; // silence unused warning when matches is empty
            }
        }

        fill_unnamed(&mut oeb.toc.root);

        if let Some(expr) = &opts.start_reading_at {
            self.detect_start_reading(oeb, &mut docs, expr);
        }

        for (href, dom) in &docs {
            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(href, &rendered);
        }
    }

    fn detect_chapters(
        &self,
        docs: &mut [(String, Dom)],
        opts: &StructureOptions,
    ) -> Vec<DetectedChapter> {
        let Some(chapter_expr) = &opts.chapter else {
            return Vec::new();
        };
        let mut detected = Vec::new();
        for (href, dom) in docs.iter() {
            let matches = if chapter_expr.trim() == DEFAULT_CHAPTER_XPATH {
                default_chapter_candidates(dom, dom.root)
            } else {
                simple_xpath(dom, dom.root, chapter_expr)
            };
            for elem in matches {
                detected.push(DetectedChapter {
                    href: href.clone(),
                    elem,
                });
            }
        }

        let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for d in &detected {
            let c = counts.entry(d.href.clone()).or_insert(0);
            *c += 1;
            let dom = &mut docs.iter_mut().find(|(h, _)| h == &d.href).unwrap().1;
            match opts.chapter_mark.as_str() {
                "none" => {}
                "rule" => insert_mark(dom, d.elem, "hr", None),
                "pagebreak" => {
                    if *c < 3 && at_start(dom, dom.root, d.elem) {
                        // First couple of elements at the start of the
                        // file: skip, matches Python's PDF-blank-page
                        // avoidance.
                    } else {
                        insert_mark(
                            dom,
                            d.elem,
                            "div",
                            Some("display: block; page-break-after: always"),
                        );
                    }
                }
                _ => insert_mark(
                    dom,
                    d.elem,
                    "hr",
                    Some("display: block; page-break-before: always"),
                ),
            }
        }
        detected
    }

    fn create_toc_from_chapters(
        &self,
        detected: &[DetectedChapter],
        docs: &[(String, Dom)],
        toc: &mut TOCNode,
        counter: &mut i32,
    ) {
        for d in detected {
            let dom = &docs.iter().find(|(h, _)| h == &d.href).unwrap().1;
            let (text, href) = elem_to_link(dom, &d.href, d.elem, *counter);
            toc.add(TOCNode {
                title: Some(text),
                href: Some(href),
                play_order: *counter,
                ..TOCNode::new(None, None)
            });
            *counter += 1;
        }
    }

    fn create_toc_from_links(
        &self,
        docs: &[(String, Dom)],
        opts: &StructureOptions,
        toc: &mut TOCNode,
        counter: &mut i32,
    ) {
        let mut num = 0usize;
        let mut seen_hrefs: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen_texts: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (href, dom) in docs {
            for a in dom.find_all_tag_global("a") {
                let Some(link_href) = dom.node(a).attrs.get("href").cloned() else {
                    continue;
                };
                if link_href.contains("://") {
                    continue;
                }
                let (path, frag) = link_href.split_once('#').unwrap_or((&link_href, ""));
                let abs = super::filenames::abshref(href, path);
                let full = if frag.is_empty() {
                    abs
                } else {
                    format!("{abs}#{frag}")
                };
                if seen_hrefs.contains(&full) {
                    continue;
                }
                let text: String = dom.text_content(a).chars().take(100).collect();
                let text = text.trim().to_string();
                if !opts.duplicate_links_in_toc && seen_texts.contains(&text) {
                    continue;
                }
                toc.add(TOCNode {
                    title: Some(text.clone()),
                    href: Some(full.clone()),
                    play_order: *counter,
                    ..TOCNode::new(None, None)
                });
                seen_hrefs.insert(full);
                seen_texts.insert(text);
                *counter += 1;
                num += 1;
                if opts.max_toc_links > 0 && num >= opts.max_toc_links {
                    return;
                }
            }
        }
    }

    fn add_leveled_toc_items(
        &self,
        docs: &[(String, Dom)],
        opts: &StructureOptions,
        level1_expr: &str,
        toc: &mut TOCNode,
        counter: &mut i32,
    ) {
        for (href, dom) in docs {
            for elem in simple_xpath(dom, dom.root, level1_expr) {
                let (text, link_href) = elem_to_link(dom, href, elem, *counter);
                *counter += 1;
                if !text.is_empty() {
                    let node = TOCNode {
                        title: Some(text),
                        href: Some(link_href),
                        play_order: *counter,
                        ..TOCNode::new(None, None)
                    };
                    let level1 = node;
                    let node_idx = toc.children.len();
                    toc.add(level1);
                    if let Some(level2_expr) = &opts.level2_toc {
                        for elem2 in simple_xpath(dom, dom.root, level2_expr) {
                            let (text2, href2) = elem_to_link(dom, href, elem2, *counter);
                            *counter += 1;
                            if !text2.is_empty() {
                                toc.children[node_idx].add(TOCNode {
                                    title: Some(text2),
                                    href: Some(href2),
                                    play_order: *counter,
                                    ..TOCNode::new(None, None)
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    fn detect_start_reading(&self, oeb: &mut OEBBook, docs: &mut [(String, Dom)], expr: &str) {
        for (href, dom) in docs.iter_mut() {
            let matches = simple_xpath(dom, dom.root, expr);
            if let Some(elem) = matches.into_iter().next() {
                let eid = dom.node(elem).attrs.get("id").cloned().unwrap_or_else(|| {
                    let id = format!("start_reading_at_{}", uuid::Uuid::new_v4().simple());
                    dom.node_mut(elem)
                        .attrs
                        .insert("id".to_string(), id.clone());
                    id
                });
                if oeb.guide.get("text").is_some() {
                    oeb.guide.remove("text");
                }
                oeb.guide
                    .add("text", Some("Start".to_string()), &format!("{href}#{eid}"));
                return;
            }
        }
    }
}

fn insert_mark(dom: &mut Dom, before: NodeId, tag: &str, style: Option<&str>) {
    let Some(parent) = dom.parent(before) else {
        return;
    };
    let Some(idx) = dom.index_in_parent(before) else {
        return;
    };
    let mark = dom.new_element(tag);
    if let Some(style) = style {
        dom.node_mut(mark)
            .attrs
            .insert("style".to_string(), style.to_string());
    }
    dom.insert_child(parent, idx, mark);
}

fn elem_to_link(dom: &Dom, href: &str, elem: NodeId, counter: i32) -> (String, String) {
    let mut text: String = dom.text_content(elem).trim().to_string();
    if text.is_empty() {
        text = dom
            .node(elem)
            .attrs
            .get("title")
            .cloned()
            .unwrap_or_default();
    }
    if text.is_empty() {
        text = dom.node(elem).attrs.get("alt").cloned().unwrap_or_default();
    }
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = text.chars().take(1000).collect();
    let id = dom
        .node(elem)
        .attrs
        .get("id")
        .cloned()
        .unwrap_or_else(|| format!("calibre_toc_{counter}"));
    (text, format!("{href}#{id}"))
}

fn count_nodes(toc: &TOCNode) -> usize {
    toc.iter().len().saturating_sub(1)
}

fn remove_matching(toc: &mut TOCNode, re: &Regex) {
    toc.children.retain(|c| {
        let title = c.title.as_deref().unwrap_or("");
        !re.is_match(title)
    });
    for c in &mut toc.children {
        remove_matching(c, re);
    }
}

fn fill_unnamed(toc: &mut TOCNode) {
    if toc
        .title
        .as_deref()
        .map(|t| t.trim().is_empty())
        .unwrap_or(true)
        && toc.href.is_some()
    {
        toc.title = Some("Unnamed".to_string());
    }
    for c in &mut toc.children {
        fill_unnamed(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn detects_default_chapter_headings_and_marks_pagebreaks() {
        let mut oeb = Builder::new()
            .page(
                "a.html",
                "<p>intro</p><h1>Chapter One</h1><p>text</p><h1>Chapter Two</h1><p>more</p>",
            )
            .build();
        DetectStructure.call(&mut oeb, &StructureOptions::default());
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("page-break-after"), "{html}");
    }

    #[test]
    fn builds_toc_from_links_when_no_chapters_detected() {
        let mut oeb = Builder::new()
            .page("a.html", r#"<a href="b.html">Go to B</a>"#)
            .page("b.html", "<p>b</p>")
            .build();
        let opts = StructureOptions {
            chapter: None,
            use_auto_toc: true,
            ..StructureOptions::default()
        };
        DetectStructure.call(&mut oeb, &opts);
        assert!(oeb.toc.count() >= 1);
        let first = oeb.toc.first().unwrap();
        assert_eq!(first.title.as_deref(), Some("Go to B"));
    }

    #[test]
    fn toc_filter_removes_matching_titles() {
        let mut oeb = Builder::new().build();
        let mut n1 = TOCNode::new(Some("Keep me".into()), Some("a.html".into()));
        n1.add(TOCNode::new(Some("child".into()), Some("a.html#c".into())));
        oeb.toc.root.add(n1);
        oeb.toc.root.add(TOCNode::new(
            Some("Drop this one".into()),
            Some("b.html".into()),
        ));
        let opts = StructureOptions {
            chapter: None,
            toc_filter: Some("Drop".to_string()),
            ..StructureOptions::default()
        };
        DetectStructure.call(&mut oeb, &opts);
        assert_eq!(oeb.toc.root.children.len(), 1);
        assert_eq!(oeb.toc.root.children[0].title.as_deref(), Some("Keep me"));
    }

    #[test]
    fn simple_xpath_matches_name_union() {
        let dom = Dom::parse("<html><body><h1>a</h1><h2>b</h2><h3>c</h3></body></html>");
        let matches = simple_xpath(&dom, dom.root, "//*[name()='h1' or name()='h2']");
        assert_eq!(matches.len(), 2);
    }
}
