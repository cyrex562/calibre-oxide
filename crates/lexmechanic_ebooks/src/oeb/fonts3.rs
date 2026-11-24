//! Port of `tinycss.fonts3`'s font-value parsing/serialization:
//! `parse_font_family`/`serialize_font_family` (the `font-family` list),
//! plus (issue #580) `parse_font`/`serialize_font`, the full `font`
//! shorthand grammar (style/variant/weight/stretch/size/line-height/
//! family). This is plain token-stream parsing, not full CSS value
//! parsing, so it is ported directly rather than routed through
//! [`crate::css`]'s object model -- matching issue #164's own scoping
//! note that this piece is "simple string parsing, not full CSS".

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

/// Port of `tinycss.fonts3`'s parsed `font` shorthand result (upstream
/// uses a plain dict keyed by CSS property name; a struct is the more
/// idiomatic Rust shape for the same fixed set of fields).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Font {
    pub style: Option<String>,
    pub variant: Option<String>,
    pub weight: Option<String>,
    pub stretch: Option<String>,
    pub size: Option<String>,
    pub line_height: Option<String>,
    pub family: Vec<String>,
}

/// Port of `serialize_font`.
pub fn serialize_font(font: &Font) -> String {
    let mut parts = Vec::new();
    if let Some(v) = &font.style {
        parts.push(v.clone());
    }
    if let Some(v) = &font.variant {
        parts.push(v.clone());
    }
    if let Some(v) = &font.weight {
        parts.push(v.clone());
    }
    if let Some(v) = &font.stretch {
        parts.push(v.clone());
    }
    if let Some(v) = &font.size {
        let mut fs = v.clone();
        if let Some(lh) = &font.line_height {
            fs.push('/');
            fs.push_str(lh);
        }
        parts.push(fs);
    }
    if !font.family.is_empty() {
        parts.push(serialize_font_family(&font.family));
    }
    parts.join(" ")
}

const GLOBAL_IDENTS: &[&str] = &["inherit", "initial", "unset", "normal"];
const STYLE_IDENTS: &[&str] = &["italic", "oblique"];
const VARIANT_IDENTS: &[&str] = &["small-caps"];
const WEIGHT_IDENTS: &[&str] = &["bold", "bolder", "lighter"];
const STRETCH_IDENTS: &[&str] = &[
    "ultra-condensed",
    "extra-condensed",
    "condensed",
    "semi-condensed",
    "semi-expanded",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
];
const SIZE_IDENTS: &[&str] = &[
    "xx-small", "x-small", "small", "medium", "large", "x-large", "xx-large", "larger", "smaller",
];
const WEIGHT_SIZES: &[i32] = &[100, 200, 300, 400, 500, 600, 700, 800, 900];
const LEGACY_FONT_SPEC: &[&str] = &[
    "caption",
    "icon",
    "menu",
    "message-box",
    "small-caption",
    "status-bar",
];

fn is_before_size_ident(s: &str) -> bool {
    STYLE_IDENTS.contains(&s) || VARIANT_IDENTS.contains(&s) || WEIGHT_IDENTS.contains(&s) || STRETCH_IDENTS.contains(&s)
}

/// One token of the flat stream `parse_font` walks, collapsing
/// `cssparser::Token` down to only the variants `tinycss.fonts3.parse_font`
/// actually branches on. `Integer`/`Number`/`Length` carry their
/// already-serialized source text (`tinycss`'s `Token.as_css()`
/// equivalent) alongside whatever value is needed for comparisons.
#[derive(Clone, Debug)]
enum FontToken {
    String(String),
    Integer { value: i32, css: String },
    Number { css: String },
    Delim(char),
    /// DIMENSION or PERCENTAGE -- `parse_font` treats both identically.
    Length { css: String },
    Ident(String),
    Other,
}

fn format_number(value: f32) -> String {
    format!("{value}")
}

