//! Port of `old_src/src/calibre/srv/fast_css_transform.cpp`'s
//! `url()`/`@import`-string rewriting behavior (issue #479, part of
//! #427's tracking epic): find every `url(...)` reference and every
//! bare-string `@import` target in a stylesheet or declaration block,
//! and hand each one to a caller-supplied callback for rewriting
//! (`transform_style_sheet`'s own use is to virtualize resource links
//! for the in-browser reader -- that virtualization logic itself is
//! #481's territory, not this module's; this module is
//! scheme-agnostic, matching upstream's own `url_callback` parameter).
//!
//! # Scope: URL rewriting only, not upstream's full property rewrite
//!
//! `fast_css_transform.cpp`'s real `transform_properties` does much
//! more than URL rewriting: `page-break-before`/`page-break-after` ->
//! `break-before`/`break-after` plus an injected
//! `-webkit-column-break-*` fallback declaration, `-epub-writing-mode`/
//! `-webkit-writing-mode` -> `writing-mode` renaming, and absolute
//! font-size units (`px`/`pt`/`in`/keywords like `small`) converted to
//! `rem` -- confirmed by reading both the .cpp and its real test
//! suite (`old_src/src/calibre/srv/tests/fast_css_transform.py`).
//! None of that is ported here; this module only does the `url()`/
//! `@import` rewriting piece, matching issue #479's own filed scope.
//! The property-value semantic rewrites are real, substantial,
//! separate work -- filed as issue #488 rather than folded in here.
//!
//! # Approach: token-level splicing, not parse-model-reserialize
//!
//! [`calibre_ebooks::css`]'s existing `Stylesheet`/`Declaration`
//! model (built for `oeb::polish`'s cascade/stats logic, #164) isn't
//! used here -- reparsing into that model and reserializing would not
//! byte-for-byte preserve untouched content (exact whitespace,
//! comments, quote style elsewhere in the sheet), which upstream's
//! own behavior requires (its test suite asserts untouched regions
//! survive verbatim). Instead this walks the raw token stream via
//! `cssparser::Parser` (the same underlying tokenizer
//! `calibre_ebooks::css` itself is built on) with
//! `next_including_whitespace_and_comments`, copying every span
//! verbatim except the specific `url(...)`/`@import`-string spans it
//! rewrites, recursing into `{}`/`()`/`[]` blocks (so a `url(...)`
//! inside `@media { ... }` or any other nested block is still found).
//!
//! # Real, disclosed behavior notes
//!
//! - A rewritten `url(...)` is always re-emitted in `url("...")` form
//!   (double-quoted), regardless of whether the source used no
//!   quotes, single quotes, or double quotes -- matches upstream
//!   exactly (confirmed by its own test suite).
//! - A rewritten `@import "..."` (the bare-string form, no `url()`)
//!   keeps that bare-string form, just re-quoted with double quotes --
//!   also matches upstream (`@import url(...)` and `@import "..."`
//!   are NOT normalized to the same shape as each other).
//! - The callback decides what "unchanged" means: this module always
//!   invokes it for every `url(...)`/import-string it finds and only
//!   rewrites the span when the callback returns `Some` (upstream's
//!   own default no-op behavior when a callback declines is to leave
//!   the original text as-is) -- data: URIs, external URLs, etc.
//!   "left alone" is a property of what the *caller's* callback
//!   chooses to return, not special-cased at this layer (matching
//!   upstream: `render_book.py`'s own `create_link_replacer` is what
//!   decides to pass through a `data:`/external URL unchanged, not
//!   `fast_css_transform.cpp` itself).

use cssparser::{Parser, ParserInput, SourcePosition, Token};

fn escape_double_quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

