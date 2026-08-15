//! Port of `old_src/src/calibre/ebooks/oeb/polish/css.py`: selector-usage
//! tracking, unused-CSS removal, rule merging/deduplication, and class
//! renaming. This is the file issue #164 exists for -- see
//! `crate::css`'s module docs for the parsing/selector-matching layer it
//! is built on.
//!
//! # Naming note
//!
//! This is `oeb::polish::css`, the port of `css.py`; the underlying
//! parser/object-model/selector-matcher crate it depends on is
//! `crate::css` (top-level, reusable outside `polish/` too). The two are
//! deliberately different modules -- see `crate::css`'s docs for why the
//! engine lives at the crate root rather than nested under `polish/`.
//!
//! # Divergences from Python, by necessity of the Rust object model
//!
//! - **Selector usage is not a mutable attribute on the selector
//!   object.** Python stamps `selector.calibre_used = True/False`
//!   directly on `css_selectors` selector objects and checks
//!   `getattr(selector, 'calibre_used', False)` to accumulate usage
//!   across multiple content documents referencing the same external
//!   sheet. [`Selector`] has no such field (mutating a shared, cached
//!   object in place isn't how this crate's owned-`Stylesheet`-per-call
//!   design works -- see [`crate::css::model`]'s docs and
//!   `Container::parsed_stylesheet`'s). This port instead threads an
//!   explicit `sheet name -> rule index -> per-selector bool` usage map
//!   through [`remove_unused_css`], OR-accumulated across every content
//!   document, then applies removals in one pass at the end. Externally
//!   observable behavior (what ends up removed) matches; the "assume
//!   used if never checked" default (Python's `getattr(..., True)` in
//!   [`remove_unused_selectors_and_rules`]) is preserved.
//! - **Class renaming is structural where Python is regex-based.**
//!   [`rename_class_in_rule_list`] renames the class in each selector's
//!   *parsed* `.classes`, not `selectorText` via regex -- more precise
//!   than Python's documented approximation (which can also match inside
//!   `#id` selectors containing the substring; see `rename_class_in_rule_list`'s
//!   Python docstring), not less. `rename_class_in_doc`'s `class=""`
//!   attribute rename is still token-based string replacement (an HTML
//!   attribute has no structured selector to walk), which normalizes
//!   inter-token whitespace to single spaces -- a cosmetic difference
//!   only.
//! - **No `normalizers` dict** (see `cascade.rs`'s
//!   [`crate::oeb::polish::cascade::iterdeclaration`] docs): `filter_declaration`
//!   only removes properties by their literal name, it does not also
//!   expand-then-filter shorthand properties a removed name might be
//!   part of. [`crate::oeb::normalize_css::normalize_filter_css`] already
//!   documents the same narrowing for the five shorthands this crate
//!   *does* have a normalizer for (`margin`/`padding`/`border-style`/
//!   `border-width`/`border-color`), so `filter_css` (which calls it)
//!   still gets those five right.
//! - **No ICU natural sort.** [`sort_sheet`] uses plain string ordering
//!   (`Ord` on `&str`) where Python uses `calibre.utils.icu.numeric_sort_key`
//!   (which treats embedded digit runs numerically, e.g. `"item2" <
//!   "item10"`); this crate has no ICU natural-sort port. Ordering is
//!   still total and stable, just not digit-aware.
//! - **No cosmetic re-indentation of rewritten `<style>` text.**
//!   [`crate::css::model::Stylesheet::to_css_text`] already emits valid,
//!   readable, multi-line CSS (one declaration per line, rules separated
//!   by a blank line); Python additionally re-indents rewritten
//!   `<style>` tag content to match its nesting depth in the surrounding
//!   HTML (`pretty_script_or_style`). This port does not perform that
//!   second, purely cosmetic pass.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::css::model::UnknownAtRule;
use crate::css::{Rule, Select, SelectorList, StyleDeclarationBlock, Stylesheet};
use crate::mobi::dom::{Dom, NodeId};
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::normalize_css::normalize_filter_css;
use crate::oeb::polish::fonts::{set_dom_element_text_only, style_tag_is_css};

use super::container::Container;

// ===================================================================
// Selector usage tracking / unused-CSS removal
// ===================================================================

/// Sheet name -> (index into `Stylesheet::rules` of a `Rule::Style`) ->
/// per-selector used flag. See the module docs' first divergence note.
type UsageMap = HashMap<String, HashMap<usize, Vec<bool>>>;

