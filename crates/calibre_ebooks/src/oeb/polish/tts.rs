//! Port of `oeb/polish/tts.py`'s sentence marking (issue #645): given an
//! HTML document, wrap each detected sentence in a `<span id="cttsw-N">`
//! so a later stage can align synthesized audio against it.
//!
//! This is the other half of issue #167 (its `report.py` half shipped as
//! #590). The remaining pieces of that epic -- SMIL generation (#646),
//! batch synthesis (#647), audio transcoding (#648) and the `embed_tts`
//! orchestrator (#649) -- are separate issues; this module is the real,
//! self-contained core they all build on.
//!
//! # Emulating lxml's text model
//!
//! Upstream is written against lxml, where a text run is *not* a node:
//! an element carries `.text` (the run before its first child) and
//! `.tail` (the run after its own closing tag), and child indices count
//! only elements. [`Dom`] instead stores a run as an ordinary child
//! node. For issue #651 (`kepubify.py`'s span wrapping) that difference
//! *simplified* the port -- the algorithm only ever needed a run's own
//! position. This one is different: it genuinely indexes by element
//! position (`self.elem.index(s.child)`, `self.elem[idx+1]`,
//! `p[idx+1:idx+1] = spans`), so rather than restructure ten branches of
//! delicate tree surgery, this module provides a small faithful lxml
//! view over [`Dom`] -- [`el_children`], [`el_index`], [`el_insert`],
//! [`get_text`]/[`set_text`], [`get_tail`]/[`set_tail`] -- and then
//! transcribes upstream's branches essentially statement for statement.
//!
//! Two consequences of that emulation are easy to get wrong and are
//! worth stating explicitly:
//!
//! * **A moved element takes its tail with it.** In lxml a tail belongs
//!   to its element, so `w.append(c)` moves the run after `c` inside `w`
//!   too. Here that run is a separate sibling node that would otherwise
//!   stay behind, so [`el_append_with_tail`] moves it explicitly. The
//!   same applies to removal: lxml's `p.remove(el)` drops the tail.
//! * **Offsets are in characters, not bytes.** See
//!   [`mark_sentences_in_html`]'s note on `split_into_sentences_for_tts_embed`.

use std::collections::{HashMap, HashSet};

use crate::dom::{Dom, NodeId, NodeKind};
use crate::oeb::polish::container::seconds_to_timestamp;
use crate::spell::break_iterator::split_into_sentences_for_tts_embed;
use crate::xmltree::{Xml, XmlNodeId};

/// Port of the `Sentence` NamedTuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub elem_id: String,
    pub text: String,
    pub lang: String,
    pub voice: String,
}

/// Port of `continued_tag_names`: inline tags whose text is folded into
/// the parent's own run rather than being descended into separately.
pub const CONTINUED_TAG_NAMES: &[&str] = &[
    "a", "span", "em", "strong", "b", "i", "u", "code", "sub", "sup", "cite", "q", "kbd",
];

/// Port of `ignored_tag_names`.
pub const IGNORED_TAG_NAMES: &[&str] = &[
    "img", "object", "script", "style", "head", "title", "form", "input", "br", "hr", "map",
    "textarea", "svg", "math", "rp", "rt", "rtc",
];

pub const ID_PREFIX: &str = "cttsw-";
pub const DATA_NAME: &str = "data-calibre-tts";
pub const SKIP_NAME: &str = "__skip__";

// ---------------------------------------------------------------------
// The lxml view over `Dom`
// ---------------------------------------------------------------------

fn is_text(dom: &Dom, id: NodeId) -> bool {
    matches!(dom.node(id).kind, NodeKind::Text(_))
}

fn text_of(dom: &Dom, id: NodeId) -> String {
    match &dom.node(id).kind {
        NodeKind::Text(t) => t.clone(),
        _ => String::new(),
    }
}

/// Port of lxml's child list: elements *and* comments, never text runs.
pub fn el_children(dom: &Dom, parent: NodeId) -> Vec<NodeId> {
    dom.node(parent).children.iter().copied().filter(|&c| !is_text(dom, c)).collect()
}

/// Port of `elem.iterchildren('*')`, which unlike the bare
/// `iterchildren()` yields only real elements -- comments excluded.
fn element_children(dom: &Dom, parent: NodeId) -> Vec<NodeId> {
    dom.node(parent)
        .children
        .iter()
        .copied()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .collect()
}

/// Port of `parent.index(child)`.
pub fn el_index(dom: &Dom, parent: NodeId, child: NodeId) -> Option<usize> {
    el_children(dom, parent).iter().position(|&c| c == child)
}

/// The [`Dom`] child position that element index `el_idx` corresponds
/// to. Inserting there puts the new element immediately *before*
/// element `el_idx` -- which is to say after element `el_idx - 1` and
/// its tail, exactly where lxml's `insert` puts it.
fn el_dom_pos(dom: &Dom, parent: NodeId, el_idx: usize) -> usize {
    let mut seen = 0;
    for (i, &c) in dom.node(parent).children.iter().enumerate() {
        if !is_text(dom, c) {
            if seen == el_idx {
                return i;
            }
            seen += 1;
        }
    }
    dom.node(parent).children.len()
}

/// Port of `parent.insert(el_idx, node)`.
pub fn el_insert(dom: &mut Dom, parent: NodeId, el_idx: usize, node: NodeId) {
    let pos = el_dom_pos(dom, parent, el_idx);
    dom.insert_child(parent, pos, node);
}

/// Moves `child` (and the run that follows it, its lxml tail) to the end
/// of `new_parent`'s children -- port of `w.append(c)`.
fn el_append_with_tail(dom: &mut Dom, new_parent: NodeId, child: NodeId) {
    let tail = tail_node(dom, child);
    dom.append_child(new_parent, child);
    if let Some(t) = tail {
        dom.append_child(new_parent, t);
    }
}

/// Port of `parent.remove(el)`, which in lxml discards the tail too.
fn el_remove(dom: &mut Dom, child: NodeId) {
    if let Some(t) = tail_node(dom, child) {
        dom.detach(t);
    }
    dom.detach(child);
}

/// The text node holding `elem`'s lxml `.tail`, if it has one.
fn tail_node(dom: &Dom, elem: NodeId) -> Option<NodeId> {
    let parent = dom.parent(elem)?;
    let idx = dom.index_in_parent(elem)?;
    dom.node(parent).children.get(idx + 1).copied().filter(|&n| is_text(dom, n))
}

/// Port of reading `elem.text`.
pub fn get_text(dom: &Dom, elem: NodeId) -> Option<String> {
    dom.node(elem).children.first().copied().filter(|&c| is_text(dom, c)).map(|c| text_of(dom, c))
}