/// Recursively walks `parser`'s token stream, rewriting `url(...)`
/// references and (only while `in_import` is true) the next bare
/// quoted string, via `callback`. Everything else is copied verbatim.
fn transform_block<'i>(parser: &mut Parser<'i, '_>, callback: &mut dyn FnMut(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut copied_upto: SourcePosition = parser.position();
    let mut at_statement_start = true;
    let mut in_import = false;

    loop {
        let before = parser.position();
        let tok = match parser.next_including_whitespace_and_comments() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match &tok {
            Token::WhiteSpace(_) | Token::Comment(_) => {
                // Neither ends nor starts a statement.
            }
            Token::Function(name) if name.eq_ignore_ascii_case("url") => {
                let mut url_value: Option<String> = None;
                let _ = parser.parse_nested_block::<_, _, ()>(|p| {
                    loop {
                        match p.next() {
                            Ok(Token::QuotedString(s)) => url_value = Some(s.as_ref().to_string()),
                            Ok(_) => continue,
                            Err(_) => break,
                        }
                    }
                    Ok(())
                });
                let end = parser.position();
                out.push_str(parser.slice(copied_upto..before));
                match url_value.as_deref().and_then(|v| callback(v).map(|new_val| (v, new_val))) {
                    Some((_, new_val)) => {
                        out.push_str("url(\"");
                        out.push_str(&escape_double_quoted(&new_val));
                        out.push_str("\")");
                    }
                    None => out.push_str(parser.slice(before..end)),
                }
                copied_upto = end;
                at_statement_start = false;
            }
            Token::UnquotedUrl(s) => {
                let end = parser.position();
                out.push_str(parser.slice(copied_upto..before));
                match callback(s.as_ref()) {
                    Some(new_val) => {
                        out.push_str("url(\"");
                        out.push_str(&escape_double_quoted(&new_val));
                        out.push_str("\")");
                    }
                    None => out.push_str(parser.slice(before..end)),
                }
                copied_upto = end;
                at_statement_start = false;
            }
            // `cssparser` is strict CSS Syntax Level 3: unescaped
            // whitespace inside an unquoted `url(...)`'s content (not
            // just leading/trailing) makes the rest a "bad url" per
            // spec. Upstream's own hand-rolled tokenizer is more
            // lenient than the spec here and still treats it as one
            // url value -- not replicated (see this module's own
            // doc's "internal whitespace" note); left as the original
            // text unchanged rather than guessing at a value.
            Token::BadUrl(_) => {
                at_statement_start = false;
            }
            Token::QuotedString(s) if in_import => {
                let end = parser.position();
                out.push_str(parser.slice(copied_upto..before));
                match callback(s.as_ref()) {
                    Some(new_val) => {
                        out.push('"');
                        out.push_str(&escape_double_quoted(&new_val));
                        out.push('"');
                    }
                    None => out.push_str(parser.slice(before..end)),
                }
                copied_upto = end;
                in_import = false;
                at_statement_start = false;
            }
            Token::AtKeyword(name) if at_statement_start => {
                in_import = name.eq_ignore_ascii_case("import");
                at_statement_start = false;
            }
            Token::Semicolon => {
                at_statement_start = true;
                in_import = false;
            }
            Token::CurlyBracketBlock | Token::ParenthesisBlock | Token::SquareBracketBlock | Token::Function(_) => {
                out.push_str(parser.slice(copied_upto..before));
                let open = match &tok {
                    Token::CurlyBracketBlock => "{".to_string(),
                    Token::ParenthesisBlock => "(".to_string(),
                    Token::SquareBracketBlock => "[".to_string(),
                    Token::Function(name) => format!("{name}("),
                    _ => unreachable!(),
                };
                out.push_str(&open);
                let close = match &tok {
                    Token::CurlyBracketBlock => "}",
                    Token::SquareBracketBlock => "]",
                    _ => ")",
                };
                let inner = parser.parse_nested_block::<_, _, ()>(|p| Ok(transform_block(p, callback))).unwrap_or_default();
                out.push_str(&inner);
                out.push_str(close);
                copied_upto = parser.position();
                at_statement_start = false;
                in_import = false;
            }
            _ => {
                at_statement_start = false;
            }
        }
    }

    out.push_str(parser.slice_from(copied_upto));
    out
}

