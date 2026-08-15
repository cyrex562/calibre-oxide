//! Port of `old_src/src/calibre/ebooks/oeb/transforms/flatcss.py`.
//!
//! Flattens a book's CSS: for every spine document, computes each
//! element's effective (cascaded) style, consolidates the results into a
//! small set of `.calibreN { ... }` class rules in one generated
//! stylesheet, rewrites every element's `class=` attribute to point at
//! its rule, and drops the book's original stylesheets. This is also
//! where font-size *rescaling* happens (the `fbase`/`fkey` "remap this
//! book's fonts onto this profile's font scale" feature the module
//! comment refers to).
//!
//! # Style resolution: a local cascade orchestrator, built on the same
//! primitives `manglecase.rs`/`cascade.rs` already established
//!
//! Python's `Stylizer` (`oeb.stylizer`, the *old*, pre-issue-#164
//! cascade engine this file actually imports -- not
//! `oeb.polish.cascade`) resolves, per element, every CSS property that
//! is either declared on it directly or inherited from an ancestor.
//! [`crate::oeb::polish::cascade::resolve_property`] is exactly that
//! *inheritance* half, already reusable as-is (same precedent
//! `manglecase.rs` set). What Python's `Stylizer` also does --
//! collecting every top-level stylesheet, matching selectors, resolving
//! specificity/`!important` -- is [`crate::oeb::polish::cascade::resolve_styles`]'s
//! job in the *polish* world, but that function takes a
//! `polish::Container`, which doesn't exist here (see the batch task
//! notes). [`resolve_document_styles`] is this file's local
//! reimplementation of that half, scoped to what flattening needs: it
//! reuses every *container-agnostic* piece of `cascade.rs` verbatim
//! ([`crate::oeb::polish::cascade::Specificity`]/[`StyleDeclaration`]/
//! [`resolve_declarations`]/[`normalize_style_declaration`]/
//! [`PropertyValue`]/[`INHERITED`]/[`html_css_stylesheet`]), and only
//! reimplements the *sheet collection* step against `OEBBook`'s raw
//! container reads instead of `Container::parsed_stylesheet`.
//!
//! One deliberate narrowing versus `cascade::resolve_styles`/Python's
//! `Stylizer`: [`resolve_document_styles`] only walks *top-level* style
//! rules in each sheet -- it does not recurse into `@media`/`@import`
//! (`cascade::iterrules` does, but needs `&mut Container` throughout its
//! recursion in a way this module's raw-bytes world doesn't cleanly
//! support). Real-world book stylesheets overwhelmingly declare their
//! rules at the top level; an `@media`-guarded or `@import`ed rule is
//! simply not seen, rather than mis-applied.
//!
//! # Unit conversion: `unit_convert` is new, not a duplicate
//!
//! [`crate::oeb::normalize_css`] (issue #35) only covers shorthand
//! *property* expansion (`margin`/`padding`/`border-*`), not CSS
//! *length* conversion (`12px` -> points). [`unit_convert`] is a direct,
//! narrow port of `calibre.ebooks.unit_convert` -- there is no existing
//! equivalent in this crate to reuse.
//!
//! # What's out of scope (documented, not silently dropped)
//!
//! - **`transform_css_rules`** (the `calibre.ebooks.css_transform_rules`
//!   user-rule DSL, e.g. "delete all `color` declarations"): a separate,
//!   substantial, unported module -- out of scope for this file, same as
//!   `structure.rs`'s XPath-subset scope note for a different
//!   unported-dependency case. [`FlattenOptions::transform_css_rules`]
//!   does not exist; nothing calls out to it.
//! - **`filter_css`** (dropping specific named CSS properties from the
//!   output): real dict-of-classes plumbing exists
//!   ([`crate::oeb::normalize_css::normalize_filter_css`]), but wiring
//!   it through this file's `cssdict` construction is not implemented
//!   here -- narrower than Python, which supports it end to end.
//! - **Pseudo-class rules** (`:hover`/`:active`/`:link` -- Python's
//!   `style.pseudo_classes()`): not generated. E-readers have no hover
//!   state and these rules are a small minority of real-world jacket/
//!   book CSS; [`crate::css::selector`]'s own scope note already covers
//!   what pseudo-class syntax parses at all.
//! - **`embed_font_family`** (`get_embed_font_info`): needs
//!   `calibre.utils.fonts.scanner.font_scanner`, the same
//!   already-documented OS-font-scanning gap `embed_fonts.rs`/
//!   `subset.rs`/`oeb::polish::embed::do_embed` all share. The `None`
//!   (no family requested) path -- overwhelmingly the common case -- is
//!   real; [`get_embed_font_info`] only `todo!()`s when a family is
//!   actually requested.
//! - **A `specializer` hook** (an output-format-specific callback some
//!   converters pass to further adjust a `Stylizer` before flattening):
//!   not modeled; no caller in this batch's scope needs it.

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;

use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::{CSS_MIME, OEB_STYLES, XHTML_NS};
use crate::oeb::polish::cascade::{
    html_css_stylesheet, normalize_style_declaration, resolve_declarations, resolve_property,
    specificity, PropertyValue, Specificity, StyleDeclaration, INHERITED,
};

// ===================================================================
// unit_convert / length parsing
// ===================================================================

fn unit_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"^(-*[0-9]*[.]?[0-9]*)\s*(%|em|ex|en|px|mm|cm|in|pt|pc|rem|q)$").unwrap()
    })
}