/// Port of assigning `elem.text`. `None` removes the run, as in lxml;
/// so does `Some("")`, since lxml's empty string is falsy everywhere
/// this algorithm tests it and serializes to nothing either way.
pub fn set_text(dom: &mut Dom, elem: NodeId, value: Option<&str>) {
    let value = value.filter(|v| !v.is_empty());
    let existing = dom.node(elem).children.first().copied().filter(|&c| is_text(dom, c));
    match (existing, value) {
        (Some(node), Some(v)) => {
            if let NodeKind::Text(t) = &mut dom.node_mut(node).kind {
                v.clone_into(t);
            }
        }
        (Some(node), None) => dom.detach(node),
        (None, Some(v)) => {
            let t = dom.new_text(v);
            dom.insert_child(elem, 0, t);
        }
        (None, None) => {}
    }
}

/// Port of reading `elem.tail`.
pub fn get_tail(dom: &Dom, elem: NodeId) -> Option<String> {
    tail_node(dom, elem).map(|n| text_of(dom, n))
}

/// Port of assigning `elem.tail`. `None` removes the run, as in lxml;
/// so does `Some("")`, for the same reason as [`set_text`].
pub fn set_tail(dom: &mut Dom, elem: NodeId, value: Option<&str>) {
    let value = value.filter(|v| !v.is_empty());
    match (tail_node(dom, elem), value) {
        (Some(node), Some(v)) => {
            if let NodeKind::Text(t) = &mut dom.node_mut(node).kind {
                v.clone_into(t);
            }
        }
        (Some(node), None) => dom.detach(node),
        (None, Some(v)) => {
            let (Some(parent), Some(idx)) = (dom.parent(elem), dom.index_in_parent(elem)) else {
                return;
            };
            let t = dom.new_text(v);
            dom.insert_child(parent, idx + 1, t);
        }
        (None, None) => {}
    }
}

// ---------------------------------------------------------------------
// Character-indexed slicing
// ---------------------------------------------------------------------
//
// Every offset in this algorithm is a *character* index, matching
// Python. See `mark_sentences_in_html`'s own note for why byte offsets
// would be actively wrong here.

fn char_len(s: &str) -> usize {
    s.chars().count()
}

fn char_range(s: &str, from: usize, to: usize) -> String {
    s.chars().skip(from).take(to.saturating_sub(from)).collect()
}

fn char_from(s: &str, from: usize) -> String {
    s.chars().skip(from).collect()
}

fn char_to(s: &str, to: usize) -> String {
    s.chars().take(to).collect()
}

/// `None` for an empty string, so a run can be tested for truthiness the
/// way Python tests `if text:`.
fn non_empty(s: &str) -> Option<&str> {
    (!s.is_empty()).then_some(s)
}

// ---------------------------------------------------------------------
// The marker
// ---------------------------------------------------------------------

/// Port of the `Chunk` NamedTuple. `start_at` is a character offset into
/// the parent's concatenated run text.
#[derive(Debug, Clone)]
struct Chunk {
    child: Option<NodeId>,
    text: String,
    start_at: usize,
    is_tail: bool,
}

/// Upstream's `mark_sentences_in_html` closure state, which its nested
/// `Parent` class reaches via `nonlocal`.
struct Marker<'a> {
    dom: &'a mut Dom,
    id_counter: u32,
    seen_ids: HashSet<String>,
    out: Vec<Sentence>,
    /// Upstream's `clones_map`, kept insertion-ordered (a Python dict is)
    /// so the final cleanup pass runs in the same order.
    clone_order: Vec<NodeId>,
    clones: HashMap<NodeId, Vec<NodeId>>,
}

/// Port of the nested `Parent` class. Upstream reuses one `self.pos`
/// field for two unrelated purposes -- a running character offset while
/// the chunk list is built, then a chunk *cursor* once `commit` resets
/// it to 0 -- which is split into two fields here.
struct Parent {
    elem: NodeId,
    lang: String,
    parent_lang: String,
    parent_voice: String,
    voice: String,
    next_offset: usize,
    cursor: usize,
    texts: Vec<Chunk>,
    children: Vec<NodeId>,
    has_tail: bool,
}

/// Port of `lang_for_elem`, shared with `kepubify.py` (see
/// [`super::kepubify::lang_for_elem`], which is the same function).
fn lang_for_elem(dom: &Dom, elem: NodeId, parent_lang: &str) -> String {
    super::kepubify::lang_for_elem(dom, elem, parent_lang)
}

fn voice_for_elem(dom: &Dom, elem: NodeId, parent_voice: &str) -> String {
    let q = dom.node(elem).attrs.get(DATA_NAME).cloned().unwrap_or_default();
    if q.starts_with('{') {
        // Upstream parses this as JSON and reads `voice`, falling back to
        // the parent's on any error -- `with suppress(Exception)`.
        return serde_json::from_str::<serde_json::Value>(&q)
            .ok()
            .and_then(|v| v.get("voice").and_then(|s| s.as_str()).map(str::to_string))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| parent_voice.to_string());
    }
    if q.is_empty() {
        parent_voice.to_string()
    } else {
        q
    }
}

impl Parent {
    fn new(dom: &Dom, elem: NodeId, parent_lang: &str, parent_voice: &str, child_lang: Option<String>) -> Self {
        let lang = child_lang.unwrap_or_else(|| lang_for_elem(dom, elem, parent_lang));
        let voice = voice_for_elem(dom, elem, parent_voice);
        let mut texts = Vec::new();
        let mut next_offset = 0;
        if let Some(t) = get_text(dom, elem) {
            if !t.trim().is_empty() {
                texts.push(Chunk { child: None, text: t.clone(), start_at: next_offset, is_tail: false });
                next_offset += char_len(&t);
            }
        }
        Parent {
            elem,
            lang,
            parent_lang: parent_lang.to_string(),
            parent_voice: parent_voice.to_string(),
            voice,
            next_offset,
            cursor: 0,
            texts,
            children: el_children(dom, elem),
            has_tail: get_tail(dom, elem).map(|t| !t.trim().is_empty()).unwrap_or(false),
        }
    }

    fn add_simple_child(&mut self, dom: &Dom, elem: NodeId) {
        if let Some(text) = get_text(dom, elem).and_then(|t| non_empty(&t).map(str::to_string)) {
            self.texts.push(Chunk { child: Some(elem), text: text.clone(), start_at: self.next_offset, is_tail: false });
            self.next_offset += char_len(&text);
        }
    }

    fn add_tail(&mut self, elem: NodeId, text: &str) {
        self.texts.push(Chunk { child: Some(elem), text: text.to_string(), start_at: self.next_offset, is_tail: true });
        self.next_offset += char_len(text);
    }
}

