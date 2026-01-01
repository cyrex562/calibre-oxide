use regex::Regex;
use lazy_static::lazy_static;
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

lazy_static! {
    static ref TOKEN_REGEX: Regex = Regex::new(r#"(?x)
        (?P<op>[()]) |
        (?P<complex_word>@.+?:[^")\s]+) |
        (?P<word>[^"()\s]+) |
        (?s)(?P<triple_quoted>"{3}(?:\\.|.)*?"{3}) |
        (?P<quoted>"(?:[^"\\]|\\.)*") |
        (?P<ws>\s+)
    "#).unwrap();
}

struct Lexer;

impl Lexer {
    fn tokenize(input: &str) -> Vec<TokenType> {
        let mut tokens = Vec::new();
        let mut pos = 0;
        let mut last_pos = 0;
        
        while pos < input.len() {
            // Rust regex doesn't support "find at" efficiently with sticky semantics easily without 'regex-automata' or creating slices.
            // But creating slices is fine.
            let chunk = &input[pos..];
            if let Some(mat) = TOKEN_REGEX.find(chunk) {
                // Must ensure it matches at start?
                // TOKEN_REGEX doesn't have ^. But we want to consume from start.
                // find() searches anywhere.
                // We should use ^ anchor in regex and match.
                // Re-compile regex with ^? Or just check mat.start() == 0.
                if mat.start() != 0 {
                    // Match found but not at start. This means we have chars not matching any token?
                    // The 'word' pattern [^"()\s]+ should basically match anything else except whitespace/quotes/parens.
                    // So if we found something later, the start is probably unrecognized?
                    // Actually, if we skip unmatched valid chars, we might loop.
                    // Let's assume the regex covers all cases or fallback.
                    // If start != 0, we skip unknown chars? Or error?
                    // Python Scanner skips unmatched? No, it usually returns match.
                    // For now, assume regex covers everything if we include a "catch all" or "word" is broad.
                    // The "word" regex `[^"()\s]+` is quite broad.
                    // The only thing not matched is... nothing really except logic error?
                    // If match is not at 0, we skip `mat.start()` bytes (garbage?).
                    pos += mat.start();
                }
                
                let cap = TOKEN_REGEX.captures(&input[pos..]).unwrap(); // Re-match to get groups? Or use 'mat'.
                // 'find' gives match, 'captures' gives groups. TOKEN_REGEX captures.
                // Optim: just use captures_at checks?
                
                let len = mat.end();
                
                if let Some(m) = cap.name("op") {
                    tokens.push(TokenType::OpCode(m.as_str().chars().next().unwrap()));
                } else if let Some(m) = cap.name("complex_word") {
                    tokens.push(TokenType::Word(m.as_str().to_string()));
                } else if let Some(m) = cap.name("word") {
                    tokens.push(TokenType::Word(m.as_str().to_string()));
                } else if let Some(m) = cap.name("triple_quoted") {
                    let s = m.as_str();
                    let content = &s[3..s.len()-3];
                    let unescaped = content.replace(r#"\""#, "\"").replace(r#"\\"#, "\\"); // Basic unescape
                    // Python tokenizer treats triple quoted string as... word?
                    // In the test `t(r#"""a\1b"""#, &[w(r#"a\1b"#)]);`, `w` produces `TokenType::Word`.
                    // So triple quoted strings should be emitted as Word? Or QuotedWord?
                    // The test helper `w` creates `TokenType::Word`.
                    // The test expects `Word` for `"""..."""`.
                    // But for `"..."` it expects `QuotedWord`.
                    // Wait, `t("\"a...\"", &[qw(...)])`.
                    // `t(r#"""..."""#, &[w(...)])`.
                    // So multiline/triple quoted strings are treated as Words?
                    // Let's check `search_query_parser_test.py`:
                    // `t(r'"""a\1b"""', 'W', r'a\1b')` -> W is WORD.
                    // `t('"a \\" () b"', 'Q', 'a " () b')` -> Q is QUOTED_WORD.
                    tokens.push(TokenType::Word(unescaped));
                } else if let Some(m) = cap.name("quoted") {
                    // Strip quotes and unescape
                    let s = m.as_str();
                    let content = &s[1..s.len()-1];
                    let unescaped = content.replace(r#"\""#, "\"").replace(r#"\\"#, "\\");
                    tokens.push(TokenType::QuotedWord(unescaped));
                }
                // Skip ws
                
                pos += len;
                if pos == last_pos {
                    // Avoid infinite loop if len 0
                    pos += 1; 
                }
                last_pos = pos;
            } else {
                // No match, skip 1 char
                pos += 1;
            }
        }
        tokens
    }
}

pub struct Parser {
    tokens: Vec<TokenType>,
    current: usize,
    locations: Vec<String>,
}

impl Parser {
    pub fn new(locations: Vec<String>) -> Self {
        Parser { tokens: Vec::new(), current: 0, locations }
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
        if self.is_eof() { None } else { Some(&self.tokens[self.current]) }
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
    
    fn or_expression(&mut self) -> Result<SearchNode, ParseError> {
        let mut lhs = self.and_expression()?;
        
        while let Some(s) = self.lcase_token() {
             if s == "or" {
                 self.advance();
                 let rhs = self.or_expression()?; // Recursion for right-assoc? Python: ['or', lhs, self.or_expression()]
                 // Yes, right recursive.
                 lhs = SearchNode::Or(Box::new(lhs), Box::new(rhs));
                 return Ok(lhs);
             }
             break;
        }
        Ok(lhs)
    }
    
    fn and_expression(&mut self) -> Result<SearchNode, ParseError> {
        let mut lhs = self.not_expression()?;
        
        loop {
            let token = self.lcase_token();
            if let Some(s) = token {
                if s == "and" {
                    self.advance();
                    let rhs = self.and_expression()?;
                    lhs = SearchNode::And(Box::new(lhs), Box::new(rhs));
                    return Ok(lhs); // Return immediately for right recursion?
                }
                // Implicit AND
                if s != "or" {
                    // Check if strictly start of next param
                    // Python: if ((self.token_type() in [WORD, QUOTED_WORD] or self.token() == '(') and self.lcase_token() != 'or'):
                    // Here we checked != 'or' already.
                    let is_start = match self.peek().unwrap() {
                        TokenType::Word(_) | TokenType::QuotedWord(_) => true,
                        TokenType::OpCode('(') => true,
                        _ => false
                    };
                    
                    if is_start {
                         let rhs = self.and_expression()?;
                         lhs = SearchNode::And(Box::new(lhs), Box::new(rhs));
                         return Ok(lhs);
                    }
                }
            }
            break;
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
            _ => Err(ParseError("Invalid syntax. Expected a lookup name or a word".to_string()))
        }
    }
    
    fn base_token(&mut self) -> Result<SearchNode, ParseError> {
        // Python logic:
        // if quoted: return token('all', quoted)
        // else split by ':'
        
        if let Some(TokenType::QuotedWord(s)) = self.peek() {
            let s = s.clone();
            self.advance();
            return Ok(SearchNode::Token { location: "all".to_string(), query: s });
        }
        
        if let Some(TokenType::Word(s)) = self.peek() {
            let s = s.clone();
            self.advance();
            
            let parts: Vec<&str> = s.split(':').collect();
            // Python: "The complexity here comes from... colon-separated search values."
            // "if len(words) > 1 and words[0].lower() in self.locations"
            // "loc = words[0]"
            // "words = words[1:]"
            // "if len(words) == 1 and token_type == QUOTED_WORD" (next token) -> consume quoted
            // else join words with :
            
            // Check locations
            if parts.len() > 1 {
                 let possible_loc = parts[0].to_lowercase();
                 if self.locations.contains(&possible_loc) {
                     let loc = possible_loc;
                     let remainder = parts[1..].join(":");
                     
                     if remainder.is_empty() {
                         // Check next token for quoted word?
                         // Python: "if len(words) == 1" (which is [empty] if s ends with :)
                         // Wait, s.split(':')? "author:".split(':') -> ["author", ""]
                         // If remainder is empty, we look ahead?
                         // Python code: words = words[1:]. "if len(words) == 1 and token() == QUOTED".
                         // If words[0] was loc, words[1:] is the rest.
                         // If input was "author:", parts=["author", ""]. words=[""].
                         if let Some(TokenType::QuotedWord(q)) = self.peek() {
                             // "author:" "foo" -> loc="author", query="foo"
                             let q = q.clone();
                             self.advance();
                             return Ok(SearchNode::Token { location: loc, query: q });
                         }
                     }
                     return Ok(SearchNode::Token { location: loc, query: remainder });
                 }
            }
            
            // Default
            return Ok(SearchNode::Token { location: "all".to_string(), query: s });
        }
        
        Err(ParseError("Unexpected error in base_token".to_string()))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer() {
        let t = |query: &str, expected: &[TokenType]| {
            assert_eq!(Lexer::tokenize(query), expected);
        };

        // Helper to construct tokens
        fn w(s: &str) -> TokenType { TokenType::Word(s.to_string()) }
        fn qw(s: &str) -> TokenType { TokenType::QuotedWord(s.to_string()) }
        fn op(c: char) -> TokenType { TokenType::OpCode(c) }

        t("xxx", &[w("xxx")]);
        t("\"a \\\" () b\"", &[qw("a \" () b")]);
        t("\"a“b\"", &[qw("a“b")]);
        t("\"a”b\"", &[qw("a”b")]);
        
        t(r#"""a\1b"""#, &[w(r#"a\1b"#)]);
        
        // ("""a\1b""" AND """c""" OR d)
        t(r#"("""a\1b""" AND """c""" OR d)"#, &[
            op('('), w(r#"a\1b"#), w("AND"), w("c"), w("OR"), w("d"), op(')')
        ]);
        
        t("template:=\"\"\"a\\1b\"\"\"", &[w("template:=a\\1b")]);
    }
}
