//! Port of `tinycss.media3`'s real CSS3 Media Queries grammar (issue
//! #581): `MediaQuery` and `CSSMedia3Parser.parse_media`. Like
//! [`super::fonts3`], this is plain token-stream parsing over a flat
//! (with one level of `(...)`-group nesting) token sequence, not
//! routed through [`crate::css`]'s stylesheet object model.
//!
//! Faithfully replicates `parse_media`'s real error-recovery behavior:
//! a malformed expression inside one comma-separated query resets
//! *that query* to `(media_type: "all", negated: true, expressions:
//! [])` and continues with the rest of the list, rather than failing
//! the whole parse.

use cssparser::{Parser, ParserInput, Token};

/// Port of `MediaQuery`.
#[derive(Clone, Debug, PartialEq)]
pub struct MediaQuery {
    pub media_type: String,
    pub expressions: Vec<(String, Option<MediaValue>)>,
    pub negated: bool,
}

impl MediaQuery {
    fn all() -> Self {
        MediaQuery {
            media_type: "all".to_string(),
            expressions: Vec::new(),
            negated: false,
        }
    }
}

/// A media-feature expression's value. Upstream keeps the raw token
/// object (of whatever type follows the `:`); this collapses
/// INTEGER/NUMBER/DIMENSION/PERCENTAGE into one `Number` variant
/// carrying the numeric value and an optional unit (`None` for a bare
/// number, `Some("px")`/`Some("%")` for a dimension/percentage) -- a
/// reasonable simplification since real callers need the numeric
/// value, not which of those four token types it came from. `RATIO`
/// (upstream's own special case, e.g. `16/9` in
/// `(device-aspect-ratio: 16/9)`) is kept as its own variant.
#[derive(Clone, Debug, PartialEq)]
pub enum MediaValue {
    Ident(String),
    Number { value: f32, unit: Option<String> },
    Ratio(f32, f32),
    /// A feature value of a type the grammar doesn't otherwise handle
    /// (e.g. a lone `:`/delimiter after the colon) -- upstream would
    /// keep the raw token in this case too, but nothing about the
    /// grammar's own rules constrains what it can be.
    Other,
}

#[derive(Clone, Debug)]
enum GroupToken {
    Ident(String),
    Colon,
    Integer(i64),
    Number { value: f32, unit: Option<String> },
    Delim(char),
    Other,
}

#[derive(Clone, Debug)]
enum MediaToken {
    Ident(String),
    Group(Vec<GroupToken>),
    /// A container token that isn't `(...)` -- `FUNCTION(...)`, `[...]`,
    /// `{...}` -- kept only to produce the same "must be in parentheses"
    /// error class upstream does, not the group's contents.
    OtherContainer,
    /// Any other, non-container token type.
    Other,
}

fn to_group_token(tok: &Token) -> GroupToken {
    match tok {
        Token::Ident(s) => GroupToken::Ident(s.to_string()),
        Token::Colon => GroupToken::Colon,
        Token::Delim(c) => GroupToken::Delim(*c),
        Token::Number { value, int_value, .. } => match int_value {
            Some(iv) => GroupToken::Integer(*iv as i64),
            None => GroupToken::Number { value: *value, unit: None },
        },
        Token::Dimension { value, unit, .. } => GroupToken::Number {
            value: *value,
            unit: Some(unit.to_string()),
        },
        Token::Percentage { unit_value, .. } => GroupToken::Number {
            value: unit_value * 100.0,
            unit: Some("%".to_string()),
        },
        _ => GroupToken::Other,
    }
}

