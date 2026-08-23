//! Port of `old_src/src/calibre/ebooks/readability/readability.py`: the
//! arc90/`python-readability` scoring algorithm itself, as calibre's
//! `Document` class.
//!
//! `readability.py`'s `Document` methods are ported here mostly as free
//! functions taking `&Dom`/`&mut Dom` explicitly rather than as
//! `Document` methods, since almost none of them need anything from
//! `self` besides the tree itself; `Document`'s own methods
//! (`content`/`title`/`short_title`/`summary`/`sanitize`/
//! `remove_unlikely_candidates`/`transform_misused_divs_into_paragraphs`/
//! `score_paragraphs`) keep the shape of the Python (they need
//! `self.log`/`self.keep_elements`/the cached parsed tree).
//!
//! `option_parser`/`main` (the `calibre-debug`-style CLI entry point) are
//! not ported: nothing in this crate has an established CLI-entry-point
//! convention for a *library* crate like `calibre_ebooks` to follow (the
//! `mobi::debug` module this crate does have is invoked from
//! `src/bin/debug_mobi.rs`, a real binary target -- there's no
//! `readability`-flavored equivalent binary requested by this issue),
//! and `option_parser`/`main` are pure CLI plumbing around `Document`,
//! not part of the scoring algorithm.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use lazy_static::lazy_static;
use regex::Regex;

use crate::dom::{Dom, NodeId, NodeKind};

use super::cleaners::clean_html;
use super::htmls::{build_doc, get_body, get_title, shorten_title};
use super::{direct_text, element_children, tail_text};

lazy_static! {
    // Port of `REGEXES` in readability.py. The three commented-out-in-
    // Python entries (`replaceBrsRe`, `replaceFontsRe`, etc.) are dead
    // in the original too and aren't ported.
    static ref UNLIKELY_CANDIDATES_RE: Regex = Regex::new(
        r"(?i)combx|comment|community|disqus|extra|foot|header|menu|remark|rss|shoutbox|sidebar|sponsor|ad-break|agegate|pagination|pager|popup|tweet|twitter"
    ).expect("static regex");
    static ref OK_MAYBE_ITS_A_CANDIDATE_RE: Regex =
        Regex::new(r"(?i)and|article|body|column|main|shadow").expect("static regex");
    static ref POSITIVE_RE: Regex = Regex::new(
        r"(?i)article|body|content|entry|hentry|main|page|pagination|post|text|blog|story"
    ).expect("static regex");
    static ref NEGATIVE_RE: Regex = Regex::new(
        r"(?i)combx|comment|com-|contact|foot|footer|footnote|masthead|media|meta|outbrain|promo|related|scroll|shoutbox|sidebar|sponsor|shopping|tags|tool|widget"
    ).expect("static regex");
    static ref DIV_TO_P_ELEMENTS_RE: Regex =
        Regex::new(r"(?i)<(a|blockquote|dl|div|img|ol|p|pre|table|ul)").expect("static regex");
    static ref TRAILING_PERIOD_RE: Regex = Regex::new(r"\.( |$)").expect("static regex");
    static ref CLEAN_NEWLINE_RE: Regex = Regex::new(r"\s*\n\s*").expect("static regex");
    static ref CLEAN_SPACES_RE: Regex = Regex::new(r"[ \t]{2,}").expect("static regex");
}

/// Port of `describe` (the copy defined in `readability.py` itself,
/// distinct from -- and simpler than -- `debug.py`'s [`super::debug::describe`],
/// which additionally numbers repeated tags). Default `depth` in the
/// Python is `1`.
pub fn describe(dom: &Dom, node: NodeId, depth: usize) -> String {
    let Some(tag) = dom.tag(node) else {
        return "[non-element node]".to_string();
    };
    let mut name = tag.to_string();
    if let Some(id) = dom.node(node).attrs.get("id") {
        if !id.is_empty() {
            name.push('#');
            name.push_str(id);
        }
    }
    if let Some(class) = dom.node(node).attrs.get("class") {
        if !class.is_empty() {
            name.push('.');
            name.push_str(&class.replace(' ', "."));
        }
    }
    if name.starts_with("div#") || name.starts_with("div.") {
        name = name[3..].to_string();
    }
    if depth > 0 {
        if let Some(parent) = dom.parent(node) {
            // `dom.tag(parent).is_some()` mirrors lxml's `getparent() is
            // not None`: `Dom` has one extra wrapper layer lxml doesn't
            // expose (the `NodeKind::Document` root above `<html>`), so
            // recursion needs to stop there instead of getting a `None`
            // straight from `parent()` the way lxml's root element would.
            if dom.tag(parent).is_some() {
                return format!("{name} - {}", describe(dom, parent, depth - 1));
            }
        }
    }
    name
}

/// Port of `to_int`. Dead code upstream (the only call site is inside a
/// commented-out block in `sanitize`), ported for completeness anyway.
/// Deviates from the Python in one respect: a whitespace-only input
/// (e.g. `"   "`) passes Python's `if not x` truthiness check (a
/// non-empty string is truthy) but then produces `int('')` after
/// `.strip()`, which raises `ValueError` -- an unhandled crash in the
/// original for that input. Since nothing calls this and Rust has no
/// equivalent-severity "let it crash on bad input" idiom for a pure
/// helper, this returns `None` for any input that doesn't parse instead
/// of panicking.
pub fn to_int(x: &str) -> Option<i64> {
    if x.is_empty() {
        return None;
    }
    let x = x.trim();
    if let Some(stripped) = x.strip_suffix("px") {
        return stripped.parse().ok();
    }
    if let Some(stripped) = x.strip_suffix("em") {
        return stripped.parse::<i64>().ok().map(|v| v * 12);
    }
    x.parse().ok()
}

