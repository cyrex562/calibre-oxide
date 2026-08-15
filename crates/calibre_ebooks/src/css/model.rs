//! The CSS object model: [`Stylesheet`] / [`Rule`] /
//! [`StyleDeclarationBlock`] / [`Declaration`]. See the `css` module docs
//! for how this maps to Python's `css_parser.css`
//! (`CSSStyleSheet`/`CSSRule`/`CSSStyleDeclaration`/`Property`).

use super::selector::SelectorList;

/// Port of a single `Property` (`name: value` inside a declaration
/// block). `value` is the declaration's original source text, trimmed
/// and with a trailing `!important` (if any) stripped into
/// [`Declaration::important`] -- matching how
/// [`crate::oeb::polish::cascade::PropertyValue`] already represents
/// values in this crate (kept as source text, not a parsed value AST;
/// see the `css` module docs' "what is out of scope" section).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// Port of `CSSStyleDeclaration`: an ordered list of [`Declaration`]s,
/// either a style rule's body or a `style="..."` attribute's parsed
/// content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StyleDeclarationBlock {
    pub properties: Vec<Declaration>,
}

impl StyleDeclarationBlock {
    /// Port of `CSSStyleDeclaration.length`.
    pub fn len(&self) -> usize {
        self.properties.len()
    }

    pub fn is_empty(&self) -> bool {
        self.properties.is_empty()
    }

    /// Port of `getProperties()`.
    pub fn get_properties(&self) -> impl Iterator<Item = &Declaration> {
        self.properties.iter()
    }

    /// Port of `getProperty(name)`/`propertyIndex` lookup (last match
    /// wins, matching how a later declaration of the same property in
    /// CSS source overrides an earlier one within the same block).
    pub fn get_property(&self, name: &str) -> Option<&Declaration> {
        self.properties
            .iter()
            .rev()
            .find(|d| d.name.eq_ignore_ascii_case(name))
    }

    /// Port of `getPropertyValue(name)`. Returns `""` when absent,
    /// matching the DOM `CSSStyleDeclaration` convention Python's
    /// `css_parser` follows.
    pub fn get_property_value(&self, name: &str) -> &str {
        self.get_property(name)
            .map(|d| d.value.as_str())
            .unwrap_or("")
    }

    /// Port of `keys()`: the distinct property names present, in first-
    /// seen order.
    pub fn keys(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for d in &self.properties {
            let lower = d.name.to_ascii_lowercase();
            if seen.insert(lower) {
                out.push(d.name.clone());
            }
        }
        out
    }

    /// Port of `setProperty(name, value, priority)`: replaces the
    /// existing declaration for `name` (case-insensitively) in place if
    /// one exists, else appends a new one.
    pub fn set_property(&mut self, name: &str, value: impl Into<String>, important: bool) {
        let value = value.into();
        if let Some(existing) = self
            .properties
            .iter_mut()
            .find(|d| d.name.eq_ignore_ascii_case(name))
        {
            existing.value = value;
            existing.important = important;
        } else {
            self.properties.push(Declaration {
                name: name.to_string(),
                value,
                important,
            });
        }
    }

    /// Port of `removeProperty(name)`: removes every declaration for
    /// `name` (case-insensitively; `css_parser` also only ever has at
    /// most one per name after `setProperty`, but source CSS can
    /// legally repeat a property, so this removes all of them like
    /// Python's underlying `removeProperty` loop does) and returns the
    /// last value removed, or `""` if `name` was not present.
    pub fn remove_property(&mut self, name: &str) -> String {
        let mut removed = String::new();
        self.properties.retain(|d| {
            if d.name.eq_ignore_ascii_case(name) {
                removed = d.value.clone();
                false
            } else {
                true
            }
        });
        removed
    }

    /// Port of `Property.parent.removeProperty`/value-list editing used
    /// by `remove_property_value`: splits `name`'s value on top-level
    /// whitespace (the shape of a CSS shorthand's value list -- e.g.
    /// `background: black url(a.png) fixed`), drops every token for
    /// which `predicate` returns true, and either rewrites the
    /// declaration with the remaining tokens joined by a single space or,
    /// if every token was removed, removes the property entirely.
    /// Returns whether anything was removed.
    pub fn remove_property_value(&mut self, name: &str, predicate: impl Fn(&str) -> bool) -> bool {
        let Some(idx) = self
            .properties
            .iter()
            .rposition(|d| d.name.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        let tokens = split_top_level_whitespace(&self.properties[idx].value);
        let kept: Vec<&str> = tokens.iter().filter(|t| !predicate(t)).copied().collect();
        if kept.len() == tokens.len() {
            return false;
        }
        if kept.is_empty() {
            self.properties.remove(idx);
        } else {
            self.properties[idx].value = kept.join(" ");
        }
        true
    }

    /// Port of `getCssText(separator=...)`.
    pub fn to_css_text(&self, separator: &str) -> String {
        self.properties
            .iter()
            .map(|d| {
                if d.important {
                    format!("{}: {} !important;", d.name, d.value)
                } else {
                    format!("{}: {};", d.name, d.value)
                }
            })
            .collect::<Vec<_>>()
            .join(separator)
    }
}

/// Splits `text` on runs of whitespace that are not inside a quoted
/// string or a `(...)`/`[...]` block (so `url(a b.png)` and
/// `"quoted value"` each stay a single token). This is the same scoped
/// approximation `split_top_level_commas` (in `parser.rs`) uses for
/// selector lists, applied to whitespace instead of commas.
fn split_top_level_whitespace(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut start: Option<usize> = None;
    let mut last_end = 0usize;
    for (idx, ch) in text.char_indices() {
        last_end = idx + ch.len_utf8();
        match ch {
            '"' | '\'' => match in_str {
                Some(q) if q == ch => in_str = None,
                Some(_) => {}
                None => in_str = Some(ch),
            },
            '(' | '[' if in_str.is_none() => depth += 1,
            ')' | ']' if in_str.is_none() => depth = (depth - 1).max(0),
            c if c.is_whitespace() && in_str.is_none() && depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(&text[s..idx]);
                }
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(idx);
        }
    }
    if let Some(s) = start {
        out.push(&text[s..last_end]);
    }
    out
}

