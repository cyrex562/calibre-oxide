//! Port of `formatter.py`'s `_Interpreter`: a tree-walking evaluator
//! for the [`super::ast::Expr`] tree produced by [`super::parser`].
//! Every value in this language is a string (numbers/booleans are
//! stringly-typed: `"1"`/`""` for true/false, arithmetic converts
//! string<->`f64` as needed) -- matches upstream exactly, not a
//! simplification.
//!
//! # Decoupling from `calibre_db` (this is #513's whole point)
//!
//! Field access ([`ValueSource`]) and function calls
//! ([`FunctionRegistry`]) are trait-based rather than hard-wired to
//! `calibre_db::Cache`/a book-metadata object, mirroring upstream's
//! own extension points (`TemplateFormatter.get_value` is itself an
//! abstract method upstream subclasses override; `EvalFormatter` is
//! upstream's own simple-dict-backed subclass, matching this port's
//! [`DictValueSource`]). Real `calibre_db`-backed implementations of
//! both traits, and the ~125 real built-in functions, are the actual
//! substance of issue #460's other split-out sub-issues (#514-520) --
//! this module is usable standalone today (exactly as useful as
//! upstream's own `EvalFormatter`/`eval_formatter` already is for
//! non-book template contexts) and doesn't need to change shape when
//! those land.
//!
//! # Disclosed narrowings
//!
//! - `break_reporter` (a GUI template-tester debug hook upstream
//!   threads through every node) isn't ported -- no GUI exists in
//!   this port to drive it.
//! - Stored GPM/Python template calls aren't ported (see
//!   [`super::parser`]'s own doc) -- there is no
//!   [`ExprKind::StoredTemplateCall`] path here to evaluate because
//!   the parser never produces one.
//! - `print(...)`'s real side effect (writing to stdout, matching
//!   upstream's own `print(res)`) is replicated via `eprintln!` to
//!   avoid polluting a caller's real stdout output; the function's
//!   *return value* (`res[0]` or `''`) is unaffected either way.
//! - A field that resolves to Python's literal `None` (as opposed to
//!   simply not existing) has no separate representation in
//!   [`RawValue`] -- treated the same as "not found" (empty result),
//!   an edge case upstream's own code path here is itself thinly
//!   specified for (see `do_node_for`'s dead fall-through when
//!   `break_reporter` is unset and `res is None`).

use super::ast::{Expr, ExprKind, Param};
use crate::icu::strcmp;
use fancy_regex::Regex;
use std::collections::HashMap;
use std::fmt;

/// What one raw (unformatted) field value looks like -- port of the
/// real distinction upstream's `getattr(self.parent_book, name, ...)`
/// makes at runtime between a plain string, a real multi-value list
/// (tags/authors/etc), and a dict-shaped field (identifiers: `{type:
/// value}`).
#[derive(Debug, Clone)]
pub enum RawValue {
    Scalar(String),
    List(Vec<String>),
    Map(Vec<(String, String)>),
}

/// Field access -- the interpreter's only way to read book/record
/// data. See this module's own doc for why this is a trait instead of
/// a direct `calibre_db` dependency.
pub trait ValueSource {
    /// Port of `field(name)` / `$name`: the *formatted* string value
    /// of a field, or `None` if the field is unknown (a real error in
    /// the interpreter, matching upstream's `Unknown field` message).
    fn get_value(&self, name: &str) -> Option<String>;
    /// Port of `getattr(self.parent_book, name, default)` for
    /// `raw_field`/`for`/`list_count_field`/`inlist_field`: the raw
    /// underlying value. `None` means the attribute doesn't exist at
    /// all (callers fall back to upstream's own `default` argument
    /// semantics at each call site, since the fallback value differs
    /// per caller).
    fn get_raw_value(&self, name: &str) -> Option<RawValue>;
    /// Port of the `with` statement's book-context switch. Returns a
    /// new [`ValueSource`] scoped to `book_id`, or `None` if
    /// unsupported/not found. The default (no override) means `with`
    /// always fails -- correct for a source with no book concept at
    /// all, like [`DictValueSource`].
    fn with_book(&self, _book_id: i64) -> Option<Box<dyn ValueSource>> {
        None
    }
}

/// Port of `EvalFormatter`: a `ValueSource` backed by a plain
/// `name -> value` map, no book concept. Real, useful on its own --
/// upstream's own `eval_formatter`/`EvalFormatter` is used wherever a
/// template needs to run against arbitrary key-value data rather than
/// a full book object (e.g. save-to-disk path templates).
#[derive(Debug, Clone, Default)]
pub struct DictValueSource {
    pub values: HashMap<String, String>,
}

impl DictValueSource {
    pub fn new(values: HashMap<String, String>) -> Self {
        Self { values }
    }
}

impl ValueSource for DictValueSource {
    fn get_value(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned()
    }
    fn get_raw_value(&self, name: &str) -> Option<RawValue> {
        self.values.get(name).cloned().map(RawValue::Scalar)
    }
}

/// A [`ValueSource`] that borrows another one -- used by `template`
/// (issue #519) to give a nested sub-evaluation the SAME real value
/// source (e.g. the same book) as its caller without needing to move
/// or clone the caller's own `Box<dyn ValueSource>`.
struct BorrowedValueSource<'a>(&'a dyn ValueSource);

impl ValueSource for BorrowedValueSource<'_> {
    fn get_value(&self, name: &str) -> Option<String> {
        self.0.get_value(name)
    }
    fn get_raw_value(&self, name: &str) -> Option<RawValue> {
        self.0.get_raw_value(name)
    }
    fn with_book(&self, book_id: i64) -> Option<Box<dyn ValueSource>> {
        self.0.with_book(book_id)
    }
}

