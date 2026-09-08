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
//! - [`numeric_functions`]: the `ARITHMETIC`/`RELATIONAL`/`BOOLEAN`
//!   built-ins (issue #517) that need no book/`Cache` access
//! - [`format_functions`]: the `FORMATTING_VALUES`/`DATE_FUNCTIONS`/
//!   `URL_FUNCTIONS` built-ins (issue #518) that need no book/`Cache`
//!   access
//! - [`misc_functions`]: the one real registry function left in
//!   `ITERATING_VALUES`/`RECURSION`/`OTHER` (issue #519) -- everything
//!   else in that batch is a new inlined `ExprKind` shortcut instead

pub mod ast;
pub mod format_functions;
pub mod interp;
pub mod lexer;
pub mod list_functions;
pub mod misc_functions;
pub mod numeric_functions;
pub mod parser;
pub mod string_functions;

use interp::FunctionRegistry;
use parser::FunctionCatalog;

/// The 5 pure (no book/`Cache` access) built-in function modules,
/// combined into one [`FunctionRegistry`]/[`FunctionCatalog`] pair --
/// promoted here (issue #596) from a pattern `calibre_db::formatter_functions`
/// already had privately (`fallback_call`/`fallback_arg_count`), since
/// a second real caller (`calibre_ebooks::covers`, evaluating
/// `covers.py`'s own field/`program:` templates against a `Metadata`
/// `ValueSource` with no `Cache` in scope at all) needs the exact same
/// "just the pure functions" registry, not `calibre_db`'s Cache-backed
/// superset.
pub struct PureFunctions;

impl FunctionRegistry for PureFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        if string_functions::arg_count(name).is_some() {
            string_functions::call(name, args)
        } else if list_functions::arg_count(name).is_some() {
            list_functions::call(name, args)
        } else if numeric_functions::arg_count(name).is_some() {
            numeric_functions::call(name, args)
        } else if format_functions::arg_count(name).is_some() {
            format_functions::call(name, args)
        } else if misc_functions::arg_count(name).is_some() {
            misc_functions::call(name, args)
        } else {
            Err(format!("No function named {name:?} exists"))
        }
    }
}

pub struct PureCatalog;

impl FunctionCatalog for PureCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        string_functions::arg_count(name)
            .or_else(|| list_functions::arg_count(name))
            .or_else(|| numeric_functions::arg_count(name))
            .or_else(|| format_functions::arg_count(name))
            .or_else(|| misc_functions::arg_count(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interp::DictValueSource;
    use std::collections::HashMap;

    fn eval_program(src: &str, values: HashMap<String, String>) -> Result<String, String> {
        let tokens = lexer::scan(src).map_err(|p| format!("lex error at {p}"))?;
        let program = parser::parse(&tokens, &PureCatalog, Default::default()).map_err(|e| e.to_string())?;
        let mut globals = HashMap::new();
        interp::evaluate(&program, "", Box::new(DictValueSource::new(values)), &PureFunctions, &mut globals).map_err(|e| e.to_string())
    }

    #[test]
    fn pure_functions_resolves_a_real_program_using_multiple_modules() {
        // count (list_functions) + strcat (an inlined AST shortcut,
        // needs no registry at all) together in one real program.
        let mut values = HashMap::new();
        values.insert("tags".to_string(), "a & b & c".to_string());
        let ans = eval_program("strcat('n=', count(field('tags'), ' & '))", values).unwrap();
        assert_eq!(ans, "n=3");
    }

    #[test]
    fn pure_catalog_and_functions_agree_on_unknown_names() {
        assert!(PureCatalog.arg_count("not_a_real_function").is_none());
        assert!(PureFunctions.call("not_a_real_function", &[]).is_err());
    }
}
