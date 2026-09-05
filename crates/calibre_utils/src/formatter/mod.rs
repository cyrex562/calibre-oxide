//! Port of `calibre.utils.formatter` (issue #513, part of the #460
//! formatter epic): the calibre template language's tokenizer,
//! recursive-descent parser, and tree-walking evaluator.
//!
//! See `docs/modules_to_port.md`'s `formatter.py` entry and issue
//! #513's own body for the full scope and disclosed narrowings.
//! Submodules mirror upstream's own internal structure (which all
//! lives in one 2142-line file) rather than the single-file layout,
//! since a faithful Rust port is itself well over a thousand lines:
//!
//! - [`lexer`]: tokenizer (`_Parser`'s `cached_lex_scanner`)
//! - [`ast`]: the `Node`/`...Node` AST types
//! - [`parser`]: the recursive-descent parser (`_Parser`)
//! - [`interp`]: the tree-walking evaluator (`_Interpreter`)
//! - [`string_functions`]: the `STRING_MANIPULATION`/`CASE_CHANGES`
//!   built-ins (issue #515) that need no book/`Cache` access
//! - [`list_functions`]: the `LIST_MANIPULATION`/`LIST_LOOKUP`
//!   built-ins (issue #516) that need no book/`Cache` access

pub mod ast;
pub mod interp;
pub mod lexer;
pub mod list_functions;
pub mod parser;
pub mod string_functions;