/// Port of `mark_used_selectors`, adapted to this crate's owned/indexed
/// `Stylesheet` shape (see the module docs): checks every selector of
/// every `Rule::Style` in `sheet.rules` against `select`, OR-ing the
/// result into `usage[sheet_name]`.
fn mark_sheet_usage(
    sheet_name: &str,
    sheet: &Stylesheet,
    select: &Select<crate::css::DomElement<'_>>,
    usage: &mut UsageMap,
) {
    let entry = usage.entry(sheet_name.to_string()).or_default();
    for (i, rule) in sheet.rules.iter().enumerate() {
        let Rule::Style(sr) = rule else { continue };
        let bools = entry
            .entry(i)
            .or_insert_with(|| vec![false; sr.selectors.0.len()]);
        if bools.len() < sr.selectors.0.len() {
            bools.resize(sr.selectors.0.len(), false);
        }
        for (j, sel) in sr.selectors.0.iter().enumerate() {
            if bools[j] {
                continue; // already known used; skip re-checking (matches Python's getattr short-circuit)
            }
            let single = SelectorList(vec![sel.clone()]);
            if select.has_matches(&single) {
                bools[j] = true;
            }
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct RemovalStats {
    rules: u32,
    selectors: u32,
}

/// Port of `remove_unused_selectors_and_rules`: removes every selector
/// (and, if all of a rule's selectors are unused, the whole rule) not
/// marked used in `usage[sheet_name]`. A rule/selector never checked at
/// all (no entry in `usage`) is treated as used, matching Python's
/// `getattr(sel, 'calibre_used', True)` default. Returns whether
/// anything was removed.
fn remove_unused_selectors_and_rules(
    sheet: &mut Stylesheet,
    sheet_name: &str,
    usage: &UsageMap,
    stats: &mut RemovalStats,
) -> bool {
    let mut any_unused = false;
    let empty = HashMap::new();
    let sheet_usage = usage.get(sheet_name).unwrap_or(&empty);
    let mut fully_remove = Vec::new();
    for (i, rule) in sheet.rules.iter_mut().enumerate() {
        let Rule::Style(sr) = rule else { continue };
        let Some(bools) = sheet_usage.get(&i) else {
            continue;
        };
        let n = sr.selectors.0.len();
        let removals: Vec<usize> = (0..n)
            .filter(|&j| !bools.get(j).copied().unwrap_or(true))
            .collect();
        if removals.is_empty() {
            continue;
        }
        any_unused = true;
        if removals.len() == n {
            fully_remove.push(i);
            stats.rules += 1;
        } else {
            stats.selectors += removals.len() as u32;
            for &j in removals.iter().rev() {
                sr.selectors.0.remove(j);
            }
            sr.sync_selector_text();
        }
    }
    for i in fully_remove.into_iter().rev() {
        sheet.rules.remove(i);
    }
    any_unused
}

/// Port of `get_imported_sheets`: names (from `sheets`) transitively
/// `@import`-ed by the sheet named `name` (or, if `seed` is given,
/// `@import`-ed by `seed` directly, resolved relative to `name` -- the
/// shape `remove_unused_css` needs for an inline `<style>` tag's own
/// `@import` rules).
pub fn get_imported_sheets(
    name: &str,
    container: &Container,
    sheets: &HashMap<String, Stylesheet>,
    seed: Option<&Stylesheet>,
    recursion_level: u32,
) -> HashSet<String> {
    let Some(sheet) = seed.or_else(|| sheets.get(name)) else {
        return HashSet::new();
    };
    let mut ans = HashSet::new();
    for imp in sheet.import_rules() {
        if let Some(iname) = container.href_to_name(&imp.href, Some(name)) {
            if sheets.contains_key(&iname) {
                ans.insert(iname);
            }
        }
    }
    if recursion_level > 0 {
        for imported in ans.clone().into_iter() {
            ans.extend(get_imported_sheets(
                &imported,
                container,
                sheets,
                None,
                recursion_level - 1,
            ));
        }
    }
    ans.remove(name);
    ans
}

/// Port of `merge_declarations`.
pub fn merge_declarations(first: &mut StyleDeclarationBlock, second: &StyleDeclarationBlock) {
    for prop in &second.properties {
        first.set_property(&prop.name, prop.value.clone(), prop.important);
    }
}

/// Port of `merge_identical_selectors`: merges the declarations of every
/// group of style rules sharing the exact same `selectorText` into the
/// first rule of the group, removing the rest. Returns how many rules
/// were removed.
pub fn merge_identical_selectors(sheet: &mut Stylesheet) -> u32 {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, rule) in sheet.rules.iter().enumerate() {
        if let Rule::Style(sr) = rule {
            groups.entry(sr.selector_text.clone()).or_default().push(i);
        }
    }
    let mut remove = Vec::new();
    for idxs in groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        let first = idxs[0];
        for &other in &idxs[1..] {
            let other_style = match &sheet.rules[other] {
                Rule::Style(sr) => sr.style.clone(),
                _ => continue,
            };
            if let Rule::Style(sr0) = &mut sheet.rules[first] {
                merge_declarations(&mut sr0.style, &other_style);
            }
            remove.push(other);
        }
    }
    remove.sort_unstable();
    let n = remove.len();
    for i in remove.into_iter().rev() {
        sheet.rules.remove(i);
    }
    n as u32
}

fn declaration_key(decl: &StyleDeclarationBlock) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = decl
        .properties
        .iter()
        .map(|d| (d.name.to_ascii_lowercase(), d.value.clone()))
        .collect();
    v.sort();
    v
}

/// Port of `merge_identical_properties`: merges every group of style
/// rules sharing the exact same set of properties into one rule (the
/// first of the group) with the union of their selectors, removing the
/// rest. Returns the total number of rules in every group that had 2+
/// members (matching Python's `num_merged += len(rule_group)`, which
/// counts the survivor too, not just the removed rules).
pub fn merge_identical_properties(sheet: &mut Stylesheet) -> u32 {
    let mut groups: HashMap<Vec<(String, String)>, Vec<usize>> = HashMap::new();
    for (i, rule) in sheet.rules.iter().enumerate() {
        if let Rule::Style(sr) = rule {
            groups
                .entry(declaration_key(&sr.style))
                .or_default()
                .push(i);
        }
    }
    let mut num_merged = 0u32;
    let mut remove = Vec::new();
    for idxs in groups.values() {
        if idxs.len() < 2 {
            continue;
        }
        num_merged += idxs.len() as u32;
        let first = idxs[0];
        let mut seen: HashSet<String> = match &sheet.rules[first] {
            Rule::Style(sr) => sr.selectors.0.iter().map(|s| s.text.clone()).collect(),
            _ => continue,
        };
        for &other in &idxs[1..] {
            let other_selectors = match &sheet.rules[other] {
                Rule::Style(sr) => sr.selectors.0.clone(),
                _ => continue,
            };
            if let Rule::Style(sr0) = &mut sheet.rules[first] {
                for s in other_selectors {
                    if seen.insert(s.text.clone()) {
                        sr0.selectors.0.push(s);
                    }
                }
                sr0.sync_selector_text();
            }
            remove.push(other);
        }
    }
    remove.sort_unstable();
    for i in remove.into_iter().rev() {
        sheet.rules.remove(i);
    }
    num_merged
}

/// Options for [`remove_unused_css`], matching `remove_unused_css`'s
/// Python keyword arguments.
#[derive(Debug, Default, Clone, Copy)]
pub struct RemoveUnusedCssOptions {
    pub remove_unused_classes: bool,
    pub merge_rules: bool,
    pub merge_rules_with_identical_properties: bool,
    pub remove_unreferenced_sheets: bool,
}

