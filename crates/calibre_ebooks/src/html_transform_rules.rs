//! Port of `calibre.ebooks.html_transform_rules` (issue #118): a small,
//! user-authorable rules engine -- match elements by tag/class/CSS
//! selector/text-content, then rename/remove/unwrap/re-class/re-attr/
//! wrap/insert them. Real upstream's own GUI-authored rule editor isn't
//! part of this port (no GUI exists); this covers the pure, serializable
//! data model + apply-loop, matching how `oeb::polish::css`'s own
//! rules-based transforms were scoped.
//!
//! # Disclosed narrowings
//!
//! - **`match_type: "xpath"`** (an arbitrary user-authored XPath
//!   expression) has no equivalent here -- this crate has no general
//!   XPath engine over [`crate::dom::Dom`] (only a narrow XPath subset
//!   over [`crate::xmltree::Xml`] for OPF/NCX documents). Building a
//!   [`Rule`] with this match type returns a real error rather than
//!   silently degrading to something else.
//! - **`match_type: "*"`** (matches every tag) is ported via the CSS
//!   universal selector `*` instead of real upstream's `XPath('//*')`
//!   -- exactly equivalent, just reusing this crate's real CSS matcher
//!   instead of introducing an XPath call for a single fixed query.
//! - **`match_type: "contains_text"`** is ported as a direct tree
//!   predicate (any element whose *own* text-node children contain the
//!   substring) rather than real upstream's `XPath('//*[contains(text(),
//!   ...)]')` -- same real semantics (an element's own text, not
//!   descendant text), just evaluated without an XPath engine.
//! - **GUI display strings** (`Action`/`Match`'s `short_text`/
//!   `long_text`/`placeholder` fields, used only to render the rule
//!   editor dialog and per-action tooltips) are dropped; [`rule_to_text`]
//!   keeps a short human-readable label per match/action kind for
//!   [`export_rules`]'s comment lines, since that's the one place they
//!   have a real, non-GUI purpose (documenting an exported rule file).
//! - **`transform_conversion_book`** (applying rules during the
//!   conversion pipeline, over `OEBBook` spine items) isn't ported --
//!   [`transform_container`] (the "Polish Book" editor integration,
//!   over a [`crate::oeb::polish::container::Container`]) is the one
//!   real integration point ported here, matching this port's existing
//!   "Polish Book" -> `oeb::polish::*` precedent; conversion-pipeline
//!   wiring is separate, unstarted scope (no rules-based conversion
//!   input plugin exists in this port yet).

use anyhow::{bail, Context, Result};

use crate::css::matcher::{DomElement, Select};
use crate::css::selector::{parse_selector_list, SelectorList};
use crate::dom::{Dom, NodeId, NodeKind};

/// Port of `MATCH_TYPE_MAP`'s keys.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchType {
    Is,
    HasClass,
    NotHasClass,
    Css,
    Xpath,
    Any,
    ContainsText,
}

impl MatchType {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "is" => MatchType::Is,
            "has_class" => MatchType::HasClass,
            "not_has_class" => MatchType::NotHasClass,
            "css" => MatchType::Css,
            "xpath" => MatchType::Xpath,
            "*" => MatchType::Any,
            "contains_text" => MatchType::ContainsText,
            other => bail!("Unknown match_type: {other}"),
        })
    }

    /// Short human-readable label, port of `MATCH_TYPE_MAP[x].text`
    /// (used only by [`rule_to_text`] now -- see the module doc).
    fn label(&self) -> &'static str {
        match self {
            MatchType::Is => "is",
            MatchType::HasClass => "has class",
            MatchType::NotHasClass => "does not have class",
            MatchType::Css => "matches CSS selector",
            MatchType::Xpath => "matches XPath selector",
            MatchType::Any => "is any tag",
            MatchType::ContainsText => "contains text",
        }
    }

}

/// Port of `ACTION_MAP`'s keys plus each action's own `data`, already
/// parsed (Python keeps `data` as a raw string until `create_action`
/// parses it per action type at `Rule` construction time; done here up
/// front instead since Rust has no dynamic per-branch parsing sugar).
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Rename(String),
    Remove,
    Unwrap,
    AddClasses(Vec<String>),
    RemoveClasses(Vec<String>),
    RemoveAttrs(Vec<String>),
    AddAttrs(Vec<(String, String)>),
    Empty,
    Wrap { tag: String, attrs: Vec<(String, String)> },
    Insert(String),
    InsertEnd(String),
    Prepend(String),
    Append(String),
}

