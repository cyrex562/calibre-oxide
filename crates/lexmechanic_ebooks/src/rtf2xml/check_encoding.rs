//! Port of `old_src/src/calibre/ebooks/rtf2xml/check_encoding.py`
//! (`CheckEncoding`).
//!
//! Scans a byte stream line by line (split on `\n`, matching Python's
//! binary-mode file iteration) and reports whether any line fails to
//! decode under a named encoding. The only encoding the rest of the
//! rtf2xml pipeline actually passes is the default, `us-ascii`
//! (`check_encoding_obj.check_encoding(sys.argv[1])`, called with no
//! explicit encoding, in the Python's own `__main__` block) -- other
//! labels are supported on a best-effort basis via `encoding_rs`.
//!
//! # Preserved-intent (not preserved-bug) deviation
//!
//! `CheckEncoding.__get_position_error`'s per-character diagnostic loop
//! is dead code in the live Python: it iterates the offending `line`
//! (a `bytes` object), and iterating `bytes` in Python 3 yields `int`
//! elements, not single-byte `bytes` -- so `char.decode(encoding)`
//! immediately raises `AttributeError: 'int' object has no attribute
//! 'decode'`, an exception the surrounding `except ValueError:` does
//! not catch. In other words, the verbose per-position diagnostic
//! crashes unconditionally the moment it would actually run (verified
//! by inspection: `for char in <bytes>` always yields `int` in Python
//! 3). This crate's convention is to never introduce a panic
//! equivalent to an upstream crash (see `crate::rtf::preprocess`'s
//! "Deliberate safety deviations"), and a literal crash-on-call would
//! serve no purpose here since nothing downstream depends on this
//! private helper's behavior -- only its caller's boolean result does.
//! [`check_encoding_bytes`] therefore implements the *intended*
//! diagnostic (report the offset of the first byte that fails to
//! decode) instead of reproducing the crash, and prints it through the
//! same `verbose` gate and message shape as the Python.

use encoding_rs::Encoding;
use thiserror::Error;

/// Port of the implicit `LookupError` Python would raise (uncaught) for
/// an encoding name the codecs registry doesn't recognize.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unknown encoding: {0}")]
pub struct UnknownEncodingError(pub String);

enum Codec {
    /// Strict 7-bit ASCII (`ascii` / `us-ascii`): any byte `>= 0x80` is
    /// invalid. Deliberately not delegated to `encoding_rs`, whose
    /// WHATWG-label resolution maps `"us-ascii"` to windows-1252 (a
    /// superset that would silently accept high-bit bytes Python's
    /// strict `'us-ascii'` codec rejects).
    Ascii,
    Utf8,
    Other(&'static Encoding),
}

fn resolve_codec(label: &str) -> Result<Codec, UnknownEncodingError> {
    let lower = label.to_ascii_lowercase();
    match lower.as_str() {
        "ascii" | "us-ascii" | "us_ascii" => Ok(Codec::Ascii),
        "utf-8" | "utf8" | "utf_8" => Ok(Codec::Utf8),
        _ => Encoding::for_label(label.as_bytes())
            .map(Codec::Other)
            .ok_or_else(|| UnknownEncodingError(label.to_string())),
    }
}

/// Returns `Some(offset)` for the first byte index in `line` that the
/// codec cannot decode, or `None` if the whole line decodes cleanly.
fn first_invalid_byte(codec: &Codec, line: &[u8]) -> Option<usize> {
    match codec {
        Codec::Ascii => line.iter().position(|&b| b >= 0x80),
        Codec::Utf8 => std::str::from_utf8(line).err().map(|e| e.valid_up_to()),
        Codec::Other(encoding) => {
            let (_, _, had_errors) = encoding.decode(line);
            if !had_errors {
                return None;
            }
            // Best-effort scan: probe growing prefixes and report the
            // shortest one that already has errors. Exact for
            // single-byte codecs (the common case for the codepage
            // labels this pipeline deals with); an approximation for
            // multi-byte ones.
            for i in 0..line.len() {
                let (_, _, had_errors) = encoding.decode(&line[..=i]);
                if had_errors {
                    return Some(i);
                }
            }
            None
        }
    }
}

fn decodes_cleanly(codec: &Codec, line: &[u8]) -> bool {
    match codec {
        Codec::Ascii => line.iter().all(|&b| b < 0x80),
        Codec::Utf8 => std::str::from_utf8(line).is_ok(),
        Codec::Other(encoding) => {
            let (_, _, had_errors) = encoding.decode(line);
            !had_errors
        }
    }
}

/// Port of `CheckEncoding.check_encoding`. Returns `Ok(true)` if a line
/// with invalid bytes for `encoding_label` was found (matching the
/// Python's inverted-sounding-but-verified return value: `True` means
/// "bad encoding found"), `Ok(false)` if every line decoded cleanly
/// (including the empty-input case, which has no lines to fail).
pub fn check_encoding_bytes(
    data: &[u8],
    encoding_label: &str,
    verbose: bool,
) -> Result<bool, UnknownEncodingError> {
    let codec = resolve_codec(encoding_label)?;
    for (idx, line) in data.split_inclusive(|&b| b == b'\n').enumerate() {
        let line_num = idx + 1;
        if !decodes_cleanly(&codec, line) {
            if verbose {
                if line.len() < 1000 {
                    if let Some(pos) = first_invalid_byte(&codec, line) {
                        eprintln!(
                            "line: {line_num} char: {}\ninvalid byte for encoding",
                            pos + 1
                        );
                    }
                } else {
                    eprintln!("line: {line_num} has bad encoding");
                }
            }
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_ascii_passes() {
        assert_eq!(
            check_encoding_bytes(b"hello world\nline two\n", "us-ascii", false),
            Ok(false)
        );
    }

    #[test]
    fn high_bit_byte_fails_strict_ascii() {
        assert_eq!(
            check_encoding_bytes(b"caf\xe9 au lait\n", "us-ascii", false),
            Ok(true)
        );
    }

    #[test]
    fn empty_input_passes() {
        assert_eq!(check_encoding_bytes(b"", "us-ascii", false), Ok(false));
    }

    #[test]
    fn valid_utf8_passes_utf8_check() {
        let data = "caf\u{e9}\n".as_bytes();
        assert_eq!(check_encoding_bytes(data, "utf-8", false), Ok(false));
    }

    #[test]
    fn invalid_utf8_fails_utf8_check() {
        assert_eq!(
            check_encoding_bytes(b"\xff\xfe\n", "utf-8", false),
            Ok(true)
        );
    }

    #[test]
    fn high_bit_byte_passes_under_windows_1252() {
        // 0xE9 is a valid windows-1252 byte (matches 'é').
        assert_eq!(
            check_encoding_bytes(b"caf\xe9\n", "windows-1252", false),
            Ok(false)
        );
    }

    #[test]
    fn unknown_encoding_label_errors_instead_of_silently_defaulting() {
        let err = check_encoding_bytes(b"x", "not-a-real-encoding", false).unwrap_err();
        assert_eq!(err, UnknownEncodingError("not-a-real-encoding".to_string()));
    }

    #[test]
    fn verbose_mode_does_not_panic_and_still_reports_bad_encoding() {
        assert_eq!(
            check_encoding_bytes(b"caf\xe9\n", "us-ascii", true),
            Ok(true)
        );
    }
}
