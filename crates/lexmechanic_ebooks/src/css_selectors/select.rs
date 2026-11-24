//! Port of `css_selectors.select` (issue #453, closes the #85 tracking
//! issue, depends on #452's AST): the selector-to-tree matching
//! engine -- combinators, attribute matchers, structural pseudo-classes,
//! `:lang()`, negation.
//!
//! # A tree-agnostic [`Element`] trait, not `lxml`
//!
//! Real upstream's `Select` is written directly against `lxml`'s
//! `_Element` API (`.tag`, `.attrib`, `.iterchildren()`,
//! `.itersiblings()`, `.getparent()`, `.index()`, `len()`). This port
//! defines its own minimal [`Element`] trait instead (per this issue's
//! own filing: "generically over any tree implementation ... not tied
//! to `lxml` specifically") and implements it for [`DomElement`], a
//! thin wrapper over [`crate::dom::Dom`] -- the real, already-ported
//! tree type this port's own real callers use. A second implementation
//! for [`crate::xmltree::Xml`] would be straightforward if a real
//! caller ever needs one, following [`DomElement`]'s shape.
//!
//! # Not tied to `crate::css::matcher::Element` either
//!
//! [`crate::css::selector`]/[`crate::css::matcher`] (issue #164) already
//! define a narrower `Element` trait for a deliberately scoped selector
//! engine. This module's [`Element`] is a separate, richer trait (adds
//! `children`/`attrs`/`is_empty_of_content`, and requires `Eq + Hash`
//! for set-based matching) -- the two modules serve different real
//! callers and are not meant to converge.
//!
//! # Dispatch by `match`, not upstream's string-keyed `dispatch_map`
//!
//! Real `Select.dispatch_map` exists so `trace=True`/a custom
//! `dispatch_map` argument can wrap or replace individual `select_*`
//! functions -- a debugging/extensibility feature nothing in this port
//! needs. Every `select_*` function here is instead an ordinary method
//! or `match` arm dispatched on the real [`crate::css_selectors::parser::Node`]
//! variant or pseudo-class/function name directly.
//!
//! # Eager indices, not upstream's lazily-built `@property` caches
//!
//! Real `Select._element_map`/`_id_map`/etc. are built on first access
//! and memoized (`self._element_map = None` until read). Ported as
//! plain fields built once in [`Select::new`] -- same observable
//! content, simpler than an interior-mutability memoization wrapper for
//! a case where every real caller ends up needing most of the indices
//! anyway (there's no real caller yet that only ever queries one).
//!
//! # Document order via a final filter pass, not per-step generator order
//!
//! Real upstream's public contract (`Select.__call__`'s own docstring)
//! is just "tags are returned in document order", achieved as an
//! emergent property of chaining several generators whose own internal
//! iteration order happens to derive from doc-order-built indices. This
//! port computes each (sub)selector's match as a `HashSet<E>` (no
//! ordering) and only sorts into document order once, at the very top
//! ([`Select::matching`]), by filtering the real document-order element
//! list down to the matched set. Simpler to reason about, and the only
//! order any real caller ever observes.
//!
//! # Two real, disclosed findings from reading the actual source
//!
//! - **A real, uncontrolled-upstream-crash converted to a controlled
//!   error**: real `Select.attribute_operator_mapping` has no entry for
//!   `!=`, even though `parser.py`'s own grammar happily parses
//!   `[attr!=value]` (its own operator-char set includes `!`) -- so a
//!   real selector using `!=` would raise an uncaught `KeyError` in
//!   upstream, not a controlled `ExpressionError`. Ported as a real,
//!   controlled [`SelectorError::Expression`] instead (see
//!   [`Select::eval_attrib`]), per this project's established
//!   convention of converting genuine uncontrolled crashes into
//!   controlled errors while still replicating deliberate-looking
//!   quirks bug-for-bug (see the next point).
//! - **A real, faithfully-replicated bug in `lang_map` construction**:
//!   real `Select.lang_map`'s own loop --
//!   `for attr in ('{...}lang', 'lang'): lang = tag.get(attr)` --
//!   unconditionally overwrites `lang` on every iteration, so the
//!   plain `lang` attribute's value always wins over `xml:lang`,
//!   regardless of which one is actually set (a real element with only
//!   `xml:lang` and no plain `lang` attribute would resolve to `None`,
//!   silently dropping its declared language). This looks like a real
//!   upstream bug -- almost certainly meant to prefer `xml:lang` and
//!   fall back to `lang` -- but per this project's "replicate observed
//!   upstream bugs bug-for-bug" convention (see #654's `uniqify_name`),
//!   [`build_lang_map`] only ever reads the plain `lang` attribute,
//!   matching real upstream's actual (buggy) observable behavior, not
//!   its likely intent. Usually invisible in practice since real EPUB
//!   content typically sets both attributes to the same value.
//!
//! # Scoped narrowing: no XML-namespace (Clark notation) folding
//!
//! Real `Select.invalidate_caches`/`attrib_map`/etc. strip a
//! `{namespace-uri}` prefix from tag/attribute names before matching,
//! when the root element's own tag contains one (`'{' in self.root.tag`,
//! lxml's Clark-notation representation of an XML-namespaced name).
//! [`crate::dom::Dom`] (this module's one real target tree) doesn't
//! represent namespaces that way at all -- its tag/attribute names are
//! plain strings -- so this narrowing is a real gap only if some future
//! caller needs namespace-aware matching against a genuinely
//! namespaced tree; not implemented since no such caller exists yet.

