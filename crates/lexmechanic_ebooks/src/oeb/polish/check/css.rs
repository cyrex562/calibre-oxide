//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/css.py`.
//!
//! # Gap: running stylelint
//!
//! Python's `check_css` doesn't lint CSS itself -- it drives a headless
//! `QWebEnginePage` (`Worker`/`Pool`/`create_profile`) that has a bundled
//! JavaScript CSS linter (`stylelint`, loaded via `stylelint_js`/
//! `stylelint-bundle.min.js`) injected into it, calls
//! `window.check_css(source)` inside that page, and reads back a JSON
//! result. The actual linting *logic* -- every rule that decides
//! something is wrong with a piece of CSS -- lives entirely inside that
//! external JS bundle, not in Python. This is fundamentally a
//! Qt-WebEngine + a Node-ecosystem linter bundled as JS: there is no
//! local equivalent to build against in this Rust workspace (no
//! embedded JS engine, no native Rust CSS linter crate in the
//! dependency graph), and unlike issue #37's `webview.py` there's no
//! narrow "inject a trait for the missing part" shape here -- the thing
//! that's missing *is* the entire rule engine. [`Worker`]/[`Pool`]/
//! [`create_job`]/[`check_css`]/[`stylelint_js`] are `todo!()`
//! accordingly; [`create_job`]'s own logic (wrapping a `style=""`
//! declaration in a synthetic rule, blank-line offsetting) is pure
//! string manipulation with no such dependency and is ported for real.
//!
//! What *is* real: [`CSSParseError`]/[`CSSError`]/[`CSSWarning`] (the
//! `BaseError` subclasses `check_css` would construct results as) and
//! [`message_to_error`]'s pure mapping logic -- given a stylelint
//! message (once you have one), decide which of the three error kinds
//! it is, format its title/help text, and mark it `FIXABLE_CSS_ERROR`
//! when the matching rule's metadata says so. None of that needs
//! stylelint itself to be real.

use std::collections::HashMap;

use super::base::{CheckError, Level};

/// Port of the per-rule metadata `message_to_error` reads out of
/// `rule_metadata` (`{rule_id: {'url': ..., 'fixable': ...}}` in
/// Python's JSON).
#[derive(Debug, Clone, Default)]
pub struct RuleMetadata {
    pub url: Option<String>,
    pub fixable: bool,
}

/// Port of one message in stylelint's `results[].warnings` array --
/// the shape [`message_to_error`] maps to a [`CheckError`]. Field types
/// mirror Python's post-`as_int_or_none` values directly (rather than a
/// raw JSON value that then needs coercing) since nothing in this port
/// produces real stylelint JSON to deserialize -- see the module docs.
#[derive(Debug, Clone, Default)]
pub struct StylelintMessage {
    pub rule: Option<String>,
    /// `"error"` or `"warning"` (any other value, including absent,
    /// means "warning" -- matches Python's `message.get('severity') ==
    /// 'error'` equality test).
    pub severity: Option<String>,
    pub text: Option<String>,
    pub line: Option<i64>,
    pub column: Option<i64>,
}

/// Port of `CSSParseError`.
pub fn css_parse_error(title: &str, name: &str, line: Option<u32>, col: Option<u32>) -> CheckError {
    CheckError::new("CSSParseError", title.to_string(), name)
        .at(line, col)
        .with_level(Level::Error)
        .parsing_error()
}

/// Port of `CSSError`.
pub fn css_error(title: &str, name: &str, line: Option<u32>, col: Option<u32>) -> CheckError {
    CheckError::new("CSSError", title.to_string(), name)
        .at(line, col)
        .with_level(Level::Error)
}

/// Port of `CSSWarning`.
pub fn css_warning(title: &str, name: &str, line: Option<u32>, col: Option<u32>) -> CheckError {
    CheckError::new("CSSWarning", title.to_string(), name)
        .at(line, col)
        .with_level(Level::Warn)
}

/// Port of `message_to_error`. Real: given a stylelint message (however
/// it was obtained), this decides which error kind it is and builds the
/// full `CheckError`, including the `FIXABLE_CSS_ERROR` flag and the
/// "See detailed description" / "will be automatically fixed" help-text
/// additions. Returns `None` for the one message Python explicitly
/// suppresses (a `property-no-unknown` complaint about `panose-1`,
/// which calibre's own conversion pipeline generates and which is
/// actually valid in CSS 2.1).
pub fn message_to_error(
    message: &StylelintMessage,
    name: &str,
    line_offset: i64,
    rule_metadata: &HashMap<String, RuleMetadata>,
) -> Option<CheckError> {
    let rule_id = message.rule.as_deref();
    if rule_id == Some("property-no-unknown")
        && message.text.as_deref().unwrap_or("").contains("panose-1")
    {
        return None;
    }
    let is_syntax_error = rule_id == Some("CssSyntaxError");

    let title = message
        .text
        .clone()
        .unwrap_or_else(|| "Unknown error".to_string());
    let title = title.split('(').next().unwrap_or(&title).trim().to_string();
    let line = message
        .line
        .and_then(|l| u32::try_from(l + line_offset).ok());
    let col = message.column.and_then(|c| u32::try_from(c - 1).ok());

    let mut err = if is_syntax_error {
        css_parse_error(&title, name, line, col)
    } else if message.severity.as_deref() == Some("error") {
        css_error(&title, name, line, col)
    } else {
        css_warning(&title, name, line, col)
    };

    let mut help = message.text.clone().unwrap_or_default();
    if !help.is_empty() {
        help.push_str(". ");
    }
    if let Some(meta) = rule_id.and_then(|r| rule_metadata.get(r)) {
        if let Some(url) = &meta.url {
            help.push_str(&format!("See <a href=\"{url}\">detailed description</a>. "));
        }
        if meta.fixable {
            err = err.fixable_css();
            help.push_str(
                "This error will be automatically fixed if you click \"Try to correct all \
                 fixable errors\" below.",
            );
        }
    }
    err.help = help;
    Some(err)
}

