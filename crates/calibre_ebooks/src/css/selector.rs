//! A deliberately **scoped** CSS selector grammar and matcher --
//! *not* an adoption of Servo's `selectors` crate. See the `css` module
//! docs for why: `selectors` requires implementing its
//! `selectors::Element` trait (pseudo-class queries, full
//! sibling/ancestor iteration protocols, tree-mutation hooks) across
//! [`crate::xmltree::Xml`]/[`crate::mobi::dom::Dom`], neither of which
//! has that surface today, making it a much larger and more intrusive
//! integration than `css.py`/`cascade.py`'s actual needs (`Select(root)
//! .has_matches(selector_text)`, specificity ordering).
//!
//! # Supported syntax
//!
//! - Type selectors (`div`), the universal selector (`*`).
//! - `.class` (a compound selector may carry several).
//! - `#id`.
//! - Attribute selectors: `[attr]` (existence) and `[attr=value]`
//!   /`[attr="value"]` (exact match only).
//! - Combinators: descendant (` `, whitespace) and child (`>`).
//! - `:not(simple-selector)`, where the argument is itself one compound
//!   selector (no combinators or further `:not()` nesting inside it --
//!   matching Selectors Level 3's `:not()`, not Level 4's
//!   `:not(<complex-selector-list>)`).
//! - Exactly the 13 pseudo-classes/elements Python's `css_selectors`
//!   calls `INAPPROPRIATE_PSEUDO_CLASSES` (`:active`/`:after`/
//!   `:disabled`/`:visited`/`:link`/`:before`/`:focus`/`:first-letter`/
//!   `:enabled`/`:first-line`/`:hover`/`:checked`/`:target`, `:` or `::`
//!   both accepted) are recognized but contribute nothing to matching
//!   (a compound carrying one still matches purely on its
//!   type/class/id/attribute parts) -- this is a direct, narrow port of
//!   `Select(root, ignore_inappropriate_pseudo_classes=True)`, which
//!   `cascade.py`'s `resolve_styles` always passes, *not* a general
//!   pseudo-class engine. [`Selector::pseudo_element`] records the first
//!   such name found in the selector text (matching `cascade.py`'s own
//!   `pseudo_pat.search(text)`), for `cascade.rs`'s
//!   `resolve_styles`/`resolve_pseudo_property` to route declarations
//!   into the pseudo-element style map instead of the plain one.
//! - Selector lists (`a, b, c`), comma-separated.
//!
//! # Explicitly *not* supported
//!
//! Anything not listed above is a [`SelectorError::Unsupported`], not a
//! silent partial match -- matching this crate's `xmltree` XPath
//! subset's "documented subset, not a general engine" convention. In
//! particular:
//!
//! - Sibling combinators `+`/`~`.
//! - Pseudo-classes/elements other than `:not()` and the 13-name
//!   whitelist above (`:first-child`/`:nth-child()`/`:lang()`/...).
//! - Namespace-prefixed type selectors (`svg|rect`) and the `|` column
//!   combinator.
//! - Attribute operators other than `=` (`~=`, `|=`, `^=`, `$=`, `*=`).
//! - `:nth-child()`/`An+B` microsyntax generally.

use cssparser::{Parser, ParserInput, Token};

/// Port of `css_selectors.SelectorSyntaxError`/`SelectorError`: a
/// selector this scoped grammar cannot parse. Matches Python's
/// `mark_used_selectors`, which treats an unparseable selector as "be
/// safe and assume it matches something" rather than failing the whole
/// operation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    #[error("CSS selector syntax error: {0}")]
    Syntax(String),
    #[error("unsupported CSS selector syntax (out of this crate's scoped subset): {0}")]
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttrSelector {
    Exists(String),
    Equals(String, String),
}

/// Port of `css_selectors.select.INAPPROPRIATE_PSEUDO_CLASSES`: the
/// exact set of pseudo-classes/elements `Select(...,
/// ignore_inappropriate_pseudo_classes=True)` tolerates (matches
/// unconditionally) rather than rejecting or actually implementing. See
/// the module docs.
pub const IGNORED_PSEUDO_CLASSES: &[&str] = &[
    "active",
    "after",
    "disabled",
    "visited",
    "link",
    "before",
    "focus",
    "first-letter",
    "enabled",
    "first-line",
    "hover",
    "checked",
    "target",
];