impl Marker<'_> {
    /// Port of `make_into_wrapper`. Upstream deliberately does *not*
    /// bump `id_counter` on success, so the next call re-tests the same
    /// number, finds it taken, and only then advances -- preserved.
    fn make_into_wrapper(&mut self, elem: NodeId) -> String {
        loop {
            let q = format!("{ID_PREFIX}{}", self.id_counter);
            if !self.seen_ids.contains(&q) {
                self.dom.node_mut(elem).attrs.insert("id".to_string(), q.clone());
                self.seen_ids.insert(q.clone());
                return q;
            }
            self.id_counter += 1;
        }
    }

    /// Port of `make_wrapper`.
    fn make_wrapper(&mut self, text: Option<&str>) -> NodeId {
        let w = self.dom.new_element("span");
        if let Some(t) = text {
            let tn = self.dom.new_text(t);
            self.dom.append_child(w, tn);
        }
        self.make_into_wrapper(w);
        w
    }

    /// Port of `replace_reference_to_child`.
    fn replace_reference_to_child(&mut self, p: &mut Parent, elem: NodeId, replacement: NodeId) {
        for i in (p.cursor + 1)..p.texts.len() {
            if p.texts[i].child == Some(elem) {
                p.texts[i].child = Some(replacement);
            } else {
                break;
            }
        }
    }

    /// Port of `wrap_contents`: wraps the element range
    /// `first_child ..= last_child` of `p.elem` in a new span. When
    /// `first_child` is `None` the range starts at the first element and
    /// the wrapper also takes over `p.elem`'s leading run.
    fn wrap_contents(&mut self, p: &mut Parent, first_child: Option<NodeId>, last_child: NodeId) -> NodeId {
        let text = if first_child.is_none() { get_text(self.dom, p.elem) } else { None };
        let w = self.make_wrapper(text.as_deref());
        let mut first_child = first_child;
        let mut in_range = false;
        // Snapshotting the children up front matches lxml's own
        // `iterchildren`, which pre-advances past the yielded node before
        // the body can move it out of `elem`.
        for c in element_children(self.dom, p.elem) {
            if !in_range && (first_child.is_none() || first_child == Some(c)) {
                in_range = true;
                let pos = el_index(self.dom, p.elem, c).unwrap_or(0);
                el_insert(self.dom, p.elem, pos, w);
                el_append_with_tail(self.dom, w, c);
                first_child = Some(c);
            }
            if in_range {
                if c == last_child {
                    // Upstream re-appends here unless it is the very
                    // element just appended above; either way `c` ends up
                    // last inside `w`.
                    if Some(last_child) != first_child {
                        el_append_with_tail(self.dom, w, c);
                    }
                    break;
                }
                el_append_with_tail(self.dom, w, c);
            }
        }
        self.replace_reference_to_child(p, last_child, w);
        w
    }

    /// Port of `clone_simple_element`.
    fn clone_simple_element(&mut self, p: &mut Parent, elem: NodeId) -> NodeId {
        let tag = self.dom.tag(elem).unwrap_or("span").to_string();
        let ans = self.dom.new_element(&tag);
        let mut attrs = self.dom.node(elem).attrs.clone();
        attrs.shift_remove("id");
        attrs.shift_remove("name");
        self.dom.node_mut(ans).attrs = attrs;
        let (text, tail) = (get_text(self.dom, elem), get_tail(self.dom, elem));
        set_text(self.dom, ans, text.as_deref());

        let parent = self.dom.parent(elem).expect("clone_simple_element needs an attached element");
        let idx = el_index(self.dom, parent, elem).expect("elem is a child of its parent");
        el_insert(self.dom, parent, idx + 1, ans);
        set_tail(self.dom, ans, tail.as_deref());

        self.replace_reference_to_child(p, elem, ans);
        if !self.clones.contains_key(&elem) {
            self.clone_order.push(elem);
        }
        self.clones.entry(elem).or_default().push(ans);
        ans
    }
}

