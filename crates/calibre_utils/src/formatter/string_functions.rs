//! Port of `formatter_functions.py`'s `STRING_MANIPULATION` +
//! `CASE_CHANGES` categories (issue #515, part of the #460 formatter
//! epic): the built-ins that need no book/`Cache` access at all, so
//! (unlike #514's `calibre_db::formatter_functions`) they live here in
//! `calibre_utils` and are usable by *any* [`super::interp::ValueSource`]
//! context, not just a book template.
//!
//! Of the 20 functions in these two upstream categories, four are NOT
//! here because #513 already inlines them as dedicated [`super::ast::ExprKind`]
//! variants (short-circuiting/lazy-eval shortcuts, same as upstream's
//! own parser): `strcat`, `test`, `contains`, `character`. Two more
//! (`check_yes_no`, `field_exists`) need real book-field access and
//! live in `calibre_db::formatter_functions` instead, falling back to
//! [`call`]/[`arg_count`] here for everything else -- see that
//! module's own doc.
//!
//! # Disclosed narrowing: `re_group`'s per-group templates
//!
//! Upstream's `re_group(value, pattern, [template]*)` runs each
//! per-group `template` argument (e.g. `"{$:uppercase()}"`) through
//! `EvalFormatter().safe_format`, calibre's *separate* `{field:func}`
//! shorthand-template compiler (distinct from the `program:` mode
//! language #513 ported -- that compiler itself isn't ported anywhere
//! in this crate). Rather than skip the feature or silently return the
//! raw group text, this port implements the narrow, real subset that
//! covers the documented example and the overwhelming majority of
//! actual usage: a template must be exactly `{$}` or
//! `{$:func1(args)[:func2(args)]*}` -- a literal `$` (the matched
//! group's text) optionally piped through a chain of function calls
//! *from this same module* (not any `calibre_db`-backed function, and
//! no nested `{...}` field references). Anything else is a real,
//! reported error rather than a silent wrong answer.

use super::interp::FunctionRegistry;
use super::parser::FunctionCatalog;
use crate::filenames::ascii_text;
use crate::icu;
use regex::{Captures, RegexBuilder};

/// A [`FunctionRegistry`] backed only by [`call`] -- no state needed,
/// since none of these functions touch a book or a `Cache`.
pub struct StringFunctions;

impl FunctionRegistry for StringFunctions {
    fn call(&self, name: &str, args: &[String]) -> Result<String, String> {
        call(name, args)
    }
}

/// A [`FunctionCatalog`] backed only by [`arg_count`].
pub struct StringCatalog;

impl FunctionCatalog for StringCatalog {
    fn arg_count(&self, name: &str) -> Option<Option<usize>> {
        arg_count(name)
    }
}

/// Parse-time arity/existence for every function [`call`] handles --
/// outer `None` = unknown name, `Some(None)` = variadic, `Some(Some(n))`
/// = fixed arity `n`. A free function (not just a method on
/// [`StringCatalog`]) so `calibre_db::formatter_functions::CacheCatalog`
/// can fall back to it directly.
pub fn arg_count(name: &str) -> Option<Option<usize>> {
    match name {
        "strlen" | "transliterate" | "swap_around_comma" | "uppercase" | "lowercase" | "titlecase" | "capitalize" => Some(Some(1)),
        "ifempty" | "swap_around_articles" => Some(Some(2)),
        "substr" | "re" => Some(Some(3)),
        "shorten" => Some(Some(4)),
        "strcat_max" | "re_group" => Some(None),
        _ => None,
    }
}

