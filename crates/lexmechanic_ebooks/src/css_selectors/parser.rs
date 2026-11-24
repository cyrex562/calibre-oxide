//! Port of `css_selectors.parser` (issue #452, split from #85, depends
//! on #451): the CSS Selectors Level 3 tokenizer, recursive-descent
//! parser, and parsed AST node types real `select.py` (#453) matches
//! against.
//!
//! # A hand-rolled tokenizer, not `cssparser` -- a real, load-bearing
//! divergence discovered while porting
//!
//! [`crate::css::selector`] (the narrower #164 engine) tokenizes with
//! `cssparser`, and reusing it here was the first approach tried. It
//! doesn't work for this module: `cssparser` implements the full CSS
//! Syntax Level 3 tokenizer, which treats a digit sequence immediately
//! followed by identifier characters as ONE `Dimension` token (e.g.
//! CSS `10px`) -- but real upstream's own hand-rolled regex tokenizer
//! has no concept of dimensions at all and always tokenizes `2n` as
//! TWO separate tokens, `NUMBER "2"` then `IDENT "n"`. This is exactly
//! the shape structural pseudo-classes like `:nth-child(2n+1)` need
//! ([`parse_series`], which reconstructs the flat token sequence back
//! into the string `"2n+1"` before parsing the `An+B` microsyntax) --
//! silently reusing `cssparser` here would have merged `2n` into one
//! opaque `Dimension` token and broken every structural pseudo-class
//! argument, the main reason this fuller grammar exists in the first
//! place. Ported as a direct, hand-rolled scanner over the real regex
//! patterns instead (`TokenMacros`'s `nmstart`/`nmchar`/`escape`
//! character classes, `_match_ident`/`_match_hash`/
//! `_match_string_by_quote`/`_match_number`), operating on `Vec<char>`
//! positions (not bytes) to match Python's own string-index semantics.
//!
//! # The parsed tree: one recursive `Node` enum, not a class hierarchy
//!
//! Real upstream represents a compound selector's chain of modifiers
//! (`div.a#b[href]:not(.c)`) as nested objects, each wrapping the
//! previous one (`Class(Hash(Attrib(Negation(Element(...)))), ...)`,
//! built up left-to-right during parsing) and each implementing its own
//! `specificity()`. Ported as one [`Node`] enum with a `Box<Node>` in
//! place of each `self.selector`, dispatched by `match` instead of
//! virtual methods -- the same "flat enum instead of a class hierarchy"
//! choice made throughout this port (e.g. #563's `Operand`, #565's
//! `SubsetError`).

use crate::css_selectors::errors::SelectorError;

// ---------------------------------------------------------------------
// Parsed AST
// ---------------------------------------------------------------------

/// Port of `Element`: `namespace|element`, or the universal selector
/// `*` when `element` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementSel {
    pub namespace: Option<String>,
    pub element: Option<String>,
}

/// Port of `Attrib`: `selector[namespace|attrib operator value]`.
/// `operator` is one of `"exists"`, `"="`, `"~="`, `"|="`, `"^="`,
/// `"$="`, `"*="`, `"!="` -- kept as a plain string (matching real
/// upstream's own field, which is never validated against a fixed enum
/// there either) rather than a closed Rust enum, since #453's real
/// matching logic just switches on this string directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttribSel {
    pub namespace: Option<String>,
    pub attrib: String,
    pub operator: String,
    /// `None` only when `operator == "exists"`.
    pub value: Option<String>,
}

/// Port of `Function`: `selector:name(arguments)` (any function other
/// than `:not()`, e.g. `:nth-child(2n+1)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSel {
    /// Lower-cased, matching real `ascii_lower(name)`.
    pub name: String,
    pub arguments: Vec<Token>,
}

impl FunctionSel {
    /// Port of `Function.argument_types`.
    pub fn argument_types(&self) -> Vec<TokenKind> {
        self.arguments.iter().map(|t| t.kind.clone()).collect()
    }

    /// Port of `Function.parsed_arguments` (the real property lazily
    /// calls [`parse_series`], wrapping a `ValueError` into
    /// `ExpressionError`).
    pub fn parsed_arguments(&self) -> Result<(i64, i64), SelectorError> {
        parse_series(&self.arguments).map_err(|e| SelectorError::Expression(format!("Invalid series: {e}")))
    }
}

/// Port of `FunctionalPseudoElement`: `selector::name(arguments)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionalPseudoElement {
    /// Lower-cased, matching real `ascii_lower(name)`.
    pub name: String,
    pub arguments: Vec<Token>,
}

impl FunctionalPseudoElement {
    pub fn argument_types(&self) -> Vec<TokenKind> {
        self.arguments.iter().map(|t| t.kind.clone()).collect()
    }
}

/// Port of `Selector.pseudo_element`'s two real shapes: a plain
/// identifier (`::before`, or the CSS2.1 single-colon form) or a
/// functional pseudo-element (`::slotted(...)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PseudoElement {
    Ident(String),
    Functional(FunctionalPseudoElement),
}