/// Port of a `@import` rule's prelude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRule {
    pub href: String,
    pub media_text: Option<String>,
}

/// Port of a `@media` rule: its media prelude text (kept unparsed -- see
/// the `css` module docs) plus the nested rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRule {
    pub media_text: String,
    pub rules: Vec<Rule>,
}

/// Port of a `@namespace` rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamespaceRule {
    pub prefix: Option<String>,
    pub uri: String,
}

/// Port of a style rule (`selector-list { declarations }`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleRule {
    /// The rule's full, original selector text (all selectors, comma
    /// separated), kept in sync with `selectors` by
    /// [`StyleRule::sync_selector_text`] after `selectors` is mutated.
    pub selector_text: String,
    pub selectors: SelectorList,
    pub style: StyleDeclarationBlock,
}

impl StyleRule {
    /// Recomputes `selector_text` from `selectors` (port of assigning
    /// `rule.selectorText = ', '.join(...)` after mutating
    /// `rule.selectorList` in place, e.g. in `sort_sheet`/
    /// `remove_unused_selectors_and_rules`).
    pub fn sync_selector_text(&mut self) {
        self.selector_text = self
            .selectors
            .0
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(", ");
    }
}

/// An at-rule this object model does not give first-class structure to
/// (`@supports`, `@page`, `@keyframes`, ...). `block` is the raw text
/// inside `{ }`, if the rule had a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownAtRule {
    pub at_keyword: String,
    pub prelude: String,
    pub block: Option<String>,
}

/// Port of the subset of `CSSRule` subtypes `css.py`/`cascade.py`/
/// `stats.py`/`fonts.py`/`subset.py` actually touch
/// (`STYLE_RULE`/`FONT_FACE_RULE`/`IMPORT_RULE`/`MEDIA_RULE`/
/// `CHARSET_RULE`/`NAMESPACE_RULE`), plus [`Rule::Unknown`] as a lossless
/// fallback for anything else (`@page`, `@supports`, `@keyframes`, ...)
/// so a stylesheet round-trips even when it contains rule types this
/// object model doesn't model structurally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rule {
    Style(StyleRule),
    FontFace(StyleDeclarationBlock),
    Import(ImportRule),
    Media(MediaRule),
    Charset(String),
    Namespace(NamespaceRule),
    Unknown(UnknownAtRule),
}

/// Port of the `CSSRule.*_RULE` type constants this crate needs, used by
/// `css.py`'s `sort_sheet`/`RULE_PRIORITIES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleType {
    Charset,
    Import,
    Namespace,
    Style,
    Media,
    FontFace,
    Unknown,
}

impl Rule {
    pub fn rule_type(&self) -> RuleType {
        match self {
            Rule::Style(_) => RuleType::Style,
            Rule::FontFace(_) => RuleType::FontFace,
            Rule::Import(_) => RuleType::Import,
            Rule::Media(_) => RuleType::Media,
            Rule::Charset(_) => RuleType::Charset,
            Rule::Namespace(_) => RuleType::Namespace,
            Rule::Unknown(_) => RuleType::Unknown,
        }
    }

