//! CSS value helpers for the DOCX writer.
//!
//! Port of `old_src/src/calibre/ebooks/docx/writer/utils.py`, whose
//! `convert_color` delegates to `tinycss.color3.parse_color_string`.
//! Both are ported here, since tinycss is a whole CSS tokenizer and
//! only its colour grammar is needed: DOCX wants `RRGGBB` hex, or the
//! literal `auto` for "whatever the reader's default is".
//!
//! A fully transparent colour resolves to nothing at all rather than to
//! a hex value — DOCX has no alpha channel, so `transparent` has to
//! mean "do not emit this property" or the text would come out black.

use std::collections::HashMap;
use std::sync::OnceLock;

/// A parsed CSS colour, channels in `0..=1`. Out-of-range channels are
/// preserved (`rgb(-51, 306, 0)` is legal CSS and clamps at use), while
/// alpha is clipped, as in tinycss.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgba {
    pub red: f64,
    pub green: f64,
    pub blue: f64,
    pub alpha: f64,
}

/// What a CSS colour value resolves to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CssColor {
    /// The `currentColor` keyword, which DOCX spells `auto`.
    Current,
    Rgba(Rgba),
}

/// The 148 CSS colour keywords, plus `transparent`, as
/// `(name, r, g, b, a)` with channels in `0..=255`. Sorted by name so
/// lookups can binary-search, though the map below is what is used.
#[rustfmt::skip]
static COLOR_KEYWORDS: &[(&str, u8, u8, u8, u8)] = &[
    ("aliceblue", 240, 248, 255, 1),
    ("antiquewhite", 250, 235, 215, 1),
    ("aqua", 0, 255, 255, 1),
    ("aquamarine", 127, 255, 212, 1),
    ("azure", 240, 255, 255, 1),
    ("beige", 245, 245, 220, 1),
    ("bisque", 255, 228, 196, 1),
    ("black", 0, 0, 0, 1),
    ("blanchedalmond", 255, 235, 205, 1),
    ("blue", 0, 0, 255, 1),
    ("blueviolet", 138, 43, 226, 1),
    ("brown", 165, 42, 42, 1),
    ("burlywood", 222, 184, 135, 1),
    ("cadetblue", 95, 158, 160, 1),
    ("chartreuse", 127, 255, 0, 1),
    ("chocolate", 210, 105, 30, 1),
    ("coral", 255, 127, 80, 1),
    ("cornflowerblue", 100, 149, 237, 1),
    ("cornsilk", 255, 248, 220, 1),
    ("crimson", 220, 20, 60, 1),
    ("cyan", 0, 255, 255, 1),
    ("darkblue", 0, 0, 139, 1),
    ("darkcyan", 0, 139, 139, 1),
    ("darkgoldenrod", 184, 134, 11, 1),
    ("darkgray", 169, 169, 169, 1),
    ("darkgreen", 0, 100, 0, 1),
    ("darkgrey", 169, 169, 169, 1),
    ("darkkhaki", 189, 183, 107, 1),
    ("darkmagenta", 139, 0, 139, 1),
    ("darkolivegreen", 85, 107, 47, 1),
    ("darkorange", 255, 140, 0, 1),
    ("darkorchid", 153, 50, 204, 1),
    ("darkred", 139, 0, 0, 1),
    ("darksalmon", 233, 150, 122, 1),
    ("darkseagreen", 143, 188, 143, 1),
    ("darkslateblue", 72, 61, 139, 1),
    ("darkslategray", 47, 79, 79, 1),
    ("darkslategrey", 47, 79, 79, 1),
    ("darkturquoise", 0, 206, 209, 1),
    ("darkviolet", 148, 0, 211, 1),
    ("deeppink", 255, 20, 147, 1),
    ("deepskyblue", 0, 191, 255, 1),
    ("dimgray", 105, 105, 105, 1),
    ("dimgrey", 105, 105, 105, 1),
    ("dodgerblue", 30, 144, 255, 1),
    ("firebrick", 178, 34, 34, 1),
    ("floralwhite", 255, 250, 240, 1),
    ("forestgreen", 34, 139, 34, 1),
    ("fuchsia", 255, 0, 255, 1),
    ("gainsboro", 220, 220, 220, 1),
    ("ghostwhite", 248, 248, 255, 1),
    ("gold", 255, 215, 0, 1),
    ("goldenrod", 218, 165, 32, 1),
    ("gray", 128, 128, 128, 1),
    ("green", 0, 128, 0, 1),
    ("greenyellow", 173, 255, 47, 1),
    ("grey", 128, 128, 128, 1),
    ("honeydew", 240, 255, 240, 1),
    ("hotpink", 255, 105, 180, 1),
    ("indianred", 205, 92, 92, 1),
    ("indigo", 75, 0, 130, 1),
    ("ivory", 255, 255, 240, 1),
    ("khaki", 240, 230, 140, 1),
    ("lavender", 230, 230, 250, 1),
    ("lavenderblush", 255, 240, 245, 1),
    ("lawngreen", 124, 252, 0, 1),
    ("lemonchiffon", 255, 250, 205, 1),
    ("lightblue", 173, 216, 230, 1),
    ("lightcoral", 240, 128, 128, 1),
    ("lightcyan", 224, 255, 255, 1),
    ("lightgoldenrodyellow", 250, 250, 210, 1),
    ("lightgray", 211, 211, 211, 1),
    ("lightgreen", 144, 238, 144, 1),
    ("lightgrey", 211, 211, 211, 1),
    ("lightpink", 255, 182, 193, 1),
    ("lightsalmon", 255, 160, 122, 1),
    ("lightseagreen", 32, 178, 170, 1),
    ("lightskyblue", 135, 206, 250, 1),
    ("lightslategray", 119, 136, 153, 1),
    ("lightslategrey", 119, 136, 153, 1),
    ("lightsteelblue", 176, 196, 222, 1),
    ("lightyellow", 255, 255, 224, 1),
    ("lime", 0, 255, 0, 1),
    ("limegreen", 50, 205, 50, 1),
    ("linen", 250, 240, 230, 1),
    ("magenta", 255, 0, 255, 1),
    ("maroon", 128, 0, 0, 1),
    ("mediumaquamarine", 102, 205, 170, 1),
    ("mediumblue", 0, 0, 205, 1),
    ("mediumorchid", 186, 85, 211, 1),
    ("mediumpurple", 147, 112, 219, 1),
    ("mediumseagreen", 60, 179, 113, 1),
    ("mediumslateblue", 123, 104, 238, 1),
    ("mediumspringgreen", 0, 250, 154, 1),
    ("mediumturquoise", 72, 209, 204, 1),
    ("mediumvioletred", 199, 21, 133, 1),
    ("midnightblue", 25, 25, 112, 1),
    ("mintcream", 245, 255, 250, 1),
    ("mistyrose", 255, 228, 225, 1),
    ("moccasin", 255, 228, 181, 1),
    ("navajowhite", 255, 222, 173, 1),
    ("navy", 0, 0, 128, 1),
    ("oldlace", 253, 245, 230, 1),
    ("olive", 128, 128, 0, 1),
    ("olivedrab", 107, 142, 35, 1),
    ("orange", 255, 165, 0, 1),
    ("orangered", 255, 69, 0, 1),
    ("orchid", 218, 112, 214, 1),
    ("palegoldenrod", 238, 232, 170, 1),
    ("palegreen", 152, 251, 152, 1),
    ("paleturquoise", 175, 238, 238, 1),
    ("palevioletred", 219, 112, 147, 1),
    ("papayawhip", 255, 239, 213, 1),
    ("peachpuff", 255, 218, 185, 1),
    ("peru", 205, 133, 63, 1),
    ("pink", 255, 192, 203, 1),
    ("plum", 221, 160, 221, 1),
    ("powderblue", 176, 224, 230, 1),
    ("purple", 128, 0, 128, 1),
    ("red", 255, 0, 0, 1),
    ("rosybrown", 188, 143, 143, 1),
    ("royalblue", 65, 105, 225, 1),
    ("saddlebrown", 139, 69, 19, 1),
    ("salmon", 250, 128, 114, 1),
    ("sandybrown", 244, 164, 96, 1),
    ("seagreen", 46, 139, 87, 1),
    ("seashell", 255, 245, 238, 1),
    ("sienna", 160, 82, 45, 1),
    ("silver", 192, 192, 192, 1),
    ("skyblue", 135, 206, 235, 1),
    ("slateblue", 106, 90, 205, 1),
    ("slategray", 112, 128, 144, 1),
    ("slategrey", 112, 128, 144, 1),
    ("snow", 255, 250, 250, 1),
    ("springgreen", 0, 255, 127, 1),
    ("steelblue", 70, 130, 180, 1),
    ("tan", 210, 180, 140, 1),
    ("teal", 0, 128, 128, 1),
    ("thistle", 216, 191, 216, 1),
    ("tomato", 255, 99, 71, 1),
    ("transparent", 0, 0, 0, 0),
    ("turquoise", 64, 224, 208, 1),
    ("violet", 238, 130, 238, 1),
    ("wheat", 245, 222, 179, 1),
    ("white", 255, 255, 255, 1),
    ("whitesmoke", 245, 245, 245, 1),
    ("yellow", 255, 255, 0, 1),
    ("yellowgreen", 154, 205, 50, 1),
];

