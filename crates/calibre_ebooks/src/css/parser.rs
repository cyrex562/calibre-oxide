//! Turns CSS text into the [`super::model`] object model, using
//! [`cssparser`]'s tokenizer to find rule/declaration/block boundaries
//! (nesting- and string/comment-aware, unlike a naive brace counter)
//! and this module's own recursive-descent for what sits inside those
//! boundaries. See the `css` module docs for the overall design.

use cssparser::{Delimiter, Delimiters, ParseError, Parser, ParserInput, Token};

use super::model::{
    Declaration, ImportRule, MediaRule, NamespaceRule, Rule, StyleDeclarationBlock, StyleRule,
    UnknownAtRule,
};
use super::selector::parse_selector_list;

/// Port of `container.parse_css(data)` for the stylesheet case (a whole
/// `.css` file or a `<style>` tag's text).
pub fn parse_stylesheet(css: &str) -> super::model::Stylesheet {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    super::model::Stylesheet {
        rules: parse_rules(&mut parser),
    }
}

/// Port of `container.parse_css(data, is_declaration=True)`: parses
/// `text` as the contents of a `style="..."` attribute or an
/// `@font-face`/rule body (`prop: value; prop2: value2`).
pub fn parse_declaration_list(text: &str) -> StyleDeclarationBlock {
    let mut input = ParserInput::new(text);
    let mut parser = Parser::new(&mut input);
    parse_declarations(&mut parser)
}

/// Captures the raw text inside a block whose opening token
/// (`{`/`[`/`(`) was just returned by `parser`'s last `next*` call, by
/// entering it via `parse_nested_block` and draining it (which correctly
/// skips over any further nested blocks, strings, and comments) while
/// recording the span. `parse_nested_block` cannot itself report an
/// error here since the closure always fully drains the block before
/// returning `Ok`.
pub(crate) fn capture_block_text<'i>(parser: &mut Parser<'i, '_>) -> String {
    let result: Result<String, ParseError<'i, ()>> = parser.parse_nested_block(|input| {
        let start = input.position();
        while input.next_including_whitespace_and_comments().is_ok() {}
        Ok(input.slice_from(start).to_string())
    });
    result.unwrap_or_default()
}

/// Captures the raw text from the parser's current position up to (not
/// including) the first token matching `delims` at the current nesting
/// level, or the end of input.
fn consume_raw_until<'i>(parser: &mut Parser<'i, '_>, delims: Delimiters) -> String {
    let start = parser.position();
    let _: Result<(), ParseError<'i, ()>> = parser.parse_until_before(delims, |input| {
        while input.next_including_whitespace_and_comments().is_ok() {}
        Ok(())
    });
    parser.slice_from(start).to_string()
}

fn parse_rules(parser: &mut Parser) -> Vec<Rule> {
    let mut rules = Vec::new();
    loop {
        parser.skip_whitespace();
        if parser.is_exhausted() {
            break;
        }
        let state = parser.state();
        let tok = match parser.next() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match tok {
            Token::AtKeyword(name) => {
                let name = name.to_string();
                let prelude =
                    consume_raw_until(parser, Delimiter::Semicolon | Delimiter::CurlyBracketBlock)
                        .trim()
                        .to_string();
                match parser.next() {
                    Ok(Token::Semicolon) => rules.push(at_rule_no_block(&name, &prelude)),
                    Ok(Token::CurlyBracketBlock) => {
                        let block_text = capture_block_text(parser);
                        rules.push(at_rule_with_block(&name, &prelude, &block_text));
                    }
                    _ => rules.push(at_rule_no_block(&name, &prelude)),
                }
            }
            _ => {
                parser.reset(&state);
                let selector_text = consume_raw_until(parser, Delimiter::CurlyBracketBlock)
                    .trim()
                    .to_string();
                match parser.next() {
                    Ok(Token::CurlyBracketBlock) => {
                        let block_text = capture_block_text(parser);
                        if !selector_text.is_empty() {
                            if let Ok(selectors) = parse_selector_list(&selector_text) {
                                rules.push(Rule::Style(StyleRule {
                                    selector_text,
                                    selectors,
                                    style: parse_declaration_list(&block_text),
                                }));
                            }
                            // An unparseable selector list is dropped
                            // rather than aborting the whole stylesheet
                            // parse -- matches `mark_used_selectors`'
                            // caller-level tolerance for selectors this
                            // scoped grammar can't handle (see
                            // `selector`'s module docs); a rule with no
                            // usable selector can't meaningfully
                            // participate in any of `css.py`'s
                            // selector-driven logic anyway.
                        }
                    }
                    _ => break,
                }
            }
        }
    }
    rules
}