/// Tokenizes `css_string` into top-level comma-separated parts, each a
/// sequence of [`MediaToken`]s -- the "flat with one level of `(...)`
/// grouping" shape `parse_media`'s real algorithm walks.
fn tokenize_media_parts(css_string: &str) -> Vec<Vec<MediaToken>> {
    let mut input = ParserInput::new(css_string.trim());
    let mut parser = Parser::new(&mut input);
    let mut parts: Vec<Vec<MediaToken>> = vec![Vec::new()];
    loop {
        let token = match parser.next() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        match token {
            Token::Comma => parts.push(Vec::new()),
            Token::Ident(s) => parts.last_mut().unwrap().push(MediaToken::Ident(s.to_string())),
            Token::ParenthesisBlock => {
                let group: Vec<GroupToken> = parser
                    .parse_nested_block(|input| -> Result<_, cssparser::ParseError<'_, ()>> {
                        let mut content = Vec::new();
                        loop {
                            match input.next() {
                                Ok(t) => content.push(to_group_token(t)),
                                Err(_) => break,
                            }
                        }
                        Ok(content)
                    })
                    .unwrap_or_default();
                parts.last_mut().unwrap().push(MediaToken::Group(group));
            }
            Token::Function(_) | Token::SquareBracketBlock | Token::CurlyBracketBlock => {
                let _ = parser.parse_nested_block(|input| -> Result<(), cssparser::ParseError<'_, ()>> {
                    while input.next().is_ok() {}
                    Ok(())
                });
                parts.last_mut().unwrap().push(MediaToken::OtherContainer);
            }
            _ => parts.last_mut().unwrap().push(MediaToken::Other),
        }
    }
    parts
}

/// Parses one parenthesized group's content (`media-feature` or
/// `media-feature: value`) per `parse_media`'s real rules. `Err` mirrors
/// upstream's `MalformedExpression`.
fn parse_group(content: &[GroupToken]) -> Result<(String, Option<MediaValue>), ()> {
    if content.is_empty() {
        return Err(());
    }
    let feature = match &content[0] {
        GroupToken::Ident(s) => s.clone(),
        _ => return Err(()),
    };
    if content.len() == 1 {
        return Ok((feature, None));
    }
    if content.len() < 3 {
        return Err(());
    }
    if !matches!(content[1], GroupToken::Colon) {
        return Err(());
    }
    let expr = &content[2..];
    let value = if expr.len() == 1 {
        match &expr[0] {
            GroupToken::Ident(s) => MediaValue::Ident(s.clone()),
            GroupToken::Integer(v) => MediaValue::Number {
                value: *v as f32,
                unit: None,
            },
            GroupToken::Number { value, unit } => MediaValue::Number {
                value: *value,
                unit: unit.clone(),
            },
            _ => MediaValue::Other,
        }
    } else if expr.len() == 3 {
        match (&expr[0], &expr[1], &expr[2]) {
            (GroupToken::Integer(a), GroupToken::Delim('/'), GroupToken::Integer(b)) => {
                MediaValue::Ratio(*a as f32, *b as f32)
            }
            _ => return Err(()),
        }
    } else {
        return Err(());
    };
    Ok((feature, Some(value)))
}

