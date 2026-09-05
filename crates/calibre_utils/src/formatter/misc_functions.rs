//! Port of `formatter_functions.py`'s `ITERATING_VALUES` (4),
//! `RECURSION` (2), and `OTHER`/`UNKNOWN` (7) categories (issue #519,
//! part of the #460 formatter epic), ~13 total.
//!
//! Of these, only `is_dark_mode` is a real [`super::interp::FunctionRegistry`]
//! function -- everything else in this batch needs direct interpreter
//! access (not a plain registry call) and lives as a new inlined
//! `ExprKind` shortcut instead: `eval`/`template`/`lookup` (see
//! `ast.rs`/`interp.rs`'s own doc on each), or was already inlined by
//! #513 (`assign`/`print`/`switch`/`switch_if`/`first_non_empty`) or
//! already specially handled by the parser/interpreter for local
//! function definitions (`arguments`/`globals`/`set_globals`).
//!
//! `is_dark_mode` needs `calibre.gui2.is_dark_theme`, a real Qt GUI
//! concept this headless port has no equivalent of anywhere -- always
//! returns the empty string (matching upstream's own `except Exception:
//! only_in_gui_error(...)` fallback path, which is what real calibre
//! itself does whenever there's no GUI to ask), a real, disclosed
//! absence rather than a guessed default.

use super::interp::FunctionRegistry;
use super::parser::FunctionCatalog;

pub struct MiscFunctions;

impl FunctionRegistry for MiscFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        call(name, args)
    }
}

pub struct MiscCatalog;

impl FunctionCatalog for MiscCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        arg_count(name)
    }
}

pub fn arg_count(name: &str) -> Option<Option<usize>> {
    match name {
        "is_dark_mode" => Some(Some(0)),
        _ => None,
    }
}

pub fn call(name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "is_dark_mode" => Ok(String::new()),
        _ => Err(format!("No function named {name:?} exists ({} args given)", args.len())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_dark_mode_reports_no_gui_exists() {
        assert_eq!(call("is_dark_mode", &[]).unwrap(), "");
    }

    #[test]
    fn unknown_function_is_a_real_error() {
        assert!(call("no_such_function", &[]).is_err());
    }

    #[test]
    fn catalog_reports_correct_arity() {
        assert_eq!(arg_count("is_dark_mode"), Some(Some(0)));
        assert_eq!(arg_count("no_such_function"), None);
    }
}