/// Port of `Job`/`create_job`: real. Wraps a `style=""` attribute's
/// declaration text in a synthetic rule (stylelint, like most CSS
/// tools, operates on stylesheets, not bare declaration lists) and
/// folds a positive line offset into leading blank lines so any
/// reported line number already lines up with the source document
/// without the caller needing to add it back on afterwards.
#[derive(Debug, Clone)]
pub struct Job {
    pub name: String,
    pub css: String,
    pub line_offset: i64,
}

pub fn create_job(name: &str, css: &str, line_offset: i64, is_declaration: bool) -> Job {
    let (mut css, mut line_offset) = if is_declaration {
        (format!("div{{\n{css}\n}}"), line_offset - 1)
    } else {
        (css.to_string(), line_offset)
    };
    if line_offset > 0 {
        css = "\n".repeat(line_offset as usize) + &css;
        line_offset = 0;
    }
    Job {
        name: name.to_string(),
        css,
        line_offset,
    }
}

/// Port of `check_css`: real running of stylelint over `jobs`. See the
/// module docs -- this needs a headless `QWebEnginePage` running a
/// bundled JS linter, which does not exist in this Rust workspace.
pub fn check_css(_jobs: &[Job]) -> Vec<CheckError> {
    todo!(
        "placeholder: linting CSS needs stylelint (a JS linter bundle) running inside a \
         headless QWebEnginePage (css.py's Worker/Pool/create_profile) -- neither a JS engine \
         embed nor a native Rust CSS linter exists in this workspace; see this module's docs"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_to_error_classifies_syntax_error() {
        let msg = StylelintMessage {
            rule: Some("CssSyntaxError".to_string()),
            severity: Some("error".to_string()),
            text: Some("Unexpected token (5:3)".to_string()),
            line: Some(5),
            column: Some(4),
        };
        let err = message_to_error(&msg, "a.css", 0, &HashMap::new()).unwrap();
        assert_eq!(err.type_name, "CSSParseError");
        assert!(err.is_parsing_error);
        assert_eq!(err.msg, "Unexpected token");
        assert_eq!(err.line, Some(5));
        assert_eq!(err.col, Some(3));
    }

    #[test]
    fn message_to_error_classifies_error_vs_warning() {
        let mut msg = StylelintMessage {
            rule: Some("some-rule".to_string()),
            severity: Some("error".to_string()),
            text: Some("bad".to_string()),
            line: None,
            column: None,
        };
        let err = message_to_error(&msg, "a.css", 0, &HashMap::new()).unwrap();
        assert_eq!(err.type_name, "CSSError");

        msg.severity = Some("warning".to_string());
        let err = message_to_error(&msg, "a.css", 0, &HashMap::new()).unwrap();
        assert_eq!(err.type_name, "CSSWarning");
    }

    #[test]
    fn message_to_error_suppresses_panose_1_warning() {
        let msg = StylelintMessage {
            rule: Some("property-no-unknown".to_string()),
            severity: Some("warning".to_string()),
            text: Some("Unexpected unknown property \"panose-1\"".to_string()),
            line: None,
            column: None,
        };
        assert!(message_to_error(&msg, "a.css", 0, &HashMap::new()).is_none());
    }

    #[test]
    fn message_to_error_applies_line_offset_and_rule_metadata() {
        let msg = StylelintMessage {
            rule: Some("color-no-invalid-hex".to_string()),
            severity: Some("error".to_string()),
            text: Some("Unexpected invalid hex color".to_string()),
            line: Some(2),
            column: Some(1),
        };
        let mut meta = HashMap::new();
        meta.insert(
            "color-no-invalid-hex".to_string(),
            RuleMetadata {
                url: Some("https://example.com/rule".to_string()),
                fixable: true,
            },
        );
        let err = message_to_error(&msg, "a.css", 10, &meta).unwrap();
        assert_eq!(err.line, Some(12));
        assert!(err.fixable_css_error);
        assert!(err.help.contains("detailed description"));
        assert!(err.help.contains("automatically fixed"));
    }

    #[test]
    fn create_job_wraps_declarations_and_offsets_blank_lines() {
        let job = create_job("a.html", "color: red", 3, true);
        // is_declaration wraps in `div{...}` and consumes one line of
        // offset for the synthetic opening brace; the remaining offset
        // (2) becomes leading blank lines, folded to zero.
        assert!(job.css.contains("div{"));
        assert!(job.css.contains("color: red"));
        assert_eq!(job.line_offset, 0);
        assert!(job.css.starts_with(&"\n".repeat(2)));
    }

    #[test]
    fn create_job_plain_stylesheet_keeps_offset_when_not_positive() {
        let job = create_job("a.css", "body{color:red}", 0, false);
        assert_eq!(job.css, "body{color:red}");
        assert_eq!(job.line_offset, 0);
    }
}
