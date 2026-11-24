//! Port of `old_src/src/calibre/ebooks/rtf2xml/tokenize.py`
//! (`Tokenize`).
//!
//! Splits raw (post line-ending-fixed, post illegal-char-stripped) RTF
//! source into one token per line -- the input [`super::process_tokens`]
//! consumes. This is **not** the same tokenizer as
//! `crate::rtf::preprocess::RtfTokenizer` (issue #50's port of
//! `calibre/ebooks/rtf/preprocess.py`): that one builds a structured
//! token *enum* for a different, narrower conversion path, whereas this
//! one is rtf2xml's own regex-substitution-and-split pipeline that
//! produces a flat `Vec<String>` of already-mostly-normalized text
//! tokens, one per output line, with a from-scratch treatment of
//! `\u` Unicode escapes (see [`process_unicode_tokens`]). Free
//! functions here (`tokenize`, `split_into_tokens`,
//! `process_unicode_tokens`) are used instead of a `Tokenizer` struct
//! specifically to avoid inviting confusion with
//! `crate::rtf::preprocess::RtfTokenizer`.

use lazy_static::lazy_static;
use regex::{Captures, Regex};
use thiserror::Error;

/// Port of the uncaught `ValueError` Python's `chr()` would raise for
/// an out-of-range code point -- see [`process_unicode_tokens`]'s docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("invalid unicode code point in \\u token: {0}")]
pub struct InvalidUnicodeCodePoint(pub i64);

// ---- Port of `Tokenize.__compile_expressions` ----

lazy_static! {
    // Port of `self.__ms_hex_exp`.
    static ref MS_HEX_EXP: Regex = Regex::new(r"\\'([0-9a-fA-F]{2})").unwrap();
    // Port of `self.__utf_exp`.
    static ref UTF_EXP: Regex = Regex::new(r"\\u(-?\d{3,6}) ?").unwrap();
    // Anchored variant of UTF_EXP for `__unicode_process`'s `.match()`
    // (Python's `re.match` only matches at the start of the string).
    static ref UTF_EXP_ANCHORED: Regex = Regex::new(r"^\\u(-?\d{3,6}) ?").unwrap();
    // Port of `self.__bin_exp`.
    static ref BIN_EXP: Regex = Regex::new(r"(?:\\bin(-?\d{0,10})[\n ]+)[01\n]+").unwrap();
    // Port of `self.__utf_ud`. See this constant's own doc comment
    // below (and the tests) for a confirmed upstream quirk: the `\\*`
    // fragment is *not* an escaped literal asterisk (that would need
    // `\\\*`), it is an escaped backslash followed by an *unescaped*
    // `*` quantifier -- i.e. "zero or more backslash characters", not
    // "a literal `\*`". That is preserved verbatim below (only the
    // brace characters are escaped, since Rust's `regex` crate is
    // stricter than Python's `re` about bare `{`/`}`).
    static ref UTF_UD: Regex = Regex::new(
        r"\\\{[\n ]?\\upr[\n ]?(?:\\\{.*?\\\})[\n ]?\\\{[\n ]?\\*[\n ]?\\ud[\n ]?(\\\{.*?\\\})[\n ]?\\\}[\n ]?\\\}"
    ).unwrap();
    // Port of `self.__splitexp`.
    static ref SPLIT_EXP: Regex =
        Regex::new(r"(\\[{}]|\n|\\[^\s\\{}&]+(?:[ \t\r\x0b\x0c])?)").unwrap();
    // Port of `self.__par_exp`.
    static ref PAR_EXP: Regex = Regex::new(r"(\\\n+|\\ )").unwrap();
    // Port of `self.__cs_ast`.
    static ref CS_AST: Regex = Regex::new(r"\\\*([\n ]*\\cs\d+[\n \\]+)").unwrap();
    // Port of `self.__cwdigit_exp`.
    static ref CWDIGIT_EXP: Regex = Regex::new(r"(\\[a-zA-Z]+[\-0-9]+)([^0-9 \\]+)").unwrap();
}