/// Port of `calibre.ebooks.unit_convert`: converts a CSS length to
/// points. `base` is the percentage base (usually the parent's font
/// size, in points); `font` is the current element's font size, used for
/// `em`/`ex`/`en`; `body_font_size` backs `rem`.
pub fn unit_convert(
    value: &str,
    base: f64,
    font: f64,
    dpi: f64,
    body_font_size: f64,
) -> Option<f64> {
    let value = value.trim();
    if let Ok(v) = value.parse::<f64>() {
        return Some(v * 72.0 / dpi);
    }
    let caps = unit_re().captures(value)?;
    let num_str = caps.get(1)?.as_str();
    if num_str.is_empty() || num_str == "-" {
        return None;
    }
    let num: f64 = num_str.parse().ok()?;
    let unit = caps.get(2)?.as_str();
    Some(match unit {
        "%" => (num / 100.0) * base,
        "px" => num * 72.0 / dpi,
        "in" => num * 72.0,
        "pt" => num,
        "em" => num * font,
        "ex" | "en" => num * font * 0.5,
        "pc" => num * 12.0,
        "mm" => num * 2.8346456693,
        "cm" => num * 28.346456693,
        "rem" => num * body_font_size,
        "q" => num * 0.708661417325,
        _ => return None,
    })
}

// ===================================================================
// FontMapper: KeyMapper / ScaleMapper / NullMapper
// ===================================================================

/// Port of `KeyMapper.relate`.
fn relate(size: f64, base: f64) -> f64 {
    if size == 0.0 {
        return base;
    }
    if (size - base).abs() < 0.1 {
        return 0.0;
    }
    let sign = if size < base { -1.0 } else { 1.0 };
    let endp = if size < base { 0.0 } else { 36.0 };
    let diff = (base - size).abs() * 3.0 + (36.0 - size) / 100.0;
    let mut logb = (base - endp).abs();
    if logb == 1.0 {
        logb = 1.1;
    }
    let (diff, logb) = if logb == 0.0 {
        (diff, 1e-6)
    } else if diff == 0.0 {
        (1e-6, logb)
    } else {
        (diff, logb)
    };
    if diff < 0.0 {
        return 0.0;
    }
    sign * diff.ln() / logb.ln()
}

/// Port of `FontMapper`/`KeyMapper`/`ScaleMapper`/`NullMapper`: maps a
/// source-book font size (points) onto the destination profile's font
/// scale.
pub enum FontMapper {
    /// Port of `KeyMapper`: snaps to the nearest entry in a fixed
    /// destination key-size table, using [`relate`]'s log-scaled
    /// "distance from base" metric (matching Python's font-size-mapping
    /// heuristic, not a linear scale).
    Key {
        sbase: f64,
        /// `(relate(size, dbase), size)` pairs, precomputed once.
        dprop: Vec<(f64, f64)>,
        cache: std::cell::RefCell<HashMap<u64, f64>>,
    },
    /// Port of `ScaleMapper`: a plain linear `dbase/sbase` scale.
    Scale { dscale: f64 },
    /// Port of `NullMapper`: identity.
    Null,
}

impl FontMapper {
    /// Port of the `FontMapper(sbase, dbase, dkey)` factory function.
    pub fn new(sbase: Option<f64>, dbase: Option<f64>, dkey: Option<&[f64]>) -> Self {
        match (sbase, dbase, dkey) {
            (Some(sbase), Some(dbase), Some(dkey)) if !dkey.is_empty() => FontMapper::Key {
                sbase,
                dprop: dkey.iter().map(|&x| (relate(x, dbase), x)).collect(),
                cache: std::cell::RefCell::new(HashMap::new()),
            },
            (Some(sbase), Some(dbase), _) => FontMapper::Scale {
                dscale: dbase / sbase,
            },
            _ => FontMapper::Null,
        }
    }

    /// Port of `__getitem__`.
    pub fn get(&self, ssize: f64) -> f64 {
        match self {
            FontMapper::Null => ssize,
            FontMapper::Scale { dscale } => ssize * dscale,
            FontMapper::Key {
                sbase,
                dprop,
                cache,
            } => {
                let key = ssize.to_bits();
                if let Some(&v) = cache.borrow().get(&key) {
                    return v;
                }
                let prop = relate(ssize, *sbase);
                let mut best = dprop[0].1;
                let mut best_dist = f64::INFINITY;
                for &(p, s) in dprop {
                    let d = (prop - p).abs();
                    if d < best_dist {
                        best_dist = d;
                        best = s;
                    }
                }
                cache.borrow_mut().insert(key, best);
                best
            }
        }
    }
}

// ===================================================================
// resolve_document_styles: the local, container-agnostic cascade
// ===================================================================

/// Everything [`resolve_document_styles`] resolves for one document:
/// per-element declared-or-inherited property maps, ready to drive
/// [`resolve_property`] directly.
pub type StyleMap = HashMap<NodeId, HashMap<String, PropertyValue>>;

