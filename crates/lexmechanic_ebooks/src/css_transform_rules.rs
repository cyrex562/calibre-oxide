//! Port of `calibre.ebooks.css_transform_rules` (issue #117): a small,
//! user-authorable rules engine over CSS *property values* (as opposed
//! to [`crate::html_transform_rules`]'s tag-matching rules) -- match a
//! named property's value (exact/negated/regex/numeric-with-unit-
//! conversion), then remove/change/append/arithmetic-transform it.
//! Real upstream's own GUI-authored rule editor isn't part of this
//! port (no GUI exists); this covers the pure, serializable data model
//! + apply-loop, matching how [`crate::html_transform_rules`] (its
//! sibling, deferred alongside it from issue #17) was scoped.
//!
//! # Disclosed narrowings
//!
//! - **Shorthand-property expansion** (matching e.g. `margin-top`
//!   against a compact `margin: 0` declaration) only covers the same
//!   five shorthands [`crate::oeb::normalize_css::normalize_filter_css`]
//!   already does -- `margin`/`padding`/`border-style`/`border-width`/
//!   `border-color` -- not real upstream's full `normalizers` dict
//!   (which also expands `border`/`font`/`background`/`list-style`/
//!   `text-decoration`). Matching against one of those un-expanded
//!   shorthands' own longhand names silently finds nothing, exactly as
//!   it already does for [`crate::oeb::polish::css::filter_css`].
//! - **`validate_rule`** (real upstream's GUI-form pre-flight
//!   validator, producing a `(title, message)` error pair for the rule
//!   editor dialog) isn't ported as its own function -- the essential
//!   invariants it checks (unknown match/action type, invalid regex,
//!   invalid length/number, missing action data) are instead real
//!   [`anyhow::Error`]s surfaced directly from [`Rule::new`], which is
//!   the only real caller path in this port (no GUI to show a
//!   validation dialog to).
//! - **GUI display strings** (`ACTION_MAP`/`MATCH_TYPE_MAP`'s
//!   long-form text, shown only in the rule editor dialog) are
//!   dropped; [`rule_to_text`] keeps a short label per match/action
//!   kind for [`export_rules`]'s comment lines, matching
//!   [`crate::html_transform_rules`]'s own precedent.
//! - **`\N`-style regex backreferences** in a `change` action's
//!   replacement text (Python `regex.sub`'s `\1`/`\2` syntax) are
//!   translated to Rust `regex`'s `$1`/`$2` syntax before substitution
//!   -- covers the common case every real rule in this file's own test
//!   suite uses; `\g<name>`-style named backreferences are not
//!   translated.

use anyhow::{bail, Context, Result};
use regex::Regex;

use crate::css::model::{Declaration, Stylesheet};
use crate::oeb::normalize_css::normalize_edge;
use crate::oeb::polish::container::Container;
use crate::oeb::polish::css::transform_css as polish_transform_css;
pub use crate::css::model::StyleDeclarationBlock;

/// Shorthands this crate can expand, mirroring
/// [`crate::oeb::normalize_css::normalize_filter_css`]'s own scope.
const EXPANDABLE_SHORTHANDS: &[&str] = &["margin", "padding", "border-style", "border-width", "border-color"];

fn expand_shorthand(name: &str, value: &str, important: bool) -> Option<Vec<Declaration>> {
    if !EXPANDABLE_SHORTHANDS.contains(&name.to_ascii_lowercase().as_str()) {
        return None;
    }
    let mut expanded: Vec<Declaration> = normalize_edge(name, value)
        .into_iter()
        .map(|(name, value)| Declaration { name, value, important })
        .collect();
    // Port of `sorted(props, key=operator.attrgetter('name'))`.
    expanded.sort_by(|a, b| a.name.cmp(&b.name));
    Some(expanded)
}