/// Port of the real parsed-tree class hierarchy
/// (`Element`/`Class`/`Hash`/`Attrib`/`Pseudo`/`Function`/`Negation`/
/// `CombinedSelector`) as one recursive enum -- see the module doc for
/// why. Each non-`Element` variant's boxed `Node` is the port of that
/// real class's own `self.selector` (or, for `Negation`/`Combined`,
/// also a `self.subselector`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(ElementSel),
    /// Port of `Class`: `selector.class_name`. Not lower-cased (real
    /// `Class.__init__` stores it raw).
    Class(Box<Node>, String),
    /// Port of `Hash`: `selector#id`. Not lower-cased.
    IdHash(Box<Node>, String),
    Attrib(Box<Node>, AttribSel),
    /// Port of `Pseudo`: `selector:ident`. Lower-cased.
    Pseudo(Box<Node>, String),
    Function(Box<Node>, FunctionSel),
    /// Port of `Negation`: `selector:not(subselector)`.
    Negation(Box<Node>, Box<Node>),
    /// Port of `CombinedSelector`: `selector combinator subselector`.
    /// `combinator` is one of `' '` (descendant), `'>'` (child), `'+'`
    /// (next sibling), `'~'` (later sibling).
    Combined(Box<Node>, char, Box<Node>),
}

impl Node {
    /// Port of every real class's own `specificity()`, unified by
    /// `match` instead of virtual dispatch.
    pub fn specificity(&self) -> (u32, u32, u32) {
        match self {
            Node::Element(e) => {
                if e.element.is_some() {
                    (0, 0, 1)
                } else {
                    (0, 0, 0)
                }
            }
            Node::Class(sel, _) | Node::Attrib(sel, _) | Node::Pseudo(sel, _) | Node::Function(sel, _) => {
                let (a, b, c) = sel.specificity();
                (a, b + 1, c)
            }
            Node::IdHash(sel, _) => {
                let (a, b, c) = sel.specificity();
                (a + 1, b, c)
            }
            Node::Negation(sel, sub) | Node::Combined(sel, _, sub) => {
                let (a1, b1, c1) = sel.specificity();
                let (a2, b2, c2) = sub.specificity();
                (a1 + a2, b1 + b2, c1 + c2)
            }
        }
    }
}

/// Port of `Selector`: one parsed selector from a comma-separated
/// group, plus its (possibly `None`) pseudo-element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    pub parsed_tree: Node,
    pub pseudo_element: Option<PseudoElement>,
}

impl Selector {
    /// Port of `Selector.__init__`'s own normalization: a plain-ident
    /// pseudo-element is lower-cased *here*, regardless of which of the
    /// two real parsing branches produced it (the CSS2.1 single-colon
    /// special form, or an un-parenthesized `::name`) -- a
    /// `FunctionalPseudoElement` is excluded (real
    /// `isinstance(pseudo_element, FunctionalPseudoElement)` guard;
    /// its own `name` field is already lower-cased by its own
    /// constructor). Caught by cross-validating against real upstream
    /// directly: an initial reading of `parse_simple_selector`'s single-
    /// colon special-pseudo-element branch (`pseudo_element =
    /// unicode_type(ident)`, no `ascii_lower`) looked like it
    /// preserved case, but real `Selector.__init__` lower-cases it one
    /// level up regardless -- `:Before` and `::Before` both really
    /// produce `"before"`, confirmed against the live Python function.
    fn new(parsed_tree: Node, pseudo_element: Option<PseudoElement>) -> Self {
        let pseudo_element = match pseudo_element {
            Some(PseudoElement::Ident(s)) => Some(PseudoElement::Ident(s.to_ascii_lowercase())),
            other => other,
        };
        Selector { parsed_tree, pseudo_element }
    }

    /// Port of `Selector.specificity`.
    pub fn specificity(&self) -> (u32, u32, u32) {
        let (a, b, c) = self.parsed_tree.specificity();
        if self.pseudo_element.is_some() {
            (a, b, c + 1)
        } else {
            (a, b, c)
        }
    }
}

// ---------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------

/// Port of `Token`'s real `type` values (`IDENT`/`HASH`/`STRING`/
/// `NUMBER`/`DELIM`/`S`/`EOF`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Ident,
    Hash,
    String,
    Number,
    Delim(char),
    Whitespace,
    Eof,
}

/// Port of `Token`/`EOFToken`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub value: String,
    pub pos: usize,
}

impl Token {
    /// Port of `Token.is_delim`.
    pub fn is_delim(&self, values: &[char]) -> bool {
        matches!(self.kind, TokenKind::Delim(c) if values.contains(&c))
    }

    fn is_delim_char(&self, c: char) -> bool {
        matches!(self.kind, TokenKind::Delim(ch) if ch == c)
    }
}

// ---------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------

fn is_nonascii(c: char) -> bool {
    (c as u32) > 0x7F
}

fn is_nmstart_char(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic() || is_nonascii(c)
}

fn is_nmchar_char(c: char) -> bool {
    c == '_' || c == '-' || c.is_ascii_alphanumeric() || is_nonascii(c)
}

/// Scans one real `escape` (`unicode_escape | \[^\n\r\f0-9a-f]`)
/// starting at `chars[pos] == '\\'`. Returns the unescaped text and the
/// position just past it, or `None` if there's nothing valid to escape
/// (end of input right after the backslash).
fn scan_escape(chars: &[char], pos: usize) -> Option<(String, usize)> {
    let mut p = pos + 1;
    if p >= chars.len() {
        return None;
    }
    if chars[p].is_ascii_hexdigit() {
        let hex_start = p;
        let mut count = 0;
        while p < chars.len() && count < 6 && chars[p].is_ascii_hexdigit() {
            p += 1;
            count += 1;
        }
        let hex: String = chars[hex_start..p].iter().collect();
        let codepoint = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
        let ch = char::from_u32(codepoint).unwrap_or('\u{FFFD}');
        // Optional single whitespace escape terminator (`\r\n` counts
        // as one terminator, matching the real `(?:\r\n|[ \n\r\t\f])?`).
        if p < chars.len() {
            if chars[p] == '\r' && chars.get(p + 1) == Some(&'\n') {
                p += 2;
            } else if matches!(chars[p], ' ' | '\n' | '\r' | '\t' | '\x0c') {
                p += 1;
            }
        }
        Some((ch.to_string(), p))
    } else {
        Some((chars[p].to_string(), p + 1))
    }
}