impl Marker<'_> {
    /// Port of `wrap_sentence`: wraps the sentence occupying characters
    /// `start .. start + length` of `p`'s concatenated run text, which
    /// may begin and end anywhere among its chunks. Returns the id of
    /// the element that now carries the sentence.
    fn wrap_sentence(&mut self, p: &mut Parent, start: usize, length: usize) -> String {
        let end = start + length;
        let mut start_chunk: Option<usize> = None;
        let mut start_offset = 0usize;
        let mut end_chunk: Option<usize> = None;
        let mut end_offset = 0usize;
        for i in p.cursor..p.texts.len() {
            let c = &p.texts[i];
            if c.start_at <= start {
                start_chunk = Some(i);
                start_offset = start - c.start_at;
            }
            if end <= c.start_at + char_len(&c.text) {
                end_chunk = Some(i);
                p.cursor = i;
                end_offset = end - c.start_at;
                break;
            }
        }
        // Upstream's `for ... else`: no chunk contained the end, so the
        // sentence runs to the end of the last one.
        let end_chunk = end_chunk.unwrap_or_else(|| {
            let last = p.texts.len() - 1;
            p.cursor = last;
            end_offset = char_len(&p.texts[last].text);
            last
        });
        let start_chunk = start_chunk.expect("some chunk contains the sentence start");
        let s = p.texts[start_chunk].clone();
        let e = p.texts[end_chunk].clone();
        let same_chunk = start_chunk == end_chunk;

        if s.child.is_none() {
            // ---- the sentence starts in `elem`'s own leading run ----
            if same_chunk {
                // ...and ends there too.
                let before = char_to(&s.text, start_offset);
                let sentence = char_range(&s.text, start_offset, end_offset);
                let after = char_from(&s.text, end_offset);
                set_text(self.dom, p.elem, Some(&before));
                let w = self.make_wrapper(Some(&sentence));
                el_insert(self.dom, p.elem, 0, w);
                set_tail(self.dom, w, Some(&after));
                self.advance(p, w, &after, end, true);
                return self.id_of(w);
            }
            if e.is_tail {
                // ...and ends in the tail of one of its children.
                let before_start = char_to(&s.text, start_offset);
                let after_start = char_from(&s.text, start_offset);
                let included = char_to(&e.text, end_offset);
                let after = char_from(&e.text, end_offset);
                let e_child = e.child.expect("a tail chunk names its element");
                set_tail(self.dom, e_child, Some(&included));
                // `wrap_contents` reads `elem.text` for the wrapper, so
                // upstream deliberately assigns it twice around the call.
                set_text(self.dom, p.elem, Some(&after_start));
                let w = self.wrap_contents(p, None, e_child);
                set_tail(self.dom, w, Some(&after));
                set_text(self.dom, p.elem, Some(&before_start));
                self.advance(p, w, &after, end, true);
                return self.id_of(w);
            }
            // ...and ends inside a child's own text.
            let before_start = char_to(&s.text, start_offset);
            let after_start = char_from(&s.text, start_offset);
            let included = char_to(&e.text, end_offset);
            let after = char_from(&e.text, end_offset);
            let e_child = e.child.expect("a text chunk of a child names it");
            set_text(self.dom, e_child, Some(&included));
            let c = self.clone_simple_element(p, e_child);
            set_text(self.dom, c, Some(&after));
            set_tail(self.dom, e_child, None);
            set_text(self.dom, p.elem, Some(&after_start));
            let w = self.wrap_contents(p, None, e_child);
            set_text(self.dom, p.elem, Some(&before_start));
            self.advance(p, c, &after, end, false);
            return self.id_of(w);
        }

        let s_child = s.child.expect("checked above");
        if s.is_tail {
            // ---- the sentence starts in a child's tail ----
            if e.is_tail {
                if same_chunk {
                    // ...and ends in that same tail.
                    let before = char_to(&s.text, start_offset);
                    let sentence = char_range(&s.text, start_offset, end_offset);
                    let after = char_from(&s.text, end_offset);
                    set_tail(self.dom, s_child, Some(&before));
                    let w = self.make_wrapper(Some(&sentence));
                    set_tail(self.dom, w, Some(&after));
                    let idx = el_index(self.dom, p.elem, s_child).expect("chunk element is a child");
                    el_insert(self.dom, p.elem, idx + 1, w);
                    self.advance(p, w, &after, end, true);
                    return self.id_of(w);
                }
                // ...and ends in a later child's tail.
                let after_start = char_from(&s.text, start_offset);
                set_tail(self.dom, s_child, Some(&char_to(&s.text, start_offset)));
                let after_end = char_from(&e.text, end_offset);
                let e_child = e.child.expect("a tail chunk names its element");
                set_tail(self.dom, e_child, Some(&char_to(&e.text, end_offset)));
                let idx = el_index(self.dom, p.elem, s_child).expect("chunk element is a child");
                let first = el_children(self.dom, p.elem).get(idx + 1).copied();
                let w = self.wrap_contents(p, first, e_child);
                set_text(self.dom, w, Some(&after_start));
                set_tail(self.dom, w, Some(&after_end));
                self.advance(p, w, &after_end, end, true);
                return self.id_of(w);
            }
            // ...and ends inside a later child's text.
            let after_start = char_from(&s.text, start_offset);
            set_tail(self.dom, s_child, Some(&char_to(&s.text, start_offset)));
            let after_end = char_from(&e.text, end_offset);
            let e_child = e.child.expect("a text chunk of a child names it");
            set_text(self.dom, e_child, Some(&char_to(&e.text, end_offset)));
            let c = self.clone_simple_element(p, e_child);
            set_text(self.dom, c, Some(&after_end));
            set_tail(self.dom, e_child, None);
            let idx = el_index(self.dom, p.elem, s_child).expect("chunk element is a child");
            let first = el_children(self.dom, p.elem).get(idx + 1).copied();
            let w = self.wrap_contents(p, first, e_child);
            set_text(self.dom, w, Some(&after_start));
            self.advance(p, c, &after_end, end, false);
            return self.id_of(w);
        }

        // ---- the sentence starts in a child's own text ----
        let e_child = e.child.expect("chunks past the first name their element");
        if s_child == e_child {
            if e.is_tail {
                // ...and ends in that same element's tail.
                let before_start = char_to(&s.text, start_offset);
                let after_start = char_from(&s.text, start_offset);
                let c = self.clone_simple_element(p, s_child);
                set_text(self.dom, s_child, Some(&before_start));
                set_tail(self.dom, s_child, None);
                let before_end = char_to(&e.text, end_offset);
                let after_end = char_from(&e.text, end_offset);
                set_text(self.dom, c, Some(&after_start));
                set_tail(self.dom, c, Some(&before_end));
                let w = self.wrap_contents(p, Some(c), c);
                set_tail(self.dom, w, Some(&after_end));
                self.advance(p, w, &after_end, end, true);
                return self.id_of(w);
            }
            // ...and ends in that same element's text.
            let before = char_to(&s.text, start_offset);
            let sentence = char_range(&s.text, start_offset, end_offset);
            let after = char_from(&s.text, end_offset);
            let c = self.clone_simple_element(p, s_child);
            set_text(self.dom, s_child, Some(&before));
            set_tail(self.dom, s_child, None);
            set_text(self.dom, c, Some(&sentence));
            set_tail(self.dom, c, None);
            let c2 = self.clone_simple_element(p, c);
            set_text(self.dom, c2, Some(&after));
            // The element itself becomes the wrapper -- no span needed,
            // since the sentence is exactly one simple element's text.
            let id = self.make_into_wrapper(c);
            self.advance(p, c2, &after, end, false);
            return id;
        }

        // ---- ...and ends in a later child ----
        let after_start = char_from(&s.text, start_offset);
        set_text(self.dom, s_child, Some(&char_to(&s.text, start_offset)));
        let c = self.clone_simple_element(p, s_child);
        set_text(self.dom, c, Some(&after_start));
        set_tail(self.dom, s_child, None);
        if e.is_tail {
            let after_end = char_from(&e.text, end_offset);
            set_tail(self.dom, e_child, Some(&char_to(&e.text, end_offset)));
            let w = self.wrap_contents(p, Some(c), e_child);
            set_tail(self.dom, w, Some(&after_end));
            self.advance(p, w, &after_end, end, true);
            return self.id_of(w);
        }
        let after_end = char_from(&e.text, end_offset);
        set_text(self.dom, e_child, Some(&char_to(&e.text, end_offset)));
        let c2 = self.clone_simple_element(p, e_child);
        set_text(self.dom, c2, Some(&after_end));
        set_tail(self.dom, e_child, None);
        let w = self.wrap_contents(p, Some(c), e_child);
        self.advance(p, c2, &after_end, end, false);
        self.id_of(w)
    }

    /// The tail of every `wrap_sentence` branch: either the leftover text
    /// becomes the cursor's new chunk, or the cursor simply moves on.
    fn advance(&mut self, p: &mut Parent, child: NodeId, leftover: &str, end: usize, is_tail: bool) {
        if leftover.is_empty() {
            p.cursor += 1;
        } else {
            p.texts[p.cursor] = Chunk { child: Some(child), text: leftover.to_string(), start_at: end, is_tail };
        }
    }

    fn id_of(&self, elem: NodeId) -> String {
        self.dom.node(elem).attrs.get("id").cloned().unwrap_or_default()
    }

    /// Port of `commit`.
    fn commit(&mut self, p: &mut Parent) {
        if !p.texts.is_empty() {
            let text: String = p.texts.iter().map(|c| c.text.as_str()).collect();
            p.cursor = 0;
            for (start, length) in sentence_char_positions(&text, &p.lang) {
                let stext = char_range(&text, start, start + length);
                if !stext.trim().is_empty() && p.voice != SKIP_NAME {
                    let elem_id = self.wrap_sentence(p, start, length);
                    self.out.push(Sentence {
                        elem_id,
                        text: stext,
                        lang: p.lang.clone(),
                        voice: p.voice.clone(),
                    });
                }
            }
        }
        if p.has_tail {
            let Some(parent) = self.dom.parent(p.elem) else { return };
            let tail = get_tail(self.dom, p.elem).unwrap_or_default();
            let mut spans: Vec<NodeId> = Vec::new();
            let mut before: Option<String> = None;
            let mut after: Option<String> = None;
            for (start, length) in sentence_char_positions(&tail, &p.parent_lang) {
                let end = start + length;
                let text = char_range(&tail, start, end);
                if text.trim().is_empty() || p.parent_voice == SKIP_NAME {
                    continue;
                }
                if before.is_none() {
                    before = Some(char_to(&tail, start));
                }
                let span = self.make_wrapper(Some(&text));
                spans.push(span);
                self.out.push(Sentence {
                    elem_id: self.id_of(span),
                    text,
                    lang: p.parent_lang.clone(),
                    voice: p.parent_voice.clone(),
                });
                after = Some(char_from(&tail, end));
            }
            set_tail(self.dom, p.elem, before.as_deref());
            if let (Some(a), Some(&last)) = (after.as_deref().filter(|a| !a.is_empty()), spans.last()) {
                set_tail(self.dom, last, Some(a));
            }
            let idx = el_index(self.dom, parent, p.elem).expect("elem is a child of its parent");
            for (i, span) in spans.into_iter().enumerate() {
                el_insert(self.dom, parent, idx + 1 + i, span);
            }
        }
    }
}