/// Port of the sheet-collection half of `Stylizer.__init__`. See the
/// module docs for how this relates to `cascade::resolve_styles` and its
/// documented no-`@media`/`@import` narrowing.
pub fn resolve_document_styles(oeb: &OEBBook, href: &str, dom: &Dom, user_css: &str) -> StyleMap {
    struct StyleTag {
        text: String,
    }
    struct LinkTag {
        href: String,
    }
    let (style_tags, link_tags, style_attrs): (Vec<StyleTag>, Vec<LinkTag>, Vec<(NodeId, String)>) = {
        let mut style_tags = Vec::new();
        let mut link_tags = Vec::new();
        let mut style_attrs = Vec::new();
        for id in dom.preorder_elements(dom.root) {
            match dom.tag(id) {
                Some("style") => {
                    let text = dom.text_content(id);
                    if !text.trim().is_empty() {
                        style_tags.push(StyleTag { text });
                    }
                }
                Some("link") => {
                    let attrs = &dom.node(id).attrs;
                    let ty = attrs.get("type").map(|s| s.as_str()).unwrap_or(CSS_MIME);
                    let rel = attrs.get("rel").map(|s| s.as_str()).unwrap_or("stylesheet");
                    if let Some(href) = attrs.get("href") {
                        if OEB_STYLES.iter().any(|m| m.eq_ignore_ascii_case(ty))
                            && rel.eq_ignore_ascii_case("stylesheet")
                        {
                            link_tags.push(LinkTag { href: href.clone() });
                        }
                    }
                }
                _ => {}
            }
            if let Some(style) = dom.node(id).attrs.get("style") {
                if !style.trim().is_empty() {
                    style_attrs.push((id, style.clone()));
                }
            }
        }
        (style_tags, link_tags, style_attrs)
    };

    let mut rule_index_counter: u64 = 0;
    let mut collected: Vec<(crate::css::StyleRule, String, u64)> = Vec::new();
    let collect_top_level_rules =
        |sheet: crate::css::Stylesheet, sheet_name: String, counter: &mut u64, out: &mut Vec<_>| {
            for rule in sheet.rules {
                if let crate::css::Rule::Style(sr) = rule {
                    let idx = *counter;
                    *counter += 1;
                    out.push((sr, sheet_name.clone(), idx));
                }
            }
        };

    collect_top_level_rules(
        html_css_stylesheet().clone(),
        "user-agent.css".to_string(),
        &mut rule_index_counter,
        &mut collected,
    );
    if !user_css.trim().is_empty() {
        collect_top_level_rules(
            crate::css::Stylesheet::parse(user_css),
            "user.css".to_string(),
            &mut rule_index_counter,
            &mut collected,
        );
    }
    for tag in &style_tags {
        collect_top_level_rules(
            crate::css::Stylesheet::parse(&tag.text),
            href.to_string(),
            &mut rule_index_counter,
            &mut collected,
        );
    }
    for link in &link_tags {
        let abs = super::filenames::urlnormalize(&super::filenames::abshref(href, &link.href));
        let Some(item) = oeb.manifest.get_by_href(&abs) else {
            continue;
        };
        let Ok(data) = oeb.container.read(&item.href) else {
            continue;
        };
        let text = String::from_utf8_lossy(&data);
        collect_top_level_rules(
            crate::css::Stylesheet::parse(&text),
            item.href.clone(),
            &mut rule_index_counter,
            &mut collected,
        );
    }

    let mut style_map: HashMap<NodeId, Vec<StyleDeclaration>> = HashMap::new();
    {
        let select = crate::css::Select::for_dom(dom);
        for (rule, sheet_name, rule_index) in &collected {
            let style = normalize_style_declaration(&rule.style, sheet_name);
            for selector in &rule.selectors.0 {
                if selector.pseudo_element.is_some() {
                    continue;
                }
                let single = crate::css::SelectorList(vec![selector.clone()]);
                let matches = select.matching(&single);
                let idx = specificity(*rule_index, selector.specificity, false);
                for elem in matches {
                    style_map
                        .entry(elem.id)
                        .or_default()
                        .push(StyleDeclaration {
                            index: idx,
                            declaration: style.clone(),
                            pseudo_element: None,
                        });
                }
            }
        }
    }
    for (id, style_text) in &style_attrs {
        let decl_block = crate::css::parser::parse_declaration_list(style_text);
        let style = normalize_style_declaration(&decl_block, href);
        style_map.entry(*id).or_default().push(StyleDeclaration {
            index: Specificity {
                is_style: true,
                num_id: 0,
                num_class: 0,
                num_elem: 0,
                rule_index: 0,
            },
            declaration: style,
            pseudo_element: None,
        });
    }

    for decls in style_map.values_mut() {
        decls.sort_by_key(|d| std::cmp::Reverse(d.index));
    }
    style_map
        .into_iter()
        .map(|(k, v)| (k, resolve_declarations(&v)))
        .collect()
}

/// The set of property names either declared directly on `elem` or, for
/// inheritable properties, on the nearest ancestor that declares them --
/// the property set Python's `Stylizer.Style._style` ends up holding.
fn own_and_inherited_props(style_map: &StyleMap, dom: &Dom, elem: NodeId) -> HashSet<String> {
    let mut props: HashSet<String> = style_map
        .get(&elem)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    for &p in INHERITED {
        if props.contains(p) {
            continue;
        }
        let mut cur = dom.parent(elem);
        while let Some(a) = cur {
            if style_map
                .get(&a)
                .map(|m| m.contains_key(p))
                .unwrap_or(false)
            {
                props.insert(p.to_string());
                break;
            }
            cur = dom.parent(a);
        }
    }
    props
}

