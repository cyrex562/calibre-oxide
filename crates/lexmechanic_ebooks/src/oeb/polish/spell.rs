//! Port of `old_src/src/calibre/ebooks/oeb/polish/spell.py`: word/
//! character extraction (with source-location tracking) across the
//! OPF/NCX/HTML content of a book, for calibre's spell-check UI's
//! "list every word" and "replace this word everywhere" features. This
//! is **not** the dictionary-lookup/spell-checking itself
//! (`crate::spell::dictionary`, a separate, much larger subsystem this
//! file only imports [`DictionaryLocale`]/[`parse_lang_code`]/
//! [`split_into_words`]/[`index_of`] from -- their canonical home is
//! `crate::spell`, issue #59's port, not this file) -- purely
//! extraction and location bookkeeping, which is fully portable.
//!
//! # Design note: `TreeNode` instead of a shared tree-node type
//!
//! Python's `Location.location_node` holds an `lxml` element uniformly,
//! whether it came from the OPF, the NCX, or an HTML content document --
//! `lxml.etree` gives every one of those the same `.text`/`.tail`/
//! `.get`/`.set` API. This crate has two structurally different tree
//! types for the same reason `container.rs` does (see its module docs):
//! [`crate::xmltree::Xml`] for strict XML (OPF, NCX) and
//! [`crate::dom::Dom`] for tag-soup HTML5. [`TreeNode`] is the
//! discriminated union that replaces "any lxml element": callers that
//! need to read/write a location's text dispatch on it via
//! [`node_item_text`]/[`set_node_item_text`] rather than relying on a
//! shared trait, since the two tree types' text/tail representations
//! also differ in shape (`Xml`/`Dom` both represent "tail" as an
//! ordinary sibling text node rather than lxml's out-of-band `.tail`
//! attribute -- see each module's own docs -- so both need their own
//! small tail-lookup helpers here, matching the per-file-private-copy
//! convention `pretty.rs`/`toc.rs` already established for the `Dom`
//! half of this).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use anyhow::Result;
use regex::Regex;

pub use crate::spell::break_iterator::{index_of, split_into_words};
pub use crate::spell::{parse_lang_code, DictionaryLocale};

use crate::dom::{Dom, NodeId, NodeKind};

use super::container::Container;
use super::toc::{find_existing_nav_toc, find_existing_ncx_toc};
use crate::xmltree::{Xml, XmlNodeId, XmlNodeKind};

/// Port of `replace`: replaces every whole-word occurrence of
/// `original_word` in `text` with `new_word`. Returns the new text and
/// whether anything was replaced.
pub fn replace(text: &str, original_word: &str, new_word: &str, lang: &str) -> (String, bool) {
    let mut indices = Vec::new();
    let mut offset = 0usize;
    loop {
        let q = &text[offset..];
        match index_of(original_word, q, lang) {
            Some(idx) => {
                indices.push(offset + idx);
                offset += idx + original_word.len();
                if offset > text.len() {
                    break;
                }
            }
            None => break,
        }
    }
    let mut result = text.to_string();
    for &idx in indices.iter().rev() {
        result.replace_range(idx..idx + original_word.len(), new_word);
    }
    (result, !indices.is_empty())
}

// ===================================================================
// Patterns
// ===================================================================

struct Patterns {
    digit_pat: Regex,
    fr_elision_pat: Regex,
    sanitize_invisible_pat: Regex,
}

fn patterns() -> &'static Patterns {
    static P: OnceLock<Patterns> = OnceLock::new();
    P.get_or_init(|| Patterns {
        digit_pat: Regex::new(r"^\d+$").unwrap(),
        fr_elision_pat: Regex::new(
            r"(?i)^(?:l|d|m|t|s|j|c|\u{e7}|lorsqu|puisqu|quoiqu|qu)['\u{2019}]",
        )
        .unwrap(),
        sanitize_invisible_pat: Regex::new(
            "[\u{ad}\u{200b}\u{200c}\u{200d}\u{feff}\0-\u{8}\u{b}\u{c}\u{e}-\u{1f}\u{7f}]",
        )
        .unwrap(),
    })
}

fn filter_words(word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    !patterns().digit_pat.is_match(word)
}

/// Port of `get_words`: returns `(filtered_words, raw_split_count)` --
/// the raw count (before `filter_words`) is what Python accumulates
/// into `file_word_count`.
fn get_words(text: &str, lang: &str) -> (Vec<String>, usize) {
    let raw = split_into_words(text, lang);
    let raw_count = raw.len();
    (
        raw.into_iter().filter(|w| filter_words(w)).collect(),
        raw_count,
    )
}

// ===================================================================
// Location / node reference
// ===================================================================