/// Port of `clean`: collapse whitespace-padded newlines to a bare `\n`,
/// collapse runs of 2+ spaces/tabs to one space, then trim.
pub fn clean(text: &str) -> String {
    let text = CLEAN_NEWLINE_RE.replace_all(text, "\n");
    let text = CLEAN_SPACES_RE.replace_all(&text, " ");
    text.trim().to_string()
}

/// Port of `text_content`. The Python's `hasattr(elem, 'text_content')`
/// branch always applies for every node type `Dom` can produce, so
/// there's no fallback branch to port -- this is a thin, named alias
/// for [`Dom::text_content`], kept as its own function for parity with
/// the Python module exposing it as a top-level name.
pub fn text_content(dom: &Dom, id: NodeId) -> String {
    dom.text_content(id)
}

/// Port of `text_length`.
pub fn text_length(dom: &Dom, id: NodeId) -> usize {
    clean(&text_content(dom, id)).chars().count()
}

/// Port of the `Unparsable` exception class.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Unparsable(pub String);

/// A tiny message sink used in place of Python's `calibre.utils.logging`
/// `Log` object, following the established convention -- see
/// `crate::mobi::MobiLog`/`crate::pdf::reflow::ReflowLog`. `exception`
/// stands in for `log.exception(msg)` (`summary()`'s catch-all handler);
/// there's no Rust backtrace to attach the way Python attaches
/// `sys.exc_info()`, so it just logs at the same "this failed" severity.
#[derive(Debug, Default, Clone)]
pub struct ReadabilityLog {
    pub messages: Vec<String>,
}

impl ReadabilityLog {
    pub fn debug(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("DEBUG: {}", msg.into()));
    }

    pub fn warn(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("WARNING: {}", msg.into()));
    }

    pub fn exception(&mut self, msg: impl Into<String>) {
        self.messages.push(format!("ERROR: {}", msg.into()));
    }
}

/// Port of `Document.__init__`'s `**options` bag -- the handful of keys
/// `readability.py` actually reads out of it (`url`, `min_text_length`,
/// `retry_length`, `keep_elements`; anything else lives in Python's
/// `defaultdict(lambda: None)` and is simply never read).
#[derive(Debug, Clone, Default)]
pub struct DocumentOptions {
    /// Base URL used to resolve relative links (`self.options['url']`).
    pub url: Option<String>,
    pub min_text_length: Option<usize>,
    pub retry_length: Option<usize>,
    /// Python's `options['keep_elements']` is an arbitrary XPath string
    /// (`self.html.xpath(path)`) naming elements that must never be
    /// removed. `Dom` has no general XPath engine -- unlike
    /// `shorten_title`'s ten *fixed, known* XPath patterns (translated
    /// directly to id/class-token lookups), this one is caller-supplied
    /// and unbounded, so it genuinely can't be evaluated here. A
    /// non-empty value is accepted for signature/API-compat with the
    /// Python constructor, but `Document` only logs a `debug` note that
    /// it's being ignored rather than evaluating it, crashing, or
    /// silently pretending it worked. The only real-world caller of this
    /// option in `old_src` is `calibre.web.feeds.news`
    /// (`self.auto_cleanup_keep`) -- unported, out of scope for this
    /// issue -- and nothing in this module's own test suite relies on
    /// it working.
    pub keep_elements_xpath: Option<String>,
}

#[derive(Debug, Clone, Copy)]
struct Candidate {
    content_score: f64,
    elem: NodeId,
}

/// Port of `Document`.
pub struct Document {
    input: Vec<u8>,
    pub options: DocumentOptions,
    pub log: ReadabilityLog,
    html: Option<Dom>,
    html_elem: Option<NodeId>,
    keep_elements: HashSet<NodeId>,
}

impl Document {
    pub const TEXT_LENGTH_THRESHOLD: usize = 25;
    pub const RETRY_LENGTH: usize = 250;

    pub fn new(input: Vec<u8>, options: DocumentOptions) -> Self {
        Document {
            input,
            options,
            log: ReadabilityLog::default(),
            html: None,
            html_elem: None,
            keep_elements: HashSet::new(),
        }
    }

    /// Port of `_html`/`_parse` combined: (re)parses `self.input` when
    /// `force` is set or nothing's been parsed yet, mirroring
    /// `build_doc` + `html_cleaner.clean_html` + the
    /// `make_links_absolute`/`resolve_base_href` base-href handling.
    fn ensure_html(&mut self, force: bool) {
        if !force && self.html.is_some() {
            return;
        }
        let mut dom = build_doc(&self.input);
        clean_html(&mut dom);
        if let Some(base_href) = self.options.url.clone() {
            make_links_absolute(&mut dom, &base_href, true);
        } else {
            resolve_base_href(&mut dom);
        }
        let html_id = dom.find_first_tag_global("html").unwrap_or(dom.root);
        self.html = Some(dom);
        self.html_elem = Some(html_id);

        if let Some(path) = &self.options.keep_elements_xpath {
            if !path.is_empty() {
                self.log.debug(format!(
                    "keep_elements XPath {path:?} is not supported by this port and is being ignored"
                ));
            }
        }
        self.keep_elements = HashSet::new();
    }