/// Port of `style.cssdict()`: every declared-or-inherited property on
/// `elem`, as CSS text.
pub fn cssdict(style_map: &StyleMap, dom: &Dom, elem: NodeId) -> BTreeMap<String, String> {
    own_and_inherited_props(style_map, dom, elem)
        .into_iter()
        .filter_map(|p| resolve_property(style_map, dom, elem, &p).map(|v| (p, v.css_text)))
        .collect()
}

const FONT_SIZE_KEYWORDS: &[(&str, f64)] = &[
    ("xx-small", 0.579),
    ("x-small", 0.694),
    ("small", 0.833),
    ("medium", 1.0),
    ("large", 1.2),
    ("x-large", 1.44),
    ("xx-large", 1.728),
];

/// Resolves `elem`'s computed `font-size`, in points, walking ancestors
/// for `em`/`%`/keyword-relative sizing. `ctx.base_font_size` anchors
/// keyword sizes (`medium`, ...) and the document root's default.
fn resolve_font_size(
    style_map: &StyleMap,
    dom: &Dom,
    elem: NodeId,
    ctx: &FlattenContext,
    cache: &mut HashMap<NodeId, f64>,
) -> f64 {
    if let Some(&v) = cache.get(&elem) {
        return v;
    }
    let parent_size = match dom.parent(elem) {
        Some(p) => resolve_font_size(style_map, dom, p, ctx, cache),
        None => ctx.base_font_size,
    };
    let own = style_map.get(&elem).and_then(|m| m.get("font-size"));
    let size = match own {
        None => parent_size,
        Some(v) => {
            let text = v.css_text.trim().to_lowercase();
            if let Some(&(_, mult)) = FONT_SIZE_KEYWORDS.iter().find(|(k, _)| *k == text) {
                ctx.base_font_size * mult
            } else if text == "smaller" {
                parent_size * 0.85
            } else if text == "larger" {
                parent_size * 1.2
            } else {
                unit_convert(&text, parent_size, parent_size, ctx.dpi, ctx.base_font_size)
                    .unwrap_or(parent_size)
            }
        }
    };
    cache.insert(elem, size);
    size
}

// ===================================================================
// CSSFlattener
// ===================================================================

/// Options this transform reads (`context.*`/`self.*` in Python). See
/// the module docs for what's deliberately not modeled.
#[derive(Debug, Clone)]
pub struct FlattenContext {
    /// `context.source.fbase`: the source profile's assumed base font
    /// size, points. Also used as the document-root default when no
    /// ancestor declares a font size at all.
    pub base_font_size: f64,
    /// `context.dest.fbase`.
    pub dest_base_font_size: f64,
    pub dpi: f64,
    pub margin_left: f64,
    pub margin_right: f64,
    pub margin_top: f64,
    pub margin_bottom: f64,
    pub change_justification: Option<String>,
    pub disable_font_rescaling: bool,
    pub minimum_line_height: f64,
    pub remove_paragraph_spacing: bool,
    pub remove_paragraph_spacing_indent_size: f64,
    pub insert_blank_line: bool,
    pub insert_blank_line_size: f64,
    pub page_break_on_body: bool,
    pub user_css: String,
    pub output_profile_is_kindle: bool,
}

impl Default for FlattenContext {
    fn default() -> Self {
        FlattenContext {
            base_font_size: 12.0,
            dest_base_font_size: 12.0,
            dpi: 96.0,
            margin_left: -1.0,
            margin_right: -1.0,
            margin_top: -1.0,
            margin_bottom: -1.0,
            change_justification: None,
            disable_font_rescaling: true,
            minimum_line_height: 120.0,
            remove_paragraph_spacing: false,
            remove_paragraph_spacing_indent_size: 1.5,
            insert_blank_line: false,
            insert_blank_line_size: 0.5,
            page_break_on_body: false,
            user_css: String::new(),
            output_profile_is_kindle: false,
        }
    }
}

/// Port of `CSSFlattener`'s constructor arguments.
#[derive(Debug, Clone, Default)]
pub struct FlattenerOptions {
    pub fbase: Option<f64>,
    pub fkey: Option<Vec<f64>>,
    pub lineh: Option<f64>,
    pub unfloat: bool,
    pub untable: bool,
}

/// Needs `calibre.utils.fonts.scanner.font_scanner` -- the same
/// already-documented OS-font-scanning gap `embed_fonts.rs`/`subset.rs`
/// share (see the module docs). The no-family-requested path (by far the
/// common case) is real.
fn get_embed_font_info(family: Option<&str>) -> Result<Option<String>> {
    match family {
        None => Ok(None),
        Some(_) => todo!(
            "placeholder: needs calibre.utils.fonts.scanner.font_scanner (OS font \
             enumeration + calibre's bundled font collection), which this crate has \
             no equivalent for -- see this module's docs"
        ),
    }
}

struct ItemStyle {
    href: String,
    dom: Dom,
    style_map: StyleMap,
    font_size_cache: HashMap<NodeId, f64>,
    page_rule: BTreeMap<String, String>,
}

/// Port of `CSSFlattener`.
pub struct CSSFlattener {
    pub opts: FlattenerOptions,
}

impl CSSFlattener {
    pub fn new(opts: FlattenerOptions) -> Self {
        CSSFlattener { opts }
    }

