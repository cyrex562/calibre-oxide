//! Port of `formatter.py`'s `_Parser`: a recursive-descent parser
//! turning a token stream (see [`super::lexer`]) into an
//! [`super::ast::Expr`] tree.
//!
//! # Disclosed narrowing
//!
//! - Stored GPM/Python templates (`StoredTemplateCallNode`,
//!   `call_expression`, `StoredObjectType`) aren't ported -- that
//!   whole mechanism (named, separately-stored template definitions a
//!   template can call by name, with their own compile-and-cache
//!   machinery) needs a storage/registry system that doesn't exist
//!   anywhere in this Rust port yet. A registered name that isn't a
//!   known [`FunctionCatalog`] entry or a local function is still a
//!   real "unknown function" parse error, same observable behavior
//!   for everything this port *does* support.
//! - The 17 inlined-shortcut names (`field`/`raw_field`/`test`/
//!   `first_non_empty`/`switch`/`switch_if`/`assign`/`contains`/
//!   `character`/`print`/`strcat`/`list_count_field`/`f_string`/
//!   `list_split`/`eval`/`template`/`lookup`) are
//!   always recognized by this parser as their special forms,
//!   regardless of what a [`FunctionCatalog`] says -- upstream
//!   actually requires these to *also* be present in the real
//!   function registry for its initial "is this a known name at all"
//!   gate to pass, since the registry is what the parser primarily
//!   consults. Since #513 has no real registry yet, gating on it here
//!   would make the language core unusable standalone; these names
//!   are internally always-known instead, and (per upstream) an
//!   arity mismatch against the inlined form is a hard parse error in
//!   this port, not a fallback to a same-named registered function of
//!   a different arity (relevant for `raw_field`'s 2-argument form --
//!   see `build_inlined`'s own comment).

use super::ast::{Expr, ExprKind, Param};
use super::lexer::{Token, TokenKind};
use anyhow::{bail, Result};
use std::collections::HashSet;

/// What the parser needs to know about a registered template
/// function to parse a call to it correctly: whether it exists at
/// all, and its arity (`None` = variadic, matching upstream's
/// `arg_count == -1`).
pub trait FunctionCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>>;
    fn contains(&self, name: &str) -> bool {
        self.arg_count(name).is_some()
    }
}

/// A catalog with no registered functions -- every call other than a
/// local function or an inlined/keyword form is an "unknown function"
/// parse error. Useful for testing the language core in isolation;
/// real callers (once #514+ land) will supply a catalog backed by the
/// real `formatter_functions` registry.
pub struct EmptyCatalog;
impl FunctionCatalog for EmptyCatalog {
    fn arg_count(&self, _name: &str) -> Option<Option<usize>> {
        None
    }
}

fn seq(line: u32, mut list: Vec<Expr>) -> Box<Expr> {
    if list.len() == 1 {
        Box::new(list.pop().unwrap())
    } else {
        Box::new(Expr::new(line, ExprKind::Sequence(list)))
    }
}

pub struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    line: u32,
    catalog: &'a dyn FunctionCatalog,
    local_functions: HashSet<String>,
}

/// Parses a full program (a top-level `;`-separated expression list)
/// from an already-tokenized source. `local_functions` seeds the set
/// of local-function names already in scope -- upstream uses this to
/// let an `f_string`'s embedded `{...}` sub-programs see local
/// functions defined in the enclosing template.
pub fn parse(tokens: &[Token], catalog: &dyn FunctionCatalog, local_functions: HashSet<String>) -> Result<Expr> {
    let mut p = Parser { tokens, pos: 0, line: 1, catalog, local_functions };
    let list = p.expression_list()?;
    if !p.at_eof() {
        bail!("Formatter: Expected end of program, found '{}'", p.token_text());
    }
    Ok(*seq(1, list))
}

impl<'a> Parser<'a> {
    fn err<T>(&mut self, message: impl Into<String>) -> Result<T> {
        let message = message.into();
        if self.pos > 0 && self.pos < self.tokens.len() {
            bail!("Formatter: {} near '{}' on line {}", message, self.token_text(), self.line)
        } else {
            bail!("Formatter: {} near the end of the program", message)
        }
    }

