//! Narrow port of `old_src/src/calibre/utils/smartypants.py`'s
//! `smartyPants(text, attr='1')` default path -- the only entry point
//! `oeb::polish::replace::smarten_punctuation` needs (`replace.py`
//! imports `calibre.ebooks.conversion.preprocess.smarten_punctuation`,
//! which is itself just `smartyPants(html)` plus a `<!--`/`-->` guard and
//! a final `xml_replace_entities` pass -- see [`super::replace::smarten_punctuation`]).
//!
//! # Scope: heuristic quote classification, not a regex-for-regex port
//!
//! Python's `educateQuotes` is a pipeline of ~15 sequential regexes,
//! several relying on lookahead/lookbehind assertions (`(?<=\W)`,
//! `(?!\s|s\b|\d)`, ...). The `regex` crate this workspace uses has no
//! lookaround support at all (the same constraint
//! [`crate::css::model`]'s `rename_class_in_text` already documents and
//! works around for a single assertion by capturing and re-emitting the
//! matched context). Re-deriving Python's entire pipeline that way would
//! mean fifteen-plus capture-and-reinsert regexes for marginal fidelity
//! gain. Instead this is a direct character-classification scan: for
//! each `'`/`"`, look at the immediately preceding and following
//! (already-substituted) character to decide "opening" vs "closing",
//! which is the same rule of thumb real-world smart-quote implementations
//! use and matches Python's own regexes for the overwhelming majority of
//! real prose. Two of Python's explicit special cases are preserved
//! (decade abbreviations `'80s` -> `’80s`; a quote directly after a
//! letter/digit is always closing, covering contractions like `Isn't`).
//! **Not preserved** (rare in real book text, and each would need its
//! own lookaround-shaped regex to match Python exactly): the
//! double-quote-adjacent-to-single-quote combos (`"'`, `'"`, `""`, `''`),
//! feet/inches marks (`19' 43.5"`), and backtick-style quotes (`` `` ``/
//! `` ` ``). `educateDashes`/`educateEllipses`/`processEscapes` are
//! plain substring replacement in Python and are ported verbatim.

use std::sync::OnceLock;

use regex::Regex;

/// Port of `_tokenize`: splits `html` into alternating `Text`/`Tag`
/// tokens on `<[^>]*>` boundaries (Python's `tag_soup` regex).
enum Token<'a> {
    Text(&'a str),
    Tag(&'a str),
}

fn tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[^<]*<[^>]*>").unwrap())
}

fn tokenize(html: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let mut last_end = 0usize;
    for m in tag_re().find_iter(html) {
        let whole = m.as_str();
        // `whole` is `text` followed by `<tag>`; split at the last `<`.
        let tag_start = whole.rfind('<').unwrap();
        let text_part = &whole[..tag_start];
        let tag_part = &whole[tag_start..];
        if !text_part.is_empty() {
            tokens.push(Token::Text(text_part));
        }
        tokens.push(Token::Tag(tag_part));
        last_end = m.end();
    }
    if last_end < html.len() {
        tokens.push(Token::Text(&html[last_end..]));
    }
    tokens
}

/// Port of `tags_to_skip_regex`: elements whose text content must not be
/// touched.
fn skippable_tag_name(tag: &str) -> Option<(bool, String)> {
    let re = {
        static RE: OnceLock<Regex> = OnceLock::new();
        RE.get_or_init(|| Regex::new(r"(?i)^<(/)?(style|pre|code|kbd|script|math)[^>]*>$").unwrap())
    };
    let caps = re.captures(tag)?;
    let is_close = caps.get(1).is_some();
    let name = caps.get(2)?.as_str().to_ascii_lowercase();
    Some((is_close, name))
}

fn is_self_closing(tag: &str) -> bool {
    tag.trim_end_matches('>').trim_end().ends_with('/')
}

/// Port of `processEscapes`'s six two-character replacements, in the
/// same order Python does (`\\` first, so later passes don't re-match
/// its output).
fn escapes(text: &str) -> String {
    let text = text.replace("\\\\", "&#92;");
    let text = text.replace("\\\"", "&#34;");
    let text = text.replace("\\'", "&#39;");
    let text = text.replace("\\.", "&#46;");
    let text = text.replace("\\-", "&#45;");
    text.replace("\\`", "&#96;")
}

/// Port of `educateDashes`.
fn educate_dashes(text: &str) -> String {
    let text = text.replace("---", "&#8211;");
    text.replace("--", "&#8212;")
}

/// Port of `educateEllipses`.
fn educate_ellipses(text: &str) -> String {
    let text = text.replace("...", "&#8230;");
    text.replace(". . .", "&#8230;")
}