fn lower_set(s: &HashSet<String>) -> HashSet<String> {
    s.iter().map(|c| c.to_lowercase()).collect()
}

/// Port of `remove_unused_css`. Removes CSS rules/selectors that don't
/// match any content in the book. See the module docs for how selector
/// usage tracking is adapted to this crate's owned-`Stylesheet` model.
pub fn remove_unused_css(
    container: &mut Container,
    options: RemoveUnusedCssOptions,
    mut report: impl FnMut(&str),
) -> Result<bool> {
    let style_names: Vec<String> = container
        .base
        .mime_map
        .iter()
        .filter(|(_, mt)| OEB_STYLES.contains(&mt.as_str()))
        .map(|(n, _)| n.clone())
        .collect();
    let mut sheets: HashMap<String, Stylesheet> = HashMap::new();
    for name in &style_names {
        if let Ok(sheet) = container.parsed_stylesheet(name) {
            sheets.insert(name.clone(), sheet);
        }
    }

    let mut num_merged = 0u32;
    let mut num_rules_merged = 0u32;
    if options.merge_rules {
        let names: Vec<String> = sheets.keys().cloned().collect();
        for name in &names {
            let n = merge_identical_selectors(sheets.get_mut(name).unwrap());
            if n > 0 {
                container.dirty(name);
                num_merged += n;
            }
        }
    }
    if options.merge_rules_with_identical_properties {
        let names: Vec<String> = sheets.keys().cloned().collect();
        for name in &names {
            let n = merge_identical_properties(sheets.get_mut(name).unwrap());
            if n > 0 {
                container.dirty(name);
                num_rules_merged += n;
            }
        }
    }

    let import_map: HashMap<String, HashSet<String>> = sheets
        .keys()
        .map(|name| {
            (
                name.clone(),
                get_imported_sheets(name, container, &sheets, None, 10),
            )
        })
        .collect();
    let mut unreferenced_sheets: HashSet<String> = sheets.keys().cloned().collect();

    let class_map: HashMap<String, HashSet<String>> = if options.remove_unused_classes {
        sheets
            .iter()
            .map(|(name, sheet)| (name.clone(), lower_set(&classes_in_rule_list(&sheet.rules))))
            .collect()
    } else {
        HashMap::new()
    };

    let mut usage: UsageMap = HashMap::new();
    let mut removal_stats = RemovalStats::default();
    let mut num_of_removed_classes = 0u32;

    let doc_names: Vec<String> = container
        .base
        .mime_map
        .iter()
        .filter(|(_, mt)| OEB_DOCS.contains(&mt.as_str()))
        .map(|(n, _)| n.clone())
        .collect();

    for name in &doc_names {
        container.ensure_parsed(name)?;

        struct StyleTagInfo {
            id: NodeId,
            text: String,
        }
        let (style_tags, link_hrefs, class_elems) = {
            let dom = container.get_xhtml(name)?;
            let mut style_tags = Vec::new();
            let mut link_hrefs = Vec::new();
            let mut class_elems = Vec::new();
            for id in dom.preorder_elements(dom.root) {
                match dom.tag(id) {
                    Some("style") => {
                        if style_tag_is_css(dom, id) {
                            let text = dom.text_content(id);
                            if !text.trim().is_empty() {
                                style_tags.push(StyleTagInfo { id, text });
                            }
                        }
                    }
                    Some("link") => {
                        if let Some(href) = dom.node(id).attrs.get("href") {
                            link_hrefs.push(href.clone());
                        }
                    }
                    _ => {}
                }
                if options.remove_unused_classes {
                    if let Some(c) = dom.node(id).attrs.get("class") {
                        let classes: Vec<String> =
                            c.split_whitespace().map(|s| s.to_string()).collect();
                        if !classes.is_empty() {
                            class_elems.push((id, classes));
                        }
                    }
                }
            }
            (style_tags, link_hrefs, class_elems)
        };

        let mut used_classes: HashSet<String> = HashSet::new();
        let mut style_tag_updates: Vec<(NodeId, String)> = Vec::new();

        {
            let dom = container.get_xhtml(name)?;
            let select = Select::for_dom(dom);

            for tag in &style_tags {
                let mut inline_sheet = Stylesheet::parse(&tag.text);
                if options.merge_rules {
                    let n = merge_identical_selectors(&mut inline_sheet);
                    if n > 0 {
                        num_merged += n;
                    }
                }
                if options.merge_rules_with_identical_properties {
                    let n = merge_identical_properties(&mut inline_sheet);
                    if n > 0 {
                        num_rules_merged += n;
                    }
                }
                if options.remove_unused_classes {
                    used_classes.extend(lower_set(&classes_in_rule_list(&inline_sheet.rules)));
                }
                let imports =
                    get_imported_sheets(name, container, &sheets, Some(&inline_sheet), 10);
                for imported in &imports {
                    unreferenced_sheets.remove(imported);
                    if let Some(s) = sheets.get(imported) {
                        mark_sheet_usage(imported, s, &select, &mut usage);
                    }
                    if options.remove_unused_classes {
                        if let Some(c) = class_map.get(imported) {
                            used_classes.extend(c.iter().cloned());
                        }
                    }
                }
                let mut local_usage: UsageMap = HashMap::new();
                mark_sheet_usage("__inline__", &inline_sheet, &select, &mut local_usage);
                let any_unused = remove_unused_selectors_and_rules(
                    &mut inline_sheet,
                    "__inline__",
                    &local_usage,
                    &mut removal_stats,
                );
                if any_unused {
                    style_tag_updates.push((tag.id, inline_sheet.to_css_text()));
                }
            }

            for href in &link_hrefs {
                let Some(sname) = container.href_to_name(href, Some(name)) else {
                    continue;
                };
                let Some(s) = sheets.get(&sname) else {
                    continue;
                };
                mark_sheet_usage(&sname, s, &select, &mut usage);
                if options.remove_unused_classes {
                    if let Some(c) = class_map.get(&sname) {
                        used_classes.extend(c.iter().cloned());
                    }
                }
                unreferenced_sheets.remove(&sname);
                if let Some(imports) = import_map.get(&sname) {
                    for iname in imports {
                        unreferenced_sheets.remove(iname);
                        if let Some(s) = sheets.get(iname) {
                            mark_sheet_usage(iname, s, &select, &mut usage);
                        }
                        if options.remove_unused_classes {
                            if let Some(c) = class_map.get(iname) {
                                used_classes.extend(c.iter().cloned());
                            }
                        }
                    }
                }
            }
        }

        let mut class_attr_updates: Vec<(NodeId, Option<String>)> = Vec::new();
        if options.remove_unused_classes {
            for (id, original_classes) in &class_elems {
                let kept: Vec<String> = original_classes
                    .iter()
                    .filter(|c| used_classes.contains(&c.to_lowercase()))
                    .cloned()
                    .collect();
                if kept.len() != original_classes.len() {
                    num_of_removed_classes += (original_classes.len() - kept.len()) as u32;
                    if kept.is_empty() {
                        class_attr_updates.push((*id, None));
                    } else {
                        class_attr_updates.push((*id, Some(kept.join(" "))));
                    }
                }
            }
        }

        if !style_tag_updates.is_empty() || !class_attr_updates.is_empty() {
            let dom = container.get_xhtml_mut(name)?;
            for (id, text) in &style_tag_updates {
                set_dom_element_text_only(dom, *id, text);
            }
            for (id, new_val) in &class_attr_updates {
                match new_val {
                    Some(v) => {
                        dom.node_mut(*id)
                            .attrs
                            .insert("class".to_string(), v.clone());
                    }
                    None => {
                        dom.node_mut(*id).attrs.shift_remove("class");
                    }
                }
            }
            container.dirty(name);
        }
    }

    let sheet_names: Vec<String> = sheets.keys().cloned().collect();
    for name in &sheet_names {
        if unreferenced_sheets.contains(name) {
            continue;
        }
        let sheet = sheets.get_mut(name).unwrap();
        if remove_unused_selectors_and_rules(sheet, name, &usage, &mut removal_stats) {
            container.set_css_text(name, sheet.to_css_text());
            container.dirty(name);
        }
    }
    let mut num_sheets_removed = 0u32;
    if options.remove_unreferenced_sheets && !unreferenced_sheets.is_empty() {
        num_sheets_removed = unreferenced_sheets.len() as u32;
        for uname in &unreferenced_sheets {
            container.remove_item(uname, true)?;
        }
    }

    let num_changes = num_merged
        + num_of_removed_classes
        + num_rules_merged
        + removal_stats.rules
        + removal_stats.selectors
        + num_sheets_removed;
    if num_changes > 0 {
        if removal_stats.rules > 0 {
            report(&plural(
                removal_stats.rules,
                "Removed one unused CSS style rule",
                "Removed {} unused CSS style rules",
            ));
        }
        if removal_stats.selectors > 0 {
            report(&plural(
                removal_stats.selectors,
                "Removed one unused CSS selector",
                "Removed {} unused CSS selectors",
            ));
        }
        if num_of_removed_classes > 0 {
            report(&plural(
                num_of_removed_classes,
                "Removed one unused class from the HTML",
                "Removed {} unused classes from the HTML",
            ));
        }
        if num_merged > 0 {
            report(&plural(
                num_merged,
                "Merged one CSS style rule with identical selectors",
                "Merged {} CSS style rules with identical selectors",
            ));
        }
        if num_rules_merged > 0 {
            report(&plural(
                num_rules_merged,
                "Merged one CSS style rule with identical properties",
                "Merged {} CSS style rules with identical properties",
            ));
        }
        if num_sheets_removed > 0 {
            report(&plural(
                num_sheets_removed,
                "Removed one unreferenced stylesheet",
                "Removed {} unreferenced stylesheets",
            ));
        }
    }
    if removal_stats.rules == 0 {
        report("No unused CSS style rules found");
    }
    if removal_stats.selectors == 0 {
        report("No unused CSS selectors found");
    }
    if options.remove_unused_classes && num_of_removed_classes == 0 {
        report("No unused class attributes found");
    }
    if options.merge_rules && num_merged == 0 {
        report("No style rules that could be merged found");
    }
    if options.remove_unreferenced_sheets && num_sheets_removed == 0 {
        report("No unused stylesheets found");
    }
    Ok(num_changes > 0)
}

