//! Port of `old_src/src/calibre/ebooks/rtf2xml/replace_illegals.py`
//! (`ReplaceIllegals`).
//!
//! Strips illegal low-ASCII control characters from raw RTF input
//! before tokenizing, via `calibre.utils.cleantext.clean_ascii_chars`
//! (not itself one of this issue's ten files, so its exact byte-range
//! behavior is reproduced here directly rather than imported -- see
//! [`clean_ascii_chars`]). [`super::line_endings`] uses the same
//! byte-range logic on raw bytes rather than `str`, so the core
//! stripping predicate lives in [`is_illegal_ascii_control_byte`] and
//! both modules build on it.

/// Port of `calibre.utils.cleantext.ascii_pat`'s character set: every
/// ASCII control character (0x00-0x1F) except tab (0x09), LF (0x0A),
/// and CR (0x0D), plus DEL (0x7F). Non-ASCII bytes/chars are never
/// touched -- this is deliberately narrower than "strip everything
/// that isn't printable ASCII".
pub fn is_illegal_ascii_control_byte(b: u8) -> bool {
    (b < 0x20 && b != 0x09 && b != 0x0A && b != 0x0D) || b == 0x7F
}

/// Port of `calibre.utils.cleantext.clean_ascii_chars` for the `str`
/// case used by `replace_illegals.py`. See
/// [`super::line_endings::clean_ascii_bytes`] for the `bytes` case used
/// by `line_endings.py`.
pub fn clean_ascii_chars(input: &str) -> String {
    input
        .chars()
        .filter(|&c| !(c.is_ascii() && is_illegal_ascii_control_byte(c as u8)))
        .collect()
}

/// Port of `ReplaceIllegals.replace_illegals`'s per-line transform,
/// applied over the whole content at once (there is no cross-line
/// state, so processing line-by-line vs. all at once is equivalent
/// other than the Python's incidental normalization of the final
/// line's missing trailing newline -- callers that need that should
/// use `.lines()` on the input themselves before calling this).
pub fn replace_illegals(content: &str) -> String {
    clean_ascii_chars(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_control_chars_but_keeps_tab_lf_cr() {
        let input = "a\u{0}b\u{1}c\td\ne\rf\u{7F}g";
        assert_eq!(replace_illegals(input), "abc\td\ne\rfg");
    }

    #[test]
    fn leaves_non_ascii_text_untouched() {
        let input = "caf\u{e9} \u{4e2d}\u{6587}";
        assert_eq!(replace_illegals(input), input);
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(replace_illegals(""), "");
    }

    #[test]
    fn is_illegal_byte_matches_the_documented_ranges() {
        assert!(is_illegal_ascii_control_byte(0x00));
        assert!(is_illegal_ascii_control_byte(0x1F));
        assert!(is_illegal_ascii_control_byte(0x7F));
        assert!(!is_illegal_ascii_control_byte(0x09));
        assert!(!is_illegal_ascii_control_byte(0x0A));
        assert!(!is_illegal_ascii_control_byte(0x0D));
        assert!(!is_illegal_ascii_control_byte(0x20));
        assert!(!is_illegal_ascii_control_byte(b'A'));
    }
}
