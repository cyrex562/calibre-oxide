//! FictionBook 2 support.
//!
//! Port of `old_src/src/calibre/ebooks/fb2/`:
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` (`base64_decode`) | this module |
//! | `fb2ml.py` | [`fb2ml`] |
//!
//! FB2 is a single XML file with the images base64'd into it, which is
//! why a tolerant base64 decoder lives at the package root: real FB2
//! files in the wild carry `<binary>` payloads that a strict decoder
//! rejects.

pub mod fb2ml;

pub use fb2ml::{Fb2Mlizer, Fb2Options, Sectionize};

/// Decode base64, ignoring anything that is not a base64 digit.
///
/// Port of the Python `base64_decode`, which first tries the standard
/// library and falls back to a hand-rolled decoder adapted from
/// FBReader. This port only implements the tolerant path: it accepts
/// everything the strict decoder would and more, so trying strict
/// first would only ever change which errors are reported.
///
/// Characters outside the base64 alphabet are skipped rather than
/// rejected — whitespace, newlines and stray punctuation are common in
/// FB2 `<binary>` elements. `=` is treated as a zero digit that still
/// occupies a position, as in the original.
pub fn base64_decode(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len() * 3 / 4);
    let mut pos = 0usize;
    while pos < raw.len() {
        let mut total: u32 = 0;
        let mut i = 0;
        while i < 4 && pos < raw.len() {
            let byte = raw[pos];
            pos += 1;
            let num = match byte {
                b'A'..=b'Z' => (byte - b'A') as u32,
                b'a'..=b'z' => (byte - b'a') as u32 + 26,
                b'0'..=b'9' => (byte - b'0') as u32 + 52,
                b'+' => 62,
                b'/' => 63,
                // Padding counts as a digit position but contributes
                // 64, exactly as the Python's lookup does.
                b'=' => 64,
                _ => continue,
            };
            total = total.wrapping_add(num << (6 * (3 - i)));
            i += 1;
        }
        let mut triple = [0u8; 3];
        for j in (0..3).rev() {
            triple[j] = (total & 0xff) as u8;
            total >>= 8;
        }
        out.extend_from_slice(&triple);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn strict(data: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    #[test]
    fn decodes_what_a_strict_encoder_produced() {
        for original in [
            &b"hello"[..],
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            &[0u8, 1, 2, 3, 255, 254],
        ] {
            let encoded = strict(original);
            let decoded = base64_decode(encoded.as_bytes());
            // The decoder emits whole triples, so a payload whose
            // length is not a multiple of three comes back with
            // trailing zero bytes — the Python behaves the same way,
            // and callers truncate using the image's own header.
            assert_eq!(&decoded[..original.len()], original, "decoding {encoded:?}");
        }
    }

    #[test]
    fn skips_characters_that_are_not_base64_digits() {
        // FB2 files wrap their binary payloads across lines, and some
        // producers indent them too.
        let encoded = strict(b"the quick brown fox");
        let mangled: String = encoded
            .as_bytes()
            .chunks(4)
            .map(|c| format!("  {}\n", String::from_utf8_lossy(c)))
            .collect();
        let decoded = base64_decode(mangled.as_bytes());
        assert_eq!(&decoded[..19], b"the quick brown fox");
    }

    #[test]
    fn an_empty_input_decodes_to_nothing() {
        assert!(base64_decode(b"").is_empty());
    }

    #[test]
    fn junk_only_input_still_emits_one_empty_triple() {
        // The Python consumes the junk inside its inner loop and then
        // writes the zero triple anyway, so a run of whitespace decodes
        // to three zero bytes rather than to nothing. Reproduced: a
        // caller reading an image header sees a zero-length payload
        // either way.
        assert_eq!(base64_decode(b"\n\n  \n"), vec![0, 0, 0]);
    }

    #[test]
    fn a_truncated_payload_does_not_panic() {
        for raw in ["a", "ab", "abc", "=", "==", "a=", "a==="] {
            let _ = base64_decode(raw.as_bytes());
        }
    }
}