/// Port of `Tokenize.__compile_expressions`'s `SIMPLE_RPL` dict, applied
/// via `MReplace` (`calibre.utils.mreplace`, not itself one of this
/// issue's ten files). `MReplace` builds one alternation regex from the
/// dict's keys sorted longest-first and substitutes each match with its
/// dict value in a single pass -- reproduced directly here rather than
/// porting the generic `MReplace` helper class, since this is its only
/// use site in scope.
const SIMPLE_RPL: &[(&str, &str)] = &[
    (r"\\", r"\backslash "),
    (r"\~", r"\~ "),
    (r"\;", r"\; "),
    ("&", "&amp;"),
    ("<", "&lt;"),
    (">", "&gt;"),
    (r"\_", r"\_ "),
    (r"\:", r"\: "),
    (r"\-", r"\- "),
    (r"\{", r"\ob "),
    (r"\}", r"\cb "),
    ("{", r"\{"),
    ("}", r"\}"),
];

lazy_static! {
    static ref SIMPLE_RPL_EXP: Regex = {
        let mut keys: Vec<&str> = SIMPLE_RPL.iter().map(|(k, _)| *k).collect();
        keys.sort_by_key(|k| std::cmp::Reverse(k.len()));
        let pattern = format!(
            "({})",
            keys.iter()
                .map(|k| regex::escape(k))
                .collect::<Vec<_>>()
                .join("|")
        );
        Regex::new(&pattern).unwrap()
    };
}

fn simple_replace(input: &str) -> String {
    SIMPLE_RPL_EXP
        .replace_all(input, |caps: &Captures| {
            let matched = &caps[0];
            SIMPLE_RPL
                .iter()
                .find(|(k, _)| *k == matched)
                .map(|(_, v)| *v)
                .unwrap_or(matched)
                .to_string()
        })
        .into_owned()
}

/// Port of `Tokenize.__sub_reg_split`: the chain of regex substitutions
/// and the final capturing split that turns raw RTF source into a flat
/// list of tokens (empty tokens and bare `"\n"` tokens already
/// filtered out, matching the Python's
/// `filter(lambda x: len(x) > 0 and x != '\n', tokens)`).
pub fn split_into_tokens(input: &str) -> Vec<String> {
    let s = simple_replace(input);
    let s = PAR_EXP.replace_all(&s, "\n\\par \n");
    let s = CWDIGIT_EXP.replace_all(&s, "${1}\n${2}");
    let s = CS_AST.replace_all(&s, "${1}");
    let s = MS_HEX_EXP.replace_all(&s, "\\mshex0${1} ");
    let s = UTF_UD.replace_all(&s, "\\{\\uc0 ${1}\\}");
    let s = BIN_EXP.replace_all(&s, |caps: &Captures| {
        format!("{}\n", caps[0].replace('\n', ""))
    });

    // Port of `re.split(self.__splitexp, input_file)`: Python's
    // `re.split` with a capturing pattern keeps the captured delimiter
    // text interspersed with the text between matches. Rust's `regex`
    // crate has no direct equivalent, so this walks matches manually,
    // pushing (text-before-match, matched-text) pairs and the trailing
    // remainder -- exactly what `re.split` does when (as here) the
    // whole pattern is wrapped in one outer group.
    let mut tokens = Vec::new();
    let mut last_end = 0;
    for m in SPLIT_EXP.find_iter(&s) {
        tokens.push(s[last_end..m.start()].to_string());
        tokens.push(m.as_str().to_string());
        last_end = m.end();
    }
    tokens.push(s[last_end..].to_string());

    tokens.retain(|t| !t.is_empty() && t != "\n");
    tokens
}

/// Port of `Tokenize.__reini_utf8_counters` + `__remove_uc_chars` +
/// `__unicode_process`'s per-token state.
struct UnicodeState {
    uc_char: i64,
    uc_bin: bool,
    uc_value: Vec<i64>,
}

impl UnicodeState {
    fn new() -> Self {
        UnicodeState {
            uc_char: 0,
            uc_bin: false,
            uc_value: vec![1],
        }
    }

    fn reinit_counters(&mut self) {
        self.uc_char = 0;
        self.uc_bin = false;
    }

