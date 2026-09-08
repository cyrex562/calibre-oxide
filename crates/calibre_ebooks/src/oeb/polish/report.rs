//! Port of `old_src/src/calibre/ebooks/oeb/polish/report.py` (issue
//! #584, split off `tinycss`'s issue #88 since this file isn't itself
//! a tinycss file). Real Python data-gathering functions for the GUI's
//! "Check Book" report, orchestrated by `gather_data`.
//!
//! Every real function in `report.py` is ported: `files_data`/
//! `images_data`/`words_data`/`chars_data`/`links_data`/
//! `create_anchor_map`/`description_for_anchor`/`css_data`/
//! `gather_data`.
//!
//! Two real, crate-wide infrastructure gaps this module's real
//! algorithm exposed were closed directly rather than worked around:
//! - `links_data` needed source line tracking on [`crate::dom::Dom`]
//!   (confirmed absent by reading `dom.rs`'s `Node` struct directly
//!   before starting, not assumed) -- added via a custom `TreeSink`
//!   wrapping `RcDom`, see `dom.rs`'s `LineTrackingSink` docs.
//! - `css_data` needed the same kind of tracking on
//!   `crate::css::model::StyleRule` (`line`/`column`, via
//!   `cssparser::Parser::current_source_location`) and exposed a
//!   pre-existing, unrelated `todo!()`: `Container::iterlinks`'s CSS
//!   branch (needed by `images_data`, run earlier in `gather_data`,
//!   for ANY book with a stylesheet -- virtually all of them) had
//!   never been implemented, citing a stale "no CSS parser exists"
//!   claim issue #164 already resolved. Fixed as a direct, faithful
//!   port of upstream's own real `itercsslinks`/`PositionFinder`/
//!   `CommentFinder` (a regex-based link scanner, not a
//!   `crate::css`-routed one -- upstream's own implementation isn't a
//!   CSS-parser job either), in `container.rs`'s `css_links_with_positions`.
//!
//! `description_for_anchor` is adapted to this crate's child-node text
//! model (text is its own [`NodeKind::Text`] child, unlike lxml's
//! `.text`/`.tail`) rather than transliterated against an API `Dom`
//! doesn't have, matching the same adaptation `html_transform_rules`
//! (#118) already established. Real CSS-selector-to-DOM-element
//! matching (`css_selectors.Select` in Python) was never a blocker for
//! `css_data` -- `crate::css::selector`/`matcher` already provides
//! that against a `Dom`.
//!
//! `tag_text` fixes a confirmed real upstream bug (a missing final
//! `return ans`, so real Python only ever returns a tag string the
//! very first time it's called for an attribute-bearing element) --
//! see [`tag_text`]'s own doc.
//!
//! Three small real primitives this file needed didn't exist yet and
//! were added directly to `calibre_utils::icu` rather than
//! reimplemented locally: `numeric_strcmp` (port of
//! `icu.numeric_sort_key`, exposed as a comparator like every other
//! `icu.*strcmp` in this crate) and `safe_chr`.
//!
//! Python's `file_words_counts`/`gather_data` orchestration threads a
//! module-level global dict between `words_data` and `files_data`;
//! this port passes it explicitly as a parameter instead (a plain,
//! Rust-idiomatic replacement for the same data flow, not a
//! behavioral change). `css_data`'s own `result_data['classes']`
//! side-effect-into-a-shared-dict likewise becomes a plain second
//! return value.

use std::collections::HashMap;

use anyhow::Result;

use calibre_utils::icu::numeric_strcmp;

use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::spell::{count_all_chars, get_all_words, DictionaryLocale, Location};

use super::container::Container;
use super::utils::OEB_FONTS;

fn posix_dirname(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[..i].to_string(),
        None => String::new(),
    }
}

fn posix_basename(path: &str) -> String {
    match path.rfind('/') {
        Some(i) => path[i + 1..].to_string(),
        None => path.to_string(),
    }
}

fn safe_size(container: &Container, name: &str) -> u64 {
    std::fs::metadata(container.name_to_abspath(name))
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Port of `get_category`.
pub fn get_category(name: &str, mime_type: &str) -> &'static str {
    let mut category = "misc";
    if mime_type.starts_with("image/") {
        category = "image";
    } else if OEB_FONTS.contains(&mime_type) {
        category = "font";
    } else if OEB_STYLES.contains(&mime_type) {
        category = "style";
    } else if OEB_DOCS.contains(&mime_type) {
        category = "text";
    }
    // Python's `name.rpartition('.')[-1]` yields the whole name (not
    // an empty string) when there's no `.` at all.
    let ext = match name.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => name.to_ascii_lowercase(),
    };
    match ext.as_str() {
        "ttf" | "otf" | "woff" | "woff2" => category = "font",
        "opf" => category = "opf",
        "ncx" => category = "toc",
        _ => {}
    }
    category
}

/// Port of the `File` namedtuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub dir: String,
    pub basename: String,
    pub size: u64,
    pub category: &'static str,
    /// `-1` matches Python's `fwc.get(name, -1)` sentinel for "not
    /// counted" (e.g. this file was never passed to `words_data`).
    pub word_count: i64,
}

/// Port of `files_data`. `file_words_counts` corresponds to Python's
/// module-level global of the same name, threaded explicitly here
/// instead (see the module docs) -- pass `None` for "not gathered yet"
/// (every file's `word_count` is then `-1`, matching the Python
/// default), or the map [`words_data`] fills in via its
/// `file_word_counts` out-parameter.
pub fn files_data(container: &Container, file_words_counts: Option<&HashMap<String, usize>>) -> Vec<FileEntry> {
    let mut names: Vec<&String> = container.name_path_map.keys().collect();
    // HashMap iteration order is not deterministic; Python's dict
    // preserves insertion order. Sorting by name is a real, disclosed
    // determinism improvement over relying on either language's
    // incidental iteration order.
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let mt = container.base.mime_map.get(name).map(String::as_str).unwrap_or("");
            FileEntry {
                dir: posix_dirname(name),
                basename: posix_basename(name),
                size: safe_size(container, name),
                category: get_category(name, mt),
                word_count: file_words_counts
                    .and_then(|m| m.get(name))
                    .map(|v| *v as i64)
                    .unwrap_or(-1),
                name: name.clone(),
            }
        })
        .collect()
}

