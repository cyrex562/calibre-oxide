//! Port of `formatter.py`'s `Node` class hierarchy (~35 `NODE_*`
//! kinds) as one Rust enum.
//!
//! # A real structural simplification, not a narrowing
//!
//! Upstream's interpreter dispatches on `isinstance(prog, list)`
//! *everywhere* an expression is evaluated (`_Interpreter.expr`):
//! parenthesized grouping `(a; b; c)` and a function-call argument
//! slot both parse to a plain Python `list` of `Node`s rather than a
//! dedicated node type, and `self.expr()` treats a bare `Node` and a
//! `list[Node]` (running each in order, keeping the last value --
//! same semantic as a C comma operator) as interchangeable at every
//! call site. This port makes that mechanism an explicit AST variant,
//! [`ExprKind::Sequence`], instead of leaning on a dynamic-typing
//! trick -- every place upstream could hand `self.expr()` either a
//! bare node or a list hands this port a `Box<Expr>` uniformly (a
//! single expression sequence-wraps to `Sequence(vec![one])` when
//! grouping parens are used, or when a function argument slot -- also
//! a real `;`-separated expression_list upstream -- has more than one
//! entry). The interpreter's `Sequence` case is exactly upstream's
//! `expression_list`: evaluate each entry, keep the last value.

#[derive(Debug, Clone)]
pub struct Expr {
    pub line: u32,
    pub kind: ExprKind,
}

impl Expr {
    pub fn new(line: u32, kind: ExprKind) -> Self {
        Self { line, kind }
    }
}

/// One parameter in a local function definition, or one entry in
/// `arguments()`/`globals()`/`set_globals()` -- port of upstream
/// reusing `AssignNode(name, default_expr)` for all of these (a bare
/// variable name parses to an assignment to `ConstantNode('')`).
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub default: Box<Expr>,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    /// `expression_list()`'s own result when used as a value position
    /// (grouping parens, function-argument slots) -- see this
    /// module's own doc.
    Sequence(Vec<Expr>),

    Constant(String),
    /// Reading a local variable (`VariableNode`).
    Variable(String),
    /// `name = expr` (`AssignNode`).
    Assign { name: String, value: Box<Expr> },

    /// `field(name)` / `$name` shorthand.
    Field(Box<Expr>),
    /// `raw_field(name[, default])` / `$$name` shorthand.
    RawField { expr: Box<Expr>, default: Option<Box<Expr>> },
    FirstNonEmpty(Vec<Expr>),
    /// `switch(val, pat1, res1, ..., default)`.
    Switch(Vec<Expr>),
    /// `switch_if(test1, res1, ..., default)`.
    SwitchIf(Vec<Expr>),
    Contains { value: Box<Expr>, test: Box<Expr>, matched: Box<Expr>, not_matched: Box<Expr> },
    Print(Vec<Expr>),
    Character(Box<Expr>),
    Strcat(Vec<Expr>),
    ListCountField(Box<Expr>),
    /// `list_split(list_val, sep, id_prefix)` (issue #516) -- splits
    /// `list_val` and assigns each piece to a local variable named
    /// `id_prefix_N`, so (like `assign`) it needs direct mutable
    /// access to the interpreter's own locals map and can't be a
    /// plain `FunctionRegistry` call.
    ListSplit { list_val: Box<Expr>, sep: Box<Expr>, id_prefix: Box<Expr> },
    /// `f_string(...)`: an embedded-`{...}`-template string, each
    /// `{...}` re-parsed and evaluated as its own sub-program.
    FString(Box<Expr>),

    If { condition: Box<Expr>, then_part: Box<Expr>, else_part: Option<Box<Expr>> },
    For { variable: String, list_expr: Box<Expr>, separator: Option<Box<Expr>>, block: Box<Expr> },
    Range { variable: String, start: Box<Expr>, stop: Box<Expr>, step: Box<Expr>, limit: Option<Box<Expr>>, block: Box<Expr> },
    /// `with book_id: ... htiw` -- temporarily switches the
    /// evaluation context to a different book.
    With { book_id: Box<Expr>, block: Box<Expr> },

    Break,
    Continue,
    Return(Box<Expr>),

    /// A call to a registered (built-in or user-defined non-GPM)
    /// template function.
    Func { name: String, args: Vec<Expr> },
    /// A call to a stored GPM/Python template, resolved by name at
    /// evaluation time (upstream stashes a direct reference to the
    /// definition object on the node itself; this port resolves by
    /// name through the interpreter's function registry instead, to
    /// keep the AST free of any registry-specific type).
    StoredTemplateCall { name: String, args: Vec<Expr> },
    LocalFunctionDefine { name: String, params: Vec<Param>, block: Box<Expr> },
    LocalFunctionCall { name: String, args: Vec<Expr> },
    /// `arguments(name[=default], ...)`: binds the current stored-
    /// template call's positional arguments to local variable names.
    Arguments(Vec<Param>),
    /// `globals(name[=default], ...)`: reads (with fallback) from the
    /// template-formatter-wide global variable map into locals.
    Globals(Vec<Param>),
    /// `set_globals(name[=default], ...)`: writes locals into the
    /// global variable map.
    SetGlobals(Vec<Param>),

    /// `==`/`!=`/`<`/`<=`/`>`/`>=`/`in`/`inlist`/`inlist_field`.
    StringCompare { op: String, left: Box<Expr>, right: Box<Expr> },
    /// `==#`/`!=#`/`<#`/`<=#`/`>#`/`>=#`.
    NumericCompare { op: String, left: Box<Expr>, right: Box<Expr> },
    /// `&&`/`||`.
    LogopBinary { op: String, left: Box<Expr>, right: Box<Expr> },
    /// `!` (logical not).
    LogopUnary { expr: Box<Expr> },
    /// `+`/`-`/`*`/`/` (numeric).
    NumericBinary { op: String, left: Box<Expr>, right: Box<Expr> },
    /// Unary `+`/`-` (numeric); `negate` is `true` for `-`.
    NumericUnary { negate: bool, expr: Box<Expr> },
    /// `&` (string concatenation).
    StringBinary { left: Box<Expr>, right: Box<Expr> },
}