/// Real dispatch for every function this module implements -- a free
/// function (not just [`StringFunctions::call`]) so
/// `calibre_db::formatter_functions::CacheFunctions::call` can fall
/// back to it directly for names it doesn't itself handle.
pub fn call(name: &str, args: &[String]) -> Result<String, String> {
    match name {
        "strlen" => Ok(args[0].chars().count().to_string()),
        "substr" => substr(&args[0], &args[1], &args[2]),
        "strcat_max" => strcat_max(args),
        "re" => re_sub(&args[0], &args[1], &args[2]),
        "re_group" => re_group(&args[0], &args[1], &args[2..]),
        "swap_around_comma" => swap_around_comma(&args[0]),
        "ifempty" => Ok(if args[0].is_empty() { args[1].clone() } else { args[0].clone() }),
        "shorten" => shorten(&args[0], &args[1], &args[2], &args[3]),
        "transliterate" => Ok(ascii_text(&args[0])),
        "swap_around_articles" => swap_around_articles(&args[0], &args[1]),
        "uppercase" => Ok(icu::upper(&args[0])),
        "lowercase" => Ok(icu::lower(&args[0])),
        "titlecase" => Ok(icu::title_case(&args[0])),
        "capitalize" => Ok(icu::capitalize(&args[0])),
        _ => Err(format!("No function named {name:?} exists")),
    }
}

/// Port of `substr`: Python slice semantics (`value[start:end]`),
/// including negative-index wraparound, on `value`'s *characters*
/// (matching Python's own codepoint-indexed strings, not bytes).
fn substr(value: &str, start_: &str, end_: &str) -> Result<String, String> {
    let start: i64 = start_.parse().map_err(|_| format!("substr: invalid start {start_:?}"))?;
    let end_raw: i64 = end_.parse().map_err(|_| format!("substr: invalid end {end_:?}"))?;
    let chars: Vec<char> = value.chars().collect();
    let n = chars.len() as i64;
    let end = if end_raw == 0 { n } else { end_raw };
    let norm = |i: i64| -> i64 { if i < 0 { (n + i).max(0) } else { i.min(n) } };
    let s = norm(start);
    let e = norm(end);
    if s >= e {
        return Ok(String::new());
    }
    Ok(chars[s as usize..e as usize].iter().collect())
}

/// Port of `strcat_max`: `args[0]` is the max length, `args[1]` the
/// initial string, then `(prefix, string)` pairs are appended as long
/// as the running length stays under max.
fn strcat_max(args: &[String]) -> Result<String, String> {
    if args.len() < 2 {
        return Err("strcat_max requires 2 or more arguments".to_string());
    }
    if args.len() % 2 != 0 {
        return Err("strcat_max requires an even number of arguments".to_string());
    }
    let max: i64 = args[0].parse().map_err(|_| "first argument to strcat_max must be an integer".to_string())?;
    let mut result = args[1].clone();
    let mut i = 2;
    while i + 1 < args.len() {
        let prefix = &args[i];
        let piece = &args[i + 1];
        if (result.chars().count() + prefix.chars().count() + piece.chars().count()) as i64 > max {
            break;
        }
        result.push_str(prefix);
        result.push_str(piece);
        i += 2;
    }
    Ok(result.trim().to_string())
}

/// Translates a Python `re.sub` replacement template (`\1`, `\2`, ...
/// group backreferences, literal `$` otherwise) into the `regex`
/// crate's own replacement syntax (`${1}`, `${2}`, ... with a literal
/// `$` doubled to `$$`).
pub(crate) fn translate_python_replacement(replacement: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => out.push_str("$$"),
            '\\' => match chars.peek() {
                Some(d) if d.is_ascii_digit() => {
                    let mut num = String::new();
                    while let Some(&d) = chars.peek() {
                        if d.is_ascii_digit() {
                            num.push(d);
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    out.push_str(&format!("${{{num}}}"));
                }
                Some(&other) => {
                    out.push(other);
                    chars.next();
                }
                None => out.push('\\'),
            },
            other => out.push(other),
        }
    }
    out
}

/// Port of `re`: case-insensitive regex replace-all with a Python-style
/// backreference replacement template.
fn re_sub(value: &str, pattern: &str, replacement: &str) -> Result<String, String> {
    let re = RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let rust_repl = translate_python_replacement(replacement);
    Ok(re.replace_all(value, rust_repl.as_str()).into_owned())
}

/// Port of `swap_around_comma`: `"B, A"` -> `"A B"`, unchanged if there
/// is no comma.
fn swap_around_comma(value: &str) -> Result<String, String> {
    let re = RegexBuilder::new(r"^(.*?),\s*(.*)$").case_insensitive(true).build().map_err(|e| e.to_string())?;
    Ok(re.replace(value, "$2 $1").trim().to_string())
}

