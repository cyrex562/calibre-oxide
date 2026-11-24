//! RTF tokenizer and token parser.
//!
//! Port of `old_src/src/calibre/ebooks/rtf/preprocess.py`
//! (`RtfTokenizer`, `RtfTokenParser`), v1.0 (1/17/2010), Gerendi Sandor
//! Attila. Tokenizes a raw RTF byte stream into a flat token list, then
//! rewrites it in two post-passes:
//!
//! - [`RtfTokenParser`]'s `process` step: `\'HH` (control symbol
//!   `\'` + 2 hex-digit data) collapses into a [`RtfToken::Char8Bit`].
//! - Its `process_unicode` step: `\u<N>` tokens swallow their RTF-spec
//!   "ucN fallback replacement characters" -- which calibre's
//!   `rtf2xml` pipeline (out of scope for this crate; see
//!   `crate::input::rtf_input`) does not use, so the walked-over
//!   fallback tokens are collected only to be thrown away, matching
//!   the Python's own `# calibre rtf2xml does not support utfreplace`
//!   comment immediately before it discards them.
//!
//! This module is fully self-contained pure logic: no XML, no Qt, no
//! I/O. `RtfTokenizer::tokenize` + `RtfTokenParser::new` +
//! `RtfTokenParser::to_rtf` round-trips real RTF structurally (see the
//! `round_trip_*` tests below) -- exact byte-for-byte equality is not
//! guaranteed for every input, because the Python this ports has a
//! handful of its own quirks, verified against a live run of the
//! original module and preserved here rather than silently fixed:
//!
//! # Preserved upstream quirks
//!
//! - **Numeric control-word arguments can never be negative**, despite
//!   the tokenizer explicitly checking for a leading `-`. The
//!   digit-consuming loop's *first* iteration re-tests the same
//!   character the entry check just matched; when that character is
//!   `-` (not a digit), the loop immediately exits having consumed
//!   nothing, so `tokenEnd == i` and the "does this control word have
//!   digits" check (`tokenEnd < i`) is false. The control word is
//!   emitted with no argument at all, and the `-` (and following
//!   digits) fall through as plain [`RtfToken::Data`] immediately
//!   after it. This does not break round-tripping (`\fi-360` still
//!   reserializes to `\fi-360`, just as two tokens instead of one),
//!   but it does mean [`RtfToken::ControlWordNumeric`]'s `argument` is
//!   never negative in practice. Verified: `RtfTokenizer(r'\fi-360
//!   x')` in the live Python produces `tokenControlWord('\fi')` then
//!   `tokenData('-360 x')`.
//! - **`\bin<N>` binary tokens capture more than their `N` bytes**, and
//!   round-trip to corrupted output. `tokenBinN`'s `data` field is
//!   sliced from the position of the control word's own leading `\`
//!   (not from after its digits), so it contains the literal `\binN`
//!   text *plus* only `N - len("\binN")` of the real payload bytes --
//!   short of the true `N`-byte payload by however many bytes the
//!   control-word text itself took up. `toRTF()` then recomputes
//!   `len(self.data)` from that already-too-long slice and prepends
//!   *another* `\bin<len>` header in front of it, so what comes out is
//!   not what went in. Verified: `RtfTokenizer('\\bin3 XYZrest')` in
//!   the live Python yields one `tokenBinN` with `data == '\bin3 XY'`
//!   (missing the third payload byte and the trailing `rest`
//!   entirely), and `toRTF()` on it returns `'\bin8 \bin3 XY'`. `\bin`
//!   is a legacy RTF construct calibre's own real-world corpus rarely
//!   emits (hex-encoded `\'HH` pairs are the common case), which is
//!   presumably why this was never caught. Ported byte-for-byte
//!   faithfully below, including in [`RtfToken::to_rtf`]'s
//!   [`RtfToken::BinN`] arm.
//! - **Trailing undelimited data is silently dropped.** `tokenize`
//!   only ever flushes a pending data run when it hits `{`, `}`, or a
//!   control character -- there is no flush after the scan loop ends.
//!   Content that runs to the very end of the buffer without one of
//!   those terminators vanishes. Verified: `RtfTokenizer('hello
//!   world')` in the live Python produces zero tokens. Real RTF always
//!   ends in a closing `}` for the outermost group, which does flush,
//!   so this only bites malformed/truncated input; the round-trip
//!   tests below use properly closed fragments for exactly this
//!   reason.
//! - **`tokenUnicode`'s stored `separator` is dead code.**
//!   `tokenUnicode.__init__` records the original `\u` token's
//!   separator, but `toRTF()` never reads it -- it always splices in a
//!   literal `' '` after the numeric argument. A `\u955X` (no space)
//!   input therefore reserializes as `\u955 X` (space inserted).
//!   Harmless for RTF validity (the space is an optional delimiter per
//!   spec either way) but not byte-identical. Verified against the
//!   live Python. Ported as-is: [`RtfToken::Unicode`] keeps its
//!   `separator` field (for fidelity with the token's shape and so
//!   nothing is silently discarded that a future caller might want),
//!   but [`RtfToken::to_rtf`] does not consult it, matching Python.
//! - **`tokenUnicode.toRTF`'s replacement-list loop never advances its
//!   own counter.** Python's loop variable `i` in `toRTF` starts at 0
//!   and is never incremented inside the `for eq in self.eqList` loop,
//!   so the `if i >= ucn: break` guard either fires on the very first
//!   token (when `ucn <= 0`) or never fires at all. In practice this
//!   is unreachable: `processUnicode` always builds `tokenUnicode`
//!   with an empty `eqList` (see "does not support utfreplace" above),
//!   so the loop body never runs regardless. Ported faithfully in
//!   [`RtfToken::to_rtf`]'s [`RtfToken::Unicode`] arm for parity, with
//!   a `round_trip_unicode_fallback_survives_a_non_empty_eq_list` test
//!   documenting that it stays inert even if a caller ever constructs
//!   a `Unicode` token with a non-empty `eq_list` by hand.
//!
//! # Deliberate safety deviations (crash → `Result`)
//!
//! A few inputs make the live Python raise an *uncaught* `IndexError`
//! rather than one of its own `Exception`s with a message (`\'` as the
//! very last token with nothing after it; a `\bin<N>` claiming more
//! bytes than remain in the document; an unbalanced closing `}` with
//! no matching `{`). This crate's porting guidance is to never panic
//! on malformed input, so these are mapped onto existing
//! [`RtfError`] variants (or, for the unbalanced-brace case, onto a
//! saturating stack that simply stops popping at its initial sentinel)
//! instead of panicking. This changes what happens on malformed input,
//! never on well-formed RTF.