use std::collections::{HashMap, HashSet};

use crate::css_selectors::errors::SelectorError;
use crate::css_selectors::parser::{self, FunctionSel, Node, PseudoElement, Selector, TokenKind};
use crate::dom::{Dom, NodeId, NodeKind};

/// Port of `INAPPROPRIATE_PSEUDO_CLASSES`.
pub const INAPPROPRIATE_PSEUDO_CLASSES: &[&str] =
    &["active", "after", "disabled", "visited", "link", "before", "focus", "first-letter", "enabled", "first-line", "hover", "checked", "target"];

// ---------------------------------------------------------------------
// Tree Integration
// ---------------------------------------------------------------------

/// The tree surface this module's matching engine needs -- see the
/// module doc for why this is a separate, richer trait from
/// [`crate::css::matcher::Element`].
pub trait Element: Copy + Eq + std::hash::Hash {
    fn tag_name(&self) -> Option<String>;
    fn get_attr(&self, name: &str) -> Option<String>;
    /// Every attribute as `(name, value)` pairs -- port of `tag.attrib`.
    fn attrs(&self) -> Vec<(String, String)>;
    fn parent(&self) -> Option<Self>;
    /// Every child *element* (not text/comment nodes), in document
    /// order -- port of `tag.iterchildren('*')`, and (via `len()`/
    /// `.index()`) the basis for every sibling-position computation.
    fn children(&self) -> Vec<Self>;
    /// Port of `Select.is_empty`: no child elements and no text content
    /// of its own (including whitespace-only text -- real upstream's
    /// own `not elem.text` is a plain truthiness check, not a
    /// stripped/trimmed one, matching the CSS `:empty` spec itself).
    fn is_empty_of_content(&self) -> bool;
}

/// An [`Element`] over [`Dom`] (XHTML content documents) -- the one
/// real tree implementation this port targets.
#[derive(Clone, Copy)]
pub struct DomElement<'a> {
    pub dom: &'a Dom,
    pub id: NodeId,
}

/// `Dom` itself doesn't derive `Debug`, so this prints just the node
/// id -- enough for test failure messages, the only real consumer.
impl std::fmt::Debug for DomElement<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DomElement({})", self.id)
    }
}

impl PartialEq for DomElement<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for DomElement<'_> {}
impl std::hash::Hash for DomElement<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl<'a> Element for DomElement<'a> {
    fn tag_name(&self) -> Option<String> {
        self.dom.tag(self.id).map(|s| s.to_string())
    }

    fn get_attr(&self, name: &str) -> Option<String> {
        self.dom.node(self.id).attrs.get(name).cloned()
    }

    fn attrs(&self) -> Vec<(String, String)> {
        self.dom.node(self.id).attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    fn parent(&self) -> Option<Self> {
        self.dom.parent(self.id).map(|id| DomElement { dom: self.dom, id })
    }

    fn children(&self) -> Vec<Self> {
        self.dom
            .children(self.id)
            .into_iter()
            .filter(|&c| matches!(self.dom.node(c).kind, NodeKind::Element(_)))
            .map(|id| DomElement { dom: self.dom, id })
            .collect()
    }

    fn is_empty_of_content(&self) -> bool {
        !self.dom.node(self.id).children.iter().any(|&c| match &self.dom.node(c).kind {
            NodeKind::Element(_) => true,
            NodeKind::Text(t) => !t.is_empty(),
            _ => false,
        })
    }
}

/// Every `Element` node in `dom`, in document order -- port of what
/// `css_selectors.Select(root)` builds its caches from.
pub fn dom_elements(dom: &Dom) -> Vec<DomElement<'_>> {
    dom.preorder_elements(dom.root).into_iter().map(|id| DomElement { dom, id }).collect()
}