/// Rewrites every `url(...)` reference (and, within an `@import`
/// statement, its bare-string target too) in `css` via `callback`,
/// leaving everything else byte-for-byte unchanged. Works for both a
/// full stylesheet and a bare declaration block (`prop: val; ...`,
/// upstream's `is_declaration=True` case) -- both are just token
/// streams to this function, no wrapping needed either way.
pub fn transform_urls(css: &str, mut callback: impl FnMut(&str) -> Option<String>) -> String {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    transform_block(&mut parser, &mut callback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upper_case(url: &str) -> Option<String> {
        Some(url.to_uppercase())
    }

    #[test]
    fn a_bare_unquoted_url_in_a_declaration_is_rewritten() {
        let out = transform_urls(".c{x:url(y)}", upper_case);
        assert_eq!(out, ".c{x:url(\"Y\")}");
    }

    #[test]
    fn a_quoted_url_is_rewritten_and_always_reemitted_double_quoted() {
        let out = transform_urls("a:url(  \"( )\" /**/ )", upper_case);
        assert_eq!(out, "a:url(\"( )\")");
    }

    #[test]
    fn internal_whitespace_in_an_unquoted_url_leaves_it_unrewritten() {
        // Upstream's own hand-rolled tokenizer is more lenient than
        // strict CSS Syntax Level 3 here and still treats
        // `url(  te  st.gif  )` as one url value ("te  st.gif") --
        // not replicated (see this module's own doc). `cssparser`
        // treats unescaped internal whitespace inside an unquoted url
        // as a "bad url" per spec; this module leaves it as-is rather
        // than guessing at an intended value. The second, well-formed
        // `url(x)` on the same line is still rewritten normally.
        let out = transform_urls("background: url(  te  st.gif  ) 12; src: url(x)", upper_case);
        assert_eq!(out, "background: url(  te  st.gif  ) 12; src: url(\"X\")");
    }

    #[test]
    fn an_escaped_close_paren_inside_an_unquoted_url_does_not_end_it_early() {
        let out = transform_urls(r"background: uRl(t\)e/st.gif)", upper_case);
        assert_eq!(out, "background: url(\"T)E/ST.GIF\")");
    }

    #[test]
    fn a_comment_inside_an_unquoted_url_is_kept_literal_not_stripped() {
        // Upstream's own tokenizer strips a comment appearing mid-url
        // (`url(te/**/st.gif)` -> value "test.gif") -- not replicated
        // here. Per strict CSS Syntax Level 3, `/*...*/ ` inside an
        // unquoted-url-token's own character run isn't treated as a
        // comment at all, so `cssparser` includes it literally in the
        // token's value; this module passes that whole literal value
        // to the callback rather than pre-stripping it.
        let out = transform_urls("background: url(te/**/st.gif)", upper_case);
        assert_eq!(out, "background: url(\"TE/**/ST.GIF\")");
    }

    #[test]
    fn a_callback_returning_none_leaves_the_original_text_unchanged() {
        let out = transform_urls("a:url(\"(/*)\")", |_| None);
        assert_eq!(out, "a:url(\"(/*)\")");
    }

    #[test]
    fn a_bare_string_import_target_is_rewritten() {
        let out = transform_urls(r#"@import "x.y";"#, upper_case);
        assert_eq!(out, r#"@import "X.Y";"#);
    }

    #[test]
    fn a_comment_splitting_the_at_keyword_itself_is_not_recognized_as_import() {
        // Upstream's own tokenizer strips comments from *inside* an
        // at-keyword before matching it, so `@im/* c */port "x.y";`
        // still counts as `@import` -- not replicated. Under strict
        // tokenizing this produces three separate tokens (`@im`, a
        // comment, `port`), never one `@import` at-keyword, so this
        // module doesn't recognize the statement as an import at all
        // and leaves it unrewritten -- a real, disclosed narrowing
        // for what's already unusual/adversarial CSS, not something
        // real-world EPUB stylesheets are expected to contain.
        let out = transform_urls("@im/* c */port \"x.y\";", upper_case);
        assert_eq!(out, "@im/* c */port \"x.y\";");
    }

    #[test]
    fn an_import_url_form_keeps_its_url_wrapper_and_trailing_media_query_is_untouched() {
        let out = transform_urls(r#"@import url("narrow.css") supports(display: flex) handheld and (max-width: 400px);"#, upper_case);
        assert_eq!(out, r#"@import url("NARROW.CSS") supports(display: flex) handheld and (max-width: 400px);"#);
    }

    #[test]
    fn only_the_first_import_string_is_rewritten_not_every_string_in_the_statement() {
        // Not a realistic stylesheet, but exercises the "in_import"
        // flag resetting after the first string is consumed.
        let out = transform_urls(r#"@import "a.css" "b.css";"#, upper_case);
        assert_eq!(out, r#"@import "A.CSS" "b.css";"#);
    }

    #[test]
    fn a_string_outside_any_import_statement_is_left_alone() {
        let out = transform_urls(r#".c { content: "hello" }"#, upper_case);
        assert_eq!(out, r#".c { content: "hello" }"#);
    }

    #[test]
    fn url_inside_a_nested_media_block_is_still_found() {
        let out = transform_urls("@media screen {\n    .cls {\n        background: url(\"b/loc.test\")\n    }\n}", upper_case);
        assert_eq!(out, "@media screen {\n    .cls {\n        background: url(\"B/LOC.TEST\")\n    }\n}");
    }

    #[test]
    fn a_full_stylesheet_only_rewrites_the_url_and_import_spans() {
        let sheet = "\n@import \"b/loc.test\";\n@media screen {\n    font: 16px calc(20vw - 30rem);\n\n    .cls {\n        color: red;\n        font-size: 16px;\n        background: url(\"b/loc.test\")\n    }\n}\n.why { font: 16px}\n";
        let expected = sheet.replace("b/loc.test", "B/LOC.TEST");
        let out = transform_urls(sheet, upper_case);
        assert_eq!(out, expected);
    }

    #[test]
    fn a_data_uri_can_be_left_unchanged_by_a_realistic_callback() {
        let out = transform_urls("x: url(data:image/png;base64,abc==)", |u| if u.starts_with("data:") { None } else { Some(u.to_uppercase()) });
        assert_eq!(out, "x: url(data:image/png;base64,abc==)");
    }

    #[test]
    fn declaration_block_mode_works_without_any_selector_or_braces() {
        let out = transform_urls("x: url(a); y: url(b)", upper_case);
        assert_eq!(out, "x: url(\"A\"); y: url(\"B\")");
    }
}