    fn check_eol(&mut self) {
        while self.pos < self.tokens.len() && self.tokens[self.pos].kind == TokenKind::Newline {
            self.line += 1;
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<&Token> {
        self.check_eol();
        self.tokens.get(self.pos)
    }

    fn consume(&mut self) {
        self.pos += 1;
    }

    fn token(&mut self) -> Option<String> {
        self.check_eol();
        let t = self.tokens.get(self.pos).map(|t| t.text.clone());
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn token_op_is(&mut self, op: &str) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Op && t.text == op)
    }

    fn token_is_string_infix(&mut self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::StringInfix)
    }

    fn token_is_numeric_infix(&mut self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::NumericInfix)
    }

    fn token_is_id(&mut self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Id)
    }

    fn token_is(&mut self, kw: &str) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Keyword && t.text == kw)
    }

    fn token_is_keyword(&mut self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Keyword)
    }

    fn token_is_constant(&mut self) -> bool {
        matches!(self.peek(), Some(t) if t.kind == TokenKind::Const)
    }

    fn at_eof(&mut self) -> bool {
        self.peek().is_none()
    }

    fn token_text(&mut self) -> String {
        self.peek().map(|t| t.text.clone()).unwrap_or_else(|| "'End of program'".to_string())
    }

    // ------------------------------------------------------------ statements

    fn expression_list(&mut self) -> Result<Vec<Expr>> {
        let mut list = Vec::new();
        loop {
            self.check_eol();
            if self.at_eof() {
                break;
            }
            list.push(self.top_expr()?);
            if self.token_op_is(";") {
                self.consume();
            } else {
                break;
            }
        }
        Ok(list)
    }

    fn if_expression(&mut self) -> Result<Expr> {
        self.consume(); // 'if'
        let line = self.line;
        let condition = Box::new(self.top_expr()?);
        if !self.token_is("then") {
            let t = self.token_text();
            return self.err(format!("'if' statement: expected 'then', found '{t}'"));
        }
        self.consume();
        let then_part = seq(line, self.expression_list()?);
        if self.token_is("elif") {
            let elif = self.if_expression()?;
            return Ok(Expr::new(line, ExprKind::If { condition, then_part, else_part: Some(seq(line, vec![elif])) }));
        }
        let else_part = if self.token_is("else") {
            self.consume();
            Some(seq(line, self.expression_list()?))
        } else {
            None
        };
        if !self.token_is("fi") {
            let t = self.token_text();
            return self.err(format!("'if' statement: expected 'fi', found '{t}'"));
        }
        self.consume();
        Ok(Expr::new(line, ExprKind::If { condition, then_part, else_part }))
    }

    fn for_expression(&mut self) -> Result<Expr> {
        let line = self.line;
        self.consume(); // 'for'
        if !self.token_is_id() {
            return self.err("'for' statement: expected an identifier");
        }
        let variable = self.token().unwrap();
        if !self.token_is("in") {
            let t = self.token_text();
            return self.err(format!("'for' statement: expected 'in', found '{t}'"));
        }
        self.consume();

        let is_range = self.token_text() == "range";
        let (list_expr, separator, start, stop, step, limit);
        if is_range {
            self.consume();
            if !self.token_op_is("(") {
                let t = self.token_text();
                return self.err(format!("'for' statement: expected '(', found '{t}'"));
            }
            self.consume();
            let mut start_e = Expr::new(line, ExprKind::Constant("0".to_string()));
            let mut step_e = Expr::new(line, ExprKind::Constant("1".to_string()));
            let mut limit_e = None;
            let mut stop_e = self.top_expr()?;
            if self.token_op_is(",") {
                self.consume();
                start_e = stop_e;
                stop_e = self.top_expr()?;
                if self.token_op_is(",") {
                    self.consume();
                    step_e = self.top_expr()?;
                    if self.token_op_is(",") {
                        self.consume();
                        limit_e = Some(Box::new(self.top_expr()?));
                    }
                }
            }
            if !self.token_op_is(")") {
                let t = self.token_text();
                return self.err(format!("'for' statement: expected ')', found '{t}'"));
            }
            self.consume();
            list_expr = None;
            separator = None;
            start = Some(Box::new(start_e));
            stop = Some(Box::new(stop_e));
            step = Some(Box::new(step_e));
            limit = limit_e;
        } else {
            let le = self.top_expr()?;
            let sep = if self.token_is("separator") {
                self.consume();
                Some(Box::new(self.expr()?))
            } else {
                None
            };
            list_expr = Some(Box::new(le));
            separator = sep;
            start = None;
            stop = None;
            step = None;
            limit = None;
        }

        if !self.token_op_is(":") {
            let t = self.token_text();
            return self.err(format!("'for' statement: expected ':', found '{t}'"));
        }
        self.consume();
        let block = seq(line, self.expression_list()?);
        if !self.token_is("rof") {
            let t = self.token_text();
            return self.err(format!("'for' statement: expected 'rof', found '{t}'"));
        }
        self.consume();

        if is_range {
            Ok(Expr::new(line, ExprKind::Range { variable, start: start.unwrap(), stop: stop.unwrap(), step: step.unwrap(), limit, block }))
        } else {
            Ok(Expr::new(line, ExprKind::For { variable, list_expr: list_expr.unwrap(), separator, block }))
        }
    }

    fn define_function_expression(&mut self) -> Result<Expr> {
        self.consume(); // 'def'
        let line = self.line;
        if !self.token_is_id() {
            return self.err("'def' statement: expected a function name identifier");
        }
        let name = self.token().unwrap();
        if self.local_functions.contains(&name) {
            return self.err(format!("Function name '{name}' is already defined"));
        }
        if !self.token_op_is("(") {
            return self.err("'def' statement: expected a '('");
        }
        self.consume();
        let mut params = Vec::new();
        while !self.token_op_is(")") {
            let a = self.top_expr()?;
            let param = match a.kind {
                ExprKind::Assign { name, value } => Param { name, default: value },
                ExprKind::Variable(name) => Param { name, default: Box::new(Expr::new(line, ExprKind::Constant(String::new()))) },
                _ => return self.err("Parameters to a function must be variables or assignments"),
            };
            params.push(param);
            if !self.token_op_is(",") {
                break;
            }
            self.consume();
        }
        if self.token().as_deref() != Some(")") {
            return self.err("'def' statement: expected a ')' at end of argument list");
        }
        if !self.token_op_is(":") {
            return self.err("'def' statement: missing ':'");
        }
        self.consume();
        let block = seq(line, self.expression_list()?);
        if !self.token_is("fed") {
            return self.err("'def' statement: missing the closing 'fed'");
        }
        self.consume();
        // Added to local_functions only *after* the body is parsed --
        // matches upstream exactly, which means a function cannot
        // recursively call itself by name (a real, faithfully
        // preserved language limitation, not a bug this port
        // introduced).
        self.local_functions.insert(name.clone());
        Ok(Expr::new(line, ExprKind::LocalFunctionDefine { name, params, block }))
    }

    fn with_expression(&mut self) -> Result<Expr> {
        self.consume(); // 'with'
        let line = self.line;
        let book_id = Box::new(self.top_expr()?);
        if !self.token_op_is(":") {
            let t = self.token_text();
            return self.err(format!("'with' statement: expected ':', found '{t}'"));
        }
        self.consume();
        let block = seq(line, self.expression_list()?);
        if !self.token_is("htiw") {
            // Upstream's own error message here literally says
            // "'def' statement: missing the closing 'fed'" -- a
            // verbatim copy-paste typo in formatter.py, not a
            // deliberate message. Preserved as-is (cosmetic only,
            // doesn't affect parsing) rather than silently "fixed",
            // per this project's general policy of disclosing rather
            // than quietly diverging from observed upstream behavior.
            return self.err("'def' statement: missing the closing 'fed'");
        }
        self.consume();
        Ok(Expr::new(line, ExprKind::With { book_id, block }))
    }

    // ------------------------------------------------------- expressions

    fn top_expr(&mut self) -> Result<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut left = self.and_expr()?;
        while self.token_op_is("||") {
            self.consume();
            let right = self.and_expr()?;
            left = Expr::new(self.line, ExprKind::LogopBinary { op: "or".to_string(), left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut left = self.not_expr()?;
        while self.token_op_is("&&") {
            self.consume();
            let right = self.not_expr()?;
            left = Expr::new(self.line, ExprKind::LogopBinary { op: "and".to_string(), left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn not_expr(&mut self) -> Result<Expr> {
        if self.token_op_is("!") {
            self.consume();
            let inner = self.not_expr()?;
            return Ok(Expr::new(self.line, ExprKind::LogopUnary { expr: Box::new(inner) }));
        }
        self.string_binary_expr()
    }

    fn string_binary_expr(&mut self) -> Result<Expr> {
        let mut left = self.compare_expr()?;
        while self.token_op_is("&") {
            self.consume();
            let right = self.compare_expr()?;
            left = Expr::new(self.line, ExprKind::StringBinary { left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn compare_expr(&mut self) -> Result<Expr> {
        let left = self.add_subtract_expr()?;
        if self.token_is_string_infix() || self.token_is("in") || self.token_is("inlist") || self.token_is("inlist_field") {
            let op = self.token().unwrap();
            let right = self.add_subtract_expr()?;
            return Ok(Expr::new(self.line, ExprKind::StringCompare { op, left: Box::new(left), right: Box::new(right) }));
        }
        if self.token_is_numeric_infix() {
            let op = self.token().unwrap();
            let right = self.add_subtract_expr()?;
            return Ok(Expr::new(self.line, ExprKind::NumericCompare { op, left: Box::new(left), right: Box::new(right) }));
        }
        Ok(left)
    }

    fn add_subtract_expr(&mut self) -> Result<Expr> {
        let mut left = self.times_divide_expr()?;
        while self.token_op_is("+") || self.token_op_is("-") {
            let op = self.token().unwrap();
            let right = self.times_divide_expr()?;
            left = Expr::new(self.line, ExprKind::NumericBinary { op, left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn times_divide_expr(&mut self) -> Result<Expr> {
        let mut left = self.unary_plus_minus_expr()?;
        while self.token_op_is("*") || self.token_op_is("/") {
            let op = self.token().unwrap();
            let right = self.unary_plus_minus_expr()?;
            left = Expr::new(self.line, ExprKind::NumericBinary { op, left: Box::new(left), right: Box::new(right) });
        }
        Ok(left)
    }

    fn unary_plus_minus_expr(&mut self) -> Result<Expr> {
        if self.token_op_is("+") {
            self.consume();
            let inner = self.unary_plus_minus_expr()?;
            return Ok(Expr::new(self.line, ExprKind::NumericUnary { negate: false, expr: Box::new(inner) }));
        }
        if self.token_op_is("-") {
            self.consume();
            let inner = self.unary_plus_minus_expr()?;
            return Ok(Expr::new(self.line, ExprKind::NumericUnary { negate: true, expr: Box::new(inner) }));
        }
        self.expr()
    }

    /// Parses one `,`-separated function-call argument list, each
    /// argument itself a full `;`-separated expression list --
    /// matches upstream's `arguments.append(self.expression_list())`.
    fn call_arguments(&mut self) -> Result<Vec<Expr>> {
        self.consume(); // '('
        let mut args = Vec::new();
        while !self.token_op_is(")") {
            let list = self.expression_list()?;
            args.push(*seq(self.line, list));
            if !self.token_op_is(",") {
                break;
            }
            self.consume();
        }
        if self.token().as_deref() != Some(")") {
            return self.err("Expected a ')' for function call, found this instead");
        }
        Ok(args)
    }

    fn expr(&mut self) -> Result<Expr> {
        if self.token_op_is("(") {
            self.consume();
            let line = self.line;
            let list = self.expression_list()?;
            if !self.token_op_is(")") {
                let t = self.token_text();
                return self.err(format!("Expected ')', found '{t}'"));
            }
            self.consume();
            return Ok(*seq(line, list));
        }

        if self.token_is_keyword() {
            let t = self.token_text();
            match t.as_str() {
                "if" => return self.if_expression(),
                "for" => return self.for_expression(),
                "break" => {
                    self.consume();
                    return Ok(Expr::new(self.line, ExprKind::Break));
                }
                "continue" => {
                    self.consume();
                    return Ok(Expr::new(self.line, ExprKind::Continue));
                }
                "return" => {
                    self.consume();
                    let v = self.top_expr()?;
                    return Ok(Expr::new(self.line, ExprKind::Return(Box::new(v))));
                }
                "def" => return self.define_function_expression(),
                "with" => return self.with_expression(),
                _ => {} // other keywords ('then'/'fi'/'in'/'rof'/... ) fall through to the error branch below
            }
        }

        if self.token_is_id() {
            let line = self.line;
            let id_raw = self.token().unwrap();
            let chars: Vec<char> = id_raw.chars().collect();
            if chars.len() > 1 && chars[0] == '$' {
                if chars[1] == '$' {
                    let name: String = chars[2..].iter().collect();
                    return Ok(Expr::new(line, ExprKind::RawField { expr: Box::new(Expr::new(line, ExprKind::Constant(name))), default: None }));
                }
                let name: String = chars[1..].iter().collect();
                return Ok(Expr::new(line, ExprKind::Field(Box::new(Expr::new(line, ExprKind::Constant(name))))));
            }

            if !self.token_op_is("(") {
                if self.token_op_is("=") {
                    self.consume();
                    let value = Box::new(self.top_expr()?);
                    return Ok(Expr::new(line, ExprKind::Assign { name: id_raw, value }));
                }
                return Ok(Expr::new(line, ExprKind::Variable(id_raw)));
            }

            let id_ = id_raw.trim().to_string();
            if !self.catalog.contains(&id_) && !self.local_functions.contains(&id_) && inlined_function_arity(&id_).is_none() && !matches!(id_.as_str(), "arguments" | "globals" | "set_globals") {
                return self.err(format!("Unknown function {id_}"));
            }

            let arguments = self.call_arguments()?;

            if let Some(node) = build_inlined(line, &id_, &arguments) {
                return node;
            }

            if matches!(id_.as_str(), "arguments" | "globals" | "set_globals") {
                let mut params = Vec::new();
                for arg in arguments {
                    let param = match arg.kind {
                        ExprKind::Assign { name, value } => Param { name, default: value },
                        ExprKind::Variable(name) => Param { name, default: Box::new(Expr::new(line, ExprKind::Constant(String::new()))) },
                        _ => return self.err(format!("Parameters to '{id_}' must be variables or assignments")),
                    };
                    params.push(param);
                }
                let kind = match id_.as_str() {
                    "arguments" => ExprKind::Arguments(params),
                    "set_globals" => ExprKind::SetGlobals(params),
                    _ => ExprKind::Globals(params),
                };
                return Ok(Expr::new(line, kind));
            }

            if self.local_functions.contains(&id_) {
                return Ok(Expr::new(line, ExprKind::LocalFunctionCall { name: id_, args: arguments }));
            }

            if let Some(arity) = self.catalog.arg_count(&id_) {
                if let Some(n) = arity {
                    if arguments.len() != n {
                        return self.err(format!("Incorrect number of arguments for function {id_}"));
                    }
                }
                return Ok(Expr::new(line, ExprKind::Func { name: id_, args: arguments }));
            }
            unreachable!("presence was already checked above");
        }

        if self.token_is_constant() {
            let line = self.line;
            let v = self.token().unwrap();
            return Ok(Expr::new(line, ExprKind::Constant(v)));
        }

        let t = self.token_text();
        self.err(format!("Expected an expression, found '{t}'"))
    }
}

/// Port of `inlined_function_nodes`'s arity-validator half -- `None`
/// means "not an inlined form at all"; `Some(())` here just means
/// membership (the real arity check happens inside
/// [`build_inlined`], which returns a real parse error for a bad
/// count rather than silently falling through to "unknown function").
fn inlined_function_arity(name: &str) -> Option<()> {
    matches!(name, "field" | "raw_field" | "test" | "first_non_empty" | "switch" | "switch_if" | "assign" | "contains" | "character" | "print" | "strcat" | "list_count_field" | "f_string" | "list_split" | "eval" | "template" | "lookup").then_some(())
}

fn build_inlined(line: u32, name: &str, args: &[Expr]) -> Option<Result<Expr>> {
    let bad_arity = || Some(Err(anyhow::anyhow!("Formatter: incorrect number of arguments for inlined function '{name}' near line {line}")));
    match name {
        "field" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Field(Box::new(args[0].clone())))))
        }
        "raw_field" => {
            // Upstream's own inlined-shortcut validator requires
            // *exactly* one argument (`RawFieldNode(ln, *args)` with
            // `len(args)==1`) -- a 2-arg `raw_field(name, default)`
            // call falls through to being looked up as a separately
            // *registered* function of the same name in upstream
            // (real in `formatter_functions.py`'s GET_FROM_METADATA
            // category), which this port doesn't have yet. Disclosed
            // narrowing: `default` is only reachable in this port
            // via the real registry once it lands (issue #514).
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::RawField { expr: Box::new(args[0].clone()), default: None })))
        }
        "test" => {
            if args.len() != 3 {
                return bad_arity();
            }
            Some(Ok(Expr::new(
                line,
                ExprKind::If { condition: Box::new(args[0].clone()), then_part: Box::new(args[1].clone()), else_part: Some(Box::new(args[2].clone())) },
            )))
        }
        "first_non_empty" => {
            if args.is_empty() {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::FirstNonEmpty(args.to_vec()))))
        }
        "switch" => {
            if args.len() < 3 || args.len() % 2 != 0 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Switch(args.to_vec()))))
        }
        "switch_if" => {
            if args.is_empty() || args.len() % 2 != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::SwitchIf(args.to_vec()))))
        }
        "assign" => {
            if args.len() != 2 {
                return bad_arity();
            }
            let ExprKind::Variable(var_name) = &args[0].kind else {
                return bad_arity();
            };
            Some(Ok(Expr::new(line, ExprKind::Assign { name: var_name.clone(), value: Box::new(args[1].clone()) })))
        }
        "contains" => {
            if args.len() != 4 {
                return bad_arity();
            }
            Some(Ok(Expr::new(
                line,
                ExprKind::Contains { value: Box::new(args[0].clone()), test: Box::new(args[1].clone()), matched: Box::new(args[2].clone()), not_matched: Box::new(args[3].clone()) },
            )))
        }
        "character" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Character(Box::new(args[0].clone())))))
        }
        "print" => Some(Ok(Expr::new(line, ExprKind::Print(args.to_vec())))),
        "strcat" => Some(Ok(Expr::new(line, ExprKind::Strcat(args.to_vec())))),
        "list_count_field" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::ListCountField(Box::new(args[0].clone())))))
        }
        "list_split" => {
            if args.len() != 3 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::ListSplit { list_val: Box::new(args[0].clone()), sep: Box::new(args[1].clone()), id_prefix: Box::new(args[2].clone()) })))
        }
        "f_string" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::FString(Box::new(args[0].clone())))))
        }
        "eval" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Eval(Box::new(args[0].clone())))))
        }
        "template" => {
            if args.len() != 1 {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Template(Box::new(args[0].clone())))))
        }
        "lookup" => {
            // Real arity ("2, or an odd count >= 3") is checked at
            // eval time in upstream (`arg_count = -1`, fully
            // variadic) -- matched here, not at parse time.
            if args.is_empty() {
                return bad_arity();
            }
            Some(Ok(Expr::new(line, ExprKind::Lookup { value: Box::new(args[0].clone()), args: args[1..].to_vec() })))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::lexer::scan;

    fn parse_src(src: &str) -> Result<Expr> {
        let tokens = scan(src).map_err(|pos| anyhow::anyhow!("lex error at byte {pos}"))?;
        parse(&tokens, &EmptyCatalog, Default::default())
    }

    #[test]
    fn parses_a_dollar_field_shorthand() {
        let e = parse_src("$title").unwrap();
        assert!(matches!(e.kind, ExprKind::Field(_)));
    }

    #[test]
    fn parses_a_double_dollar_raw_field_shorthand() {
        let e = parse_src("$$title").unwrap();
        assert!(matches!(e.kind, ExprKind::RawField { default: None, .. }));
    }

    #[test]
    fn respects_arithmetic_operator_precedence() {
        // 1 + 2 * 3 should parse as 1 + (2 * 3), not (1 + 2) * 3.
        let e = parse_src("1 + 2 * 3").unwrap();
        let ExprKind::NumericBinary { op, left, right } = &e.kind else { panic!("expected NumericBinary, got {:?}", e.kind) };
        assert_eq!(op, "+");
        assert!(matches!(left.kind, ExprKind::Constant(_)));
        assert!(matches!(right.kind, ExprKind::NumericBinary { .. }));
    }

    #[test]
    fn parses_an_if_then_elif_else_fi_chain() {
        let e = parse_src("if 1 then 'a' elif 2 then 'b' else 'c' fi").unwrap();
        let ExprKind::If { else_part, .. } = &e.kind else { panic!("expected If") };
        let else_part = else_part.as_ref().unwrap();
        // The elif chain becomes a single-element Sequence wrapping a
        // nested If -- but seq() unwraps a length-1 list to the bare
        // node, so else_part should just *be* the nested If directly.
        assert!(matches!(else_part.kind, ExprKind::If { .. }));
    }

    #[test]
    fn parses_a_for_in_list_loop_with_a_separator() {
        let e = parse_src("for x in $tags separator ', ': x rof").unwrap();
        assert!(matches!(e.kind, ExprKind::For { separator: Some(_), .. }));
    }

    #[test]
    fn parses_a_for_in_range_loop_with_all_four_positional_args() {
        let e = parse_src("for x in range(0, 10, 2, 100): x rof").unwrap();
        let ExprKind::Range { limit, .. } = &e.kind else { panic!("expected Range") };
        assert!(limit.is_some());
    }

    #[test]
    fn rejects_a_local_function_calling_itself_recursively() {
        // Matches upstream: a local function is only added to the
        // known-local-functions set *after* its body is parsed.
        let err = parse_src("def f(x): f(x) fed; f(1)").unwrap_err();
        assert!(err.to_string().contains("Unknown function"), "got: {err}");
    }

    #[test]
    fn a_local_function_can_be_called_after_its_definition() {
        // Real upstream grammar note: top-level statements must be
        // `;`-separated -- bare juxtaposition (`fed f(1)` with no
        // `;`) leaves `f(1)` as unconsumed trailing text and is a
        // parse error, not two implicitly-sequenced statements.
        let e = parse_src("def f(x): x fed; f(1)").unwrap();
        // The program is a two-statement Sequence: the def, then the call.
        let ExprKind::Sequence(stmts) = &e.kind else { panic!("expected Sequence, got {:?}", e.kind) };
        assert_eq!(stmts.len(), 2);
        assert!(matches!(stmts[0].kind, ExprKind::LocalFunctionDefine { .. }));
        assert!(matches!(stmts[1].kind, ExprKind::LocalFunctionCall { .. }));
    }

    #[test]
    fn rejects_redefining_a_function_name_thats_already_local() {
        let err = parse_src("def f(): 1 fed; def f(): 2 fed").unwrap_err();
        assert!(err.to_string().contains("already defined"), "got: {err}");
    }

    #[test]
    fn a_bare_variable_parameter_defaults_to_the_empty_string() {
        let e = parse_src("def f(x): x fed").unwrap();
        let ExprKind::LocalFunctionDefine { params, .. } = &e.kind else { panic!("expected LocalFunctionDefine") };
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].name, "x");
        assert!(matches!(params[0].default.kind, ExprKind::Constant(ref s) if s.is_empty()));
    }

    #[test]
    fn rejects_an_unknown_function_name() {
        let err = parse_src("totally_bogus_function(1)").unwrap_err();
        assert!(err.to_string().contains("Unknown function"), "got: {err}");
    }

    #[test]
    fn rejects_a_dangling_open_paren() {
        assert!(parse_src("(1 + 2").is_err());
    }

    #[test]
    fn parses_switch_with_an_even_arg_count_including_default() {
        assert!(parse_src("switch($x, 'a', '1', 'b')").is_ok());
        // Odd count (missing default) should be rejected.
        assert!(parse_src("switch($x, 'a', '1')").is_err());
    }

    #[test]
    fn semicolon_joins_multiple_top_level_statements_into_one_sequence() {
        let e = parse_src("a=1; b=2; a").unwrap();
        let ExprKind::Sequence(stmts) = &e.kind else { panic!("expected Sequence") };
        assert_eq!(stmts.len(), 3);
    }

    #[test]
    fn parses_local_variable_assignment_and_reference() {
        let e = parse_src("x = 5").unwrap();
        assert!(matches!(e.kind, ExprKind::Assign { .. }));
    }

    #[test]
    fn parses_with_block_and_flags_a_missing_htiw_with_upstreams_own_worded_error() {
        assert!(parse_src("with $id: 'x' htiw").is_ok());
        let err = parse_src("with $id: 'x'").unwrap_err();
        // Preserved verbatim upstream copy-paste typo -- see
        // `with_expression`'s own comment.
        assert!(err.to_string().contains("'def' statement"), "got: {err}");
    }
}