fn plural(n: u32, one: &str, many_template: &str) -> String {
    if n == 1 {
        one.to_string()
    } else {
        many_template.replace("{}", &n.to_string())
    }
}

// ===================================================================
// Property filtering
// ===================================================================

/// Port of `filter_declaration`. See the module docs: no `normalizers`
/// dict, so this only removes properties by their literal name.
pub fn filter_declaration(style: &mut StyleDeclarationBlock, properties: &HashSet<String>) -> bool {
    let mut changed = false;
    for prop in properties {
        if !style.remove_property(prop).is_empty() {
            changed = true;
        }
    }
    changed
}

/// Port of `filter_sheet`.
pub fn filter_sheet(sheet: &mut Stylesheet, properties: &HashSet<String>) -> bool {
    let mut changed = false;
    let mut remove = Vec::new();
    for (i, rule) in sheet.rules.iter_mut().enumerate() {
        if let Rule::Style(sr) = rule {
            if filter_declaration(&mut sr.style, properties) {
                changed = true;
                if sr.style.is_empty() {
                    remove.push(i);
                }
            }
        }
    }
    for i in remove.into_iter().rev() {
        sheet.rules.remove(i);
    }
    changed
}

/// Port of `transform_inline_styles`.
pub fn transform_inline_styles(
    container: &mut Container,
    name: &str,
    mut transform_sheet: impl FnMut(&mut Stylesheet) -> bool,
    mut transform_style: impl FnMut(&mut StyleDeclarationBlock) -> bool,
) -> Result<bool> {
    container.ensure_parsed(name)?;
    let (style_tags, style_attrs) = {
        let dom = container.get_xhtml(name)?;
        let mut style_tags = Vec::new();
        let mut style_attrs = Vec::new();
        for id in dom.preorder_elements(dom.root) {
            if dom.tag(id) == Some("style") && style_tag_is_css(dom, id) {
                let text = dom.text_content(id);
                if !text.trim().is_empty() {
                    style_tags.push((id, text));
                }
            }
            if let Some(style) = dom.node(id).attrs.get("style") {
                if !style.is_empty() {
                    style_attrs.push((id, style.clone()));
                }
            }
        }
        (style_tags, style_attrs)
    };

    let mut changed = false;
    let mut style_tag_updates = Vec::new();
    for (id, text) in style_tags {
        let mut sheet = Stylesheet::parse(&text);
        if transform_sheet(&mut sheet) {
            changed = true;
            style_tag_updates.push((id, sheet.to_css_text()));
        }
    }
    let mut style_attr_updates = Vec::new();
    for (id, text) in style_attrs {
        let mut decl = crate::css::parser::parse_declaration_list(&text);
        if transform_style(&mut decl) {
            changed = true;
            if decl.is_empty() {
                style_attr_updates.push((id, None));
            } else {
                style_attr_updates.push((id, Some(decl.to_css_text(" "))));
            }
        }
    }

    if changed {
        let dom = container.get_xhtml_mut(name)?;
        for (id, text) in style_tag_updates {
            set_dom_element_text_only(dom, id, &text);
        }
        for (id, new_val) in style_attr_updates {
            match new_val {
                Some(v) => {
                    dom.node_mut(id).attrs.insert("style".to_string(), v);
                }
                None => {
                    dom.node_mut(id).attrs.shift_remove("style");
                }
            }
        }
    }
    Ok(changed)
}