/// Port of `LinkLocation`. `text_on_line` is `Option` (issue #590's
/// `links_data`/`create_anchor_map` need a real `None` here, matching
/// upstream's own untyped `LinkLocation(dest, None, None)` construction
/// for a resolved-anchor's own location).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkLocation {
    pub name: String,
    pub line_number: Option<u32>,
    pub text_on_line: Option<String>,
}

fn safe_href_to_name(container: &Container, href: &str, base: &str) -> Option<String> {
    container.href_to_name(href, Some(base))
}

/// Port of `sort_locations`.
fn sort_locations(container: &mut Container, mut locations: Vec<LinkLocation>) -> Result<Vec<LinkLocation>> {
    let spine_names = container.spine_names()?;
    let order: HashMap<&str, usize> = spine_names.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let fallback = order.len();
    locations.sort_by(|a, b| {
        let ia = order.get(a.name.as_str()).copied().unwrap_or(fallback);
        let ib = order.get(b.name.as_str()).copied().unwrap_or(fallback);
        ia.cmp(&ib)
            .then_with(|| numeric_strcmp(&a.name, &b.name))
            .then_with(|| a.line_number.cmp(&b.line_number))
    });
    Ok(locations)
}

fn safe_img_data(container: &mut Container, name: &str, mime_type: &str) -> (i64, i64) {
    if mime_type.contains("svg") {
        return (0, 0);
    }
    match container.raw_data(name, false) {
        Ok(data) => {
            let (_, width, height) = calibre_utils::imghdr::identify(&data);
            (width, height)
        }
        Err(_) => (0, 0),
    }
}

/// Port of the `Image` namedtuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageEntry {
    pub name: String,
    pub mime_type: String,
    pub usage: Vec<LinkLocation>,
    pub size: u64,
    pub basename: String,
    pub id: usize,
    pub width: i64,
    pub height: i64,
}

/// Port of `images_data`.
pub fn images_data(container: &mut Container) -> Result<Vec<ImageEntry>> {
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    let mut image_usage: HashMap<String, std::collections::HashSet<LinkLocation>> = HashMap::new();
    for (name, mt) in &names {
        if OEB_STYLES.contains(&mt.as_str()) || OEB_DOCS.contains(&mt.as_str()) {
            for (href, line_number, _offset) in container.iterlinks(name)? {
                let Some(target) = safe_href_to_name(container, &href, name) else {
                    continue;
                };
                if !container.exists(&target) {
                    continue;
                }
                let target_mt = container.base.mime_map.get(&target).cloned().unwrap_or_default();
                if target_mt.starts_with("image/") {
                    image_usage.entry(target).or_default().insert(LinkLocation {
                        name: name.clone(),
                        line_number,
                        text_on_line: Some(href),
                    });
                }
            }
        }
    }

    let mut image_data = Vec::new();
    for (name, mt) in &names {
        if mt.starts_with("image/") && container.exists(name) {
            let usage: Vec<LinkLocation> = image_usage.remove(name).unwrap_or_default().into_iter().collect();
            let usage = sort_locations(container, usage)?;
            let (width, height) = safe_img_data(container, name, mt);
            let id = image_data.len();
            image_data.push(ImageEntry {
                name: name.clone(),
                mime_type: mt.clone(),
                usage,
                size: safe_size(container, name),
                basename: posix_basename(name),
                id,
                width,
                height,
            });
        }
    }
    Ok(image_data)
}

/// Port of the `Word` namedtuple.
#[derive(Debug, Clone)]
pub struct WordEntry {
    pub id: usize,
    pub word: String,
    pub locale: DictionaryLocale,
    pub usage: Vec<Location>,
}

/// Port of `words_data`. Returns the total word count, the per-word
/// entries, and (as an explicit out-parameter -- see the module docs)
/// the per-file word counts [`files_data`] can use.
pub fn words_data(
    container: &mut Container,
    book_locale: &DictionaryLocale,
) -> Result<(usize, Vec<WordEntry>, HashMap<String, usize>)> {
    let mut file_word_counts = HashMap::new();
    let excluded_files = std::collections::HashSet::new();
    let (count, words) = get_all_words(container, book_locale, &excluded_files, &mut file_word_counts)?;
    // HashMap iteration order isn't deterministic; sort by (word,
    // locale) for stable, reproducible `id` assignment -- Python relies
    // on dict insertion order here, an accident of first-encountered
    // order rather than a meaningful sort, so this is a real
    // improvement, not a narrowing.
    let mut entries: Vec<((String, DictionaryLocale), Vec<Location>)> = words.into_iter().collect();
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    let words_vec = entries
        .into_iter()
        .enumerate()
        .map(|(id, ((word, locale), usage))| WordEntry { id, word, locale, usage })
        .collect();
    Ok((count, words_vec, file_word_counts))
}

/// Port of the `Char` namedtuple. `char` is already a validated
/// Unicode scalar value in Rust (unlike Python's raw integer
/// codepoint), so `icu::safe_chr` isn't needed at this specific call
/// site -- [`crate::oeb::polish::spell::CharCounter`] already stores
/// real `char`s, not codepoints that could be an invalid surrogate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharEntry {
    pub id: usize,
    pub ch: char,
    pub codepoint: u32,
    pub usage: Vec<String>,
    pub count: u32,
}