    /// Port of `__remove_uc_chars`.
    fn remove_uc_chars<'a>(&mut self, start: usize, token: &'a str) -> &'a str {
        let chars: Vec<(usize, char)> = token.char_indices().collect();
        for &(byte_idx, _) in chars.iter().skip(start) {
            if self.uc_char > 0 {
                self.uc_char -= 1;
            } else {
                return &token[byte_idx..];
            }
        }
        ""
    }
}

/// Converts a `\u` token's resolved code point to what Python's
/// `chr(uni_char).encode('ascii', 'xmlcharrefreplace').decode('ascii')`
/// produces: the literal character itself if it is ASCII (`< 128`),
/// else an XML numeric character reference `&#NNN;` using the raw
/// numeric value -- deliberately *not* routed through a real Rust
/// `char`, because RTF commonly emits UTF-16 surrogate halves as
/// separate `\u` tokens (for astral-plane characters), and lone
/// surrogate code points (0xD800-0xDFFF) are not valid `char` scalar
/// values in Rust even though Python's `str`/`chr()` freely allows
/// them. Bypassing `char` entirely reproduces the Python's per-token,
/// no-surrogate-pairing behavior exactly.
fn unicode_char_ref(code_point: i64) -> Result<String, InvalidUnicodeCodePoint> {
    if code_point < 0 {
        // Port of `chr()` raising `ValueError` for a negative
        // argument -- ported as a proper error instead of panicking
        // (see this crate's fault-tolerance convention).
        return Err(InvalidUnicodeCodePoint(code_point));
    }
    if code_point < 128 {
        // Safe: any value in 0..128 is a valid Unicode scalar value.
        Ok(char::from_u32(code_point as u32).unwrap().to_string())
    } else {
        Ok(format!("&#{code_point};"))
    }
}

/// Port of `Tokenize.__unicode_process`, applied to each token in
/// order (state threads across calls, exactly like the Python's
/// `self.__uc_char`/`self.__uc_bin`/`self.__uc_value` instance fields).
///
/// # Preserved-as-unreachable quirk
///
/// The Python has an `if token[:4] == '\bin':` branch inside the
/// `elif self.__uc_char:` arm that is unconditionally dead: `'\bin'`
/// (not a raw string) is the 3-character string `'\x08in'`
/// (backspace + `"in"`), and a 4-character slice can never equal a
/// 3-character string, so the comparison is always `False` (verified:
/// `'\bin'` has `len() == 3`). Execution therefore always falls through
/// to the next `elif token[:1] == '\\':` arm. This function implements
/// only the reachable behavior -- the always-false branch is omitted
/// rather than transcribed as dead code that could never execute here
/// either.
fn unicode_process_token(
    state: &mut UnicodeState,
    token: &str,
) -> Result<String, InvalidUnicodeCodePoint> {
    if token == r"\{" {
        let top = *state.uc_value.last().unwrap_or(&1);
        state.uc_value.push(top);
        state.reinit_counters();
        return Ok(token.to_string());
    }
    if token == r"\}" {
        if !state.uc_value.is_empty() {
            state.uc_value.pop();
        }
        state.reinit_counters();
        return Ok(token.to_string());
    }
    if let Some(rest) = token.strip_prefix(r"\uc") {
        // Port of `self.__uc_value[-1] = int(token[3:])`. A `\uc`
        // token with a non-numeric (or missing) argument would make
        // Python's `int()` raise an uncaught `ValueError`; real RTF
        // never emits `\uc` without a numeric argument, so this
        // no-ops rather than crashing, per this crate's convention of
        // never panicking on malformed input.
        if let Ok(value) = rest.parse::<i64>() {
            if let Some(last) = state.uc_value.last_mut() {
                *last = value;
            } else {
                state.uc_value.push(value);
            }
        }
        state.reinit_counters();
        return Ok(token.to_string());
    }
    if state.uc_bin {
        state.uc_bin = false;
        return Ok(String::new());
    }
    if state.uc_char > 0 {
        // See "Preserved-as-unreachable quirk" above: the Python's
        // `\bin` special case never fires, so this always takes the
        // `token[:1] == '\\'` path when it applies.
        if token.starts_with('\\') {
            state.uc_char -= 1;
            return Ok(String::new());
        }
        return Ok(state.remove_uc_chars(0, token).to_string());
    }

    if let Some(caps) = UTF_EXP_ANCHORED.captures(token) {
        state.reinit_counters();
        let raw: i64 = caps[1].parse().unwrap_or(0);
        let uni_len = caps[0].len();
        let mut uni_char_value = raw;
        if uni_char_value < 0 {
            uni_char_value += 65536;
        }
        let uni_char = unicode_char_ref(uni_char_value)?;
        state.uc_char = *state.uc_value.last().unwrap_or(&1);

        if token.len() <= uni_len {
            return Ok(uni_char);
        }
        if state.uc_char == 0 {
            return Ok(format!("{uni_char}{}", &token[uni_len..]));
        }
        let remainder = state.remove_uc_chars(uni_len, token);
        return Ok(format!("{uni_char}{remainder}"));
    }

    Ok(token.to_string())
}