/// Port of `UNIT_RE`/`parse_css_length`. Deliberately case-sensitive on
/// the unit suffix, matching real upstream's own un-flagged regex
/// (units must be lowercase in the source to be recognized).
fn parse_css_length(raw: &str) -> Option<(f64, String)> {
    let re = Regex::new(r"^(-*[0-9]*[.]?[0-9]*)\s*(%|em|ex|en|px|mm|cm|in|pt|pc|rem|q)$").unwrap();
    let caps = re.captures(raw.trim())?;
    let num = caps.get(1)?.as_str();
    if num.is_empty() {
        return None;
    }
    let value: f64 = num.parse().ok()?;
    Some((value, caps.get(2)?.as_str().to_string()))
}

/// Port of `parse_css_length_or_number`.
fn parse_css_length_or_number(raw: &str, default_unit: Option<&str>) -> Option<(f64, Option<String>)> {
    if let Ok(v) = raw.trim().parse::<f64>() {
        return Some((v, default_unit.map(str::to_string)));
    }
    parse_css_length(raw).map(|(v, u)| (v, Some(u)))
}

/// Port of `unit_convert`.
fn unit_convert(value: f64, unit: &str, dpi: f64, body_font_size: f64) -> Option<f64> {
    Some(match unit {
        "px" => value * 72.0 / dpi,
        "in" => value * 72.0,
        "pt" => value,
        "pc" => value * 12.0,
        "mm" => value * 2.8346456693,
        "cm" => value * 28.346456693,
        "rem" => value * body_font_size,
        "q" => value * 0.708661417325,
        _ => return None,
    })
}

/// Port of `numeric_match`.
fn numeric_match(value: f64, unit: Option<&str>, pts: Option<f64>, op: CmpOp, raw: &str) -> bool {
    let Some((v, u)) = parse_css_length_or_number(raw, None) else {
        return false;
    };
    if unit.is_none() || u.is_none() || unit == u.as_deref() {
        return op.apply(v, value);
    }
    let Some(pts) = pts else { return false };
    let Some(p) = unit_convert(v, u.as_deref().unwrap(), 96.0, 12.0) else {
        return false;
    };
    op.apply(p, pts)
}

/// Port of `transform_number`.
fn transform_number(val: f64, op: NumOp, raw: &str) -> String {
    let Some((v, u)) = parse_css_length_or_number(raw, Some("")) else {
        return raw.to_string();
    };
    let v2 = op.apply(v, val);
    let unit = u.unwrap_or_default();
    if v2.fract() == 0.0 {
        format!("{}{unit}", v2 as i64)
    } else {
        format!("{v2}{unit}")
    }
}

#[derive(Debug, Clone, Copy)]
enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl CmpOp {
    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "==" => CmpOp::Eq,
            "!=" => CmpOp::Ne,
            "<" => CmpOp::Lt,
            "<=" => CmpOp::Le,
            ">" => CmpOp::Gt,
            ">=" => CmpOp::Ge,
            other => bail!("Unknown match_type: {other}"),
        })
    }

    fn apply(self, a: f64, b: f64) -> bool {
        match self {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum NumOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl NumOp {
    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            NumOp::Add => a + b,
            NumOp::Sub => a - b,
            NumOp::Mul => a * b,
            NumOp::Div => a / b,
        }
    }
}

/// Translates Python `regex.sub`'s `\N` backreference syntax to Rust
/// `regex`'s `$N` -- see the module doc's disclosed narrowing.
fn translate_backreferences(replacement: &str) -> String {
    let re = Regex::new(r"\\(\d+)").unwrap();
    re.replace_all(replacement, "$${$1}").to_string()
}

#[derive(Debug)]
enum RuleAction {
    Remove,
    Change(String),
    Append(Vec<Declaration>),
    Arithmetic(NumOp, f64),
}

/// Port of `Rule`.
pub struct Rule {
    property_name: String,
    action: RuleAction,
    match_pat: Option<Regex>,
    property_matches: Box<dyn Fn(&str) -> bool>,
}

impl std::fmt::Debug for Rule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rule")
            .field("property_name", &self.property_name)
            .field("action", &self.action)
            .field("match_pat", &self.match_pat)
            .finish()
    }
}

