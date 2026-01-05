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
