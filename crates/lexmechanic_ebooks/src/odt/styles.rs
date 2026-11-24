//! A scoped-down port of the style-resolution half of
//! `old_src/src/odf/odf2xhtml.py` (`StyleToCSS`, plus the
//! `s_style_style`/`s_style_default_style`/`s_style_handle_properties`/
//! `s_style_font_face`/`generate_stylesheet` family of handlers, and the
//! `text:list-style` / `text:list-level-style-*` handling that feeds
//! `Extract.apply_list_starts`).
//!
//! Unlike the original, which resolves styles incrementally while
//! streaming SAX events (so it can only see styles it has already parsed),
//! this reads the whole `content.xml`/`styles.xml` tree with `roxmltree`
//! first and resolves styles in a second pass, which sidesteps needing the
//! original's ordering assumptions.
//!
//! Scope: paragraph/text/table/table-cell/table-column/table-row/graphic
//! family styles, resolved through `style:parent-style-name` chains and
//! `style:default-style` family defaults, converted to a modest set of
//! CSS2 properties (bold/italic/underline/strikethrough, color,
//! background-color, font-family/size, text-align, margins/padding,
//! width, border-collapse). Left out, matching the "legitimately out of
//! scope" carve-out for this issue: `style:text-position` (sub/superscript
//! sizing), `style:horizontal-pos`/wrap-based floating, page/print layout
//! (`@page`, headers/footers, `fo:break-*`), and fill-image backgrounds --
//! these are real gaps in `StyleToCSS.ruleconversions` we don't reproduce,
//! documented here rather than silently mis-converted.