fn at_rule_no_block(name: &str, prelude: &str) -> Rule {
    match name.to_ascii_lowercase().as_str() {
        "import" => Rule::Import(parse_import_prelude(prelude)),
        "charset" => Rule::Charset(strip_quotes(prelude.trim()).to_string()),
        "namespace" => Rule::Namespace(parse_namespace_prelude(prelude)),
        _ => Rule::Unknown(UnknownAtRule {
            at_keyword: name.to_string(),
            prelude: prelude.to_string(),
            block: None,
        }),
    }
}

fn at_rule_with_block(name: &str, prelude: &str, block: &str) -> Rule {
    match name.to_ascii_lowercase().as_str() {
        "media" => Rule::Media(MediaRule {
            media_text: prelude.to_string(),
            rules: parse_stylesheet(block).rules,
        }),
        "font-face" => Rule::FontFace(parse_declaration_list(block)),
        _ => Rule::Unknown(UnknownAtRule {
            at_keyword: name.to_string(),
            prelude: prelude.to_string(),
            block: Some(block.to_string()),
        }),
    }
}

/// `@import url(foo.css) screen;` / `@import "foo.css";`. A narrow,
/// hand-written scan rather than routing through `cssparser` again: the
/// shape is fixed (one `url(...)`/string token, then an optional media
/// list kept as opaque text, matching [`super::model::MediaRule`]'s own
/// scope).
fn parse_import_prelude(prelude: &str) -> ImportRule {
    let prelude = prelude.trim();
    if let Some(rest) = prelude
        .strip_prefix("url(")
        .or_else(|| prelude.strip_prefix("URL("))
    {
        if let Some(end) = rest.find(')') {
            let href = strip_quotes(rest[..end].trim()).to_string();
            let media = rest[end + 1..].trim();
            return ImportRule {
                href,
                media_text: (!media.is_empty()).then(|| media.to_string()),
            };
        }
    }
    if let Some(quote) = prelude.chars().next().filter(|c| matches!(c, '"' | '\'')) {
        if let Some(end) = prelude[1..].find(quote) {
            let href = prelude[1..1 + end].to_string();
            let media = prelude[2 + end..].trim();
            return ImportRule {
                href,
                media_text: (!media.is_empty()).then(|| media.to_string()),
            };
        }
    }
    ImportRule {
        href: String::new(),
        media_text: None,
    }
}

/// `@namespace prefix "uri";` / `@namespace "uri";`.
fn parse_namespace_prelude(prelude: &str) -> NamespaceRule {
    let prelude = prelude.trim();
    let (prefix, rest) = match prelude.find(|c: char| c.is_whitespace()) {
        Some(i) if !prelude[..i].starts_with(['"', '\'']) => {
            (Some(prelude[..i].to_string()), prelude[i..].trim())
        }
        _ => (None, prelude),
    };
    NamespaceRule {
        prefix,
        uri: strip_quotes(rest).to_string(),
    }
}

fn strip_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if s.len() > 1 && matches!(b[0], b'"' | b'\'') && b[0] == b[s.len() - 1] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn parse_declarations(parser: &mut Parser) -> StyleDeclarationBlock {
    let mut block = StyleDeclarationBlock::default();
    loop {
        parser.skip_whitespace();
        if parser.is_exhausted() {
            break;
        }
        let tok = match parser.next() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match tok {
            Token::Semicolon => continue,
            Token::Ident(name) => {
                let name = name.to_string();
                if parser.expect_colon().is_err() {
                    skip_to_semicolon(parser);
                    continue;
                }
                let raw_value = consume_raw_until(parser, Delimiter::Semicolon)
                    .trim()
                    .to_string();
                let _ = parser.next(); // consume the trailing ';', if any
                if raw_value.is_empty() {
                    continue;
                }
                let (value, important) = split_important(&raw_value);
                block.properties.push(Declaration {
                    name,
                    value,
                    important,
                });
            }
            _ => skip_to_semicolon(parser),
        }
    }
    block
}

fn skip_to_semicolon(parser: &mut Parser) {
    let _: Result<(), ParseError<()>> = parser.parse_until_after(Delimiter::Semicolon, |input| {
        while input.next_including_whitespace_and_comments().is_ok() {}
        Ok(())
    });
}