/// Shared `nmstart nmchar*` scanner, with an optional leading `-`, used
/// by both the ident matcher (`-?(nmstart)(nmchar)*`) and inline for
/// each subsequent name character.
fn scan_name(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut pos = start;
    let mut out = String::new();
    if pos < chars.len() && chars[pos] == '-' {
        out.push('-');
        pos += 1;
    }
    if pos < chars.len() && chars[pos] == '\\' {
        let (s, np) = scan_escape(chars, pos)?;
        out.push_str(&s);
        pos = np;
    } else if pos < chars.len() && is_nmstart_char(chars[pos]) {
        out.push(chars[pos]);
        pos += 1;
    } else {
        return None;
    }
    loop {
        if pos < chars.len() && chars[pos] == '\\' {
            if let Some((s, np)) = scan_escape(chars, pos) {
                out.push_str(&s);
                pos = np;
                continue;
            }
            break;
        }
        if pos < chars.len() && is_nmchar_char(chars[pos]) {
            out.push(chars[pos]);
            pos += 1;
            continue;
        }
        break;
    }
    Some((out, pos))
}

/// Port of `_match_hash`: `#(?:nmchar)+` -- unlike an ident, the first
/// character may be any `nmchar` (including a digit), no `nmstart`
/// restriction.
fn scan_hash(chars: &[char], pos: usize) -> Option<(String, usize)> {
    let mut p = pos + 1;
    let mut out = String::new();
    loop {
        if p < chars.len() && chars[p] == '\\' {
            if let Some((s, np)) = scan_escape(chars, p) {
                out.push_str(&s);
                p = np;
                continue;
            }
            break;
        }
        if p < chars.len() && is_nmchar_char(chars[p]) {
            out.push(chars[p]);
            p += 1;
            continue;
        }
        break;
    }
    if out.is_empty() {
        None
    } else {
        Some((out, p))
    }
}

/// Port of `_match_number`: `[+-]?(?:[0-9]*\.[0-9]+|[0-9]+)`.
fn scan_number(chars: &[char], pos: usize) -> Option<(String, usize)> {
    let start = pos;
    let mut p = pos;
    if p < chars.len() && (chars[p] == '+' || chars[p] == '-') {
        p += 1;
    }
    let int_start = p;
    while p < chars.len() && chars[p].is_ascii_digit() {
        p += 1;
    }
    let int_len = p - int_start;
    if p < chars.len() && chars[p] == '.' {
        let frac_start = p + 1;
        let mut fp = frac_start;
        while fp < chars.len() && chars[fp].is_ascii_digit() {
            fp += 1;
        }
        if fp > frac_start {
            return Some((chars[start..fp].iter().collect(), fp));
        }
    }
    if int_len > 0 {
        Some((chars[start..p].iter().collect(), p))
    } else {
        None
    }
}

/// Port of `_match_string_by_quote[quote]` plus the surrounding
/// `tokenize`/unescape logic for one quoted string, starting at
/// `chars[pos] == quote`. Returns the unescaped content and the
/// position just past the closing quote.
fn scan_string(chars: &[char], pos: usize, quote: char) -> Result<(String, usize), SelectorError> {
    let mut p = pos + 1;
    let mut out = String::new();
    loop {
        if p >= chars.len() {
            return Err(SelectorError::Syntax(format!("Unclosed string at {pos}")));
        }
        let c = chars[p];
        if c == quote {
            return Ok((out, p + 1));
        }
        if c == '\n' || c == '\r' || c == '\x0c' {
            return Err(SelectorError::Syntax(format!("Invalid string at {pos}")));
        }
        if c == '\\' {
            if chars.get(p + 1) == Some(&'\n') {
                p += 2;
                continue;
            }
            if chars.get(p + 1) == Some(&'\r') && chars.get(p + 2) == Some(&'\n') {
                p += 3;
                continue;
            }
            if chars.get(p + 1) == Some(&'\r') || chars.get(p + 1) == Some(&'\x0c') {
                p += 2;
                continue;
            }
            if let Some((s, np)) = scan_escape(chars, p) {
                out.push_str(&s);
                p = np;
                continue;
            }
            return Err(SelectorError::Syntax(format!("Invalid string at {pos}")));
        }
        out.push(c);
        p += 1;
    }
}