/// Port of `transform_css`.
pub fn transform_css(
    container: &mut Container,
    mut transform_sheet: impl FnMut(&mut Stylesheet) -> bool,
    mut transform_style: impl FnMut(&mut StyleDeclarationBlock) -> bool,
    names: &[String],
) -> Result<bool> {
    let names: Vec<String> = if names.is_empty() {
        container
            .base
            .mime_map
            .iter()
            .filter(|(_, mt)| OEB_STYLES.contains(&mt.as_str()) || OEB_DOCS.contains(&mt.as_str()))
            .map(|(n, _)| n.clone())
            .collect()
    } else {
        names.to_vec()
    };
    let mut doc_changed = false;
    for name in names {
        let mt = container
            .base
            .mime_map
            .get(&name)
            .cloned()
            .unwrap_or_default();
        if OEB_STYLES.contains(&mt.as_str()) {
            let mut sheet = container.parsed_stylesheet(&name)?;
            if transform_sheet(&mut sheet) {
                container.set_css_text(&name, sheet.to_css_text());
                container.dirty(&name);
                doc_changed = true;
            }
        } else if OEB_DOCS.contains(&mt.as_str())
            && transform_inline_styles(
                container,
                &name,
                &mut transform_sheet,
                &mut transform_style,
            )?
        {
            container.dirty(&name);
            doc_changed = true;
        }
    }
    Ok(doc_changed)
}

/// Port of `filter_css`: removes the specified CSS properties from all
/// CSS rules in the book.
pub fn filter_css(
    container: &mut Container,
    properties: &HashSet<String>,
    names: &[String],
) -> Result<bool> {
    let properties = normalize_filter_css(properties);
    transform_css(
        container,
        |sheet| filter_sheet(sheet, &properties),
        |style| filter_declaration(style, &properties),
        names,
    )
}

// ===================================================================
// Class enumeration
// ===================================================================

/// Port of `classes_in_selector`: every class referenced anywhere in
/// `text` (a selector list's source text), including inside `:not()`.
/// Returns an empty set for unparseable text, matching Python's
/// `except SelectorSyntaxError: pass`.
pub fn classes_in_selector(text: &str) -> HashSet<String> {
    crate::css::selector::parse_selector_list(text)
        .map(|l| l.classes())
        .unwrap_or_default()
}

/// Port of `classes_in_rule_list`: every class referenced by any style
/// rule in `rules`, recursing into `@media` (matching Python's
/// `hasattr(rule, 'cssRules')` check, true for media rules).
pub fn classes_in_rule_list(rules: &[Rule]) -> HashSet<String> {
    let mut out = HashSet::new();
    for rule in rules {
        match rule {
            Rule::Style(sr) => out.extend(sr.selectors.classes()),
            Rule::Media(m) => out.extend(classes_in_rule_list(&m.rules)),
            _ => {}
        }
    }
    out
}

// ===================================================================
// Declaration/value iteration
// ===================================================================

/// Port of `iter_declarations`, for the `Stylesheet` case (the shape
/// every real caller in this crate has -- see [`crate::css::model::Stylesheet::iter_declarations`],
/// which this simply re-exposes under `css.py`'s name).
pub fn iter_declarations(sheet: &Stylesheet) -> Vec<&StyleDeclarationBlock> {
    sheet.iter_declarations()
}

/// Port of `remove_property_value`. Python's version takes a `Property`
/// object (which carries a `.parent` back-pointer to its owning
/// declaration) plus a predicate over its `Value` list; this crate's
/// [`crate::css::model::Declaration`] has no parent pointer (see
/// [`crate::css::model::StyleDeclarationBlock`]'s docs), so this takes the
/// owning `style` plus the property's name instead.
pub fn remove_property_value(
    style: &mut StyleDeclarationBlock,
    prop_name: &str,
    predicate: impl Fn(&str) -> bool,
) -> bool {
    style.remove_property_value(prop_name, predicate)
}

// ===================================================================
// Sheet sorting
// ===================================================================

fn rule_priority(rule: &Rule) -> i32 {
    match rule {
        Rule::Charset(_) => 0,
        Rule::Import(_) => 1,
        Rule::Namespace(_) => 2,
        Rule::Style(_) => 4,
        _ => 3,
    }
}

fn rule_atkeyword(rule: &Rule) -> &str {
    match rule {
        Rule::Media(_) => "media",
        Rule::FontFace(_) => "font-face",
        Rule::Import(_) => "import",
        Rule::Charset(_) => "charset",
        Rule::Namespace(_) => "namespace",
        Rule::Unknown(UnknownAtRule { at_keyword, .. }) => at_keyword,
        Rule::Style(_) => "",
    }
}