/// Runs [`split_into_sentences_for_tts_embed`] and converts its byte
/// offsets (into that function's own normalized text) into *character*
/// offsets, which are then valid against the original untransformed
/// text as well.
///
/// This conversion is load-bearing. Upstream indexes the original text
/// with positions computed on the normalized one, which works because
/// the normalization is character-length-preserving: `\r` and `\n`
/// become single spaces, and a run of N newlines becomes
/// `PARAGRAPH_SEPARATOR` plus N-1 spaces. It is *not* byte-length
/// preserving -- `PARAGRAPH_SEPARATOR` is `U+2029`, one character but
/// three UTF-8 bytes -- so using the byte offsets directly would
/// misalign every sentence after the first blank line in a document.
fn sentence_char_positions(text: &str, lang: &str) -> Vec<(usize, usize)> {
    let (normalized, positions) = split_into_sentences_for_tts_embed(text, lang);
    positions
        .into_iter()
        .map(|(byte_pos, byte_len)| {
            let char_pos = normalized[..byte_pos].chars().count();
            let char_len = normalized[byte_pos..byte_pos + byte_len].chars().count();
            (char_pos, char_len)
        })
        .collect()
}

/// Port of `mark_sentences_in_html`: wraps every sentence in `root` in a
/// `<span id="cttsw-N">` and returns them in document order.
///
/// # Disclosed upstream bug: a split element's tail text is dropped
///
/// When a sentence starts *and* ends inside one simple element's own
/// text, upstream clears the tail on both that element and the clone it
/// makes of it (`s.child.text, s.child.tail = before, None` followed by
/// `c.text, c.tail = sentence, None`). Any text that followed the
/// element is thereby lost from the tree -- for
/// `<p><b>one. Two</b> mid <i>three.</i></p>` the `" mid "` run simply
/// disappears. The chunk list still accounts for it, so the recorded
/// [`Sentence::text`] stays correct and only the document is damaged.
///
/// This is reproduced rather than repaired, matching this port's
/// standing treatment of upstream bugs (see issue #139, which tracks
/// the decision for the set of them as a whole). It was confirmed by
/// running upstream's own unmodified code against the same input, not
/// inferred from reading it.
///
/// # Disclosed divergence: comments
///
/// Upstream treats a comment as an ordinary child, so it builds a
/// `Parent` for one and -- because lxml exposes a comment's body as its
/// `.text` -- would try to wrap "sentences" inside it and then insert a
/// `<span>` into a comment, which lxml's content-only comment node does
/// not support. Here a comment's body is not a child text run at all, so
/// such a `Parent` simply has no chunks and contributes nothing; its
/// *tail* is still handled identically. The reachable behavior is the
/// same for any real document, minus an upstream crash hazard.
pub fn mark_sentences_in_html(dom: &mut Dom, root: NodeId, lang: &str, voice: &str) -> Vec<Sentence> {
    let base = calibre_utils::localization::canonicalize_lang(if lang.is_empty() { "eng" } else { lang })
        .unwrap_or_default();
    let root_lang = lang_for_elem(dom, root, &base);
    let root_lang = calibre_utils::localization::canonicalize_lang(if root_lang.is_empty() { "en" } else { &root_lang })
        .unwrap_or_default();

    let seen_ids: HashSet<String> = dom
        .preorder_elements(root)
        .into_iter()
        .filter_map(|e| dom.node(e).attrs.get("id").cloned())
        .collect();

    let mut marker = Marker {
        dom,
        id_counter: 1,
        seen_ids,
        out: Vec::new(),
        clone_order: Vec::new(),
        clones: HashMap::new(),
    };

    let bodies: Vec<NodeId> = element_children(marker.dom, root)
        .into_iter()
        .filter(|&e| marker.dom.tag(e) == Some("body"))
        .collect();
    let mut stack: Vec<Parent> = bodies
        .into_iter()
        .map(|b| Parent::new(marker.dom, b, &root_lang, voice, None))
        .collect();

    while let Some(mut p) = stack.pop() {
        let mut simple_allowed = true;
        let mut children_to_process: Vec<Parent> = Vec::new();
        for child in p.children.clone() {
            let child_voice = marker.dom.node(child).attrs.get(DATA_NAME).cloned().unwrap_or_default();
            let child_lang = lang_for_elem(marker.dom, child, &p.lang);
            let child_tag_name = marker.dom.tag(child).map(|t| t.to_ascii_lowercase()).unwrap_or_default();
            if simple_allowed
                && child_lang == p.lang
                && child_voice == p.voice
                && CONTINUED_TAG_NAMES.contains(&child_tag_name.as_str())
                && el_children(marker.dom, child).is_empty()
            {
                p.add_simple_child(marker.dom, child);
            } else if !IGNORED_TAG_NAMES.contains(&child_tag_name.as_str()) {
                simple_allowed = false;
                children_to_process.push(Parent::new(marker.dom, child, &p.lang, &p.voice, Some(child_lang)));
            }
            if simple_allowed {
                if let Some(text) = get_tail(marker.dom, child).and_then(|t| non_empty(&t).map(str::to_string)) {
                    p.add_tail(child, &text);
                }
            }
        }
        marker.commit(&mut p);
        for child in children_to_process.into_iter().rev() {
            stack.push(child);
        }
    }

    // Port of the trailing cleanup: drop clones (and sources) left with
    // nothing to say.
    for src in marker.clone_order.clone() {
        let mut group = marker.clones.get(&src).cloned().unwrap_or_default();
        group.push(src);
        for clone in group {
            let has_text = get_text(marker.dom, clone).map(|t| !t.is_empty()).unwrap_or(false);
            let has_tail = get_tail(marker.dom, clone).map(|t| !t.is_empty()).unwrap_or(false);
            let attrs = &marker.dom.node(clone).attrs;
            let has_id = attrs.get("id").map(|v| !v.is_empty()).unwrap_or(false);
            let has_name = attrs.get("name").map(|v| !v.is_empty()).unwrap_or(false);
            if !has_text && !has_tail && !has_id && !has_name && marker.dom.parent(clone).is_some() {
                el_remove(marker.dom, clone);
            }
        }
    }

    marker.out
}