/// Registered (built-in or user-defined) template function calls --
/// the `Func` AST node's only evaluation path. See this module's own
/// doc for why real functions (issue #460's other sub-issues) aren't
/// part of #513.
pub trait FunctionRegistry {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String>;
}

/// A registry with no functions -- every call errors. The parser's
/// own `FunctionCatalog` already rejects unknown names before this
/// port would ever construct a `Func` node the registry doesn't
/// recognize, so this only matters if a caller mismatches its parse-
/// time catalog and its run-time registry.
pub struct EmptyFunctionRegistry;
impl FunctionRegistry for EmptyFunctionRegistry {
    fn call(&self, name: &str, _args: &[String]) -> Result<String, String> {
        Err(format!("No function named {name:?} exists"))
    }
}

#[derive(Debug, Clone)]
pub enum ControlFlow {
    Break(String),
    Continue(String),
    Return(String),
}

#[derive(Debug, Clone)]
pub enum EvalError {
    Flow(ControlFlow),
    /// Port of `_Interpreter.error`: a real evaluation error, with
    /// the offending line number.
    Msg { message: String, line: u32 },
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::Flow(ControlFlow::Break(_)) => write!(f, "break outside of for loop"),
            EvalError::Flow(ControlFlow::Continue(_)) => write!(f, "continue outside of for loop"),
            EvalError::Flow(ControlFlow::Return(_)) => write!(f, "return outside of for loop"),
            EvalError::Msg { message, line } => write!(f, "Interpreter: {message} - line number {line}"),
        }
    }
}
impl std::error::Error for EvalError {}

type EvalResult = Result<String, EvalError>;

pub struct Interpreter<'a> {
    locals: HashMap<String, String>,
    local_functions: HashMap<String, (Vec<Param>, Expr)>,
    globals: &'a mut HashMap<String, String>,
    values: Box<dyn ValueSource + 'a>,
    functions: &'a dyn FunctionRegistry,
}

/// Evaluates `program` (as produced by [`super::parser::parse`])
/// against `val` (bound to the special `$` local, matching upstream's
/// `self.locals = {'$': val}`), `values`, `functions`, and a
/// persistent `globals` map a caller keeps across template calls
/// (upstream's own `global_vars`, threaded through recursive/composite
/// template evaluation).
pub fn evaluate(program: &Expr, val: &str, values: Box<dyn ValueSource + '_>, functions: &dyn FunctionRegistry, globals: &mut HashMap<String, String>) -> Result<String, EvalError> {
    let mut interp = Interpreter { locals: HashMap::from([("$".to_string(), val.to_string())]), local_functions: HashMap::new(), globals, values, functions };
    match interp.eval(program) {
        Ok(v) => Ok(v),
        Err(EvalError::Flow(ControlFlow::Return(v))) => Ok(v),
        Err(other) => Err(other),
    }
}

pub(crate) fn float_deal_with_none(v: &str) -> Option<f64> {
    if v.is_empty() || v == "None" {
        Some(0.0)
    } else {
        v.parse::<f64>().ok()
    }
}

/// Port of `str(answer if modf(answer)[0] != 0 else int(answer))`:
/// format a computed float as a bare integer when it has no
/// fractional part, matching upstream's "numbers in this language
/// don't show a trailing `.0`" convention.
fn format_number(v: f64) -> String {
    if v.fract() != 0.0 {
        v.to_string()
    } else {
        (v as i64).to_string()
    }
}

fn regex_search_ci(pattern: &str, text: &str) -> Result<bool, String> {
    let re = Regex::new(&format!("(?i){pattern}")).map_err(|e| e.to_string())?;
    re.is_match(text).map_err(|e| e.to_string())
}

/// Shared by `eval`/`template` (issue #519). Real upstream's own
/// `TemplateFormatter.evaluate` dispatches on the (post `[[`/`]]`->
/// `{`/`}` conversion) string's OWN prefix: `"program:"` for a full
/// Template Program Mode expression (evaluated by this same
/// lexer/parser/interpreter this port already has), `"python:"` for
/// `exec()`-based templates (permanently out of scope everywhere in
/// this port, see `parser.rs`'s own doc), and otherwise the *separate*
/// old-style `{field}`/`{field:func}` shorthand-template compiler
/// (`vformat`) -- which isn't ported anywhere in this crate (the same
/// gap `string_functions::re_group`'s own narrowed grammar and
/// `lookup`'s bare-field-only key both already work around). Only the
/// `"program:"` case is supported here; anything else is a real,
/// reported error rather than a silent misinterpretation.
///
/// A plain free function, not an `Interpreter` method, so its
/// `values` argument can borrow the caller's own `self.values` field
/// without conflicting with the `&mut self.globals` reborrow this
/// needs -- `self.method(values_borrowed_from_self, ...)` would
/// require a whole-`self` mutable borrow that overlaps that immutable
/// one.
fn eval_sub_program(template: &str, line: u32, values: Box<dyn ValueSource + '_>, local_functions: &HashMap<String, (Vec<Param>, Expr)>, functions: &dyn FunctionRegistry, globals: &mut HashMap<String, String>) -> EvalResult {
    let template = template.replace("[[", "{").replace("]]", "}");
    let Some(program_text) = template.strip_prefix("program:") else {
        return Err(EvalError::Msg {
            message: "eval/template: only a 'program:'-prefixed argument (full Template Program Mode) is supported in this port -- the old-style '{field}' shorthand-template compiler isn't ported anywhere in this crate".to_string(),
            line,
        });
    };
    let tokens = super::lexer::scan(program_text).map_err(|pos| EvalError::Msg { message: format!("eval/template: lex error at byte {pos}"), line })?;
    let sub_program = super::parser::parse(&tokens, &super::parser::EmptyCatalog, local_functions.keys().cloned().collect()).map_err(|e| EvalError::Msg { message: e.to_string(), line })?;
    evaluate(&sub_program, "", values, functions, globals)
}