/// Port of `educateBackticks`.
fn educate_backticks(text: &str) -> String {
    let text = text.replace("``", "&#8220;");
    text.replace("''", "&#8221;")
}

fn is_open_context(c: Option<char>) -> bool {
    match c {
        None => true,
        Some(c) => c.is_whitespace() || "([{-\u{2013}\u{2014}".contains(c),
    }
}

/// Heuristic replacement for `educateQuotes` -- see the module docs for
/// what is and isn't preserved from Python's regex pipeline.
fn educate_quotes(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 8);
    // Tracks the last *emitted* plain character (post-substitution),
    // matching Python's `prev_token_last_char` cheat for single-quote
    // tokens, generalized to every quote in the stream.
    let mut prev_plain: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            let next = chars.get(i + 1).copied();
            let is_decade = c == '\''
                && is_open_context(prev_plain)
                && next.map(|n| n.is_ascii_digit()).unwrap_or(false)
                && chars
                    .get(i + 2)
                    .map(|n| n.is_ascii_digit())
                    .unwrap_or(false)
                && chars.get(i + 3).map(|&n| n == 's').unwrap_or(true);
            let after_word_char = prev_plain.map(|p| p.is_alphanumeric()).unwrap_or(false);
            let opening = !is_decade && !after_word_char && is_open_context(prev_plain);
            if c == '\'' {
                out.push_str(if opening { "&#8216;" } else { "&#8217;" });
            } else {
                out.push_str(if opening { "&#8220;" } else { "&#8221;" });
            }
            // The entity substitution's last plain character, for the
            // *next* iteration's context, is the quote glyph itself
            // (matches Python treating the curly quote as ordinary
            // non-whitespace, non-open-punct text).
            prev_plain = Some('”');
        } else {
            out.push(c);
            prev_plain = Some(c);
        }
        i += 1;
    }
    out
}

/// Applies the default (`attr='1'`: dashes + ellipses + backticks +
/// quotes) educating pipeline to a single non-tag text run, matching the
/// per-token body of Python's `smartyPants` loop.
fn educate_text_run(t: &str) -> String {
    let t = escapes(t);
    let t = t.replace("&quot;", "\"");
    let t = educate_dashes(&t);
    let t = educate_ellipses(&t);
    let t = educate_backticks(&t);
    educate_quotes(&t)
}

/// Port of `smartyPants(html)` (default `attr='1'`): dashes, ellipses,
/// backtick-quotes, and curly quotes, skipping tag markup and the
/// content of `style`/`pre`/`code`/`kbd`/`script`/`math` elements. See
/// the module docs for the quote-educating fidelity tradeoff.
pub fn smarten_punctuation_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() + 32);
    let mut skip_stack: Vec<String> = Vec::new();
    for token in tokenize(html) {
        match token {
            Token::Tag(tag) => {
                out.push_str(tag);
                if let Some((is_close, name)) = skippable_tag_name(tag) {
                    if !is_self_closing(tag) {
                        if !is_close {
                            skip_stack.push(name);
                        } else if skip_stack.last() == Some(&name) {
                            skip_stack.pop();
                        }
                    }
                }
            }
            Token::Text(text) => {
                if skip_stack.is_empty() {
                    out.push_str(&educate_text_run(text));
                } else {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn educates_simple_quoted_sentence() {
        // Matches Python's `smartyPants`, which emits numeric HTML
        // entities, not literal curly-quote characters.
        let out = smarten_punctuation_html("<p>\"Isn't this fun?\"</p>");
        assert_eq!(out, "<p>&#8220;Isn&#8217;t this fun?&#8221;</p>");
    }

    #[test]
    fn educates_dashes_and_ellipses() {
        assert_eq!(smarten_punctuation_html("a--b"), "a&#8212;b");
        assert_eq!(smarten_punctuation_html("a---b"), "a&#8211;b");
        assert_eq!(smarten_punctuation_html("Huh...?"), "Huh&#8230;?");
    }

    #[test]
    fn leaves_style_and_script_content_untouched() {
        let out = smarten_punctuation_html("<style>.a::before{content:\"x\"}</style><p>\"hi\"</p>");
        assert!(out.contains("content:\"x\""), "{out}");
        assert!(out.contains("&#8220;hi&#8221;"), "{out}");
    }

    #[test]
    fn decade_abbreviation_uses_closing_quote() {
        let out = smarten_punctuation_html("the '80s");
        assert!(out.contains("&#8217;80s"), "{out}");
    }

    #[test]
    fn no_op_when_nothing_to_smarten() {
        let out = smarten_punctuation_html("<p>plain text</p>");
        assert_eq!(out, "<p>plain text</p>");
    }
}