/// Port of `chars_data`.
pub fn chars_data(container: &mut Container, book_locale: &DictionaryLocale) -> Result<Vec<CharEntry>> {
    let cc = count_all_chars(container, book_locale)?;
    let spine_names = container.spine_names()?;
    let order: HashMap<&str, usize> = spine_names.iter().enumerate().map(|(i, (n, _))| (n.as_str(), i)).collect();
    let fallback = order.len();

    // Deterministic ordering by codepoint, rather than relying on
    // HashMap iteration order the way Python relies on dict insertion
    // order (itself just first-encountered order, not a meaningful
    // sort) -- see `words_data`'s doc for the same reasoning.
    let mut chars: Vec<char> = cc.chars.keys().copied().collect();
    chars.sort();

    let mut result = Vec::with_capacity(chars.len());
    for (id, ch) in chars.into_iter().enumerate() {
        let mut usage: Vec<String> = cc.chars.get(&ch).cloned().unwrap_or_default().into_iter().collect();
        usage.sort_by(|a, b| {
            let ia = order.get(a.as_str()).copied().unwrap_or(fallback);
            let ib = order.get(b.as_str()).copied().unwrap_or(fallback);
            ia.cmp(&ib).then_with(|| numeric_strcmp(a, b))
        });
        result.push(CharEntry {
            id,
            ch,
            codepoint: ch as u32,
            usage,
            count: cc.counter.get(&ch).copied().unwrap_or(0),
        });
    }
    Ok(result)
}

// ===================================================================
// links_data (issue #590 -- needed real source-line tracking on
// crate::dom::Dom, added directly to that module rather than worked
// around here)
// ===================================================================

/// Port of `description_for_anchor`. Adapted to this crate's
/// child-node text model (text is its own [`NodeKind::Text`] child,
/// unlike lxml's `.text`/`.tail` attributes -- see the crate's own
/// `html_transform_rules` precedent for this same adaptation) rather
/// than transliterated against an API this crate's [`crate::dom::Dom`]
/// doesn't have.
fn description_for_anchor(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> Option<String> {
    fn check(x: Option<&str>, min_len: usize) -> Option<String> {
        let x = x?.trim();
        if x.chars().count() >= min_len {
            Some(x.chars().take(30).collect())
        } else {
            None
        }
    }
    fn leading_text(dom: &crate::dom::Dom, id: crate::dom::NodeId) -> Option<&str> {
        match dom.children(id).first() {
            Some(&c) => match &dom.node(c).kind {
                crate::dom::NodeKind::Text(t) => Some(t.as_str()),
                _ => None,
            },
            None => None,
        }
    }

    if let Some(d) = check(dom.node(id).attrs.get("title").map(String::as_str), 4) {
        return Some(d);
    }
    if let Some(d) = check(leading_text(dom, id), 4) {
        return Some(d);
    }
    let first_child_elem = dom
        .children(id)
        .into_iter()
        .find(|&c| matches!(dom.node(c).kind, crate::dom::NodeKind::Element(_)));
    if let Some(fc) = first_child_elem {
        if let Some(d) = check(leading_text(dom, fc), 4) {
            return Some(d);
        }
    }
    // "Get full text for tags that have only a few descendants" --
    // upstream's real cutoff (a `for...else` loop breaking once more
    // than 6 descendant elements are seen) is <= 6.
    let descendant_elements = dom.preorder_elements(id).len().saturating_sub(1);
    if descendant_elements <= 6 {
        let full_text = dom.text_content(id);
        if let Some(d) = check(Some(&full_text), 1) {
            return Some(d);
        }
    }
    None
}

/// Port of `create_anchor_map`.
fn create_anchor_map(
    dom: &crate::dom::Dom,
    name: &str,
) -> HashMap<String, (LinkLocation, Option<String>)> {
    let mut ans = HashMap::new();
    for id in dom.preorder_elements(dom.root) {
        let node = dom.node(id);
        let anchor = node
            .attrs
            .get("id")
            .or_else(|| node.attrs.get("name"))
            .cloned();
        if let Some(anchor) = anchor {
            ans.entry(anchor.clone()).or_insert_with(|| {
                let loc = LinkLocation {
                    name: name.to_string(),
                    line_number: node.sourceline,
                    text_on_line: Some(anchor.clone()),
                };
                (loc, description_for_anchor(dom, id))
            });
        }
    }
    ans
}

/// Port of `Anchor`. `id` is `None` only for a link with no `#fragment`
/// at all (`links_data`'s real "no frag" branch passes Python's literal
/// `None`) -- when a fragment IS present but happens to be an empty
/// string (a bare trailing `#`), `id` is `Some(String::new())`, not
/// `None`, matching upstream's own real distinction (it passes `frag`,
/// a plain possibly-empty `str`, directly through in every other
/// branch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub id: Option<String>,
    pub location: Option<LinkLocation>,
    pub text: Option<String>,
}

/// Port of the `Link`-building function (Python names both the
/// namedtuple and the factory function `Link`; this splits them into a
/// [`Link`] struct and a private constructor computing `ok`, the only
/// derived field).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub location: LinkLocation,
    pub text: Option<String>,
    pub is_external: bool,
    pub href: String,
    pub path_ok: bool,
    pub anchor_ok: bool,
    pub anchor: Anchor,
    pub ok: Option<bool>,
}

fn make_link(
    location: LinkLocation,
    text: Option<String>,
    is_external: bool,
    href: String,
    path_ok: bool,
    anchor_ok: bool,
    anchor: Anchor,
) -> Link {
    let ok = if is_external { None } else { Some(path_ok && anchor_ok) };
    Link {
        location,
        text,
        is_external,
        href,
        path_ok,
        anchor_ok,
        anchor,
        ok,
    }
}