impl Action {
    fn parse(kind: &str, data: &str) -> Result<Self> {
        Ok(match kind {
            "rename" => Action::Rename(data.to_string()),
            "remove" => Action::Remove,
            "unwrap" => Action::Unwrap,
            "add_classes" => Action::AddClasses(data.split_whitespace().map(str::to_string).collect()),
            "remove_classes" => Action::RemoveClasses(data.split_whitespace().map(str::to_string).collect()),
            "remove_attrs" => Action::RemoveAttrs(data.split_whitespace().map(str::to_string).collect()),
            "add_attrs" => Action::AddAttrs(parse_attrs(data)),
            "empty" => Action::Empty,
            "wrap" => {
                let (tag, attrs) = parse_start_tag(data)?;
                Action::Wrap { tag, attrs }
            }
            "insert" => Action::Insert(data.to_string()),
            "insert_end" => Action::InsertEnd(data.to_string()),
            "prepend" => Action::Prepend(data.to_string()),
            "append" => Action::Append(data.to_string()),
            other => bail!("Unknown action type: {other}"),
        })
    }

    /// Applies this action to `tag`, returning whether it changed
    /// anything. Port of each `action_map` closure's target function.
    fn apply(&self, dom: &mut Dom, tag: NodeId) -> bool {
        match self {
            Action::Rename(new_name) => rename_tag(dom, new_name, tag),
            Action::Remove => {
                dom.detach(tag);
                true
            }
            Action::Unwrap => {
                dom.remove_promoting_children(tag);
                true
            }
            Action::AddClasses(classes) => add_classes(dom, classes, tag),
            Action::RemoveClasses(classes) => remove_classes(dom, classes, tag),
            Action::RemoveAttrs(attrs) => remove_attrs(dom, attrs, tag),
            Action::AddAttrs(attrs) => add_attrs(dom, attrs, tag),
            Action::Empty => empty(dom, tag),
            Action::Wrap { tag: wrap_tag, attrs } => wrap(dom, wrap_tag, attrs, tag),
            Action::Insert(html) => insert_snippet(dom, html, true, tag),
            Action::InsertEnd(html) => insert_snippet(dom, html, false, tag),
            Action::Prepend(html) => append_snippet(dom, html, true, tag),
            Action::Append(html) => append_snippet(dom, html, false, tag),
        }
    }
}

fn rename_tag(dom: &mut Dom, new_name: &str, tag: NodeId) -> bool {
    if dom.tag(tag) != Some(new_name) {
        dom.set_tag(tag, new_name);
        true
    } else {
        false
    }
}

