//! Port of `TemplateFormatter`'s `{field}`/`{field:format_spec}`
//! `string.Formatter`-style substitution dialect (`vformat`) plus its
//! shared `program:`/default dispatch (`evaluate`) and never-fails
//! wrapper (`safe_format`).
//!
//! Generalized (issue #751) from `calibre_ebooks::covers`'s original
//! port of this same logic, which was written concretely against that
//! module's own private `CoverValueSource` -- `calibre_ebooks::covers`'s
//! own `template_tester` sibling (issue #763) explicitly disclosed
//! that generalizing this to a generic [`ValueSource`] was real,
//! separable follow-up work "if the shorthand dialect is ever needed
//! through [another] route" -- issue #751 (save-to-disk path
//! templates, whose real upstream default template
//! `'{author_sort}/{title}/{title} - {authors}'` uses exactly this
//! dialect) is that need. `calibre_ebooks::covers` now calls into this
//! module instead of keeping its own copy.

use std::collections::HashMap;

use super::interp::{self, RawValue, ValueSource};
use super::parser::{self, FunctionCatalog};
use super::{interp::FunctionRegistry, lexer};

/// Forwards to a borrowed `&dyn ValueSource` so [`interp::evaluate`]'s
/// `Box<dyn ValueSource + '_>` parameter can be satisfied without
/// requiring every caller's own value source to implement `Clone`
/// (`calibre_ebooks::covers`'s original `CoverValueSource` did; a
/// `Cache`-backed source generally shouldn't need to).
struct RefSource<'a>(&'a dyn ValueSource);

impl ValueSource for RefSource<'_> {
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

/// Port of `_eval_program`/`Formatter.evaluate`'s `program:` branch:
/// runs `program_text` through the real GPM lexer/parser/interpreter,
/// with `dollar_val` bound to the special `$` local (matching
/// `format_field`'s own `_eval_program(val, expr, ...)` call, where
/// `val` is the field's current value).
pub fn run_gpm(program_text: &str, dollar_val: &str, values: &dyn ValueSource, catalog: &dyn FunctionCatalog, functions: &dyn FunctionRegistry) -> Result<String, String> {
    let tokens = lexer::scan(program_text).map_err(|p| format!("lex error at byte {p}"))?;
    let expr = parser::parse(&tokens, catalog, Default::default()).map_err(|e| e.to_string())?;
    let mut globals = HashMap::new();
    interp::evaluate(&expr, dollar_val, Box::new(RefSource(values)), functions, &mut globals).map_err(|e| e.to_string())
}

/// Port of `_explode_format_string`: unwraps a `prefix|fmt|suffix`
/// format spec into its 3 parts (matching upstream's own regex
/// `^(.*)\|([^\|]*)\|(.*)$`); a spec with no `|`-delimited middle
/// section returns unchanged with empty prefix/suffix.
fn explode_format_string(fmt: &str) -> (&str, &str, &str) {
    if let Some(first) = fmt.find('|') {
        if let Some(rel_last) = fmt[first + 1..].rfind('|') {
            let last = first + 1 + rel_last;
            if last > first {
                return (&fmt[first + 1..last], &fmt[..first], &fmt[last + 1..]);
            }
        }
    }
    (fmt, "", "")
}

/// Port of `_do_format`: applies a Python `str.format()`-style
/// single-char type spec to `val`. Real upstream supports the full
/// numeric/width/precision/alignment grammar via `('{0:'+fmt+'}').format(val)`;
/// this only implements the empty-spec passthrough (`if not fmt or
/// not val: return val`), matching this port's original narrowing
/// carried over unchanged from `calibre_ebooks::covers`.
fn apply_display_format(val: &str, fmt: &str) -> String {
    if fmt.is_empty() || val.is_empty() {
        return val.to_string();
    }
    val.to_string()
}

/// Port of `TemplateFormatter.format_field`.
pub fn format_field(val: &str, fmt: &str, values: &dyn ValueSource, catalog: &dyn FunctionCatalog, functions: &dyn FunctionRegistry) -> Result<String, String> {
    let (fmt, prefix, suffix) = explode_format_string(fmt);

    let mut val = val.to_string();
    let mut dispfmt = fmt.to_string();

    let p = if fmt.starts_with('\'') {
        Some(0usize)
    } else {
        fmt.find(":'").map(|i| i + 1)
    };
    if let Some(p) = p {
        if fmt.ends_with('\'') && fmt.len() > p + 1 {
            let inner = &fmt[p + 1..fmt.len() - 1];
            val = run_gpm(inner, &val, values, catalog, functions)?;
            dispfmt = match fmt[..p].find(':') {
                None => String::new(),
                Some(colon) => fmt[..colon].to_string(),
            };
        }
        // else: malformed (starts with a quote-triggering pattern but
        // doesn't end in a quote) -- falls through with dispfmt
        // unchanged, matching upstream's own fallthrough to the
        // old-style-call check (which this port doesn't implement
        // either, see this section's own doc).
    }
    if !val.is_empty() {
        val = apply_display_format(&val, &dispfmt);
    }
    if val.is_empty() {
        return Ok(String::new());
    }
    Ok(format!("{prefix}{val}{suffix}"))
}