/// Port of `links_data`.
pub fn links_data(container: &mut Container) -> Result<Vec<Link>> {
    let mut anchor_map: HashMap<String, HashMap<String, (LinkLocation, Option<String>)>> = HashMap::new();
    let mut raw_links: Vec<(String, String, Option<String>, LinkLocation, Option<String>)> = Vec::new();

    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    for (name, mt) in &names {
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(name)?;
        let dom = container.get_xhtml(name)?;
        anchor_map.insert(name.clone(), create_anchor_map(dom, name));
        for id in dom.preorder_elements(dom.root) {
            if dom.tag(id) != Some("a") {
                continue;
            }
            let Some(href) = dom.node(id).attrs.get("href").cloned() else {
                continue;
            };
            let text = description_for_anchor(dom, id);
            let location = LinkLocation {
                name: name.clone(),
                line_number: dom.node(id).sourceline,
                text_on_line: Some(href.clone()),
            };
            if href.is_empty() {
                raw_links.push((String::new(), String::new(), None, location, text));
                continue;
            }
            let (base, frag) = match href.split_once('#') {
                Some((b, f)) => (b.to_string(), f.to_string()),
                None => (href.clone(), String::new()),
            };
            let dest = if !frag.is_empty() && base.is_empty() {
                Some(name.clone())
            } else {
                safe_href_to_name(container, &href, name)
            };
            raw_links.push((base, frag, dest, location, text));
        }
    }

    let mut links = Vec::with_capacity(raw_links.len());
    for (base, frag, dest, location, text) in raw_links {
        let link = match dest {
            None => make_link(
                location,
                text,
                true,
                base,
                true,
                true,
                Anchor {
                    id: Some(frag),
                    location: None,
                    text: None,
                },
            ),
            Some(dest) if anchor_map.contains_key(&dest) => {
                let loc = LinkLocation {
                    name: dest.clone(),
                    line_number: None,
                    text_on_line: None,
                };
                if !frag.is_empty() {
                    match anchor_map[&dest].get(&frag) {
                        None => make_link(
                            location,
                            text,
                            false,
                            dest,
                            true,
                            false,
                            Anchor {
                                id: Some(frag),
                                location: Some(loc),
                                text: None,
                            },
                        ),
                        Some((a_loc, a_text)) => make_link(
                            location,
                            text,
                            false,
                            dest,
                            true,
                            true,
                            Anchor {
                                id: Some(frag),
                                location: Some(a_loc.clone()),
                                text: a_text.clone(),
                            },
                        ),
                    }
                } else {
                    make_link(
                        location,
                        text,
                        false,
                        dest,
                        true,
                        true,
                        Anchor {
                            id: None,
                            location: Some(loc),
                            text: None,
                        },
                    )
                }
            }
            Some(dest) => make_link(
                location,
                text,
                false,
                dest,
                false,
                false,
                Anchor {
                    id: Some(frag),
                    location: None,
                    text: None,
                },
            ),
        };
        links.push(link);
    }
    Ok(links)
}

// ===================================================================
// css_data (issue #590's last real piece -- needed real line/column
// tracking on crate::css::model, added directly to StyleRule rather
// than worked around here)
// ===================================================================

use crate::css::matcher::{DomElement, Select};
use crate::dom::{Dom, NodeId};

/// Port of `RuleLocation`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RuleLocation {
    pub file_name: String,
    pub line: u32,
    pub column: u32,
}

/// Port of `CSSRule` (the namedtuple `(selector, location)` -- named
/// `CssRuleEntry` here since `Rule` already names something else in
/// `crate::css::model`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CssRuleEntry {
    pub selector: String,
    pub location: RuleLocation,
}

/// One entry of a flattened stylesheet: either a real style rule, or
/// (Python: a plain string appended in place of an `@import`) a
/// resolved, existing import target name to recurse into.
enum CssRuleOrImport {
    Rule(CssRuleEntry),
    Import(String),
}

/// Port of the `css_rules` closure.
fn css_rules(
    container: &Container,
    file_name: &str,
    rules: &[crate::css::Rule],
    sourceline_offset: u32,
) -> Vec<CssRuleOrImport> {
    let mut ans = Vec::new();
    for rule in rules {
        match rule {
            crate::css::Rule::Style(sr) => {
                ans.push(CssRuleOrImport::Rule(CssRuleEntry {
                    selector: sr.selector_text.clone(),
                    location: RuleLocation {
                        file_name: file_name.to_string(),
                        line: sourceline_offset + sr.line,
                        column: sr.column,
                    },
                }));
            }
            crate::css::Rule::Import(imp) => {
                if let Some(import_name) = safe_href_to_name(container, &imp.href, file_name) {
                    if container.exists(&import_name) {
                        ans.push(CssRuleOrImport::Import(import_name));
                    }
                }
            }
            crate::css::Rule::Media(m) => {
                ans.extend(css_rules(container, file_name, &m.rules, sourceline_offset));
            }
            _ => {}
        }
    }
    ans
}

fn rules_in_sheet<'a>(
    sheet: &'a [CssRuleOrImport],
    importable_sheets: &'a HashMap<String, Vec<CssRuleOrImport>>,
    out: &mut Vec<&'a CssRuleEntry>,
) {
    for entry in sheet {
        match entry {
            CssRuleOrImport::Rule(r) => out.push(r),
            CssRuleOrImport::Import(name) => {
                if let Some(isheet) = importable_sheets.get(name) {
                    rules_in_sheet(isheet, importable_sheets, out);
                }
            }
        }
    }
}

