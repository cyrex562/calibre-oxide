//! Port of `old_src/src/calibre/utils/search_query_parser.py`'s
//! `Parser` (the tokenizer/grammar the `db/search.py`
//! `SearchQueryParser` subclass builds on).
//!
//! # The tokenizer (`Lexer`): fixed for real, not just patched
//!
//! The previous version of this file tried to tokenize in one pass
//! with a single combined alternation regex, including a
//! `triple_quoted` alternative to handle `"""..."""` docstrings
//! inline. That doesn't match what upstream actually does, and it's
//! why #134's bug existed: upstream's `Parser.tokenize` is a
//! **three-step pipeline**, not a single scan:
//!
//! 1. **Docstring extraction**: `"""..."""` spans are found first
//!    (`re.sub(r'(""")(..*?)(""")', ...)`, DOTALL) and replaced with a
//!    sentinel (`docstring_sep`, three specific Unicode characters
//!    chosen because they won't appear naturally) wrapping the
//!    span's content hex-encoded. This makes the span's contents --
//!    including any quotes or backslashes *inside* it -- completely
//!    opaque to every later step; they're restored byte-for-byte at
//!    the very end, never escape-processed at all.
//! 2. **Escape neutralization**: literal `\\`, `\"`, `\(`, `\)` in
//!    what's left are replaced with single control characters
//!    (U+0001..U+0004) so the scanner's patterns -- which look for
//!    literal `"`, `(`, `)` -- can't be confused by an escaped one.
//! 3. **Scanning**: `re.Scanner` tries each pattern *in a fixed
//!    priority order* (op, `@loc:word`, plain word, quoted, then skip
//!    whitespace) anchored at the current position, taking the first
//!    one that matches -- not a single combined-alternation regex
//!    (which is what the old version of this file did, and why a
//!    `quoted` alternative starting with `""` inside a `"""` run could
//!    win over `triple_quoted` and split it into three garbage
//!    tokens, exactly #134's reported failure).
//!
//! Then `unescape` reverses both steps 1 and 2 on each token's text.
//! [`Lexer::tokenize`] mirrors this exact pipeline instead of trying
//! to fold it into one regex.
//!
//! Once preprocessing has run, the `quoted` pattern doesn't need
//! upstream's `(?<!\\)"` negative-lookbehind (unsupported by the
//! plain `regex` crate): step 2 has already consumed every `\"`
//! sequence, so a literal `"` immediately following a real `\` can no
//! longer occur inside the text being scanned -- a plain non-greedy
//! `".*?"` is equivalent post-preprocessing.

use lazy_static::lazy_static;
use regex::Regex;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum SearchNode {
    And(Box<SearchNode>, Box<SearchNode>),
    Or(Box<SearchNode>, Box<SearchNode>),
    Not(Box<SearchNode>),
    Token { location: String, query: String },
    // "all" is explicit location
}

// Token types
#[derive(Debug, Clone, PartialEq)]
enum TokenType {
    OpCode(char),
    Word(String),
    QuotedWord(String),
}