/// Port of `tokenize`.
pub fn tokenize(s: &str) -> Result<Vec<Token>, SelectorError> {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut pos = 0usize;
    let mut out = Vec::new();
    while pos < len {
        if matches!(chars[pos], ' ' | '\t' | '\r' | '\n' | '\x0c') {
            let start = pos;
            while pos < len && matches!(chars[pos], ' ' | '\t' | '\r' | '\n' | '\x0c') {
                pos += 1;
            }
            out.push(Token { kind: TokenKind::Whitespace, value: " ".to_string(), pos: start });
            continue;
        }
        if let Some((value, new_pos)) = scan_name(&chars, pos) {
            out.push(Token { kind: TokenKind::Ident, value, pos });
            pos = new_pos;
            continue;
        }
        if chars[pos] == '#' {
            if let Some((value, new_pos)) = scan_hash(&chars, pos) {
                out.push(Token { kind: TokenKind::Hash, value, pos });
                pos = new_pos;
                continue;
            }
        }
        if chars[pos] == '\'' || chars[pos] == '"' {
            let quote = chars[pos];
            let (value, new_pos) = scan_string(&chars, pos, quote)?;
            out.push(Token { kind: TokenKind::String, value, pos });
            pos = new_pos;
            continue;
        }
        if let Some((value, new_pos)) = scan_number(&chars, pos) {
            out.push(Token { kind: TokenKind::Number, value, pos });
            pos = new_pos;
            continue;
        }
        if chars[pos] == '/' && chars.get(pos + 1) == Some(&'*') {
            let mut p = pos + 2;
            let mut found = false;
            while p + 1 < len {
                if chars[p] == '*' && chars[p + 1] == '/' {
                    p += 2;
                    found = true;
                    break;
                }
                p += 1;
            }
            pos = if found { p } else { len };
            continue;
        }
        out.push(Token { kind: TokenKind::Delim(chars[pos]), value: chars[pos].to_string(), pos });
        pos += 1;
    }
    out.push(Token { kind: TokenKind::Eof, value: String::new(), pos });
    Ok(out)
}

// ---------------------------------------------------------------------
// TokenStream
// ---------------------------------------------------------------------

/// Port of `TokenStream`. Real `self.used` is a list only ever
/// consulted via `len(stream.used)` (to detect "did
/// `parse_simple_selector` consume anything at all") -- ported as a
/// plain counter instead of storing every consumed token.
struct TokenStream {
    tokens: std::vec::IntoIter<Token>,
    peeked: Option<Token>,
    used_count: usize,
}

impl TokenStream {
    fn new(tokens: Vec<Token>) -> Self {
        TokenStream { tokens: tokens.into_iter(), peeked: None, used_count: 0 }
    }

    fn next(&mut self) -> Token {
        let t = match self.peeked.take() {
            Some(t) => t,
            None => self.tokens.next().expect("tokenize() always yields a trailing EOF token"),
        };
        self.used_count += 1;
        t
    }

    fn peek(&mut self) -> &Token {
        if self.peeked.is_none() {
            self.peeked = Some(self.tokens.next().expect("tokenize() always yields a trailing EOF token"));
        }
        self.peeked.as_ref().unwrap()
    }

    fn skip_whitespace(&mut self) {
        if matches!(self.peek().kind, TokenKind::Whitespace) {
            self.next();
        }
    }

    fn next_ident(&mut self) -> Result<String, SelectorError> {
        let t = self.next();
        if matches!(t.kind, TokenKind::Ident) {
            Ok(t.value)
        } else {
            Err(SelectorError::Syntax(format!("Expected ident, got {t:?}")))
        }
    }

    fn next_ident_or_star(&mut self) -> Result<Option<String>, SelectorError> {
        let t = self.next();
        match t.kind {
            TokenKind::Ident => Ok(Some(t.value)),
            TokenKind::Delim('*') => Ok(None),
            _ => Err(SelectorError::Syntax(format!("Expected ident or '*', got {t:?}"))),
        }
    }
}

// ---------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------

/// Port of `special_pseudo_elements`: CSS2.1 pseudo-elements allowed a
/// single `:`; any newer pseudo-element needs `::`.
const SPECIAL_PSEUDO_ELEMENTS: &[&str] = &["first-line", "first-letter", "before", "after"];

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c')
}

/// Port of `_el_re`'s fast path: `^[ \t\r\n\f]*([a-zA-Z]+)[ \t\r\n\f]*$`.
fn fast_path_element(css: &str) -> Option<Selector> {
    let t = css.trim_matches(is_ws);
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_alphabetic()) {
        Some(Selector { parsed_tree: Node::Element(ElementSel { namespace: None, element: Some(t.to_string()) }), pseudo_element: None })
    } else {
        None
    }
}

/// Port of `_id_re`'s fast path: `^[ \t\r\n\f]*([a-zA-Z]*)#([a-zA-Z0-9_-]+)[ \t\r\n\f]*$`.
fn fast_path_id(css: &str) -> Option<Selector> {
    let t = css.trim_matches(is_ws);
    let hash_pos = t.find('#')?;
    let elem_part = &t[..hash_pos];
    let id_part = &t[hash_pos + 1..];
    if !elem_part.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    if id_part.is_empty() || !id_part.chars().all(|c| c == '_' || c == '-' || c.is_ascii_alphanumeric()) {
        return None;
    }
    let element = if elem_part.is_empty() { None } else { Some(elem_part.to_string()) };
    Some(Selector {
        parsed_tree: Node::IdHash(Box::new(Node::Element(ElementSel { namespace: None, element })), id_part.to_string()),
        pseudo_element: None,
    })
}

/// Port of `_class_re`'s fast path: `^[ \t\r\n\f]*([a-zA-Z]*)\.([a-zA-Z][a-zA-Z0-9_-]*)[ \t\r\n\f]*$`.
fn fast_path_class(css: &str) -> Option<Selector> {
    let t = css.trim_matches(is_ws);
    let dot_pos = t.find('.')?;
    let elem_part = &t[..dot_pos];
    let class_part = &t[dot_pos + 1..];
    if !elem_part.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let mut chars = class_part.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if !chars.clone().all(|c| c == '_' || c == '-' || c.is_ascii_alphanumeric()) {
        return None;
    }
    let element = if elem_part.is_empty() { None } else { Some(elem_part.to_string()) };
    Some(Selector {
        parsed_tree: Node::Class(Box::new(Node::Element(ElementSel { namespace: None, element })), class_part.to_string()),
        pseudo_element: None,
    })
}

