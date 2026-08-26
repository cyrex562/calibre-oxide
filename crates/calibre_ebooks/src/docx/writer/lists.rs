//! List numbering (`numbering.xml`): port of `docx/writer/lists.py`.
//!
//! `Block.list_tag`/`.numbering_id` (already ported, PR #333) are
//! exactly the seam this file needs -- `Block.list_tag`'s Python
//! second tuple element (the `<li>`'s CSS `Style`) was deliberately
//! dropped when `Block` was ported, since [`Style`] is cheap to
//! reconstruct from a [`NodeId`] (it's `Copy`); this file does exactly
//! that reconstruction, via `dom`/`resolved`/`profile` passed
//! explicitly rather than Python's cached-on-`Style` `_stylizer`
//! reference (itself, on inspection, stored on `NumberingDefinition`
//! but never actually read again -- dropped here as genuinely dead
//! state, not a simplification with any real effect).
//!
//! **A deliberate divergence from Python's literal code, not just its
//! representation**: `NumberingDefinition.__hash__` hashes `self.levels`
//! (intending numbering definitions with identical visual level
//! sequences to dedupe into one shared `w:abstractNum`), but the class
//! never defines `__eq__` -- so Python's `dict`-based dedup pass
//! (`definitions[defn]`) falls back to *object identity*, which never
//! matches a different instance, meaning the dedup **never actually
//! fires** in the shipped Python. Worse: if it ever did fire, the
//! surviving (canonical) definition's `link_blocks()` would run
//! instead of the discarded duplicate's, silently leaving the
//! duplicate's own blocks with no `numbering_id` at all -- a real bug
//! were the dedup to ever match. This port does **not** implement
//! working dedup: every distinct top-level list gets its own
//! `NumberingDefinition` and sequential `num_id`, matching Python's
//! actual (never-deduping) runtime behavior, not its evidently-broken
//! intent. See issue #132's tracking notes if this needs revisiting --
//! a *correct* dedup pass would need `link_blocks` to run once per
//! *source* definition even when it shares a canonical target's id,
//! not once per canonical definition.

use std::collections::BTreeMap;

use crate::dom::{Dom, NodeId};
use crate::oeb::polish::cascade::ResolvedStyles;
use crate::oeb::polish::style::{Profile, Style};

use indexmap::IndexMap;

use super::from_html::{BlockId, Blocks};
use super::xml::Element;

/// Port of `LIST_STYLES`.
const LIST_STYLES: &[&str] = &[
    "disc",
    "circle",
    "square",
    "decimal",
    "decimal-leading-zero",
    "lower-roman",
    "upper-roman",
    "lower-greek",
    "lower-alpha",
    "lower-latin",
    "upper-alpha",
    "upper-latin",
    "hiragana",
    "hebrew",
    "katakana-iroha",
    "cjk-ideographic",
];

/// Port of `STYLE_MAP`.
fn style_map(list_type: &str) -> Option<&'static str> {
    Some(match list_type {
        "disc" => "bullet",
        "circle" => "o",
        "square" => "\u{f0a7}",
        "decimal" => "decimal",
        "decimal-leading-zero" => "decimalZero",
        "lower-roman" => "lowerRoman",
        "upper-roman" => "upperRoman",
        "lower-alpha" | "lower-latin" => "lowerLetter",
        "upper-alpha" | "upper-latin" => "upperLetter",
        "hiragana" => "aiueo",
        "hebrew" => "hebrew1",
        "katakana-iroha" => "iroha",
        "cjk-ideographic" => "chineseCounting",
        _ => return None,
    })
}

