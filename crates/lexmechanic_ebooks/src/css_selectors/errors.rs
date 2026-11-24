//! Port of `css_selectors.errors` (issue #451, split from #85).
//!
//! Real upstream defines a small `ValueError` subclass hierarchy:
//! `SelectorError` (common parent) -> `SelectorSyntaxError` (grammar
//! violation) / `ExpressionError` (a syntactically valid but unknown or
//! unsupported selector feature, e.g. an unrecognized pseudo-class).
//! Ported as one enum with two variants rather than a class hierarchy
//! -- nothing in this port ever needs to catch "any `SelectorError`"
//! separately from its two concrete cases, so an enum plus a `matches!`
//! check covers the same real distinction with no inheritance needed.

use std::fmt;

/// Port of `SelectorError`/`SelectorSyntaxError`/`ExpressionError`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectorError {
    /// Port of `SelectorSyntaxError`: parsing a selector that does not
    /// match the grammar.
    Syntax(String),
    /// Port of `ExpressionError`: a syntactically valid selector using
    /// an unknown or unsupported feature (e.g. a pseudo-class).
    Expression(String),
}

impl fmt::Display for SelectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SelectorError::Syntax(msg) | SelectorError::Expression(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for SelectorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syntax_and_expression_errors_carry_their_own_message() {
        let syntax = SelectorError::Syntax("Expected ident, got EOF".to_string());
        let expr = SelectorError::Expression(":foo is not supported".to_string());
        assert_eq!(syntax.to_string(), "Expected ident, got EOF");
        assert_eq!(expr.to_string(), ":foo is not supported");
        assert_ne!(syntax, expr);
    }
}