fn rule_tertiary(rule: &Rule) -> Option<((u32, u32, u32), String)> {
    match rule {
        Rule::Style(sr) => sr
            .selectors
            .0
            .first()
            .map(|s| (s.specificity, s.text.clone())),
        Rule::FontFace(decl) => {
            let v = decl.get_property_value("font-family");
            if v.is_empty() {
                None
            } else {
                Some(((0, 0, 0), v.to_string()))
            }
        }
        _ => None,
    }
}

/// Port of `sort_sheet`. See the module docs: uses plain string
/// ordering, not ICU natural sort.
pub fn sort_sheet(sheet: &mut Stylesheet) {
    for rule in sheet.rules.iter_mut() {
        if let Rule::Style(sr) = rule {
            sr.selectors.0.sort_by(|a, b| {
                a.specificity
                    .cmp(&b.specificity)
                    .then_with(|| a.text.cmp(&b.text))
            });
            sr.sync_selector_text();
        }
    }
    sheet.rules.sort_by(|a, b| {
        rule_priority(a)
            .cmp(&rule_priority(b))
            .then_with(|| rule_atkeyword(a).cmp(rule_atkeyword(b)))
            .then_with(|| rule_tertiary(a).cmp(&rule_tertiary(b)))
    });
}

// ===================================================================
// Adding stylesheet links
// ===================================================================

/// Port of `add_stylesheet_links`: parses `text` as XHTML and appends a
/// `<link rel="stylesheet">` to `<head>` for every stylesheet in the
/// book's manifest. Returns `None` if there is no `<head>` or no
/// stylesheets (matching Python's early `return` with no value).
pub fn add_stylesheet_links(
    container: &mut Container,
    name: &str,
    text: &[u8],
) -> Result<Option<String>> {
    let mut dom = container.base.parse_xhtml(text);
    let Some(head) = dom.find_first_tag_global("head") else {
        return Ok(None);
    };
    let sheets = container.manifest_items_of_type(|mt| OEB_STYLES.contains(&mt))?;
    if sheets.is_empty() {
        return Ok(None);
    }
    for sname in &sheets {
        let href = container.name_to_href(sname, Some(name));
        let link = dom.new_element("link");
        {
            let attrs = &mut dom.node_mut(link).attrs;
            attrs.insert("type".to_string(), "text/css".to_string());
            attrs.insert("rel".to_string(), "stylesheet".to_string());
            attrs.insert("href".to_string(), href);
        }
        dom.append_child(head, link);
    }
    super::pretty::pretty_dom_xml_tree(&mut dom, head, 0, "  ");
    Ok(Some(dom.serialize(dom.root)))
}

// ===================================================================
// Class renaming
// ===================================================================

/// Renames `.old_name` to `.new_name` inside `text` wherever it appears
/// as a class reference (i.e. preceded by a literal `.` and followed by
/// a non-word character or the end of the string) -- port of
/// `rename_class_in_rule_list`'s regex, minus true lookbehind (the
/// `regex` crate doesn't support it): matching the `.` itself and
/// re-emitting it is equivalent. Returns `None` if nothing matched.
fn rename_class_in_text(text: &str, old_name: &str, new_name: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r"\.{}(\W|$)", regex::escape(old_name))).ok()?;
    if !re.is_match(text) {
        return None;
    }
    Some(
        re.replace_all(text, |caps: &regex::Captures| {
            format!(".{new_name}{}", &caps[1])
        })
        .into_owned(),
    )
}

/// Port of `rename_class_in_rule_list`. See the module docs: structural
/// (renames `SimpleSelector.classes`, via a re-parse of the
/// text-renamed selector to keep both in sync), not regex-on-serialized-text
/// like Python.
pub fn rename_class_in_rule_list(rules: &mut [Rule], old_name: &str, new_name: &str) -> bool {
    let mut changed = false;
    for rule in rules.iter_mut() {
        match rule {
            Rule::Style(sr) => {
                let mut rule_changed = false;
                for sel in sr.selectors.0.iter_mut() {
                    if let Some(new_text) = rename_class_in_text(&sel.text, old_name, new_name) {
                        if let Ok(list) = crate::css::selector::parse_selector_list(&new_text) {
                            if let Some(new_sel) = list.0.into_iter().next() {
                                *sel = new_sel;
                                rule_changed = true;
                            }
                        }
                    }
                }
                if rule_changed {
                    sr.sync_selector_text();
                    changed = true;
                }
            }
            Rule::Media(m) => {
                if rename_class_in_rule_list(&mut m.rules, old_name, new_name) {
                    changed = true;
                }
            }
            _ => {}
        }
    }
    changed
}

fn rename_class_in_class_attr(value: &str, old_name: &str, new_name: &str) -> Option<String> {
    let mut changed = false;
    let tokens: Vec<String> = value
        .split_whitespace()
        .map(|t| {
            if t == old_name {
                changed = true;
                new_name.to_string()
            } else {
                t.to_string()
            }
        })
        .collect();
    if changed {
        Some(tokens.join(" "))
    } else {
        None
    }
}

/// Port of `rename_class_in_doc`. Python's version takes a `container`
/// solely to call `container.parse_css`; this crate's `Stylesheet::parse`
/// needs no container, so this only takes the `Dom`.
pub fn rename_class_in_doc(dom: &mut Dom, old_name: &str, new_name: &str) -> bool {
    let mut changed = false;
    let elems_with_class: Vec<NodeId> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&id| dom.node(id).attrs.contains_key("class"))
        .collect();
    for id in elems_with_class {
        let old = dom.node(id).attrs.get("class").cloned().unwrap_or_default();
        if let Some(new_val) = rename_class_in_class_attr(&old, old_name, new_name) {
            dom.node_mut(id).attrs.insert("class".to_string(), new_val);
            changed = true;
        }
    }
    let style_tags: Vec<(NodeId, String)> = dom
        .preorder_elements(dom.root)
        .into_iter()
        .filter(|&id| dom.tag(id) == Some("style") && style_tag_is_css(dom, id))
        .map(|id| (id, dom.text_content(id)))
        .filter(|(_, t)| !t.trim().is_empty())
        .collect();
    let mut updates = Vec::new();
    for (id, text) in style_tags {
        let mut sheet = Stylesheet::parse(&text);
        if rename_class_in_rule_list(&mut sheet.rules, old_name, new_name) {
            updates.push((id, sheet.to_css_text()));
        }
    }
    for (id, text) in updates {
        set_dom_element_text_only(dom, id, &text);
        changed = true;
    }
    changed
}