fn tokenize_font_value(css_string: &str) -> Vec<FontToken> {
    let text = css_string.trim();
    let mut input = ParserInput::new(text);
    let mut parser = Parser::new(&mut input);
    let mut out = Vec::new();
    loop {
        match parser.next() {
            Err(_) => break,
            Ok(token) => out.push(match token {
                Token::QuotedString(s) => FontToken::String(s.to_string()),
                Token::Ident(s) => FontToken::Ident(s.to_string()),
                Token::Number { value, int_value, .. } => match int_value {
                    Some(iv) => FontToken::Integer {
                        value: *iv,
                        css: iv.to_string(),
                    },
                    None => FontToken::Number {
                        css: format_number(*value),
                    },
                },
                Token::Dimension { value, int_value, unit, .. } => {
                    let num = match int_value {
                        Some(iv) => iv.to_string(),
                        None => format_number(*value),
                    };
                    FontToken::Length {
                        css: format!("{num}{unit}"),
                    }
                }
                Token::Percentage { unit_value, int_value, .. } => {
                    let num = match int_value {
                        Some(iv) => iv.to_string(),
                        None => format_number(*unit_value * 100.0),
                    };
                    FontToken::Length {
                        css: format!("{num}%"),
                    }
                }
                Token::Delim(c) => FontToken::Delim(*c),
                Token::Comma => FontToken::Delim(','),
                _ => FontToken::Other,
            }),
        }
    }
    out
}

fn family_from_tokens(tokens: &[FontToken]) -> Vec<String> {
    let mut families = Vec::new();
    let mut current = String::new();
    for tok in tokens {
        match tok {
            FontToken::String(s) => {
                commit(&mut current, &mut families);
                current = s.clone();
            }
            FontToken::Delim(',') => commit(&mut current, &mut families),
            FontToken::Ident(s) => {
                current.push(' ');
                current.push_str(s);
            }
            _ => {}
        }
    }
    commit(&mut current, &mut families);
    families
}

