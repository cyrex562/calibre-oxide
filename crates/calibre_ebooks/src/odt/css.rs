//! Narrow CSS post-processing for the stylesheet the ODT converter itself
//! generates.
//!
//! `input.py`'s `Extract.extract_css`/`filter_css`/`do_filter_css` use the
//! real `css_parser` package to manipulate arbitrary CSS. As noted in
//! issues #34/#35, this workspace has no general CSS parser, and building
//! one is out of scope here. But `do_filter_css` only ever runs against
//! CSS *this converter itself wrote* (see
//! [`crate::odt::styles::StyleResolver`] / [`crate::odt::convert`]), which
//! is always a flat sequence of
//! `selector[, selector]* {\n\tprop: val;\n...\n}\n` blocks -- never
//! `@media`/`@import`/nested rules/comments. That's a small enough grammar
//! to parse and round-trip correctly with straightforward string handling,
//! so unlike the general case, it's implemented for real here rather than
//! left as a placeholder.

use indexmap::IndexMap;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct CssRule {
    pub selectors: Vec<String>,
    pub decls: IndexMap<String, String>,
}

/// Parses `css` as a sequence of `selector[, selector]* { decls }` blocks.
/// Anything that doesn't fit that shape (stray text outside a block) is
/// silently skipped -- this is a targeted parser for our own generator's
/// output, not a general CSS parser.
pub fn parse_rules(css: &str) -> Vec<CssRule> {
    let mut rules = Vec::new();
    let mut rest = css;
    while let Some(open) = rest.find('{') {
        let selector_part = &rest[..open];
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let body = &after_open[..close];
        let selectors: Vec<String> = selector_part
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !selectors.is_empty() {
            let mut decls = IndexMap::new();
            for decl in body.split(';') {
                let decl = decl.trim();
                if decl.is_empty() {
                    continue;
                }
                if let Some((k, v)) = decl.split_once(':') {
                    decls.insert(k.trim().to_string(), v.trim().to_string());
                }
            }
            rules.push(CssRule { selectors, decls });
        }
        rest = &after_open[close + 1..];
    }
    rules
}

/// Renders rules back to text in the same shape [`parse_rules`] accepts
/// (and that [`crate::odt::styles::StyleResolver`]'s stylesheet emitter
/// produces), so this round-trips.
pub fn serialize_rules(rules: &[CssRule]) -> String {
    let mut out = String::new();
    for rule in rules {
        out.push_str(&rule.selectors.join(", "));
        out.push_str(" {\n");
        for (k, v) in &rule.decls {
            out.push('\t');
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str(";\n");
        }
        out.push_str("}\n");
    }
    out
}

/// Port of `Extract.get_css_for_class`: the first rule whose selector list
/// contains `.cls`.
pub fn get_css_for_class<'a>(rules: &'a [CssRule], cls: &str) -> Option<&'a CssRule> {
    if cls.is_empty() {
        return None;
    }
    let needle = format!(".{cls}");
    rules.iter().find(|r| r.selectors.contains(&needle))
}

/// Port of `Extract.do_filter_css`: any rule with two or more selectors
/// that are *all* plain class selectors gets those selectors replaced
/// with a single synthetic class (`c_odt0`, `c_odt1`, ...); the returned
/// map records, for each original class name (no leading `.`), which
/// synthetic class names it should additionally carry on any element that
/// has it -- the caller (`fixup::filter_css`) applies that to the HTML.
pub fn do_filter_css(css: &str) -> (String, HashMap<String, Vec<String>>) {
    let mut rules = parse_rules(css);
    let mut sel_map: HashMap<String, Vec<String>> = HashMap::new();
    let mut count = 0usize;
    for rule in rules.iter_mut() {
        let all_class_selectors = rule.selectors.iter().all(|s| s.starts_with('.'));
        if rule.selectors.len() > 1 && all_class_selectors {
            let replace_name = format!("c_odt{count}");
            count += 1;
            for sel in &rule.selectors {
                let cls = sel.trim_start_matches('.').to_string();
                sel_map.entry(cls).or_default().push(replace_name.clone());
            }
            rule.selectors = vec![format!(".{replace_name}")];
        }
    }
    (serialize_rules(&rules), sel_map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_serializes_round_trip() {
        let css = ".P-A, .P-B {\n\tcolor: red;\n\tfont-weight: bold;\n}\n.P-C {\n\ttext-align: left;\n}\n";
        let rules = parse_rules(css);
        assert_eq!(rules.len(), 2);
        assert_eq!(
            rules[0].selectors,
            vec![".P-A".to_string(), ".P-B".to_string()]
        );
        assert_eq!(rules[0].decls.get("color"), Some(&"red".to_string()));
        assert_eq!(rules[1].selectors, vec![".P-C".to_string()]);

        let out = serialize_rules(&rules);
        let reparsed = parse_rules(&out);
        assert_eq!(reparsed, rules);
    }

    #[test]
    fn consolidates_multi_class_selectors() {
        let css = ".P-A, .P-B {\n\tcolor: red;\n}\n.P-C {\n\ttext-align: left;\n}\n";
        let (out, sel_map) = do_filter_css(css);
        let rules = parse_rules(&out);
        // The two-selector, all-class rule collapses to one synthetic class.
        assert_eq!(rules[0].selectors, vec![".c_odt0".to_string()]);
        // The single-selector rule is untouched.
        assert_eq!(rules[1].selectors, vec![".P-C".to_string()]);
        assert_eq!(sel_map.get("P-A"), Some(&vec!["c_odt0".to_string()]));
        assert_eq!(sel_map.get("P-B"), Some(&vec!["c_odt0".to_string()]));
        assert!(!sel_map.contains_key("P-C"));
    }

    #[test]
    fn leaves_mixed_selectors_alone() {
        // Not all-class (has an element selector) -> not consolidated.
        let css = "p, .P-B {\n\tcolor: red;\n}\n";
        let (out, sel_map) = do_filter_css(css);
        let rules = parse_rules(&out);
        assert_eq!(
            rules[0].selectors,
            vec!["p".to_string(), ".P-B".to_string()]
        );
        assert!(sel_map.is_empty());
    }

    #[test]
    fn get_css_for_class_finds_matching_rule() {
        let css = ".P-A, .P-B {\n\tcolor: red;\n}\n";
        let rules = parse_rules(css);
        let rule = get_css_for_class(&rules, "P-B").expect("rule found");
        assert_eq!(rule.decls.get("color"), Some(&"red".to_string()));
        assert!(get_css_for_class(&rules, "P-Z").is_none());
    }
}