/// Port of the serialized-rule dict real upstream's GUI produces
/// (`allowed_keys`).
#[derive(Debug, Clone, Default)]
pub struct SerializedRule {
    pub property: String,
    pub match_type: String,
    pub query: String,
    pub action: String,
    pub action_data: String,
}

impl Rule {
    /// Port of `Rule.__init__`.
    pub fn new(rule: &SerializedRule) -> Result<Self> {
        let property_name = rule.property.to_lowercase();

        let action = match rule.action.as_str() {
            "remove" => RuleAction::Remove,
            "change" => RuleAction::Change(rule.action_data.clone()),
            "append" => {
                let block = crate::css::parser::parse_declaration_list(&rule.action_data);
                RuleAction::Append(block.properties)
            }
            "+" => RuleAction::Arithmetic(NumOp::Add, rule.action_data.trim().parse().context("action_data must be a number")?),
            "-" => RuleAction::Arithmetic(NumOp::Sub, rule.action_data.trim().parse().context("action_data must be a number")?),
            "*" => RuleAction::Arithmetic(NumOp::Mul, rule.action_data.trim().parse().context("action_data must be a number")?),
            "/" => RuleAction::Arithmetic(NumOp::Div, rule.action_data.trim().parse().context("action_data must be a number")?),
            other => bail!("Unknown action type: {other}"),
        };

        let mut match_pat: Option<Regex> = None;
        let property_matches: Box<dyn Fn(&str) -> bool> = match rule.match_type.as_str() {
            "is" => {
                let q = rule.query.to_lowercase();
                Box::new(move |x: &str| x.to_lowercase() == q)
            }
            "is_not" => {
                let q = rule.query.to_lowercase();
                Box::new(move |x: &str| x.to_lowercase() != q)
            }
            "*" => Box::new(|_: &str| true),
            mt if mt.contains("matches") => {
                let pat = Regex::new(&format!("(?i){}", rule.query)).with_context(|| format!("{} is not a valid regular expression", rule.query))?;
                match_pat = Some(pat.clone());
                let negate = mt.starts_with("not_");
                Box::new(move |x: &str| {
                    let matched_at_start = pat.find(x).is_some_and(|m| m.start() == 0);
                    if negate {
                        !matched_at_start
                    } else {
                        matched_at_start
                    }
                })
            }
            mt => {
                let (value, unit) = parse_css_length_or_number(&rule.query, None).with_context(|| format!("{} is not a valid length or number", rule.query))?;
                let op = CmpOp::parse(mt)?;
                let pts = unit.as_deref().and_then(|u| unit_convert(value, u, 96.0, 12.0));
                Box::new(move |x: &str| numeric_match(value, unit.as_deref(), pts, op, x))
            }
        };

        Ok(Rule {
            property_name,
            action,
            match_pat,
            property_matches,
        })
    }

    /// Port of `StyleDeclaration.change_property`'s value computation:
    /// a regex substitution when `match_type` was `matches`/`not_matches`
    /// (in which case [`Rule::match_pat`] is `Some`), else a literal
    /// replacement.
    fn apply_change(&self, action_data: &str, current_value: &str) -> String {
        match &self.match_pat {
            Some(pat) => pat.replace(current_value, translate_backreferences(action_data)).to_string(),
            None => action_data.to_string(),
        }
    }

    fn apply_to_matched(&self, target: &Declaration, appends: &mut Vec<Declaration>) -> Option<String> {
        match &self.action {
            RuleAction::Remove => None,
            RuleAction::Change(action_data) => Some(self.apply_change(action_data, &target.value)),
            RuleAction::Append(props) => {
                appends.extend(props.iter().cloned());
                Some(target.value.clone())
            }
            RuleAction::Arithmetic(op, val) => Some(transform_number(*val, *op, &target.value)),
        }
    }