    /// Port of `content`.
    pub fn content(&mut self) -> String {
        self.ensure_html(true);
        get_body(self.html.as_mut().expect("just parsed"))
    }

    /// Port of `title`.
    pub fn title(&mut self) -> String {
        self.ensure_html(true);
        get_title(self.html.as_ref().expect("just parsed"))
    }

    /// Port of `short_title`.
    pub fn short_title(&mut self) -> String {
        self.ensure_html(true);
        shorten_title(self.html.as_ref().expect("just parsed"))
    }

    /// Port of `summary`. Returns `Result<String, Unparsable>` for
    /// parity with the Python (which wraps *any* exception raised
    /// during summarization as `Unparsable`), but note this
    /// implementation always returns `Ok`: unlike the Python, none of
    /// the steps below can fail in a data-dependent way (`Dom::parse`
    /// never errors -- it falls back to an empty document -- and every
    /// index/lookup here is done through `Option`-returning `Dom`
    /// accessors, not panicking indexing). The `Result` shape is kept
    /// so `Unparsable` is a real, usable type and the signature matches
    /// what callers of the Python API expect.
    pub fn summary(&mut self) -> Result<String, Unparsable> {
        let mut ruthless = true;
        loop {
            self.ensure_html(true);
            let html_id = self.html_elem.expect("just parsed");

            {
                let dom = self.html.as_mut().expect("just parsed");
                for id in dom.find_all_tag(html_id, "script") {
                    dom.detach(id);
                }
                for id in dom.find_all_tag(html_id, "style") {
                    dom.detach(id);
                }
                for id in dom.find_all_tag(html_id, "body") {
                    dom.node_mut(id)
                        .attrs
                        .insert("id".to_string(), "readabilityBody".to_string());
                }
            }

            if ruthless {
                self.remove_unlikely_candidates();
            }
            self.transform_misused_divs_into_paragraphs();
            let candidates = self.score_paragraphs();

            let best_candidate = select_best_candidate(
                &mut self.log,
                self.html.as_ref().expect("just parsed"),
                &candidates,
            );

            let article: NodeId;
            if let Some(best) = best_candidate {
                let dom = self.html.as_mut().expect("just parsed");
                article = get_article(dom, &candidates, &best, &self.keep_elements);
            } else if ruthless {
                self.log.debug("ruthless removal did not work. ");
                ruthless = false;
                self.log
                    .debug("ended up stripping too much - going for a safer _parse");
                continue;
            } else {
                self.log
                    .debug("Ruthless and lenient parsing did not work. Returning raw html");
                let dom = self.html.as_ref().expect("just parsed");
                // `self.html.find('body')`: a *direct child* search, not
                // a `.//body` descendant search.
                article = element_children(dom, html_id)
                    .into_iter()
                    .find(|&c| dom.tag(c) == Some("body"))
                    .unwrap_or(html_id);
            }

            let cleaned_article = self.sanitize(article, &candidates);
            let retry_length = self.options.retry_length.unwrap_or(Self::RETRY_LENGTH);
            let of_acceptable_length = cleaned_article.chars().count() >= retry_length;
            if ruthless && !of_acceptable_length {
                ruthless = false;
                continue;
            }
            return Ok(cleaned_article);
        }
    }

    /// Port of `score_paragraphs`.
    fn score_paragraphs(&self) -> IndexMap<NodeId, Candidate> {
        let min_len = self
            .options
            .min_text_length
            .unwrap_or(Self::TEXT_LENGTH_THRESHOLD);
        let dom = self.html.as_ref().expect("just parsed");
        let html_id = self.html_elem.expect("just parsed");

        // `IndexMap` (rather than `HashMap`) so `candidates` preserves
        // insertion order, matching Python's `dict` (insertion-ordered
        // since 3.7) -- `select_best_candidate`'s tie-break and
        // `shorten_title`'s equivalent both rely on a *reproducible*
        // order for candidates with equal scores/lengths.
        let mut candidates: IndexMap<NodeId, Candidate> = IndexMap::new();

        // `self.tags(self.html, 'p', 'pre', 'td')`: descendants of
        // `html`, searched *per tag type* in sequence (all `<p>`s in
        // document order, then all `<pre>`s, then all `<td>`s) -- not a
        // single merged document-order walk.
        for tag in ["p", "pre", "td"] {
            for elem in dom.find_all_tag(html_id, tag) {
                let Some(parent_node) = dom.parent(elem) else {
                    continue;
                };
                let grand_parent_node = dom.parent(parent_node);

                let inner_text = clean(&dom.text_content(elem));
                let inner_text_len = inner_text.chars().count();
                if inner_text_len < min_len {
                    continue;
                }

                candidates
                    .entry(parent_node)
                    .or_insert_with(|| score_node(dom, parent_node));
                if let Some(gp) = grand_parent_node {
                    candidates.entry(gp).or_insert_with(|| score_node(dom, gp));
                }

                let mut content_score = 1.0;
                content_score += inner_text.split(',').count() as f64;
                content_score += ((inner_text_len as f64) / 100.0).min(3.0);

                candidates
                    .get_mut(&parent_node)
                    .expect("just inserted")
                    .content_score += content_score;
                if let Some(gp) = grand_parent_node {
                    candidates
                        .get_mut(&gp)
                        .expect("just inserted")
                        .content_score += content_score / 2.0;
                }
            }
        }

        // Scale by `(1 - link_density)`. Iterating `candidates` here
        // (rather than a separate `ordered` list, as the Python keeps)
        // is equivalent since `IndexMap` iterates in insertion order,
        // which is exactly what `ordered` tracked.
        let ids: Vec<NodeId> = candidates.keys().copied().collect();
        for id in ids {
            let ld = get_link_density(dom, id);
            let c = candidates.get_mut(&id).expect("in map");
            c.content_score *= 1.0 - ld;
        }

        candidates
    }