fn class_list(dom: &Dom, tag: NodeId) -> Vec<String> {
    dom.node(tag)
        .attrs
        .get("class")
        .map(|c| c.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn set_class_list(dom: &mut Dom, tag: NodeId, classes: &[String]) {
    if classes.is_empty() {
        dom.node_mut(tag).attrs.shift_remove("class");
    } else {
        dom.node_mut(tag).attrs.insert("class".to_string(), classes.join(" "));
    }
}

/// Port of `add_classes`: appends any of `classes` not already present,
/// preserving insertion order and de-duplicating (`tag_mapper.uniq`).
fn add_classes(dom: &mut Dom, classes: &[String], tag: NodeId) -> bool {
    let orig = class_list(dom, tag);
    let mut seen: std::collections::HashSet<&str> = orig.iter().map(String::as_str).collect();
    let mut merged = orig.clone();
    for c in classes {
        if seen.insert(c.as_str()) {
            merged.push(c.clone());
        }
    }
    if merged != orig {
        let orig_attr = dom.node(tag).attrs.get("class").cloned().unwrap_or_default();
        set_class_list(dom, tag, &merged);
        merged.join(" ") != orig_attr
    } else {
        false
    }
}

/// Port of `remove_classes`: removes every occurrence of each named
/// class.
fn remove_classes(dom: &mut Dom, classes: &[String], tag: NodeId) -> bool {
    let orig = class_list(dom, tag);
    let filtered: Vec<String> = orig.iter().filter(|c| !classes.contains(c)).cloned().collect();
    if filtered != orig {
        set_class_list(dom, tag, &filtered);
        true
    } else {
        false
    }
}

/// Port of `remove_attrs`. `"*"` clears every attribute.
fn remove_attrs(dom: &mut Dom, attrs: &[String], tag: NodeId) -> bool {
    let node = dom.node_mut(tag);
    if node.attrs.is_empty() {
        return false;
    }
    let mut changed = false;
    for a in attrs {
        if a == "*" {
            changed = true;
            node.attrs.clear();
        } else if node.attrs.shift_remove(a).is_some() {
            changed = true;
        }
    }
    changed
}

/// Port of `add_attrs`: sets each `(name, value)` pair, overwriting any
/// existing value.
fn add_attrs(dom: &mut Dom, attrs: &[(String, String)], tag: NodeId) -> bool {
    let mut changed = false;
    for (k, v) in attrs {
        let node = dom.node_mut(tag);
        if node.attrs.get(k) != Some(v) {
            changed = true;
        }
        node.attrs.insert(k.clone(), v.clone());
    }
    changed
}

/// Port of `empty`: drops every child (text and element alike -- in
/// this crate's standard-DOM tree, text is already just another child
/// node, so there's no separate `tag.text` to clear).
fn empty(dom: &mut Dom, tag: NodeId) -> bool {
    let had_children = !dom.node(tag).children.is_empty();
    dom.node_mut(tag).children.clear();
    had_children
}

/// Port of `parse_attrs`: parses a bare attribute-list fragment (e.g.
/// `class="red" name="test"`) via the same "parse a synthetic wrapper,
/// read its own attributes back" trick this crate already uses
/// elsewhere for fragment parsing.
fn parse_attrs(text: &str) -> Vec<(String, String)> {
    let dom = Dom::parse(&format!("<div {text}></div>"));
    let Some(div) = dom.find_first_tag_global("div") else {
        return Vec::new();
    };
    dom.node(div).attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Port of `parse_start_tag`: parses an opening-tag fragment (e.g.
/// `<div class="box">`) into its tag name and attributes, for the
/// `wrap` action.
fn parse_start_tag(text: &str) -> Result<(String, Vec<(String, String)>)> {
    let dom = Dom::parse(text);
    let body = dom.find_first_tag_global("body").context("parsing a start-tag fragment")?;
    let first_elem = dom
        .children(body)
        .into_iter()
        .find(|&c| matches!(dom.node(c).kind, NodeKind::Element(_)))
        .with_context(|| format!("No tag found in: {text}. The tag specification must be of the form <tag> for example: <p>"))?;
    let tag = dom.tag(first_elem).unwrap_or_default().to_string();
    let attrs = dom.node(first_elem).attrs.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    Ok((tag, attrs))
}

/// Port of `wrap`: inserts a new `<tag attrs...>` element at `tag`'s own
/// position and moves `tag` inside it. No manual "move the tail text"
/// step is needed (unlike lxml's `.text`/`.tail` model) -- in this
/// crate's standard-DOM tree, `tag`'s following siblings are already
/// separate nodes that stay exactly where they are.
fn wrap(dom: &mut Dom, tag_name: &str, attrs: &[(String, String)], tag: NodeId) -> bool {
    let Some(parent) = dom.parent(tag) else {
        return false;
    };
    let idx = dom.index_in_parent(tag).unwrap_or(0);
    let wrapper = dom.new_element(tag_name);
    for (k, v) in attrs {
        dom.node_mut(wrapper).attrs.insert(k.clone(), v.clone());
    }
    dom.insert_child(parent, idx, wrapper);
    dom.append_child(wrapper, tag);
    true
}

/// Parses an HTML snippet (`<div>{html}</div>`, matching
/// `parse_html_snippet`'s own `fragment_context='div'` convention) and
/// returns the synthetic wrapper's own children -- the snippet's real
/// top-level content.
fn parse_snippet_children(html: &str) -> (Dom, Vec<NodeId>) {
    let dom = Dom::parse(&format!("<div>{html}</div>"));
    let children = dom.find_first_tag_global("div").map(|d| dom.children(d)).unwrap_or_default();
    (dom, children)
}

/// Port of `insert_snippet`: splices a parsed HTML snippet in as `tag`'s
/// own children, either at the start (`insert`) or end (`insert_end`).
fn insert_snippet(dom: &mut Dom, html: &str, before_children: bool, tag: NodeId) -> bool {
    let (snippet_dom, children) = parse_snippet_children(html);
    if before_children {
        for (i, &child) in children.iter().enumerate() {
            let cloned = dom.clone_from(&snippet_dom, child);
            dom.insert_child(tag, i, cloned);
        }
    } else {
        for &child in &children {
            let cloned = dom.clone_from(&snippet_dom, child);
            dom.append_child(tag, cloned);
        }
    }
    true
}

/// Port of `append_snippet`: splices a parsed HTML snippet in as
/// `tag`'s own siblings, either right before it (`prepend`) or right
/// after it (`append`).
fn append_snippet(dom: &mut Dom, html: &str, before_tag: bool, tag: NodeId) -> bool {
    let Some(parent) = dom.parent(tag) else {
        return false;
    };
    let (snippet_dom, children) = parse_snippet_children(html);
    let base_idx = dom.index_in_parent(tag).unwrap_or(0);
    let insert_at = if before_tag { base_idx } else { base_idx + 1 };
    for (i, &child) in children.iter().enumerate() {
        let cloned = dom.clone_from(&snippet_dom, child);
        dom.insert_child(parent, insert_at + i, cloned);
    }
    true
}

#[derive(Debug)]
enum RuleSelector {
    Css(SelectorList),
    /// An element whose own text-node children contain this substring.
    ContainsText(String),
}

/// Port of `Rule`.
#[derive(Debug)]
pub struct Rule {
    selector: RuleSelector,
    actions: Vec<Action>,
}

/// Port of `validate_rule`'s `allowed_keys` -- a plain struct standing
/// in for the serialized-rule dict real upstream's GUI produces.
#[derive(Debug, Clone)]
pub struct SerializedAction {
    pub kind: String,
    pub data: String,
}

#[derive(Debug, Clone)]
pub struct SerializedRule {
    pub match_type: String,
    pub query: String,
    pub actions: Vec<SerializedAction>,
}

impl Rule {
    /// Port of `Rule.__init__`.
    pub fn new(serialized: &SerializedRule) -> Result<Self> {
        let match_type = MatchType::parse(&serialized.match_type)?;
        let query = serialized.query.clone();
        let selector = match &match_type {
            MatchType::Xpath => {
                bail!("XPath match rules are not supported in this port -- no general XPath engine exists over the HTML tree type")
            }
            MatchType::Is | MatchType::Css => RuleSelector::Css(parse_selector_list(&query).map_err(|e| anyhow::anyhow!("{e}"))?),
            MatchType::Any => RuleSelector::Css(parse_selector_list("*").expect("the universal selector always parses")),
            MatchType::HasClass => {
                let sel = format!(".{query}");
                RuleSelector::Css(parse_selector_list(&sel).map_err(|e| anyhow::anyhow!("{e}"))?)
            }
            MatchType::NotHasClass => {
                let sel = format!(":not(.{query})");
                RuleSelector::Css(parse_selector_list(&sel).map_err(|e| anyhow::anyhow!("{e}"))?)
            }
            MatchType::ContainsText => RuleSelector::ContainsText(query.clone()),
        };
        let actions = serialized
            .actions
            .iter()
            .map(|a| Action::parse(&a.kind, &a.data))
            .collect::<Result<Vec<_>>>()?;
        if actions.is_empty() {
            bail!("The rule has no actions");
        }
        Ok(Rule { selector, actions })
    }

    fn matches(&self, dom: &Dom) -> Vec<NodeId> {
        match &self.selector {
            RuleSelector::Css(selectors) => Select::for_dom(dom)
                .matching(selectors)
                .into_iter()
                .map(|e: DomElement| e.id)
                .collect(),
            RuleSelector::ContainsText(needle) => dom
                .preorder_elements(dom.root)
                .into_iter()
                .filter(|&id| {
                    dom.children(id).iter().any(|&c| matches!(&dom.node(c).kind, NodeKind::Text(t) if t.contains(needle.as_str())))
                })
                .collect(),
        }
    }

    /// Port of `Rule.__call__`: applies every action to every matching
    /// element, returning whether anything changed.
    pub fn apply(&self, dom: &mut Dom) -> bool {
        let mut changed = false;
        for tag in self.matches(dom) {
            for action in &self.actions {
                if action.apply(dom, tag) {
                    changed = true;
                }
            }
        }
        changed
    }
}

/// Port of `transform_doc`.
pub fn transform_doc(dom: &mut Dom, rules: &[Rule]) -> bool {
    let mut changed = false;
    for rule in rules {
        if rule.apply(dom) {
            changed = true;
        }
    }
    changed
}

/// Port of `transform_html`.
pub fn transform_html(html: &str, serialized_rules: &[SerializedRule]) -> Result<(bool, String)> {
    let mut dom = Dom::parse(html);
    let rules = serialized_rules.iter().map(Rule::new).collect::<Result<Vec<_>>>()?;
    let changed = transform_doc(&mut dom, &rules);
    Ok((changed, dom.serialize(dom.root)))
}

/// Port of `transform_container`: applies every rule to every real
/// (X)HTML document in `container` (or just `names`, if given).
pub fn transform_container(
    container: &mut crate::oeb::polish::container::Container,
    serialized_rules: &[SerializedRule],
    names: &[String],
) -> Result<bool> {
    use crate::oeb::constants::OEB_DOCS;

    let rules = serialized_rules.iter().map(Rule::new).collect::<Result<Vec<_>>>()?;
    let target_names: Vec<String> = if names.is_empty() {
        container
            .base
            .mime_map
            .iter()
            .filter(|(_, mt)| OEB_DOCS.contains(&mt.as_str()))
            .map(|(n, _)| n.clone())
            .collect()
    } else {
        names.to_vec()
    };

    let mut doc_changed = false;
    for name in target_names {
        let Some(mt) = container.base.mime_map.get(&name).cloned() else {
            continue;
        };
        if !OEB_DOCS.contains(&mt.as_str()) {
            continue;
        }
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml_mut(&name)?;
        if transform_doc(dom, &rules) {
            container.dirty(&name);
            doc_changed = true;
        }
    }
    Ok(doc_changed)
}

/// Port of `rule_to_text`, using [`MatchType::label`]/each action's own
/// short label instead of the dropped GUI `ACTION_MAP`/`MATCH_TYPE_MAP`
/// display strings (see the module doc).
fn action_label(kind: &str) -> &'static str {
    match kind {
        "rename" => "Change tag name",
        "remove" => "Remove tag and children",
        "unwrap" => "Remove tag only",
        "add_classes" => "Add classes",
        "remove_classes" => "Remove classes",
        "remove_attrs" => "Remove attributes",
        "add_attrs" => "Add attributes",
        "empty" => "Empty the tag",
        "wrap" => "Wrap the tag",
        "insert" => "Insert HTML at start",
        "insert_end" => "Insert HTML at end",
        "prepend" => "Insert HTML before tag",
        "append" => "Insert HTML after tag",
        _ => "",
    }
}

pub fn rule_to_text(rule: &SerializedRule) -> Result<String> {
    let mt = MatchType::parse(&rule.match_type)?;
    let mut text = format!("If the tag {} {}", mt.label(), rule.query);
    for action in &rule.actions {
        text.push('\n');
        text.push_str(&format!("{} {}", action_label(&action.kind), action.data));
    }
    Ok(text)
}

const ALLOWED_KEYS: &[&str] = &["match_type", "query", "actions"];

/// Port of `export_rules`: the same plain-text (not JSON) rule-file
/// format real upstream's GUI import/export uses.
pub fn export_rules(rules: &[SerializedRule]) -> Result<Vec<u8>> {
    let mut lines: Vec<String> = Vec::new();
    for rule in rules {
        for l in rule_to_text(rule)?.lines() {
            lines.push(format!("# {l}"));
        }
        lines.push(format!("match_type: {}", rule.match_type.replace('\n', " ")));
        lines.push(format!("query: {}", rule.query.replace('\n', " ")));
        for action in &rule.actions {
            lines.push(format!("action: {}: {}", action.kind, action.data));
        }
        lines.push(String::new());
    }
    Ok(lines.join("\n").into_bytes())
}

/// Port of `import_rules`.
pub fn import_rules(raw_data: &[u8]) -> Result<Vec<SerializedRule>> {
    let text = String::from_utf8(raw_data.to_vec()).context("rule file is not valid UTF-8")?;
    let mut out = Vec::new();
    let mut match_type = String::new();
    let mut query = String::new();
    let mut actions: Vec<SerializedAction> = Vec::new();
    let mut has_current = false;

    let flush = |out: &mut Vec<SerializedRule>, match_type: &mut String, query: &mut String, actions: &mut Vec<SerializedAction>, has_current: &mut bool| {
        if *has_current {
            out.push(SerializedRule {
                match_type: std::mem::take(match_type),
                query: std::mem::take(query),
                actions: std::mem::take(actions),
            });
        }
        *has_current = false;
    };

    for line in text.lines() {
        if line.trim().is_empty() {
            flush(&mut out, &mut match_type, &mut query, &mut actions, &mut has_current);
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.to_lowercase();
        let k = k.trim();
        let v = v.trim();
        has_current = true;
        if k == "action" {
            let (t, d) = v.split_once(':').map(|(t, d)| (t.trim(), d.trim())).unwrap_or((v.trim(), ""));
            actions.push(SerializedAction {
                kind: t.to_string(),
                data: d.to_string(),
            });
        } else if ALLOWED_KEYS.contains(&k) {
            match k {
                "match_type" => match_type = v.to_string(),
                "query" => query = v.to_string(),
                _ => {}
            }
        }
    }
    flush(&mut out, &mut match_type, &mut query, &mut actions, &mut has_current);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(match_type: &str, query: &str, actions: Vec<(&str, &str)>) -> SerializedRule {
        SerializedRule {
            match_type: match_type.to_string(),
            query: query.to_string(),
            actions: actions.into_iter().map(|(k, d)| SerializedAction { kind: k.to_string(), data: d.to_string() }).collect(),
        }
    }

    #[test]
    fn renaming_a_tag_by_css_selector() {
        let (changed, html) = transform_html("<p class='x'>hi</p>", &[rule("css", "p.x", vec![("rename", "div")])]).unwrap();
        assert!(changed);
        assert!(html.contains("<div"), "{html}");
        assert!(!html.contains("<p "), "{html}");
    }

    #[test]
    fn removing_a_tag_and_its_children() {
        let (changed, html) = transform_html("<p>keep</p><p class='drop'>gone<b>bye</b></p>", &[rule("has_class", "drop", vec![("remove", "")])]).unwrap();
        assert!(changed);
        assert!(html.contains("keep"), "{html}");
        assert!(!html.contains("gone"), "{html}");
        assert!(!html.contains("bye"), "{html}");
    }

    #[test]
    fn unwrapping_a_tag_keeps_its_children_and_siblings_in_place() {
        let (changed, html) = transform_html("<div>before<span class='w'>middle</span>after</div>", &[rule("has_class", "w", vec![("unwrap", "")])]).unwrap();
        assert!(changed);
        assert!(!html.contains("<span"), "{html}");
        assert!(html.contains("before"), "{html}");
        assert!(html.contains("middle"), "{html}");
        assert!(html.contains("after"), "{html}");
    }

    #[test]
    fn not_has_class_matches_everything_else() {
        let (_changed, html) = transform_html(
            "<p class='a'>one</p><p class='b'>two</p>",
            &[rule("not_has_class", "a", vec![("add_classes", "marked")])],
        )
        .unwrap();
        assert!(!html.contains("class=\"a marked\""), "{html}");
        assert!(html.contains("class=\"b marked\""), "{html}");
    }

    #[test]
    fn add_and_remove_classes_deduplicate_and_preserve_order() {
        let (_c, html) = transform_html("<p class='a b'>x</p>", &[rule("is", "p", vec![("add_classes", "b c")])]).unwrap();
        assert!(html.contains("class=\"a b c\""), "{html}");

        let (_c, html) = transform_html("<p class='a b c'>x</p>", &[rule("is", "p", vec![("remove_classes", "b")])]).unwrap();
        assert!(html.contains("class=\"a c\""), "{html}");
    }

    #[test]
    fn remove_attrs_star_clears_everything() {
        let (changed, html) = transform_html("<p id='x' data-y='1'>hi</p>", &[rule("is", "p", vec![("remove_attrs", "*")])]).unwrap();
        assert!(changed);
        assert!(!html.contains("id="), "{html}");
        assert!(!html.contains("data-y="), "{html}");
    }

    #[test]
    fn add_attrs_sets_new_values() {
        let (_c, html) = transform_html("<p>hi</p>", &[rule("is", "p", vec![("add_attrs", "class=\"red\" data-x=\"1\"")])]).unwrap();
        assert!(html.contains("class=\"red\""), "{html}");
        assert!(html.contains("data-x=\"1\""), "{html}");
    }

    #[test]
    fn empty_drops_all_children() {
        let (changed, html) = transform_html("<p>hi <b>there</b></p>", &[rule("is", "p", vec![("empty", "")])]).unwrap();
        assert!(changed);
        assert!(html.contains("<p"), "{html}");
        assert!(!html.contains("there"), "{html}");
    }

    #[test]
    fn wrap_inserts_a_new_parent_around_the_matched_tag() {
        let (changed, html) = transform_html("<span>before</span><p class='x'>hi</p><span>after</span>", &[rule("has_class", "x", vec![("wrap", "<div class=\"box\">")])]).unwrap();
        assert!(changed);
        assert!(html.contains("<div class=\"box\"><p"), "{html}");
        assert!(html.contains("before"), "{html}");
        assert!(html.contains("after"), "{html}");
    }

    #[test]
    fn insert_and_insert_end_splice_html_inside_the_tag() {
        let (_c, html) = transform_html("<p class='x'>middle</p>", &[rule("has_class", "x", vec![("insert", "<b>start</b>")])]).unwrap();
        assert!(html.contains("<p class=\"x\"><b>start</b>middle"), "{html}");

        let (_c, html) = transform_html("<p class='x'>middle</p>", &[rule("has_class", "x", vec![("insert_end", "<b>end</b>")])]).unwrap();
        assert!(html.contains("middle<b>end</b></p>"), "{html}");
    }

    #[test]
    fn prepend_and_append_splice_html_as_siblings() {
        let (_c, html) = transform_html("<p class='x'>hi</p>", &[rule("has_class", "x", vec![("prepend", "<hr>")])]).unwrap();
        assert!(html.contains("<hr"), "{html}");
        let hr_pos = html.find("<hr").unwrap();
        let p_pos = html.find("<p").unwrap();
        assert!(hr_pos < p_pos, "{html}");

        let (_c, html) = transform_html("<p class='x'>hi</p>", &[rule("has_class", "x", vec![("append", "<hr>")])]).unwrap();
        let hr_pos = html.find("<hr").unwrap();
        let p_pos = html.find("<p").unwrap();
        assert!(p_pos < hr_pos, "{html}");
    }

    #[test]
    fn contains_text_matches_an_elements_own_text() {
        let (changed, html) = transform_html("<p>hello world</p><p>other</p>", &[rule("contains_text", "hello", vec![("add_classes", "hit")])]).unwrap();
        assert!(changed);
        assert!(html.contains("class=\"hit\""), "{html}");
    }

    #[test]
    fn xpath_match_type_is_a_real_reported_error() {
        let err = Rule::new(&rule("xpath", "//p", vec![("remove", "")])).unwrap_err();
        assert!(err.to_string().contains("XPath"), "{err}");
    }

    #[test]
    fn export_then_import_round_trips_a_rule() {
        let original = vec![rule("has_class", "drop-me", vec![("remove", ""), ("add_classes", "x y")])];
        let exported = export_rules(&original).unwrap();
        let text = String::from_utf8(exported.clone()).unwrap();
        assert!(text.contains("# If the tag"), "{text}");
        assert!(text.contains("match_type: has_class"), "{text}");
        assert!(text.contains("query: drop-me"), "{text}");
        assert!(text.contains("action: remove:"), "{text}");
        assert!(text.contains("action: add_classes: x y"), "{text}");

        let imported = import_rules(&exported).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].match_type, "has_class");
        assert_eq!(imported[0].query, "drop-me");
        assert_eq!(imported[0].actions.len(), 2);
        assert_eq!(imported[0].actions[0].kind, "remove");
        assert_eq!(imported[0].actions[1].data, "x y");
    }

    #[test]
    fn a_rule_with_no_actions_is_a_real_reported_error() {
        let err = Rule::new(&rule("is", "p", vec![])).unwrap_err();
        assert!(err.to_string().contains("no actions"), "{err}");
    }
}