/// Port of `find_list_containers`: walks `list_tag`'s ancestors,
/// collecting every one whose *own* (non-inherited -- `Style::own`,
/// matching Python's `style._style.get(...)`) `list-style-type` is a
/// real list style. `ans[0]` is the nearest such ancestor, `ans.last()`
/// the outermost -- callers use `ans.len() - 1` as the nesting depth
/// and `ans.last()` as the shared grouping key for one physical list.
fn find_list_containers(
    dom: &Dom,
    resolved: &ResolvedStyles,
    profile: &Profile,
    list_tag: NodeId,
) -> Vec<NodeId> {
    let mut node = list_tag;
    let mut ans = Vec::new();
    loop {
        let Some(parent) = dom.parent(node) else {
            break;
        };
        if parent == node {
            break;
        }
        node = parent;
        let style = Style::new(dom, resolved, profile, node);
        let lst = style
            .own("list-style-type")
            .unwrap_or_default()
            .to_lowercase();
        if LIST_STYLES.contains(&lst.as_str()) {
            ans.push(node);
        }
    }
    ans
}

/// Port of `Level`.
#[derive(Debug, Clone)]
struct Level {
    ilvl: u32,
    start: i64,
    num_fmt: String,
    lvl_text: String,
}

/// Dedup key for [`Level`], matching Python's `Level.__hash__` -- it
/// hashes `(start, num_fmt, lvl_text)`, deliberately excluding `ilvl`
/// (which always equals this level's position within a
/// [`NumberingDefinition`]'s `levels` vector, making it redundant for
/// comparing whole `levels` sequences positionally).
impl PartialEq for Level {
    fn eq(&self, other: &Self) -> bool {
        (self.start, &self.num_fmt, &self.lvl_text)
            == (other.start, &other.num_fmt, &other.lvl_text)
    }
}
impl Eq for Level {}

impl Level {
    /// Port of `Level.__init__`.
    fn new(dom: &Dom, container: NodeId, items: &[NodeId], ilvl: u32, list_type: &str) -> Self {
        let mut start = dom
            .node(container)
            .attrs
            .get("start")
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(1);
        if let Some(&first) = items.first() {
            if let Some(v) = dom
                .node(first)
                .attrs
                .get("value")
                .and_then(|s| s.parse::<i64>().ok())
            {
                start = v;
            }
        }
        let (num_fmt, lvl_text) = if matches!(list_type, "disc" | "circle" | "square") {
            let lvl_text = if list_type == "disc" {
                "\u{f0b7}".to_string()
            } else {
                style_map(list_type).unwrap_or_default().to_string()
            };
            ("bullet".to_string(), lvl_text)
        } else {
            (
                style_map(list_type).unwrap_or("decimal").to_string(),
                format!("%{}.", ilvl + 1),
            )
        };
        Level {
            ilvl,
            start,
            num_fmt,
            lvl_text,
        }
    }

    /// Port of `Level.serialize`.
    fn serialize(&self) -> Element {
        let mut lvl = Element::new("w:lvl")
            .attr("w:ilvl", self.ilvl.to_string())
            .with(Element::new("w:start").attr("w:val", self.start.to_string()))
            .with(Element::new("w:numFmt").attr("w:val", self.num_fmt.clone()))
            .with(Element::new("w:lvlText").attr("w:val", self.lvl_text.clone()))
            .with(Element::new("w:lvlJc").attr("w:val", "left"))
            .with(
                Element::new("w:pPr").with(
                    Element::new("w:ind")
                        .attr("w:hanging", "360")
                        .attr("w:left", (1152 + self.ilvl as i64 * 360).to_string()),
                ),
            );
        if self.num_fmt == "bullet" {
            let ff = match self.lvl_text.as_str() {
                "\u{f0b7}" => "Symbol",
                "\u{f0a7}" => "Wingdings",
                _ => "Courier New",
            };
            lvl.append(
                Element::new("w:rPr").with(
                    Element::new("w:rFonts")
                        .attr("w:ascii", ff)
                        .attr("w:hAnsi", ff)
                        .attr("w:hint", "default"),
                ),
            );
        }
        lvl
    }
}

/// Port of `NumberingDefinition`. `stylizer` isn't stored -- see the
/// module docs.
#[derive(Debug)]
struct NumberingDefinition {
    /// (container, list_tag, block, list_type) per ilvl, in the order
    /// list items were encountered walking `all_blocks`.
    level_map: BTreeMap<u32, Vec<(NodeId, NodeId, BlockId, String)>>,
    num_id: Option<u32>,
    levels: Vec<Level>,
}