    /// Port of `remove_unlikely_candidates`.
    fn remove_unlikely_candidates(&mut self) {
        let html_id = self.html_elem.expect("just parsed");
        let dom = self.html.as_mut().expect("just parsed");
        // `self.html.iter()`: every element from `html_id` itself
        // (inclusive) down, in document order -- `preorder_elements`
        // matches this directly (unlike `find_all_tag`, which excludes
        // the start node).
        let elements = dom.preorder_elements(html_id);
        for elem in elements {
            // Snapshotted before any drops in this loop happen; an
            // earlier drop in this same pass can orphan a later element
            // in the snapshot, which the live Python generator would
            // simply never reach. `is_attached` reproduces that.
            if !is_attached(dom, elem) {
                continue;
            }
            if self.keep_elements.contains(&elem) {
                continue;
            }
            let class = dom
                .node(elem)
                .attrs
                .get("class")
                .cloned()
                .unwrap_or_default();
            let id_attr = dom.node(elem).attrs.get("id").cloned().unwrap_or_default();
            let s = format!("{class} {id_attr}");
            if UNLIKELY_CANDIDATES_RE.is_match(&s)
                && !OK_MAYBE_ITS_A_CANDIDATE_RE.is_match(&s)
                && dom.tag(elem) != Some("body")
            {
                self.log.debug(format!(
                    "Removing unlikely candidate - {}",
                    describe(dom, elem, 1)
                ));
                dom.detach(elem);
            }
        }
    }

    /// Port of `transform_misused_divs_into_paragraphs`.
    fn transform_misused_divs_into_paragraphs(&mut self) {
        let html_id = self.html_elem.expect("just parsed");
        let dom = self.html.as_mut().expect("just parsed");

        // Pass 1: retag `<div>`s with no block-level element anywhere
        // in their (serialized) element children to `<p>`.
        let divs = dom.find_all_tag(html_id, "div");
        for elem in divs {
            if !is_attached(dom, elem) {
                continue;
            }
            let mut combined = String::new();
            for child in element_children(dom, elem) {
                combined.push_str(&dom.serialize(child));
            }
            if !DIV_TO_P_ELEMENTS_RE.is_match(&combined) {
                dom.set_tag(elem, "p");
            }
        }

        // Pass 2: a *fresh* div search -- elements retagged to `<p>`
        // above are no longer `<div>`s and are correctly excluded here,
        // matching the Python's two separate `self.tags(...)` calls.
        let divs = dom.find_all_tag(html_id, "div");
        for elem in divs {
            if !is_attached(dom, elem) {
                continue;
            }

            if let Some(text) = direct_text(dom, elem) {
                if !text.trim().is_empty() {
                    let p = dom.new_element("p");
                    let t = dom.new_text(&text);
                    dom.append_child(p, t);
                    remove_leading_text(dom, elem);
                    dom.insert_child(elem, 0, p);
                }
            }

            // `reversed(list(enumerate(elem)))`: process element
            // children from last to first, so inserting a new `<p>`
            // after a later child never shifts the position of an
            // earlier, not-yet-processed one.
            for child in element_children(dom, elem).into_iter().rev() {
                if let Some(tail) = tail_text(dom, child) {
                    if !tail.trim().is_empty() {
                        let p = dom.new_element("p");
                        let t = dom.new_text(&tail);
                        dom.append_child(p, t);
                        if let Some(tail_node) = dom.next_sibling(child) {
                            dom.detach(tail_node);
                        }
                        let idx = dom.index_in_parent(child).map(|i| i + 1).unwrap_or(0);
                        dom.insert_child(elem, idx, p);
                    }
                }
                if dom.tag(child) == Some("br") {
                    dom.detach(child);
                }
            }
        }
    }