use thiserror::Error;

/// Everything that can go wrong tokenizing or parsing an RTF byte
/// stream. Combines the Python `RtfTokenizer`/`RtfTokenParser`'s own
/// raised `Exception`s (messages matched verbatim) with a couple of
/// safety-net variants for inputs that would otherwise panic -- see
/// the module docs' "Deliberate safety deviations" section.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RtfError {
    /// Port of `'Error: Control character found at the end of the
    /// document.'`
    #[error("Error: Control character found at the end of the document.")]
    ControlCharAtEnd,
    /// Port of `f'Error (at:{tokenStart}): Control Word without
    /// end.'`
    #[error("Error (at:{0}): Control Word without end.")]
    ControlWordWithoutEnd(usize),
    /// Port of `f'Error (at:{tokenStart}): Too many digits in control
    /// word numeric argument.'`
    #[error("Error (at:{0}): Too many digits in control word numeric argument.")]
    TooManyDigits(usize),
    /// Port of `f'Error (at:{tokenStart}): Control Word without
    /// numeric argument end.'`
    #[error("Error (at:{0}): Control Word without numeric argument end.")]
    NumericArgumentWithoutEnd(usize),
    /// Port of `'Error: token8bitChar without data.'`. Also raised
    /// (rather than panicking) where the live Python would instead
    /// crash with an uncaught `IndexError` -- a `\'` control symbol as
    /// the very last token in the stream.
    #[error("Error: token8bitChar without data.")]
    Char8BitWithoutData,
    /// Port of `'Error: incorrect utf replacement.'`
    #[error("Error: incorrect utf replacement.")]
    IncorrectUtfReplacement,
    /// Not raised by the Python (which would instead crash with an
    /// uncaught `IndexError` reading past the end of `rtfData`): a
    /// `\bin<N>` control word claims more bytes than remain in the
    /// document.
    #[error("Error: unexpected end of document reading \\bin payload.")]
    UnexpectedEndOfDocument,
}

pub type Result<T> = std::result::Result<T, RtfError>;

/// One RTF token. Port of `preprocess.py`'s `token*` class hierarchy
/// as a tagged union -- this crate's convention for small ASTs (see
/// `crate::css::model::Rule` or `crate::pml::pmlconverter`'s own token
/// types) over a `Vec<Box<dyn Trait>>`.
///
/// Control-word/symbol names and separators are `String` (assumed
/// ASCII, which is what the tokenizer itself only ever produces for
/// `name`/`separator` -- control words are restricted to ASCII letters
/// by the scan loop below). Raw payload bytes ([`RtfToken::Data`],
/// [`RtfToken::BinN`]'s `data`, [`RtfToken::Char8Bit`]) are `Vec<u8>`
/// for full binary fidelity (RTF data runs and `\bin` payloads are not
/// text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RtfToken {
    /// Port of `tokenDelimitatorStart`: `{`.
    DelimStart,
    /// Port of `tokenDelimitatorEnd`: `}`.
    DelimEnd,
    /// Port of `tokenControlWord`.
    ControlWord { name: String, separator: String },
    /// Port of `tokenControlWordWithNumericArgument`.
    ControlWordNumeric {
        name: String,
        argument: i64,
        separator: String,
    },
    /// Port of `tokenControlSymbol`.
    ControlSymbol { name: String },
    /// Port of `tokenData`.
    Data(Vec<u8>),
    /// Port of `tokenBinN`. See the module docs' "`\bin<N>` binary
    /// tokens capture more than their `N` bytes" quirk: `data` is
    /// *not* just the `N` raw payload bytes, it is the Python's own
    /// (buggy) over-inclusive slice, preserved for byte-for-byte
    /// parity with `RtfTokenizer`.
    BinN { data: Vec<u8>, separator: String },
    /// Port of `token8bitChar`: the 2 raw bytes after a `\'`
    /// control-symbol, always exactly 2 long.
    Char8Bit(Vec<u8>),
    /// Port of `tokenUnicode`. `eq_list` is the (always-empty, per the
    /// module docs) discarded fallback-replacement list;
    /// `current_ucn` is the `\ucN` value in force when this token was
    /// produced.
    Unicode {
        data: i64,
        separator: String,
        current_ucn: i64,
        eq_list: Vec<RtfToken>,
    },
}