    /// Port of `StyleDeclaration.process_declaration`. Returns whether
    /// anything changed.
    pub fn process_declaration(&self, block: &mut crate::css::model::StyleDeclarationBlock) -> bool {
        let mut changed = false;
        let mut appends: Vec<Declaration> = Vec::new();
        let mut new_props: Vec<Declaration> = Vec::with_capacity(block.properties.len());

        for real in std::mem::take(&mut block.properties) {
            if let Some(mut expansion) = expand_shorthand(&real.name, &real.value, real.important) {
                if let Some(pos) = expansion.iter().position(|d| d.name.eq_ignore_ascii_case(&self.property_name)) {
                    if (self.property_matches)(&expansion[pos].value) {
                        changed = true;
                        match self.apply_to_matched(&expansion[pos], &mut appends) {
                            Some(new_value) => {
                                expansion[pos].value = new_value;
                                new_props.extend(expansion);
                            }
                            None => {
                                expansion.remove(pos);
                                new_props.extend(expansion);
                            }
                        }
                        continue;
                    }
                }
                new_props.push(real);
                continue;
            }

            if real.name.eq_ignore_ascii_case(&self.property_name) && (self.property_matches)(&real.value) {
                changed = true;
                match self.apply_to_matched(&real, &mut appends) {
                    Some(new_value) => {
                        let mut d = real;
                        d.value = new_value;
                        new_props.push(d);
                    }
                    None => {}
                }
            } else {
                new_props.push(real);
            }
        }

        block.properties = new_props;
        for p in appends {
            block.set_property(&p.name, p.value, p.important);
            changed = true;
        }
        changed
    }
}

/// Port of `compile_rules`.
pub fn compile_rules(serialized_rules: &[SerializedRule]) -> Result<Vec<Rule>> {
    serialized_rules.iter().map(Rule::new).collect()
}

/// Port of `transform_declaration`.
pub fn transform_declaration(rules: &[Rule], decl: &mut crate::css::model::StyleDeclarationBlock) -> bool {
    let mut changed = false;
    for rule in rules {
        if rule.process_declaration(decl) {
            changed = true;
        }
    }
    changed
}

/// Port of `transform_sheet`.
pub fn transform_sheet(rules: &[Rule], sheet: &mut Stylesheet) -> bool {
    let mut changed = false;
    for style_rule in sheet.style_rules_mut() {
        if transform_declaration(rules, &mut style_rule.style) {
            changed = true;
        }
    }
    changed
}

/// Port of `transform_container`: wired directly against the real,
/// already-ported [`crate::oeb::polish::css::transform_css`], which
/// already knows how to walk every real stylesheet and inline
/// `style="..."` attribute in the book.
pub fn transform_container(container: &mut Container, serialized_rules: &[SerializedRule], names: &[String]) -> Result<bool> {
    let rules = compile_rules(serialized_rules)?;
    polish_transform_css(container, |sheet| transform_sheet(&rules, sheet), |style| transform_declaration(&rules, style), names)
}

fn match_type_label(mt: &str) -> &str {
    match mt {
        "is" => "is",
        "is_not" => "is not",
        "*" => "is any value",
        "matches" => "matches pattern",
        "not_matches" => "does not match pattern",
        "==" => "is the same length as",
        "!=" => "is not the same length as",
        "<" => "is less than",
        ">" => "is greater than",
        "<=" => "is less than or equal to",
        ">=" => "is greater than or equal to",
        other => other,
    }
}

fn action_label(action: &str) -> &str {
    match action {
        "remove" => "Remove the property",
        "append" => "Add extra properties",
        "change" => "Change the value to",
        "*" => "Multiply the value by",
        "/" => "Divide the value by",
        "+" => "Add to the value",
        "-" => "Subtract from the value",
        other => other,
    }
}

/// Port of `rule_to_text`.
pub fn rule_to_text(rule: &SerializedRule) -> String {
    let mut text = format!("If the property {} {} {}\n{}", rule.property, match_type_label(&rule.match_type), rule.query, action_label(&rule.action));
    if !rule.action_data.is_empty() {
        text.push_str(&rule.action_data);
    }
    text
}

const ALLOWED_KEYS: &[&str] = &["property", "match_type", "query", "action", "action_data"];

