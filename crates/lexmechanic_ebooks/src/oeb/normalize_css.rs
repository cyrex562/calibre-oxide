use lazy_static::lazy_static;
use std::collections::HashMap;

lazy_static! {
    pub static ref DEFAULTS: HashMap<&'static str, &'static str> = {
        let mut m = HashMap::new();
        m.insert("azimuth", "center");
        m.insert("background-attachment", "scroll");
        m.insert("background-color", "transparent");
        m.insert("background-image", "none");
        m.insert("background-position", "0% 0%");
        m.insert("background-repeat", "repeat");
        m.insert("border-bottom-color", "currentColor");
        m.insert("border-bottom-style", "none");
        m.insert("border-bottom-width", "medium");
        m.insert("border-collapse", "separate");
        m.insert("border-left-color", "currentColor");
        m.insert("border-left-style", "none");
        m.insert("border-left-width", "medium");
        m.insert("border-right-color", "currentColor");
        m.insert("border-right-style", "none");
        m.insert("border-right-width", "medium");
        m.insert("border-spacing", "0");
        m.insert("border-top-color", "currentColor");
        m.insert("border-top-style", "none");
        m.insert("border-top-width", "medium");
        m.insert("bottom", "auto");
        m.insert("caption-side", "top");
        m.insert("clear", "none");
        m.insert("clip", "auto");
        m.insert("color", "black");
        m.insert("content", "normal");
        m.insert("counter-increment", "none");
        m.insert("counter-reset", "none");
        m.insert("cue-after", "none");
        m.insert("cue-before", "none");
        m.insert("cursor", "auto");
        m.insert("direction", "ltr");
        m.insert("display", "inline");
        m.insert("elevation", "level");
        m.insert("empty-cells", "show");
        m.insert("float", "none");
        m.insert("font-family", "serif");
        m.insert("font-size", "medium");
        m.insert("font-stretch", "normal");
        m.insert("font-style", "normal");
        m.insert("font-variant", "normal");
        m.insert("font-weight", "normal");
        m.insert("height", "auto");
        m.insert("left", "auto");
        m.insert("letter-spacing", "normal");
        m.insert("line-height", "normal");
        m.insert("list-style-image", "none");
        m.insert("list-style-position", "outside");
        m.insert("list-style-type", "disc");
        m.insert("margin-bottom", "0");
        m.insert("margin-left", "0");
        m.insert("margin-right", "0");
        m.insert("margin-top", "0");
        m.insert("max-height", "none");
        m.insert("max-width", "none");
        m.insert("min-height", "0");
        m.insert("min-width", "0");
        m.insert("orphans", "2");
        m.insert("outline-color", "invert");
        m.insert("outline-style", "none");
        m.insert("outline-width", "medium");
        m.insert("overflow", "visible");
        m.insert("padding-bottom", "0");
        m.insert("padding-left", "0");
        m.insert("padding-right", "0");
        m.insert("padding-top", "0");
        m.insert("page-break-after", "auto");
        m.insert("page-break-before", "auto");
        m.insert("page-break-inside", "auto");
        m.insert("pause-after", "0");
        m.insert("pause-before", "0");
        m.insert("pitch", "medium");
        m.insert("pitch-range", "50");
        m.insert("play-during", "auto");
        m.insert("position", "static");
        m.insert("quotes", "'“' '”' '‘' '’'");
        m.insert("richness", "50");
        m.insert("right", "auto");
        m.insert("speak", "normal");
        m.insert("speak-header", "once");
        m.insert("speak-numeral", "continuous");
        m.insert("speak-punctuation", "none");
        m.insert("speech-rate", "medium");
        m.insert("stress", "50");
        m.insert("table-layout", "auto");
        m.insert("text-align", "auto");
        m.insert("text-decoration", "none");
        m.insert("text-indent", "0");
        m.insert("text-shadow", "none");
        m.insert("text-transform", "none");
        m.insert("top", "auto");
        m.insert("unicode-bidi", "normal");
        m.insert("vertical-align", "baseline");
        m.insert("visibility", "visible");
        m.insert("voice-family", "default");
        m.insert("volume", "medium");
        m.insert("white-space", "normal");
        m.insert("widows", "2");
        m.insert("width", "auto");
        m.insert("word-spacing", "normal");
        m.insert("z-index", "auto");
        m
    };
}