impl RtfToken {
    /// Port of every token class's `toRTF`/`__repr__` (identical in
    /// the Python). Returns raw bytes, not `String`, because
    /// [`RtfToken::Data`]/[`RtfToken::BinN`]/[`RtfToken::Char8Bit`]
    /// payloads are not guaranteed valid UTF-8.
    pub fn to_rtf(&self) -> Vec<u8> {
        match self {
            RtfToken::DelimStart => b"{".to_vec(),
            RtfToken::DelimEnd => b"}".to_vec(),
            RtfToken::ControlWord { name, separator } => {
                let mut out = name.as_bytes().to_vec();
                out.extend_from_slice(separator.as_bytes());
                out
            }
            RtfToken::ControlWordNumeric {
                name,
                argument,
                separator,
            } => {
                let mut out = name.as_bytes().to_vec();
                out.extend_from_slice(argument.to_string().as_bytes());
                out.extend_from_slice(separator.as_bytes());
                out
            }
            RtfToken::ControlSymbol { name } => name.as_bytes().to_vec(),
            RtfToken::Data(data) => data.clone(),
            RtfToken::BinN { data, separator } => {
                // Port of `'\\bin' + repr(len(self.data)) +
                // self.separator + self.data` -- see the module docs'
                // `\bin` quirk. `len(self.data)` here is the length of
                // the over-inclusive slice `RtfTokenizer` produced,
                // not the original `N`.
                let mut out = b"\\bin".to_vec();
                out.extend_from_slice(data.len().to_string().as_bytes());
                out.extend_from_slice(separator.as_bytes());
                out.extend_from_slice(data);
                out
            }
            RtfToken::Char8Bit(data) => {
                let mut out = b"\\'".to_vec();
                out.extend_from_slice(data);
                out
            }
            RtfToken::Unicode {
                data,
                current_ucn,
                eq_list,
                ..
                // `separator` deliberately unused -- see the module
                // docs' "stored `separator` is dead code" quirk.
            } => {
                let mut result = format!("\\u{data} ").into_bytes();
                let mut ucn = *current_ucn;
                if (eq_list.len() as i64) < ucn {
                    ucn = eq_list.len() as i64;
                    let prefix = RtfToken::ControlWordNumeric {
                        name: "\\uc".to_string(),
                        argument: ucn,
                        separator: String::new(),
                    }
                    .to_rtf();
                    let mut combined = prefix;
                    combined.extend_from_slice(&result);
                    result = combined;
                }
                // Port of the Python `i = 0` that is never incremented
                // inside the loop -- see the module docs' quirk. Dead
                // in practice because `eq_list` is always empty.
                let i: i64 = 0;
                for eq in eq_list {
                    if i >= ucn {
                        break;
                    }
                    result.extend_from_slice(&eq.to_rtf());
                }
                result
            }
        }
    }
}

/// Port of `isAsciiLetter`.
fn is_ascii_letter(value: u8) -> bool {
    value.is_ascii_alphabetic()
}

/// Port of `isDigit`.
fn is_digit(value: u8) -> bool {
    value.is_ascii_digit()
}

/// Port of `isChar` (`value == char`, specialized to a single byte).
fn is_char(value: u8, ch: u8) -> bool {
    value == ch
}

/// Port of `isString` (`buffer == string`, specialized to `&[u8]` vs.
/// a `&str`'s bytes).
fn is_string(buffer: &[u8], s: &str) -> bool {
    buffer == s.as_bytes()
}

/// Port of `RtfTokenizer`: a hand-rolled scanner over raw RTF bytes.
#[derive(Debug, Clone, Default)]
pub struct RtfTokenizer {
    pub tokens: Vec<RtfToken>,
}