impl NumberingDefinition {
    fn new() -> Self {
        NumberingDefinition {
            level_map: BTreeMap::new(),
            num_id: None,
            levels: Vec::new(),
        }
    }

    /// Port of `NumberingDefinition.finalize`. Where an ilvl saw more
    /// than one distinct `(container, list_type)` pair (rare -- would
    /// mean sibling `<li>`s at the same depth disagreeing), the
    /// *last*-encountered one wins, matching Python's `dict`
    /// overwrite-on-assign semantics in its own `finalize`.
    fn finalize(&mut self, dom: &Dom) {
        self.levels = self
            .level_map
            .iter()
            .map(|(&ilvl, items)| {
                let (container, _, _, list_type) = items
                    .last()
                    .expect("level_map only ever holds non-empty Vecs");
                let item_tags: Vec<NodeId> = items.iter().map(|i| i.1).collect();
                Level::new(dom, *container, &item_tags, ilvl, list_type)
            })
            .collect();
    }

    /// Port of `NumberingDefinition.link_blocks`.
    fn link_blocks(&self, blocks: &mut Blocks) {
        let num_id = self
            .num_id
            .expect("link_blocks is only called after num_id has been assigned");
        for (&ilvl, items) in &self.level_map {
            for &(_, _, block_id, _) in items {
                blocks.block_mut(block_id).numbering_id = Some((num_id + 1, ilvl));
            }
        }
    }

    /// Port of `NumberingDefinition.serialize`.
    fn serialize(&self, parent: &mut Element) {
        let num_id = self
            .num_id
            .expect("serialize is only called after num_id has been assigned");
        let an = parent.append(
            Element::new("w:abstractNum")
                .attr("w:abstractNumId", num_id.to_string())
                .with(Element::new("w:multiLevelType").attr("w:val", "hybridMultilevel"))
                .with(Element::new("w:name").attr("w:val", format!("List {}", num_id + 1))),
        );
        for level in &self.levels {
            an.append(level.serialize());
        }
    }
}

/// Port of `ListsManager`. `namespace` isn't stored (see [`Element`]'s
/// own module docs); `self.lists` (Python's `__init__`-only dict) is
/// dropped too -- `finalize` always shadows it with a fresh local
/// `lists`, so the field is genuinely never read.
#[derive(Debug, Default)]
pub struct ListsManager {
    definitions: Vec<NumberingDefinition>,
}

impl ListsManager {
    pub fn new() -> Self {
        ListsManager::default()
    }

    /// Port of `ListsManager.finalize`. `all_blocks` is
    /// `Blocks::all_blocks()`'s current contents (Python's
    /// `Convert.__call__` passes `self.blocks.all_blocks` after its
    /// own skip/dedup pass -- not ported here, so the caller is
    /// responsible for having done any such cleanup on `blocks` first).
    pub fn finalize(
        &mut self,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
        blocks: &mut Blocks,
    ) {
        let all_blocks: Vec<BlockId> = blocks.all_blocks().to_vec();
        let mut lists: IndexMap<NodeId, NumberingDefinition> = IndexMap::new();
        for block_id in all_blocks {
            let Some(list_tag) = blocks.block(block_id).list_tag else {
                continue;
            };
            let tag_style = Style::new(dom, resolved, profile, list_tag);
            let list_type = tag_style.get("list-style-type").to_lowercase();
            if !LIST_STYLES.contains(&list_type.as_str()) {
                continue;
            }
            let container_tags = find_list_containers(dom, resolved, profile, list_tag);
            let Some(&top_most) = container_tags.last() else {
                continue;
            };
            let nd = lists
                .entry(top_most)
                .or_insert_with(NumberingDefinition::new);
            let ilvl = (container_tags.len() - 1) as u32;
            nd.level_map.entry(ilvl).or_default().push((
                container_tags[0],
                list_tag,
                block_id,
                list_type,
            ));
        }

        for nd in lists.values_mut() {
            nd.finalize(dom);
        }
        // Every distinct top-level list gets its own num_id, in
        // first-encountered order -- see the module docs for why this
        // doesn't attempt Python's (non-functional) dedup pass.
        let mut definitions: Vec<NumberingDefinition> = lists.into_values().collect();
        for (i, nd) in definitions.iter_mut().enumerate() {
            nd.num_id = Some(i as u32);
        }
        for nd in &definitions {
            nd.link_blocks(blocks);
        }
        self.definitions = definitions;
    }