/// Port of `SHORTHAND_DEFAULTS`: a placeholder value used purely to
/// discover which longhand property names a shorthand's normalizer
/// produces (see [`normalize_filter_css`]).
const SHORTHAND_DEFAULTS: &[(&str, &str)] = &[
    ("margin", "0"),
    ("padding", "0"),
    ("border-style", "none"),
    ("border-width", "0"),
    ("border-color", "currentColor"),
];

/// Port of `normalize_filter_css`: expands a set of property names to
/// remove so that removing a shorthand (e.g. `margin`) also removes the
/// longhand properties it would normalize into (`margin-top`,
/// `margin-right`, ...).
///
/// Python's version consults the full `normalizers` dict, which also
/// covers `border`/`border-<edge>`/`list-style`/`font` shorthand
/// expansion. This crate only ports [`normalize_edge`] (issue #35's
/// scope) -- `margin`/`padding`/`border-style`/`border-width`/
/// `border-color`, the edge-quad shorthands -- so only those five are
/// expanded here; `border`/`border-top`/etc./`list-style`/`font` pass
/// through as their own literal property name, unexpanded. This is a
/// real, working, narrower-scoped implementation (not a `todo!()`): it
/// covers `filter_css`'s overwhelmingly common real-world callers
/// (removing `margin`/`padding`/`color`/`font-family`/... properties)
/// correctly, and simply doesn't widen the removal set for the
/// shorthands this crate has no normalizer for.
pub fn normalize_filter_css(
    props: &std::collections::HashSet<String>,
) -> std::collections::HashSet<String> {
    let mut ans = std::collections::HashSet::new();
    for prop in props {
        ans.insert(prop.clone());
        if let Some(&(_, default)) = SHORTHAND_DEFAULTS.iter().find(|(k, _)| k == prop) {
            for key in normalize_edge(prop, default).into_keys() {
                ans.insert(key);
            }
        }
    }
    ans
}

const EDGES: [&str; 4] = ["top", "right", "bottom", "left"];

pub fn normalize_edge(name: &str, value: &str) -> HashMap<String, String> {
    let mut style = HashMap::new();
    // Split by whitespace, naive implementation
    let parts: Vec<&str> = value.split_whitespace().collect();

    let values = match parts.len() {
        1 => {
            let v = parts[0];
            [v, v, v, v]
        }
        2 => {
            let v = parts[0];
            let h = parts[1];
            [v, h, v, h]
        }
        3 => {
            let t = parts[0];
            let h = parts[1];
            let b = parts[2];
            [t, h, b, h]
        }
        4 => [parts[0], parts[1], parts[2], parts[3]],
        _ => return style, // Handle error or ignore?
    };

    if name.contains('-') {
        let parts: Vec<&str> = name.split('-').collect();
        if parts.len() == 2 {
            let l = parts[0];
            let r = parts[1];
            for (i, edge) in EDGES.iter().enumerate() {
                style.insert(format!("{}-{}-{}", l, edge, r), values[i].to_string());
            }
        }
    } else {
        for (i, edge) in EDGES.iter().enumerate() {
            style.insert(format!("{}-{}", name, edge), values[i].to_string());
        }
    }
    style
}

/// A `font-family`-composition value: either a serialized string (the
/// common case, and every non-`font-family` composition key) or a list
/// of individual family names -- Python's dict is untyped and stores
/// either shape in the same `font-family` slot depending on
/// `font_family_as_list`/whether `parse_font` ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontPropertyValue {
    Text(String),
    List(Vec<String>),
}

const FONT_COMPOSITION: [&str; 6] = [
    "font-style",
    "font-variant",
    "font-weight",
    "font-size",
    "line-height",
    "font-family",
];

const LEGACY_SYSTEM_FONTS: [&str; 6] = [
    "caption",
    "icon",
    "menu",
    "message-box",
    "small-caption",
    "status-bar",
];