fn keyword_map() -> &'static HashMap<&'static str, Rgba> {
    static MAP: OnceLock<HashMap<&'static str, Rgba>> = OnceLock::new();
    MAP.get_or_init(|| {
        COLOR_KEYWORDS
            .iter()
            .map(|(name, r, g, b, a)| {
                (
                    *name,
                    Rgba {
                        red: *r as f64 / 255.0,
                        green: *g as f64 / 255.0,
                        blue: *b as f64 / 255.0,
                        alpha: *a as f64,
                    },
                )
            })
            .collect()
    })
}

/// Parse a CSS colour value.
///
/// Port of `tinycss.color3.parse_color_string`, restricted to the
/// forms that grammar accepts: keywords, `#rgb`, `#rrggbb`, and the
/// `rgb()`/`rgba()`/`hsl()`/`hsla()` functions.
pub fn parse_color_string(value: &str) -> Option<CssColor> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if value.eq_ignore_ascii_case("currentcolor") {
        return Some(CssColor::Current);
    }
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hash(hex).map(CssColor::Rgba);
    }
    if let Some((name, rest)) = value.split_once('(') {
        let args = rest.strip_suffix(')')?;
        return parse_function(&name.trim().to_lowercase(), args).map(CssColor::Rgba);
    }
    keyword_map()
        .get(value.to_lowercase().as_str())
        .copied()
        .map(CssColor::Rgba)
}