/// One compound selector's simple-selector components (`div.a.b#c[x]`),
/// port of what `css_selectors` calls a `Selector`'s innermost
/// `simple_selector`/`class_name`/etc chain of `parsed_tree` nodes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SimpleSelector {
    /// `None` for the universal selector `*` or when no type selector
    /// was written (matches any element). Lower-cased.
    pub type_name: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    /// `:not(...)` arguments; the element must match *none* of these.
    pub not: Vec<SimpleSelector>,
    /// A recognized-but-ignored pseudo-class/element from
    /// [`IGNORED_PSEUDO_CLASSES`], if this compound carried one. Never
    /// affects matching; see the module docs.
    pub pseudo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// ` ` -- the next compound must be a descendant.
    Descendant,
    /// `>` -- the next compound must be a direct child.
    Child,
}

/// One compound selector plus how it connects to the *next* (rightward)
/// compound in the same [`Selector`]. `combinator` is `None` only for
/// the last (rightmost) compound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompoundSelector {
    pub simple: SimpleSelector,
    pub combinator: Option<Combinator>,
}

/// Port of a `Selector` object (one item of a comma-separated selector
/// list) plus its specificity, computed once at parse time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub compounds: Vec<CompoundSelector>,
    /// The original source text for just this selector (trimmed),
    /// matching Python's `selector.selectorText`.
    pub text: String,
    /// `(num_id, num_class_and_attr, num_type)`, port of
    /// `selector.specificity`.
    pub specificity: (u32, u32, u32),
    /// The first [`IGNORED_PSEUDO_CLASSES`] name found anywhere in this
    /// selector, if any -- port of `cascade.py`'s
    /// `pseudo_pat.search(text).group(1)`, used to route a rule's
    /// declarations into `cascade.rs`'s pseudo-element style map.
    pub pseudo_element: Option<String>,
}

/// Port of `SelectorList` / `parse(text)`'s return value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectorList(pub Vec<Selector>);

impl SelectorList {
    /// Port of `select.has_matches`/iterating `Select(root)` against
    /// each selector in the list: true if *any* selector in this list
    /// matches `elem`.
    pub fn matches<E: super::matcher::Element>(&self, elem: E) -> bool {
        self.0
            .iter()
            .any(|s| super::matcher::selector_matches(s, elem))
    }

    /// Every distinct class name referenced anywhere in this list
    /// (including inside `:not()`), port of `_classes_in_selector`.
    pub fn classes(&self) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for sel in &self.0 {
            for c in &sel.compounds {
                collect_classes(&c.simple, &mut out);
            }
        }
        out
    }
}

fn collect_classes(s: &SimpleSelector, out: &mut std::collections::HashSet<String>) {
    out.extend(s.classes.iter().cloned());
    for n in &s.not {
        collect_classes(n, out);
    }
}

/// Port of `css_selectors.parse`/`Select(...).has_matches`'s
/// selector-text parsing: a comma-separated selector list.
///
/// Each comma-separated part is parsed independently, and a part this
/// scoped grammar can't handle is dropped rather than failing the whole
/// list -- matching how Python's `css_parser` already splits a rule's
/// `selectorText` into individual `Selector` objects at parse time, with
/// syntax errors only surfacing later, per selector, when
/// `select.has_matches(selector.selectorText)` is actually called (see
/// `mark_used_selectors` in `css.rs`, which treats that per-selector
/// failure as "be safe and assume it matches"). Returns
/// [`SelectorError::Syntax`] only if *every* part failed or the list was
/// empty.
pub fn parse_selector_list(text: &str) -> Result<SelectorList, SelectorError> {
    let mut selectors = Vec::new();
    let mut last_err = None;
    for part in split_top_level_commas(text) {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_one_selector(trimmed) {
            Ok(sel) => selectors.push(sel),
            // Keep the specific error (e.g. `Unsupported` vs. `Syntax`)
            // in case every part fails and it needs to be reported.
            Err(e) => last_err = Some(e),
        }
    }
    if selectors.is_empty() {
        return Err(last_err
            .unwrap_or_else(|| SelectorError::Syntax(format!("no usable selector in {text:?}"))));
    }
    Ok(SelectorList(selectors))
}