/// Port of `unmark_sentences_in_html`: the inverse of
/// [`mark_sentences_in_html`].
pub fn unmark_sentences_in_html(dom: &mut Dom, root: NodeId) {
    let marked: Vec<NodeId> = dom
        .preorder_elements(root)
        .into_iter()
        .filter(|&e| dom.node(e).attrs.get("id").is_some_and(|i| i.starts_with(ID_PREFIX)))
        .collect();
    for x in marked {
        dom.node_mut(x).attrs.shift_remove("id");
        if dom.node(x).attrs.is_empty() && dom.tag(x) == Some("span") {
            unwrap_tag(dom, x);
        }
    }
}

/// Port of `html_transform_rules.unwrap_tag`: replaces `tag` with its
/// own contents. Upstream's `.text`/`.tail` merging into the preceding
/// sibling is automatic here -- those runs are ordinary children that
/// simply stay where they are -- so this only has to re-join runs that
/// end up adjacent, keeping lxml's invariant that two never are.
fn unwrap_tag(dom: &mut Dom, tag: NodeId) {
    let Some(parent) = dom.parent(tag) else { return };
    dom.remove_promoting_children(tag);
    merge_adjacent_text(dom, parent);
}

fn merge_adjacent_text(dom: &mut Dom, parent: NodeId) {
    let children = dom.node(parent).children.clone();
    let mut prev_text: Option<NodeId> = None;
    for c in children {
        if is_text(dom, c) {
            if let Some(prev) = prev_text {
                let extra = text_of(dom, c);
                if let NodeKind::Text(t) = &mut dom.node_mut(prev).kind {
                    t.push_str(&extra);
                }
                dom.detach(c);
                continue;
            }
            prev_text = Some(c);
        } else {
            prev_text = None;
        }
    }
}


// ---------------------------------------------------------------------
// SMIL media-overlay generation (issue #646)
// ---------------------------------------------------------------------

/// Port of `make_par`: appends one `<par>` (a `<text>`/`<audio>` pair)
/// to an existing SMIL `<seq>`, recording that `elem_id` in `html_href`
/// is read aloud by the `[pos, pos + duration)` clip of `audio_href`.
///
/// Two disclosed departures from a literal transcription:
///
/// * Upstream's `container` parameter is never read by the function
///   body (it uses only the module-level `EPUB`/`seconds_to_timestamp`
///   imports) -- dropped rather than carried forward unused.
/// * Every `.tail`/`.text` assignment in the real function exists
///   purely to keep lxml's pretty-printed indentation consistent after
///   inserting new elements; [`Xml`] represents whitespace as ordinary
///   sibling text nodes rather than out-of-band `.text`/`.tail`
///   properties precisely so that removal and insertion don't need this
///   bookkeeping (see the module's own scope note on `insert_element`).
///   None of it is reproduced here.
///
/// `par`/`text`/`audio` are created with no namespace, matching real
/// upstream: `seq.makeelement('par')` with a bare tag name creates a
/// namespace-*less* element regardless of `seq`'s own namespace --
/// lxml does not propagate an ancestor's default `xmlns` onto new
/// children created this way. Upstream's own `remove_embedded_tts`
/// corroborates this isn't accidental-looking to its author either: it
/// reads these elements back with `local-name() = "audio"` rather than
/// a namespace-qualified test.
///
/// The one attribute that must keep its namespace prefix to stay
/// EPUB3-conformant, `epub:textref`, is stored under that literal
/// qualified string. [`Xml`]'s own documented convention is that
/// attributes are stored unprefixed because every document it has
/// handled until now (OPF/NCX/OCF, DOCX) only ever has unprefixed
/// attributes; SMIL's one real namespaced attribute is a genuine, narrow
/// exception, not a violation of that convention for those other
/// documents.
pub fn make_par(xml: &mut Xml, seq: XmlNodeId, html_href: &str, audio_href: &str, elem_id: &str, pos: f64, duration: f64) {
    xml.set_attr(seq, "epub:textref", html_href);

    let par_number = xml.element_children(seq).len() + 1;
    let par = xml.new_element("par", None);
    xml.set_attr(par, "id", format!("par-{par_number}"));
    xml.insert_element(seq, par, None);

    let text = xml.new_element("text", None);
    xml.set_attr(text, "src", format!("{html_href}#{elem_id}"));
    xml.insert_element(par, text, None);

    let audio = xml.new_element("audio", None);
    xml.set_attr(audio, "src", audio_href);
    xml.set_attr(audio, "clipBegin", seconds_to_timestamp(pos));
    xml.set_attr(audio, "clipEnd", seconds_to_timestamp(pos + duration));
    xml.insert_element(par, audio, None);
}


#[cfg(test)]
mod tests {
    use super::*;

    // Every expectation below was cross-validated against upstream's own
    // `mark_sentences_in_html`/`unmark_sentences_in_html`, extracted
    // verbatim from `oeb/polish/tts.py` and run against a minimal
    // lxml-shaped tree: 54 fixtures, byte-identical structural dumps,
    // sentence lists and unmark round-trips, with all ten branches of
    // `wrap_sentence` confirmed exercised.

    fn marked(html: &str) -> (Dom, NodeId, Vec<Sentence>) {
        let mut dom = Dom::parse(html);
        let root = dom.find_first_tag_global("html").expect("fixture has an <html>");
        let sentences = mark_sentences_in_html(&mut dom, root, "en", "");
        (dom, root, sentences)
    }

    fn body_html(html: &str) -> String {
        let (dom, _, _) = marked(html);
        let body = dom.find_first_tag_global("body").unwrap();
        dom.serialize(body)
    }

    fn ids_and_texts(html: &str) -> Vec<(String, String)> {
        let (_, _, s) = marked(html);
        s.into_iter().map(|s| (s.elem_id, s.text)).collect()
    }

    #[test]
    fn wraps_each_sentence_of_a_plain_paragraph() {
        assert_eq!(
            body_html("<html><body><p>Hello there. Bye now.</p></body></html>"),
            "<body><p><span id=\"cttsw-1\">Hello there. </span><span id=\"cttsw-2\">Bye now.</span></p></body>"
        );
    }

    #[test]
    fn a_sentence_ending_in_a_childs_tail_wraps_the_whole_range() {
        assert_eq!(
            body_html("<html><body><p>Start <b>bold</b> tail. Next.</p></body></html>"),
            "<body><p><span id=\"cttsw-1\">Start <b>bold</b> tail. </span><span id=\"cttsw-2\">Next.</span></p></body>"
        );
    }