/// Port of `sheets_for_html`.
fn sheets_for_html<'a>(
    dom: &Dom,
    container: &Container,
    name: &str,
    importable_sheets: &'a HashMap<String, Vec<CssRuleOrImport>>,
) -> Vec<&'a Vec<CssRuleOrImport>> {
    let mut out = Vec::new();
    for id in dom.preorder_elements(dom.root) {
        if dom.tag(id) != Some("link") {
            continue;
        }
        let Some(href) = dom.node(id).attrs.get("href") else {
            continue;
        };
        let Some(tname) = safe_href_to_name(container, href, name) else {
            continue;
        };
        if let Some(sheet) = importable_sheets.get(&tname) {
            out.push(sheet);
        }
    }
    out
}

/// Port of `tag_text`. Real upstream has a confirmed bug here (its own
/// `tag_text` is missing a final `return ans`, so it only ever returns
/// a non-`None` value the very first time it's called for an
/// attribute-bearing element -- every cache hit, and every first call
/// for an attribute-less element, silently returns `None` instead).
/// This port implements the function correctly (always returns the
/// real tag string, real caching) rather than reproducing that latent
/// bug, matching this project's convention of fixing (and disclosing)
/// confirmed-unintentional upstream defects rather than silently
/// replicating them.
fn tag_text(dom: &Dom, id: NodeId, cache: &mut HashMap<NodeId, String>) -> String {
    if let Some(s) = cache.get(&id) {
        return s.clone();
    }
    let tag = dom.tag(id).unwrap_or("").to_string();
    let attrs = &dom.node(id).attrs;
    let text = if attrs.is_empty() {
        format!("<{tag}>")
    } else {
        let attribs: Vec<String> = attrs
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", crate::xml_util::prepare_string_for_xml(v, true)))
            .collect();
        format!("<{tag} {}>", attribs.join(" "))
    };
    cache.insert(id, text.clone());
    text
}

/// Port of `matches_for_selector`.
fn matches_for_selector(
    dom: &Dom,
    select: &Select<DomElement>,
    cmap: &mut HashMap<String, HashMap<NodeId, Vec<CssRuleEntry>>>,
    rule: &CssRuleEntry,
    tt_cache: &mut HashMap<NodeId, String>,
) -> Vec<MatchLocation> {
    let lsel = rule.selector.to_lowercase();
    let selectors = match crate::css::selector::parse_selector_list(&rule.selector) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let matches = select.matching(&selectors);
    let mut seen: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
    for e in &matches {
        let mut p = Some(e.id);
        while let Some(id) = p {
            if seen.insert(id) {
                if let Some(class_attr) = dom.node(id).attrs.get("class") {
                    for cls in class_attr.split_whitespace() {
                        if lsel.contains(&format!(".{}", cls.to_lowercase())) {
                            cmap.entry(cls.to_string()).or_default().entry(id).or_default().push(rule.clone());
                        }
                    }
                }
            }
            p = dom.parent(id);
        }
    }
    matches
        .iter()
        .map(|e| MatchLocation {
            tag: tag_text(dom, e.id, tt_cache),
            sourceline: dom.node(e.id).sourceline,
        })
        .collect()
}

/// Port of `MatchLocation`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchLocation {
    pub tag: String,
    pub sourceline: Option<u32>,
}

/// Port of `CSSFileMatch`. `sort_key` is not stored as a field (see
/// [`ClassEntry`]'s doc for why) -- `matched_files` is pre-sorted by
/// file name before being returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssFileMatch {
    pub file_name: String,
    pub locations: Vec<MatchLocation>,
}

/// Port of `CSSEntry`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssEntry {
    pub rule: CssRuleEntry,
    pub count: usize,
    pub matched_files: Vec<CssFileMatch>,
}

/// Port of `ClassElement`. `text_on_line` is the element's raw `class`
/// attribute text (matching `elem.get('class')`), not the `href`-style
/// value [`LinkLocation`]'s own field usually carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassElement {
    pub name: String,
    pub line_number: Option<u32>,
    pub text_on_line: String,
    pub tag: String,
    pub matched_rules: Vec<CssRuleEntry>,
}

/// Port of `ClassFileMatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassFileMatch {
    pub file_name: String,
    pub class_elements: Vec<ClassElement>,
}

/// Port of `ClassEntry`. Upstream stores a precomputed
/// `numeric_sort_key(...)` result as a `sort_key` field on this and
/// three sibling namedtuples, so a GUI table sorter doesn't need to
/// recompute it. This port doesn't have a sort-key *value* to store at
/// all -- [`calibre_utils::icu::numeric_strcmp`] is a comparator, not a
/// key producer (the same real ICU4X limitation already disclosed on
/// that function) -- so instead, every `Vec` this module returns is
/// already sorted into the equivalent real order before being handed
/// back, achieving the same observable ordering without an exposed key
/// field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassEntry {
    pub cls: String,
    pub num_of_matches: usize,
    pub matched_files: Vec<ClassFileMatch>,
}