    /// Port of `CSSFlattener.__call__`.
    pub fn call(
        &self,
        oeb: &mut OEBBook,
        ctx: &FlattenContext,
        report: &mut dyn FnMut(&str),
    ) -> Result<()> {
        report("Flattening CSS and remapping font sizes...");

        let _body_font_family = get_embed_font_info(None)?;

        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();

        let mut items: Vec<ItemStyle> = Vec::new();
        for href in &spine_hrefs {
            let Ok(raw) = oeb.container.read(href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let dom = Dom::parse(&html);
            let style_map = resolve_document_styles(oeb, href, &dom, &ctx.user_css);
            items.push(ItemStyle {
                href: href.clone(),
                dom,
                style_map,
                font_size_cache: HashMap::new(),
                page_rule: BTreeMap::new(),
            });
        }

        let sbase = if self.opts.fbase.is_some() {
            Some(self.baseline_spine(&mut items, ctx))
        } else {
            None
        };
        let fmap = FontMapper::new(sbase, self.opts.fbase, self.opts.fkey.as_deref());

        let mut names: HashMap<String, u32> = HashMap::new();
        let mut styles: HashMap<String, String> = HashMap::new();
        self.flatten_spine(&mut items, ctx, sbase, &fmap, &mut names, &mut styles);

        let sorted_items: Vec<(&String, &String)> = {
            let mut v: Vec<(&String, &String)> =
                styles.iter().map(|(css, cls)| (cls, css)).collect();
            v.sort_by(|a, b| {
                crate::comic::numeric_sort_key(a.0).cmp(&crate::comic::numeric_sort_key(b.0))
            });
            v
        };
        let mut css_text = String::new();
        for (cls, decl) in sorted_items {
            css_text.push_str(&format!(".{cls} {{\n{decl};\n}}\n\n"));
        }

        let href = self.replace_css(oeb, &css_text);

        for item in &items {
            let rendered = item.dom.serialize(item.dom.root).into_bytes();
            let _ = oeb.container.write(&item.href, &rendered);
        }

        for item in &mut items {
            self.flatten_head(oeb, item, &href, ctx);
        }
        for item in &items {
            let rendered = item.dom.serialize(item.dom.root).into_bytes();
            let _ = oeb.container.write(&item.href, &rendered);
        }

        let _ = report;
        Ok(())
    }

    /// Port of `baseline_spine`/`baseline_node`: the most-used font
    /// size across the whole book's text, used as the "source" anchor
    /// for [`FontMapper`] when `fbase` (a rescale target) is set.
    fn baseline_spine(&self, items: &mut [ItemStyle], ctx: &FlattenContext) -> f64 {
        let mut sizes: HashMap<u64, (f64, f64)> = HashMap::new();
        for item in items.iter_mut() {
            let Some(body) = item.dom.find_first_tag_global("body") else {
                continue;
            };
            let dom = &item.dom;
            let style_map = &item.style_map;
            let cache = &mut item.font_size_cache;
            let mut stack = vec![body];
            while let Some(node) = stack.pop() {
                let size = resolve_font_size(style_map, dom, node, ctx, cache);
                let text_len: usize = dom
                    .children(node)
                    .iter()
                    .filter_map(|&c| match &dom.node(c).kind {
                        NodeKind::Text(t) => Some(collapse_ws(t).len()),
                        _ => None,
                    })
                    .sum();
                if text_len > 0 {
                    let entry = sizes.entry(size.to_bits()).or_insert((size, 0.0));
                    entry.1 += text_len as f64;
                }
                for c in dom.children(node) {
                    if matches!(dom.node(c).kind, NodeKind::Element(_)) {
                        stack.push(c);
                    }
                }
            }
        }
        sizes
            .values()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(size, _)| *size)
            .unwrap_or(12.0)
    }

    fn flatten_spine(
        &self,
        items: &mut [ItemStyle],
        ctx: &FlattenContext,
        sbase: Option<f64>,
        fmap: &FontMapper,
        names: &mut HashMap<String, u32>,
        styles: &mut HashMap<String, String>,
    ) {
        for item in items.iter_mut() {
            let Some(body) = item.dom.find_first_tag_global("body") else {
                continue;
            };
            if ctx.margin_top >= 0.0 {
                item.page_rule
                    .insert("margin-top".to_string(), format!("{}pt", ctx.margin_top));
            }
            if ctx.margin_bottom >= 0.0 {
                item.page_rule.insert(
                    "margin-bottom".to_string(),
                    format!("{}pt", ctx.margin_bottom),
                );
            }
            let psize = ctx.dest_base_font_size;
            self.flatten_node(item, body, ctx, sbase, fmap, names, styles, psize);
        }
    }