/// Port of "any lxml element" for `Location.location_node`. See the
/// module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeNode {
    /// An element in the OPF or NCX document (both are [`Xml`] trees).
    Opf(XmlNodeId),
    /// An element in an XHTML content document.
    Html(NodeId),
}

/// Port of `Location.node_item` (Python's `(is_attr, attr)` tuple).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NodeItem {
    Attr(String),
    /// The node's own leading text (lxml's `.text`).
    Text,
    /// The text immediately following the node, before its next sibling
    /// (lxml's `.tail`).
    Tail,
}

/// Port of `Location`.
#[derive(Debug, Clone)]
pub struct Location {
    pub file_name: String,
    pub elided_prefix: String,
    pub original_word: String,
    pub location_node: TreeNode,
    pub node_item: NodeItem,
    pub sourceline: Option<u32>,
}

impl Location {
    pub fn new(
        file_name: impl Into<String>,
        elided_prefix: impl Into<String>,
        original_word: impl Into<String>,
        location_node: TreeNode,
        node_item: NodeItem,
        sourceline: Option<u32>,
    ) -> Self {
        Location {
            file_name: file_name.into(),
            elided_prefix: elided_prefix.into(),
            original_word: original_word.into(),
            location_node,
            node_item,
            sourceline,
        }
    }

    /// Port of `Location.replace`.
    pub fn replace_word(&mut self, new_word: &str) {
        self.original_word = format!("{}{}", self.elided_prefix, new_word);
    }
}

/// `words[(sword, locale)] -> [Location, ...]`, port of the `defaultdict(list)`
/// keyed by `(sword, locale)` in `get_all_words`.
pub type WordsMap = HashMap<(String, DictionaryLocale), Vec<Location>>;

// ===================================================================
// Xml (OPF/NCX) tail/id helpers -- see the module docs on why these
// are small private copies rather than shared with `pretty.rs`/`toc.rs`.
// ===================================================================

fn xml_iterdescendants(xml: &Xml, id: XmlNodeId) -> Vec<XmlNodeId> {
    let mut out = Vec::new();
    fn walk(xml: &Xml, id: XmlNodeId, out: &mut Vec<XmlNodeId>) {
        for &c in xml.children(id) {
            if matches!(xml.node(c).kind, XmlNodeKind::Element { .. }) {
                out.push(c);
                walk(xml, c, out);
            }
        }
    }
    walk(xml, id, &mut out);
    out
}

fn xml_tail(xml: &Xml, id: XmlNodeId) -> Option<&str> {
    let parent = xml.parent(id)?;
    let pos = xml.index_in_parent(id)?;
    let next = *xml.children(parent).get(pos + 1)?;
    match &xml.node(next).kind {
        XmlNodeKind::Text(t) => Some(t.as_str()),
        _ => None,
    }
}

fn xml_set_tail(xml: &mut Xml, id: XmlNodeId, text: &str) {
    let Some(parent) = xml.parent(id) else { return };
    let Some(pos) = xml.index_in_parent(id) else {
        return;
    };
    let next = xml.children(parent).get(pos + 1).copied();
    if let Some(next) = next {
        if let XmlNodeKind::Text(_) = xml.node(next).kind {
            xml.node_mut(next).kind = XmlNodeKind::Text(text.to_string());
            return;
        }
    }
    let t = xml.new_text(text);
    xml.insert_element(parent, t, None);
}

/// Port of `root_is_excluded_from_spell_check`'s `child.text` check.
/// Real books place the marker as an HTML/XML *comment*
/// (`<!-- calibre-no-spell-check -->`, per
/// `gui2/tweak_book/spell.py`), not literal text -- lxml gives comment
/// nodes a `.text` too, which is what Python's `getattr(child, 'text',
/// '')` actually reads for the common case. This checks both: a
/// comment's own text, or a plain element child's own leading text.
fn xml_root_excluded(xml: &Xml, root: XmlNodeId) -> bool {
    xml.children(root).iter().any(|&c| {
        let text = match &xml.node(c).kind {
            XmlNodeKind::Comment(t) => Some(t.as_str()),
            XmlNodeKind::Element { .. } => xml.element_text(c),
            _ => None,
        };
        text.map(|t| t.trim().eq_ignore_ascii_case("calibre-no-spell-check"))
            .unwrap_or(false)
    })
}

// ===================================================================
// Dom (HTML) leading-text/tail helpers -- private copies matching the
// convention already established in `pretty.rs`/`toc.rs`.
// ===================================================================

fn dom_html_root(dom: &Dom) -> Option<NodeId> {
    dom.children(dom.root)
        .into_iter()
        .find(|&c| dom.tag(c).is_some())
}

fn leading_text(dom: &Dom, id: NodeId) -> Option<String> {
    match dom.children(id).first() {
        Some(&f) => match &dom.node(f).kind {
            NodeKind::Text(t) => Some(t.clone()),
            _ => None,
        },
        None => None,
    }
}