impl<'a> Interpreter<'a> {
    fn err<T>(&self, line: u32, message: impl Into<String>) -> Result<T, EvalError> {
        Err(EvalError::Msg { message: message.into(), line })
    }

    fn eval_block(&mut self, block: &Expr) -> EvalResult {
        self.eval(block)
    }

    /// Port of `expression_list`: run each entry, keep the last
    /// value; a Break/Continue signal is re-raised carrying that
    /// running value (Return propagates unmodified, matching upstream
    /// -- only `(BreakExecuted, ContinueExecuted)` are caught here).
    fn eval_sequence(&mut self, list: &[Expr]) -> EvalResult {
        let mut val = String::new();
        for e in list {
            match self.eval(e) {
                Ok(v) => val = v,
                Err(EvalError::Flow(ControlFlow::Break(_))) => return Err(EvalError::Flow(ControlFlow::Break(val))),
                Err(EvalError::Flow(ControlFlow::Continue(_))) => return Err(EvalError::Flow(ControlFlow::Continue(val))),
                Err(other) => return Err(other),
            }
        }
        Ok(val)
    }

    fn eval(&mut self, e: &Expr) -> EvalResult {
        let line = e.line;
        match &e.kind {
            ExprKind::Sequence(list) => self.eval_sequence(list),
            ExprKind::Constant(v) => Ok(v.clone()),
            ExprKind::Variable(name) => self.locals.get(name).cloned().ok_or_else(|| EvalError::Msg { message: format!("Unknown identifier '{name}'"), line }),
            ExprKind::Assign { name, value } => {
                let v = self.eval(value)?;
                self.locals.insert(name.clone(), v.clone());
                Ok(v)
            }

            ExprKind::Field(expr) => {
                let name = self.eval(expr)?;
                self.values.get_value(&name).ok_or_else(|| EvalError::Msg { message: format!("Unknown field '{name}'"), line })
            }
            ExprKind::RawField { expr, default } => {
                let name = self.eval(expr)?;
                match self.values.get_raw_value(&name) {
                    Some(RawValue::Scalar(s)) => Ok(s),
                    Some(RawValue::List(items)) => Ok(items.join(", ")),
                    Some(RawValue::Map(pairs)) => Ok(pairs.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(", ")),
                    None => match default {
                        Some(d) => self.eval(d),
                        None => Ok("None".to_string()),
                    },
                }
            }
            ExprKind::ListCountField(expr) => {
                let name = self.eval(expr)?;
                match self.values.get_raw_value(&name) {
                    Some(RawValue::List(items)) => Ok(items.len().to_string()),
                    Some(RawValue::Map(pairs)) => Ok(pairs.len().to_string()),
                    _ => self.err(line, format!("Field '{name}' is either not a field or not a list")),
                }
            }
            ExprKind::ListSplit { list_val, sep, id_prefix } => {
                let list_val = self.eval(list_val)?;
                let sep = self.eval(sep)?;
                let id_prefix = self.eval(id_prefix)?;
                let mut res = String::new();
                for (i, v) in list_val.split(sep.as_str()).enumerate() {
                    let v = v.trim().to_string();
                    self.locals.insert(format!("{id_prefix}_{i}"), v.clone());
                    res = v;
                }
                Ok(res)
            }

            ExprKind::FirstNonEmpty(exprs) => {
                for expr in exprs {
                    let v = self.eval(expr)?;
                    if !v.is_empty() {
                        return Ok(v);
                    }
                }
                Ok(String::new())
            }
            ExprKind::Switch(exprs) => {
                let val = self.eval(&exprs[0])?;
                let mut i = 1;
                while i + 1 < exprs.len() {
                    let pat = self.eval(&exprs[i])?;
                    if regex_search_ci(&pat, &val).map_err(|m| EvalError::Msg { message: m, line })? {
                        return self.eval(&exprs[i + 1]);
                    }
                    i += 2;
                }
                self.eval(exprs.last().unwrap())
            }
            ExprKind::SwitchIf(exprs) => {
                let mut i = 0;
                while i + 1 < exprs.len() {
                    let t = self.eval(&exprs[i])?;
                    if !t.is_empty() {
                        return self.eval(&exprs[i + 1]);
                    }
                    i += 2;
                }
                self.eval(exprs.last().unwrap())
            }
            ExprKind::Contains { value, test, matched, not_matched } => {
                let v = self.eval(value)?;
                let t = self.eval(test)?;
                if regex_search_ci(&t, &v).map_err(|m| EvalError::Msg { message: m, line })? {
                    self.eval(matched)
                } else {
                    self.eval(not_matched)
                }
            }
            ExprKind::Print(args) => {
                let mut res = Vec::new();
                for a in args {
                    res.push(self.eval(a)?);
                }
                eprintln!("{res:?}");
                Ok(res.into_iter().next().unwrap_or_default())
            }
            ExprKind::Character(expr) => {
                let key = self.eval(expr)?;
                match key.as_str() {
                    "return" => Ok("\r".to_string()),
                    "newline" => Ok("\n".to_string()),
                    "tab" => Ok("\t".to_string()),
                    "backslash" => Ok("\\".to_string()),
                    _ => self.err(line, format!("Function character: invalid character name '{key}'")),
                }
            }
            ExprKind::Strcat(exprs) => {
                let mut out = String::new();
                for e in exprs {
                    out.push_str(&self.eval(e)?);
                }
                Ok(out)
            }
            ExprKind::FString(string) => {
                let template = self.eval(string)?;
                self.eval_f_string(&template, line)
            }
            ExprKind::Eval(string) => {
                let template = self.eval(string)?;
                let values = Box::new(DictValueSource::new(self.locals.clone()));
                eval_sub_program(&template, line, values, &self.local_functions, self.functions, self.globals)
            }
            ExprKind::Template(string) => {
                let template = self.eval(string)?;
                let values = Box::new(BorrowedValueSource(self.values.as_ref()));
                eval_sub_program(&template, line, values, &self.local_functions, self.functions, self.globals)
            }
            ExprKind::Lookup { value, args } => {
                let val = self.eval(value)?;
                let mut arg_strs = Vec::with_capacity(args.len());
                for a in args {
                    arg_strs.push(self.eval(a)?);
                }
                let key = if arg_strs.len() == 2 {
                    // Backwards-compatibility 2-arg form, matching
                    // upstream's own special case.
                    if !val.is_empty() { &arg_strs[0] } else { &arg_strs[1] }
                } else {
                    if arg_strs.len() % 2 != 1 {
                        return self.err(line, "lookup requires either 2 or an odd number of arguments".to_string());
                    }
                    let mut chosen = None;
                    let mut i = 0;
                    while i < arg_strs.len() {
                        if i + 1 >= arg_strs.len() {
                            chosen = Some(&arg_strs[i]);
                            break;
                        }
                        if regex_search_ci(&arg_strs[i], &val).map_err(|m| EvalError::Msg { message: m, line })? {
                            chosen = Some(&arg_strs[i + 1]);
                            break;
                        }
                        i += 2;
                    }
                    chosen.expect("the odd-length check above guarantees a trailing else_key is always reached")
                };
                let key = key.trim();
                if key.contains(':') {
                    return self.err(line, format!("lookup: the field key '{key}' uses the old-style '{{field:func}}' shorthand chain, which isn't supported in this port -- only a bare field name is"));
                }
                self.values.get_value(key).ok_or_else(|| EvalError::Msg { message: format!("Unknown field '{key}'"), line })
            }

            ExprKind::If { condition, then_part, else_part } => {
                let test = self.eval(condition)?;
                if !test.is_empty() {
                    self.eval_block(then_part)
                } else if let Some(else_part) = else_part {
                    self.eval_block(else_part)
                } else {
                    Ok(String::new())
                }
            }
            ExprKind::For { variable, list_expr, separator, block } => self.eval_for(line, variable, list_expr, separator.as_deref(), block),
            ExprKind::Range { variable, start, stop, step, limit, block } => self.eval_range(line, variable, start, stop, step, limit.as_deref(), block),
            ExprKind::With { book_id, block } => self.eval_with(line, book_id, block),

            ExprKind::Break => Err(EvalError::Flow(ControlFlow::Break(String::new()))),
            ExprKind::Continue => Err(EvalError::Flow(ControlFlow::Continue(String::new()))),
            ExprKind::Return(expr) => {
                let v = self.eval(expr)?;
                Err(EvalError::Flow(ControlFlow::Return(v)))
            }

            ExprKind::Func { name, args } => {
                let mut vals = Vec::with_capacity(args.len());
                for a in args {
                    vals.push(self.eval(a)?);
                }
                self.functions.call(name, &vals).map_err(|m| EvalError::Msg { message: format!("Error in function {name} :: {m}"), line })
            }
            ExprKind::StoredTemplateCall { .. } => self.err(line, "stored templates are not supported by this port"),

            ExprKind::LocalFunctionDefine { name, params, block } => {
                self.local_functions.insert(name.clone(), (params.clone(), (**block).clone()));
                Ok(String::new())
            }
            ExprKind::LocalFunctionCall { name, args } => self.eval_local_call(line, name, args),

            ExprKind::Arguments(params) => {
                for (i, p) in params.iter().enumerate() {
                    let key = format!("*arg_{i}");
                    let v = match self.locals.get(&key) {
                        Some(v) => v.clone(),
                        None => self.eval(&p.default)?,
                    };
                    self.locals.insert(p.name.clone(), v);
                }
                Ok(String::new())
            }
            ExprKind::Globals(params) => {
                let mut res = String::new();
                for p in params {
                    let v = match self.globals.get(&p.name) {
                        Some(v) => v.clone(),
                        None => self.eval(&p.default)?,
                    };
                    self.locals.insert(p.name.clone(), v.clone());
                    res = v;
                }
                Ok(res)
            }
            ExprKind::SetGlobals(params) => {
                let mut res = String::new();
                for p in params {
                    let v = match self.locals.get(&p.name) {
                        Some(v) => v.clone(),
                        None => self.eval(&p.default)?,
                    };
                    self.globals.insert(p.name.clone(), v.clone());
                    res = v;
                }
                Ok(res)
            }

            ExprKind::StringCompare { op, left, right } => self.eval_string_compare(line, op, left, right),
            ExprKind::NumericCompare { op, left, right } => self.eval_numeric_compare(line, op, left, right),
            ExprKind::LogopBinary { op, left, right } => {
                let res = match op.as_str() {
                    "and" => !self.eval(left)?.is_empty() && !self.eval(right)?.is_empty(),
                    "or" => !self.eval(left)?.is_empty() || !self.eval(right)?.is_empty(),
                    _ => return self.err(line, format!("Unknown logical operator '{op}'")),
                };
                Ok(if res { "1".to_string() } else { String::new() })
            }
            ExprKind::LogopUnary { expr } => {
                let v = self.eval(expr)?;
                Ok(if v.is_empty() { "1".to_string() } else { String::new() })
            }
            ExprKind::NumericBinary { op, left, right } => {
                let l = self.eval_float(left, line)?;
                let r = self.eval_float(right, line)?;
                let res = match op.as_str() {
                    "+" => l + r,
                    "-" => l - r,
                    "*" => l * r,
                    "/" => {
                        if r == 0.0 {
                            return self.err(line, "Error during operator evaluation: division by zero");
                        }
                        l / r
                    }
                    _ => return self.err(line, format!("Unknown arithmetic operator '{op}'")),
                };
                Ok(format_number(res))
            }
            ExprKind::NumericUnary { negate, expr } => {
                let v = self.eval(expr)?;
                let f: f64 = v.parse().map_err(|_| EvalError::Msg { message: format!("Error during operator evaluation: operator '{}'", if *negate { "-" } else { "+" }), line })?;
                Ok(format_number(if *negate { -f } else { f }))
            }
            ExprKind::StringBinary { left, right } => {
                let l = self.eval(left)?;
                let r = self.eval(right)?;
                Ok(l + &r)
            }
        }
    }