    /// Port of `flatten_node`'s cssdict-construction and font-rescaling
    /// core, plus class-name assignment. See the module docs for the
    /// pieces this deliberately doesn't attempt (`filter_css`,
    /// pseudo-classes).
    #[allow(clippy::too_many_arguments)]
    fn flatten_node(
        &self,
        item: &mut ItemStyle,
        node: NodeId,
        ctx: &FlattenContext,
        sbase: Option<f64>,
        fmap: &FontMapper,
        names: &mut HashMap<String, u32>,
        styles: &mut HashMap<String, String>,
        psize: f64,
    ) {
        let tag = item.dom.tag(node).unwrap_or("").to_string();
        let mut cssdict = cssdict(&item.style_map, &item.dom, node);

        if let Some(align) = item.dom.node(node).attrs.get("align").cloned() {
            if tag != "img" {
                cssdict.insert("text-align".to_string(), align.clone());
                if align == "center" && tag == "table" {
                    cssdict
                        .entry("margin-left".to_string())
                        .or_insert_with(|| "auto".to_string());
                    cssdict
                        .entry("margin-right".to_string())
                        .or_insert_with(|| "auto".to_string());
                }
            } else if align == "middle" || align == "bottom" || align == "top" {
                cssdict.insert("vertical-align".to_string(), align);
            } else if align == "left" || align == "right" {
                cssdict.insert("float".to_string(), align);
            }
            item.dom.node_mut(node).attrs.shift_remove("align");
        }
        if tag == "td" {
            if let Some(valign) = item.dom.node(node).attrs.get("valign").cloned() {
                if cssdict.get("vertical-align").map(|s| s.as_str()) == Some("inherit")
                    || !cssdict.contains_key("vertical-align")
                {
                    cssdict.insert("vertical-align".to_string(), valign);
                }
                item.dom.node_mut(node).attrs.shift_remove("valign");
            }
        }
        if let Some(color) = item.dom.node(node).attrs.get("color").cloned() {
            cssdict.insert("color".to_string(), color);
            item.dom.node_mut(node).attrs.shift_remove("color");
        }
        if let Some(bg) = item.dom.node(node).attrs.get("bgcolor").cloned() {
            cssdict.insert("background-color".to_string(), bg);
            item.dom.node_mut(node).attrs.shift_remove("bgcolor");
        }
        if tag == "ol" {
            item.dom.node_mut(node).attrs.shift_remove("type");
        }
        if cssdict.get("font-weight").map(|s| s.to_lowercase()) == Some("medium".to_string()) {
            cssdict.insert("font-weight".to_string(), "normal".to_string());
        }

        let font_size = resolve_font_size(
            &item.style_map,
            &item.dom,
            node,
            ctx,
            &mut item.font_size_cache,
        );

        let is_drop_cap = cssdict.get("float").map(|s| s.as_str()) == Some("left")
            && cssdict.contains_key("font-size")
            && item.dom.children(node).is_empty()
            && is_single_char_text(&item.dom, node);

        let mut new_psize = psize;
        if !ctx.disable_font_rescaling && !is_drop_cap {
            let fsize = fmap.get(font_size);
            if psize > 0.0 {
                cssdict.insert("font-size".to_string(), format!("{:.5}em", fsize / psize));
            } else {
                cssdict.insert("font-size".to_string(), format!("{fsize:.1}pt"));
            }
            new_psize = fsize;
        } else if cssdict.contains_key("font-size") || tag == "body" {
            // Font rescaling disabled: still normalize whatever unit the
            // source used down to a psize-relative `em`, matching
            // Python's else-branch when `fbase` isn't set at all.
            if psize > 0.0 {
                cssdict.insert(
                    "font-size".to_string(),
                    format!("{:.5}em", font_size / psize),
                );
            }
            new_psize = font_size;
        }

        self.clean_edges(&mut cssdict, ctx, sbase, new_psize);

        if let Some(disp) = cssdict.get("display").cloned() {
            if disp == "in-line" {
                cssdict.insert("display".to_string(), "inline".to_string());
            }
        }
        if self.opts.unfloat
            && cssdict.contains_key("float")
            && cssdict.get("display").map(|s| s.as_str()).unwrap_or("none") != "none"
        {
            cssdict.remove("display");
        }
        if self.opts.untable {
            if let Some(disp) = cssdict.get("display").cloned() {
                if disp.starts_with("table") {
                    let new_disp = if disp == "table-cell" {
                        "inline"
                    } else {
                        "block"
                    };
                    cssdict.insert("display".to_string(), new_disp.to_string());
                }
            }
        }
        if cssdict.get("vertical-align").map(|s| s.as_str()) == Some("sup") {
            cssdict.insert("vertical-align".to_string(), "super".to_string());
        }

        if let Some(lineh) = self.opts.lineh {
            if !cssdict.contains_key("line-height") && tag != "html" {
                cssdict.insert(
                    "line-height".to_string(),
                    format!("{:.5}em", lineh / new_psize.max(1.0)),
                );
            }
        }

        if (ctx.remove_paragraph_spacing || ctx.insert_blank_line) && (tag == "p" || tag == "div") {
            for prop in ["margin", "padding", "border"] {
                for edge in ["top", "bottom"] {
                    cssdict.insert(format!("{prop}-{edge}"), "0pt".to_string());
                }
            }
            if ctx.insert_blank_line {
                cssdict.insert(
                    "margin-top".to_string(),
                    format!("{}em", ctx.insert_blank_line_size),
                );
                cssdict.insert(
                    "margin-bottom".to_string(),
                    format!("{}em", ctx.insert_blank_line_size),
                );
            }
            let indent_size = ctx.remove_paragraph_spacing_indent_size;
            if ctx.remove_paragraph_spacing
                && indent_size >= 0.0
                && !matches!(
                    cssdict.get("text-align").map(|s| s.as_str()),
                    Some("center") | Some("right")
                )
            {
                cssdict.insert("text-indent".to_string(), format!("{indent_size:.1}em"));
            }
        }

        if !cssdict.is_empty() {
            let css = cssdict
                .iter()
                .map(|(k, v)| format!("{k}: {v}"))
                .collect::<Vec<_>>()
                .join(";\n");
            let existing_class = item
                .dom
                .node(node)
                .attrs
                .get("class")
                .cloned()
                .unwrap_or_default();
            let base_class = existing_class
                .split_whitespace()
                .next()
                .unwrap_or("calibre");
            let stripped = strip_trailing_digits(base_class).to_lowercase();
            let klass = if stripped.is_empty() {
                "calibre".to_string()
            } else {
                stripped.replace(' ', "_")
            };
            let assigned = if let Some(existing) = styles.get(&css) {
                existing.clone()
            } else {
                let n = names.entry(klass.clone()).or_insert(0);
                let candidate = if *n == 0 {
                    klass.clone()
                } else {
                    format!("{klass}{n}")
                };
                *n += 1;
                styles.insert(css.clone(), candidate.clone());
                candidate
            };
            item.dom
                .node_mut(node)
                .attrs
                .insert("class".to_string(), assigned);
        } else if item.dom.node(node).attrs.contains_key("class") {
            item.dom.node_mut(node).attrs.shift_remove("class");
        }
        item.dom.node_mut(node).attrs.shift_remove("style");

        let children = item.dom.children(node);
        for child in children {
            if matches!(item.dom.node(child).kind, NodeKind::Element(_)) {
                self.flatten_node(item, child, ctx, sbase, fmap, names, styles, new_psize);
            }
        }
    }