    #[test]
    fn a_sentence_ending_inside_a_later_childs_text_splits_that_child() {
        // Starts in a child's tail, ends mid-way through a later child's
        // own text, so that child is cloned and the range wrapped.
        assert_eq!(
            body_html("<html><body><p><b>a</b>start. Mid <i>inside. Rest</i> tail.</p></body></html>"),
            "<body><p>\
               <span id=\"cttsw-1\"><b>a</b>start. </span>\
               <span id=\"cttsw-2\">Mid <i>inside. </i></span>\
               <span id=\"cttsw-3\"><i>Rest</i> tail.</span>\
             </p></body>"
                .replace("               ", "")
                .replace("             ", "")
        );
    }

    #[test]
    fn a_sentence_spanning_from_one_childs_text_into_a_later_childs_text() {
        assert_eq!(
            body_html("<html><body><p><b>one. Two</b> mid <i>three. Four</i> end.</p></body></html>"),
            "<body><p>\
               <b id=\"cttsw-1\">one. </b>\
               <span id=\"cttsw-2\"><b>Two</b><i>three. </i></span>\
               <span id=\"cttsw-3\"><i>Four</i> end.</span>\
             </p></body>"
                .replace("               ", "")
                .replace("             ", "")
        );
    }

    #[test]
    fn a_whole_sentence_inside_one_element_marks_that_element_itself() {
        // No span is introduced: the element already delimits the
        // sentence exactly, so it just gets the id.
        assert_eq!(
            body_html("<html><body><p><b>one. two. three.</b></p></body></html>"),
            "<body><p><b id=\"cttsw-1\">one. two. three.</b></p></body>"
        );
    }

    #[test]
    fn an_elements_tail_text_is_dropped_when_a_sentence_ends_inside_its_text() {
        // Real upstream bug, reproduced deliberately -- see this
        // module's own docs. The `" mid "` run is lost from the tree...
        let html = body_html("<html><body><p><b>one. Two</b> mid <i>three. Four</i> end.</p></body></html>");
        assert!(!html.contains(" mid "), "{html}");
        // ...but is still accounted for in the recorded sentence text.
        let texts: Vec<String> = ids_and_texts("<html><body><p><b>one. Two</b> mid <i>three. Four</i> end.</p></body></html>")
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(texts, ["one. ", "Two mid three. ", "Four end."]);
    }

    #[test]
    fn an_elements_own_tail_is_wrapped_by_its_parents_pass() {
        assert_eq!(
            body_html("<html><body><div><p>Inside.</p>After the para. More after.</div></body></html>"),
            "<body><div><p><span id=\"cttsw-1\">Inside.</span></p>\
             <span id=\"cttsw-2\">After the para. </span>\
             <span id=\"cttsw-3\">More after.</span></div></body>"
                .replace("             ", "")
        );
    }

    #[test]
    fn ignored_tags_are_skipped_entirely() {
        let html = body_html("<html><body><p>a. <img src=\"x.png\"> b.</p><script>j.</script><p>c.</p></body></html>");
        assert!(html.contains("<script>j.</script>"), "{html}");
        assert!(html.contains("<img src=\"x.png\" />"), "{html}");
        assert_eq!(
            ids_and_texts("<html><body><p>a. <img src=\"x.png\"> b.</p><script>j.</script><p>c.</p></body></html>")
                .into_iter()
                .map(|(i, _)| i)
                .collect::<Vec<_>>(),
            ["cttsw-1", "cttsw-2"]
        );
    }

    #[test]
    fn a_lang_change_starts_its_own_run() {
        let (_, _, s) = marked("<html><body><p lang=\"en\">Hello. <span lang=\"fr\">Bonjour.</span> Bye.</p></body></html>");
        let langs: Vec<&str> = s.iter().map(|s| s.lang.as_str()).collect();
        assert_eq!(langs, ["eng", "fra", "eng"]);
    }

    #[test]
    fn the_voice_attribute_is_inherited_and_can_be_json() {
        let (_, _, plain) = marked("<html><body><p data-calibre-tts=\"bob\">One. Two.</p></body></html>");
        assert!(plain.iter().all(|s| s.voice == "bob"), "{plain:?}");
        let (_, _, json) = marked("<html><body><p data-calibre-tts='{\"voice\":\"amy\"}'>One. Two.</p></body></html>");
        assert!(json.iter().all(|s| s.voice == "amy"), "{json:?}");
    }

    #[test]
    fn a_skip_voice_suppresses_marking_entirely() {
        let (_, _, s) = marked("<html><body><p data-calibre-tts=\"__skip__\">One. Two.</p></body></html>");
        assert!(s.is_empty(), "{s:?}");
        assert_eq!(
            body_html("<html><body><p data-calibre-tts=\"__skip__\">One. Two.</p></body></html>"),
            "<body><p data-calibre-tts=\"__skip__\">One. Two.</p></body>"
        );
    }

    #[test]
    fn generated_ids_avoid_ones_the_document_already_uses() {
        let html = body_html("<html><body><p id=\"cttsw-1\">One. <b id=\"cttsw-2\">Two.</b> Three.</p></body></html>");
        // cttsw-1 and cttsw-2 were taken, so generation starts at 3.
        assert!(html.contains("id=\"cttsw-3\""), "{html}");
        assert!(html.contains("id=\"cttsw-4\""), "{html}");
        assert!(html.contains("id=\"cttsw-5\""), "{html}");
    }

    #[test]
    fn offsets_are_characters_so_multibyte_text_stays_aligned() {
        // Every offset here is past at least one multi-byte character;
        // byte-based arithmetic would slice these sentences apart.
        assert_eq!(
            ids_and_texts("<html><body><p>Café <b>ünd</b> Straße. Nächster.</p></body></html>")
                .into_iter()
                .map(|(_, t)| t)
                .collect::<Vec<_>>(),
            ["Café ünd Straße. ", "Nächster."]
        );
    }

    #[test]
    fn a_blank_line_becomes_a_paragraph_separator_without_shifting_offsets() {
        // The normalization swaps a run of newlines for U+2029 plus
        // spaces: same character count, three more bytes. Sentence text
        // sliced from the *original* must therefore keep the newlines.
        let texts: Vec<String> = ids_and_texts("<html><body><p>Café one.\n\nCafé two. Drei.</p></body></html>")
            .into_iter()
            .map(|(_, t)| t)
            .collect();
        assert_eq!(texts, ["Café one.\n", "\nCafé two. ", "Drei."]);
    }

