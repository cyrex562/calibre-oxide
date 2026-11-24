//! PML (the eReader/PalmDoc-family markup format) support.
//!
//! Port of `old_src/src/calibre/ebooks/pml/__init__.py`,
//! `pmlconverter.py` and `pmlml.py` (issue #47). `pmlconverter` is the
//! PML -> HTML direction ([`pmlconverter::PmlHtmlizer`]); `pmlml` is the
//! reverse, OEB/XHTML -> PML ([`pmlml::PmlMlizer`]).
//!
//! This module used to have two narrow stand-ins living in
//! `input::pml_input::pml_to_html` and `output::pml_output::html_to_pml`
//! (issue #43's ereader port needed *some* PML<->HTML conversion and
//! didn't want to duplicate it across `pml_input`/`pml_output` and
//! `pdb::ereader`). Both are now gone: every caller uses the real
//! converters here instead.

pub mod pmlconverter;
pub mod pmlml;

/// Uncommon characters supported by PML's `\a` tag codes: their
/// Windows-1252 byte values.
///
/// Port of `A_CHARS` (`r(160, 256) + r(130, 136) + r(138, 141) +
/// r(145, 152) + r(153, 157) + [159]`).
fn a_chars() -> &'static [u16] {
    static CHARS: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    CHARS.get_or_init(|| {
        (160..256)
            .chain(130..136)
            .chain(138..141)
            .chain(145..152)
            .chain(153..157)
            .chain(std::iter::once(159))
            .collect()
    })
}

/// Extended Unicode characters supported by PML's `\U` tag codes.
///
/// Port of `U_CHARS`, built from the named Unicode-block sub-lists in
/// `__init__.py`, transcribed range-for-range and literal-for-literal.
fn u_chars() -> &'static std::collections::HashSet<u32> {
    static CHARS: std::sync::OnceLock<std::collections::HashSet<u32>> = std::sync::OnceLock::new();
    CHARS.get_or_init(|| {
        let mut v: Vec<u32> = Vec::new();

        // Latin_ExtendedA
        v.extend(0x0100..0x0104);
        v.extend([
            0x0105, 0x0107, 0x010C, 0x010D, 0x0112, 0x0113, 0x0115, 0x0117, 0x0119, 0x011B, 0x011D,
            0x011F, 0x012A, 0x012B, 0x012D, 0x012F, 0x0131, 0x0141, 0x0142, 0x0144, 0x0148,
        ]);
        v.extend(0x014B..0x014E);
        v.extend([0x014F, 0x0151, 0x0155]);
        v.extend(0x0159..0x015C);
        v.extend([
            0x015F, 0x0163, 0x0169, 0x016B, 0x016D, 0x0177, 0x017A, 0x017D, 0x017E,
        ]);

        // Latin_ExtendedB
        v.extend([
            0x01BF, 0x01CE, 0x01D0, 0x01D2, 0x01D4, 0x01E1, 0x01E3, 0x01E7, 0x01EB, 0x01F0, 0x0207,
            0x021D, 0x0227, 0x022F, 0x0233,
        ]);

        // IPA_Extensions
        v.extend([
            0x0251, 0x0251, 0x0254, 0x0259, 0x025C, 0x0265, 0x026A, 0x0272, 0x0283, 0x0289, 0x028A,
            0x028C, 0x028F, 0x0292, 0x0294, 0x029C,
        ]);

        // Spacing_Modifier_Letters
        v.extend([
            0x02BE, 0x02BF, 0x02C7, 0x02C8, 0x02CC, 0x02D0, 0x02D8, 0x02D9,
        ]);

        // Greek_and_Coptic
        v.extend(0x0391..0x03A2);
        v.extend(0x03A3..0x03AA);
        v.extend(0x03B1..0x03CA);
        v.extend([0x03D1, 0x03DD]);

        // Hebrew
        v.extend(0x05D0..0x05EB);

        // Latin_Extended_Additional
        v.extend([
            0x1E0B, 0x1E0D, 0x1E17, 0x1E22, 0x1E24, 0x1E25, 0x1E2B, 0x1E33, 0x1E37, 0x1E41, 0x1E43,
            0x1E45, 0x1E47, 0x1E53,
        ]);
        v.extend(0x1E59..0x1E5C);
        v.extend([
            0x1E61, 0x1E63, 0x1E6B, 0x1E6D, 0x1E6F, 0x1E91, 0x1E93, 0x1E96, 0x1EA1, 0x1ECD, 0x1EF9,
        ]);

        // General_Punctuation
        v.extend([0x2011, 0x2038, 0x203D, 0x2042]);

        // Arrows
        v.extend([0x2190, 0x2192]);

        // Mathematical_Operators
        v.extend([
            0x2202, 0x221A, 0x221E, 0x2225, 0x222B, 0x2260, 0x2294, 0x2295, 0x22EE,
        ]);

        // Enclosed_Alphanumerics
        v.push(0x24CA);

        // Miscellaneous_Symbols
        v.extend(0x261C..0x2641);
        v.extend(0x2642..0x2648);
        v.extend(0x2660..0x2664);
        v.extend(0x266D..0x2670);

        // Dingbats
        v.extend([0x2713, 0x2720]);

        // Private_Use_Area
        v.extend(0xE000..0xE01D);
        v.extend(0xE01E..0xE029);
        v.extend(0xE02A..0xE052);

        // Alphabetic_Presentation_Forms
        v.extend([0xFB02, 0xFB2A, 0xFB2B]);

        v.into_iter().collect()
    })
}

/// The PML escape code for a character outside ASCII: `\aNNN` if it has
/// a Windows-1252 byte value in [`a_chars`], `\UXXXX` (uppercase hex) if
/// its code point is in [`u_chars`], or `?` if neither table covers it.
///
/// Port of `unipmlcode`.
pub fn unipmlcode(c: char) -> String {
    let as_str = c.to_string();
    let (cow, _enc, had_errors) = encoding_rs::WINDOWS_1252.encode(&as_str);
    if !had_errors && cow.len() == 1 {
        let val = cow[0] as u16;
        if a_chars().contains(&val) {
            return format!("\\a{val}");
        }
    }
    let val = c as u32;
    if u_chars().contains(&val) {
        // Python builds this as `'\\U%04x'.upper() % val`: the format
        // *string* is uppercased before substitution, turning `%04x`
        // into `%04X` -- so the result is uppercase hex, not lowercase.
        format!("\\U{val:04X}")
    } else {
        "?".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_leaning_cp1252_chars_use_the_a_code() {
        // 0x93 (147) is a cp1252 curly left double quote, in A_CHARS.
        assert_eq!(unipmlcode('\u{201C}'), "\\a147");
    }

    #[test]
    fn extended_unicode_chars_use_the_u_code_in_uppercase_hex() {
        // 0x0100 (Latin Capital Letter A with Macron) is in U_CHARS.
        assert_eq!(unipmlcode('\u{0100}'), "\\U0100");
        // 0x01BF is in U_CHARS and exercises actual hex letters.
        assert_eq!(unipmlcode('\u{01BF}'), "\\U01BF");
    }

    #[test]
    fn an_uncovered_character_becomes_a_question_mark() {
        // CJK characters are outside both tables.
        assert_eq!(unipmlcode('\u{4E2D}'), "?");
    }
}