impl RtfTokenizer {
    /// Port of `RtfTokenizer.__init__` + `tokenize`.
    pub fn tokenize(rtf_data: &[u8]) -> Result<Self> {
        let mut tokens = Vec::new();
        let len = rtf_data.len();
        let mut i = 0usize;
        let mut last_data_start: Option<usize> = None;

        while i < len {
            let c = rtf_data[i];

            if is_char(c, b'{') {
                if let Some(start) = last_data_start.take() {
                    tokens.push(RtfToken::Data(rtf_data[start..i].to_vec()));
                }
                tokens.push(RtfToken::DelimStart);
                i += 1;
                continue;
            }

            if is_char(c, b'}') {
                if let Some(start) = last_data_start.take() {
                    tokens.push(RtfToken::Data(rtf_data[start..i].to_vec()));
                }
                tokens.push(RtfToken::DelimEnd);
                i += 1;
                continue;
            }

            if is_char(c, b'\\') {
                if i + 1 >= len {
                    return Err(RtfError::ControlCharAtEnd);
                }

                if let Some(start) = last_data_start.take() {
                    tokens.push(RtfToken::Data(rtf_data[start..i].to_vec()));
                }

                let token_start = i;
                i += 1;

                // Control Words
                if is_ascii_letter(rtf_data[i]) {
                    // consume <ASCII Letter Sequence>
                    let mut consumed = false;
                    let mut token_end = i;
                    while i < len {
                        if !is_ascii_letter(rtf_data[i]) {
                            token_end = i;
                            consumed = true;
                            break;
                        }
                        i += 1;
                    }
                    if !consumed {
                        return Err(RtfError::ControlWordWithoutEnd(token_start));
                    }

                    // We have a numeric argument before the
                    // delimiter -- NOTE: see the module docs' "Numeric
                    // control-word arguments can never be negative"
                    // quirk. The entry check below matches on a
                    // leading `-`, but the digit loop's own first
                    // check immediately rejects it (it is not a
                    // digit), so `i`/`token_end` never move past it.
                    if is_char(rtf_data[i], b'-') || is_digit(rtf_data[i]) {
                        let mut consumed_digits = false;
                        let mut digit_count = 0u32;
                        while i < len {
                            if !is_digit(rtf_data[i]) {
                                consumed_digits = true;
                                break;
                            }
                            digit_count += 1;
                            i += 1;
                            if digit_count > 10 {
                                return Err(RtfError::TooManyDigits(token_start));
                            }
                        }
                        if !consumed_digits {
                            return Err(RtfError::NumericArgumentWithoutEnd(token_start));
                        }
                    }

                    let mut separator = String::new();
                    if i < len && is_char(rtf_data[i], b' ') {
                        separator = " ".to_string();
                    }

                    let control_word = &rtf_data[token_start..token_end];
                    if token_end < i {
                        let value = parse_digits(&rtf_data[token_end..i]);
                        if is_string(control_word, "\\bin") {
                            // Port of `i = i + value` -- see the
                            // module docs' `\bin` quirk: this does NOT
                            // skip past the control word's own text
                            // first, it advances from wherever `i`
                            // already is.
                            let new_i = i
                                .checked_add(value as usize)
                                .filter(|n| *n <= len)
                                .ok_or(RtfError::UnexpectedEndOfDocument)?;
                            i = new_i;
                            tokens.push(RtfToken::BinN {
                                data: rtf_data[token_start..i].to_vec(),
                                separator,
                            });
                        } else {
                            tokens.push(RtfToken::ControlWordNumeric {
                                name: String::from_utf8_lossy(control_word).into_owned(),
                                argument: value,
                                separator,
                            });
                        }
                    } else {
                        tokens.push(RtfToken::ControlWord {
                            name: String::from_utf8_lossy(control_word).into_owned(),
                            separator,
                        });
                    }
                    // space delimiter, we should discard it
                    if i < len && rtf_data[i] == b' ' {
                        i += 1;
                    }
                } else {
                    // Control Symbol
                    tokens.push(RtfToken::ControlSymbol {
                        name: String::from_utf8_lossy(&rtf_data[token_start..i + 1]).into_owned(),
                    });
                    i += 1;
                }
                continue;
            }

            if last_data_start.is_none() {
                last_data_start = Some(i);
            }
            i += 1;
        }

        // NOTE: no trailing flush of `last_data_start` here -- see the
        // module docs' "Trailing undelimited data is silently dropped"
        // quirk. Ported as-is.

        Ok(RtfTokenizer { tokens })
    }

    /// Port of `RtfTokenizer.toRTF`.
    pub fn to_rtf(&self) -> Vec<u8> {
        self.tokens.iter().flat_map(RtfToken::to_rtf).collect()
    }
}

/// Manually folds an ASCII digit run into an `i64`. Never fails: the
/// tokenizer only ever calls this on a byte range it has already
/// verified is 1-10 ASCII digits (`digit_count > 10` errors out
/// before this is reached), which comfortably fits in an `i64`.
/// Avoids an `unwrap()`/`expect()` on `str::parse` per this crate's
/// "avoid unwrap" convention.
fn parse_digits(digits: &[u8]) -> i64 {
    let mut value: i64 = 0;
    for &b in digits {
        value = value * 10 + i64::from(b - b'0');
    }
    value
}

/// Port of `RtfTokenParser`: two post-passes over a token stream.
#[derive(Debug, Clone, Default)]
pub struct RtfTokenParser {
    pub tokens: Vec<RtfToken>,
}

impl RtfTokenParser {
    /// Port of `RtfTokenParser.__init__`, which runs `process` then
    /// `processUnicode` on construction.
    pub fn new(tokens: Vec<RtfToken>) -> Result<Self> {
        let tokens = Self::process(tokens)?;
        let tokens = Self::process_unicode(tokens)?;
        Ok(RtfTokenParser { tokens })
    }