    fn eval_float(&mut self, e: &Expr, line: u32) -> Result<f64, EvalError> {
        let v = self.eval(e)?;
        float_deal_with_none(&v).ok_or_else(|| EvalError::Msg { message: "Value used in numeric expression is not a number".to_string(), line })
    }

    fn eval_string_compare(&mut self, line: u32, op: &str, left: &Expr, right: &Expr) -> EvalResult {
        let l = self.eval(left)?;
        let r = self.eval(right)?;
        let truthy = match op {
            "==" => strcmp(&l, &r) == std::cmp::Ordering::Equal,
            "!=" => strcmp(&l, &r) != std::cmp::Ordering::Equal,
            "<" => strcmp(&l, &r) == std::cmp::Ordering::Less,
            "<=" => strcmp(&l, &r) != std::cmp::Ordering::Greater,
            ">" => strcmp(&l, &r) == std::cmp::Ordering::Greater,
            ">=" => strcmp(&l, &r) != std::cmp::Ordering::Less,
            "in" => regex_search_ci(&l, &r).map_err(|m| EvalError::Msg { message: m, line })?,
            "inlist" => r.split(',').map(str::trim).filter(|s| !s.is_empty()).any(|item| regex_search_ci(&l, item).unwrap_or(false)),
            "inlist_field" => return self.eval_inlist_field(line, &l, &r),
            _ => return self.err(line, format!("Unknown string operator '{op}'")),
        };
        Ok(if truthy { "1".to_string() } else { String::new() })
    }

