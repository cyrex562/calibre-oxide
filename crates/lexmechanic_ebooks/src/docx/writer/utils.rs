//! CSS value helpers for the DOCX writer.
//!
//! Port of `old_src/src/calibre/ebooks/docx/writer/utils.py`, whose
//! `convert_color` delegates to `tinycss.color3.parse_color_string`
//! ([`crate::oeb::color3`], issue #583 -- originally implemented
//! privately in this file, promoted to a shared module once
//! `mobi::utils::convert_color_for_font_tag` turned out to need the
//! exact same real upstream function). DOCX wants `RRGGBB` hex, or the
//! literal `auto` for "whatever the reader's default is".
//!
//! A fully transparent colour resolves to nothing at all rather than to
//! a hex value — DOCX has no alpha channel, so `transparent` has to
//! mean "do not emit this property" or the text would come out black.

use crate::oeb::color3::{parse_color_string, CssColor};

/// Convert a CSS colour to the `RRGGBB` hex DOCX wants, or `auto` for
/// `currentColor`.
///
/// Returns `None` for an empty value, an unparseable one, or a
/// (near-)transparent one — all of which mean "emit no colour here".
///
/// Port of the Python `convert_color`.
pub fn convert_color(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.is_empty() {
        return None;
    }
    match parse_color_string(value)? {
        CssColor::Current => Some("auto".to_string()),
        CssColor::Rgba(c) => {
            if c.alpha < 0.01 {
                return None;
            }
            // Truncation, not rounding, matching Python's int().
            Some(format!(
                "{:02X}{:02X}{:02X}",
                (c.red * 255.0) as i64 as u8,
                (c.green * 255.0) as i64 as u8,
                (c.blue * 255.0) as i64 as u8
            ))
        }
    }
}

/// Parse an integer, treating anything unparseable as zero.
///
/// Port of the Python `int_or_zero`.
pub fn int_or_zero(raw: Option<&str>) -> i64 {
    raw.and_then(|r| r.trim().parse::<i64>().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The assertions from calibre's own `test_convert_color`, verbatim.
    #[test]
    fn matches_calibres_colour_conversion_tests() {
        assert_eq!(convert_color(None), None);
        assert_eq!(convert_color(Some("transparent")), None);
        assert_eq!(convert_color(Some("none")), None);
        assert_eq!(convert_color(Some("#12j456")), None);
        assert_eq!(convert_color(Some("currentColor")).as_deref(), Some("auto"));
        assert_eq!(convert_color(Some("AliceBlue")).as_deref(), Some("F0F8FF"));
        assert_eq!(convert_color(Some("black")).as_deref(), Some("000000"));
        assert_eq!(convert_color(Some("red")).as_deref(), Some("FF0000"));
        assert_eq!(convert_color(Some("lime")).as_deref(), Some("00FF00"));
        assert_eq!(convert_color(Some("#001")).as_deref(), Some("000011"));
        assert_eq!(convert_color(Some("#12345d")).as_deref(), Some("12345D"));
        assert_eq!(
            convert_color(Some("rgb(255, 255, 255)")).as_deref(),
            Some("FFFFFF")
        );
        // Alpha out of range is clipped to 1 rather than rejected.
        assert_eq!(
            convert_color(Some("rgba(255, 0, 0, 23)")).as_deref(),
            Some("FF0000")
        );
    }

    #[test]
    fn a_fully_transparent_colour_emits_nothing() {
        // DOCX has no alpha, so `rgba(0,0,0,0)` must not come out black.
        assert_eq!(convert_color(Some("rgba(0, 0, 0, 0)")), None);
        assert_eq!(convert_color(Some("rgba(255, 0, 0, 0.001)")), None);
        // Just above the threshold it is kept.
        assert_eq!(
            convert_color(Some("rgba(255, 0, 0, 0.5)")).as_deref(),
            Some("FF0000")
        );
    }

    #[test]
    fn percentages_and_hsl_are_understood() {
        assert_eq!(
            convert_color(Some("rgb(100%, 0%, 0%)")).as_deref(),
            Some("FF0000")
        );
        assert_eq!(
            convert_color(Some("hsl(0, 100%, 50%)")).as_deref(),
            Some("FF0000")
        );
        assert_eq!(
            convert_color(Some("hsl(120, 100%, 50%)")).as_deref(),
            Some("00FF00")
        );
        assert_eq!(
            convert_color(Some("hsl(0, 0%, 100%)")).as_deref(),
            Some("FFFFFF")
        );
        assert_eq!(
            convert_color(Some("hsla(240, 100%, 50%, 0.9)")).as_deref(),
            Some("0000FF")
        );
        // Hue wraps rather than being rejected.
        assert_eq!(
            convert_color(Some("hsl(480, 100%, 50%)")).as_deref(),
            convert_color(Some("hsl(120, 100%, 50%)")).as_deref()
        );
    }

    #[test]
    fn malformed_values_are_rejected_rather_than_guessed() {
        for bad in [
            "",
            "   ",
            "#",
            "#1234",
            "#1234567",
            "rgb(1, 2)",
            "rgb(1, 2, 3, 4)",
            "rgba(1, 2, 3)",
            // tinycss requires all three channels to be the same kind.
            "rgb(100%, 0, 0)",
            // hsl wants degrees then two percentages.
            "hsl(0, 100, 50)",
            "notacolour",
            "rgb(a, b, c)",
        ] {
            assert_eq!(convert_color(Some(bad)), None, "should reject {bad:?}");
        }
    }

    #[test]
    fn keyword_lookup_is_case_insensitive() {
        // The keyword table itself (COLOR_KEYWORDS/keyword_map,
        // including the `transparent`/`rebeccapurple` checks) now lives
        // in crate::oeb::color3's own tests, since it moved there under
        // issue #583.
        assert_eq!(
            convert_color(Some("DarkOliveGreen")),
            convert_color(Some("darkolivegreen"))
        );
    }

    #[test]
    fn int_or_zero_swallows_everything_unparseable() {
        assert_eq!(int_or_zero(Some("700")), 700);
        assert_eq!(int_or_zero(Some(" 42 ")), 42);
        assert_eq!(int_or_zero(Some("-3")), -3);
        assert_eq!(int_or_zero(Some("bold")), 0);
        assert_eq!(int_or_zero(Some("1.5")), 0);
        assert_eq!(int_or_zero(None), 0);
    }
}