    /// Port of `process`: collapses a `\'` control symbol plus the
    /// first 2 bytes of the following [`RtfToken::Data`] into a
    /// [`RtfToken::Char8Bit`], splitting any excess data off into a
    /// new [`RtfToken::Data`].
    fn process(tokens: Vec<RtfToken>) -> Result<Vec<RtfToken>> {
        let mut new_tokens = Vec::with_capacity(tokens.len());
        let mut i = 0usize;
        while i < tokens.len() {
            if let RtfToken::ControlSymbol { name } = &tokens[i] {
                if name == "\\'" {
                    i += 1;
                    // Port of `self.tokens[i]` with no prior bounds
                    // check -- the live Python raises an uncaught
                    // `IndexError` here if `\'` was the last token;
                    // this maps that onto `Char8BitWithoutData`
                    // instead of panicking (see the module docs).
                    let data = match tokens.get(i) {
                        Some(RtfToken::Data(d)) => d,
                        _ => return Err(RtfError::Char8BitWithoutData),
                    };
                    if data.len() < 2 {
                        return Err(RtfError::Char8BitWithoutData);
                    }
                    new_tokens.push(RtfToken::Char8Bit(data[0..2].to_vec()));
                    if data.len() > 2 {
                        new_tokens.push(RtfToken::Data(data[2..].to_vec()));
                    }
                    i += 1;
                    continue;
                }
            }
            new_tokens.push(tokens[i].clone());
            i += 1;
        }
        Ok(new_tokens)
    }

    /// Port of `processUnicode`: walks the stream maintaining a stack
    /// of the current `\ucN` value (pushed/popped by `{`/`}`), and
    /// rewrites each `\u<N>` control word plus the `ucN` fallback
    /// tokens that follow it into a single [`RtfToken::Unicode`],
    /// discarding the walked fallback tokens (see the module docs).
    fn process_unicode(tokens: Vec<RtfToken>) -> Result<Vec<RtfToken>> {
        let mut new_tokens = Vec::with_capacity(tokens.len());
        // Port of `ucNbStack = [1]`.
        let mut uc_nb_stack: Vec<i64> = vec![1];
        let mut i = 0usize;

        while i < tokens.len() {
            match &tokens[i] {
                RtfToken::DelimStart => {
                    let top = *uc_nb_stack.last().unwrap_or(&1);
                    uc_nb_stack.push(top);
                    new_tokens.push(tokens[i].clone());
                    i += 1;
                    continue;
                }
                RtfToken::DelimEnd => {
                    // Port of `ucNbStack.pop()`. See the module docs'
                    // "Deliberate safety deviations": an unbalanced
                    // `}` (more closes than opens) would pop the
                    // Python's list to empty and then raise
                    // `IndexError` on the very next stack read; this
                    // never pops the initial `[1]` sentinel away, so
                    // malformed input degrades gracefully instead of
                    // panicking.
                    if uc_nb_stack.len() > 1 {
                        uc_nb_stack.pop();
                    }
                    new_tokens.push(tokens[i].clone());
                    i += 1;
                    continue;
                }
                RtfToken::ControlWordNumeric { name, argument, .. }
                    if is_string(name.as_bytes(), "\\uc") =>
                {
                    if let Some(top) = uc_nb_stack.last_mut() {
                        *top = *argument;
                    }
                    new_tokens.push(tokens[i].clone());
                    i += 1;
                    continue;
                }
                RtfToken::ControlWordNumeric {
                    name,
                    argument,
                    separator,
                } if is_string(name.as_bytes(), "\\u") => {
                    let orig_argument = *argument;
                    let orig_separator = separator.clone();
                    i += 1;
                    let mut j: i64 = 0;
                    let ucn = *uc_nb_stack.last().unwrap_or(&1);
                    let mut partial_data: Option<RtfToken> = None;
                    // Port of the `replace` list: built up exactly as
                    // the Python walks fallback tokens, then always
                    // discarded -- see the module docs. Kept as a
                    // real (if unused past this point) `Vec` rather
                    // than skipped outright, so this stays an honest
                    // "build it, then throw it away" rather than a
                    // silent behavioral shortcut.
                    let mut replace: Vec<RtfToken> = Vec::new();
                    while i < tokens.len() && j < ucn {
                        match &tokens[i] {
                            RtfToken::DelimStart | RtfToken::DelimEnd => break,
                            RtfToken::Data(data) => {
                                let need = ucn - j;
                                if (data.len() as i64) >= need {
                                    let need = need as usize;
                                    replace.push(RtfToken::Data(data[..need].to_vec()));
                                    if data.len() > need {
                                        partial_data = Some(RtfToken::Data(data[need..].to_vec()));
                                    }
                                    i += 1;
                                    break;
                                } else {
                                    replace.push(tokens[i].clone());
                                    j += data.len() as i64;
                                    i += 1;
                                }
                            }
                            RtfToken::Char8Bit(_) | RtfToken::BinN { .. } => {
                                replace.push(tokens[i].clone());
                                i += 1;
                                j += 1;
                            }
                            _ => return Err(RtfError::IncorrectUtfReplacement),
                        }
                    }

                    // calibre rtf2xml does not support utfreplace.
                    replace = Vec::new();

                    let current_ucn = *uc_nb_stack.last().unwrap_or(&1);
                    new_tokens.push(RtfToken::Unicode {
                        data: orig_argument,
                        separator: orig_separator,
                        current_ucn,
                        eq_list: replace,
                    });
                    if let Some(pd) = partial_data {
                        new_tokens.push(pd);
                    }
                    continue;
                }
                _ => {}
            }
            new_tokens.push(tokens[i].clone());
            i += 1;
        }

        Ok(new_tokens)
    }

    /// Port of `RtfTokenParser.toRTF`.
    pub fn to_rtf(&self) -> Vec<u8> {
        self.tokens.iter().flat_map(RtfToken::to_rtf).collect()
    }
}