/// Port of `parse`: parses a CSS *group of selectors* (comma-separated).
pub fn parse(css: &str) -> Result<Vec<Selector>, SelectorError> {
    if let Some(s) = fast_path_element(css) {
        return Ok(vec![s]);
    }
    if let Some(s) = fast_path_id(css) {
        return Ok(vec![s]);
    }
    if let Some(s) = fast_path_class(css) {
        return Ok(vec![s]);
    }
    let tokens = tokenize(css)?;
    let mut stream = TokenStream::new(tokens);
    parse_selector_group(&mut stream)
}

/// Port of `parse_selector_group`.
fn parse_selector_group(stream: &mut TokenStream) -> Result<Vec<Selector>, SelectorError> {
    stream.skip_whitespace();
    let mut out = Vec::new();
    loop {
        let (tree, pseudo_element) = parse_selector(stream)?;
        out.push(Selector::new(tree, pseudo_element));
        if stream.peek().is_delim_char(',') {
            stream.next();
            stream.skip_whitespace();
        } else {
            break;
        }
    }
    Ok(out)
}

/// Port of `parse_selector`.
fn parse_selector(stream: &mut TokenStream) -> Result<(Node, Option<PseudoElement>), SelectorError> {
    let (mut result, mut pseudo_element) = parse_simple_selector(stream, false)?;
    loop {
        stream.skip_whitespace();
        let peek = stream.peek().clone();
        if matches!(peek.kind, TokenKind::Eof) || peek.is_delim_char(',') {
            break;
        }
        if pseudo_element.is_some() {
            return Err(SelectorError::Syntax("Got a pseudo-element not at the end of a selector".to_string()));
        }
        let combinator = if peek.is_delim(&['+', '>', '~']) {
            let c = match stream.next().kind {
                TokenKind::Delim(c) => c,
                _ => unreachable!(),
            };
            stream.skip_whitespace();
            c
        } else {
            ' '
        };
        let (next_selector, pe) = parse_simple_selector(stream, false)?;
        pseudo_element = pe;
        result = Node::Combined(Box::new(result), combinator, Box::new(next_selector));
    }
    Ok((result, pseudo_element))
}

/// Port of `parse_simple_selector`.
fn parse_simple_selector(stream: &mut TokenStream, inside_negation: bool) -> Result<(Node, Option<PseudoElement>), SelectorError> {
    stream.skip_whitespace();
    let selector_start = stream.used_count;
    let peek = stream.peek().clone();
    let (namespace, element) = if matches!(peek.kind, TokenKind::Ident) || peek.is_delim_char('*') {
        let tentative = if matches!(stream.peek().kind, TokenKind::Ident) { Some(stream.next().value) } else {
            stream.next();
            None
        };
        if stream.peek().is_delim_char('|') {
            stream.next();
            let el = stream.next_ident_or_star()?;
            (tentative, el)
        } else {
            (None, tentative)
        }
    } else {
        (None, None)
    };
    let mut result = Node::Element(ElementSel { namespace, element });
    let mut pseudo_element: Option<PseudoElement> = None;
    loop {
        let peek = stream.peek().clone();
        if matches!(peek.kind, TokenKind::Whitespace | TokenKind::Eof) || peek.is_delim(&[',', '+', '>', '~']) || (inside_negation && peek.is_delim_char(')')) {
            break;
        }
        if pseudo_element.is_some() {
            return Err(SelectorError::Syntax("Got a pseudo-element not at the end of a selector".to_string()));
        }
        match peek.kind {
            TokenKind::Hash => {
                let v = stream.next().value;
                result = Node::IdHash(Box::new(result), v);
            }
            TokenKind::Delim('.') => {
                stream.next();
                let ident = stream.next_ident()?;
                result = Node::Class(Box::new(result), ident);
            }
            TokenKind::Delim('[') => {
                stream.next();
                result = parse_attrib(result, stream)?;
            }
            TokenKind::Delim(':') => {
                stream.next();
                if stream.peek().is_delim_char(':') {
                    stream.next();
                    let name = stream.next_ident()?;
                    if stream.peek().is_delim_char('(') {
                        stream.next();
                        let args = parse_arguments(stream)?;
                        pseudo_element = Some(PseudoElement::Functional(FunctionalPseudoElement { name: name.to_ascii_lowercase(), arguments: args }));
                    } else {
                        // Not lower-cased here -- `Selector::new` does
                        // it uniformly for both this and the special
                        // single-colon branch below (matching real
                        // `Selector.__init__`, see its doc).
                        pseudo_element = Some(PseudoElement::Ident(name));
                    }
                    continue;
                }
                let ident = stream.next_ident()?;
                if SPECIAL_PSEUDO_ELEMENTS.contains(&ident.to_ascii_lowercase().as_str()) {
                    // Real `pseudo_element = unicode_type(ident)` is
                    // NOT lower-cased at this point -- but `Selector::new`
                    // lower-cases it uniformly afterwards regardless, so
                    // the final observable value is always lower-case
                    // either way (see `Selector::new`'s doc).
                    pseudo_element = Some(PseudoElement::Ident(ident));
                    continue;
                }
                if !stream.peek().is_delim_char('(') {
                    result = Node::Pseudo(Box::new(result), ident.to_ascii_lowercase());
                    continue;
                }
                stream.next();
                stream.skip_whitespace();
                if ident.eq_ignore_ascii_case("not") {
                    if inside_negation {
                        return Err(SelectorError::Syntax("Got nested :not()".to_string()));
                    }
                    let (argument, argument_pseudo_element) = parse_simple_selector(stream, true)?;
                    let next = stream.next();
                    if argument_pseudo_element.is_some() {
                        return Err(SelectorError::Syntax(format!("Got a pseudo-element inside :not() at {}", next.pos)));
                    }
                    if !next.is_delim_char(')') {
                        return Err(SelectorError::Syntax(format!("Expected ')', got {next:?}")));
                    }
                    result = Node::Negation(Box::new(result), Box::new(argument));
                } else {
                    let args = parse_arguments(stream)?;
                    result = Node::Function(Box::new(result), FunctionSel { name: ident.to_ascii_lowercase(), arguments: args });
                }
            }
            _ => return Err(SelectorError::Syntax(format!("Expected selector, got {peek:?}"))),
        }
    }
    if stream.used_count == selector_start {
        return Err(SelectorError::Syntax(format!("Expected selector, got {:?}", stream.peek())));
    }
    Ok((result, pseudo_element))
}