    /// Port of `sanitize`.
    fn sanitize(&mut self, node: NodeId, candidates: &IndexMap<NodeId, Candidate>) -> String {
        let min_len = self
            .options
            .min_text_length
            .unwrap_or(Self::TEXT_LENGTH_THRESHOLD);
        let dom = self.html.as_mut().expect("just parsed");

        for tag in ["h1", "h2", "h3", "h4", "h5", "h6"] {
            for header in dom.find_all_tag(node, tag) {
                if !is_attached(dom, header) {
                    continue;
                }
                if class_weight(dom, header) < 0 || get_link_density(dom, header) > 0.33 {
                    dom.detach(header);
                }
            }
        }

        for tag in ["form", "iframe", "textarea"] {
            for elem in dom.find_all_tag(node, tag) {
                if !is_attached(dom, elem) {
                    continue;
                }
                dom.detach(elem);
            }
        }

        let mut allowed: HashSet<NodeId> = HashSet::new();

        for tag in ["table", "ul", "div"] {
            let mut matches = dom.find_all_tag(node, tag);
            matches.reverse();
            for el in matches {
                if !is_attached(dom, el) {
                    continue;
                }
                if allowed.contains(&el) || self.keep_elements.contains(&el) {
                    continue;
                }

                let weight = class_weight(dom, el) as f64;
                let own_content_score = candidates.get(&el).map(|c| c.content_score).unwrap_or(0.0);
                let tag_name = dom.tag(el).unwrap_or("").to_string();

                if weight + own_content_score < 0.0 {
                    self.log.debug(format!(
                        "Cleaned {} with score {:6.3} and weight {:<3}",
                        describe(dom, el, 1),
                        own_content_score,
                        weight
                    ));
                    dom.detach(el);
                    continue;
                }

                if dom.text_content(el).matches(',').count() >= 10 {
                    continue;
                }

                let mut counts: HashMap<&str, i64> = HashMap::new();
                for kind in ["p", "img", "li", "a", "embed", "input"] {
                    counts.insert(kind, dom.find_all_tag(el, kind).len() as i64);
                }
                *counts.get_mut("li").expect("just inserted") -= 100;

                let content_length = text_length(dom, el);
                let link_density = get_link_density(dom, el);
                let mut content_score = own_content_score;
                if let Some(parent_node) = dom.parent(el) {
                    content_score = candidates
                        .get(&parent_node)
                        .map(|c| c.content_score)
                        .unwrap_or(0.0);
                }

                let p_count = counts["p"];
                let img_count = counts["img"];
                let li_count = counts["li"];
                let input_count = counts["input"];
                let embed_count = counts["embed"];

                let mut to_remove;
                let reason;

                // clippy::if_same_then_else fires on the `weight < 25`/
                // `weight >= 25` arms below: the Python has two separate
                // `elif`s there (`weight < 25 and link_density > 0.2` /
                // `weight >= 25 and link_density > 0.5`) with the same
                // body but genuinely different, non-overlapping
                // thresholds -- kept as two branches for a direct,
                // checkable correspondence to the source lines rather
                // than merged into one `||` condition.
                #[allow(clippy::if_same_then_else)]
                if p_count > 0 && img_count > p_count {
                    reason = format!("too many images ({img_count})");
                    to_remove = true;
                } else if li_count > p_count && tag_name != "ul" && tag_name != "ol" {
                    reason = "more <li>s than <p>s".to_string();
                    to_remove = true;
                } else if (input_count as f64) > (p_count as f64 / 3.0) {
                    reason = "less than 3x <p>s than <input>s".to_string();
                    to_remove = true;
                } else if content_length < min_len && (img_count == 0 || img_count > 2) {
                    reason =
                        format!("too short content length {content_length} without a single image");
                    to_remove = true;
                } else if weight < 25.0 && link_density > 0.2 {
                    reason = format!("too many links {link_density:.3} for its weight {weight}");
                    to_remove = true;
                } else if weight >= 25.0 && link_density > 0.5 {
                    reason = format!("too many links {link_density:.3} for its weight {weight}");
                    to_remove = true;
                } else if (embed_count == 1 && content_length < 75) || embed_count > 1 {
                    reason =
                        "<embed>s with too short content length, or too many <embed>s".to_string();
                    to_remove = true;

                    // Find (up to) one non-empty following sibling and
                    // one non-empty preceding sibling and see if their
                    // combined content is long enough to "rescue" this
                    // element after all. `itersiblings()`/
                    // `itersiblings(preceding=True)` in lxml only ever
                    // yield *Element* siblings (text lives in
                    // `.text`/`.tail`, never as a standalone sibling),
                    // so `Dom`'s literal `next_sibling`/`prev_sibling`
                    // (which *do* return text nodes) are filtered to
                    // `Element` kind here to match.
                    //
                    // The Python has `j =+ 1` in the preceding-siblings
                    // loop below (`j` gets *reassigned* to `1` on every
                    // hit, rather than incremented via `j += 1`) --
                    // reads like a stray-`+`-sign typo for what should
                    // be `j += 1`. It's provably harmless here, though:
                    // the loop's only stop condition is `x == 1` (`x` is
                    // hardcoded to `1` a few lines up), and it breaks
                    // immediately on the *first* non-empty preceding
                    // sibling either way -- `j = 1` and `j = 0 + 1` are
                    // the same value the first (and only, since it
                    // breaks right after) time either assignment runs.
                    // So there is no reachable input for which the typo
                    // changes behavior, and this port implements the
                    // (identical either way) correct increment rather
                    // than reproducing the typo for its own sake.
                    let mut siblings: Vec<usize> = Vec::new();
                    let mut cur = dom.next_sibling(el);
                    while let Some(c) = cur {
                        if matches!(dom.node(c).kind, NodeKind::Element(_)) {
                            let len = text_length(dom, c);
                            if len != 0 {
                                siblings.push(len);
                                break;
                            }
                        }
                        cur = dom.next_sibling(c);
                    }
                    let mut cur = dom.prev_sibling(el);
                    while let Some(c) = cur {
                        if matches!(dom.node(c).kind, NodeKind::Element(_)) {
                            let len = text_length(dom, c);
                            if len != 0 {
                                siblings.push(len);
                                break;
                            }
                        }
                        cur = dom.prev_sibling(c);
                    }
                    if !siblings.is_empty() && siblings.iter().sum::<usize>() > 1000 {
                        to_remove = false;
                        self.log.debug(format!("Allowing {}", describe(dom, el, 1)));
                        for tag2 in ["table", "ul", "div"] {
                            for desnode in dom.find_all_tag(el, tag2) {
                                allowed.insert(desnode);
                            }
                        }
                    }
                } else {
                    to_remove = false;
                    reason = String::new();
                }

                if to_remove {
                    self.log.debug(format!(
                        "Cleaned {content_score:6.3} {} with weight {weight} cause it has {reason}.",
                        describe(dom, el, 1)
                    ));
                    dom.detach(el);
                }
            }
        }

        clean_attributes_wrapper(dom, node)
    }
}