/// Port of `TemplateFormatter.evaluate`'s default (`vformat`) branch:
/// `string.Formatter`-style `{field}`/`{field:format_spec}`
/// substitution. `{{`/`}}` escape to literal braces. Every substituted
/// field value comes back already escaped from [`ValueSource::get_value`]
/// (matching `Formatter(SafeFormat).get_value`'s real override, and
/// GPM's own `field()` builtin, which delegates to the identical
/// method).
///
/// **Real, disclosed narrowing**: finds the format spec via the first
/// top-level `}` after the field name rather than reproducing Python's
/// full brace-nesting-aware `string.Formatter.parse` grammar (which
/// also allows a nested replacement field *inside* a format spec, e.g.
/// `{val:{width}}`).
pub fn vformat(fmt: &str, values: &dyn ValueSource, catalog: &dyn FunctionCatalog, functions: &dyn FunctionRegistry) -> Result<String, String> {
    let mut out = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        match chars[i] {
            '{' if chars.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if chars.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '{' => {
                let start = i + 1;
                let end = chars[start..].iter().position(|&c| c == '}').map(|p| start + p).unwrap_or(chars.len());
                let field_spec: String = chars[start..end].iter().collect();
                let (field_name, format_spec) = match field_spec.find(':') {
                    Some(p) => (&field_spec[..p], &field_spec[p + 1..]),
                    None => (field_spec.as_str(), ""),
                };
                // `get_value` already escapes (matching real
                // `Formatter(SafeFormat).get_value`'s override, which
                // both `vformat`'s own field substitution AND GPM's
                // `field()` builtin delegate to identically) -- no
                // separate escape step here.
                let val = values.get_value(field_name).unwrap_or_default();
                out.push_str(&format_field(&val, format_spec, values, catalog, functions)?);
                i = end + 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Port of `TemplateFormatter.evaluate`'s real dispatch (`program:` /
/// default). `python:` templates are N/A -- no Python interpreter in
/// this port.
pub fn evaluate_template(fmt: &str, values: &dyn ValueSource, catalog: &dyn FunctionCatalog, functions: &dyn FunctionRegistry) -> Result<String, String> {
    match fmt.strip_prefix("program:") {
        Some(rest) => run_gpm(rest, "", values, catalog, functions),
        None => vformat(fmt, values, catalog, functions),
    }
}

/// Port of `Formatter.safe_format`: never fails, substituting
/// `"Template error <message>"` on any real evaluation error (matching
/// upstream's `error_value + ' ' + error_message(e)`, with
/// `error_value` being the untranslated literal `"Template error"`).
pub fn safe_format(fmt: &str, values: &dyn ValueSource, catalog: &dyn FunctionCatalog, functions: &dyn FunctionRegistry) -> String {
    match evaluate_template(fmt, values, catalog, functions) {
        Ok(s) => s,
        Err(e) => format!("Template error {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formatter::interp::DictValueSource;
    use crate::formatter::{PureCatalog, PureFunctions};

    fn source(pairs: &[(&str, &str)]) -> DictValueSource {
        DictValueSource::new(pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect())
    }

    #[test]
    fn vformat_substitutes_a_plain_field() {
        let values = source(&[("title", "My Book")]);
        let out = vformat("{title}", &values, &PureCatalog, &PureFunctions).unwrap();
        assert_eq!(out, "My Book");
    }

    #[test]
    fn vformat_handles_escaped_braces() {
        let values = source(&[]);
        let out = vformat("{{literal}}", &values, &PureCatalog, &PureFunctions).unwrap();
        assert_eq!(out, "{literal}");
    }

    #[test]
    fn vformat_composes_a_real_path_template_with_multiple_fields() {
        let values = source(&[("author_sort", "Doe, Jane"), ("title", "My Book"), ("authors", "Jane Doe")]);
        let out = vformat("{author_sort}/{title}/{title} - {authors}", &values, &PureCatalog, &PureFunctions).unwrap();
        assert_eq!(out, "Doe, Jane/My Book/My Book - Jane Doe");
    }

    #[test]
    fn evaluate_template_dispatches_program_prefix_to_gpm() {
        let values = source(&[("title", "My Book")]);
        let out = evaluate_template("program: field('title')", &values, &PureCatalog, &PureFunctions).unwrap();
        assert_eq!(out, "My Book");
    }

    #[test]
    fn safe_format_never_fails_on_a_bad_template() {
        let values = source(&[]);
        let out = safe_format("program: not_a_real_function()", &values, &PureCatalog, &PureFunctions);
        assert!(out.starts_with("Template error"), "{out}");
    }
}