    fn eval_inlist_field(&mut self, line: u32, pattern: &str, field_name: &str) -> EvalResult {
        let raw = self.values.get_raw_value(field_name);
        let found = match raw {
            Some(RawValue::List(items)) => items.iter().any(|v| regex_search_ci(pattern, v).unwrap_or(false)),
            Some(RawValue::Map(pairs)) => pairs.iter().any(|(k, v)| regex_search_ci(pattern, &format!("{k}:{v}")).unwrap_or(false)),
            Some(RawValue::Scalar(_)) | None => return self.err(line, format!("Field '{field_name}' is either not a field or not a list")),
        };
        Ok(if found { "1".to_string() } else { String::new() })
    }

    fn eval_numeric_compare(&mut self, line: u32, op: &str, left: &Expr, right: &Expr) -> EvalResult {
        let l = self.eval_float(left, line)?;
        let r = self.eval_float(right, line)?;
        let truthy = match op {
            "==#" => l == r,
            "!=#" => l != r,
            "<#" => l < r,
            "<=#" => l <= r,
            ">#" => l > r,
            ">=#" => l >= r,
            _ => return self.err(line, format!("Unknown numeric operator '{op}'")),
        };
        Ok(if truthy { "1".to_string() } else { String::new() })
    }

    fn eval_for(&mut self, line: u32, variable: &str, list_expr: &Expr, separator: Option<&Expr>, block: &Expr) -> EvalResult {
        let sep = match separator {
            Some(e) => self.eval(e)?,
            None => ",".to_string(),
        };
        if sep.is_empty() {
            return self.err(line, "'for': separator must not be empty");
        }
        let name = self.eval(list_expr)?;
        let items: Vec<String> = match self.values.get_raw_value(&name) {
            Some(RawValue::List(items)) => items,
            Some(RawValue::Map(pairs)) => pairs.into_iter().map(|(k, v)| format!("{k}:{v}")).collect(),
            Some(RawValue::Scalar(s)) => split_trim_nonempty(&s, &sep),
            None => split_trim_nonempty(&name, &sep),
        };
        let mut ret = String::new();
        for item in items {
            self.locals.insert(variable.to_string(), item);
            match self.eval(block) {
                Ok(v) => ret = v,
                Err(EvalError::Flow(ControlFlow::Continue(v))) => ret = v,
                Err(EvalError::Flow(ControlFlow::Break(v))) => {
                    ret = v;
                    break;
                }
                Err(other) => return Err(other),
            }
        }
        Ok(ret)
    }