fn clean_attributes_wrapper(dom: &Dom, node: NodeId) -> String {
    super::cleaners::clean_attributes(&dom.serialize(node))
}

/// True if `id` is reachable from the tree's true root by walking
/// `parent()` links -- i.e. hasn't been [`Dom::detach`]ed (possibly as
/// part of a dropped ancestor's subtree) since some earlier snapshot of
/// the tree was taken. Needed because several passes in this port
/// gather a fixed `Vec<NodeId>` up front (mirroring a single
/// `findall()`/`.iter()` call in the Python) and then mutate the tree
/// while iterating it, exactly as the Python's lazy generators do --
/// but Python's generators, being *live* views, would simply never
/// visit a node whose ancestor already got `drop_tree()`d earlier in
/// the same pass, whereas a pre-materialized `Vec` still contains it.
fn is_attached(dom: &Dom, mut id: NodeId) -> bool {
    loop {
        match dom.parent(id) {
            Some(p) => id = p,
            None => return id == dom.root,
        }
    }
}

/// Removes `id`'s leading `Text` node children (lxml's `elem.text =
/// None`, applied when that text has just been moved into a new `<p>`).
fn remove_leading_text(dom: &mut Dom, id: NodeId) {
    while let Some(first) = dom.children(id).first().copied() {
        match dom.node(first).kind {
            NodeKind::Text(_) => dom.detach(first),
            _ => break,
        }
    }
}

/// Port of `Document.class_weight`.
fn class_weight(dom: &Dom, e: NodeId) -> i64 {
    let mut weight = 0i64;
    if let Some(class) = dom.node(e).attrs.get("class") {
        if !class.is_empty() {
            if NEGATIVE_RE.is_match(class) {
                weight -= 25;
            }
            if POSITIVE_RE.is_match(class) {
                weight += 25;
            }
        }
    }
    if let Some(id) = dom.node(e).attrs.get("id") {
        if !id.is_empty() {
            if NEGATIVE_RE.is_match(id) {
                weight -= 25;
            }
            if POSITIVE_RE.is_match(id) {
                weight += 25;
            }
        }
    }
    weight
}

/// Port of `Document.score_node`.
fn score_node(dom: &Dom, elem: NodeId) -> Candidate {
    let mut content_score = class_weight(dom, elem) as f64;
    let name = dom.tag(elem).unwrap_or("").to_lowercase();
    match name.as_str() {
        "div" => content_score += 5.0,
        "pre" | "td" | "blockquote" => content_score += 3.0,
        "address" | "ol" | "ul" | "dl" | "dd" | "dt" | "li" | "form" => content_score -= 3.0,
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th" => content_score -= 5.0,
        _ => {}
    }
    Candidate {
        content_score,
        elem,
    }
}

/// Port of `Document.get_link_density`.
fn get_link_density(dom: &Dom, elem: NodeId) -> f64 {
    let mut link_length = 0usize;
    for a in dom.find_all_tag(elem, "a") {
        link_length += text_length(dom, a);
    }
    let total_length = text_length(dom, elem);
    link_length as f64 / (total_length.max(1)) as f64
}