/// Port of `parse_arguments`.
fn parse_arguments(stream: &mut TokenStream) -> Result<Vec<Token>, SelectorError> {
    let mut arguments = Vec::new();
    loop {
        stream.skip_whitespace();
        let next = stream.next();
        match next.kind {
            TokenKind::Ident | TokenKind::String | TokenKind::Number => arguments.push(next),
            TokenKind::Delim('+') | TokenKind::Delim('-') => arguments.push(next),
            TokenKind::Delim(')') => return Ok(arguments),
            _ => return Err(SelectorError::Syntax(format!("Expected an argument, got {next:?}"))),
        }
    }
}

/// Port of `parse_attrib`.
///
/// A real, necessary narrowing from Python's duck-typed `attrib=None`
/// possibility: `AttribSel::attrib` is a plain (non-`Option`) `String`,
/// so the pathological `[*|=x]`-shaped input (a wildcard attribute name
/// immediately followed by `|=`, which real upstream lets flow through
/// as an `Attrib` with `attrib=None` and would presumably fail later in
/// `select.py`) is rejected here directly with a syntax error instead.
/// No real selector anyone writes hits this path.
fn parse_attrib(selector: Node, stream: &mut TokenStream) -> Result<Node, SelectorError> {
    stream.skip_whitespace();
    let attrib_opt = stream.next_ident_or_star()?;
    if attrib_opt.is_none() && !stream.peek().is_delim_char('|') {
        return Err(SelectorError::Syntax(format!("Expected '|', got {:?}", stream.peek())));
    }
    let mut namespace: Option<String> = None;
    let mut attrib = attrib_opt;
    let mut op: Option<String> = None;
    if stream.peek().is_delim_char('|') {
        stream.next();
        if stream.peek().is_delim_char('=') {
            stream.next();
            op = Some("|=".to_string());
        } else {
            namespace = attrib.clone();
            attrib = Some(stream.next_ident()?);
        }
    }
    let attrib = attrib.ok_or_else(|| SelectorError::Syntax("Expected an attribute name".to_string()))?;
    let op = match op {
        Some(o) => o,
        None => {
            stream.skip_whitespace();
            let next = stream.next();
            if next.is_delim_char(']') {
                return Ok(Node::Attrib(Box::new(selector), AttribSel { namespace, attrib, operator: "exists".to_string(), value: None }));
            } else if next.is_delim_char('=') {
                "=".to_string()
            } else if next.is_delim(&['^', '$', '*', '~', '|', '!']) && stream.peek().is_delim_char('=') {
                let c = match next.kind {
                    TokenKind::Delim(c) => c,
                    _ => unreachable!(),
                };
                stream.next();
                format!("{c}=")
            } else {
                return Err(SelectorError::Syntax(format!("Operator expected, got {next:?}")));
            }
        }
    };
    stream.skip_whitespace();
    let value_tok = stream.next();
    if !matches!(value_tok.kind, TokenKind::Ident | TokenKind::String) {
        return Err(SelectorError::Syntax(format!("Expected string or ident, got {value_tok:?}")));
    }
    stream.skip_whitespace();
    let next = stream.next();
    if !next.is_delim_char(']') {
        return Err(SelectorError::Syntax(format!("Expected ']', got {next:?}")));
    }
    Ok(Node::Attrib(Box::new(selector), AttribSel { namespace, attrib, operator: op, value: Some(value_tok.value) }))
}