    fn eval_range(&mut self, line: u32, variable: &str, start: &Expr, stop: &Expr, step: &Expr, limit: Option<&Expr>, block: &Expr) -> EvalResult {
        let start_val = self.eval_float(start, line)? as i64;
        let stop_val = self.eval_float(stop, line)? as i64;
        let step_val = self.eval_float(step, line)? as i64;
        let limit_val = match limit {
            Some(e) => self.eval_float(e, line)? as i64,
            None => 1000,
        };
        if step_val == 0 {
            return self.err(line, "'for': step must not be zero");
        }
        let count = range_len(start_val, stop_val, step_val);
        if count > limit_val {
            return self.err(line, format!("'for': the range length ({count}) is larger than the limit ({limit_val})"));
        }
        let mut ret = String::new();
        let mut x = start_val;
        loop {
            if step_val > 0 && x >= stop_val {
                break;
            }
            if step_val < 0 && x <= stop_val {
                break;
            }
            self.locals.insert(variable.to_string(), x.to_string());
            match self.eval(block) {
                Ok(v) => ret = v,
                Err(EvalError::Flow(ControlFlow::Continue(v))) => ret = v,
                Err(EvalError::Flow(ControlFlow::Break(v))) => {
                    ret = v;
                    break;
                }
                Err(other) => return Err(other),
            }
            x += step_val;
        }
        Ok(ret)
    }

    fn eval_with(&mut self, line: u32, book_id: &Expr, block: &Expr) -> EvalResult {
        let id_str = self.eval(book_id)?;
        let id: i64 = id_str.parse().map_err(|_| EvalError::Msg { message: format!("'with': book id '{id_str}' is not an integer"), line })?;
        let Some(new_values) = self.values.with_book(id) else {
            return self.err(line, format!("'with': unknown book id {id}"));
        };
        let saved = std::mem::replace(&mut self.values, new_values);
        let result = self.eval(block);
        self.values = saved;
        result
    }

    fn eval_local_call(&mut self, line: u32, name: &str, args: &[Expr]) -> EvalResult {
        let Some((params, block)) = self.local_functions.get(name).cloned() else {
            return self.err(line, format!("Unknown local function '{name}'"));
        };
        if args.len() > params.len() {
            return self.err(line, format!("Function {name}: argument count mismatch -- {} given, at most {} required", args.len(), params.len()));
        }
        let mut new_locals = HashMap::new();
        for (i, p) in params.iter().enumerate() {
            let v = if i < args.len() { self.eval(&args[i])? } else { self.eval(&p.default)? };
            new_locals.insert(p.name.clone(), v);
        }
        let saved = std::mem::replace(&mut self.locals, new_locals);
        let result = self.eval(&block);
        self.locals = saved;
        match result {
            Ok(v) => Ok(v),
            Err(EvalError::Flow(ControlFlow::Return(v))) => Ok(v),
            Err(other) => Err(other),
        }
    }

    /// Port of `do_node_f_string`: re-parses and evaluates each
    /// `{...}` span in `template` as its own sub-program, replacing it
    /// with the resulting value.
    fn eval_f_string(&mut self, template: &str, line: u32) -> EvalResult {
        let re = Regex::new(r"(?s)\{.*?\}").unwrap();
        let mut out = String::new();
        let mut last = 0;
        for m in re.find_iter(template) {
            let m = m.map_err(|e| EvalError::Msg { message: e.to_string(), line })?;
            out.push_str(&template[last..m.start()]);
            let inner = &m.as_str()[1..m.as_str().len() - 1];
            let tokens = super::lexer::scan(inner).map_err(|pos| EvalError::Msg { message: format!("f_string: lex error at byte {pos}"), line })?;
            let sub_program = super::parser::parse(&tokens, &super::parser::EmptyCatalog, self.local_functions.keys().cloned().collect()).map_err(|e| EvalError::Msg { message: e.to_string(), line })?;
            out.push_str(&self.eval(&sub_program)?);
            last = m.end();
        }
        out.push_str(&template[last..]);
        Ok(out)
    }
}