/// Port of `css_data`.
pub fn css_data(container: &mut Container) -> Result<(Vec<ClassEntry>, Vec<CssEntry>)> {
    let spine_names: std::collections::HashSet<String> =
        container.spine_names()?.into_iter().map(|(n, _)| n).collect();

    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    let mut importable_sheets: HashMap<String, Vec<CssRuleOrImport>> = HashMap::new();
    let mut html_sheets: HashMap<String, Vec<Vec<CssRuleOrImport>>> = HashMap::new();

    for (name, mt) in &names {
        if OEB_STYLES.contains(&mt.as_str()) {
            let raw = container.raw_data(name, true)?;
            let text = String::from_utf8_lossy(&raw).into_owned();
            let sheet = crate::css::Stylesheet::parse(&text);
            importable_sheets.insert(name.clone(), css_rules(container, name, &sheet.rules, 0));
        } else if OEB_DOCS.contains(&mt.as_str()) && spine_names.contains(name) {
            container.ensure_parsed(name)?;
            let dom = container.get_xhtml(name)?;
            let mut sheets_here = Vec::new();
            for id in dom.preorder_elements(dom.root) {
                if dom.tag(id) != Some("style") || !super::fonts::style_tag_is_css(dom, id) {
                    continue;
                }
                let text = dom.text_content(id);
                if text.trim().is_empty() {
                    continue;
                }
                let offset = dom.node(id).sourceline.unwrap_or(1).saturating_sub(1);
                let sheet = crate::css::Stylesheet::parse(&text);
                sheets_here.push(css_rules(container, name, &sheet.rules, offset));
            }
            html_sheets.insert(name.clone(), sheets_here);
        }
    }

    let mut tt_cache: HashMap<NodeId, String> = HashMap::new();
    let mut rule_map: HashMap<CssRuleEntry, HashMap<String, Vec<MatchLocation>>> = HashMap::new();
    let mut class_map: HashMap<String, HashMap<String, Vec<ClassElement>>> = HashMap::new();

    let mut html_names: Vec<&String> = html_sheets.keys().collect();
    html_names.sort();
    for name in html_names {
        let inline_sheets = &html_sheets[name];
        let dom = container.get_xhtml(name)?;

        let mut cmap: HashMap<String, HashMap<NodeId, Vec<CssRuleEntry>>> = HashMap::new();
        for id in dom.preorder_elements(dom.root) {
            if let Some(class_attr) = dom.node(id).attrs.get("class") {
                for cls in class_attr.split_whitespace() {
                    cmap.entry(cls.to_string()).or_default().entry(id).or_default();
                }
            }
        }

        let select = Select::for_dom(dom);
        let linked = sheets_for_html(dom, container, name, &importable_sheets);
        for sheet in linked.into_iter().chain(inline_sheets.iter()) {
            let mut flat = Vec::new();
            rules_in_sheet(sheet, &importable_sheets, &mut flat);
            for rule in flat {
                let locs = matches_for_selector(dom, &select, &mut cmap, rule, &mut tt_cache);
                rule_map.entry(rule.clone()).or_default().entry(name.clone()).or_default().extend(locs);
            }
        }

        for (cls, elem_map) in cmap {
            let class_elements = class_map.entry(cls).or_default().entry(name.clone()).or_default();
            for (elem_id, usage) in elem_map {
                let node = dom.node(elem_id);
                class_elements.push(ClassElement {
                    name: name.clone(),
                    line_number: node.sourceline,
                    text_on_line: node.attrs.get("class").cloned().unwrap_or_default(),
                    tag: tag_text(dom, elem_id, &mut tt_cache),
                    matched_rules: usage,
                });
            }
        }
    }

    let mut classes: Vec<ClassEntry> = Vec::new();
    for (cls, name_map) in class_map {
        let mut matched_files: Vec<ClassFileMatch> = name_map
            .into_iter()
            .filter(|(_, elems)| !elems.is_empty())
            .map(|(file_name, class_elements)| ClassFileMatch { file_name, class_elements })
            .collect();
        matched_files.sort_by(|a, b| numeric_strcmp(&a.file_name, &b.file_name));
        let num_of_matches: usize = matched_files
            .iter()
            .map(|cfm| cfm.class_elements.iter().map(|ce| ce.matched_rules.len()).sum::<usize>())
            .sum();
        classes.push(ClassEntry { cls, num_of_matches, matched_files });
    }
    classes.sort_by(|a, b| numeric_strcmp(&a.cls, &b.cls));

    let mut rules: Vec<CssEntry> = Vec::new();
    for (rule, loc_map) in rule_map {
        let mut matched_files: Vec<CssFileMatch> = loc_map
            .into_iter()
            .filter(|(_, locs)| !locs.is_empty())
            .map(|(file_name, locations)| CssFileMatch { file_name, locations })
            .collect();
        matched_files.sort_by(|a, b| numeric_strcmp(&a.file_name, &b.file_name));
        let count: usize = matched_files.iter().map(|fm| fm.locations.len()).sum();
        rules.push(CssEntry { rule, count, matched_files });
    }
    rules.sort_by(|a, b| numeric_strcmp(&a.rule.selector, &b.rule.selector));

    Ok((classes, rules))
}

// ===================================================================
// gather_data
// ===================================================================

/// Port of `gather_data`'s `data` dict. Python's `css_data` sets
/// `result_data['classes']` as a side effect on the same dict it also
/// returns rules into; this port's `css_data` just returns both
/// directly (see its own doc), so `classes`/`css` land here without
/// needing a shared mutable map threaded through every call.
#[derive(Debug, Clone)]
pub struct ReportData {
    pub chars: Vec<CharEntry>,
    pub images: Vec<ImageEntry>,
    pub links: Vec<Link>,
    pub word_count: usize,
    pub words: Vec<WordEntry>,
    pub classes: Vec<ClassEntry>,
    pub css: Vec<CssEntry>,
    pub files: Vec<FileEntry>,
}