/// Port of `parse_font` (<https://www.w3.org/TR/css-fonts-3/#font-prop>).
///
/// Faithfully replicates upstream's exact token-by-token state machine,
/// including its quirks: an ident/keyword encountered once every shorthand
/// slot is already filled is silently dropped rather than treated as part
/// of the family list (matches `tinycss.fonts3.parse_font`'s own behavior,
/// not "fixed" here), and keyword matching is case-sensitive (upstream
/// never lowercases tokens before comparing against e.g. `GLOBAL_IDENTS`).
pub fn parse_font(css_string: &str) -> Font {
    let tokens = tokenize_font_value(css_string);

    if let Some(first) = tokens.first() {
        let first_value = match first {
            FontToken::Ident(s) => Some(s.as_str()),
            FontToken::String(s) => Some(s.as_str()),
            _ => None,
        };
        if let Some(v) = first_value {
            if LEGACY_FONT_SPEC.contains(&v) {
                return Font {
                    family: vec!["sans-serif".to_string()],
                    ..Default::default()
                };
            }
        }
    }

    let mut style: Option<String> = None;
    let mut variant: Option<String> = None;
    let mut weight: Option<String> = None;
    let mut stretch: Option<String> = None;
    let mut size: Option<String> = None;
    let mut height: Option<String> = None;

    let n = tokens.len();
    let mut i = 0usize;
    let mut leftover_start = n;

    while i < n {
        let tok = tokens[i].clone();
        i += 1;
        match tok {
            FontToken::String(_) => {
                leftover_start = i - 1;
                break;
            }
            FontToken::Integer { value, css } => {
                if size.is_none() {
                    if weight.is_none() && WEIGHT_SIZES.contains(&value) {
                        weight = Some(css);
                        continue;
                    }
                } else if height.is_none() {
                    height = Some(css);
                }
                leftover_start = i;
                break;
            }
            FontToken::Number { css } => {
                if size.is_some() && height.is_none() {
                    height = Some(css);
                }
                leftover_start = i;
                break;
            }
            FontToken::Delim(c) => {
                if c == '/' && size.is_some() && height.is_none() {
                    continue;
                }
                leftover_start = i;
                break;
            }
            FontToken::Length { css } => {
                if size.is_none() {
                    size = Some(css);
                    continue;
                }
                if height.is_none() {
                    height = Some(css);
                }
                leftover_start = i;
                break;
            }
            FontToken::Ident(s) => {
                if GLOBAL_IDENTS.contains(&s.as_str()) {
                    if size.is_some() {
                        if height.is_none() {
                            height = Some(s);
                            leftover_start = i;
                        } else {
                            leftover_start = i - 1;
                        }
                        break;
                    }
                    if style.is_none() {
                        style = Some(s);
                    } else if variant.is_none() {
                        variant = Some(s);
                    } else if weight.is_none() {
                        weight = Some(s);
                    } else if stretch.is_none() {
                        stretch = Some(s);
                    } else if size.is_none() {
                        size = Some(s);
                    } else if height.is_none() {
                        height = Some(s);
                        leftover_start = i;
                        break;
                    } else {
                        leftover_start = i - 1;
                        break;
                    }
                    continue;
                }
                if is_before_size_ident(&s) {
                    if size.is_some() {
                        leftover_start = i;
                        break;
                    }
                    if STYLE_IDENTS.contains(&s.as_str()) {
                        style = Some(s);
                    } else if VARIANT_IDENTS.contains(&s.as_str()) {
                        variant = Some(s);
                    } else if WEIGHT_IDENTS.contains(&s.as_str()) {
                        weight = Some(s);
                    } else if STRETCH_IDENTS.contains(&s.as_str()) {
                        stretch = Some(s);
                    }
                    continue;
                }
                if SIZE_IDENTS.contains(&s.as_str()) {
                    size = Some(s);
                    continue;
                }
                leftover_start = i - 1;
                break;
            }
            FontToken::Other => continue,
        }
    }

    let family = family_from_tokens(&tokens[leftover_start.min(n)..]);
    Font {
        style,
        variant,
        weight,
        stretch,
        size,
        line_height: height,
        family,
    }
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

    #[test]
    fn parse_font_handles_weight_keyword_size_and_family() {
        let font = parse_font("bold 12px Georgia, serif");
        assert_eq!(font.weight.as_deref(), Some("bold"));
        assert_eq!(font.size.as_deref(), Some("12px"));
        assert_eq!(font.family, vec!["Georgia".to_string(), "serif".to_string()]);
        assert_eq!(serialize_font(&font), "bold 12px Georgia, serif");
    }

    #[test]
    fn parse_font_handles_all_six_slots_plus_line_height() {
        let font = parse_font("italic small-caps bold condensed 16px/1.5 Georgia, serif");
        assert_eq!(font.style.as_deref(), Some("italic"));
        assert_eq!(font.variant.as_deref(), Some("small-caps"));
        assert_eq!(font.weight.as_deref(), Some("bold"));
        assert_eq!(font.stretch.as_deref(), Some("condensed"));
        assert_eq!(font.size.as_deref(), Some("16px"));
        assert_eq!(font.line_height.as_deref(), Some("1.5"));
        assert_eq!(font.family, vec!["Georgia".to_string(), "serif".to_string()]);
        assert_eq!(
            serialize_font(&font),
            "italic small-caps bold condensed 16px/1.5 Georgia, serif"
        );
    }

    #[test]
    fn parse_font_accepts_a_numeric_weight() {
        let font = parse_font("700 12px Georgia");
        assert_eq!(font.weight.as_deref(), Some("700"));
        assert_eq!(font.size.as_deref(), Some("12px"));
        assert_eq!(font.family, vec!["Georgia".to_string()]);
    }

    #[test]
    fn parse_font_maps_legacy_system_font_keywords_to_sans_serif() {
        for legacy in ["caption", "icon", "menu", "message-box", "small-caption", "status-bar"] {
            let font = parse_font(legacy);
            assert_eq!(font.family, vec!["sans-serif".to_string()]);
            assert!(font.style.is_none());
            assert!(font.size.is_none());
        }
    }

    #[test]
    fn parse_font_a_lone_global_keyword_fills_the_first_slot() {
        let font = parse_font("normal");
        assert_eq!(font.style.as_deref(), Some("normal"));
        assert!(font.family.is_empty());
        assert_eq!(serialize_font(&font), "normal");
    }

    #[test]
    fn parse_font_drops_a_before_size_keyword_appearing_after_the_size_matching_upstream() {
        // Faithful replication of a real tinycss.fonts3.parse_font quirk:
        // once `size` is set, a BEFORE_SIZE_IDENTS keyword (like "bold")
        // encountered afterwards is silently discarded -- not set as the
        // weight, and not treated as part of the family list either.
        let font = parse_font("12px bold Georgia");
        assert_eq!(font.size.as_deref(), Some("12px"));
        assert!(font.weight.is_none());
        assert_eq!(font.family, vec!["Georgia".to_string()]);
    }

    #[test]
    fn parse_font_handles_a_size_only_keyword_with_family() {
        let font = parse_font("large Georgia");
        assert_eq!(font.size.as_deref(), Some("large"));
        assert_eq!(font.family, vec!["Georgia".to_string()]);
    }

    #[test]
    fn serialize_font_omits_line_height_when_size_is_absent() {
        let font = Font {
            line_height: Some("1.5".to_string()),
            family: vec!["serif".to_string()],
            ..Default::default()
        };
        assert_eq!(serialize_font(&font), "serif");
    }
}