/// Splits `text` on top-level commas -- i.e. not inside `[...]`/`(...)`
/// or a quoted string. Real selector lists never legitimately have a
/// comma inside `:not(...)` under the Level 3 grammar this module
/// supports (its argument is one compound selector), so this only needs
/// to guard against a comma inside an attribute-selector's quoted value
/// (`[title="a, b"]`).
fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        match ch {
            '"' | '\'' => match in_str {
                Some(q) if q == ch => in_str = None,
                Some(_) => {}
                None => in_str = Some(ch),
            },
            '(' | '[' if in_str.is_none() => depth += 1,
            ')' | ']' if in_str.is_none() => depth = (depth - 1).max(0),
            ',' if in_str.is_none() && depth == 0 => {
                out.push(&text[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// A single scoped-selector token, produced by [`tokenize`] from one
/// (comma-free) selector's text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SelTok {
    Type(String),
    Universal,
    Class(String),
    Id(String),
    Attr(AttrSelector),
    Not(SimpleSelector),
    /// A recognized [`IGNORED_PSEUDO_CLASSES`] name (`:`/`::` already
    /// stripped, lower-cased).
    Pseudo(String),
    Ws,
    Gt,
}

fn tokenize(text: &str) -> Result<Vec<SelTok>, SelectorError> {
    let mut input = ParserInput::new(text);
    let mut parser = Parser::new(&mut input);
    let mut out = Vec::new();
    loop {
        let tok = match parser.next_including_whitespace() {
            Err(_) => break,
            Ok(t) => t.clone(),
        };
        match tok {
            Token::WhiteSpace(_) => out.push(SelTok::Ws),
            Token::Delim('>') => out.push(SelTok::Gt),
            Token::Delim('*') => out.push(SelTok::Universal),
            Token::Delim('.') => match parser.next_including_whitespace() {
                Ok(Token::Ident(name)) => out.push(SelTok::Class(name.to_string())),
                _ => {
                    return Err(SelectorError::Syntax(
                        "expected a class name after '.'".to_string(),
                    ))
                }
            },
            Token::IDHash(name) | Token::Hash(name) => out.push(SelTok::Id(name.to_string())),
            Token::Ident(name) => out.push(SelTok::Type(name.to_string())),
            Token::SquareBracketBlock => {
                let inner_text = capture_block_text(&mut parser);
                out.push(SelTok::Attr(parse_attr_inner(&inner_text)?));
            }
            Token::Colon => {
                // Accept both `:name` and `::name` -- Selectors Level 3
                // pseudo-elements use `::`, Level 2 allowed a single
                // `:` for the same four, and Python's `pseudo_pat`
                // matches `:{1,2}` uniformly. cssparser tokenizes `::`
                // as two adjacent `Colon` tokens, so peek for a second
                // one before the actual name.
                let mut next_tok = match parser.next_including_whitespace() {
                    Ok(t) => t.clone(),
                    Err(_) => {
                        return Err(SelectorError::Syntax("malformed pseudo-class".to_string()))
                    }
                };
                if matches!(next_tok, Token::Colon) {
                    next_tok = match parser.next_including_whitespace() {
                        Ok(t) => t.clone(),
                        Err(_) => {
                            return Err(SelectorError::Syntax("malformed pseudo-class".to_string()))
                        }
                    };
                }
                match next_tok {
                    Token::Function(name) if name.eq_ignore_ascii_case("not") => {
                        let inner_text = capture_block_text(&mut parser);
                        let simple = parse_compound_only(inner_text.trim())?;
                        out.push(SelTok::Not(simple));
                    }
                    Token::Ident(name) => {
                        let lname = name.to_ascii_lowercase();
                        if IGNORED_PSEUDO_CLASSES.contains(&lname.as_str()) {
                            out.push(SelTok::Pseudo(lname));
                        } else {
                            return Err(SelectorError::Unsupported(format!(":{name}")));
                        }
                    }
                    Token::Function(name) => {
                        return Err(SelectorError::Unsupported(format!(":{name}(...)")));
                    }
                    _ => return Err(SelectorError::Syntax("malformed pseudo-class".to_string())),
                }
            }
            Token::Comma => {
                return Err(SelectorError::Syntax(
                    "unexpected ',' (selector lists must be split before tokenizing)".to_string(),
                ))
            }
            other => {
                return Err(SelectorError::Unsupported(format!("{other:?}")));
            }
        }
    }
    Ok(out)
}

use super::parser::capture_block_text;

/// Parses an attribute selector's contents, i.e. what's already inside
/// the `[...]` (`attr` or `attr=value`/`attr="value"`).
fn parse_attr_inner(text: &str) -> Result<AttrSelector, SelectorError> {
    let mut parser_input = ParserInput::new(text);
    let mut input = Parser::new(&mut parser_input);
    input.skip_whitespace();
    let name = input
        .expect_ident_cloned()
        .map_err(|_| SelectorError::Syntax("expected an attribute name".to_string()))?
        .to_string();
    input.skip_whitespace();
    if input.is_exhausted() {
        return Ok(AttrSelector::Exists(name));
    }
    match input.next() {
        Ok(Token::Delim('=')) => {
            input.skip_whitespace();
            let value = match input.next() {
                Ok(Token::QuotedString(s)) => s.to_string(),
                Ok(Token::Ident(s)) => s.to_string(),
                _ => {
                    return Err(SelectorError::Syntax(
                        "expected an attribute value".to_string(),
                    ))
                }
            };
            Ok(AttrSelector::Equals(name, value))
        }
        _ => Err(SelectorError::Unsupported(
            "attribute operators other than '=' (~=, |=, ^=, $=, *=)".to_string(),
        )),
    }
}

/// Builds a single [`SimpleSelector`] (no combinators) from raw text,
/// used for `:not(...)`'s argument.
fn parse_compound_only(text: &str) -> Result<SimpleSelector, SelectorError> {
    let toks = tokenize(text)?;
    if toks.iter().any(|t| matches!(t, SelTok::Ws | SelTok::Gt)) {
        return Err(SelectorError::Unsupported(
            ":not() with a combinator (only a single compound selector is supported)".to_string(),
        ));
    }
    build_compound(&toks)
}

fn build_compound(run: &[SelTok]) -> Result<SimpleSelector, SelectorError> {
    if run.is_empty() {
        return Err(SelectorError::Syntax("empty compound selector".to_string()));
    }
    let mut s = SimpleSelector::default();
    for t in run {
        match t {
            SelTok::Universal => s.type_name = None,
            SelTok::Type(n) => s.type_name = Some(n.to_ascii_lowercase()),
            SelTok::Class(c) => s.classes.push(c.clone()),
            SelTok::Id(i) => s.id = Some(i.clone()),
            SelTok::Attr(a) => s.attrs.push(a.clone()),
            SelTok::Not(inner) => s.not.push(inner.clone()),
            SelTok::Pseudo(name) => s.pseudo = Some(name.clone()),
            SelTok::Ws | SelTok::Gt => {
                unreachable!("combinators are split out before build_compound")
            }
        }
    }
    Ok(s)
}

fn parse_one_selector(text: &str) -> Result<Selector, SelectorError> {
    let toks = tokenize(text)?;
    // Trim leading/trailing whitespace tokens.
    let mut g: &[SelTok] = &toks;
    while matches!(g.first(), Some(SelTok::Ws)) {
        g = &g[1..];
    }
    while matches!(g.last(), Some(SelTok::Ws)) {
        g = &g[..g.len() - 1];
    }
    if g.is_empty() {
        return Err(SelectorError::Syntax("empty selector".to_string()));
    }

    let mut compounds = Vec::new();
    let mut cur_run: Vec<SelTok> = Vec::new();
    let mut idx = 0;
    while idx < g.len() {
        match &g[idx] {
            SelTok::Ws => {
                let mut j = idx + 1;
                while matches!(g.get(j), Some(SelTok::Ws)) {
                    j += 1;
                }
                if matches!(g.get(j), Some(SelTok::Gt)) {
                    compounds.push(CompoundSelector {
                        simple: build_compound(&cur_run)?,
                        combinator: Some(Combinator::Child),
                    });
                    cur_run = Vec::new();
                    idx = j + 1;
                    while matches!(g.get(idx), Some(SelTok::Ws)) {
                        idx += 1;
                    }
                } else {
                    compounds.push(CompoundSelector {
                        simple: build_compound(&cur_run)?,
                        combinator: Some(Combinator::Descendant),
                    });
                    cur_run = Vec::new();
                    idx = j;
                }
            }
            SelTok::Gt => {
                compounds.push(CompoundSelector {
                    simple: build_compound(&cur_run)?,
                    combinator: Some(Combinator::Child),
                });
                cur_run = Vec::new();
                idx += 1;
                while matches!(g.get(idx), Some(SelTok::Ws)) {
                    idx += 1;
                }
            }
            other => {
                cur_run.push(other.clone());
                idx += 1;
            }
        }
    }
    compounds.push(CompoundSelector {
        simple: build_compound(&cur_run)?,
        combinator: None,
    });

    let specificity = compute_specificity(&compounds);
    let pseudo_element = compounds.iter().find_map(|c| c.simple.pseudo.clone());
    Ok(Selector {
        compounds,
        text: text.to_string(),
        specificity,
        pseudo_element,
    })
}

fn compute_specificity(compounds: &[CompoundSelector]) -> (u32, u32, u32) {
    let mut id = 0u32;
    let mut class = 0u32;
    let mut ty = 0u32;
    for c in compounds {
        add_simple_specificity(&c.simple, &mut id, &mut class, &mut ty);
    }
    (id, class, ty)
}

fn add_simple_specificity(s: &SimpleSelector, id: &mut u32, class: &mut u32, ty: &mut u32) {
    if s.id.is_some() {
        *id += 1;
    }
    *class += s.classes.len() as u32 + s.attrs.len() as u32;
    if s.type_name.is_some() {
        *ty += 1;
    }
    for n in &s.not {
        add_simple_specificity(n, id, class, ty);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_type_class_id_and_attr() {
        let list = parse_selector_list("div.a.b#c[x]").unwrap();
        assert_eq!(list.0.len(), 1);
        let s = &list.0[0].compounds[0].simple;
        assert_eq!(s.type_name.as_deref(), Some("div"));
        assert_eq!(s.id.as_deref(), Some("c"));
        assert_eq!(s.classes, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.attrs, vec![AttrSelector::Exists("x".to_string())]);
    }

    #[test]
    fn parses_attr_equals() {
        let list = parse_selector_list(r#"a[href="foo"]"#).unwrap();
        let s = &list.0[0].compounds[0].simple;
        assert_eq!(
            s.attrs,
            vec![AttrSelector::Equals("href".to_string(), "foo".to_string())]
        );
    }

    #[test]
    fn parses_descendant_and_child_combinators() {
        let list = parse_selector_list("div p > b").unwrap();
        let compounds = &list.0[0].compounds;
        assert_eq!(compounds.len(), 3);
        assert_eq!(compounds[0].combinator, Some(Combinator::Descendant));
        assert_eq!(compounds[1].combinator, Some(Combinator::Child));
        assert_eq!(compounds[2].combinator, None);
    }

    #[test]
    fn parses_selector_list_and_not() {
        let list = parse_selector_list("a, b:not(.x)").unwrap();
        assert_eq!(list.0.len(), 2);
        assert_eq!(list.0[0].text, "a");
        let not_sel = &list.0[1].compounds[0].simple;
        assert_eq!(not_sel.type_name.as_deref(), Some("b"));
        assert_eq!(not_sel.not.len(), 1);
        assert_eq!(not_sel.not[0].classes, vec!["x".to_string()]);
    }

    #[test]
    fn specificity_orders_id_over_class_over_type() {
        let id_sel = &parse_selector_list("#a").unwrap().0[0];
        let class_sel = &parse_selector_list(".a.b.c.d").unwrap().0[0];
        let type_sel = &parse_selector_list("div").unwrap().0[0];
        assert!(id_sel.specificity > class_sel.specificity);
        assert!(class_sel.specificity > type_sel.specificity);
    }

    #[test]
    fn unsupported_pseudo_class_is_reported() {
        // `:nth-child()` is not in the `INAPPROPRIATE_PSEUDO_CLASSES`
        // whitelist, unlike `:hover` (see the next test).
        let err = parse_selector_list("a:nth-child(2)").unwrap_err();
        assert!(matches!(err, SelectorError::Unsupported(_)));
    }

    #[test]
    fn unsupported_sibling_combinator_is_reported() {
        let err = parse_selector_list("a + b").unwrap_err();
        assert!(matches!(err, SelectorError::Unsupported(_)));
    }

    #[test]
    fn ignored_pseudo_classes_do_not_affect_matching() {
        let list = parse_selector_list("a:hover").unwrap();
        assert_eq!(
            list.0[0].compounds[0].simple.type_name.as_deref(),
            Some("a")
        );
        let dom = crate::mobi::dom::Dom::parse("<html><body><a href=\"x\">x</a></body></html>");
        let a = dom.find_first_tag_global("a").unwrap();
        let elem = super::super::matcher::DomElement { dom: &dom, id: a };
        assert!(list.matches(elem));
    }

    #[test]
    fn pseudo_element_is_recorded_for_before_and_first_line() {
        let list = parse_selector_list("p::before, .fl::first-line").unwrap();
        assert_eq!(list.0[0].pseudo_element.as_deref(), Some("before"));
        assert_eq!(list.0[1].pseudo_element.as_deref(), Some("first-line"));
        // A single leading colon (Level 2 style) is accepted too.
        let list2 = parse_selector_list("p:first-letter").unwrap();
        assert_eq!(list2.0[0].pseudo_element.as_deref(), Some("first-letter"));
    }

    #[test]
    fn classes_collects_from_top_level_and_not() {
        let list = parse_selector_list("div.a, span.b:not(.c)").unwrap();
        let classes = list.classes();
        assert!(classes.contains("a"));
        assert!(classes.contains("b"));
        assert!(classes.contains("c"));
    }
}
