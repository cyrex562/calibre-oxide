//! Port of `formatter.py`'s `cached_lex_scanner` (a Python
//! `re.Scanner`): tokenizes calibre template-language source text.
//!
//! `re.Scanner` tries each rule *in list order* at the current
//! position and takes the first one that matches there (ordinary
//! regex alternation priority, not longest-match) -- this port
//! replicates that exactly by trying each compiled rule, anchored to
//! the current byte offset via a fresh `\A`-prefixed match against the
//! remaining slice, in the same order upstream lists them.

use fancy_regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Op,
    Id,
    Const,
    StringInfix,
    NumericInfix,
    Keyword,
    Newline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
}

enum Emit {
    Token(TokenKind),
    /// The comment-swallowing newline rule and the plain-newline rule
    /// both conditionally emit a `Newline` token or nothing -- matches
    /// upstream's own lambda bodies exactly (`LEX_NEWLINE if t=='\n' else None`).
    NewlineOnly,
    Skip,
}

struct Rule {
    re: Regex,
    emit: Emit,
    /// Strip the outer quote characters from a matched string constant
    /// (upstream's `t[1:-1]` in the two quoted-string lambdas).
    strip_quotes: bool,
}

fn rules() -> &'static Vec<Rule> {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        // `(?s)` == Python's `re.DOTALL` for the whole scanner: `.`
        // matches `\n` too (relevant for multi-line quoted strings and
        // the comment-to-end-of-line pattern's `.*?`).
        let mk = |pat: &str, emit: Emit, strip_quotes: bool| Rule { re: Regex::new(&format!(r"(?s)\A(?:{pat})")).unwrap(), emit, strip_quotes };
        vec![
            mk(r"(?:==#|!=#|<=#|<#|>=#|>#)", Emit::Token(TokenKind::NumericInfix), false),
            mk(r"(?:==|!=|<=|<|>=|>)", Emit::Token(TokenKind::StringInfix), false),
            mk(r"(?:if|then|else|elif|fi)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:for|in|rof|separator)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:separator|limit)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:def|fed|continue)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:return|inlist|break)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:inlist_field)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:with|htiw)\b", Emit::Token(TokenKind::Keyword), false),
            mk(r"(?:\|\||&&|!|\{|\})", Emit::Token(TokenKind::Op), false),
            mk(r"[(),=;:\+\-*/&]", Emit::Token(TokenKind::Op), false),
            mk(r"-?[\d\.]+", Emit::Token(TokenKind::Const), false),
            mk(r"\$\$?#?\w+", Emit::Token(TokenKind::Id), false),
            mk(r"\$", Emit::Token(TokenKind::Id), false),
            mk(r"\w+", Emit::Token(TokenKind::Id), false),
            mk(r#"".*?(?:(?<!\\)")"#, Emit::Token(TokenKind::Const), true),
            mk(r"'.*?(?:(?<!\\)')", Emit::Token(TokenKind::Const), true),
            mk(r"\n[ \t]*#.*?(?:(?=\n)|$)", Emit::NewlineOnly, false),
            mk(r"\s", Emit::Skip, false),
        ]
    })
}

/// Port of `LEX_NEWLINE`'s two conditional-emit lambdas: the
/// whitespace rule only emits a token for an actual `\n`, and the
/// comment rule always emits one (it can only match if it consumed a
/// real `\n` at its start).
fn is_newline_text(text: &str) -> bool {
    text.starts_with('\n')
}