    #[test]
    fn unmark_restores_the_original_document() {
        for fixture in [
            "<html><body><p>Hello there. Bye now.</p></body></html>",
            "<html><body><p>Start <b>bold</b> tail. Next.</p></body></html>",
            "<html><body><div><p>One.</p><p>Two.</p></div></body></html>",
            "<html><body><p>Café <b>ünd</b> Straße. Nächster.</p></body></html>",
            "<html><body><div><p>Inside.</p>After the para. More after.</div></body></html>",
        ] {
            let original = {
                let dom = Dom::parse(fixture);
                let body = dom.find_first_tag_global("body").unwrap();
                dom.serialize(body)
            };
            let mut dom = Dom::parse(fixture);
            let root = dom.find_first_tag_global("html").unwrap();
            mark_sentences_in_html(&mut dom, root, "en", "");
            unmark_sentences_in_html(&mut dom, root);
            let body = dom.find_first_tag_global("body").unwrap();
            assert_eq!(dom.serialize(body), original, "round trip failed for {fixture}");
        }
    }

    #[test]
    fn unmark_leaves_unrelated_ids_and_spans_alone() {
        let mut dom = Dom::parse("<html><body><p><span id=\"keep\">x</span><span class=\"c\">y</span></p></body></html>");
        let root = dom.find_first_tag_global("html").unwrap();
        unmark_sentences_in_html(&mut dom, root);
        let body = dom.find_first_tag_global("body").unwrap();
        assert_eq!(
            dom.serialize(body),
            "<body><p><span id=\"keep\">x</span><span class=\"c\">y</span></p></body>"
        );
    }

    #[test]
    fn the_lxml_view_reads_and_writes_leading_and_trailing_runs() {
        let mut dom = Dom::parse("<html><body><p>lead<b>x</b>tail</p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let b = dom.find_first_tag_global("b").unwrap();
        assert_eq!(get_text(&dom, p).as_deref(), Some("lead"));
        assert_eq!(get_tail(&dom, b).as_deref(), Some("tail"));
        assert_eq!(el_index(&dom, p, b), Some(0));

        set_tail(&mut dom, b, Some("TAIL"));
        set_text(&mut dom, p, Some("LEAD"));
        assert_eq!(dom.serialize(p), "<p>LEAD<b>x</b>TAIL</p>");

        // An empty assignment removes the run, as lxml's falsy '' does.
        set_tail(&mut dom, b, Some(""));
        set_text(&mut dom, p, None);
        assert_eq!(dom.serialize(p), "<p><b>x</b></p>");
    }

    // ---- SMIL media-overlay generation (issue #646) --------------------

    /// The exact template `embed_tts` writes before calling `make_par`
    /// (see `old_src/.../tts.py`'s `embed_tts`), minus the `X` marker
    /// text upstream immediately truncates away.
    fn smil_fixture() -> (Xml, XmlNodeId) {
        let xml = Xml::parse(
            r#"<smil xmlns="http://www.w3.org/ns/SMIL" xmlns:epub="http://www.idpf.org/2007/ops" version="3.0">
  <body>
    <seq id="generated-by-calibre">
    </seq>
  </body>
</smil>"#,
        )
        .unwrap();
        let smil = xml.root_element().unwrap();
        let body = xml.element_children(smil)[0];
        let seq = xml.element_children(body)[0];
        (xml, seq)
    }

    #[test]
    fn make_par_appends_a_text_audio_pair() {
        let (mut xml, seq) = smil_fixture();
        make_par(&mut xml, seq, "chapter1.xhtml", "audio.m4a", "cttsw-1", 1.5, 2.25);

        let pars = xml.element_children(seq);
        assert_eq!(pars.len(), 1);
        let par = pars[0];
        assert_eq!(xml.local_name(par), Some("par"));
        assert_eq!(xml.namespace(par), None, "par must have no namespace, matching real upstream");
        assert_eq!(xml.get_attr(par, "id"), Some("par-1"));

        let children = xml.element_children(par);
        assert_eq!(children.len(), 2);
        let (text, audio) = (children[0], children[1]);
        assert_eq!(xml.local_name(text), Some("text"));
        assert_eq!(xml.get_attr(text, "src"), Some("chapter1.xhtml#cttsw-1"));
        assert_eq!(xml.local_name(audio), Some("audio"));
        assert_eq!(xml.get_attr(audio, "src"), Some("audio.m4a"));
        assert_eq!(xml.get_attr(audio, "clipBegin"), Some("00:00:01.5"));
        assert_eq!(xml.get_attr(audio, "clipEnd"), Some("00:00:03.75"));
    }

    #[test]
    fn make_par_sets_the_seqs_textref_to_the_html_href() {
        let (mut xml, seq) = smil_fixture();
        make_par(&mut xml, seq, "chapter1.xhtml", "audio.m4a", "cttsw-1", 0.0, 1.0);
        assert_eq!(xml.get_attr(seq, "epub:textref"), Some("chapter1.xhtml"));
    }

    #[test]
    fn successive_calls_number_pars_sequentially_and_keep_them_in_order() {
        let (mut xml, seq) = smil_fixture();
        make_par(&mut xml, seq, "c1.xhtml", "a.m4a", "cttsw-1", 0.0, 1.0);
        make_par(&mut xml, seq, "c1.xhtml", "a.m4a", "cttsw-2", 1.0, 1.0);
        make_par(&mut xml, seq, "c1.xhtml", "a.m4a", "cttsw-3", 2.0, 1.0);

        let pars = xml.element_children(seq);
        assert_eq!(pars.len(), 3);
        let ids: Vec<&str> = pars.iter().map(|&p| xml.get_attr(p, "id").unwrap()).collect();
        assert_eq!(ids, ["par-1", "par-2", "par-3"]);
        let elem_ids: Vec<&str> = pars
            .iter()
            .map(|&p| {
                let text = xml.element_children(p)[0];
                let src = xml.get_attr(text, "src").unwrap();
                src.rsplit('#').next().unwrap()
            })
            .collect();
        assert_eq!(elem_ids, ["cttsw-1", "cttsw-2", "cttsw-3"]);
    }

    #[test]
    fn the_epub_textref_prefix_survives_serialization() {
        let (mut xml, seq) = smil_fixture();
        make_par(&mut xml, seq, "chapter1.xhtml", "audio.m4a", "cttsw-1", 0.0, 1.0);
        let out = String::from_utf8(xml.serialize()).unwrap();
        assert!(out.contains("epub:textref=\"chapter1.xhtml\""), "{out}");
        assert!(out.contains("<par id=\"par-1\">"), "{out}");
        assert!(out.contains("<text src=\"chapter1.xhtml#cttsw-1\" />"), "{out}");
        assert!(out.contains("<audio src=\"audio.m4a\" clipBegin=\"00:00:00\" clipEnd=\"00:00:01\" />"), "{out}");
    }

    #[test]
    fn clip_timestamps_use_the_real_hms_format() {
        let (mut xml, seq) = smil_fixture();
        make_par(&mut xml, seq, "c.xhtml", "a.m4a", "s1", 3661.0, 5.0);
        let par = xml.element_children(seq)[0];
        let audio = xml.element_children(par)[1];
        assert_eq!(xml.get_attr(audio, "clipBegin"), Some("01:01:01"));
        assert_eq!(xml.get_attr(audio, "clipEnd"), Some("01:01:06"));
    }}