fn split_trim_nonempty(s: &str, sep: &str) -> Vec<String> {
    s.split(sep).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn range_len(start: i64, stop: i64, step: i64) -> i64 {
    if step > 0 {
        if stop > start {
            (stop - start + step - 1) / step
        } else {
            0
        }
    } else if stop < start {
        (start - stop + (-step) - 1) / (-step)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::parser::{parse, EmptyCatalog};

    fn run_with(src: &str, values: HashMap<String, String>) -> Result<String, String> {
        let tokens = super::super::lexer::scan(src).map_err(|p| format!("lex error at {p}"))?;
        let program = parse(&tokens, &EmptyCatalog, Default::default()).map_err(|e| e.to_string())?;
        let mut globals = HashMap::new();
        evaluate(&program, "", Box::new(DictValueSource::new(values)), &EmptyFunctionRegistry, &mut globals).map_err(|e| e.to_string())
    }

    fn run(src: &str) -> Result<String, String> {
        run_with(src, HashMap::new())
    }

    #[test]
    fn evaluates_arithmetic_with_correct_precedence() {
        assert_eq!(run("1 + 2 * 3").unwrap(), "7");
        assert_eq!(run("(1 + 2) * 3").unwrap(), "9");
        assert_eq!(run("10 / 4").unwrap(), "2.5");
        assert_eq!(run("10 / 5").unwrap(), "2", "an exact integer result should not show a trailing .0");
    }

    #[test]
    fn division_by_zero_is_a_real_error() {
        assert!(run("1 / 0").is_err());
    }

    #[test]
    fn unary_minus_and_plus_work() {
        assert_eq!(run("-5 + 3").unwrap(), "-2");
        assert_eq!(run("- -5").unwrap(), "5");
    }

    #[test]
    fn if_then_elif_else_fi_picks_the_right_branch() {
        assert_eq!(run("if 1 then 'yes' else 'no' fi").unwrap(), "yes");
        assert_eq!(run("if '' then 'yes' else 'no' fi").unwrap(), "no");
        assert_eq!(run("if '' then 'a' elif 1 then 'b' else 'c' fi").unwrap(), "b");
        assert_eq!(run("if '' then 'a' elif '' then 'b' else 'c' fi").unwrap(), "c");
    }

    #[test]
    fn string_comparison_uses_real_icu_collation() {
        assert_eq!(run("if 'apple' < 'Banana' then '1' else '0' fi").unwrap(), "1");
        assert_eq!(run("if 'a' == 'a' then '1' else '0' fi").unwrap(), "1");
    }

    #[test]
    fn numeric_comparison_operators() {
        assert_eq!(run("if 3 >#2 then 'y' else 'n' fi").unwrap(), "y");
        assert_eq!(run("if 3 <#2 then 'y' else 'n' fi").unwrap(), "n");
    }

    #[test]
    fn logical_and_or_not() {
        assert_eq!(run("if 1 && 1 then 'y' else 'n' fi").unwrap(), "y");
        assert_eq!(run("if '' && 1 then 'y' else 'n' fi").unwrap(), "n");
        assert_eq!(run("if '' || 1 then 'y' else 'n' fi").unwrap(), "y");
        assert_eq!(run("if !'' then 'y' else 'n' fi").unwrap(), "y");
    }

    #[test]
    fn string_concatenation_operator() {
        assert_eq!(run("'a' & 'b' & 'c'").unwrap(), "abc");
    }

    #[test]
    fn variable_assignment_and_reference() {
        assert_eq!(run("x = 'hello'; x & ' world'").unwrap(), "hello world");
    }

    #[test]
    fn for_in_range_iterates_and_break_stops_early() {
        assert_eq!(run("s=''; for i in range(0, 5): s = s & i rof; s").unwrap(), "01234");
        assert_eq!(run("s=''; for i in range(0, 10): if i ==# 3 then break fi; s = s & i rof; s").unwrap(), "012");
    }

    #[test]
    fn for_in_range_respects_step_and_negative_direction() {
        assert_eq!(run("s=''; for i in range(10, 0, -2): s = s & i & ',' rof; s").unwrap(), "10,8,6,4,2,");
    }

    #[test]
    fn for_in_range_rejects_exceeding_the_limit() {
        assert!(run("for i in range(0, 100000, 1, 10): i rof").is_err());
    }

    #[test]
    fn for_in_list_iterates_a_raw_multi_value_field() {
        let mut values = HashMap::new();
        values.insert("tags".to_string(), "should not be used".to_string());
        let result = run_with("s=''; for t in 'tags': s = s & t & '|' rof; s", values);
        // DictValueSource only has Scalar values, so 'tags' resolves
        // to its scalar string, comma-split by the default separator
        // (there's only one item since there's no comma).
        assert_eq!(result.unwrap(), "should not be used|");
    }

    #[test]
    fn for_in_list_splits_on_a_custom_separator() {
        let mut values = HashMap::new();
        values.insert("tags".to_string(), "a;b;c".to_string());
        let result = run_with("s=''; for t in 'tags' separator ';': s = s & t & ',' rof; s", values);
        assert_eq!(result.unwrap(), "a,b,c,");
    }

    #[test]
    fn continue_skips_to_the_next_iteration() {
        assert_eq!(run("s=''; for i in range(0, 5): if i ==# 2 then continue fi; s = s & i rof; s").unwrap(), "0134");
    }

    #[test]
    fn local_function_define_and_call_with_default_arguments() {
        assert_eq!(run("def double(x): x + x fed; double(21)").unwrap(), "42");
        assert_eq!(run("def greet(name='world'): 'hi ' & name fed; greet()").unwrap(), "hi world");
        assert_eq!(run("def greet(name='world'): 'hi ' & name fed; greet('bob')").unwrap(), "hi bob");
    }

    #[test]
    fn local_function_return_stops_early() {
        assert_eq!(run("def f(x): if x ==# 1 then return 'one' fi; 'other' fed; f(1)").unwrap(), "one");
        assert_eq!(run("def f(x): if x ==# 1 then return 'one' fi; 'other' fed; f(2)").unwrap(), "other");
    }

    #[test]
    fn top_level_return_ends_evaluation_with_that_value() {
        assert_eq!(run("return 'early'; 'never reached'").unwrap(), "early");
    }

    #[test]
    fn first_non_empty_switch_and_switch_if() {
        assert_eq!(run("first_non_empty('', '', 'third')").unwrap(), "third");
        assert_eq!(run("switch('hello', 'ell', 'matched', 'nope')").unwrap(), "matched");
        assert_eq!(run("switch('xyz', 'ell', 'matched', 'nope')").unwrap(), "nope");
        assert_eq!(run("switch_if('', 'a', 1, 'b', 'default')").unwrap(), "b");
    }

    #[test]
    fn contains_dispatches_on_a_regex_match() {
        assert_eq!(run("contains('hello world', 'wor', 'yes', 'no')").unwrap(), "yes");
        assert_eq!(run("contains('hello', 'xyz', 'yes', 'no')").unwrap(), "no");
    }

    #[test]
    fn character_returns_the_named_control_character() {
        assert_eq!(run("character('newline')").unwrap(), "\n");
        assert_eq!(run("character('tab')").unwrap(), "\t");
        assert!(run("character('bogus')").is_err());
    }

    #[test]
    fn field_and_raw_field_shorthand() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "My Book".to_string());
        assert_eq!(run_with("$title", values.clone()).unwrap(), "My Book");
        assert_eq!(run_with("$$title", values).unwrap(), "My Book");
        assert!(run("$unknown_field").is_err());
    }

    #[test]
    fn f_string_evaluates_embedded_braces() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "Dune".to_string());
        let result = run_with("f_string('Book: {$title}!')", values);
        assert_eq!(result.unwrap(), "Book: Dune!");
    }

    #[test]
    fn globals_persist_across_calls_via_the_shared_map() {
        let tokens1 = super::super::lexer::scan("set_globals(counter=1)").unwrap();
        let program1 = parse(&tokens1, &EmptyCatalog, Default::default()).unwrap();
        let tokens2 = super::super::lexer::scan("globals(counter=0)").unwrap();
        let program2 = parse(&tokens2, &EmptyCatalog, Default::default()).unwrap();
        let mut globals = HashMap::new();
        evaluate(&program1, "", Box::new(DictValueSource::default()), &EmptyFunctionRegistry, &mut globals).unwrap();
        let result = evaluate(&program2, "", Box::new(DictValueSource::default()), &EmptyFunctionRegistry, &mut globals).unwrap();
        assert_eq!(result, "1");
    }

    #[test]
    fn with_fails_gracefully_against_a_source_with_no_book_concept() {
        assert!(run("with 1: 'x' htiw").is_err());
    }

    #[test]
    fn list_count_field_counts_a_raw_list() {
        struct ListSource;
        impl ValueSource for ListSource {
            fn get_value(&self, _name: &str) -> Option<String> {
                None
            }
            fn get_raw_value(&self, name: &str) -> Option<RawValue> {
                if name == "tags" {
                    Some(RawValue::List(vec!["a".into(), "b".into(), "c".into()]))
                } else {
                    None
                }
            }
        }
        let tokens = super::super::lexer::scan("list_count_field('tags')").unwrap();
        let program = parse(&tokens, &EmptyCatalog, Default::default()).unwrap();
        let mut globals = HashMap::new();
        let result = evaluate(&program, "", Box::new(ListSource), &EmptyFunctionRegistry, &mut globals).unwrap();
        assert_eq!(result, "3");
    }

    #[test]
    fn list_split_assigns_indexed_locals_and_returns_the_last_value() {
        assert_eq!(run("list_split('one:two:foo', ':', 'var'); var_0 & ' ' & var_1 & ' ' & var_2").unwrap(), "one two foo");
    }

    #[test]
    fn eval_uses_current_locals_as_its_field_source_not_the_real_book() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "Real Book Title".to_string());
        // `$x` inside eval() should resolve `x` against eval's own
        // field source (the caller's locals), not against the real
        // book's fields at all.
        assert_eq!(run_with("x = 'hello'; eval('program: $x')", values).unwrap(), "hello");
    }

    #[test]
    fn eval_cannot_see_the_real_books_fields() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "Real Book Title".to_string());
        assert!(run_with("eval('program: $title')", values).is_err(), "title isn't one of eval's own locals, so its field lookup should fail");
    }

    #[test]
    fn eval_without_a_program_prefix_is_a_real_reported_error() {
        assert!(run("eval('{x}')").is_err(), "the old-style shorthand-template compiler isn't ported, so this must error rather than silently misinterpret");
    }

    #[test]
    fn template_shares_the_real_value_source_but_not_locals() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "Real Book Title".to_string());
        assert_eq!(run_with("template('program: $title')", values).unwrap(), "Real Book Title");
    }

    #[test]
    fn template_gets_a_fresh_locals_scope() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "T".to_string());
        assert!(run_with("x = 'outer'; template('program: x')", values).is_err(), "template()'s sub-program has its own fresh locals, unrelated to the caller's");
    }

    #[test]
    fn lookup_picks_a_field_by_regex_match_against_the_value() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "The Title".to_string());
        values.insert("author_sort".to_string(), "Author, A".to_string());
        assert_eq!(run_with("lookup('Fiction', '^Fic', 'title', 'author_sort')", values).unwrap(), "The Title");
    }

    #[test]
    fn lookup_falls_back_to_the_else_key_and_supports_the_2_arg_form() {
        let mut values = HashMap::new();
        values.insert("title".to_string(), "T".to_string());
        values.insert("author_sort".to_string(), "A".to_string());
        assert_eq!(run_with("lookup('nomatch', '^Fic', 'title', 'author_sort')", values.clone()).unwrap(), "A");
        assert_eq!(run_with("lookup('', 'title', 'author_sort')", values).unwrap(), "A", "2-arg backwards-compat form: empty value picks the second key");
    }

    #[test]
    fn unknown_function_call_is_rejected_at_parse_time_not_at_eval_time() {
        // The empty catalog rejects the call before evaluation ever
        // starts -- confirms parse-time and eval-time function
        // knowledge stay in sync for this test harness.
        assert!(run("totally_bogus(1, 2)").is_err());
    }
}