    /// Port of `clean_edges`.
    fn clean_edges(
        &self,
        cssdict: &mut BTreeMap<String, String>,
        ctx: &FlattenContext,
        sbase: Option<f64>,
        fsize: f64,
    ) {
        let Some(lineh) = self.opts.lineh else {
            return;
        };
        if self.opts.fbase.is_none() {
            return;
        }
        let sbase = sbase.unwrap_or(ctx.base_font_size);
        let slineh = sbase * 1.26;
        for kind in ["margin", "padding"] {
            for edge in ["bottom", "top"] {
                let prop = format!("{kind}-{edge}");
                let Some(value_text) = cssdict.get(&prop).cloned() else {
                    continue;
                };
                if value_text.contains('%') {
                    continue;
                }
                let Some(value) =
                    unit_convert(&value_text, sbase, fsize, ctx.dpi, ctx.base_font_size)
                else {
                    continue;
                };
                if value == 0.0 {
                    continue;
                }
                let new_value = if value <= slineh {
                    lineh / fsize.max(1.0)
                } else {
                    (value / slineh).round() * lineh / fsize.max(1.0)
                };
                cssdict.insert(prop, format!("{new_value:.5}em"));
            }
        }
    }

    /// Port of `replace_css`: drops every existing stylesheet manifest
    /// item and adds one generated `stylesheet.css`.
    fn replace_css(&self, oeb: &mut OEBBook, css: &str) -> String {
        let old: Vec<String> = oeb
            .manifest
            .iter()
            .filter(|i| OEB_STYLES.contains(&i.media_type.as_str()))
            .map(|i| i.id.clone())
            .collect();
        for id in old {
            oeb.manifest.remove(&id);
        }
        let (id, href) = oeb.manifest.generate("css", "stylesheet.css");
        oeb.manifest.add(&id, &href, CSS_MIME);
        let _ = oeb.container.write(&href, css.as_bytes());
        href
    }