/// Port of `parse_series`: parses `:nth-child()`-and-friends' `An+B`
/// microsyntax from an already-tokenized argument list. Returns a
/// plain `Err(String)` (port of the real `raise ValueError`) rather
/// than [`SelectorError`] directly -- [`FunctionSel::parsed_arguments`]
/// wraps it into `SelectorError::Expression`, matching real
/// `Function.parsed_arguments`'s own `except ValueError: raise
/// ExpressionError(...)`.
pub fn parse_series(tokens: &[Token]) -> Result<(i64, i64), String> {
    if tokens.iter().any(|t| matches!(t.kind, TokenKind::String)) {
        return Err("String tokens not allowed in series.".to_string());
    }
    let joined: String = tokens.iter().map(|t| t.value.as_str()).collect();
    let s = joined.trim();
    if s == "odd" {
        return Ok((2, 1));
    }
    if s == "even" {
        return Ok((2, 0));
    }
    if s == "n" {
        return Ok((1, 0));
    }
    if !s.contains('n') {
        let b: i64 = s.parse().map_err(|_| format!("invalid integer: {s}"))?;
        return Ok((0, b));
    }
    let mut parts = s.splitn(2, 'n');
    let a_str = parts.next().unwrap_or("");
    let b_str = parts.next().unwrap_or("");
    let a: i64 = if a_str.is_empty() {
        1
    } else if a_str == "-" {
        -1
    } else if a_str == "+" {
        1
    } else {
        a_str.parse().map_err(|_| format!("invalid integer: {a_str}"))?
    };
    let b: i64 = if b_str.is_empty() { 0 } else { b_str.parse().map_err(|_| format!("invalid integer: {b_str}"))? };
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok_values(tokens: &[Token]) -> Vec<(TokenKind, &str)> {
        tokens.iter().map(|t| (t.kind, t.value.as_str())).collect()
    }

    #[test]
    fn tokenizer_splits_a_digit_then_ident_into_two_tokens_not_a_dimension() {
        // The whole reason this module doesn't reuse `cssparser` -- see
        // the module doc.
        let tokens = tokenize("2n+1").unwrap();
        assert_eq!(
            tok_values(&tokens[..tokens.len() - 1]),
            vec![(TokenKind::Number, "2"), (TokenKind::Ident, "n"), (TokenKind::Number, "+1")]
        );
    }

    #[test]
    fn tokenizer_handles_escapes_in_idents_and_hex_unicode_escapes() {
        let tokens = tokenize(r"\41 bc").unwrap(); // \41 + space-terminator + "bc" -> "Abc"
        assert_eq!(tokens[0].kind, TokenKind::Ident);
        assert_eq!(tokens[0].value, "Abc");
    }

    #[test]
    fn tokenizer_reads_quoted_strings_with_escapes_and_the_other_quote_char_free() {
        let tokens = tokenize(r#""it's \"ok\"""#).unwrap();
        assert_eq!(tokens[0].kind, TokenKind::String);
        assert_eq!(tokens[0].value, "it's \"ok\"");
    }

    #[test]
    fn tokenizer_errors_on_an_unclosed_string() {
        let err = tokenize("\"abc").unwrap_err();
        assert!(matches!(err, SelectorError::Syntax(_)));
    }

    #[test]
    fn tokenizer_skips_comments() {
        let tokens = tokenize("a/* comment */b").unwrap();
        assert_eq!(tok_values(&tokens[..tokens.len() - 1]), vec![(TokenKind::Ident, "a"), (TokenKind::Ident, "b")]);
    }

    #[test]
    fn fast_path_handles_a_bare_type_selector() {
        let sels = parse("  div  ").unwrap();
        assert_eq!(sels.len(), 1);
        assert_eq!(sels[0].parsed_tree, Node::Element(ElementSel { namespace: None, element: Some("div".to_string()) }));
    }

    #[test]
    fn fast_path_handles_a_bare_id_and_class_selector() {
        let id_sel = parse("#foo").unwrap();
        assert_eq!(
            id_sel[0].parsed_tree,
            Node::IdHash(Box::new(Node::Element(ElementSel { namespace: None, element: None })), "foo".to_string())
        );
        let class_sel = parse("div.bar").unwrap();
        assert_eq!(
            class_sel[0].parsed_tree,
            Node::Class(Box::new(Node::Element(ElementSel { namespace: None, element: Some("div".to_string()) })), "bar".to_string())
        );
    }

    #[test]
    fn parses_a_compound_selector_with_class_id_and_attribute() {
        let sels = parse(r#"div.a#b[href="x"]"#).unwrap();
        assert_eq!(sels.len(), 1);
        let Node::Attrib(inner, attr) = &sels[0].parsed_tree else { panic!("expected Attrib at the top") };
        assert_eq!(attr.attrib, "href");
        assert_eq!(attr.operator, "=");
        assert_eq!(attr.value.as_deref(), Some("x"));
        let Node::IdHash(inner2, id) = inner.as_ref() else { panic!("expected IdHash") };
        assert_eq!(id, "b");
        let Node::Class(inner3, class) = inner2.as_ref() else { panic!("expected Class") };
        assert_eq!(class, "a");
        assert_eq!(**inner3, Node::Element(ElementSel { namespace: None, element: Some("div".to_string()) }));
    }

    #[test]
    fn parses_not_and_a_pseudo_class_with_arguments() {
        let sels = parse(":not(.c):nth-child(2n+1)").unwrap();
        let Node::Function(inner, func) = &sels[0].parsed_tree else { panic!("expected Function") };
        assert_eq!(func.name, "nth-child");
        assert_eq!(func.parsed_arguments().unwrap(), (2, 1));
        let Node::Negation(_, sub) = inner.as_ref() else { panic!("expected Negation") };
        assert_eq!(**sub, Node::Class(Box::new(Node::Element(ElementSel { namespace: None, element: None })), "c".to_string()));
    }

    #[test]
    fn nested_not_is_rejected() {
        let err = parse(":not(:not(.a))").unwrap_err();
        assert!(matches!(err, SelectorError::Syntax(msg) if msg.contains("nested")));
    }

    #[test]
    fn parses_descendant_child_and_sibling_combinators() {
        let sels = parse("a b").unwrap();
        assert!(matches!(&sels[0].parsed_tree, Node::Combined(_, ' ', _)));
        let sels = parse("a > b").unwrap();
        assert!(matches!(&sels[0].parsed_tree, Node::Combined(_, '>', _)));
        let sels = parse("a + b").unwrap();
        assert!(matches!(&sels[0].parsed_tree, Node::Combined(_, '+', _)));
        let sels = parse("a ~ b").unwrap();
        assert!(matches!(&sels[0].parsed_tree, Node::Combined(_, '~', _)));
    }

    #[test]
    fn parses_a_comma_separated_selector_group() {
        let sels = parse("a, b , c").unwrap();
        assert_eq!(sels.len(), 3);
    }

    #[test]
    fn parses_attribute_operator_variants() {
        for (src, op) in [
            (r#"[a~="x"]"#, "~="),
            (r#"[a|="x"]"#, "|="),
            (r#"[a^="x"]"#, "^="),
            (r#"[a$="x"]"#, "$="),
            (r#"[a*="x"]"#, "*="),
            (r#"[a!="x"]"#, "!="),
            ("[a]", "exists"),
        ] {
            let sels = parse(src).unwrap();
            let Node::Attrib(_, attr) = &sels[0].parsed_tree else { panic!("expected Attrib for {src}") };
            assert_eq!(attr.operator, op, "for {src}");
        }
    }

    /// Cross-validated directly against the live real Python
    /// `css_selectors.parser.parse` (a temporary stub `css_selectors`
    /// package pointing only at `errors.py`/`parser.py`, sidestepping
    /// `select.py`'s `lxml` import -- this module has no real caller
    /// needing `select.py` yet). This is what caught the pseudo-element
    /// casing bug fixed in `Selector::new`.
    #[test]
    fn namespace_and_functional_pseudo_element_parsing_matches_real_upstream() {
        let sels = parse("ns|div").unwrap();
        assert_eq!(sels[0].parsed_tree, Node::Element(ElementSel { namespace: Some("ns".to_string()), element: Some("div".to_string()) }));

        let sels = parse("*|div").unwrap();
        assert_eq!(sels[0].parsed_tree, Node::Element(ElementSel { namespace: None, element: Some("div".to_string()) }));

        let sels = parse("div[ns|attr=x]").unwrap();
        let Node::Attrib(_, attr) = &sels[0].parsed_tree else { panic!("expected Attrib") };
        assert_eq!(attr.namespace.as_deref(), Some("ns"));
        assert_eq!(attr.attrib, "attr");
        assert_eq!(attr.operator, "=");
        assert_eq!(attr.value.as_deref(), Some("x"));

        let sels = parse("a::foo(bar)").unwrap();
        let Some(PseudoElement::Functional(fpe)) = &sels[0].pseudo_element else { panic!("expected a functional pseudo-element") };
        assert_eq!(fpe.name, "foo");
        assert_eq!(fpe.arguments.len(), 1);
        assert_eq!(fpe.arguments[0].value, "bar");
    }

    #[test]
    fn pseudo_element_names_are_always_lower_cased_regardless_of_which_colon_form() {
        // Verified directly against the live real Python function
        // (see `Selector::new`'s doc): both `:Before` and `::Before`
        // really produce `"before"`, even though the single-colon
        // branch's own local variable is untouched at that point --
        // `Selector.__init__` lower-cases it one level up regardless.
        let sels = parse(":Before").unwrap();
        let Some(PseudoElement::Ident(name)) = &sels[0].pseudo_element else { panic!("expected an ident pseudo-element") };
        assert_eq!(name, "before");

        let sels = parse("a::Before").unwrap();
        let Some(PseudoElement::Ident(name)) = &sels[0].pseudo_element else { panic!("expected an ident pseudo-element") };
        assert_eq!(name, "before");
    }

    #[test]
    fn specificity_matches_the_real_a_b_c_formula() {
        // #id.class[attr] -> a=1 (id), b=2 (class+attr), c=0 (no type)
        let sels = parse(r#"#id.class[attr]"#).unwrap();
        assert_eq!(sels[0].specificity(), (1, 2, 0));
        // div -> a=0,b=0,c=1
        let sels = parse("div").unwrap();
        assert_eq!(sels[0].specificity(), (0, 0, 1));
        // a pseudo-element adds 1 to c
        let sels = parse("div::before").unwrap();
        assert_eq!(sels[0].specificity(), (0, 0, 2));
    }

    #[test]
    fn parse_series_handles_odd_even_n_and_an_plus_b() {
        let series = |s: &str| {
            let tokens = tokenize(s).unwrap();
            let args: Vec<Token> = tokens.into_iter().filter(|t| !matches!(t.kind, TokenKind::Eof)).collect();
            parse_series(&args).unwrap()
        };
        assert_eq!(series("odd"), (2, 1));
        assert_eq!(series("even"), (2, 0));
        assert_eq!(series("n"), (1, 0));
        assert_eq!(series("3"), (0, 3));
        assert_eq!(series("2n+1"), (2, 1));
        assert_eq!(series("-n+3"), (-1, 3));
        assert_eq!(series("n-1"), (1, -1));
    }

    #[test]
    fn parse_series_rejects_string_tokens() {
        let tokens = vec![Token { kind: TokenKind::String, value: "x".to_string(), pos: 0 }];
        assert!(parse_series(&tokens).is_err());
    }
}