use crate::odt::namespaces::{class_name_for, sanitize_style_name, FONS, STYLENS, SVGNS, TEXTNS};
use indexmap::IndexMap;
use roxmltree::{Document, Node};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
struct StyleDef {
    family: String,
    parent: Option<String>,
    raw_props: IndexMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ListLevelDef {
    pub ordered: bool,
    pub list_style_type: String,
    /// The declared start value (`text:start-value`), only set when it
    /// differs from the default of `1` -- matches
    /// `ODF2XHTML.list_starts` only ever recording non-default starts.
    pub start_value: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StyleResolver {
    /// class name (e.g. "P-Heading_20_1") -> definition, in document order.
    defs: IndexMap<String, StyleDef>,
    /// ODF `style:family` -> raw default properties (`style:default-style`).
    family_defaults: HashMap<String, IndexMap<String, String>>,
    /// `style:font-face` name -> (font-family value, CSS generic fallback).
    fonts: HashMap<String, (String, String)>,
    /// `text:list-style` name -> per-level definitions, keyed by level.
    list_styles: HashMap<String, HashMap<u32, ListLevelDef>>,
}

impl StyleResolver {
    /// Builds a resolver from `content.xml`'s document (styles declared
    /// inline are unprefixed, matching `autoprefix = ''`) and, if present,
    /// `styles.xml`'s document (styles inside its `office:automatic-styles`
    /// get an `A` prefix on their class name, matching `autoprefix = 'A'`
    /// in `s_office_automatic_styles`; styles inside `office:styles` --
    /// i.e. common/named styles -- stay unprefixed since they're
    /// referenced by name from content.xml just like automatic ones).
    pub fn build(content_doc: &Document, styles_doc: Option<&Document>) -> StyleResolver {
        let mut resolver = StyleResolver::default();
        resolver.collect_from(content_doc, false);
        if let Some(doc) = styles_doc {
            resolver.collect_from(doc, true);
        }
        resolver
    }

    fn collect_from(&mut self, doc: &Document, is_styles_xml: bool) {
        for node in doc.descendants() {
            if !node.is_element() {
                continue;
            }
            let ns = node.tag_name().namespace();
            let name = node.tag_name().name();
            if ns == Some(STYLENS) && name == "font-face" {
                self.collect_font_face(node);
            } else if ns == Some(STYLENS) && name == "default-style" {
                self.collect_default_style(node);
            } else if ns == Some(STYLENS) && name == "style" {
                let autoprefix = if is_styles_xml && in_automatic_styles(node) {
                    "A"
                } else {
                    ""
                };
                self.collect_style(node, autoprefix);
            } else if ns == Some(TEXTNS) && name == "list-style" {
                self.collect_list_style(node);
            }
        }
    }

    fn collect_font_face(&mut self, node: Node) {
        let Some(name) = node.attribute((STYLENS, "name")) else {
            return;
        };
        let family = node
            .attribute((SVGNS, "font-family"))
            .unwrap_or(name)
            .trim_matches('\'')
            .to_string();
        let generic = node
            .attribute((STYLENS, "font-family-generic"))
            .unwrap_or("");
        let css_generic = match generic {
            "roman" => "serif",
            "swiss" => "sans-serif",
            "modern" => "monospace",
            "decorative" => "sans-serif",
            "script" => "monospace",
            "system" => "serif",
            _ => "sans-serif",
        };
        self.fonts
            .insert(name.to_string(), (family, css_generic.to_string()));
    }

    fn collect_default_style(&mut self, node: Node) {
        let Some(family) = node.attribute((STYLENS, "family")) else {
            return;
        };
        let props = collect_properties(node);
        self.family_defaults.insert(family.to_string(), props);
    }

    fn collect_style(&mut self, node: Node, autoprefix: &str) {
        let (Some(name), Some(family)) = (
            node.attribute((STYLENS, "name")),
            node.attribute((STYLENS, "family")),
        ) else {
            return;
        };
        let class_name = format!("{autoprefix}{}", class_name_for(family, name));
        let parent = node
            .attribute((STYLENS, "parent-style-name"))
            .map(|p| format!("{autoprefix}{}", class_name_for(family, p)));
        let raw_props = collect_properties(node);
        let entry = self.defs.entry(class_name).or_default();
        entry.family = family.to_string();
        if parent.is_some() {
            entry.parent = parent;
        }
        for (k, v) in raw_props {
            entry.raw_props.insert(k, v);
        }
    }

    fn collect_list_style(&mut self, node: Node) {
        let Some(name) = node.attribute((STYLENS, "name")) else {
            return;
        };
        let name = sanitize_style_name(name);
        let levels = self.list_styles.entry(name).or_default();
        for child in node.children().filter(|c| c.is_element()) {
            let ns = child.tag_name().namespace();
            let local = child.tag_name().name();
            let Some(level_str) = child.attribute((TEXTNS, "level")) else {
                continue;
            };
            let Ok(level) = level_str.parse::<u32>() else {
                continue;
            };
            if ns == Some(TEXTNS) && local == "list-level-style-bullet" {
                let list_style_type = ["square", "disc", "circle"][(level % 3) as usize];
                levels.insert(
                    level,
                    ListLevelDef {
                        ordered: false,
                        list_style_type: list_style_type.to_string(),
                        start_value: None,
                    },
                );
            } else if ns == Some(TEXTNS) && local == "list-level-style-number" {
                let num_format = child.attribute((STYLENS, "num-format")).unwrap_or("1");
                let list_style_type = match num_format {
                    "1" => "decimal",
                    "I" => "upper-roman",
                    "i" => "lower-roman",
                    "A" => "upper-alpha",
                    "a" => "lower-alpha",
                    _ => "decimal",
                };
                let start_value = child
                    .attribute((TEXTNS, "start-value"))
                    .filter(|v| *v != "1")
                    .map(|v| v.to_string());
                levels.insert(
                    level,
                    ListLevelDef {
                        ordered: true,
                        list_style_type: list_style_type.to_string(),
                        start_value,
                    },
                );
            }
        }
    }

    /// The declared level definition for `list_style_name` at `level`
    /// (1-based), if any -- used to decide `<ol>` vs `<ul>` and to record
    /// `Extract.list_starts` entries.
    pub fn list_level(&self, list_style_name: &str, level: u32) -> Option<&ListLevelDef> {
        self.list_styles
            .get(&sanitize_style_name(list_style_name))
            .and_then(|levels| levels.get(&level))
    }

    /// Resolves `class_name`'s full cascade (family default \< ancestor
    /// `parent-style-name` chain \< the style's own local properties) into
    /// raw ODF property key/value pairs (key = `"prefix:local"`, e.g.
    /// `"fo:font-weight"`).
    fn resolve_raw(&self, class_name: &str) -> IndexMap<String, String> {
        let mut seen = HashSet::new();
        self.resolve_raw_inner(class_name, &mut seen)
    }

    fn resolve_raw_inner(
        &self,
        class_name: &str,
        seen: &mut HashSet<String>,
    ) -> IndexMap<String, String> {
        let mut result = IndexMap::new();
        let Some(def) = self.defs.get(class_name) else {
            return result;
        };
        if let Some(fam_default) = self.family_defaults.get(&def.family) {
            for (k, v) in fam_default {
                result.insert(k.clone(), v.clone());
            }
        }
        if let Some(parent_name) = &def.parent {
            if seen.insert(parent_name.clone()) {
                for (k, v) in self.resolve_raw_inner(parent_name, seen) {
                    result.insert(k, v);
                }
            }
        }
        for (k, v) in &def.raw_props {
            result.insert(k.clone(), v.clone());
        }
        result
    }

    /// Resolves and CSS2-converts `class_name`'s properties (`StyleToCSS`).
    /// Empty if the class was never declared.
    pub fn resolve_css(&self, class_name: &str) -> IndexMap<String, String> {
        let raw = self.resolve_raw(class_name);
        self.convert_props(&raw)
    }

    fn convert_props(&self, raw: &IndexMap<String, String>) -> IndexMap<String, String> {
        let mut out = IndexMap::new();
        for (key, val) in raw {
            match key.as_str() {
                "fo:background-color" => {
                    out.insert("background-color".to_string(), val.clone());
                }
                "fo:border" | "fo:border-bottom" | "fo:border-left" | "fo:border-right"
                | "fo:border-top" => {
                    out.insert(key.trim_start_matches("fo:").to_string(), val.clone());
                }
                "fo:color" => {
                    out.insert("color".to_string(), val.clone());
                }
                "fo:font-family" => {
                    out.insert("font-family".to_string(), val.clone());
                }
                "fo:font-size" => {
                    out.insert("font-size".to_string(), val.clone());
                }
                "fo:font-style" => {
                    out.insert("font-style".to_string(), val.clone());
                }
                "fo:font-variant" => {
                    out.insert("font-variant".to_string(), val.clone());
                }
                "fo:font-weight" => {
                    out.insert("font-weight".to_string(), val.clone());
                }
                "fo:line-height" => {
                    out.insert("line-height".to_string(), val.clone());
                }
                "fo:margin" | "fo:margin-bottom" | "fo:margin-left" | "fo:margin-right"
                | "fo:margin-top" | "fo:min-height" => {
                    out.insert(key.trim_start_matches("fo:").to_string(), val.clone());
                }
                "fo:padding" | "fo:padding-bottom" | "fo:padding-left" | "fo:padding-right"
                | "fo:padding-top" => {
                    out.insert(key.trim_start_matches("fo:").to_string(), val.clone());
                }
                "fo:text-align" => {
                    let mapped = match val.as_str() {
                        "start" => "left",
                        "end" => "right",
                        other => other,
                    };
                    out.insert("text-align".to_string(), mapped.to_string());
                }
                "fo:text-indent" => {
                    out.insert("text-indent".to_string(), val.clone());
                }
                "table:border-model" => {
                    let collapse = if val == "collapsing" {
                        "collapse"
                    } else {
                        "separate"
                    };
                    out.insert("border-collapse".to_string(), collapse.to_string());
                }
                "style:font-name" => {
                    let (family, generic) = self
                        .fonts
                        .get(val.as_str())
                        .cloned()
                        .unwrap_or_else(|| (val.clone(), "serif".to_string()));
                    out.insert("font-family".to_string(), format!("{family}, {generic}"));
                }
                "style:text-underline-style" => {
                    if val != "none" {
                        out.insert("text-decoration".to_string(), "underline".to_string());
                    }
                }
                "style:text-line-through-style" => {
                    if val != "none" {
                        out.insert("text-decoration".to_string(), "line-through".to_string());
                    }
                }
                "style:width" | "style:column-width" => {
                    out.insert("width".to_string(), val.clone());
                }
                _ => { /* out of scope, see module docs */ }
            }
        }
        out
    }

    /// Every declared class name, in document order (used to emit the
    /// `<style>` block deterministically).
    pub fn class_names(&self) -> impl Iterator<Item = &String> {
        self.defs.keys()
    }

    /// `(html class token "name_level", list-style-type value)` for every
    /// declared list level across every `text:list-style`, sorted for
    /// deterministic output.
    pub fn list_class_rules(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (name, levels) in &self.list_styles {
            for (level, def) in levels {
                out.push((format!("{name}_{level}"), def.list_style_type.clone()));
            }
        }
        out.sort();
        out
    }
}

fn in_automatic_styles(node: Node) -> bool {
    node.ancestors().any(|a| {
        a.is_element()
            && a.tag_name().namespace() == Some(crate::odt::namespaces::OFFICENS)
            && a.tag_name().name() == "automatic-styles"
    })
}

/// Collects every attribute from `node`'s `*-properties` children (`style:
/// paragraph-properties`, `style:text-properties`, `style:table-properties`,
/// `style:table-column-properties`, `style:table-cell-properties`,
/// `style:graphic-properties`, `style:drawing-page-properties`) into a flat
/// `"prefix:local" -> value` map, matching `s_style_handle_properties`
/// copying `attrs.items()` into `self.styledict[self.currentstyle]`.
fn collect_properties(style_node: Node) -> IndexMap<String, String> {
    const PROPERTY_TAGS: &[&str] = &[
        "paragraph-properties",
        "text-properties",
        "table-properties",
        "table-column-properties",
        "table-cell-properties",
        "graphic-properties",
        "drawing-page-properties",
    ];
    let mut props = IndexMap::new();
    for child in style_node.children().filter(|c| c.is_element()) {
        if child.tag_name().namespace() != Some(STYLENS)
            || !PROPERTY_TAGS.contains(&child.tag_name().name())
        {
            continue;
        }
        for attr in child.attributes() {
            let Some(prefix) = ns_prefix(attr.namespace()) else {
                continue;
            };
            props.insert(
                format!("{prefix}:{}", attr.name()),
                attr.value().to_string(),
            );
        }
    }
    props
}

/// Maps the handful of namespace URIs `StyleToCSS.ruleconversions` cares
/// about to their conventional short prefix, matching `odf.namespaces.nsdict`.
fn ns_prefix(uri: Option<&str>) -> Option<&'static str> {
    match uri {
        Some(FONS) => Some("fo"),
        Some(STYLENS) => Some("style"),
        Some("urn:oasis:names:tc:opendocument:xmlns:table:1.0") => Some("table"),
        Some(SVGNS) => Some("svg"),
        Some(TEXTNS) => Some("text"),
        _ => None,
    }
}