/// Port of `tokens = map(self.__unicode_process, tokens); tokens =
/// list(filter(lambda x: len(x) > 0, tokens))`.
pub fn process_unicode_tokens(tokens: Vec<String>) -> Result<Vec<String>, InvalidUnicodeCodePoint> {
    let mut state = UnicodeState::new();
    let mut out = Vec::with_capacity(tokens.len());
    for token in tokens {
        let processed = unicode_process_token(&mut state, &token)?;
        if !processed.is_empty() {
            out.push(processed);
        }
    }
    Ok(out)
}

/// Port of `Tokenize.tokenize`'s in-memory transformation (the
/// temp-file / `Copy` / rename dance around it is pipeline plumbing,
/// not ported here -- see [`super::copy`] for that helper). Returns the
/// token stream joined by `\n`, matching `'\n'.join(tokens)`.
pub fn tokenize(input: &str) -> Result<String, InvalidUnicodeCodePoint> {
    let tokens = split_into_tokens(input);
    let tokens = process_unicode_tokens(tokens)?;
    Ok(tokens.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every fixture below was cross-checked against a live run of the
    // real Python `Tokenize` class (stubbing only its two out-of-scope
    // dependencies, `calibre.ptempfile.better_mktemp` and
    // `calibre.utils.mreplace.MReplace`, neither of which change
    // behavior) to guard against hand-derived regex misreadings.

    // ---- split_into_tokens: simple replacements ----

    #[test]
    fn bare_braces_become_group_delimiter_tokens() {
        // Bare `{`/`}` (real RTF group delimiters) become `\{`/`\}`
        // tokens -- *not* `\ob `/`\cb `, which is reserved for
        // already-escaped literal brace characters (see the next
        // test). Verified: `run_tokenize('{\\b bold}')` ==
        // `'\\{\n\\b \nbold\n\\}'`.
        let tokens = split_into_tokens(r"{\b bold}");
        assert_eq!(
            tokens,
            vec![
                r"\{".to_string(),
                r"\b ".to_string(),
                "bold".to_string(),
                r"\}".to_string(),
            ]
        );
    }

    #[test]
    fn escaped_literal_braces_become_ob_cb_tokens() {
        // An *already-escaped* `\{`/`\}` in the source (a literal
        // brace character in RTF text, not a group delimiter) becomes
        // `\ob `/`\cb ` instead.
        let tokens = split_into_tokens(r"\{x\}");
        assert_eq!(
            tokens,
            vec![r"\ob ".to_string(), "x".to_string(), r"\cb ".to_string()]
        );
    }

    #[test]
    fn ampersand_and_angle_brackets_are_escaped() {
        let tokens = split_into_tokens("a & b < c > d");
        let joined = tokens.join("");
        assert!(joined.contains("&amp;"));
        assert!(joined.contains("&lt;"));
        assert!(joined.contains("&gt;"));
    }

    #[test]
    fn backslash_becomes_backslash_token() {
        let tokens = split_into_tokens(r"\\");
        assert!(tokens.iter().any(|t| t.contains("backslash")));
    }

    // ---- split_into_tokens: control words ----

    #[test]
    fn control_word_with_trailing_space_is_a_single_token() {
        let tokens = split_into_tokens(r"\par ");
        assert!(tokens.contains(&r"\par ".to_string()));
    }

    #[test]
    fn hex_escape_becomes_mshex_token() {
        let tokens = split_into_tokens(r"\'e9");
        assert!(tokens.iter().any(|t| t.starts_with(r"\mshex0e9")));
    }

    #[test]
    fn cwdigit_exp_splits_digit_argument_from_trailing_letters() {
        // `\f1abc` (no space delimiter): the digit-argument control
        // word and the trailing letters get split onto their own
        // token via a synthesized newline. Verified:
        // `run_tokenize('\\f1abc')` == `'\\f1\nabc'`.
        let tokens = split_into_tokens(r"\f1abc");
        assert_eq!(tokens, vec![r"\f1".to_string(), "abc".to_string()]);
    }

    // ---- unicode_process: `\u<N>` needs 3-6 digits ----

    #[test]
    fn a_two_digit_u_argument_never_matches_and_passes_through_unchanged() {
        // `self.__utf_exp = re.compile(r'\\u(-?\d{3,6}) ?')` requires
        // *at least 3* digits, so `\u65` (2 digits, 'A' in decimal) is
        // never recognized as a unicode escape at all -- verified:
        // `run_tokenize('\\u65 ')` == `'\\u65 '` (unchanged).
        assert_eq!(tokenize(r"\u65 ").unwrap(), r"\u65 ");
    }

    #[test]
    fn ascii_unicode_token_becomes_the_literal_char() {
        // A leading zero satisfies the 3-digit minimum without
        // changing the decoded value (`int("065") == 65`). Verified:
        // `run_tokenize('\\u065 ')` == `'A'`.
        let out = tokenize(r"\u065 ").unwrap();
        assert_eq!(out, "A");
    }

    #[test]
    fn non_ascii_unicode_token_becomes_xml_numeric_ref() {
        let out = tokenize(r"\u955 ").unwrap();
        assert_eq!(out, "&#955;");
    }

    #[test]
    fn negative_unicode_value_wraps_via_65536_offset() {
        // -100 + 65536 = 65436, non-ASCII -> numeric ref. Verified:
        // `run_tokenize('\\u-100 ')` == `'&#65436;'`.
        let out = tokenize(r"\u-100 ").unwrap();
        assert_eq!(out, "&#65436;");
    }

    // ---- unicode_process: fallback-char consumption across tokens ----

    #[test]
    fn uc_default_of_one_drops_the_single_following_text_token() {
        // Default `\uc` value is 1: exactly one fallback token/char
        // following a recognized `\u` escape is dropped. Verified:
        // `run_tokenize('\\u100 X')` == `'d'` (the trailing "X" token
        // is entirely swallowed, not appended).
        assert_eq!(tokenize(r"\u100 X").unwrap(), "d");
    }

    #[test]
    fn uc_n_drops_n_chars_of_a_single_trailing_text_token() {
        // `\uc2` -> 2 fallback chars consumed from the very next text
        // token ("XYZ", tokenized as one run since it contains no
        // whitespace/backslash/brace/ampersand): 'X' and 'Y' are
        // dropped, 'Z' survives. Input tokens here match exactly what
        // `split_into_tokens("\\uc2\\u100 XYZ")` itself produces
        // (three tokens: the `\uc2` control word, `\u100 ` with its
        // trailing space delimiter, and the separate plain-text run
        // `XYZ`). Verified: `run_tokenize('\\uc2\\u100 XYZ')` ==
        // `'\\uc2\nd\nZ'`.
        let tokens = vec![
            r"\uc2".to_string(),
            r"\u100 ".to_string(),
            "XYZ".to_string(),
        ];
        let processed = process_unicode_tokens(tokens).unwrap();
        assert_eq!(
            processed,
            vec![r"\uc2".to_string(), "d".to_string(), "Z".to_string()]
        );
    }

    #[test]
    fn fallback_consumption_can_also_happen_within_one_combined_token() {
        // `\u955 ABC`: "ABC" trails directly after the `\u955 `
        // control word with no further backslash/brace in between, so
        // splitexp actually hands `__unicode_process` two tokens
        // (`"\u955 "` and `"ABC"`) -- the one-char default `\uc`
        // fallback consumes just the leading 'A' from the second.
        // Verified: `run_tokenize('\\u955 ABC')` == `'&#955;\nBC'`.
        assert_eq!(tokenize(r"\u955 ABC").unwrap(), "&#955;\nBC");
    }

    #[test]
    fn a_bare_group_delimiter_token_is_never_swallowed_as_a_fallback_char() {
        // `token == r'\}'` (a real group-close delimiter) is checked
        // *before* the fallback-consuming `uc_char` branch in the
        // Python's if/elif chain, so it always survives -- unlike an
        // escaped literal `\cb ` (see the next test), which is not an
        // exact match for `r'\}'` and therefore falls through to the
        // generic (and here, swallowing) `token[:1] == '\\'` check.
        let tokens = vec![
            r"\{".to_string(),
            r"\uc1".to_string(),
            r"\u065 ".to_string(),
            r"\}".to_string(),
        ];
        let processed = process_unicode_tokens(tokens).unwrap();
        assert_eq!(
            processed,
            vec![
                r"\{".to_string(),
                r"\uc1".to_string(),
                "A".to_string(),
                r"\}".to_string(),
            ]
        );
    }

    #[test]
    fn an_escaped_literal_brace_token_can_be_swallowed_as_a_fallback_char() {
        // Escaped literal braces (`\{x\}`, tokenizing to `\ob `/`x`/
        // `\cb `) do *not* exactly equal `r'\{'`/`r'\}'`, so
        // `\cb ` here is treated as an ordinary backslash token and
        // consumed by the pending `\uc1` fallback from `\u065`.
        // Verified: `run_tokenize('\\{\\uc1\\u065 \\}')` ==
        // `'\\ob \n\\uc1\nA'` (no trailing `\cb ` at all).
        let out = tokenize(r"\{\uc1\u065 \}").unwrap();
        assert_eq!(out, "\\ob \n\\uc1\nA");
    }

    #[test]
    fn uc_zero_keeps_all_following_text() {
        let tokens = vec![r"\uc0".to_string(), r"\u065 XYZ".to_string()];
        let processed = process_unicode_tokens(tokens).unwrap();
        assert_eq!(processed, vec![r"\uc0".to_string(), "AXYZ".to_string()]);
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(tokenize("").unwrap(), "");
    }

    // ---- utf_ud: confirmed-dead-on-real-RTF quirk ----

    #[test]
    fn utf_ud_never_matches_genuine_upr_ud_rtf_because_of_a_backslash_star_typo() {
        // See `UTF_UD`'s own doc comment: its `\\*` fragment means
        // "zero or more literal backslashes", not "a literal `\*`",
        // so it can never consume the `\*` that real
        // `{\upr{...}{\*\ud{...}}}` RTF constructs always contain
        // (the RTF spec's "ignorable primary destination" marker).
        // Verified against a live run of the Python on exactly this
        // real-world-shaped input: the substitution simply never
        // fires, and every token passes through unmolested.
        let out = tokenize(r"{\upr{fallback}{\*\ud{unicode}}}").unwrap();
        assert_eq!(
            out,
            "\\{\n\\upr\n\\{\nfallback\n\\}\n\\{\n\\*\n\\ud\n\\{\nunicode\n\\}\n\\}\n\\}"
        );
    }

    #[test]
    fn utf_ud_matches_the_asterisk_free_variant_the_regex_actually_accepts() {
        // The same shape *without* the `\*` (never produced by real
        // RTF, but demonstrates the substitution is not simply inert
        // code -- it does fire for what its `\\*` fragment can
        // actually match: zero backslashes). Verified against a live
        // run of the Python.
        let out = tokenize(r"{\upr{fallback}{\ud{unicode}}}").unwrap();
        assert_eq!(out, "\\{\n\\uc0 \n\\{\nunicode\n\\}\n\\}");
    }
}