/// Port of `shorten`: keep `left_chars` from the start and
/// `right_chars` from the end (character-counted, not byte-counted),
/// joined by `middle_text`; unchanged if already short enough.
fn shorten(val: &str, leading: &str, center: &str, trailing: &str) -> Result<String, String> {
    let l: i64 = leading.parse().map_err(|_| format!("shorten: invalid left_chars {leading:?}"))?;
    let t: i64 = trailing.parse().map_err(|_| format!("shorten: invalid right_chars {trailing:?}"))?;
    let l = l.max(0) as usize;
    let t = t.max(0) as usize;
    let chars: Vec<char> = val.chars().collect();
    if chars.len() > l + center.chars().count() + t {
        let left: String = chars[..l.min(chars.len())].iter().collect();
        let right: String = if t == 0 { String::new() } else { chars[chars.len().saturating_sub(t)..].iter().collect() };
        Ok(format!("{left}{center}{right}"))
    } else {
        Ok(val.to_string())
    }
}

/// Duplicated (not imported) from `calibre_ebooks::metadata::meta::title_sort`
/// -- `calibre_ebooks` depends on `calibre_utils`, so the reverse
/// dependency this function would need is unavailable. Kept in exact
/// sync by being a direct copy of that function's own small regex
/// logic, not a reinterpretation.
fn title_sort_str(title: &str) -> String {
    let re = regex::Regex::new(r"^(A|The|An)\s+").unwrap();
    let title = title.trim();
    if let Some(mat) = re.find(title) {
        let pfx = mat.as_str();
        format!("{}, {}", &title[pfx.len()..], pfx.trim())
    } else {
        title.to_string()
    }
}

/// Port of `swap_around_articles`: run [`title_sort_str`] on `value`
/// (or each `separator`-split item of it), swap the resulting comma
/// for a semicolon, and (for the list form) sort the results using
/// real Unicode collation.
fn swap_around_articles(val: &str, separator: &str) -> Result<String, String> {
    if val.is_empty() {
        return Ok(String::new());
    }
    if separator.is_empty() {
        return Ok(title_sort_str(val).replace(',', ";"));
    }
    let mut result: Vec<String> = val.split(separator).map(|v| title_sort_str(v.trim()).replace(',', ";")).collect();
    result.sort_by(|a, b| icu::strcmp(a, b));
    Ok(result.join(separator))
}

/// Port of `re_group`: replace every match of `pattern` in `value`
/// with the concatenation of its capture groups, each optionally
/// rendered through the corresponding entry of `templates` -- see this
/// module's own doc for the real, narrowed per-group template grammar.
pub(crate) fn re_group(value: &str, pattern: &str, templates: &[String]) -> Result<String, String> {
    let re = RegexBuilder::new(pattern).case_insensitive(true).build().map_err(|e| e.to_string())?;
    let mut result = String::new();
    let mut last_end = 0;
    for caps in re.captures_iter(value) {
        let m = caps.get(0).unwrap();
        result.push_str(&value[last_end..m.start()]);
        result.push_str(&render_group_replacement(&caps, templates)?);
        last_end = m.end();
    }
    result.push_str(&value[last_end..]);
    Ok(result)
}

/// Port of `re_group`'s inner `repl(mo)` closure. If no capture group
/// in `caps` actually participated in the match (matching upstream's
/// own `mo.lastindex is None` case -- including the case where
/// `pattern` has no capture groups at all), the whole match is dropped
/// (replaced with the empty string), a real, non-obvious preserved
/// upstream behavior, not a bug in this port.
fn render_group_replacement(caps: &Captures, templates: &[String]) -> Result<String, String> {
    let Some(lastindex) = (1..caps.len()).filter(|&i| caps.get(i).is_some()).max() else {
        return Ok(String::new());
    };
    let mut res = String::new();
    for dex in 1..=lastindex {
        let Some(gv) = caps.get(dex) else { continue };
        let gv = gv.as_str();
        if let Some(template) = templates.get(dex - 1) {
            let template = template.replace("[[", "{").replace("]]", "}");
            res.push_str(&eval_group_template(&template, gv)?);
        } else {
            res.push_str(gv);
        }
    }
    Ok(res)
}