/// Port of `export_rules`.
pub fn export_rules(rules: &[SerializedRule]) -> Vec<u8> {
    let mut lines: Vec<String> = Vec::new();
    for rule in rules {
        for l in rule_to_text(rule).lines() {
            lines.push(format!("# {l}"));
        }
        lines.push(format!("property: {}", rule.property.replace('\n', " ")));
        lines.push(format!("match_type: {}", rule.match_type.replace('\n', " ")));
        lines.push(format!("query: {}", rule.query.replace('\n', " ")));
        lines.push(format!("action: {}", rule.action.replace('\n', " ")));
        lines.push(format!("action_data: {}", rule.action_data.replace('\n', " ")));
        lines.push(String::new());
    }
    lines.join("\n").into_bytes()
}

/// Port of `import_rules`.
pub fn import_rules(raw_data: &[u8]) -> Result<Vec<SerializedRule>> {
    let text = String::from_utf8(raw_data.to_vec()).context("rule file is not valid UTF-8")?;
    let mut out = Vec::new();
    let mut rule = SerializedRule::default();
    let mut has_current = false;

    for line in text.lines() {
        if line.trim().is_empty() {
            if has_current {
                out.push(std::mem::take(&mut rule));
            }
            has_current = false;
            continue;
        }
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let k = k.trim().to_lowercase();
        let v = v.trim();
        if !ALLOWED_KEYS.contains(&k.as_str()) {
            continue;
        }
        has_current = true;
        match k.as_str() {
            "property" => rule.property = v.to_string(),
            "match_type" => rule.match_type = v.to_string(),
            "query" => rule.query = v.to_string(),
            "action" => rule.action = v.to_string(),
            "action_data" => rule.action_data = v.to_string(),
            _ => {}
        }
    }
    if has_current {
        out.push(rule);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(property: &str, match_type: &str, query: &str, action: &str, action_data: &str) -> SerializedRule {
        SerializedRule {
            property: property.to_string(),
            match_type: match_type.to_string(),
            query: query.to_string(),
            action: action.to_string(),
            action_data: action_data.to_string(),
        }
    }

    fn apply(style: &str, r: &SerializedRule) -> String {
        let mut block = crate::css::parser::parse_declaration_list(style);
        Rule::new(r).unwrap().process_declaration(&mut block);
        block.to_css_text("\n").trim_end_matches(';').trim().to_string()
    }

    #[test]
    fn matches_with_regex_substitution_converts_units() {
        let r = rule("font-size", "matches", "(.+)rem", "change", r"\1em");
        assert_eq!(apply("font-size: 1.2rem", &r), "font-size: 1.2em");
    }

    #[test]
    fn is_and_is_not_match_case_insensitively() {
        let r = rule("color", "is", "red", "remove", "");
        assert_eq!(apply("color: red; margin: 0", &r), "margin: 0");

        let r = rule("color", "is_not", "blue", "remove", "");
        assert_eq!(apply("color: red; margin: 0", &r), "margin: 0");

        let r = rule("color", "is", "blue", "remove", "");
        assert_eq!(apply("color: red; margin: 0", &r), "color: red;\nmargin: 0");
    }

    #[test]
    fn numeric_comparisons_with_unit_conversion() {
        for (mt, q) in [("==", "1"), ("==", "1mm"), ("==", "4q")] {
            let r = rule("margin-top", mt, q, "remove", "");
            assert_eq!(apply("color: red; margin-top: 1mm", &r), "color: red", "match_type={mt} query={q}");
        }
        let r = rule("margin-top", "==", "1pt", "remove", "");
        assert_eq!(apply("color: red; margin-top: 1mm", &r), "color: red;\nmargin-top: 1mm");
    }

    #[test]
    fn shorthand_expansion_on_remove() {
        let r = rule("margin-top", "*", "", "remove", "");
        assert_eq!(apply("margin: 0", &r), "margin-bottom: 0;\nmargin-left: 0;\nmargin-right: 0");
    }

    #[test]
    fn shorthand_expansion_on_change() {
        let r = rule("margin-top", "*", "", "change", "1pt");
        assert_eq!(apply("margin: 0", &r), "margin-bottom: 0;\nmargin-left: 0;\nmargin-right: 0;\nmargin-top: 1pt");
    }

    #[test]
    fn append_adds_new_properties_when_a_property_matches() {
        let r = rule("color", "*", "", "append", "margin: 1pt; font-weight: bold");
        assert_eq!(apply("color: red", &r), "color: red;\nmargin: 1pt;\nfont-weight: bold");
    }

    #[test]
    fn change_replaces_the_value_literally_for_non_regex_match_types() {
        let r = rule("font-family", "*", "", "change", "\"c c\", d");
        assert_eq!(apply("font-family: a, b", &r), "font-family: \"c c\", d");
    }

    #[test]
    fn arithmetic_actions_transform_numeric_values() {
        let r = rule("line-height", "*", "", "*", "3");
        assert_eq!(apply("line-height: 1", &r), "line-height: 3");
        let r = rule("line-height", "*", "", "+", "3");
        assert_eq!(apply("line-height: 1em", &r), "line-height: 4em");
        let r = rule("line-height", "*", "", "-", "1");
        assert_eq!(apply("line-height: 1", &r), "line-height: 0");
        let r = rule("line-height", "*", "", "/", "2");
        assert_eq!(apply("line-height: 2", &r), "line-height: 1");
    }

    #[test]
    fn arithmetic_action_expands_a_shorthand_too() {
        let r = rule("border-top-width", "*", "", "*", "3");
        assert_eq!(apply("border-width: 1", &r), "border-bottom-width: 1;\nborder-left-width: 1;\nborder-right-width: 1;\nborder-top-width: 3");
    }

    #[test]
    fn export_then_import_round_trips_a_rule() {
        let rules = vec![rule("a", "*", "some text", "remove", "")];
        let exported = export_rules(&rules);
        let text = String::from_utf8(exported.clone()).unwrap();
        assert!(text.contains("property: a"), "{text}");
        assert!(text.contains("match_type: *"), "{text}");
        assert!(text.contains("query: some text"), "{text}");
        assert!(text.contains("action: remove"), "{text}");

        let imported = import_rules(&exported).unwrap();
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].property, "a");
        assert_eq!(imported[0].query, "some text");
    }

    #[test]
    fn unknown_match_type_is_a_real_reported_error() {
        // Query must itself parse as a valid number, matching real
        // upstream's own construction order (`parse_css_length_or_number`
        // is called before `operator_map[match_type]` is looked up) --
        // this isolates the "unknown match_type" error specifically.
        let err = Rule::new(&rule("color", "totally_bogus", "5", "remove", "")).unwrap_err();
        assert!(err.to_string().contains("Unknown match_type"), "{err}");
    }

    #[test]
    fn transform_declaration_applies_every_rule() {
        // `margin` itself (the shorthand's own name) is deliberately not
        // a matchable property name here -- a normalizable shorthand's
        // own compact declaration is never yielded to a rule at all in
        // real upstream (`StyleDeclaration.__iter__` yields ONLY the
        // expanded longhand names for a normalizable property); only
        // `margin-top`/`margin-right`/etc are ever visible to a rule.
        let rules = compile_rules(&[rule("color", "*", "", "remove", ""), rule("font-weight", "*", "", "remove", "")]).unwrap();
        let mut block = crate::css::parser::parse_declaration_list("color: red; font-weight: bold; padding: 1px");
        assert!(transform_declaration(&rules, &mut block));
        assert_eq!(block.to_css_text("\n").trim_end_matches(';').trim(), "padding: 1px");
    }

    #[test]
    fn a_shorthands_own_name_is_never_directly_matchable() {
        let rules = compile_rules(&[rule("margin", "*", "", "remove", "")]).unwrap();
        let mut block = crate::css::parser::parse_declaration_list("margin: 0");
        assert!(!transform_declaration(&rules, &mut block), "a shorthand's own compact name should never match, matching real upstream");
        assert_eq!(block.to_css_text("\n").trim_end_matches(';').trim(), "margin: 0");
    }
}