    /// Port of `ListsManager.serialize`.
    pub fn serialize(&self, parent: &mut Element) {
        for defn in &self.definitions {
            defn.serialize(parent);
        }
        for defn in &self.definitions {
            let num_id = defn
                .num_id
                .expect("serialize is only called after finalize has assigned num_id");
            parent.append(
                Element::new("w:num")
                    .attr("w:numId", (num_id + 1).to_string())
                    .with(Element::new("w:abstractNumId").attr("w:val", num_id.to_string())),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docx::writer::styles::StylesManager;
    use crate::oeb::polish::cascade::PropertyValue;
    use crate::oeb::polish::style::Profile;
    use std::collections::HashMap;

    fn make(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn resolved_with(entries: &[(NodeId, &[(&str, &str)])]) -> ResolvedStyles {
        let mut style_map = HashMap::new();
        for &(id, props) in entries {
            let mut m = HashMap::new();
            for &(k, v) in props {
                m.insert(k.to_string(), PropertyValue::new(v, None, false));
            }
            style_map.insert(id, m);
        }
        ResolvedStyles {
            style_map,
            pseudo_style_map: HashMap::new(),
        }
    }

    fn find(dom: &Dom, tag: &str) -> NodeId {
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&id| dom.tag(id) == Some(tag))
            .unwrap()
    }

    fn find_all(dom: &Dom, tag: &str) -> Vec<NodeId> {
        dom.preorder_elements(dom.root)
            .into_iter()
            .filter(|&id| dom.tag(id) == Some(tag))
            .collect()
    }

    /// Creates a Block for `node` with `list_tag` set (matching
    /// `Block.list_tag = (html_block, style) if is_list_item`) and
    /// adds it to `blocks.all_blocks()`/`.items()`.
    fn add_list_item_block(
        blocks: &mut Blocks,
        mgr: &mut StylesManager,
        dom: &Dom,
        resolved: &ResolvedStyles,
        profile: &Profile,
        node: NodeId,
    ) -> BlockId {
        let style = Style::new(dom, resolved, profile, node);
        let id = blocks.start_new_block(mgr, dom, node, &style, false, None, true);
        blocks.end_current_block();
        id
    }

    #[test]
    fn find_list_containers_collects_ancestors_nearest_first() {
        let dom = make("<html><body><ul><li><ol><li>x</li></ol></li></ul></body></html>");
        let ul = find(&dom, "ul");
        let ol = find(&dom, "ol");
        let lis = find_all(&dom, "li");
        let inner_li = lis[1];
        let resolved = resolved_with(&[
            (ul, &[("list-style-type", "disc")]),
            (ol, &[("list-style-type", "decimal")]),
        ]);
        let profile = Profile::default();
        let containers = find_list_containers(&dom, &resolved, &profile, inner_li);
        assert_eq!(
            containers,
            vec![ol, ul],
            "nearest ancestor first, outermost last"
        );
    }

    #[test]
    fn find_list_containers_skips_ancestors_without_their_own_list_style() {
        let dom = make("<html><body><ul><li><div><span>x</span></div></li></ul></body></html>");
        let ul = find(&dom, "ul");
        let span = find(&dom, "span");
        let resolved = resolved_with(&[(ul, &[("list-style-type", "disc")])]);
        let profile = Profile::default();
        let containers = find_list_containers(&dom, &resolved, &profile, span);
        assert_eq!(containers, vec![ul]);
    }

    #[test]
    fn level_bullet_uses_the_disc_glyph_and_bullet_format() {
        let dom = make("<html><body><ul><li>x</li></ul></body></html>");
        let ul = find(&dom, "ul");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ul, &[li], 0, "disc");
        assert_eq!(level.num_fmt, "bullet");
        assert_eq!(level.lvl_text, "\u{f0b7}");
        assert_eq!(level.start, 1);
    }

    #[test]
    fn level_decimal_uses_percent_ilvl_level_text() {
        let dom = make("<html><body><ol><li>x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[li], 1, "decimal");
        assert_eq!(level.num_fmt, "decimal");
        assert_eq!(level.lvl_text, "%2.");
    }

    #[test]
    fn level_unknown_list_type_falls_back_to_decimal() {
        let dom = make("<html><body><ol><li>x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[li], 0, "armenian");
        assert_eq!(level.num_fmt, "decimal");
    }

    #[test]
    fn level_start_comes_from_the_container_start_attribute() {
        let dom = make("<html><body><ol start=\"5\"><li>x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[li], 0, "decimal");
        assert_eq!(level.start, 5);
    }

    #[test]
    fn level_start_prefers_the_first_items_own_value_attribute() {
        let dom = make("<html><body><ol start=\"5\"><li value=\"9\">x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[li], 0, "decimal");
        assert_eq!(level.start, 9);
    }

    #[test]
    fn level_start_defaults_to_one_with_no_attributes_at_all() {
        let dom = make("<html><body><ol><li>x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[], 0, "decimal");
        assert_eq!(level.start, 1);
    }

    #[test]
    fn level_serialize_emits_the_expected_ooxml_shape() {
        let dom = make("<html><body><ol><li>x</li></ol></body></html>");
        let ol = find(&dom, "ol");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ol, &[li], 2, "lower-roman");
        let el = level.serialize();
        assert_eq!(el.name, "w:lvl");
        assert_eq!(el.get("w:ilvl"), Some("2"));
        assert_eq!(
            el.children_named("w:numFmt").next().unwrap().get("w:val"),
            Some("lowerRoman")
        );
        let ind = el
            .children_named("w:pPr")
            .next()
            .unwrap()
            .children_named("w:ind")
            .next()
            .unwrap();
        assert_eq!(
            ind.get("w:left"),
            Some((1152 + 2 * 360).to_string().as_str())
        );
        assert!(
            el.children_named("w:rPr").next().is_none(),
            "only bullet levels get w:rPr"
        );
    }

    #[test]
    fn level_serialize_bullet_emits_symbol_font() {
        let dom = make("<html><body><ul><li>x</li></ul></body></html>");
        let ul = find(&dom, "ul");
        let li = find(&dom, "li");
        let level = Level::new(&dom, ul, &[li], 0, "disc");
        let el = level.serialize();
        let rpr = el.children_named("w:rPr").next().unwrap();
        let rfonts = rpr.children_named("w:rFonts").next().unwrap();
        assert_eq!(rfonts.get("w:ascii"), Some("Symbol"));
    }

    #[test]
    fn lists_manager_finalize_assigns_numbering_id_to_a_single_flat_list() {
        let dom = make("<html><body><ul><li>a</li><li>b</li></ul></body></html>");
        let ul = find(&dom, "ul");
        let lis = find_all(&dom, "li");
        let resolved = resolved_with(&[(ul, &[("list-style-type", "disc")])]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let a = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[0]);
        let b = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[1]);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        assert_eq!(blocks.block(a).numbering_id, Some((1, 0)));
        assert_eq!(blocks.block(b).numbering_id, Some((1, 0)));
    }

    #[test]
    fn lists_manager_finalize_computes_nesting_depth_for_a_sublist() {
        let dom = make("<html><body><ul><li>a<ol><li>nested</li></ol></li></ul></body></html>");
        let ul = find(&dom, "ul");
        let ol = find(&dom, "ol");
        let lis = find_all(&dom, "li");
        let resolved = resolved_with(&[
            (ul, &[("list-style-type", "disc")]),
            (ol, &[("list-style-type", "decimal")]),
        ]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let outer = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[0]);
        let inner = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[1]);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        // Both list items are inside the SAME outermost <ul>, so they
        // share one NumberingDefinition/num_id but different ilvl.
        assert_eq!(blocks.block(outer).numbering_id, Some((1, 0)));
        assert_eq!(blocks.block(inner).numbering_id, Some((1, 1)));
    }

    #[test]
    fn lists_manager_finalize_gives_two_separate_lists_distinct_num_ids() {
        let dom = make("<html><body><ul><li>a</li></ul><ul><li>b</li></ul></body></html>");
        let uls = find_all(&dom, "ul");
        let lis = find_all(&dom, "li");
        let resolved = resolved_with(&[
            (uls[0], &[("list-style-type", "disc")]),
            (uls[1], &[("list-style-type", "disc")]),
        ]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let a = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[0]);
        let b = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, lis[1]);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        // Structurally identical (same bullet style) but two distinct
        // <ul> roots -- Python's own dedup never fires (see the module
        // docs), so each gets its own num_id.
        assert_eq!(blocks.block(a).numbering_id, Some((1, 0)));
        assert_eq!(blocks.block(b).numbering_id, Some((2, 0)));
    }

    #[test]
    fn lists_manager_finalize_skips_a_block_whose_list_style_type_is_not_recognized() {
        let dom = make("<html><body><ul><li>a</li></ul></body></html>");
        let ul = find(&dom, "ul");
        let li = find(&dom, "li");
        let resolved = resolved_with(&[(ul, &[("list-style-type", "none")])]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let a = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, li);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        assert_eq!(blocks.block(a).numbering_id, None);
    }

    #[test]
    fn lists_manager_finalize_skips_a_block_with_no_list_container_ancestor() {
        // list-style-type resolves via inheritance on the <li> itself
        // (Style::get, not Style::own), but no ancestor OWN-declares a
        // real list style, so find_list_containers returns empty.
        let dom = make("<html><body><div><li>a</li></div></body></html>");
        let li = find(&dom, "li");
        let resolved = resolved_with(&[(li, &[("list-style-type", "disc")])]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let a = add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, li);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        assert_eq!(blocks.block(a).numbering_id, None);
    }

    #[test]
    fn lists_manager_finalize_ignores_blocks_with_no_list_tag() {
        let dom = make("<html><body><p>x</p></body></html>");
        let p = find(&dom, "p");
        let resolved = resolved_with(&[]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        let style = Style::new(&dom, &resolved, &profile, p);
        let id = blocks.start_new_block(&mut mgr, &dom, p, &style, false, None, false);
        blocks.end_current_block();
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        assert_eq!(blocks.block(id).numbering_id, None);
    }

    #[test]
    fn lists_manager_serialize_emits_abstract_num_and_num() {
        let dom = make("<html><body><ul><li>a</li></ul></body></html>");
        let ul = find(&dom, "ul");
        let li = find(&dom, "li");
        let resolved = resolved_with(&[(ul, &[("list-style-type", "disc")])]);
        let profile = Profile::default();
        let mut mgr = StylesManager::new("en");
        let mut blocks = Blocks::new();
        add_list_item_block(&mut blocks, &mut mgr, &dom, &resolved, &profile, li);
        let mut lm = ListsManager::new();
        lm.finalize(&dom, &resolved, &profile, &mut blocks);
        let mut numbering = Element::new("w:numbering");
        lm.serialize(&mut numbering);
        let an = numbering.children_named("w:abstractNum").next().unwrap();
        assert_eq!(an.get("w:abstractNumId"), Some("0"));
        assert!(an.children_named("w:lvl").next().is_some());
        let n = numbering.children_named("w:num").next().unwrap();
        assert_eq!(n.get("w:numId"), Some("1"));
        assert_eq!(
            n.children_named("w:abstractNumId")
                .next()
                .unwrap()
                .get("w:val"),
            Some("0")
        );
    }
}