/// Splits a trailing `!important` (allowing whitespace around `!`, e.g.
/// `red ! important`) off a declaration's raw value text.
fn split_important(raw: &str) -> (String, bool) {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if let Some(pos) = lower.rfind('!') {
        if lower[pos + 1..].trim() == "important" {
            return (trimmed[..pos].trim_end().to_string(), true);
        }
    }
    (trimmed.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simple_style_rule() {
        let sheet = parse_stylesheet("div.a { color: red; font-size: 12px }");
        assert_eq!(sheet.rules.len(), 1);
        let r = sheet.rules[0].as_style().unwrap();
        assert_eq!(r.selector_text, "div.a");
        assert_eq!(r.style.get_property_value("color"), "red");
        assert_eq!(r.style.get_property_value("font-size"), "12px");
    }

    #[test]
    fn parses_important() {
        let sheet = parse_stylesheet("p { color: blue !important; }");
        let r = sheet.rules[0].as_style().unwrap();
        assert!(r.style.get_property("color").unwrap().important);
    }

    #[test]
    fn parses_multiple_rules_and_comments() {
        let sheet = parse_stylesheet(
            "/* leading comment */\n.a { color: red }\n/* mid */\n.b { color: blue }",
        );
        assert_eq!(sheet.rules.len(), 2);
    }

    #[test]
    fn parses_font_face_rule() {
        let sheet = parse_stylesheet(
            "@font-face { font-family: \"MyFont\"; src: url(fonts/my.otf); font-weight: bold; }",
        );
        assert_eq!(sheet.font_face_rules().count(), 1);
        let decl = sheet.font_face_rules().next().unwrap();
        assert_eq!(decl.get_property_value("font-family"), "\"MyFont\"");
        assert_eq!(decl.get_property_value("src"), "url(fonts/my.otf)");
    }

    #[test]
    fn parses_media_rule_with_nested_style_rules() {
        let sheet = parse_stylesheet("@media screen and (max-width: 200px) { .a { color: red } }");
        assert_eq!(sheet.rules.len(), 1);
        match &sheet.rules[0] {
            Rule::Media(m) => {
                assert_eq!(m.media_text, "screen and (max-width: 200px)");
                assert_eq!(m.rules.len(), 1);
                assert!(m.rules[0].as_style().is_some());
            }
            other => panic!("expected a media rule, got {other:?}"),
        }
    }

    #[test]
    fn parses_import_rule_with_url_and_media() {
        let sheet = parse_stylesheet("@import url(foo.css) screen;");
        match &sheet.rules[0] {
            Rule::Import(i) => {
                assert_eq!(i.href, "foo.css");
                assert_eq!(i.media_text.as_deref(), Some("screen"));
            }
            other => panic!("expected an import rule, got {other:?}"),
        }
    }

    #[test]
    fn parses_import_rule_with_quoted_string_href() {
        let sheet = parse_stylesheet("@import \"foo.css\";");
        match &sheet.rules[0] {
            Rule::Import(i) => {
                assert_eq!(i.href, "foo.css");
                assert_eq!(i.media_text, None);
            }
            other => panic!("expected an import rule, got {other:?}"),
        }
    }

    #[test]
    fn parses_charset_rule() {
        let sheet = parse_stylesheet("@charset \"UTF-8\";");
        assert_eq!(sheet.rules[0], Rule::Charset("UTF-8".to_string()));
    }

    #[test]
    fn declaration_list_parses_style_attribute_text() {
        let block = parse_declaration_list("display: none; font-weight: bold");
        assert_eq!(block.len(), 2);
        assert_eq!(block.get_property_value("display"), "none");
    }

    #[test]
    fn round_trips_through_to_css_text_and_reparse() {
        let sheet = parse_stylesheet(".a, .b { color: red; margin: 1px 2px }");
        let text = sheet.to_css_text();
        let reparsed = parse_stylesheet(&text);
        assert_eq!(reparsed.rules.len(), 1);
        let r = reparsed.rules[0].as_style().unwrap();
        assert_eq!(r.selectors.0.len(), 2);
        assert_eq!(r.style.get_property_value("margin"), "1px 2px");
    }

    #[test]
    fn a_rule_with_one_unsupported_selector_keeps_the_others() {
        // `svg|svg` (namespace-prefixed type selector) is out of this
        // crate's scoped selector grammar; the rest of the list must
        // still parse -- this is the real shape of a rule in the
        // bundled `templates/html.css` user-agent stylesheet.
        let sheet = parse_stylesheet("img, object, svg|svg { width: auto }");
        let r = sheet.rules[0].as_style().unwrap();
        assert_eq!(r.selectors.0.len(), 2);
        assert_eq!(r.selectors.0[0].text, "img");
        assert_eq!(r.selectors.0[1].text, "object");
    }
}