// ---------------------------------------------------------------------
// Tree-structural helpers (pure functions of `Element`, no index needed)
// ---------------------------------------------------------------------

fn descendants_of<E: Element>(e: E) -> Vec<E> {
    let mut out = Vec::new();
    let mut stack = e.children();
    while let Some(c) = stack.pop() {
        let mut grandchildren = c.children();
        out.push(c);
        stack.append(&mut grandchildren);
    }
    out
}

fn following_siblings<E: Element>(e: E) -> Vec<E> {
    let Some(parent) = e.parent() else { return Vec::new() };
    let siblings = parent.children();
    match siblings.iter().position(|&s| s == e) {
        Some(pos) => siblings[pos + 1..].to_vec(),
        None => Vec::new(),
    }
}

fn next_element_sibling<E: Element>(e: E) -> Option<E> {
    following_siblings(e).into_iter().next()
}

/// Port of `Select.sibling_count`. Real upstream intersects
/// `cache.element_map[tag]` (a document-wide index) with the local
/// sibling set for `same_type`; since siblings are already a subset of
/// the whole document, filtering `parent.children()` by tag name
/// directly gives the identical count with no document-wide index
/// needed -- a real simplification, not a behavior change.
fn sibling_count<E: Element>(child: E, before: bool, same_type: bool) -> Result<usize, ()> {
    let parent = child.parent().ok_or(())?;
    let siblings = parent.children();
    let filtered: Vec<E> = if same_type {
        let tag = child.tag_name();
        siblings.into_iter().filter(|s| s.tag_name() == tag).collect()
    } else {
        siblings
    };
    let pos = filtered.iter().position(|&s| s == child).ok_or(())?;
    Ok(if before { pos } else { filtered.len() - pos - 1 })
}

/// Port of `Select.all_sibling_count`.
fn all_sibling_count<E: Element>(child: E, same_type: bool) -> Result<usize, ()> {
    let parent = child.parent().ok_or(())?;
    let siblings = parent.children();
    if same_type {
        let tag = child.tag_name();
        Ok(siblings.iter().filter(|s| s.tag_name() == tag).count() - 1)
    } else {
        Ok(siblings.len() - 1)
    }
}

/// Port of `select_nth_child`/`select_nth_last_child`/
/// `select_nth_of_type`/`select_nth_last_of_type`'s shared `An+B` test.
fn nth_matches(count_before: i64, a: i64, b: i64) -> bool {
    let num = count_before + 1;
    if a == 0 {
        num == b
    } else {
        let n = (num - b) as f64 / a as f64;
        n.fract() == 0.0 && n > -1.0
    }
}

// ---------------------------------------------------------------------
// normalize_language_tag
// ---------------------------------------------------------------------