/// `#rgb` and `#rrggbb`. Any other length is not a colour.
fn parse_hash(hex: &str) -> Option<Rgba> {
    let channel = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f64 / 255.0);
    let (r, g, b) = match hex.len() {
        3 => {
            let d: Vec<char> = hex.chars().collect();
            (
                channel(&format!("{}{}", d[0], d[0]))?,
                channel(&format!("{}{}", d[1], d[1]))?,
                channel(&format!("{}{}", d[2], d[2]))?,
            )
        }
        6 => (
            channel(&hex[0..2])?,
            channel(&hex[2..4])?,
            channel(&hex[4..6])?,
        ),
        _ => return None,
    };
    Some(Rgba {
        red: r,
        green: g,
        blue: b,
        alpha: 1.0,
    })
}

/// One `rgb()`/`hsl()` argument: a plain number or a percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Arg {
    Number(f64),
    Percentage(f64),
}

fn parse_args(raw: &str) -> Option<Vec<Arg>> {
    let mut args = Vec::new();
    for part in raw.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        args.push(match part.strip_suffix('%') {
            Some(num) => Arg::Percentage(num.trim().parse().ok()?),
            None => Arg::Number(part.parse().ok()?),
        });
    }
    Some(args)
}

fn parse_function(name: &str, raw: &str) -> Option<Rgba> {
    let args = parse_args(raw)?;
    let (expected, has_alpha) = match name {
        "rgb" | "hsl" => (3, false),
        "rgba" | "hsla" => (4, true),
        _ => return None,
    };
    if args.len() != expected {
        return None;
    }
    let alpha = if has_alpha {
        match args[3] {
            Arg::Number(v) => v.clamp(0.0, 1.0),
            Arg::Percentage(_) => return None,
        }
    } else {
        1.0
    };

    match name {
        "rgb" | "rgba" => {
            // tinycss accepts three integers or three percentages, not
            // a mixture.
            let all_numbers = args[..3].iter().all(|a| matches!(a, Arg::Number(_)));
            let all_pct = args[..3].iter().all(|a| matches!(a, Arg::Percentage(_)));
            let channels: Vec<f64> = args[..3]
                .iter()
                .map(|a| match a {
                    Arg::Number(v) => v / 255.0,
                    Arg::Percentage(v) => v / 100.0,
                })
                .collect();
            if !(all_numbers || all_pct) {
                return None;
            }
            Some(Rgba {
                red: channels[0],
                green: channels[1],
                blue: channels[2],
                alpha,
            })
        }
        _ => {
            // hsl: one number (degrees) then two percentages.
            let (Arg::Number(h), Arg::Percentage(s), Arg::Percentage(l)) =
                (args[0], args[1], args[2])
            else {
                return None;
            };
            let (r, g, b) = hsl_to_rgb(h, s, l);
            Some(Rgba {
                red: r,
                green: g,
                blue: b,
                alpha,
            })
        }
    }
}

