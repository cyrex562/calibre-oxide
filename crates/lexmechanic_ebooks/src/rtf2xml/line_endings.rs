//! Port of `old_src/src/calibre/ebooks/rtf2xml/line_endings.py`
//! (`FixLineEndings`).
//!
//! Normalizes raw RTF input to Unix line endings (`\r\n` and lone `\r`
//! both become `\n`) before tokenizing, operating on raw bytes exactly
//! as the Python does (`open(..., 'rb')`) rather than decoded text --
//! RTF input is not guaranteed to be valid UTF-8 (or even
//! ASCII-compatible) until later passes normalize its encoding, so byte
//! operations avoid a premature (and potentially lossy) decode.

use super::replace_illegals::is_illegal_ascii_control_byte;

/// Port of `calibre.utils.cleantext.clean_ascii_chars` for the `bytes`
/// case. See [`super::replace_illegals::clean_ascii_chars`] for the
/// `str` case.
pub fn clean_ascii_bytes(input: &[u8]) -> Vec<u8> {
    input
        .iter()
        .copied()
        .filter(|&b| !is_illegal_ascii_control_byte(b))
        .collect()
}

/// Port of `FixLineEndings.fix_endings`. `replace_illegals` mirrors the
/// constructor's `replace_illegals=1` default (Python's `self.__replace_illegals`).
pub fn fix_line_endings(input: &[u8], replace_illegals: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'\r' {
            out.push(b'\n');
            if i + 1 < input.len() && input[i + 1] == b'\n' {
                i += 2;
            } else {
                i += 1;
            }
        } else {
            out.push(input[i]);
            i += 1;
        }
    }
    if replace_illegals {
        clean_ascii_bytes(&out)
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_becomes_lf() {
        assert_eq!(fix_line_endings(b"a\r\nb", true), b"a\nb");
    }

    #[test]
    fn lone_cr_becomes_lf() {
        assert_eq!(fix_line_endings(b"a\rb", true), b"a\nb");
    }

    #[test]
    fn lf_is_left_alone() {
        assert_eq!(fix_line_endings(b"a\nb", true), b"a\nb");
    }

    #[test]
    fn mixed_line_endings_all_normalize() {
        assert_eq!(fix_line_endings(b"a\r\nb\rc\nd", true), b"a\nb\nc\nd");
    }

    #[test]
    fn strips_illegal_control_bytes_when_enabled() {
        assert_eq!(fix_line_endings(b"a\x00b\x01c", true), b"abc");
    }

    #[test]
    fn leaves_illegal_control_bytes_when_disabled() {
        assert_eq!(fix_line_endings(b"a\x00b", false), b"a\x00b");
    }

    #[test]
    fn empty_input_stays_empty() {
        assert_eq!(fix_line_endings(b"", true), b"");
    }
}