/// Tokenize, parse, and reserialize in one call. Not part of the
/// Python's public API (that was only ever the `if __name__ ==
/// '__main__':` file-conversion script, not ported -- see the module
/// docs), but a convenient entry point for callers and tests that just
/// want "clean up this RTF" without touching the tokenizer/parser
/// separately.
pub fn round_trip(rtf_data: &[u8]) -> Result<Vec<u8>> {
    let tokenizer = RtfTokenizer::tokenize(rtf_data)?;
    let parser = RtfTokenParser::new(tokenizer.tokens)?;
    Ok(parser.to_rtf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(s: &str) -> Vec<RtfToken> {
        RtfTokenizer::tokenize(s.as_bytes()).unwrap().tokens
    }

    fn rtf_str(tokens: &[RtfToken]) -> String {
        let bytes: Vec<u8> = tokens.iter().flat_map(RtfToken::to_rtf).collect();
        String::from_utf8(bytes).unwrap()
    }

    // ---- tokenizer: delimiters, control words, control symbols ----

    #[test]
    fn tokenizes_delimiters() {
        let tokens = tokenize("{}");
        assert_eq!(tokens, vec![RtfToken::DelimStart, RtfToken::DelimEnd]);
    }

    #[test]
    fn tokenizes_a_control_word_without_a_numeric_argument() {
        let tokens = tokenize(r"\ansi ");
        assert_eq!(
            tokens,
            vec![RtfToken::ControlWord {
                name: "\\ansi".to_string(),
                separator: " ".to_string(),
            }]
        );
    }

    #[test]
    fn tokenizes_a_control_word_with_a_numeric_argument() {
        let tokens = tokenize(r"\li720 ");
        assert_eq!(
            tokens,
            vec![RtfToken::ControlWordNumeric {
                name: "\\li".to_string(),
                argument: 720,
                separator: " ".to_string(),
            }]
        );
    }

    #[test]
    fn control_word_numeric_argument_terminator_is_consumed_and_discarded() {
        // `\li720X` (no space): the terminator 'X' becomes data, not
        // part of the control word, and is not itself consumed (only
        // a *space* terminator is discarded). Wrapped in braces so the
        // trailing 'X' is flushed rather than silently dropped by the
        // "trailing undelimited data" quirk documented above.
        let tokens = tokenize(r"{\li720X}");
        assert_eq!(
            tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWordNumeric {
                    name: "\\li".to_string(),
                    argument: 720,
                    separator: String::new(),
                },
                RtfToken::Data(b"X".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
    }

    #[test]
    fn tokenizes_a_control_symbol() {
        let tokens = tokenize(r"\~");
        assert_eq!(
            tokens,
            vec![RtfToken::ControlSymbol {
                name: "\\~".to_string()
            }]
        );
    }

    #[test]
    fn tokenizes_plain_data_between_control_constructs() {
        let tokens = tokenize(r"{\b bold}");
        assert_eq!(
            tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWord {
                    name: "\\b".to_string(),
                    separator: " ".to_string(),
                },
                RtfToken::Data(b"bold".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
    }

    // ---- preserved quirk: negative numeric arguments ----

    #[test]
    fn negative_numeric_arguments_are_not_parsed_as_an_argument() {
        // See the module docs: the leading '-' is never consumed, so
        // this becomes a bare control word followed by literal data,
        // not a `ControlWordNumeric` with a negative argument. Matches
        // the live Python exactly (verified separately). Wrapped in
        // braces so the trailing data is flushed rather than silently
        // dropped by the "trailing undelimited data" quirk documented
        // above.
        let tokens = tokenize(r"{\fi-360 x}");
        assert_eq!(
            tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWord {
                    name: "\\fi".to_string(),
                    separator: String::new(),
                },
                RtfToken::Data(b"-360 x".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
        // Still round-trips byte-for-byte despite not being
        // "understood" as a signed argument.
        assert_eq!(rtf_str(&tokens), r"{\fi-360 x}");
    }

    // ---- tokenizer error conditions ----

    #[test]
    fn errors_on_a_control_character_at_end_of_document() {
        let err = RtfTokenizer::tokenize(b"\\").unwrap_err();
        assert_eq!(err, RtfError::ControlCharAtEnd);
    }

    #[test]
    fn errors_on_a_control_word_that_never_terminates() {
        let err = RtfTokenizer::tokenize(b"\\b").unwrap_err();
        assert_eq!(err, RtfError::ControlWordWithoutEnd(0));
    }

    #[test]
    fn errors_on_more_than_ten_digits_in_a_numeric_argument() {
        let err = RtfTokenizer::tokenize(b"\\bin12345678901 x").unwrap_err();
        assert_eq!(err, RtfError::TooManyDigits(0));
    }

    #[test]
    fn errors_when_a_numeric_argument_runs_to_end_of_document() {
        let err = RtfTokenizer::tokenize(b"\\li720").unwrap_err();
        assert_eq!(err, RtfError::NumericArgumentWithoutEnd(0));
    }

    // ---- preserved quirk: trailing undelimited data is dropped ----

    #[test]
    fn trailing_undelimited_data_is_dropped() {
        // See the module docs. Matches the live Python's `tokenize()`
        // producing zero tokens for `'hello world'`.
        let tokens = tokenize("hello world");
        assert!(tokens.is_empty());
    }

    #[test]
    fn data_closed_by_a_brace_is_not_dropped() {
        let tokens = tokenize("{hello world}");
        assert_eq!(
            tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::Data(b"hello world".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
    }

    // ---- preserved quirk: `\bin` over-captures ----

    #[test]
    fn bin_n_over_captures_and_mis_serializes() {
        // Verified against a live run of the Python: `data` includes
        // the "\bin3 " prefix and only 2 of the 3 true payload bytes,
        // and `rest` is left dangling as unconsumed data.
        let tok = RtfTokenizer::tokenize(b"\\bin3 XYZrest").unwrap();
        assert_eq!(
            tok.tokens,
            vec![RtfToken::BinN {
                data: b"\\bin3 XY".to_vec(),
                separator: " ".to_string(),
            }]
        );
        assert_eq!(tok.to_rtf(), b"\\bin8 \\bin3 XY".to_vec());
    }

    #[test]
    fn bin_n_claiming_more_bytes_than_exist_errors_instead_of_panicking() {
        let err = RtfTokenizer::tokenize(b"\\bin999999 x").unwrap_err();
        assert_eq!(err, RtfError::UnexpectedEndOfDocument);
    }

    // ---- RtfTokenParser::process: `\'HH` hex-char pass ----

    #[test]
    fn hex_char_pass_collapses_backslash_quote_and_two_hex_digits() {
        let tok = tokenize(r"{\'e9abc}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert_eq!(
            parsed.tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::Char8Bit(b"e9".to_vec()),
                RtfToken::Data(b"abc".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
        assert_eq!(rtf_str(&parsed.tokens), r"{\'e9abc}");
    }

    #[test]
    fn hex_char_pass_with_exactly_two_chars_leaves_no_leftover_data() {
        let tok = tokenize(r"{\'e9}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert_eq!(
            parsed.tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::Char8Bit(b"e9".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
    }

    #[test]
    fn hex_char_pass_errors_without_following_data() {
        let tok = tokenize(r"{\'}");
        let err = RtfTokenParser::new(tok).unwrap_err();
        assert_eq!(err, RtfError::Char8BitWithoutData);
    }

    #[test]
    fn hex_char_pass_errors_with_fewer_than_two_chars_of_data() {
        let tok = tokenize(r"{\'e}");
        let err = RtfTokenParser::new(tok).unwrap_err();
        assert_eq!(err, RtfError::Char8BitWithoutData);
    }

    #[test]
    fn hex_char_pass_errors_when_quote_symbol_is_the_last_token() {
        // Live Python: uncaught IndexError. Ported as a proper error.
        let tok = vec![RtfToken::ControlSymbol {
            name: "\\'".to_string(),
        }];
        let err = RtfTokenParser::new(tok).unwrap_err();
        assert_eq!(err, RtfError::Char8BitWithoutData);
    }

    // ---- RtfTokenParser::process_unicode ----

    #[test]
    fn unicode_token_with_zero_uc_has_no_fallback_and_round_trips() {
        let tok = tokenize(r"{\uc0\u955 replaced text}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert_eq!(
            parsed.tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWordNumeric {
                    name: "\\uc".to_string(),
                    argument: 0,
                    separator: String::new(),
                },
                RtfToken::Unicode {
                    data: 955,
                    separator: " ".to_string(),
                    current_ucn: 0,
                    eq_list: vec![],
                },
                RtfToken::Data(b"replaced text".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
        assert_eq!(rtf_str(&parsed.tokens), r"{\uc0\u955 replaced text}");
    }

    #[test]
    fn unicode_fallback_is_always_discarded_even_though_it_was_present() {
        // `\uc2` means 2 fallback chars follow `\u955`. The live
        // Python's `replace` list would capture `tokenData('ab')`
        // (and split the rest into a fresh `tokenData(' more')`), but
        // then unconditionally discards it -- `eq_list` on the
        // resulting `Unicode` token is empty regardless.
        let tok = tokenize(r"{\uc2\u955 ab more}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert_eq!(
            parsed.tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWordNumeric {
                    name: "\\uc".to_string(),
                    argument: 2,
                    separator: String::new(),
                },
                RtfToken::Unicode {
                    data: 955,
                    separator: " ".to_string(),
                    current_ucn: 2,
                    eq_list: vec![],
                },
                RtfToken::Data(b" more".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
        // Reserialization synthesizes `\uc0` (0 == the discarded
        // eq_list's length) ahead of the `\u955 `, matching the live
        // Python's `'{\\uc2\\uc0\\u955  more}'` exactly (including the
        // resulting double space: one from `\u955 `'s own trailing
        // space, one from the leftover `' more'`).
        assert_eq!(rtf_str(&parsed.tokens), r"{\uc2\uc0\u955  more}");
    }

    #[test]
    fn nested_uc_scopes_use_the_innermost_value_and_restore_on_close() {
        // Outer `\uc2`; a nested group sets `\uc1` for its own `\u`,
        // then closing the group restores the outer value of 2 for
        // anything after it (here just plain data 'c').
        let tok = tokenize(r"{\uc2\u955 a{\uc1\u321 b}c}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert_eq!(
            parsed.tokens,
            vec![
                RtfToken::DelimStart,
                RtfToken::ControlWordNumeric {
                    name: "\\uc".to_string(),
                    argument: 2,
                    separator: String::new(),
                },
                // `\u955`'s fallback loop stops at the nested `{`
                // having only consumed 1 of the 2 chars it wanted
                // ('a'), so there is no leftover partial-data token.
                RtfToken::Unicode {
                    data: 955,
                    separator: " ".to_string(),
                    current_ucn: 2,
                    eq_list: vec![],
                },
                RtfToken::DelimStart,
                RtfToken::ControlWordNumeric {
                    name: "\\uc".to_string(),
                    argument: 1,
                    separator: String::new(),
                },
                // `\u321`'s fallback loop fully consumes the 1-char
                // 'b' data token (ucn - j == 1 == len('b')), so there
                // is no leftover data and no partial split.
                RtfToken::Unicode {
                    data: 321,
                    separator: " ".to_string(),
                    current_ucn: 1,
                    eq_list: vec![],
                },
                RtfToken::DelimEnd,
                RtfToken::Data(b"c".to_vec()),
                RtfToken::DelimEnd,
            ]
        );
        assert_eq!(
            rtf_str(&parsed.tokens),
            r"{\uc2\uc0\u955 {\uc1\uc0\u321 }c}"
        );
    }

    #[test]
    fn unicode_fallback_can_split_a_token_data_run() {
        // ucn=2, 8 chars of data follow ("ab more1"): the first 2
        // become the (discarded) fallback, the remaining 6 become a
        // fresh trailing `Data` token.
        let tok = tokenize(r"{\uc2\u955 ab more1}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        let unicode_idx = parsed
            .tokens
            .iter()
            .position(|t| matches!(t, RtfToken::Unicode { .. }))
            .unwrap();
        assert_eq!(
            parsed.tokens[unicode_idx + 1],
            RtfToken::Data(b" more1".to_vec())
        );
    }

    #[test]
    fn unicode_fallback_spanning_a_char8bit_and_bin_n_token_is_discarded() {
        // ucn=2: one `\'e9` (Char8Bit) contributes 1 toward the
        // fallback count, then a `Data` token contributes the rest.
        let tok = tokenize(r"{\uc2\u955 \'e9x}");
        let parsed = RtfTokenParser::new(tok).unwrap();
        assert!(
            parsed
                .tokens
                .iter()
                .any(|t| matches!(t, RtfToken::Unicode { eq_list, .. } if eq_list.is_empty())),
            "{:?}",
            parsed.tokens
        );
    }

    #[test]
    fn unicode_fallback_errors_on_an_unreplaceable_token() {
        // `\x41` tokenizes as a (harmless-looking) numeric control
        // word, which is not one of the fallback-eligible token kinds
        // (`Data`/`Char8Bit`/`BinN`) -- matches the live Python's
        // `Exception('Error: incorrect utf replacement.')`.
        let tok = tokenize(r"{\uc3\u955 \x41more}");
        let err = RtfTokenParser::new(tok).unwrap_err();
        assert_eq!(err, RtfError::IncorrectUtfReplacement);
    }

    #[test]
    fn unicode_fallback_survives_a_non_empty_eq_list_but_stays_inert() {
        // Not producible via `process_unicode` (which always empties
        // `eq_list`), but documents `to_rtf`'s dead-loop-counter quirk
        // for real (verified against a live run of the Python):
        // `eq_list.len()` (3) is not less than `current_ucn` (1), so
        // no `\ucN` prefix is synthesized (`ucn` stays 1) -- but the
        // loop's own counter `i` is never incremented, so the `i >=
        // ucn` break condition (0 >= 1) never fires, and *all three*
        // eq_list items get appended instead of just the first `ucn`
        // worth.
        let token = RtfToken::Unicode {
            data: 65,
            separator: String::new(),
            current_ucn: 1,
            eq_list: vec![
                RtfToken::Data(b"A".to_vec()),
                RtfToken::Data(b"B".to_vec()),
                RtfToken::Data(b"C".to_vec()),
            ],
        };
        assert_eq!(token.to_rtf(), b"\\u65 ABC".to_vec());
    }

    #[test]
    fn unicode_fallback_prefix_shrinks_uc_when_eq_list_is_shorter() {
        // `eq_list.len()` (1) IS less than `current_ucn` (2), so `ucn`
        // shrinks to 1 and a `\uc1` prefix is synthesized ahead of the
        // `\u955 `. Verified against a live run of the Python
        // (`tokenUnicode(955, '', 2, [tokenData('X')]).toRTF()` ->
        // `'\\uc1\\u955 X'`).
        let token = RtfToken::Unicode {
            data: 955,
            separator: String::new(),
            current_ucn: 2,
            eq_list: vec![RtfToken::Data(b"X".to_vec())],
        };
        assert_eq!(token.to_rtf(), b"\\uc1\\u955 X".to_vec());
    }

    // ---- round trip ----

    #[test]
    fn round_trip_structurally_faithful_rtf_fragment() {
        let src = r"{\rtf1\ansi\deff0{\fonttbl{\f0\froman Times;}}\f0\fs24\b bold\b0  plain}";
        let out = round_trip(src.as_bytes()).unwrap();
        assert_eq!(out, src.as_bytes());
    }

    #[test]
    fn round_trip_with_hex_char_and_negative_indent() {
        let src = r"{\pard\fi-360\li720 caf\'e9}";
        let out = round_trip(src.as_bytes()).unwrap();
        assert_eq!(out, src.as_bytes());
    }
}