#[derive(Debug, Clone)]
pub struct ParseError(String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ParseError: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

/// Port of `Parser.docstring_sep`: three characters chosen because
/// they won't appear naturally in a search query (Unicode white
/// square, Tibetan letter A (om), Arabic-Indic cube root).
const DOCSTRING_SEP: &str = "\u{25a1}\u{0f00}\u{0606}";

lazy_static! {
    static ref DOCSTRING_RE: Regex = Regex::new(r#"(?s)"{3}(.+?)"{3}"#).unwrap();
    static ref DOCSTRING_RESTORE_RE: Regex = {
        let sep = regex::escape(DOCSTRING_SEP);
        Regex::new(&format!(r"(?s){sep}(.+?){sep}")).unwrap()
    };
    static ref OP_RE: Regex = Regex::new(r"^[()]").unwrap();
    static ref COMPLEX_WORD_RE: Regex = Regex::new(r#"(?s)^@.+?:[^")\s]+"#).unwrap();
    static ref WORD_RE: Regex = Regex::new(r#"^[^"()\s]+"#).unwrap();
    static ref QUOTED_RE: Regex = Regex::new(r#"(?s)^".*?""#).unwrap();
    static ref WS_RE: Regex = Regex::new(r"^\s+").unwrap();
}

/// Port of `Parser.REPLACEMENTS`: `('\\' + x, chr(i + 1))` for `i, x`
/// in `enumerate('\\"()')` -- i.e. the two-character escape sequences
/// `\\`, `\"`, `\(`, `\)`, each mapped to one of the first four
/// control characters.
const REPLACEMENTS: [(&str, char); 4] = [
    ("\\\\", '\u{1}'),
    ("\\\"", '\u{2}'),
    ("\\(", '\u{3}'),
    ("\\)", '\u{4}'),
];

fn hex_encode(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

fn hex_decode(s: &str) -> String {
    let bytes: Vec<u8> = s
        .as_bytes()
        .chunks(2)
        .filter_map(|c| {
            std::str::from_utf8(c)
                .ok()
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

struct Lexer;

impl Lexer {
    /// Port of `Parser.tokenize`. See the module docs for the
    /// three-step pipeline this mirrors.
    fn tokenize(input: &str) -> Vec<TokenType> {
        let expr = DOCSTRING_RE
            .replace_all(input, |caps: &regex::Captures| {
                format!("{DOCSTRING_SEP}{}{DOCSTRING_SEP}", hex_encode(&caps[1]))
            })
            .into_owned();

        let mut expr = expr;
        for (k, v) in REPLACEMENTS {
            expr = expr.replace(k, &v.to_string());
        }

        scan(&expr)
            .into_iter()
            .map(|t| match t {
                TokenType::OpCode(c) => TokenType::OpCode(c),
                TokenType::Word(s) => TokenType::Word(unescape(&s)),
                TokenType::QuotedWord(s) => TokenType::QuotedWord(unescape(&s)),
            })
            .collect()
    }
}

/// Port of `Parser.lex_scanner`'s `re.Scanner` behavior: at each
/// position, try every pattern *in this fixed order* and take the
/// first one that matches there -- not a single alternation regex
/// (see the module docs for why that distinction matters).
fn scan(expr: &str) -> Vec<TokenType> {
    let mut tokens = Vec::new();
    let mut rest = expr;
    while !rest.is_empty() {
        if let Some(m) = OP_RE.find(rest) {
            tokens.push(TokenType::OpCode(m.as_str().chars().next().unwrap()));
            rest = &rest[m.end()..];
        } else if let Some(m) = COMPLEX_WORD_RE.find(rest) {
            tokens.push(TokenType::Word(m.as_str().to_string()));
            rest = &rest[m.end()..];
        } else if let Some(m) = WORD_RE.find(rest) {
            tokens.push(TokenType::Word(m.as_str().to_string()));
            rest = &rest[m.end()..];
        } else if let Some(m) = QUOTED_RE.find(rest) {
            let s = m.as_str();
            tokens.push(TokenType::QuotedWord(s[1..s.len() - 1].to_string()));
            rest = &rest[m.end()..];
        } else if let Some(m) = WS_RE.find(rest) {
            rest = &rest[m.end()..];
        } else {
            // No pattern matches at this position -- matches
            // `re.Scanner`, which would stop and report the remainder
            // as unconsumed leftover. `word`'s `[^"()\s]+` is broad
            // enough that this shouldn't happen for well-formed input.
            break;
        }
    }
    tokens
}

/// Port of `Parser.tokenize`'s inner `unescape`: reverses docstring
/// extraction (hex-decoding the sentinel-wrapped span back to its
/// original, byte-for-byte, unprocessed content) and escape
/// neutralization (control chars back to their two-character escape
/// sequences, minus the leading backslash -- `x.replace(v, k[1:])`).
fn unescape(x: &str) -> String {
    let x = DOCSTRING_RESTORE_RE
        .replace_all(x, |caps: &regex::Captures| hex_decode(&caps[1]))
        .into_owned();
    let mut x = x;
    for (k, v) in REPLACEMENTS {
        x = x.replace(v, &k[1..]);
    }
    x
}

pub struct Parser {
    tokens: Vec<TokenType>,
    current: usize,
    locations: Vec<String>,
}

impl Parser {
    pub fn new(locations: Vec<String>) -> Self {
        Parser {
            tokens: Vec::new(),
            current: 0,
            locations,
        }
    }

    pub fn parse(&mut self, query: &str) -> Result<SearchNode, ParseError> {
        // Tokenize
        self.tokens = Lexer::tokenize(query);
        self.current = 0;

        // Parse
        let prog = self.or_expression()?;
        if !self.is_eof() {
            return Err(ParseError("Extra characters at end of search".to_string()));
        }
        Ok(prog)
    }

    fn is_eof(&self) -> bool {
        self.current >= self.tokens.len()
    }

    fn peek(&self) -> Option<&TokenType> {
        if self.is_eof() {
            None
        } else {
            Some(&self.tokens[self.current])
        }
    }

    fn advance(&mut self) {
        self.current += 1;
    }

    fn lcase_token(&self) -> Option<String> {
        self.peek().map(|t| match t {
            TokenType::Word(s) => s.to_lowercase(),
            TokenType::QuotedWord(s) => s.to_lowercase(),
            TokenType::OpCode(c) => c.to_string(), // op code string
        })
    }

    /// Port of `or_expression`: right-recursive, matching upstream's
    /// `['or', lhs, self.or_expression()]`.
    fn or_expression(&mut self) -> Result<SearchNode, ParseError> {
        let lhs = self.and_expression()?;

        if self.lcase_token().as_deref() == Some("or") {
            self.advance();
            let rhs = self.or_expression()?;
            return Ok(SearchNode::Or(Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    /// Port of `and_expression`, including the optional/implicit `and`
    /// between two adjacent tokens (`"author:Asimov tag:unread"` reads
    /// the same as `"author:Asimov and tag:unread"`).
    fn and_expression(&mut self) -> Result<SearchNode, ParseError> {
        let lhs = self.not_expression()?;

        if let Some(s) = self.lcase_token() {
            if s == "and" {
                self.advance();
                let rhs = self.and_expression()?;
                return Ok(SearchNode::And(Box::new(lhs), Box::new(rhs)));
            }
            let starts_next_operand = s != "or"
                && matches!(
                    self.peek().unwrap(),
                    TokenType::Word(_) | TokenType::QuotedWord(_) | TokenType::OpCode('(')
                );
            if starts_next_operand {
                let rhs = self.and_expression()?;
                return Ok(SearchNode::And(Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn not_expression(&mut self) -> Result<SearchNode, ParseError> {
        if let Some(s) = self.lcase_token() {
            if s == "not" {
                self.advance();
                let expr = self.not_expression()?;
                return Ok(SearchNode::Not(Box::new(expr)));
            }
        }
        self.location_expression()
    }

    fn location_expression(&mut self) -> Result<SearchNode, ParseError> {
        if let Some(TokenType::OpCode('(')) = self.peek() {
            self.advance();
            let expr = self.or_expression()?;
            if let Some(TokenType::OpCode(')')) = self.peek() {
                self.advance();
                return Ok(expr);
            } else {
                return Err(ParseError("missing )".to_string()));
            }
        }

        match self.peek() {
            Some(TokenType::Word(_)) | Some(TokenType::QuotedWord(_)) => self.base_token(),
            _ => Err(ParseError(
                "Invalid syntax. Expected a lookup name or a word".to_string(),
            )),
        }
    }

    fn base_token(&mut self) -> Result<SearchNode, ParseError> {
        if let Some(TokenType::QuotedWord(s)) = self.peek() {
            let s = s.clone();
            self.advance();
            return Ok(SearchNode::Token {
                location: "all".to_string(),
                query: s,
            });
        }

        if let Some(TokenType::Word(s)) = self.peek() {
            let s = s.clone();
            self.advance();

            let parts: Vec<&str> = s.split(':').collect();

            // We have a location if there is more than one word and the
            // first word is in locations (matches upstream's comment
            // about "author: \"foo\"" being interpreted as
            // "author:\"foo\"" -- a known, accepted quirk, not fixed
            // here either).
            if parts.len() > 1 {
                let possible_loc = parts[0].to_lowercase();
                if self.locations.contains(&possible_loc) {
                    let loc = possible_loc;
                    let remainder = parts[1..].join(":");

                    if remainder.is_empty() {
                        if let Some(TokenType::QuotedWord(q)) = self.peek() {
                            let q = q.clone();
                            self.advance();
                            return Ok(SearchNode::Token {
                                location: loc,
                                query: q,
                            });
                        }
                    }
                    return Ok(SearchNode::Token {
                        location: loc,
                        query: remainder,
                    });
                }
            }

            // Default
            return Ok(SearchNode::Token {
                location: "all".to_string(),
                query: s,
            });
        }

        Err(ParseError("Unexpected error in base_token".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str) -> TokenType {
        TokenType::Word(s.to_string())
    }
    fn qw(s: &str) -> TokenType {
        TokenType::QuotedWord(s.to_string())
    }
    fn op(c: char) -> TokenType {
        TokenType::OpCode(c)
    }

    fn t(query: &str, expected: &[TokenType]) {
        assert_eq!(Lexer::tokenize(query), expected, "tokenizing {query:?}");
    }

    /// Every case here is transcribed directly from upstream's own
    /// `test_sqp_tokenizer` (`search_query_parser_test.py`), not
    /// invented -- this is cross-validation against the real test
    /// suite, not just re-testing whatever this port happens to do.
    #[test]
    fn test_tokenizer_matches_upstream_test_sqp_tokenizer() {
        t("xxx", &[w("xxx")]);
        t("\"a \\\" () b\"", &[qw("a \" () b")]);
        t("\"a\u{201c}b\"", &[qw("a\u{201c}b")]);
        t("\"a\u{201d}b\"", &[qw("a\u{201d}b")]);

        // docstring tests
        //
        // Note the `r##"..."##` delimiter (not `r#"..."#`): these
        // queries contain a literal `"""`, and a single-hash raw
        // string closes at the *first* `"#` it finds -- which a `"""`
        // run supplies well before the intended end, silently
        // truncating the literal (the exact class of bug already hit
        // once this session in `fb2_input_test.rs`; verified with a
        // standalone `rustc` check before use here, not just assumed).
        t(r##""""a\1b""""##, &[w(r#"a\1b"#)]);
        t(
            r##"("""a\1b""" AND """c""" OR d)"##,
            &[
                op('('),
                w(r#"a\1b"#),
                w("AND"),
                w("c"),
                w("OR"),
                w("d"),
                op(')'),
            ],
        );
        t(r##"template:="""a\1b""""##, &[w(r#"template:=a\1b"#)]);
        t("template:=\"\"\"a\nb\"\"\"", &[w("template:=a\nb")]);
        t(r##"template:"""=a\1b""""##, &[w(r#"template:=a\1b"#)]);
        t(
            "template:\"\"\"program: return (\"\\\"1\\\"\")#@#n:1\"\"\"",
            &[w("template:program: return (\"\\\"1\\\"\")#@#n:1")],
        );
    }

    #[test]
    fn tokenizer_handles_at_prefixed_complex_words() {
        // `@loc:word` -- the `complex_word` pattern, checked before
        // the plain `word` pattern so a leading `@` doesn't get
        // treated as an ordinary character mid-word differently.
        t("@author:Asimov", &[w("@author:Asimov")]);
    }

    #[test]
    fn parser_builds_and_or_not_trees_matching_upstream_semantics() {
        let mut p = Parser::new(vec!["author".to_string(), "tag".to_string()]);
        let node = p.parse("author:Asimov and tag:unread").unwrap();
        assert_eq!(
            node,
            SearchNode::And(
                Box::new(SearchNode::Token {
                    location: "author".to_string(),
                    query: "Asimov".to_string()
                }),
                Box::new(SearchNode::Token {
                    location: "tag".to_string(),
                    query: "unread".to_string()
                }),
            )
        );

        // Implicit AND (no operator between two tokens).
        let node2 = p.parse("author:Asimov tag:unread").unwrap();
        assert_eq!(node, node2);

        let node3 = p.parse("author:Asimov or author:Hardy").unwrap();
        assert_eq!(
            node3,
            SearchNode::Or(
                Box::new(SearchNode::Token {
                    location: "author".to_string(),
                    query: "Asimov".to_string()
                }),
                Box::new(SearchNode::Token {
                    location: "author".to_string(),
                    query: "Hardy".to_string()
                }),
            )
        );

        let node4 = p.parse("not tag:read").unwrap();
        assert_eq!(
            node4,
            SearchNode::Not(Box::new(SearchNode::Token {
                location: "tag".to_string(),
                query: "read".to_string()
            }))
        );
    }

    #[test]
    fn parser_defaults_to_all_location_for_unrecognized_prefix() {
        let mut p = Parser::new(vec!["author".to_string()]);
        // "unknown" isn't a registered location, so the whole thing is
        // one "all" token, not location="unknown".
        let node = p.parse("unknown:Asimov").unwrap();
        assert_eq!(
            node,
            SearchNode::Token {
                location: "all".to_string(),
                query: "unknown:Asimov".to_string()
            }
        );
    }

    #[test]
    fn parser_errors_on_unmatched_paren() {
        let mut p = Parser::new(vec!["author".to_string()]);
        assert!(p.parse("(author:Asimov").is_err());
    }
}