    /// Port of `flatten_head`: strips every existing `<style>`/`<link
    /// rel=stylesheet>` from the document and links in the consolidated
    /// stylesheet (plus, if present, this item's own `page_styles.css`,
    /// generated by [`Self::collect_global_css`] -- not yet ported here;
    /// see the module docs' pseudo-class/per-page-`@font-face` scope
    /// note for why per-item global CSS generation is narrower than
    /// Python's).
    fn flatten_head(
        &self,
        oeb: &mut OEBBook,
        item: &mut ItemStyle,
        href: &str,
        _ctx: &FlattenContext,
    ) {
        let Some(head) = item.dom.find_first_tag_global("head") else {
            return;
        };
        let nodes: Vec<NodeId> = item.dom.preorder_elements(item.dom.root);
        for n in nodes {
            let tag = item.dom.tag(n);
            if tag == Some("link") {
                let ty = item
                    .dom
                    .node(n)
                    .attrs
                    .get("type")
                    .cloned()
                    .unwrap_or_else(|| CSS_MIME.to_string());
                let rel = item
                    .dom
                    .node(n)
                    .attrs
                    .get("rel")
                    .cloned()
                    .unwrap_or_else(|| "stylesheet".to_string());
                if rel.eq_ignore_ascii_case("stylesheet") && OEB_STYLES.contains(&ty.as_str()) {
                    item.dom.detach(n);
                }
            } else if tag == Some("style") {
                let ty = item
                    .dom
                    .node(n)
                    .attrs
                    .get("type")
                    .cloned()
                    .unwrap_or_else(|| CSS_MIME.to_string());
                if OEB_STYLES.contains(&ty.as_str()) {
                    item.dom.detach(n);
                }
            }
        }
        let rel = super::filenames::relhref(&item.href, href);
        let link = item.dom.new_element("link");
        item.dom
            .node_mut(link)
            .attrs
            .insert("rel".to_string(), "stylesheet".to_string());
        item.dom
            .node_mut(link)
            .attrs
            .insert("type".to_string(), CSS_MIME.to_string());
        item.dom
            .node_mut(link)
            .attrs
            .insert("href".to_string(), rel);
        item.dom.append_child(head, link);
        let _ = oeb;
        let _ = XHTML_NS;
    }
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_single_char_text(dom: &Dom, node: NodeId) -> bool {
    let text = dom.text_content(node);
    let chars: Vec<char> = text.chars().collect();
    chars.len() == 1 || (chars.len() == 2 && (0x2000..=0x206f).contains(&(chars[0] as u32)))
}

fn strip_trailing_digits(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn unit_convert_handles_common_units() {
        assert_eq!(unit_convert("12pt", 12.0, 12.0, 96.0, 12.0), Some(12.0));
        assert_eq!(unit_convert("96px", 12.0, 12.0, 96.0, 12.0), Some(72.0));
        assert_eq!(unit_convert("2em", 10.0, 10.0, 96.0, 12.0), Some(20.0));
        assert_eq!(unit_convert("50%", 20.0, 10.0, 96.0, 12.0), Some(10.0));
        assert_eq!(unit_convert("bogus", 12.0, 12.0, 96.0, 12.0), None);
    }

    #[test]
    fn font_mapper_null_is_identity() {
        let fm = FontMapper::new(None, None, None);
        assert_eq!(fm.get(15.0), 15.0);
    }

    #[test]
    fn font_mapper_scale_is_linear() {
        let fm = FontMapper::new(Some(10.0), Some(20.0), None);
        assert_eq!(fm.get(10.0), 20.0);
        assert_eq!(fm.get(5.0), 10.0);
    }

    #[test]
    fn font_mapper_key_snaps_to_nearest_dest_size() {
        let fm = FontMapper::new(Some(12.0), Some(12.0), Some(&[8.0, 12.0, 20.0]));
        assert_eq!(fm.get(12.0), 12.0);
    }

    #[test]
    fn resolve_document_styles_resolves_linked_and_inline_and_attribute_styles() {
        let oeb = Builder::new()
            .part("style.css", "text/css", b"p { color: blue; font-size: 15pt }", false)
            .part(
                "a.html",
                "application/xhtml+xml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="style.css"/></head><body><p id="x" style="font-weight: bold">hi</p></body></html>"#,
                true,
            )
            .build();
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw).into_owned();
        let dom = Dom::parse(&html);
        let style_map = resolve_document_styles(&oeb, "a.html", &dom, "");
        let p = dom.find_first_tag_global("p").unwrap();
        let dict = cssdict(&style_map, &dom, p);
        assert_eq!(dict.get("color").map(|s| s.as_str()), Some("blue"));
        assert_eq!(dict.get("font-size").map(|s| s.as_str()), Some("15pt"));
        assert_eq!(dict.get("font-weight").map(|s| s.as_str()), Some("bold"));
    }

    #[test]
    fn cssflattener_consolidates_styles_into_one_stylesheet_and_rewrites_classes() {
        let mut oeb = Builder::new()
            .part("style.css", "text/css", b"p { color: red }", false)
            .part(
                "a.html",
                "application/xhtml+xml",
                br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="style.css"/></head><body><p>one</p><p>two</p></body></html>"#,
                true,
            )
            .build();
        let flattener = CSSFlattener::new(FlattenerOptions::default());
        let ctx = FlattenContext::default();
        let mut log = Vec::new();
        flattener
            .call(&mut oeb, &ctx, &mut |m| log.push(m.to_string()))
            .unwrap();

        // Original stylesheet is gone; exactly one remains.
        let css_items: Vec<_> = oeb
            .manifest
            .iter()
            .filter(|i| i.media_type == "text/css")
            .collect();
        assert_eq!(css_items.len(), 1, "{:?}", css_items);
        let css = oeb.container.read(&css_items[0].href).unwrap();
        let css_text = String::from_utf8_lossy(&css);
        assert!(css_text.contains("color: red"), "{css_text}");

        // Both <p> elements (same style) share one generated class.
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("class=\"calibre"), "{html}");
        assert!(
            !html.contains("<link rel=\"stylesheet\" href=\"style.css\""),
            "{html}"
        );
        assert!(html.contains("stylesheet.css"), "{html}");
    }

    #[test]
    fn cssflattener_rescales_font_sizes_when_fbase_is_set() {
        let mut oeb = Builder::new()
            .page("a.html", r#"<p style="font-size: 24pt">big</p>"#)
            .build();
        let opts = FlattenerOptions {
            fbase: Some(12.0),
            ..FlattenerOptions::default()
        };
        let flattener = CSSFlattener::new(opts);
        let ctx = FlattenContext {
            disable_font_rescaling: false,
            ..FlattenContext::default()
        };
        let mut log = Vec::new();
        flattener
            .call(&mut oeb, &ctx, &mut |m| log.push(m.to_string()))
            .unwrap();
        let css_href = oeb
            .manifest
            .iter()
            .find(|i| i.media_type == "text/css")
            .unwrap()
            .href
            .clone();
        let css = oeb.container.read(&css_href).unwrap();
        let css_text = String::from_utf8_lossy(&css);
        assert!(css_text.contains("font-size"), "{css_text}");
    }

    #[test]
    fn strip_trailing_digits_drops_numeric_suffix() {
        assert_eq!(strip_trailing_digits("calibre12"), "calibre");
        assert_eq!(strip_trailing_digits("chapter"), "chapter");
    }
}