/// Port of `rename_class`: renames a class everywhere in the book
/// (stylesheets and HTML content documents).
pub fn rename_class(container: &mut Container, old_name: &str, new_name: &str) -> Result<bool> {
    let mut changed = false;
    if old_name.is_empty() || old_name == new_name {
        return Ok(false);
    }
    let sheet_names = container.manifest_items_of_type(|mt| OEB_STYLES.contains(&mt))?;
    for name in sheet_names {
        let mut sheet = container.parsed_stylesheet(&name)?;
        if rename_class_in_rule_list(&mut sheet.rules, old_name, new_name) {
            container.set_css_text(&name, sheet.to_css_text());
            container.dirty(&name);
            changed = true;
        }
    }
    let doc_names = container.manifest_items_of_type(|mt| OEB_DOCS.contains(&mt))?;
    for name in doc_names {
        container.ensure_parsed(&name)?;
        let dom = container.get_xhtml_mut(&name)?;
        if rename_class_in_doc(dom, old_name, new_name) {
            container.dirty(&name);
            changed = true;
        }
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
    fn remove_unused_css_removes_unmatched_selectors_and_rules_but_keeps_used_ones() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head><link rel=\"stylesheet\" href=\"s.css\"/></head>\
                      <body><p class=\"a\">hi</p></body></html>",
                ),
                (
                    "s.css",
                    "text/css",
                    b".a { color: red } .unused { color: blue } p { color: green }",
                ),
            ],
            &["index.html"],
        );
        let mut messages = Vec::new();
        let changed = remove_unused_css(&mut container, RemoveUnusedCssOptions::default(), |m| {
            messages.push(m.to_string())
        })
        .unwrap();
        assert!(changed);
        assert!(container.dirtied.contains("s.css"));
        let sheet = container.parsed_stylesheet("s.css").unwrap();
        let selectors: Vec<&str> = sheet
            .style_rules()
            .map(|r| r.selector_text.as_str())
            .collect();
        assert!(selectors.contains(&".a"), "{selectors:?}");
        assert!(selectors.contains(&"p"), "{selectors:?}");
        assert!(!selectors.contains(&".unused"), "{selectors:?}");
        assert!(messages.iter().any(|m| m.contains("unused CSS style rule")));
    }

    #[test]
    fn remove_unused_css_removes_unreferenced_sheets_when_asked() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><body><p>hi</p></body></html>",
                ),
                ("orphan.css", "text/css", b".a { color: red }"),
            ],
            &["index.html"],
        );
        let options = RemoveUnusedCssOptions {
            remove_unreferenced_sheets: true,
            ..Default::default()
        };
        let changed = remove_unused_css(&mut container, options, |_| {}).unwrap();
        assert!(changed);
        assert!(!container.has_name("orphan.css"));
    }

    #[test]
    fn remove_unused_css_removes_unused_classes_from_html() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head><link rel=\"stylesheet\" href=\"s.css\"/></head>\
                      <body><p class=\"a b\">hi</p></body></html>",
                ),
                ("s.css", "text/css", b".a { color: red }"),
            ],
            &["index.html"],
        );
        let options = RemoveUnusedCssOptions {
            remove_unused_classes: true,
            ..Default::default()
        };
        remove_unused_css(&mut container, options, |_| {}).unwrap();
        let dom = container.get_xhtml("index.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(
            dom.node(p).attrs.get("class").map(|s| s.as_str()),
            Some("a")
        );
    }

    #[test]
    fn merge_identical_selectors_combines_declarations_and_drops_the_duplicate() {
        let mut sheet = Stylesheet::parse(".a { color: red } .a { font-weight: bold }");
        let n = merge_identical_selectors(&mut sheet);
        assert_eq!(n, 1);
        assert_eq!(sheet.rules.len(), 1);
        let r = sheet.rules[0].as_style().unwrap();
        assert_eq!(r.style.get_property_value("color"), "red");
        assert_eq!(r.style.get_property_value("font-weight"), "bold");
    }

    #[test]
    fn merge_identical_properties_unions_selectors() {
        let mut sheet = Stylesheet::parse(".a { color: red } .b { color: red }");
        let n = merge_identical_properties(&mut sheet);
        assert_eq!(n, 2);
        assert_eq!(sheet.rules.len(), 1);
        let r = sheet.rules[0].as_style().unwrap();
        assert_eq!(r.selectors.0.len(), 2);
        assert!(r.selector_text.contains(".a"));
        assert!(r.selector_text.contains(".b"));
    }

    #[test]
    fn filter_css_removes_named_properties_and_drops_empty_rules() {
        let (_dir, mut container) = make_container(
            &[(
                "s.css",
                "text/css",
                b".a { color: red; font-weight: bold } .b { color: blue }",
            )],
            &[],
        );
        let mut props = HashSet::new();
        props.insert("color".to_string());
        let changed = filter_css(&mut container, &props, &[]).unwrap();
        assert!(changed);
        let sheet = container.parsed_stylesheet("s.css").unwrap();
        // `.b` had only `color`, which is now empty and dropped; `.a`
        // survives with just `font-weight`.
        assert_eq!(sheet.rules.len(), 1);
        let r = sheet.rules[0].as_style().unwrap();
        assert_eq!(r.selector_text, ".a");
        assert!(r.style.get_property("color").is_none());
        assert_eq!(r.style.get_property_value("font-weight"), "bold");
    }

    #[test]
    fn filter_css_removes_inline_style_attribute_when_it_becomes_empty() {
        let (_dir, mut container) = make_container(
            &[(
                "index.html",
                "application/xhtml+xml",
                b"<html><body><p style=\"color: red\">x</p></body></html>",
            )],
            &[],
        );
        let mut props = HashSet::new();
        props.insert("color".to_string());
        let changed = filter_css(&mut container, &props, &[]).unwrap();
        assert!(changed);
        let dom = container.get_xhtml("index.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        assert!(!dom.node(p).attrs.contains_key("style"));
    }

    #[test]
    fn classes_in_selector_collects_including_inside_not() {
        let classes = classes_in_selector("div.a b.c:not(.d)");
        assert!(classes.contains("a"));
        assert!(classes.contains("c"));
        assert!(classes.contains("d"));
    }

    #[test]
    fn classes_in_rule_list_recurses_into_media_rules() {
        let sheet = Stylesheet::parse("@media screen { .a { color: red } }");
        let classes = classes_in_rule_list(&sheet.rules);
        assert!(classes.contains("a"));
    }

    #[test]
    fn sort_sheet_orders_charset_import_then_style_rules_by_specificity() {
        let mut sheet = Stylesheet::parse(
            "p { color: red } @import url(a.css); #id { color: blue } @charset \"UTF-8\";",
        );
        sort_sheet(&mut sheet);
        let kinds: Vec<&str> = sheet
            .rules
            .iter()
            .map(|r| match r {
                Rule::Charset(_) => "charset",
                Rule::Import(_) => "import",
                Rule::Style(sr) => {
                    if sr.selector_text == "#id" {
                        "style-id"
                    } else {
                        "style-type"
                    }
                }
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["charset", "import", "style-type", "style-id"]);
    }

    #[test]
    fn rename_class_in_rule_list_renames_structurally_and_syncs_text() {
        let mut sheet = Stylesheet::parse(".old.other { color: red } .old > p { color: blue }");
        let changed = rename_class_in_rule_list(&mut sheet.rules, "old", "new");
        assert!(changed);
        let r0 = sheet.rules[0].as_style().unwrap();
        assert_eq!(r0.selector_text, ".new.other");
        assert_eq!(
            r0.selectors.0[0].compounds[0].simple.classes,
            vec!["new".to_string(), "other".to_string()]
        );
        let r1 = sheet.rules[1].as_style().unwrap();
        assert_eq!(r1.selector_text, ".new > p");
    }

    #[test]
    fn rename_class_updates_stylesheet_and_html_class_attribute_and_style_tag() {
        let (_dir, mut container) = make_container(
            &[
                ("s.css", "text/css", b".old { color: red }"),
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head><style>.old { font-weight: bold }</style></head>\
                      <body><p class=\"old extra\">hi</p></body></html>",
                ),
            ],
            &[],
        );
        let changed = rename_class(&mut container, "old", "new").unwrap();
        assert!(changed);
        let sheet = container.parsed_stylesheet("s.css").unwrap();
        assert_eq!(sheet.style_rules().next().unwrap().selector_text, ".new");

        let dom = container.get_xhtml("index.html").unwrap();
        let p = dom.find_first_tag_global("p").unwrap();
        assert_eq!(
            dom.node(p).attrs.get("class").map(|s| s.as_str()),
            Some("new extra")
        );
        let style_tag = dom.find_first_tag_global("style").unwrap();
        assert!(Stylesheet::parse(&dom.text_content(style_tag))
            .style_rules()
            .any(|r| r.selector_text == ".new"));
    }

    #[test]
    fn rename_class_is_a_no_op_for_empty_or_identical_names() {
        let (_dir, mut container) =
            make_container(&[("s.css", "text/css", b".a { color: red }")], &[]);
        assert!(!rename_class(&mut container, "", "x").unwrap());
        assert!(!rename_class(&mut container, "a", "a").unwrap());
    }

    #[test]
    fn add_stylesheet_links_adds_a_link_per_manifest_stylesheet() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head></head><body/></html>",
                ),
                ("s.css", "text/css", b".a{color:red}"),
            ],
            &[],
        );
        let text = b"<html><head></head><body>hi</body></html>";
        let out = add_stylesheet_links(&mut container, "index.html", text)
            .unwrap()
            .unwrap();
        assert!(out.contains("rel=\"stylesheet\""), "{out}");
        assert!(out.contains("href=\"s.css\""), "{out}");
    }

    #[test]
    fn add_stylesheet_links_returns_none_when_no_stylesheets_in_manifest() {
        let (_dir, mut container) = make_container(
            &[(
                "index.html",
                "application/xhtml+xml",
                b"<html><head></head><body/></html>",
            )],
            &[],
        );
        let text = b"<html><head></head><body>hi</body></html>";
        let out = add_stylesheet_links(&mut container, "index.html", text).unwrap();
        assert!(out.is_none());
    }

    #[test]
    fn get_imported_sheets_resolves_transitively_and_excludes_self() {
        let (_dir, container) = make_container(
            &[
                ("a.css", "text/css", b"@import url(b.css);"),
                ("b.css", "text/css", b"@import url(a.css); .b{color:red}"),
            ],
            &[],
        );
        let mut sheets = HashMap::new();
        sheets.insert(
            "a.css".to_string(),
            Stylesheet::parse("@import url(b.css);"),
        );
        sheets.insert(
            "b.css".to_string(),
            Stylesheet::parse("@import url(a.css); .b{color:red}"),
        );
        let imported = get_imported_sheets("a.css", &container, &sheets, None, 10);
        assert!(imported.contains("b.css"));
        assert!(!imported.contains("a.css"), "must not include itself");
    }

    #[test]
    fn remove_property_value_wrapper_matches_the_underlying_block_method() {
        let mut style = crate::css::parser::parse_declaration_list(
            "background-image: url(b.png); background: black url(a.png) fixed",
        );
        assert!(remove_property_value(&mut style, "background-image", |v| v.contains("png")));
        assert!(remove_property_value(&mut style, "background", |v| v.contains("png")));
        assert!(style.get_property("background-image").is_none());
        assert_eq!(style.get_property_value("background"), "black fixed");
    }
}