fn set_leading_text(dom: &mut Dom, id: NodeId, text: &str) {
    if let Some(&first) = dom.children(id).first() {
        if let NodeKind::Text(_) = dom.node(first).kind {
            dom.node_mut(first).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(id, 0, t);
}

fn dom_tail(dom: &Dom, id: NodeId) -> Option<String> {
    let parent = dom.parent(id)?;
    let pos = dom.index_in_parent(id)?;
    let next = *dom.children(parent).get(pos + 1)?;
    match &dom.node(next).kind {
        NodeKind::Text(t) => Some(t.clone()),
        _ => None,
    }
}

fn set_dom_tail(dom: &mut Dom, id: NodeId, text: &str) {
    let Some(parent) = dom.parent(id) else { return };
    let Some(pos) = dom.index_in_parent(id) else {
        return;
    };
    let next = dom.children(parent).get(pos + 1).copied();
    if let Some(next) = next {
        if let NodeKind::Text(_) = dom.node(next).kind {
            dom.node_mut(next).kind = NodeKind::Text(text.to_string());
            return;
        }
    }
    let t = dom.new_text(text);
    dom.insert_child(parent, pos + 1, t);
}

/// See [`xml_root_excluded`]'s docs for why comment children are
/// checked, not just element children.
fn dom_root_excluded(dom: &Dom, root: NodeId) -> bool {
    dom.children(root).iter().any(|&c| {
        let text = match &dom.node(c).kind {
            NodeKind::Comment(t) => Some(t.clone()),
            NodeKind::Element(_) => leading_text(dom, c),
            _ => None,
        };
        text.map(|t| t.trim().eq_ignore_ascii_case("calibre-no-spell-check"))
            .unwrap_or(false)
    })
}

fn locale_from_tag(dom: &Dom, id: NodeId) -> Option<DictionaryLocale> {
    let attrs = &dom.node(id).attrs;
    if let Some(lang) = attrs.get("lang") {
        if let Ok(loc) = parse_lang_code(lang) {
            return Some(loc);
        }
    }
    if let Some(lang) = attrs.get("xml:lang") {
        if let Ok(loc) = parse_lang_code(lang) {
            return Some(loc);
        }
    }
    None
}

// ===================================================================
// Word extraction
// ===================================================================

const OPF_SPELL_TAGS: &[&str] = &["title", "creator", "subject", "description", "publisher"];
const HTML_SPELL_TAGS: &[&str] = &["script", "style", "link"];

#[allow(clippy::too_many_arguments)]
fn add_words(
    text: &str,
    location_node: TreeNode,
    words: &mut WordsMap,
    file_name: &str,
    locale: &DictionaryLocale,
    node_item: NodeItem,
    sourceline: Option<u32>,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    let (candidates, raw_count) = get_words(text, &locale.langcode);
    *file_word_count += raw_count;
    if candidates.is_empty() {
        return;
    }
    let p = patterns();
    let is_fr = locale.langcode == "fra";
    for word in candidates {
        let sanitized = p.sanitize_invisible_pat.replace_all(&word, "");
        let mut sword = sanitized.trim().to_string();
        let mut elided_prefix = String::new();
        if is_fr {
            if let Some(m) = p.fr_elision_pat.find(&sword) {
                if m.end() > elided_prefix.len() {
                    elided_prefix = sword[..m.end()].to_string();
                    sword = sword[m.end()..].to_string();
                }
            }
        }
        let loc = Location::new(
            file_name,
            elided_prefix,
            word,
            location_node,
            node_item.clone(),
            sourceline,
        );
        words.entry((sword, locale.clone())).or_default().push(loc);
        *total_word_count += 1;
    }
}

/// Port of `add_words_from_escaped_html`. See the module docs' remapping
/// note: every word extracted from the synthetic parse is reattributed
/// to `owner`/`node_item` (the *original* element/attribute the escaped
/// HTML came from), matching Python exactly.
#[allow(clippy::too_many_arguments)]
fn add_words_from_escaped_html(
    text: &str,
    words: &mut WordsMap,
    file_name: &str,
    owner: XmlNodeId,
    node_item: NodeItem,
    locale: &DictionaryLocale,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    let decoded = crate::html_entities::xml_replace_entities(text);
    let wrapped = format!("<html><body><div>{decoded}</div></body></html>");
    let dom = super::parsing::parse(&wrapped, false, false);
    let Some(html_root) = dom_html_root(&dom) else {
        return;
    };
    let mut ewords: WordsMap = HashMap::new();
    let mut inner_total = 0usize;
    read_words_from_html(
        &dom,
        html_root,
        &mut ewords,
        file_name,
        locale,
        file_word_count,
        &mut inner_total,
    );
    for locs in ewords.values_mut() {
        for loc in locs.iter_mut() {
            loc.location_node = TreeNode::Opf(owner);
            loc.node_item = node_item.clone();
        }
    }
    for (k, locs) in ewords {
        words.entry(k).or_default().extend(locs);
    }
    *total_word_count += inner_total;
}

/// Port of `read_words_from_opf`.
pub fn read_words_from_opf(
    xml: &Xml,
    words: &mut WordsMap,
    file_name: &str,
    book_locale: &DictionaryLocale,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    let root = xml.root_element().unwrap_or(xml.root);
    for tag in xml_iterdescendants(xml, root) {
        if let Some(local) = xml.local_name(tag) {
            if OPF_SPELL_TAGS.contains(&local) {
                if local == "description" {
                    if let Some(text) = xml.element_text(tag) {
                        if !text.is_empty() {
                            add_words_from_escaped_html(
                                text,
                                words,
                                file_name,
                                tag,
                                NodeItem::Text,
                                book_locale,
                                file_word_count,
                                total_word_count,
                            );
                        }
                    }
                    for child in xml.element_children(tag) {
                        if let Some(tail) = xml_tail(xml, child) {
                            if !tail.is_empty() {
                                add_words_from_escaped_html(
                                    tail,
                                    words,
                                    file_name,
                                    child,
                                    NodeItem::Tail,
                                    book_locale,
                                    file_word_count,
                                    total_word_count,
                                );
                            }
                        }
                    }
                } else {
                    if let Some(text) = xml.element_text(tag) {
                        if !text.is_empty() {
                            add_words(
                                text,
                                TreeNode::Opf(tag),
                                words,
                                file_name,
                                book_locale,
                                NodeItem::Text,
                                xml.node(tag).sourceline,
                                file_word_count,
                                total_word_count,
                            );
                        }
                    }
                    for child in xml.element_children(tag) {
                        if let Some(tail) = xml_tail(xml, child) {
                            if !tail.is_empty() {
                                add_words(
                                    tail,
                                    TreeNode::Opf(child),
                                    words,
                                    file_name,
                                    book_locale,
                                    NodeItem::Tail,
                                    xml.node(child).sourceline,
                                    file_word_count,
                                    total_word_count,
                                );
                            }
                        }
                    }
                }
            }
        }
        if let Some(file_as) = xml.get_attr(tag, "file-as") {
            if !file_as.is_empty() {
                let file_as = file_as.to_string();
                add_words(
                    &file_as,
                    TreeNode::Opf(tag),
                    words,
                    file_name,
                    book_locale,
                    NodeItem::Attr("file-as".to_string()),
                    xml.node(tag).sourceline,
                    file_word_count,
                    total_word_count,
                );
            }
        }
    }
}

/// Port of `read_words_from_ncx`.
pub fn read_words_from_ncx(
    xml: &Xml,
    words: &mut WordsMap,
    file_name: &str,
    book_locale: &DictionaryLocale,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    fn walk(xml: &Xml, id: XmlNodeId, out: &mut Vec<XmlNodeId>) {
        if xml.local_name(id) == Some("text") {
            out.push(id);
        }
        for &c in xml.children(id) {
            if matches!(xml.node(c).kind, XmlNodeKind::Element { .. }) {
                walk(xml, c, out);
            }
        }
    }
    let mut tags = Vec::new();
    walk(xml, xml.root, &mut tags);
    for tag in tags {
        if let Some(text) = xml.element_text(tag) {
            add_words(
                text,
                TreeNode::Opf(tag),
                words,
                file_name,
                book_locale,
                NodeItem::Text,
                xml.node(tag).sourceline,
                file_word_count,
                total_word_count,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn read_words_from_html_tag(
    dom: &Dom,
    node: NodeId,
    parent: Option<NodeId>,
    words: &mut WordsMap,
    file_name: &str,
    parent_locale: &DictionaryLocale,
    locale: &DictionaryLocale,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    let own_tag_ok = dom
        .tag(node)
        .map(|t| !HTML_SPELL_TAGS.contains(&t))
        .unwrap_or(false);
    if own_tag_ok {
        if let Some(text) = leading_text(dom, node) {
            if !text.is_empty() {
                add_words(
                    &text,
                    TreeNode::Html(node),
                    words,
                    file_name,
                    locale,
                    NodeItem::Text,
                    None,
                    file_word_count,
                    total_word_count,
                );
            }
        }
    }
    for attr in ["alt", "title"] {
        if let Some(v) = dom.node(node).attrs.get(attr) {
            if !v.is_empty() {
                let v = v.clone();
                add_words(
                    &v,
                    TreeNode::Html(node),
                    words,
                    file_name,
                    locale,
                    NodeItem::Attr(attr.to_string()),
                    None,
                    file_word_count,
                    total_word_count,
                );
            }
        }
    }
    let parent_tag_ok = parent
        .map(|p| {
            dom.tag(p)
                .map(|t| !HTML_SPELL_TAGS.contains(&t))
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if parent_tag_ok {
        if let Some(tail) = dom_tail(dom, node) {
            if !tail.is_empty() {
                add_words(
                    &tail,
                    TreeNode::Html(node),
                    words,
                    file_name,
                    parent_locale,
                    NodeItem::Tail,
                    None,
                    file_word_count,
                    total_word_count,
                );
            }
        }
    }
}

/// Port of `read_words_from_html`.
pub fn read_words_from_html(
    dom: &Dom,
    html_root: NodeId,
    words: &mut WordsMap,
    file_name: &str,
    book_locale: &DictionaryLocale,
    file_word_count: &mut usize,
    total_word_count: &mut usize,
) {
    let mut stack: Vec<(NodeId, Option<NodeId>, DictionaryLocale)> =
        vec![(html_root, None, book_locale.clone())];
    while let Some((node, parent, parent_locale)) = stack.pop() {
        let locale = locale_from_tag(dom, node).unwrap_or_else(|| parent_locale.clone());
        read_words_from_html_tag(
            dom,
            node,
            parent,
            words,
            file_name,
            &parent_locale,
            &locale,
            file_word_count,
            total_word_count,
        );
        for child in dom.children(node) {
            if dom.tag(child).is_some() {
                stack.push((child, Some(node), locale.clone()));
            }
        }
    }
}

/// Port of `group_sort`.
fn group_sort(mut locations: Vec<Location>) -> Vec<Location> {
    let mut order: HashMap<String, usize> = HashMap::new();
    for loc in &locations {
        if !order.contains_key(&loc.file_name) {
            let n = order.len();
            order.insert(loc.file_name.clone(), n);
        }
    }
    locations.sort_by_key(|l| (order[&l.file_name], l.sourceline.unwrap_or(0)));
    locations
}

/// Port of `merge_locations`.
pub fn merge_locations(locs1: Vec<Location>, locs2: Vec<Location>) -> Vec<Location> {
    let mut combined = locs1;
    combined.extend(locs2);
    group_sort(combined)
}

/// Port of `get_checkable_file_names`.
pub fn get_checkable_file_names(
    container: &mut Container,
) -> Result<(Vec<String>, Option<String>)> {
    let mut file_names: Vec<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    file_names.push(container.opf_name.clone());
    let mut ncx_toc = find_existing_ncx_toc(container)?;
    if let Some(n) = ncx_toc.clone() {
        if container.exists(&n) && !file_names.contains(&n) {
            file_names.push(n);
        } else {
            ncx_toc = None;
        }
    }
    if let Some(n) = find_existing_nav_toc(container)? {
        if container.exists(&n) && !file_names.contains(&n) {
            file_names.push(n);
        }
    }
    Ok((file_names, ncx_toc))
}

/// Port of `get_all_words`. Always returns `(total_word_count,
/// grouped_words)`; Python's `get_word_count` flag just controls
/// whether the count is included in the return tuple, which has no
/// ergonomic equivalent worth modeling in Rust -- callers that don't
/// need the count simply ignore it.
pub fn get_all_words(
    container: &mut Container,
    book_locale: &DictionaryLocale,
    excluded_files: &HashSet<String>,
    file_words_counts: &mut HashMap<String, usize>,
) -> Result<(usize, WordsMap)> {
    let (file_names, ncx_toc) = get_checkable_file_names(container)?;
    let mut words: WordsMap = HashMap::new();
    let mut total_word_count = 0usize;
    for file_name in file_names {
        if !container.exists(&file_name) || excluded_files.contains(&file_name) {
            continue;
        }
        if container.ensure_parsed(&file_name).is_err() {
            continue;
        }
        let mut file_word_count = 0usize;
        if file_name == container.opf_name {
            let root = container.opf_root()?;
            let xml = container.get_xml(&file_name)?;
            if xml_root_excluded(xml, root) {
                continue;
            }
            read_words_from_opf(
                xml,
                &mut words,
                &file_name,
                book_locale,
                &mut file_word_count,
                &mut total_word_count,
            );
        } else if Some(&file_name) == ncx_toc.as_ref() {
            let xml = container.get_xml(&file_name)?;
            let Some(root) = xml.root_element() else {
                continue;
            };
            if xml_root_excluded(xml, root) {
                continue;
            }
            read_words_from_ncx(
                xml,
                &mut words,
                &file_name,
                book_locale,
                &mut file_word_count,
                &mut total_word_count,
            );
        } else if let Ok(dom) = container.get_xhtml(&file_name) {
            let Some(html_root) = dom_html_root(dom) else {
                continue;
            };
            if dom_root_excluded(dom, html_root) {
                continue;
            }
            read_words_from_html(
                dom,
                html_root,
                &mut words,
                &file_name,
                book_locale,
                &mut file_word_count,
                &mut total_word_count,
            );
        } else {
            continue;
        }
        file_words_counts.insert(file_name, file_word_count);
    }
    let grouped: WordsMap = words.into_iter().map(|(k, v)| (k, group_sort(v))).collect();
    Ok((total_word_count, grouped))
}

// ===================================================================
// Character counting
// ===================================================================

/// Port of `CharCounter`.
#[derive(Default)]
pub struct CharCounter {
    pub counter: HashMap<char, u32>,
    pub chars: HashMap<char, HashSet<String>>,
}

impl CharCounter {
    fn update(&mut self, text: &str, file_name: &str) {
        for c in text.chars() {
            *self.counter.entry(c).or_insert(0) += 1;
            self.chars
                .entry(c)
                .or_default()
                .insert(file_name.to_string());
        }
    }
}

/// Port of `count_all_chars`. Walks the same checkable files/tags as
/// [`get_all_words`], but only counts characters (no word splitting).
pub fn count_all_chars(
    container: &mut Container,
    book_locale: &DictionaryLocale,
) -> Result<CharCounter> {
    let (file_names, ncx_toc) = get_checkable_file_names(container)?;
    let mut counter = CharCounter::default();
    for file_name in file_names {
        if !container.exists(&file_name) {
            continue;
        }
        if container.ensure_parsed(&file_name).is_err() {
            continue;
        }
        if file_name == container.opf_name {
            let xml = container.get_xml(&file_name)?;
            let root = xml.root_element().unwrap_or(xml.root);
            for tag in xml_iterdescendants(xml, root) {
                if let Some(local) = xml.local_name(tag) {
                    if OPF_SPELL_TAGS.contains(&local) {
                        if let Some(text) = xml.element_text(tag) {
                            counter.update(text, &file_name);
                        }
                        for child in xml.element_children(tag) {
                            if let Some(tail) = xml_tail(xml, child) {
                                counter.update(tail, &file_name);
                            }
                        }
                    }
                }
                if let Some(file_as) = xml.get_attr(tag, "file-as") {
                    counter.update(file_as, &file_name);
                }
            }
        } else if Some(&file_name) == ncx_toc.as_ref() {
            let xml = container.get_xml(&file_name)?;
            fn walk(xml: &Xml, id: XmlNodeId, out: &mut Vec<XmlNodeId>) {
                if xml.local_name(id) == Some("text") {
                    out.push(id);
                }
                for &c in xml.children(id) {
                    if matches!(xml.node(c).kind, XmlNodeKind::Element { .. }) {
                        walk(xml, c, out);
                    }
                }
            }
            let mut tags = Vec::new();
            walk(xml, xml.root, &mut tags);
            for tag in tags {
                if let Some(text) = xml.element_text(tag) {
                    counter.update(text, &file_name);
                }
            }
        } else if let Ok(dom) = container.get_xhtml(&file_name) {
            let Some(html_root) = dom_html_root(dom) else {
                continue;
            };
            let mut stack = vec![html_root];
            while let Some(node) = stack.pop() {
                let tag_ok = dom
                    .tag(node)
                    .map(|t| !HTML_SPELL_TAGS.contains(&t))
                    .unwrap_or(false);
                if tag_ok {
                    if let Some(text) = leading_text(dom, node) {
                        counter.update(&text, &file_name);
                    }
                }
                for attr in ["alt", "title"] {
                    if let Some(v) = dom.node(node).attrs.get(attr) {
                        counter.update(v, &file_name);
                    }
                }
                if tag_ok {
                    if let Some(tail) = dom_tail(dom, node) {
                        counter.update(&tail, &file_name);
                    }
                }
                for child in dom.children(node) {
                    if dom.tag(child).is_some() {
                        stack.push(child);
                    }
                }
            }
        }
    }
    let _ = book_locale;
    Ok(counter)
}

// ===================================================================
// Replace word everywhere / undo
// ===================================================================

fn node_item_text(
    container: &mut Container,
    file_name: &str,
    node: TreeNode,
    item: &NodeItem,
) -> Result<Option<String>> {
    container.ensure_parsed(file_name)?;
    Ok(match (node, item) {
        (TreeNode::Opf(id), NodeItem::Attr(a)) => container
            .get_xml(file_name)?
            .get_attr(id, a)
            .map(|s| s.to_string()),
        (TreeNode::Opf(id), NodeItem::Text) => container
            .get_xml(file_name)?
            .element_text(id)
            .map(|s| s.to_string()),
        (TreeNode::Opf(id), NodeItem::Tail) => {
            xml_tail(container.get_xml(file_name)?, id).map(|s| s.to_string())
        }
        (TreeNode::Html(id), NodeItem::Attr(a)) => container
            .get_xhtml(file_name)?
            .node(id)
            .attrs
            .get(a)
            .cloned(),
        (TreeNode::Html(id), NodeItem::Text) => leading_text(container.get_xhtml(file_name)?, id),
        (TreeNode::Html(id), NodeItem::Tail) => dom_tail(container.get_xhtml(file_name)?, id),
    })
}

fn set_node_item_text(
    container: &mut Container,
    file_name: &str,
    node: TreeNode,
    item: &NodeItem,
    value: &str,
) -> Result<()> {
    match (node, item) {
        (TreeNode::Opf(id), NodeItem::Attr(a)) => {
            container
                .get_xml_mut(file_name)?
                .set_attr(id, a, value.to_string());
        }
        (TreeNode::Opf(id), NodeItem::Text) => {
            container
                .get_xml_mut(file_name)?
                .set_element_text(id, value.to_string());
        }
        (TreeNode::Opf(id), NodeItem::Tail) => {
            xml_set_tail(container.get_xml_mut(file_name)?, id, value);
        }
        (TreeNode::Html(id), NodeItem::Attr(a)) => {
            container
                .get_xhtml_mut(file_name)?
                .node_mut(id)
                .attrs
                .insert(a.clone(), value.to_string());
        }
        (TreeNode::Html(id), NodeItem::Text) => {
            set_leading_text(container.get_xhtml_mut(file_name)?, id, value);
        }
        (TreeNode::Html(id), NodeItem::Tail) => {
            set_dom_tail(container.get_xhtml_mut(file_name)?, id, value);
        }
    }
    Ok(())
}

/// Key for [`replace_word`]'s undo cache: `(file_name, node, item)`.
pub type UndoKey = (String, TreeNode, NodeItem);

/// Port of `replace_word`. Returns the set of files that were changed.
/// Pass `Some(cache)` to record pre-replacement text for
/// [`undo_replace_word`].
pub fn replace_word(
    container: &mut Container,
    new_word: &str,
    locations: &[Location],
    locale: &DictionaryLocale,
    mut undo_cache: Option<&mut HashMap<UndoKey, String>>,
) -> Result<HashSet<String>> {
    let mut changed = HashSet::new();
    for loc in locations {
        let Some(text) =
            node_item_text(container, &loc.file_name, loc.location_node, &loc.node_item)?
        else {
            continue;
        };
        let replacement = format!("{}{}", loc.elided_prefix, new_word);
        let (rtext, replaced) = replace(&text, &loc.original_word, &replacement, &locale.langcode);
        if replaced {
            if let Some(cache) = undo_cache.as_deref_mut() {
                cache.insert(
                    (
                        loc.file_name.clone(),
                        loc.location_node,
                        loc.node_item.clone(),
                    ),
                    text,
                );
            }
            set_node_item_text(
                container,
                &loc.file_name,
                loc.location_node,
                &loc.node_item,
                &rtext,
            )?;
            container.dirty(&loc.file_name);
            changed.insert(loc.file_name.clone());
        }
    }
    Ok(changed)
}

/// Port of `undo_replace_word`.
pub fn undo_replace_word(
    container: &mut Container,
    undo_cache: HashMap<UndoKey, String>,
) -> Result<HashSet<String>> {
    let mut changed = HashSet::new();
    for ((file_name, node, item), text) in undo_cache {
        set_node_item_text(container, &file_name, node, &item, &text)?;
        container.dirty(&file_name);
        changed.insert(file_name);
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(
        files: &[(&str, &str, &[u8])],
        spine: &[&str],
    ) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let spine_items: String = spine
            .iter()
            .map(|n| format!(r#"<itemref idref="{n}"/>"#))
            .collect();
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata><dc:title>The Great Book</dc:title><dc:creator opf:file-as="Doe, Jane">Jane Doe</dc:creator><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine>{spine_items}</spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    fn en() -> DictionaryLocale {
        DictionaryLocale::new("eng", None)
    }

    // parse_lang_code/split_into_words are now imported from
    // crate::spell -- see that module's own tests for their coverage.

    #[test]
    fn replace_replaces_whole_word_occurrences_only() {
        let (out, changed) = replace("cat category cat", "cat", "dog", "eng");
        assert!(changed);
        assert_eq!(out, "dog category dog");
    }

    #[test]
    fn get_all_words_finds_opf_title_and_creator_with_file_as() {
        let (_dir, mut container) = make_container(&[], &[]);
        let mut counts = HashMap::new();
        let (total, words) =
            get_all_words(&mut container, &en(), &HashSet::new(), &mut counts).unwrap();
        assert!(total > 0);
        let has_word = |w: &str| words.keys().any(|(sword, _)| sword == w);
        assert!(has_word("The"), "{:?}", words.keys().collect::<Vec<_>>());
        assert!(has_word("Great"));
        assert!(has_word("Jane"));
        assert!(has_word("Doe"), "file-as attribute should be scanned too");
    }

    #[test]
    fn get_all_words_finds_html_content_and_alt_text() {
        let (_dir, mut container) = make_container(
            &[(
                "chap1.html",
                "application/xhtml+xml",
                b"<html><body><p>Hello wonderful world</p><img alt=\"A cute cat\"/></body></html>",
            )],
            &["chap1.html"],
        );
        let mut counts = HashMap::new();
        let (_total, words) =
            get_all_words(&mut container, &en(), &HashSet::new(), &mut counts).unwrap();
        let has_word = |w: &str| words.keys().any(|(sword, _)| sword == w);
        assert!(has_word("wonderful"));
        assert!(has_word("cute"), "alt attribute text should be scanned");
        // 3 words in the <p> text + 3 words in the alt attribute ("A",
        // "cute", "cat") -- file_word_count accumulates every
        // get_words() call for the file, matching Python's global
        // `file_word_count` counter.
        assert_eq!(counts.get("chap1.html").copied(), Some(6));
    }

    #[test]
    fn get_all_words_skips_script_and_style_content() {
        let (_dir, mut container) = make_container(
            &[(
                "chap1.html",
                "application/xhtml+xml",
                b"<html><body><script>var forbiddenWord = 1;</script><p>ordinary text</p></body></html>",
            )],
            &["chap1.html"],
        );
        let mut counts = HashMap::new();
        let (_total, words) =
            get_all_words(&mut container, &en(), &HashSet::new(), &mut counts).unwrap();
        let has_word = |w: &str| words.keys().any(|(sword, _)| sword == w);
        assert!(!has_word("forbiddenWord"));
        assert!(has_word("ordinary"));
    }

    #[test]
    fn replace_word_updates_html_text_and_supports_undo() {
        let (_dir, mut container) = make_container(
            &[(
                "chap1.html",
                "application/xhtml+xml",
                b"<html><body><p>the cat sat</p></body></html>",
            )],
            &["chap1.html"],
        );
        let mut counts = HashMap::new();
        let (_total, words) =
            get_all_words(&mut container, &en(), &HashSet::new(), &mut counts).unwrap();
        let locations = words.get(&("cat".to_string(), en())).unwrap().clone();
        let mut undo_cache = HashMap::new();
        let changed = replace_word(
            &mut container,
            "dog",
            &locations,
            &en(),
            Some(&mut undo_cache),
        )
        .unwrap();
        assert!(changed.contains("chap1.html"));
        let dom = container.get_xhtml("chap1.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(leading_text(dom, p).as_deref(), Some("the dog sat"));

        let undone = undo_replace_word(&mut container, undo_cache).unwrap();
        assert!(undone.contains("chap1.html"));
        let dom = container.get_xhtml("chap1.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(leading_text(dom, p).as_deref(), Some("the cat sat"));
    }

    #[test]
    fn root_excluded_from_spell_check_marker_skips_file() {
        // Real books place the marker as an HTML comment directly under
        // <html> (matching `gui2/tweak_book/spell.py`'s
        // `<!-- calibre-no-spell-check -->`), not as element text.
        let (_dir, mut container) = make_container(
            &[(
                "chap1.html",
                "application/xhtml+xml",
                b"<html><!-- calibre-no-spell-check --><body><p>ordinary secretword</p></body></html>",
            )],
            &["chap1.html"],
        );
        let mut counts = HashMap::new();
        let (_total, words) =
            get_all_words(&mut container, &en(), &HashSet::new(), &mut counts).unwrap();
        assert!(!words.keys().any(|(w, _)| w == "secretword"));
    }

    #[test]
    fn count_all_chars_counts_and_tracks_source_files() {
        // Note: `count_all_chars` (like Python) always scans the OPF's
        // spell-checkable metadata tags too (dc:title/dc:creator/...),
        // so this asserts on a character ('z') that only appears in the
        // HTML body, rather than assuming the HTML file is the only
        // source scanned.
        let (_dir, mut container) = make_container(
            &[(
                "chap1.html",
                "application/xhtml+xml",
                b"<html><body><p>zzz</p></body></html>",
            )],
            &["chap1.html"],
        );
        let counter = count_all_chars(&mut container, &en()).unwrap();
        assert_eq!(counter.counter.get(&'z').copied(), Some(3));
        assert!(counter
            .chars
            .get(&'z')
            .map(|s| s.contains("chap1.html"))
            .unwrap_or(false));
    }
}