/// Port of `gather_data`. Real per-section wall-clock timing (Python's
/// own `timing` dict, a GUI progress-display diagnostic), in the same
/// real order upstream calls the six `*_data` functions in --
/// `files_data` runs last so it can see the just-computed
/// per-file word counts.
pub fn gather_data(
    container: &mut Container,
    book_locale: &DictionaryLocale,
) -> Result<(ReportData, HashMap<String, std::time::Duration>)> {
    let mut timing = HashMap::new();

    let t = std::time::Instant::now();
    let chars = chars_data(container, book_locale)?;
    timing.insert("chars".to_string(), t.elapsed());

    let t = std::time::Instant::now();
    let images = images_data(container)?;
    timing.insert("images".to_string(), t.elapsed());

    let t = std::time::Instant::now();
    let links = links_data(container)?;
    timing.insert("links".to_string(), t.elapsed());

    let t = std::time::Instant::now();
    let (word_count, words, file_word_counts) = words_data(container, book_locale)?;
    timing.insert("words".to_string(), t.elapsed());

    let t = std::time::Instant::now();
    let (classes, css) = css_data(container)?;
    timing.insert("css".to_string(), t.elapsed());

    let t = std::time::Instant::now();
    let files = files_data(container, Some(&file_word_counts));
    timing.insert("files".to_string(), t.elapsed());

    Ok((
        ReportData { chars, images, links, word_count, words, classes, css, files },
        timing,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spell::DictionaryLocale as SpellLocale;
    use std::fs;

    #[test]
    fn get_category_covers_images_fonts_styles_text_and_extension_overrides() {
        assert_eq!(get_category("cover.jpg", "image/jpeg"), "image");
        assert_eq!(get_category("style.css", "text/css"), "style");
        assert_eq!(get_category("chapter1.xhtml", "application/xhtml+xml"), "text");
        assert_eq!(get_category("weird.ttf", "application/octet-stream"), "font");
        assert_eq!(get_category("content.opf", "application/oebps-package+xml"), "opf");
        assert_eq!(get_category("toc.ncx", "application/x-dtbncx+xml"), "toc");
        assert_eq!(get_category("README", "text/plain"), "misc");
    }

    fn make_container(files: &[(&str, &str, &[u8])]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        let mut spine_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#));
            if mt.contains("xhtml") {
                spine_items.push_str(&format!(r#"<itemref idref="{name}"/>"#));
            }
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine>{spine_items}</spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn files_data_lists_every_manifest_item_with_category_and_size() {
        let (_dir, container) = make_container(&[
            ("chapter1.xhtml", "application/xhtml+xml", b"<html><body>hi</body></html>"),
            ("style.css", "text/css", b"body { color: red }"),
        ]);
        let files = files_data(&container, None);
        // Plus the container's own OPF file.
        assert_eq!(files.len(), 3);
        let chapter = files.iter().find(|f| f.name == "chapter1.xhtml").unwrap();
        assert_eq!(chapter.category, "text");
        assert_eq!(chapter.dir, "");
        assert_eq!(chapter.basename, "chapter1.xhtml");
        assert!(chapter.size > 0);
        assert_eq!(chapter.word_count, -1);
    }

    #[test]
    fn files_data_uses_the_provided_word_counts() {
        let (_dir, container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body>hi</body></html>",
        )]);
        let mut counts = HashMap::new();
        counts.insert("chapter1.xhtml".to_string(), 5usize);
        let files = files_data(&container, Some(&counts));
        assert_eq!(files[0].word_count, 5);
    }

    #[test]
    fn images_data_finds_a_linked_image_and_its_dimensions() {
        // A minimal, real 1x1 PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00,
            0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00,
            0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01,
            0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
            0x82,
        ];
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><body><img src=\"cover.png\"/></body></html>",
            ),
            ("cover.png", "image/png", png),
        ]);
        let images = images_data(&mut container).unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].name, "cover.png");
        assert_eq!(images[0].width, 1);
        assert_eq!(images[0].height, 1);
        assert_eq!(images[0].usage.len(), 1);
        assert_eq!(images[0].usage[0].name, "chapter1.xhtml");
    }

    #[test]
    fn words_and_chars_data_walk_real_html_content() {
        let (_dir, mut container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body><p>hello world</p></body></html>",
        )]);
        let locale = SpellLocale::new("en", Some("US".to_string()));
        let (count, words, file_word_counts) = words_data(&mut container, &locale).unwrap();
        // 2 from the chapter's own text, plus the OPF's own dc:title ("T").
        assert_eq!(count, 3);
        let words_set: std::collections::HashSet<&str> = words.iter().map(|w| w.word.as_str()).collect();
        assert!(words_set.contains("hello"));
        assert!(words_set.contains("world"));
        assert_eq!(file_word_counts.get("chapter1.xhtml"), Some(&2));

        let chars = chars_data(&mut container, &locale).unwrap();
        let h = chars.iter().find(|c| c.ch == 'h').unwrap();
        assert!(h.count >= 1);
        assert!(h.usage.contains(&"chapter1.xhtml".to_string()));
    }

    #[test]
    fn links_data_resolves_a_real_internal_anchor() {
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><body><a href=\"chapter2.xhtml#sec1\">Go</a></body></html>",
            ),
            (
                "chapter2.xhtml",
                "application/xhtml+xml",
                b"<html><body><p id=\"sec1\" title=\"Section One Longer Title\">text</p></body></html>",
            ),
        ]);
        let links = links_data(&mut container).unwrap();
        let link = links.iter().find(|l| l.location.name == "chapter1.xhtml").unwrap();
        assert!(!link.is_external);
        assert!(link.path_ok);
        assert!(link.anchor_ok);
        assert_eq!(link.ok, Some(true));
        assert_eq!(link.anchor.id.as_deref(), Some("sec1"));
        assert_eq!(link.anchor.text.as_deref(), Some("Section One Longer Title"));
        assert_eq!(link.anchor.location.as_ref().unwrap().name, "chapter2.xhtml");
    }

    #[test]
    fn links_data_flags_a_missing_anchor_but_a_valid_path() {
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><body><a href=\"chapter2.xhtml#nope\">Go</a></body></html>",
            ),
            ("chapter2.xhtml", "application/xhtml+xml", b"<html><body><p>text</p></body></html>"),
        ]);
        let links = links_data(&mut container).unwrap();
        let link = links.iter().find(|l| l.location.name == "chapter1.xhtml").unwrap();
        assert!(link.path_ok);
        assert!(!link.anchor_ok);
        assert_eq!(link.ok, Some(false));
    }

    #[test]
    fn links_data_flags_a_broken_path() {
        let (_dir, mut container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body><a href=\"does-not-exist.xhtml\">Go</a></body></html>",
        )]);
        let links = links_data(&mut container).unwrap();
        let link = links.iter().find(|l| l.location.name == "chapter1.xhtml").unwrap();
        assert!(!link.is_external);
        assert!(!link.path_ok);
        assert!(!link.anchor_ok);
        assert_eq!(link.ok, Some(false));
    }

    #[test]
    fn links_data_marks_an_absolute_url_as_external() {
        let (_dir, mut container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><body><a href=\"https://example.com/x\">Go</a></body></html>",
        )]);
        let links = links_data(&mut container).unwrap();
        let link = links.iter().find(|l| l.location.name == "chapter1.xhtml").unwrap();
        assert!(link.is_external);
        assert_eq!(link.ok, None);
        assert_eq!(link.href, "https://example.com/x");
    }

    #[test]
    fn create_anchor_map_uses_the_title_attribute_for_description() {
        let dom = crate::dom::Dom::parse("<html><body><p id=\"a\" title=\"A real heading here\">x</p></body></html>");
        let map = create_anchor_map(&dom, "chapter1.xhtml");
        let (loc, text) = map.get("a").unwrap();
        assert_eq!(loc.name, "chapter1.xhtml");
        assert!(loc.line_number.is_some());
        assert_eq!(text.as_deref(), Some("A real heading here"));
    }

    #[test]
    fn description_for_anchor_falls_back_to_leading_text() {
        let dom = crate::dom::Dom::parse("<html><body><a id=\"a\">Some real link text</a></body></html>");
        let id = dom.find_by_id("a").unwrap();
        assert_eq!(description_for_anchor(&dom, id).as_deref(), Some("Some real link text"));
    }

    #[test]
    fn css_data_matches_a_linked_stylesheet_rule_against_its_real_selector() {
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><head><link rel=\"stylesheet\" href=\"style.css\"/></head><body><p class=\"intro highlight\">Hello</p></body></html>",
            ),
            ("style.css", "text/css", b".intro { color: red }\n.unused { color: blue }\n"),
        ]);
        let (classes, rules) = css_data(&mut container).unwrap();

        let intro_rule = rules.iter().find(|r| r.rule.selector == ".intro").unwrap();
        assert_eq!(intro_rule.count, 1);
        assert_eq!(intro_rule.rule.location.file_name, "style.css");
        assert_eq!(intro_rule.rule.location.line, 1);
        let file_match = &intro_rule.matched_files[0];
        assert_eq!(file_match.file_name, "chapter1.xhtml");
        assert_eq!(file_match.locations[0].tag, "<p class=\"intro highlight\">");

        // A rule that matches nothing still shows up, with zero real matches.
        let unused_rule = rules.iter().find(|r| r.rule.selector == ".unused").unwrap();
        assert_eq!(unused_rule.count, 0);
        assert!(unused_rule.matched_files.is_empty());

        // "intro" is used by a real rule; "highlight" is a real class on
        // the element with no rule targeting it at all.
        let intro_class = classes.iter().find(|c| c.cls == "intro").unwrap();
        assert_eq!(intro_class.num_of_matches, 1);
        let highlight_class = classes.iter().find(|c| c.cls == "highlight").unwrap();
        assert_eq!(highlight_class.num_of_matches, 0);
    }

    #[test]
    fn css_data_matches_an_inline_style_tag_rule() {
        let (_dir, mut container) = make_container(&[(
            "chapter1.xhtml",
            "application/xhtml+xml",
            b"<html><head><style>.foo { color: green }</style></head><body><p class=\"foo\">Hi</p></body></html>",
        )]);
        let (_classes, rules) = css_data(&mut container).unwrap();
        let foo_rule = rules.iter().find(|r| r.rule.selector == ".foo").unwrap();
        assert_eq!(foo_rule.count, 1);
        assert_eq!(foo_rule.rule.location.file_name, "chapter1.xhtml");
    }

    #[test]
    fn css_data_follows_an_import_chain_to_a_matching_rule() {
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><head><link rel=\"stylesheet\" href=\"main.css\"/></head><body><p class=\"bar\">Hi</p></body></html>",
            ),
            ("main.css", "text/css", b"@import url(base.css);\n"),
            ("base.css", "text/css", b".bar { color: purple }\n"),
        ]);
        let (_classes, rules) = css_data(&mut container).unwrap();
        let bar_rule = rules.iter().find(|r| r.rule.selector == ".bar").unwrap();
        assert_eq!(bar_rule.count, 1);
        assert_eq!(bar_rule.rule.location.file_name, "base.css");
    }

    #[test]
    fn gather_data_orchestrates_all_six_real_sections() {
        let (_dir, mut container) = make_container(&[
            (
                "chapter1.xhtml",
                "application/xhtml+xml",
                b"<html><head><link rel=\"stylesheet\" href=\"style.css\"/></head><body><p class=\"intro\">hello world</p></body></html>",
            ),
            ("style.css", "text/css", b".intro { color: red }\n"),
        ]);
        let locale = SpellLocale::new("en", Some("US".to_string()));
        let (data, timing) = gather_data(&mut container, &locale).unwrap();

        assert!(!data.chars.is_empty());
        assert!(data.images.is_empty());
        assert!(data.links.is_empty());
        assert!(data.words.iter().any(|w| w.word == "hello"));
        assert!(data.classes.iter().any(|c| c.cls == "intro" && c.num_of_matches == 1));
        assert!(data.css.iter().any(|r| r.rule.selector == ".intro" && r.count == 1));
        // files_data ran last and can see the just-computed word counts.
        let chapter_file = data.files.iter().find(|f| f.name == "chapter1.xhtml").unwrap();
        assert_eq!(chapter_file.word_count, 2);

        for key in ["chars", "images", "links", "words", "css", "files"] {
            assert!(timing.contains_key(key), "missing timing for {key}");
        }
    }
}