/// Tokenizes `src`, or returns the byte offset of the first
/// unrecognized character -- matches upstream's own scanner contract
/// (`re.Scanner.scan` returns `(tokens, remainder)`; a non-empty
/// remainder means scanning stopped early, checked by `_Parser.program`
/// via `if prog[1] != '': self.error(...)`).
pub fn scan(src: &str) -> Result<Vec<Token>, usize> {
    let mut tokens = Vec::new();
    let mut pos = 0usize;
    let rules = rules();
    'outer: while pos < src.len() {
        let rest = &src[pos..];
        for rule in rules {
            if let Ok(Some(m)) = rule.re.find(rest) {
                if m.start() == 0 {
                    let matched = m.as_str();
                    match &rule.emit {
                        Emit::Token(kind) => {
                            let text = if rule.strip_quotes {
                                let chars: Vec<char> = matched.chars().collect();
                                chars[1..chars.len() - 1].iter().collect::<String>()
                            } else {
                                matched.to_string()
                            };
                            tokens.push(Token { kind: *kind, text });
                        }
                        Emit::NewlineOnly => {
                            if is_newline_text(matched) {
                                tokens.push(Token { kind: TokenKind::Newline, text: matched.to_string() });
                            }
                        }
                        Emit::Skip => {
                            if is_newline_text(matched) {
                                tokens.push(Token { kind: TokenKind::Newline, text: matched.to_string() });
                            }
                        }
                    }
                    pos += matched.len();
                    continue 'outer;
                }
            }
        }
        return Err(pos);
    }
    Ok(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<(TokenKind, String)> {
        scan(src).unwrap().into_iter().map(|t| (t.kind, t.text)).collect()
    }

    #[test]
    fn tokenizes_a_simple_field_reference() {
        assert_eq!(kinds("field('title')"), vec![(TokenKind::Id, "field".into()), (TokenKind::Op, "(".into()), (TokenKind::Const, "title".into()), (TokenKind::Op, ")".into())]);
    }

    #[test]
    fn distinguishes_string_and_numeric_infix_operators() {
        assert_eq!(kinds("=="), vec![(TokenKind::StringInfix, "==".into())]);
        assert_eq!(kinds("==#"), vec![(TokenKind::NumericInfix, "==#".into())]);
        assert_eq!(kinds(">="), vec![(TokenKind::StringInfix, ">=".into())]);
        assert_eq!(kinds(">=#"), vec![(TokenKind::NumericInfix, ">=#".into())]);
    }

    #[test]
    fn recognizes_keywords_before_falling_back_to_identifiers() {
        assert_eq!(kinds("if"), vec![(TokenKind::Keyword, "if".into())]);
        assert_eq!(kinds("iffy"), vec![(TokenKind::Id, "iffy".into())], "'iffy' must not be split as keyword 'if' + id 'fy' -- \\b boundary must hold");
    }

    #[test]
    fn handles_dollar_field_shorthand_forms() {
        assert_eq!(kinds("$title"), vec![(TokenKind::Id, "$title".into())]);
        assert_eq!(kinds("$$title"), vec![(TokenKind::Id, "$$title".into())]);
        assert_eq!(kinds("$"), vec![(TokenKind::Id, "$".into())]);
    }

    #[test]
    fn strips_quotes_and_respects_backslash_escaped_quotes() {
        assert_eq!(kinds(r#""a \" b""#), vec![(TokenKind::Const, "a \\\" b".into())]);
        assert_eq!(kinds(r"'a \' b'"), vec![(TokenKind::Const, "a \\' b".into())]);
    }

    #[test]
    fn double_quoted_strings_can_span_multiple_lines() {
        // DOTALL: `.` inside the quoted-string pattern matches `\n`.
        assert_eq!(kinds("\"line1\nline2\""), vec![(TokenKind::Const, "line1\nline2".into())]);
    }

    #[test]
    fn a_leading_hash_comment_swallows_through_just_before_the_next_newline() {
        // The comment rule's lookahead `(?=\n)` stops it right before
        // the *following* real newline rather than consuming it, so
        // that newline is tokenized separately on the next pass by
        // the plain-whitespace rule -- two Newline tokens, not one.
        // Harmless for parsing: `check_eol()` just loops consuming
        // every consecutive Newline token regardless of count.
        let toks = kinds("a\n  # a comment\nb");
        assert_eq!(toks, vec![(TokenKind::Id, "a".into()), (TokenKind::Newline, "\n  # a comment".into()), (TokenKind::Newline, "\n".into()), (TokenKind::Id, "b".into())]);
    }

    #[test]
    fn plain_whitespace_between_tokens_is_skipped_except_newlines() {
        assert_eq!(kinds("a   b"), vec![(TokenKind::Id, "a".into()), (TokenKind::Id, "b".into())]);
        assert_eq!(kinds("a\nb"), vec![(TokenKind::Id, "a".into()), (TokenKind::Newline, "\n".into()), (TokenKind::Id, "b".into())]);
    }

    #[test]
    fn a_number_immediately_followed_by_letters_splits_into_two_tokens() {
        // Real upstream quirk: `-?[\d\.]+` (CONST) is tried before the
        // `\w+` (ID) rule, so it greedily eats only the leading
        // digits/dots, leaving the letters for the next token.
        assert_eq!(kinds("1abc"), vec![(TokenKind::Const, "1".into()), (TokenKind::Id, "abc".into())]);
    }

    #[test]
    fn a_leading_minus_sign_tokenizes_separately_from_the_number() {
        // The CONST rule's own `-?` prefix is effectively unreachable
        // in practice: the single-char-op rule (which matches bare
        // `-`) is tried first in upstream's own rule list and always
        // wins at a position where a lone `-` sits before a digit, so
        // a leading minus becomes its own Op token and unary-minus is
        // handled at the *parser* level (`unary_plus_minus_expr`),
        // not by the lexer swallowing the sign into the constant.
        assert_eq!(kinds("-3.5"), vec![(TokenKind::Op, "-".into()), (TokenKind::Const, "3.5".into())]);
    }

    #[test]
    fn reports_the_offset_of_an_unrecognized_character() {
        // `@` isn't matched by any rule.
        assert_eq!(scan("a @ b"), Err(2));
    }
}
