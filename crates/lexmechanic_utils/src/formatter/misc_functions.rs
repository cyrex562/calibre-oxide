//! Port of `formatter_functions.py`'s `ITERATING_VALUES` (4),
//! `RECURSION` (2), `OTHER`/`UNKNOWN` (7) (issue #519), and
//! `GUI_FUNCTIONS` (4, issue #520) categories, ~17 total.
//!
//! Of the #519 batch, only `is_dark_mode` was a real
//! [`super::interp::FunctionRegistry`] function -- everything else in
//! that batch needs direct interpreter access (not a plain registry
//! call) and lives as a new inlined `ExprKind` shortcut instead:
//! `eval`/`template`/`lookup` (see `ast.rs`/`interp.rs`'s own doc on
//! each), or was already inlined by #513
//! (`assign`/`print`/`switch`/`switch_if`/`first_non_empty`) or
//! already specially handled by the parser/interpreter for local
//! function definitions (`arguments`/`globals`/`set_globals`).
//!
//! `is_dark_mode` and all 4 `GUI_FUNCTIONS` (`selected_books`,
//! `sort_book_ids`, `selected_column`, `show_dialog`) need a real Qt
//! GUI (`calibre.gui2.ui.get_gui()`, `QDialog`, etc.) that doesn't
//! exist anywhere in this headless port -- upstream's own real
//! behavior when there's no GUI is `only_in_gui_error(name)`, which
//! **raises** `ValueError('The function {name} can be used only in
//! the GUI')` (NOT a silent empty-string fallback -- an earlier draft
//! of `is_dark_mode` in #519 wrongly returned `""` here, fixed as
//! part of #520 once the real `only_in_gui_error` behavior was
//! checked directly instead of assumed). All 5 functions now report
//! that same real error unconditionally.

use super::interp::FunctionRegistry;
use super::parser::FunctionCatalog;

/// The 5 functions in this module that need a GUI this port doesn't
/// have -- kept as one list so `call`'s error and `arg_count`'s
/// registration can't drift apart.
const GUI_ONLY_FUNCTIONS: &[(&str, Option<usize>)] = &[("is_dark_mode", Some(0)), ("selected_books", None), ("sort_book_ids", None), ("selected_column", Some(0)), ("show_dialog", Some(1))];

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
    GUI_ONLY_FUNCTIONS.iter().find(|(n, _)| *n == name).map(|(_, arity)| *arity)
}

pub fn call(name: &str, _args: &[String]) -> Result<String, String> {
    if GUI_ONLY_FUNCTIONS.iter().any(|(n, _)| *n == name) {
        Err(format!("The function {name} can be used only in the GUI"))
    } else {
        Err(format!("No function named {name:?} exists"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gui_only_function_reports_the_real_only_in_gui_error() {
        for (name, arity) in GUI_ONLY_FUNCTIONS {
            let n = arity.unwrap_or(1);
            let args = vec![String::new(); n];
            let err = call(name, &args).unwrap_err();
            assert_eq!(err, format!("The function {name} can be used only in the GUI"), "for {name}");
        }
    }

    #[test]
    fn unknown_function_is_a_different_real_error() {
        let err = call("no_such_function", &[]).unwrap_err();
        assert!(!err.contains("only in the GUI"), "got: {err}");
    }

    #[test]
    fn catalog_reports_correct_arity() {
        assert_eq!(arg_count("is_dark_mode"), Some(Some(0)));
        assert_eq!(arg_count("sort_book_ids"), Some(None));
        assert_eq!(arg_count("show_dialog"), Some(Some(1)));
        assert_eq!(arg_count("no_such_function"), None);
    }
}