/// Port of the Python `hsl_to_rgb`, itself a transcription of the ABC
/// in the CSS Color 3 spec.
fn hsl_to_rgb(hue: f64, saturation: f64, lightness: f64) -> (f64, f64, f64) {
    let hue = (hue / 360.0).rem_euclid(1.0);
    let saturation = (saturation / 100.0).clamp(0.0, 1.0);
    let lightness = (lightness / 100.0).clamp(0.0, 1.0);

    fn hue_to_rgb(m1: f64, m2: f64, mut h: f64) -> f64 {
        if h < 0.0 {
            h += 1.0;
        }
        if h > 1.0 {
            h -= 1.0;
        }
        if h * 6.0 < 1.0 {
            return m1 + (m2 - m1) * h * 6.0;
        }
        if h * 2.0 < 1.0 {
            return m2;
        }
        if h * 3.0 < 2.0 {
            return m1 + (m2 - m1) * (2.0 / 3.0 - h) * 6.0;
        }
        m1
    }

    let m2 = if lightness <= 0.5 {
        lightness * (saturation + 1.0)
    } else {
        lightness + saturation - lightness * saturation
    };
    let m1 = lightness * 2.0 - m2;
    (
        hue_to_rgb(m1, m2, hue + 1.0 / 3.0),
        hue_to_rgb(m1, m2, hue),
        hue_to_rgb(m1, m2, hue - 1.0 / 3.0),
    )
}

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
    fn the_keyword_table_is_complete() {
        // 147 named colours plus `transparent`.
        assert_eq!(COLOR_KEYWORDS.len(), 148);
        // tinycss's table is CSS Color 3, which predates
        // `rebeccapurple` — so calibre does not know it either.
        assert!(!keyword_map().contains_key("rebeccapurple"));
        assert!(keyword_map().contains_key("lightgoldenrodyellow"));
        assert_eq!(
            keyword_map().get("transparent").map(|c| c.alpha),
            Some(0.0),
            "transparent must carry zero alpha, which is what suppresses it"
        );
        // Keyword lookup is case-insensitive.
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