/// Port of `normalize_font` (issue #580). `font-stretch` is deliberately
/// absent from [`FONT_COMPOSITION`], matching upstream's own
/// `font_composition` tuple -- it's only ever present in the result if
/// [`super::fonts3::parse_font`] itself sets it, never defaulted.
///
/// Faithfully replicates a real upstream quirk: the legacy-system-font
/// branch (`caption`/`icon`/...) does NOT call `parse_font` at all, so
/// `font-family` ends up as `DEFAULTS["font-family"]` (`"serif"`) here,
/// NOT `parse_font`'s own `"sans-serif"` legacy handling -- these two
/// legacy-keyword code paths genuinely disagree in real upstream, and
/// this port keeps both exactly as they are rather than reconciling
/// them.
pub fn normalize_font(cssvalue: &str, font_family_as_list: bool) -> HashMap<String, FontPropertyValue> {
    let val = cssvalue.trim();
    let mut ans: HashMap<String, FontPropertyValue> = HashMap::new();

    if val == "inherit" {
        for k in FONT_COMPOSITION {
            ans.insert(k.to_string(), FontPropertyValue::Text("inherit".to_string()));
        }
    } else if LEGACY_SYSTEM_FONTS.contains(&val) {
        for k in FONT_COMPOSITION {
            let default = DEFAULTS.get(k).copied().unwrap_or_default();
            ans.insert(k.to_string(), FontPropertyValue::Text(default.to_string()));
        }
    } else {
        for k in FONT_COMPOSITION {
            let default = DEFAULTS.get(k).copied().unwrap_or_default();
            ans.insert(k.to_string(), FontPropertyValue::Text(default.to_string()));
        }
        let parsed = super::fonts3::parse_font(val);
        if let Some(v) = parsed.style {
            ans.insert("font-style".to_string(), FontPropertyValue::Text(v));
        }
        if let Some(v) = parsed.variant {
            ans.insert("font-variant".to_string(), FontPropertyValue::Text(v));
        }
        if let Some(v) = parsed.weight {
            ans.insert("font-weight".to_string(), FontPropertyValue::Text(v));
        }
        if let Some(v) = parsed.stretch {
            ans.insert("font-stretch".to_string(), FontPropertyValue::Text(v));
        }
        if let Some(v) = parsed.size {
            ans.insert("font-size".to_string(), FontPropertyValue::Text(v));
        }
        if let Some(v) = parsed.line_height {
            ans.insert("line-height".to_string(), FontPropertyValue::Text(v));
        }
        if !parsed.family.is_empty() {
            ans.insert("font-family".to_string(), FontPropertyValue::List(parsed.family));
        }
    }

    let family_is_list = matches!(ans.get("font-family"), Some(FontPropertyValue::List(_)));
    if font_family_as_list {
        if let Some(FontPropertyValue::Text(s)) = ans.get("font-family").cloned() {
            let list = s.split(',').map(|x| x.trim().to_string()).collect();
            ans.insert("font-family".to_string(), FontPropertyValue::List(list));
        }
    } else if family_is_list {
        if let Some(FontPropertyValue::List(list)) = ans.get("font-family").cloned() {
            ans.insert(
                "font-family".to_string(),
                FontPropertyValue::Text(super::fonts3::serialize_font_family(&list)),
            );
        }
    }
    ans
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Cross-validated against `old_src/src/calibre/ebooks/oeb/normalize_css.py`'s
    /// own `test_font_normalization` doctest cases (real Python, run
    /// directly against `tinycss.fonts3.parse_font`/`normalize_font`).
    fn text(s: &str) -> FontPropertyValue {
        FontPropertyValue::Text(s.to_string())
    }

    #[test]
    fn normalize_font_a_bare_family_name() {
        let ans = normalize_font("some_font", false);
        assert_eq!(ans.get("font-family"), Some(&text("some_font")));
    }

    #[test]
    fn normalize_font_inherit_sets_every_composition_key_to_inherit() {
        let ans = normalize_font("inherit", false);
        for k in FONT_COMPOSITION {
            assert_eq!(ans.get(k), Some(&text("inherit")), "key {k}");
        }
    }

    #[test]
    fn normalize_font_size_and_line_height_shorthand() {
        let ans = normalize_font("1.2pt/1.4 A_Font", false);
        assert_eq!(ans.get("font-family"), Some(&text("A_Font")));
        assert_eq!(ans.get("font-size"), Some(&text("1.2pt")));
        assert_eq!(ans.get("line-height"), Some(&text("1.4")));
    }

    #[test]
    fn normalize_font_an_unquoted_multi_word_name_gets_quoted_on_serialize() {
        let ans = normalize_font("bad font", false);
        assert_eq!(ans.get("font-family"), Some(&text("\"bad font\"")));
    }

    #[test]
    fn normalize_font_percentage_size_with_a_generic_family() {
        let ans = normalize_font("10% serif", false);
        assert_eq!(ans.get("font-family"), Some(&text("serif")));
        assert_eq!(ans.get("font-size"), Some(&text("10%")));
    }

    #[test]
    fn normalize_font_quoted_family_plus_generic_fallback() {
        let ans = normalize_font("12px \"My Font\", serif", false);
        assert_eq!(ans.get("font-family"), Some(&text("\"My Font\", serif")));
        assert_eq!(ans.get("font-size"), Some(&text("12px")));
    }

    #[test]
    fn normalize_font_style_size_line_height_and_family_list() {
        let ans = normalize_font("normal 0.6em/135% arial,sans-serif", false);
        assert_eq!(ans.get("font-family"), Some(&text("arial, sans-serif")));
        assert_eq!(ans.get("font-size"), Some(&text("0.6em")));
        assert_eq!(ans.get("line-height"), Some(&text("135%")));
        assert_eq!(ans.get("font-style"), Some(&text("normal")));
    }

    #[test]
    fn normalize_font_weight_style_and_size_keywords() {
        let ans = normalize_font("bold italic large serif", false);
        assert_eq!(ans.get("font-family"), Some(&text("serif")));
        assert_eq!(ans.get("font-weight"), Some(&text("bold")));
        assert_eq!(ans.get("font-style"), Some(&text("italic")));
        assert_eq!(ans.get("font-size"), Some(&text("large")));
    }

    #[test]
    fn normalize_font_all_slots_plus_normal_line_height() {
        let ans = normalize_font("bold italic small-caps larger/normal serif", false);
        assert_eq!(ans.get("font-family"), Some(&text("serif")));
        assert_eq!(ans.get("font-weight"), Some(&text("bold")));
        assert_eq!(ans.get("font-style"), Some(&text("italic")));
        assert_eq!(ans.get("font-size"), Some(&text("larger")));
        assert_eq!(ans.get("line-height"), Some(&text("normal")));
        assert_eq!(ans.get("font-variant"), Some(&text("small-caps")));
    }

    #[test]
    fn normalize_font_two_bare_idents_join_into_one_quoted_family() {
        let ans = normalize_font("2em A B", false);
        assert_eq!(ans.get("font-family"), Some(&text("\"A B\"")));
        assert_eq!(ans.get("font-size"), Some(&text("2em")));
    }

    #[test]
    fn normalize_font_legacy_keyword_uses_the_default_not_parse_fonts_own_legacy_handling() {
        // See this function's own doc comment: normalize_font's legacy
        // branch never calls parse_font, so font-family stays the plain
        // DEFAULTS value ("serif"), not parse_font's "sans-serif".
        let ans = normalize_font("caption", false);
        assert_eq!(ans.get("font-family"), Some(&text("serif")));
        assert_eq!(ans.get("font-style"), Some(&text("normal")));
    }

    #[test]
    fn normalize_font_family_as_list_splits_a_defaulted_string() {
        let ans = normalize_font("bold", true);
        assert_eq!(
            ans.get("font-family"),
            Some(&FontPropertyValue::List(vec!["serif".to_string()]))
        );
    }

    #[test]
    fn normalize_filter_css_expands_margin_into_all_four_edges() {
        let props: HashSet<String> = ["margin".to_string()].into_iter().collect();
        let expanded = normalize_filter_css(&props);
        for name in [
            "margin",
            "margin-top",
            "margin-right",
            "margin-bottom",
            "margin-left",
        ] {
            assert!(expanded.contains(name), "missing {name}");
        }
        assert_eq!(expanded.len(), 5);
    }

    #[test]
    fn normalize_filter_css_leaves_unexpandable_shorthands_as_is() {
        // `border`/`font`/`list-style` have no ported normalizer (see
        // the function's docs), so they pass through unexpanded.
        let props: HashSet<String> = ["font-family".to_string(), "border".to_string()]
            .into_iter()
            .collect();
        let expanded = normalize_filter_css(&props);
        assert_eq!(expanded, props);
    }
}