/// Evaluates one narrowed per-group template -- see this module's own
/// doc for the exact supported grammar.
fn eval_group_template(template: &str, group_value: &str) -> Result<String, String> {
    let unsupported = || format!("re_group: unsupported per-group template {template:?} (only '{{$}}' or '{{$:func(args)}}' chains calling functions from this module are supported in this port)");
    let body = template.strip_prefix('{').and_then(|s| s.strip_suffix('}')).ok_or_else(unsupported)?;
    let parts = split_top_level(body, ':');
    if parts.first().map(String::as_str) != Some("$") {
        return Err(unsupported());
    }
    let mut value = group_value.to_string();
    for part in &parts[1..] {
        let (name, call_args) = parse_function_call(part).map_err(|_| unsupported())?;
        let mut full_args = vec![value];
        full_args.extend(call_args);
        value = call(&name, &full_args)?;
    }
    Ok(value)
}

/// Splits `s` on top-level (outside `(...)`) occurrences of `sep`.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            c if c == sep && depth == 0 => parts.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

/// Parses `"func(a, b)"` into `("func", ["a", "b"])`.
fn parse_function_call(s: &str) -> Result<(String, Vec<String>), String> {
    let s = s.trim();
    let open = s.find('(').ok_or_else(|| format!("expected a function call, got {s:?}"))?;
    if !s.ends_with(')') {
        return Err(format!("expected a function call, got {s:?}"));
    }
    let name = s[..open].trim().to_string();
    let arg_str = &s[open + 1..s.len() - 1];
    let args = if arg_str.trim().is_empty() { Vec::new() } else { split_top_level(arg_str, ',').into_iter().map(|a| a.trim().to_string()).collect() };
    Ok((name, args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strlen_counts_characters() {
        assert_eq!(call("strlen", &["hello".to_string()]).unwrap(), "5");
        assert_eq!(call("strlen", &["".to_string()]).unwrap(), "0");
    }

    #[test]
    fn substr_matches_python_slice_semantics() {
        assert_eq!(substr("12345", "1", "0").unwrap(), "2345");
        assert_eq!(substr("12345", "1", "-1").unwrap(), "234");
        assert_eq!(substr("12345", "0", "0").unwrap(), "12345");
        assert_eq!(substr("12345", "-2", "0").unwrap(), "45");
        assert_eq!(substr("12345", "10", "0").unwrap(), "", "start past the end clips to empty");
    }

    #[test]
    fn strcat_max_stops_before_exceeding_max() {
        assert_eq!(call("strcat_max", &["10".to_string(), "abc".to_string()]).unwrap(), "abc");
        assert_eq!(call("strcat_max", &["10".to_string(), "abc".to_string(), "-".to_string(), "defgh".to_string()]).unwrap(), "abc-defgh");
        // Second pair would push it over 10 chars, so it's dropped.
        assert_eq!(call("strcat_max", &["8".to_string(), "abc".to_string(), "-".to_string(), "defgh".to_string()]).unwrap(), "abc");
        assert!(call("strcat_max", &["10".to_string()]).is_err(), "needs 2+ args");
        assert!(call("strcat_max", &["10".to_string(), "a".to_string(), "b".to_string()]).is_err(), "needs an even count");
        assert!(call("strcat_max", &["notanumber".to_string(), "a".to_string()]).is_err());
    }

    #[test]
    fn re_replaces_case_insensitively_with_backreferences() {
        assert_eq!(call("re", &["HELLO world".to_string(), "(hello) (world)".to_string(), r"\2 \1".to_string()]).unwrap(), "world HELLO");
        assert_eq!(call("re", &["price: $5".to_string(), r"\$(\d+)".to_string(), r"USD\1".to_string()]).unwrap(), "price: USD5");
    }

    #[test]
    fn re_group_replaces_each_match_and_defaults_unrendered_groups_to_raw_text() {
        assert_eq!(call("re_group", &["a1 b2".to_string(), r"([a-z])(\d)".to_string()]).unwrap(), "a1 b2");
    }

    #[test]
    fn re_group_runs_the_narrow_dollar_template_grammar() {
        let out = call(
            "re_group",
            &["hello world".to_string(), r"(\S* )(.*)".to_string(), "{$:uppercase()}".to_string(), "{$}".to_string()],
        )
        .unwrap();
        assert_eq!(out, "HELLO world");
    }

    #[test]
    fn re_group_with_no_capture_groups_drops_every_match() {
        // Matches `mo.lastindex is None` when the pattern has no
        // groups at all -- a real, preserved upstream quirk.
        assert_eq!(call("re_group", &["abcabc".to_string(), "abc".to_string()]).unwrap(), "");
    }

    #[test]
    fn re_group_rejects_unsupported_template_grammar() {
        assert!(call("re_group", &["ab".to_string(), "(a)(b)".to_string(), "{some_field}".to_string()]).is_err());
    }

    #[test]
    fn swap_around_comma_reorders_and_leaves_no_comma_unchanged() {
        assert_eq!(call("swap_around_comma", &["Doe, John".to_string()]).unwrap(), "John Doe");
        assert_eq!(call("swap_around_comma", &["John Doe".to_string()]).unwrap(), "John Doe");
    }

    #[test]
    fn ifempty_falls_back_only_when_empty() {
        assert_eq!(call("ifempty", &["x".to_string(), "fallback".to_string()]).unwrap(), "x");
        assert_eq!(call("ifempty", &["".to_string(), "fallback".to_string()]).unwrap(), "fallback");
    }

    #[test]
    fn shorten_keeps_both_ends_or_leaves_short_values_unchanged() {
        assert_eq!(call("shorten", &["Ancient English Laws in the Times of Ivanhoe".to_string(), "9".to_string(), "-".to_string(), "5".to_string()]).unwrap(), "Ancient E-anhoe");
        assert_eq!(call("shorten", &["The Dome".to_string(), "9".to_string(), "-".to_string(), "5".to_string()]).unwrap(), "The Dome");
    }

    #[test]
    fn transliterate_ports_accented_text_to_ascii() {
        let out = call("transliterate", &["Fedor".to_string()]).unwrap();
        assert!(out.is_ascii());
    }

    #[test]
    fn swap_around_articles_moves_leading_articles_and_sorts_lists() {
        assert_eq!(call("swap_around_articles", &["The Dome".to_string(), "".to_string()]).unwrap(), "Dome; The");
        let out = call("swap_around_articles", &["The Zoo & The Ant".to_string(), " & ".to_string()]).unwrap();
        assert_eq!(out, "Ant; The & Zoo; The");
    }

    #[test]
    fn case_functions_delegate_to_icu() {
        assert_eq!(call("uppercase", &["abc".to_string()]).unwrap(), icu::upper("abc"));
        assert_eq!(call("lowercase", &["ABC".to_string()]).unwrap(), icu::lower("ABC"));
        assert_eq!(call("titlecase", &["a tale".to_string()]).unwrap(), icu::title_case("a tale"));
        assert_eq!(call("capitalize", &["hello world".to_string()]).unwrap(), icu::capitalize("hello world"));
    }

    #[test]
    fn unknown_function_is_a_real_error() {
        assert!(call("no_such_function", &[]).is_err());
    }

    #[test]
    fn catalog_reports_correct_arity() {
        assert_eq!(arg_count("strlen"), Some(Some(1)));
        assert_eq!(arg_count("shorten"), Some(Some(4)));
        assert_eq!(arg_count("strcat_max"), Some(None));
        assert_eq!(arg_count("no_such_function"), None);
    }

    #[test]
    fn registry_and_catalog_structs_delegate_to_the_free_functions() {
        let reg = StringFunctions;
        assert_eq!(reg.call("strlen", &["abcd".to_string()]).unwrap(), "4");
        let cat = StringCatalog;
        assert_eq!(cat.arg_count("strlen"), Some(Some(1)));
    }
}
