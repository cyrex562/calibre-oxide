//! Port of `tinycss.fonts3`'s `font-family` list parsing/serialization
//! (`parse_font_family`/`serialize_font_family` only -- `cascade.py` and
//! `stats.py` are this port's only callers, and neither needs
//! `parse_font`/`serialize_font`'s full `font` shorthand handling, so
//! that part of `tinycss.fonts3` is not ported). This is plain string
//! tokenizing, not full CSS value parsing, so it is ported directly
//! rather than routed through [`crate::css`]'s object model -- matching
//! issue #164's own scoping note that this piece is "simple string
//! parsing, not full CSS".

use cssparser::{Parser, ParserInput, Token};

/// Port of `parse_font_family`: splits a `font-family` declaration's
/// value text into individual family names, e.g. `"Georgia", serif` ->
/// `["Georgia", "serif"]`. An unquoted multi-word name (`Times New
/// Roman`) is joined back into one entry, matching Python's
/// token-by-token accumulation.
pub fn parse_font_family(css_string: &str) -> Vec<String> {
    let text = css_string.trim();
    let mut input = ParserInput::new(text);
    let mut parser = Parser::new(&mut input);
    let mut families = Vec::new();
    let mut current = String::new();
    loop {
        match parser.next_including_whitespace() {
            Err(_) => break,
            Ok(Token::QuotedString(s)) => {
                if !current.trim().is_empty() {
                    commit(&mut current, &mut families);
                }
                current = s.to_string();
            }
            Ok(Token::Comma) => commit(&mut current, &mut families),
            Ok(Token::Ident(s)) => {
                current.push(' ');
                current.push_str(s);
            }
            // Whitespace and anything else (numbers, delimiters other
            // than the comma handled above, ...) are not part of the
            // grammar `tinycss.fonts3`'s tokenizer acts on either --
            // ignored, matching its implicit fallthrough.
            Ok(_) => {}
        }
    }
    commit(&mut current, &mut families);
    families
}

fn commit(current: &mut String, families: &mut Vec<String>) {
    let val = current.trim();
    if !val.is_empty() {
        families.push(val.to_string());
    }
    current.clear();
}

const GENERIC_FAMILIES: &[&str] = &[
    "serif",
    "sans-serif",
    "sansserif",
    "cursive",
    "fantasy",
    "monospace",
];

fn is_simple_name(x: &str) -> bool {
    let mut chars = x.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn serialize_single_font_family(x: &str) -> String {
    let xl = x.to_ascii_lowercase();
    if GENERIC_FAMILIES.contains(&xl.as_str()) {
        return if xl == "sansserif" {
            "sans-serif".to_string()
        } else {
            xl
        };
    }
    if is_simple_name(x) && !xl.starts_with("and") {
        return x.to_string();
    }
    format!("\"{}\"", x.replace('"', "\\\""))
}

/// Port of `serialize_font_family`.
pub fn serialize_font_family(families: &[String]) -> String {
    families
        .iter()
        .map(|f| serialize_single_font_family(f))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_font_family_splits_quoted_and_generic_names() {
        assert_eq!(
            parse_font_family("\"Georgia\", serif"),
            vec!["Georgia".to_string(), "serif".to_string()]
        );
    }

    #[test]
    fn parse_font_family_joins_multi_word_unquoted_names() {
        assert_eq!(
            parse_font_family("Times New Roman, Arial"),
            vec!["Times New Roman".to_string(), "Arial".to_string()]
        );
    }

    #[test]
    fn serialize_font_family_quotes_names_with_spaces() {
        assert_eq!(
            serialize_font_family(&["Times New Roman".to_string(), "serif".to_string()]),
            "\"Times New Roman\", serif"
        );
    }

    #[test]
    fn serialize_font_family_leaves_simple_names_unquoted() {
        assert_eq!(serialize_font_family(&["Georgia".to_string()]), "Georgia");
    }

    #[test]
    fn round_trips_a_font_family_declaration_value() {
        let parsed = parse_font_family("Georgia, \"Times New Roman\", serif");
        assert_eq!(
            serialize_font_family(&parsed),
            "Georgia, \"Times New Roman\", serif"
        );
    }
}