/// Port of the real regex `re.sub(r'-([a-zA-Z0-9])-', r'-\1_', tag)`:
/// marks a BCP47 singleton subtag (a lone alphanumeric between two
/// hyphens) by gluing it to what follows with `_` instead of `-`, so it
/// isn't split into a separately combinable subtag below. Non-
/// overlapping, matching `re.sub`'s own scan behavior.
fn mark_singletons(tag: &str) -> String {
    let chars: Vec<char> = tag.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '-' && i + 2 < chars.len() && chars[i + 1].is_ascii_alphanumeric() && chars[i + 2] == '-' {
            out.push('-');
            out.push(chars[i + 1]);
            out.push('_');
            i += 3;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

/// Port of Python's `itertools.combinations` for this one call site:
/// every length-`k` sub-sequence of `items`, preserving relative order.
fn combinations<T: Clone>(items: &[T], k: usize) -> Vec<Vec<T>> {
    if k == 0 {
        return vec![vec![]];
    }
    let Some((first, rest)) = items.split_first() else { return Vec::new() };
    let mut out: Vec<Vec<T>> = combinations(rest, k - 1)
        .into_iter()
        .map(|mut tail| {
            let mut v = vec![first.clone()];
            v.append(&mut tail);
            v
        })
        .collect();
    out.extend(combinations(rest, k));
    out
}

/// Port of `normalize_language_tag`.
pub fn normalize_language_tag(tag: &str) -> HashSet<String> {
    let lowered = tag.to_ascii_lowercase().replace('_', "-");
    let marked = mark_singletons(&lowered);
    let subtags: Vec<String> = marked.split('-').map(|s| s.replace('_', "-")).collect();
    let Some((base, rest)) = subtags.split_first() else { return HashSet::new() };
    let mut taglist: HashSet<String> = HashSet::new();
    taglist.insert(base.clone());
    for n in (1..=rest.len()).rev() {
        for combo in combinations(rest, n) {
            let mut parts = vec![base.clone()];
            parts.extend(combo);
            taglist.insert(parts.join("-"));
        }
    }
    taglist
}

// ---------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------

/// Port of `Select`: a per-document index plus the real matching logic.
pub struct Select<E: Element> {
    root: E,
    all_elements: Vec<E>,
    ignore_inappropriate: bool,
    element_map: HashMap<String, HashSet<E>>,
    id_map: HashMap<String, HashSet<E>>,
    class_map: HashMap<String, HashSet<E>>,
    attrib_map: HashMap<String, HashMap<String, HashSet<E>>>,
    attrib_space_map: HashMap<String, HashMap<String, HashSet<E>>>,
    lang_map: HashMap<String, HashSet<E>>,
}

fn build_lang_map<E: Element>(all_elements: &[E], default_lang: Option<&str>) -> HashMap<String, HashSet<E>> {
    let default_langs = default_lang.map(normalize_language_tag);
    let mut lmap: HashMap<E, HashSet<String>> = HashMap::new();
    if let Some(dl) = &default_langs {
        for &e in all_elements {
            lmap.insert(e, dl.clone());
        }
    }
    for &tag in all_elements {
        // Real upstream bug, faithfully replicated -- see the module doc.
        let lang = tag.get_attr("lang");
        if let Some(lang) = lang {
            if !lang.is_empty() {
                let normalized = normalize_language_tag(&lang);
                lmap.insert(tag, normalized.clone());
                for dtag in descendants_of(tag) {
                    lmap.insert(dtag, normalized.clone());
                }
            }
        }
    }
    let mut lm: HashMap<String, HashSet<E>> = HashMap::new();
    for (tag, langs) in lmap {
        for lang in langs {
            lm.entry(lang).or_default().insert(tag);
        }
    }
    lm
}

fn intersect<E: Element>(base: HashSet<E>, items: &HashSet<E>) -> HashSet<E> {
    base.into_iter().filter(|e| items.contains(e)).collect()
}

impl<E: Element> Select<E> {
    /// Port of `Select.__init__` (+ its lazily-built caches, made
    /// eager -- see the module doc). `all_elements` must be every
    /// `Element` node in the document, in document order (e.g.
    /// [`dom_elements`]).
    pub fn new(root: E, all_elements: Vec<E>, default_lang: Option<&str>, ignore_inappropriate_pseudo_classes: bool) -> Self {
        let mut element_map: HashMap<String, HashSet<E>> = HashMap::new();
        let mut id_map: HashMap<String, HashSet<E>> = HashMap::new();
        let mut class_map: HashMap<String, HashSet<E>> = HashMap::new();
        let mut attrib_map: HashMap<String, HashMap<String, HashSet<E>>> = HashMap::new();
        let mut attrib_space_map: HashMap<String, HashMap<String, HashSet<E>>> = HashMap::new();

        for &elem in &all_elements {
            if let Some(tag) = elem.tag_name() {
                element_map.entry(tag.to_ascii_lowercase()).or_default().insert(elem);
            }
            for (attr, val) in elem.attrs() {
                let attr_lower = attr.to_ascii_lowercase();
                if attr_lower == "id" {
                    id_map.entry(val.to_ascii_lowercase()).or_default().insert(elem);
                }
                if attr_lower == "class" {
                    for cls in val.split_whitespace() {
                        class_map.entry(cls.to_ascii_lowercase()).or_default().insert(elem);
                    }
                }
                attrib_map.entry(attr_lower.clone()).or_default().entry(val.clone()).or_default().insert(elem);
                for token in val.split_whitespace() {
                    attrib_space_map.entry(attr_lower.clone()).or_default().entry(token.to_string()).or_default().insert(elem);
                }
            }
        }

        let lang_map = build_lang_map(&all_elements, default_lang);

        Select { root, all_elements, ignore_inappropriate: ignore_inappropriate_pseudo_classes, element_map, id_map, class_map, attrib_map, attrib_space_map, lang_map }
    }

    /// Port of `tuple(select(selector_text))`: every element matching
    /// any selector in `selectors`, in document order.
    pub fn matching(&self, selectors: &[Selector]) -> Result<Vec<E>, SelectorError> {
        let mut matched: HashSet<E> = HashSet::new();
        for sel in selectors {
            matched.extend(self.eval_selector(sel)?);
        }
        Ok(self.all_elements.iter().copied().filter(|e| matched.contains(e)).collect())
    }

    /// Port of `Select.has_matches`.
    pub fn has_matches(&self, selectors: &[Selector]) -> Result<bool, SelectorError> {
        for sel in selectors {
            if !self.eval_selector(sel)?.is_empty() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Port of `select_selector`.
    fn eval_selector(&self, selector: &Selector) -> Result<HashSet<E>, SelectorError> {
        match &selector.pseudo_element {
            None => self.eval(&selector.parsed_tree),
            Some(PseudoElement::Functional(fpe)) => Err(SelectorError::Expression(format!("The pseudo-element ::{} is not supported", fpe.name))),
            Some(PseudoElement::Ident(name)) => self.eval_pseudo(&selector.parsed_tree, name),
        }
    }

    /// Port of every real `select_*(cache, node)` function, unified by
    /// `match` on [`Node`] instead of `type(node).__name__` dispatch.
    fn eval(&self, node: &Node) -> Result<HashSet<E>, SelectorError> {
        match node {
            Node::Element(e) => Ok(self.eval_element(e)),
            Node::Class(sel, name) => {
                let base = self.eval(sel)?;
                let items = self.class_map.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default();
                Ok(intersect(base, &items))
            }
            Node::IdHash(sel, id) => {
                let base = self.eval(sel)?;
                let items = self.id_map.get(&id.to_ascii_lowercase()).cloned().unwrap_or_default();
                Ok(intersect(base, &items))
            }
            Node::Attrib(sel, attr) => {
                let base = self.eval(sel)?;
                let items = self.eval_attrib(attr)?;
                Ok(intersect(base, &items))
            }
            Node::Pseudo(sel, ident) => self.eval_pseudo(sel, ident),
            Node::Function(sel, func) => self.eval_function(sel, func),
            Node::Negation(sel, sub) => {
                let base = self.eval(sel)?;
                let exclude = self.eval(sub)?;
                Ok(base.difference(&exclude).copied().collect())
            }
            Node::Combined(sel, combinator, sub) => {
                let left = self.eval(sel)?;
                let right = self.eval(sub)?;
                Ok(self.combine(&left, &right, *combinator))
            }
        }
    }

    /// Port of `select_element`.
    fn eval_element(&self, e: &parser::ElementSel) -> HashSet<E> {
        match &e.element {
            None => self.all_elements.iter().copied().collect(),
            Some(name) if name == "*" => self.all_elements.iter().copied().collect(),
            Some(name) => self.element_map.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default(),
        }
    }

    /// Port of `select_attrib` + the `select_exists`/`select_equals`/
    /// `select_includes`/`select_dashmatch`/`select_prefixmatch`/
    /// `select_suffixmatch`/`select_substringmatch` dispatch. See the
    /// module doc for the `!=` uncontrolled-crash-to-controlled-error
    /// fix (the `other =>` arm below).
    fn eval_attrib(&self, attr: &parser::AttribSel) -> Result<HashSet<E>, SelectorError> {
        let name = attr.attrib.to_ascii_lowercase();
        let value = attr.value.as_deref().unwrap_or("");
        let items = match attr.operator.as_str() {
            "exists" => self.attrib_map.get(&name).map(|m| m.values().flatten().copied().collect()).unwrap_or_default(),
            "=" => self.attrib_map.get(&name).and_then(|m| m.get(value)).cloned().unwrap_or_default(),
            "~=" => {
                if value.is_empty() || value.chars().any(char::is_whitespace) {
                    HashSet::new()
                } else {
                    self.attrib_space_map.get(&name).and_then(|m| m.get(value)).cloned().unwrap_or_default()
                }
            }
            "|=" => {
                let mut out = HashSet::new();
                if !value.is_empty() {
                    if let Some(m) = self.attrib_map.get(&name) {
                        let prefix = format!("{value}-");
                        for (val, elems) in m {
                            if val == value || val.starts_with(&prefix) {
                                out.extend(elems.iter().copied());
                            }
                        }
                    }
                }
                out
            }
            "^=" => self.attrib_map_filter(&name, value, |val, v| val.starts_with(v)),
            "$=" => self.attrib_map_filter(&name, value, |val, v| val.ends_with(v)),
            "*=" => self.attrib_map_filter(&name, value, |val, v| val.contains(v)),
            other => {
                return Err(SelectorError::Expression(format!(
                    "the '{other}' attribute operator is grammatically valid but not implemented by the matching engine \
                     (a real upstream gap: `attribute_operator_mapping` has no entry for it either, which raises an \
                     uncontrolled KeyError there instead of this controlled error)"
                )));
            }
        };
        Ok(items)
    }

    fn attrib_map_filter(&self, name: &str, value: &str, pred: impl Fn(&str, &str) -> bool) -> HashSet<E> {
        let mut out = HashSet::new();
        if !value.is_empty() {
            if let Some(m) = self.attrib_map.get(name) {
                for (val, elems) in m {
                    if pred(val, value) {
                        out.extend(elems.iter().copied());
                    }
                }
            }
        }
        out
    }

    /// Port of `select_pseudo`/`select_selector`'s shared
    /// `get_func_for_pseudo` dispatch (both real call sites route
    /// through the identical logic, including the `:root` shortcut).
    fn eval_pseudo(&self, inner: &Node, ident: &str) -> Result<HashSet<E>, SelectorError> {
        let ident_lower = ident.to_ascii_lowercase();
        if ident_lower == "root" {
            // Real quirk, faithfully kept: `:root` ignores `inner`
            // entirely and always yields just the document root, even
            // for something like `div:root`.
            let mut s = HashSet::new();
            s.insert(self.root);
            return Ok(s);
        }
        let base = self.eval(inner)?;
        self.filter_by_pseudo_ident(base, &ident_lower)
    }

    fn filter_by_pseudo_ident(&self, base: HashSet<E>, ident: &str) -> Result<HashSet<E>, SelectorError> {
        let pred: fn(E) -> bool = match ident {
            "first-child" => |e| sibling_count(e, true, false) == Ok(0),
            "last-child" => |e| sibling_count(e, false, false) == Ok(0),
            "only-child" => |e| all_sibling_count(e, false) == Ok(0),
            "first-of-type" => |e| sibling_count(e, true, true) == Ok(0),
            "last-of-type" => |e| sibling_count(e, false, true) == Ok(0),
            "only-of-type" => |e| all_sibling_count(e, true) == Ok(0),
            "empty" => |e| e.is_empty_of_content(),
            other => {
                if self.ignore_inappropriate && INAPPROPRIATE_PSEUDO_CLASSES.contains(&other) {
                    return Ok(base);
                }
                return Err(SelectorError::Expression(format!("The pseudo-class :{other} is not supported")));
            }
        };
        Ok(base.into_iter().filter(|&e| pred(e)).collect())
    }

    /// Port of `select_function` + `select_lang`/`select_nth_*`.
    fn eval_function(&self, sel: &Node, func: &FunctionSel) -> Result<HashSet<E>, SelectorError> {
        match func.name.as_str() {
            "lang" => {
                let items = self.select_lang(func)?;
                let base = self.eval(sel)?;
                Ok(intersect(base, &items))
            }
            "nth-child" | "nth-last-child" | "nth-of-type" | "nth-last-of-type" => {
                let (a, b) = func.parsed_arguments()?;
                let (before, same_type) = match func.name.as_str() {
                    "nth-child" => (true, false),
                    "nth-last-child" => (false, false),
                    "nth-of-type" => (true, true),
                    _ => (false, true),
                };
                let base = self.eval(sel)?;
                Ok(base
                    .into_iter()
                    .filter(|&e| match sibling_count(e, before, same_type) {
                        Ok(count) => nth_matches(count as i64, a, b),
                        Err(()) => false,
                    })
                    .collect())
            }
            other => Err(SelectorError::Expression(format!("The pseudo-class :{other}() is unknown"))),
        }
    }

    /// Port of `select_lang`.
    fn select_lang(&self, func: &FunctionSel) -> Result<HashSet<E>, SelectorError> {
        let types = func.argument_types();
        if types != [TokenKind::String] && types != [TokenKind::Ident] {
            return Err(SelectorError::Expression(format!("Expected a single string or ident for :lang(), got {:?}", func.arguments)));
        }
        let lang = &func.arguments[0].value;
        if lang.is_empty() {
            return Ok(HashSet::new());
        }
        let lang = lang.to_ascii_lowercase();
        let prefix = format!("{lang}-");
        let mut out = HashSet::new();
        for (tlang, elems) in &self.lang_map {
            if *tlang == lang || tlang.starts_with(&prefix) {
                out.extend(elems.iter().copied());
            }
        }
        Ok(out)
    }

    /// Port of `select_combinedselector` + `select_descendant`/
    /// `select_child`/`select_direct_adjacent`/`select_indirect_adjacent`.
    fn combine(&self, left: &HashSet<E>, right: &HashSet<E>, combinator: char) -> HashSet<E> {
        let mut out = HashSet::new();
        match combinator {
            ' ' => {
                for &l in left {
                    for d in descendants_of(l) {
                        if right.contains(&d) {
                            out.insert(d);
                        }
                    }
                }
            }
            '>' => {
                for &l in left {
                    for c in l.children() {
                        if right.contains(&c) {
                            out.insert(c);
                        }
                    }
                }
            }
            '+' => {
                for &l in left {
                    if let Some(sib) = next_element_sibling(l) {
                        if right.contains(&sib) {
                            out.insert(sib);
                        }
                    }
                }
            }
            '~' => {
                for &l in left {
                    for sib in following_siblings(l) {
                        if right.contains(&sib) {
                            out.insert(sib);
                        }
                    }
                }
            }
            _ => unreachable!("crate::css_selectors::parser only ever produces ' '/'>'/'+'/'~' combinators"),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_selectors::parser::parse;

    fn select_for(html: &str) -> (Dom, NodeId) {
        let dom = Dom::parse(html);
        let root = dom_elements(&dom)[0].id;
        (dom, root)
    }

    fn matches(dom: &Dom, root_id: NodeId, css: &str) -> Vec<NodeId> {
        let elements = dom_elements(dom);
        let root = DomElement { dom, id: root_id };
        let select = Select::new(root, elements, None, true);
        let selectors = parse(css).unwrap();
        select.matching(&selectors).unwrap().into_iter().map(|e| e.id).collect()
    }

    #[test]
    fn matches_type_class_id_and_universal_selectors() {
        let (dom, root) = select_for(r#"<html><body><div id="d" class="a b"><p class="a">x</p></div></body></html>"#);
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(matches(&dom, root, "p"), vec![p]);
        assert_eq!(matches(&dom, root, ".a"), vec![dom.find_first_tag_global("div").unwrap(), p]);
        assert_eq!(matches(&dom, root, "#d"), vec![dom.find_first_tag_global("div").unwrap()]);
        assert!(matches(&dom, root, "*").len() >= 3);
    }

    #[test]
    fn matches_descendant_child_and_sibling_combinators() {
        let (dom, root) = select_for(r#"<html><body><div><span><p>x</p></span><b>y</b><i>z</i></div></body></html>"#);
        let p = dom.find_first_tag_global("p").unwrap();
        let b = dom.find_first_tag_global("b").unwrap();
        let i = dom.find_first_tag_global("i").unwrap();
        assert_eq!(matches(&dom, root, "div p"), vec![p]);
        assert!(matches(&dom, root, "div > p").is_empty(), "p is a grandchild of div, not a direct child");
        assert_eq!(matches(&dom, root, "span > p"), vec![p]);
        assert_eq!(matches(&dom, root, "span + b"), vec![b]);
        assert!(matches(&dom, root, "span + i").is_empty(), "i is not the immediately-next sibling of span");
        assert_eq!(matches(&dom, root, "span ~ i"), vec![i]);
    }

    #[test]
    fn not_selector_excludes_matches() {
        let (dom, root) = select_for(r#"<html><body><p class="a">x</p><p class="b">y</p></body></html>"#);
        let ps = dom.find_all_tag_global("p");
        assert_eq!(matches(&dom, root, "p:not(.a)"), vec![ps[1]]);
    }

    #[test]
    fn attribute_operators_match_real_semantics() {
        let (dom, root) = select_for(r#"<html><body><a href="foo-bar" data-x="a b c" title="hello world"></a></body></html>"#);
        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(matches(&dom, root, "a[href]"), vec![a]);
        assert_eq!(matches(&dom, root, r#"a[href="foo-bar"]"#), vec![a]);
        assert_eq!(matches(&dom, root, r#"a[href|=foo]"#), vec![a], "dashmatch: foo-bar starts with foo-");
        assert_eq!(matches(&dom, root, r#"a[href^=foo]"#), vec![a]);
        assert_eq!(matches(&dom, root, r#"a[href$=bar]"#), vec![a]);
        assert_eq!(matches(&dom, root, r#"a[href*=oo-b]"#), vec![a]);
        assert_eq!(matches(&dom, root, r#"a[data-x~=b]"#), vec![a], "includes: 'b' is one of the space-separated tokens");
        assert!(matches(&dom, root, r#"a[title~=hello]"#).len() == 1);
    }

    #[test]
    fn the_not_equal_operator_is_a_controlled_error_not_a_panic() {
        let (dom, root) = select_for(r#"<html><body><a href="x"></a></body></html>"#);
        let elements = dom_elements(&dom);
        let root_elem = DomElement { dom: &dom, id: root };
        let select = Select::new(root_elem, elements, None, true);
        let selectors = parse(r#"a[href!=x]"#).unwrap();
        let err = select.matching(&selectors).unwrap_err();
        assert!(matches!(err, SelectorError::Expression(_)));
    }

    #[test]
    fn structural_pseudo_classes_match_real_sibling_positions() {
        let (dom, root) = select_for(r#"<html><body><div><p>1</p><p>2</p><p>3</p><span>x</span></div></body></html>"#);
        let ps = dom.find_all_tag_global("p");
        assert_eq!(matches(&dom, root, "p:first-child"), vec![ps[0]]);
        assert_eq!(matches(&dom, root, "p:last-of-type"), vec![ps[2]]);
        assert_eq!(matches(&dom, root, "p:nth-child(2n+1)"), vec![ps[0], ps[2]]);
        assert_eq!(matches(&dom, root, "p:nth-of-type(2)"), vec![ps[1]]);
        // div IS body's only child element, so it matches :only-child.
        let divs = dom.find_all_tag_global("div");
        assert_eq!(matches(&dom, root, "div:only-child"), divs);
        // None of the <p> siblings are an only-child (there are 3 of them).
        assert!(matches(&dom, root, "p:only-child").is_empty());
    }

    /// Cross-validated directly against the live real Python `Select`
    /// class (a temporary fake `lxml.etree`/`El` tree, per the
    /// established lxml-shaped xval technique -- no real caller needs
    /// `select.py`'s own lxml dependency, so a fake tree sidesteps it
    /// entirely) over ~30 real selector/tree combinations covering
    /// every combinator, attribute operator, structural pseudo-class,
    /// `:not()`, `:root`, `:lang()`, and the negative-coefficient
    /// `An+B` case below. Every case matched; this locks in the one
    /// most surprising result (a negative `a` coefficient selects a
    /// *prefix* of siblings, not a suffix).
    #[test]
    fn nth_child_with_a_negative_coefficient_selects_a_prefix() {
        let (dom, root) = select_for(r#"<html><body><div><p>1</p><p>2</p><p>3</p><p>4</p><p>5</p></div></body></html>"#);
        let ps = dom.find_all_tag_global("p");
        assert_eq!(matches(&dom, root, "p:nth-child(-n+3)"), vec![ps[0], ps[1], ps[2]]);
    }

    #[test]
    fn empty_matches_elements_with_no_children_and_no_text() {
        let (dom, root) = select_for(r#"<html><body><div></div><p>x</p><span> </span></body></html>"#);
        let div = dom.find_first_tag_global("div").unwrap();
        assert_eq!(matches(&dom, root, ":empty").iter().filter(|&&id| id == div).count(), 1);
        assert!(!matches(&dom, root, ":empty").contains(&dom.find_first_tag_global("p").unwrap()));
        assert!(!matches(&dom, root, ":empty").contains(&dom.find_first_tag_global("span").unwrap()), "whitespace-only text still counts as content");
    }

    #[test]
    fn root_pseudo_class_ignores_the_inner_selector() {
        let (dom, root) = select_for(r#"<html><body><div>x</div></body></html>"#);
        // Real quirk: `div:root` still just yields the document root,
        // not "a div that is also the root" (there is none here).
        assert_eq!(matches(&dom, root, "div:root"), vec![root]);
        assert_eq!(matches(&dom, root, ":root"), vec![root]);
    }

    #[test]
    fn lang_matches_via_inheritance_and_prefix() {
        let (dom, root) = select_for(r#"<html lang="en"><body><p lang="en-GB">x</p><div><span>y</span></div></body></html>"#);
        let span = dom.find_first_tag_global("span").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        // span inherits "en" from <html lang="en">.
        assert!(matches(&dom, root, ":lang(en)").contains(&span));
        // p's own "en-GB" still matches the "en" prefix query.
        assert!(matches(&dom, root, ":lang(en)").contains(&p));
        // But p does NOT match the more specific "en-us" query.
        assert!(!matches(&dom, root, ":lang(en-us)").contains(&p));
    }

    #[test]
    fn ignore_inappropriate_pseudo_classes_matches_unconditionally() {
        let (dom, root) = select_for(r#"<html><body><a href="x">y</a></body></html>"#);
        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(matches(&dom, root, "a:hover"), vec![a]);
        assert_eq!(matches(&dom, root, "a::before"), vec![a]);
    }

    #[test]
    fn an_unrecognized_pseudo_class_is_a_controlled_error() {
        let (dom, root) = select_for(r#"<html><body><a href="x"></a></body></html>"#);
        let elements = dom_elements(&dom);
        let root_elem = DomElement { dom: &dom, id: root };
        let select = Select::new(root_elem, elements, None, false);
        let selectors = parse(":totally-made-up").unwrap();
        let err = select.matching(&selectors).unwrap_err();
        assert!(matches!(err, SelectorError::Expression(_)));
    }

    #[test]
    fn normalize_language_tag_produces_every_real_combination() {
        let combos = normalize_language_tag("de_AT-1901");
        for expected in ["de-at-1901", "de-at", "de-1901", "de"] {
            assert!(combos.contains(expected), "missing {expected} in {combos:?}");
        }
        assert_eq!(combos.len(), 4);
    }
}