    /// Port of `rulesOfType(CSSRule.STYLE_RULE)`'s per-rule test.
    pub fn as_style(&self) -> Option<&StyleRule> {
        match self {
            Rule::Style(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_style_mut(&mut self) -> Option<&mut StyleRule> {
        match self {
            Rule::Style(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_font_face(&self) -> Option<&StyleDeclarationBlock> {
        match self {
            Rule::FontFace(s) => Some(s),
            _ => None,
        }
    }

    /// Serializes this rule back to CSS text. See the `css` module docs:
    /// this is well-formed, stable output, not a byte-for-byte match for
    /// `cssutils`' formatting (the same convention `xmltree`/`pretty`
    /// already use).
    pub fn to_css_text(&self) -> String {
        match self {
            Rule::Style(r) => format!("{} {{\n{}\n}}", r.selector_text, indent_decls(&r.style)),
            Rule::FontFace(decls) => format!("@font-face {{\n{}\n}}", indent_decls(decls)),
            Rule::Import(i) => match &i.media_text {
                Some(m) if !m.is_empty() => format!("@import url({}) {};", i.href, m),
                _ => format!("@import url({});", i.href),
            },
            Rule::Media(m) => {
                let inner = m
                    .rules
                    .iter()
                    .map(Rule::to_css_text)
                    .collect::<Vec<_>>()
                    .join("\n\n");
                format!("@media {} {{\n{}\n}}", m.media_text, inner)
            }
            Rule::Charset(v) => format!("@charset \"{v}\";"),
            Rule::Namespace(n) => match &n.prefix {
                Some(p) => format!("@namespace {p} \"{}\";", n.uri),
                None => format!("@namespace \"{}\";", n.uri),
            },
            Rule::Unknown(u) => match &u.block {
                Some(b) => format!("@{} {} {{\n{}\n}}", u.at_keyword, u.prelude, b),
                None => format!("@{} {};", u.at_keyword, u.prelude),
            },
        }
    }
}

fn indent_decls(decls: &StyleDeclarationBlock) -> String {
    decls
        .properties
        .iter()
        .map(|d| {
            if d.important {
                format!("  {}: {} !important;", d.name, d.value)
            } else {
                format!("  {}: {};", d.name, d.value)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Port of `CSSStyleSheet`: an ordered list of [`Rule`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    pub fn parse(text: &str) -> Stylesheet {
        super::parser::parse_stylesheet(text)
    }

    /// Port of `cssRules.rulesOfType(CSSRule.STYLE_RULE)`.
    pub fn style_rules(&self) -> impl Iterator<Item = &StyleRule> {
        self.rules.iter().filter_map(Rule::as_style)
    }

    pub fn style_rules_mut(&mut self) -> impl Iterator<Item = &mut StyleRule> {
        self.rules.iter_mut().filter_map(Rule::as_style_mut)
    }

    /// Port of `cssRules.rulesOfType(CSSRule.FONT_FACE_RULE)`.
    pub fn font_face_rules(&self) -> impl Iterator<Item = &StyleDeclarationBlock> {
        self.rules.iter().filter_map(Rule::as_font_face)
    }

    /// Port of `cssRules.rulesOfType(CSSRule.IMPORT_RULE)`.
    pub fn import_rules(&self) -> impl Iterator<Item = &ImportRule> {
        self.rules.iter().filter_map(|r| match r {
            Rule::Import(i) => Some(i),
            _ => None,
        })
    }

    /// Port of `sheet.cssText`.
    pub fn to_css_text(&self) -> String {
        self.rules
            .iter()
            .map(Rule::to_css_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Every declaration block reachable from this stylesheet (style
    /// rules and `@font-face` rules, recursing into `@media`), port of
    /// `iter_declarations` for the `Stylesheet` case.
    pub fn iter_declarations(&self) -> Vec<&StyleDeclarationBlock> {
        let mut out = Vec::new();
        collect_declarations(&self.rules, &mut out);
        out
    }
}

fn collect_declarations<'a>(rules: &'a [Rule], out: &mut Vec<&'a StyleDeclarationBlock>) {
    for rule in rules {
        match rule {
            Rule::Style(s) => out.push(&s.style),
            Rule::FontFace(d) => out.push(d),
            Rule::Media(m) => collect_declarations(&m.rules, out),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_property_replaces_existing_in_place() {
        let mut block = StyleDeclarationBlock::default();
        block.set_property("color", "red", false);
        block.set_property("font-size", "12px", false);
        block.set_property("color", "blue", true);
        assert_eq!(block.len(), 2);
        assert_eq!(block.get_property_value("color"), "blue");
        assert!(block.get_property("color").unwrap().important);
    }

    #[test]
    fn remove_property_returns_last_value() {
        let mut block = StyleDeclarationBlock::default();
        block.set_property("color", "red", false);
        assert_eq!(block.remove_property("color"), "red");
        assert_eq!(block.remove_property("color"), "");
        assert!(block.is_empty());
    }

    #[test]
    fn remove_property_value_drops_matching_tokens_and_rejoins() {
        let mut block = StyleDeclarationBlock::default();
        block.set_property("background-image", "url(b.png)", false);
        block.set_property("background", "black url(a.png) fixed", false);
        let pred = |v: &str| v.contains("png");
        assert!(block.remove_property_value("background-image", pred));
        assert!(block.remove_property_value("background", pred));
        assert!(block.get_property("background-image").is_none());
        assert_eq!(block.get_property_value("background"), "black fixed");
    }

    #[test]
    fn keys_are_unique_and_first_seen_order() {
        let mut block = StyleDeclarationBlock::default();
        block.set_property("color", "red", false);
        block.set_property("Color", "blue", false);
        block.set_property("display", "block", false);
        assert_eq!(
            block.keys(),
            vec!["color".to_string(), "display".to_string()]
        );
    }
}