/// Port of `CSSMedia3Parser.parse_media`. `errors` collects one message
/// per malformed expression encountered (mirroring upstream's `errors`
/// out-parameter); pass an empty `&mut Vec` and ignore it if the
/// diagnostics aren't needed.
pub fn parse_media(css_string: &str, errors: &mut Vec<String>) -> Vec<MediaQuery> {
    let parts = tokenize_media_parts(css_string);
    // A single, entirely-empty part (the common "no tokens at all" case)
    // means the whole list was empty -- matches upstream's `if not
    // tokens: return [MediaQuery('all')]`.
    if parts.len() == 1 && parts[0].is_empty() {
        return vec![MediaQuery::all()];
    }

    let mut queries = Vec::new();
    for part in parts {
        let mut negated = false;
        let mut media_type: Option<String> = None;
        let mut expressions = Vec::new();
        let mut malformed = false;

        for (i, tok) in part.iter().enumerate() {
            if i == 0 {
                if let MediaToken::Ident(s) = tok {
                    let lower = s.to_ascii_lowercase();
                    if lower == "only" {
                        continue;
                    }
                    if lower == "not" {
                        negated = true;
                        continue;
                    }
                }
            }
            if media_type.is_none() {
                if let MediaToken::Ident(s) = tok {
                    media_type = Some(s.clone());
                    continue;
                }
                media_type = Some("all".to_string());
                // Falls through to process this same token below, matching
                // upstream's lack of `continue` in this branch.
            }
            if let MediaToken::Ident(s) = tok {
                if s.eq_ignore_ascii_case("and") {
                    continue;
                }
            }
            match tok {
                MediaToken::Group(content) => match parse_group(content) {
                    Ok(expr) => expressions.push(expr),
                    Err(_) => {
                        errors.push("malformed media feature definition".to_string());
                        malformed = true;
                        break;
                    }
                },
                MediaToken::OtherContainer => {
                    errors.push("media expressions must be in parentheses".to_string());
                    malformed = true;
                    break;
                }
                _ => {
                    errors.push("expected a media expression".to_string());
                    malformed = true;
                    break;
                }
            }
        }

        if malformed {
            queries.push(MediaQuery {
                media_type: "all".to_string(),
                negated: true,
                expressions: Vec::new(),
            });
        } else {
            queries.push(MediaQuery {
                media_type: media_type.unwrap_or_else(|| "all".to_string()),
                negated,
                expressions,
            });
        }
    }
    queries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Vec<MediaQuery> {
        let mut errors = Vec::new();
        parse_media(s, &mut errors)
    }

    #[test]
    fn empty_input_defaults_to_a_single_all_query() {
        let qs = parse("");
        assert_eq!(qs, vec![MediaQuery::all()]);
    }

    #[test]
    fn a_bare_media_type() {
        let qs = parse("screen");
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].media_type, "screen");
        assert!(!qs[0].negated);
        assert!(qs[0].expressions.is_empty());
    }

    #[test]
    fn only_prefix_is_ignored() {
        let qs = parse("only screen");
        assert_eq!(qs[0].media_type, "screen");
        assert!(!qs[0].negated);
    }

    #[test]
    fn not_prefix_negates() {
        let qs = parse("not screen");
        assert_eq!(qs[0].media_type, "screen");
        assert!(qs[0].negated);
    }

    #[test]
    fn comma_separates_independent_queries() {
        let qs = parse("screen, print");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].media_type, "screen");
        assert_eq!(qs[1].media_type, "print");
    }

    #[test]
    fn a_feature_only_expression_with_no_value() {
        let qs = parse("(color)");
        assert_eq!(qs[0].media_type, "all");
        assert_eq!(qs[0].expressions, vec![("color".to_string(), None)]);
    }

    #[test]
    fn a_dimension_valued_feature_expression() {
        let qs = parse("screen and (min-width: 400px)");
        assert_eq!(qs[0].media_type, "screen");
        assert_eq!(
            qs[0].expressions,
            vec![(
                "min-width".to_string(),
                Some(MediaValue::Number {
                    value: 400.0,
                    unit: Some("px".to_string())
                })
            )]
        );
    }

    #[test]
    fn a_ratio_valued_feature_expression() {
        let qs = parse("(device-aspect-ratio: 16/9)");
        assert_eq!(
            qs[0].expressions,
            vec![("device-aspect-ratio".to_string(), Some(MediaValue::Ratio(16.0, 9.0)))]
        );
    }

    #[test]
    fn an_ident_valued_feature_expression() {
        let qs = parse("(orientation: portrait)");
        assert_eq!(
            qs[0].expressions,
            vec![("orientation".to_string(), Some(MediaValue::Ident("portrait".to_string())))]
        );
    }

    #[test]
    fn malformed_expression_resets_that_query_to_negated_all() {
        let mut errors = Vec::new();
        let qs = parse_media("screen and (min-width:)", &mut errors);
        assert_eq!(qs[0].media_type, "all");
        assert!(qs[0].negated);
        assert!(qs[0].expressions.is_empty());
        assert!(!errors.is_empty());
    }

    #[test]
    fn malformed_query_does_not_affect_other_comma_separated_queries() {
        let qs = parse("screen and (min-width:), print");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].media_type, "all");
        assert!(qs[0].negated);
        assert_eq!(qs[1].media_type, "print");
        assert!(!qs[1].negated);
    }
}