/// Port of `Document.select_best_candidate`. A free function (taking
/// `log`/`dom` explicitly) rather than a `Document` method to sidestep
/// borrowing `self.html` and `self.log` mutably/immutably at once from
/// inside `summary()`.
fn select_best_candidate(
    log: &mut ReadabilityLog,
    dom: &Dom,
    candidates: &IndexMap<NodeId, Candidate>,
) -> Option<Candidate> {
    let mut sorted: Vec<&Candidate> = candidates.values().collect();
    sorted.sort_by(|a, b| {
        b.content_score
            .partial_cmp(&a.content_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for c in sorted.iter().take(5) {
        log.debug(format!(
            "Top 5 : {:6.3} {}",
            c.content_score,
            describe(dom, c.elem, 1)
        ));
    }
    sorted.first().map(|c| **c)
}

/// Port of `Document.get_article`. Builds a fresh `<body><div>...</div></body>`
/// wrapper *within the same [`Dom`] arena* (rather than a genuinely
/// separate document, as the Python's `document_fromstring('<div/>')`
/// does) and moves the chosen siblings into it -- `Dom::append_child`
/// already detaches a node from wherever it currently lives before
/// reattaching it, which is exactly the "move" semantics
/// `parent.append(sibling)` has in lxml.
fn get_article(
    dom: &mut Dom,
    candidates: &IndexMap<NodeId, Candidate>,
    best_candidate: &Candidate,
    keep_elements: &HashSet<NodeId>,
) -> NodeId {
    let sibling_score_threshold = (best_candidate.content_score * 0.2).max(10.0);
    let best_elem = best_candidate.elem;

    let body = dom.new_element("body");
    let div = dom.new_element("div");
    dom.append_child(body, div);

    // `best_elem.getparent().getchildren()`. `Dom`'s extra
    // `NodeKind::Document` wrapper above `<html>` means `parent()`
    // essentially never returns `None` here in practice (unlike lxml,
    // where the root element's `getparent()` really is `None`); the
    // `unwrap_or` fallback below only matters for that theoretical edge
    // case and just keeps `best_elem` itself as the sole candidate to
    // keep, rather than panicking.
    let siblings = match dom.parent(best_elem) {
        Some(p) => element_children(dom, p),
        None => vec![best_elem],
    };

    for sibling in siblings {
        let mut append = false;
        if sibling == best_elem {
            append = true;
        }
        if let Some(c) = candidates.get(&sibling) {
            if c.content_score >= sibling_score_threshold {
                append = true;
            }
        }
        if keep_elements.contains(&sibling) {
            append = true;
        }

        if dom.tag(sibling) == Some("p") {
            let link_density = get_link_density(dom, sibling);
            let node_content = direct_text(dom, sibling).unwrap_or_default();
            let node_length = node_content.chars().count();
            // clippy::if_same_then_else: both arms just set `append =
            // true`, but they're the Python's two distinct heuristics
            // ("long paragraph with low link density" vs. "short
            // paragraph, no links, ends in a sentence") kept separate
            // for a direct correspondence to the source.
            #[allow(clippy::if_same_then_else)]
            if node_length > 80 && link_density < 0.25 {
                append = true;
            } else if node_length < 80
                && link_density == 0.0
                && TRAILING_PERIOD_RE.is_match(&node_content)
            {
                append = true;
            }
        }

        if append {
            dom.append_child(div, sibling);
        }
    }

    body
}

/// Port of `Document._parse`'s `doc.resolve_base_href()` call (used
/// when no explicit `url` option is given): find the last `<base
/// href="...">` element in the document, drop every `<base>` element,
/// and (if one was found) make all links absolute against that href.
fn resolve_base_href(dom: &mut Dom) {
    let base_tags: Vec<NodeId> = dom
        .find_all_tag_global("base")
        .into_iter()
        .filter(|&id| dom.node(id).attrs.contains_key("href"))
        .collect();
    let mut base_href: Option<String> = None;
    for &b in &base_tags {
        // lxml's loop reassigns on every match, keeping the *last* one.
        base_href = dom.node(b).attrs.get("href").cloned();
    }
    for b in base_tags {
        dom.detach(b);
    }
    if let Some(href) = base_href {
        if !href.is_empty() {
            make_links_absolute(dom, &href, false);
        }
    }
}

/// Port of `Document._parse`'s `doc.make_links_absolute(base_href,
/// resolve_base_href=True)` call (used when an explicit `url` option is
/// given). Scoped to the attributes that carry URLs in practice
/// (`href`/`src`/`action`/`cite`/`longdesc`/`usemap`/`background`/`data`)
/// rather than lxml's full `iterlinks()` attribute list -- this port's
/// own test suite and `Document`'s scoring pipeline never depend on the
/// exhaustive list, only on `<a href>`/`<img src>` being resolved.
/// Uses `url::Url::join` for real RFC 3986 resolution (already a
/// dependency of this crate, used the same way in
/// `oeb::polish::download`/`pdf::render::links`); a `base_url` that
/// doesn't parse as an absolute URL leaves every link untouched rather
/// than erroring, matching the fact that this is a best-effort
/// convenience step, not something `Document`'s own algorithm depends
/// on for correctness.
fn make_links_absolute(dom: &mut Dom, base_url: &str, resolve_base_first: bool) {
    if resolve_base_first {
        resolve_base_href(dom);
    }
    let Ok(base) = url::Url::parse(base_url) else {
        return;
    };
    const LINK_ATTRS: &[&str] = &[
        "href",
        "src",
        "action",
        "cite",
        "longdesc",
        "usemap",
        "background",
        "data",
    ];
    let elements = dom.preorder_elements(dom.root);
    for id in elements {
        for attr in LINK_ATTRS {
            if let Some(val) = dom.node(id).attrs.get(*attr).cloned() {
                if let Ok(joined) = base.join(&val) {
                    dom.node_mut(id)
                        .attrs
                        .insert((*attr).to_string(), joined.to_string());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(html: &str) -> Dom {
        Dom::parse(html)
    }

    #[test]
    fn to_int_parses_px_em_and_plain() {
        assert_eq!(to_int("10px"), Some(10));
        assert_eq!(to_int("2em"), Some(24));
        assert_eq!(to_int("5"), Some(5));
        assert_eq!(to_int(""), None);
        assert_eq!(to_int("   "), None);
        assert_eq!(to_int("not a number"), None);
    }

    #[test]
    fn clean_collapses_whitespace() {
        assert_eq!(clean("  a  \n  b   c  "), "a\nb c");
    }

    #[test]
    fn describe_formats_tag_id_class_and_parent_chain() {
        let dom = parse("<html><body><div id=\"x\" class=\"a b\"><p>hi</p></div></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let d = describe(&dom, p, 2);
        assert_eq!(d, "p - #x.a.b - body");
    }

    #[test]
    fn class_weight_positive_and_negative() {
        let dom = parse(
            "<html><body><div class=\"article-content\" id=\"x\"></div>\
             <div class=\"sidebar\" id=\"y\"></div></body></html>",
        );
        let divs = dom.find_all_tag_global("div");
        assert_eq!(class_weight(&dom, divs[0]), 25);
        assert_eq!(class_weight(&dom, divs[1]), -25);
    }

    #[test]
    fn score_node_applies_tag_bonus() {
        let dom = parse("<html><body><div></div><ul></ul><h1></h1></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let ul = dom.find_first_tag_global("ul").unwrap();
        let h1 = dom.find_first_tag_global("h1").unwrap();
        assert_eq!(score_node(&dom, div).content_score, 5.0);
        assert_eq!(score_node(&dom, ul).content_score, -3.0);
        assert_eq!(score_node(&dom, h1).content_score, -5.0);
    }

    #[test]
    fn get_link_density_all_text_linked() {
        let dom = parse("<html><body><p><a href=\"x\">hello world</a></p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(get_link_density(&dom, p), 1.0);
    }

    #[test]
    fn get_link_density_no_links() {
        let dom = parse("<html><body><p>hello world</p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(get_link_density(&dom, p), 0.0);
    }

    #[test]
    fn get_link_density_partial() {
        let dom = parse("<html><body><p>hello <a href=\"x\">world</a></p></body></html>");
        let p = dom.find_first_tag_global("p").unwrap();
        let ld = get_link_density(&dom, p);
        assert!(ld > 0.0 && ld < 1.0, "{ld}");
    }

    #[test]
    fn sibling_scan_j_quirk_is_behaviorally_a_no_op() {
        // Documents the decision on the Python's `j =+ 1` typo: with
        // `x` hardcoded to `1`, "reassign to 1" and "increment from 0"
        // are identical on the first (and only, since it then breaks)
        // hit, so a plain, correct increment is what's implemented.
        // This test exercises the actual preceding/following-sibling
        // rescue path in `sanitize` end to end.
        let html = format!(
            "<html><body>{}<div class=\"a\"><embed></embed><embed></embed></div>{}</body></html>",
            "<p>".to_string() + &"x".repeat(600) + "</p>",
            "<p>".to_string() + &"y".repeat(600) + "</p>",
        );
        let mut doc = Document::new(html.into_bytes(), DocumentOptions::default());
        let out = doc.summary().expect("summary");
        // The two-embed div would normally be dropped (`embed_count >
        // 1`), but its neighboring <p>s are long enough (>1000 combined
        // chars) that the sibling-content-length rescue keeps it.
        assert!(out.contains("<embed"), "{out}");
    }

    #[test]
    fn summary_extracts_article_and_drops_clutter() {
        let html = r#"
        <html>
        <head><title>Great Article Title - Example Site</title></head>
        <body>
        <div id="nav" class="menu"><a href="/1">Home</a><a href="/2">About</a><a href="/3">Contact</a></div>
        <div id="sidebar" class="sidebar widget">
            <p>Buy our stuff! <a href="/ad1">Click here</a> for amazing deals on <a href="/ad2">products</a>.</p>
            <p>Subscribe to our <a href="/newsletter">newsletter</a> now.</p>
        </div>
        <div id="content" class="article-content">
            <h1>Great Article Title</h1>
            <p>This is the first paragraph of a genuinely substantial article about
            something interesting. It goes on for a while, describing the topic in
            detail, with plenty of real prose so that the content-scoring algorithm
            recognizes it as the main body of the page rather than boilerplate.</p>
            <p>Here is a second paragraph continuing the discussion, adding more
            detail and context, still with no links at all, just plain narrative
            text that a reader would actually want to read when visiting this page.</p>
            <p>And a third paragraph to make sure there is more than enough text
            for the retry-length threshold, wrapping up the article with a
            concluding thought that ties the whole piece together nicely.</p>
        </div>
        <div id="footer" class="footer"><p>Copyright 2024. <a href="/terms">Terms</a> | <a href="/privacy">Privacy</a></p></div>
        </body>
        </html>
        "#;
        let mut doc = Document::new(html.as_bytes().to_vec(), DocumentOptions::default());
        let out = doc.summary().expect("summary should succeed");

        assert!(
            out.contains("first paragraph of a genuinely substantial article"),
            "{out}"
        );
        assert!(
            out.contains("second paragraph continuing the discussion"),
            "{out}"
        );
        assert!(out.contains("concluding thought"), "{out}");

        assert!(!out.contains("Buy our stuff"), "{out}");
        assert!(!out.contains("Subscribe to our"), "{out}");
        assert!(!out.contains("Home</a>"), "{out}");
        assert!(!out.contains("Copyright 2024"), "{out}");

        let title = doc.title();
        assert_eq!(title, "Great Article Title - Example Site");
    }

    #[test]
    fn summary_falls_back_when_content_too_short() {
        // A page with no real content at all: the ruthless pass will
        // fail to find a good candidate, and the retry-length check
        // should force the lenient (non-ruthless) fallback path rather
        // than panicking or looping forever.
        let html = "<html><body><div class=\"sidebar\">x</div></body></html>";
        let mut doc = Document::new(html.as_bytes().to_vec(), DocumentOptions::default());
        let out = doc.summary().expect("summary should not error");
        assert!(!out.is_empty());
    }

    #[test]
    fn make_links_absolute_resolves_relative_hrefs() {
        let html = b"<html><body><a href=\"/foo\">x</a></body></html>";
        let mut doc = Document::new(
            html.to_vec(),
            DocumentOptions {
                url: Some("https://example.com/bar/baz.html".to_string()),
                ..Default::default()
            },
        );
        let body = doc.content();
        assert!(body.contains("https://example.com/foo"), "{body}");
    }

    #[test]
    fn resolve_base_href_uses_embedded_base_tag() {
        let html =
            b"<html><head><base href=\"https://example.com/dir/\"></head><body><a href=\"x\">x</a></body></html>";
        let mut doc = Document::new(html.to_vec(), DocumentOptions::default());
        let body = doc.content();
        assert!(body.contains("https://example.com/dir/x"), "{body}");
    }
}
