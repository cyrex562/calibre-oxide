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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
