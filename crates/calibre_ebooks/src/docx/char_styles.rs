//! Run-level (character) formatting: reading `w:rPr` into a property
//! set and turning that set into CSS.
//!
//! Port of `old_src/src/calibre/ebooks/docx/char_styles.py`.
//!
//! As in [`super::block_styles`], `None` is the Python `inherit`
//! sentinel. Run properties add one wrinkle paragraphs do not have:
//! **toggle properties** (bold, italic, caps, ...). Per ECMA-376
//! §17.7.3 these XOR down the style chain rather than being overridden
//! by the nearest specified value, which is why
//! [`RunStyle::TOGGLE_PROPERTIES`] is called out separately — the
//! resolution logic that consumes it lives in the styles module.

use indexmap::IndexMap;
use roxmltree::Node;

use super::block_styles::{
    binary_property, format_g3, line_style, read_shd, simple_color, simple_float, Css,
};
use super::lcid::language_for_lcid;
use super::names::DocxNamespace;

/// A run's `w:bdr` border, which unlike a paragraph's has a single edge.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextBorder {
    pub color: Option<String>,
    pub style: Option<String>,
    pub width: Option<f64>,
}

impl TextBorder {
    fn is_empty(&self) -> bool {
        self.color.is_none() && self.style.is_none() && self.width.is_none()
    }
}

/// Port of the Python `read_text_border`. Returns the border and the
/// padding, which the Python stores as a separate property.
fn read_text_border(parent: Node, ns: &DocxNamespace) -> (TextBorder, Option<f64>) {
    let mut border = TextBorder::default();
    let mut padding = None;
    let elems = ns.children(parent, &["w:bdr"]);

    // A `w:bdr` carrying any attribute at all establishes a border;
    // the individual attributes then refine it.
    if elems.first().is_some_and(|e| e.attributes().len() > 0) {
        border.color = Some(simple_color(Some("auto"), "currentColor"));
        border.style = Some("none".to_string());
        border.width = Some(1.0);
    }

    for elem in elems {
        if let Some(color) = ns.get(elem, "w:color") {
            border.color = Some(simple_color(Some(color), "currentColor"));
        }
        if let Some(style) = ns.get(elem, "w:val") {
            border.style = Some(line_style(style).to_string());
        }
        if let Some(space) = ns.get(elem, "w:space") {
            if let Ok(v) = space.trim().parse::<f64>() {
                padding = Some(v);
            }
        }
        if let Some(sz) = ns.get(elem, "w:sz") {
            // Floored at 8 eighths: WebKit does not render a border
            // thinner than 1pt.
            if let Ok(v) = sz.trim().parse::<f64>() {
                border.width = Some(v.clamp(8.0, 96.0) / 8.0);
            }
        }
    }
    (border, padding)
}

/// Word's named highlight colours that CSS does not know.
///
/// Port of the Python `convert_highlight_color`.
pub fn convert_highlight_color(val: &str) -> &str {
    match val {
        "darkBlue" => "#000080",
        "darkCyan" => "#008080",
        "darkGray" => "#808080",
        "darkGreen" => "#008000",
        "darkMagenta" => "#800080",
        "darkRed" => "#800000",
        "darkYellow" => "#808000",
        "lightGray" => "#c0c0c0",
        other => other,
    }
}

/// Port of the Python `read_underline`. Returns a CSS `text-decoration`
/// value.
fn read_underline(parent: Node, ns: &DocxNamespace) -> Option<String> {
    let mut ans = None;
    for u in ns.children(parent, &["w:u"]) {
        let Some(val) = ns.get(u, "w:val").filter(|v| !v.is_empty()) else {
            continue;
        };
        let style = match val {
            "dotted" | "dashDotDotHeavy" | "dotDash" | "dotDotDash" | "dottedHeavy" => "dotted",
            "dash" | "dashDotHeavy" | "dashedHeavy" | "dashLong" | "dashLongHeavy" => "dashed",
            "double" => "double",
            "none" => "none",
            "single" | "thick" | "words" => "solid",
            "wave" | "wavyDouble" | "wavyHeavy" => "wavy",
            _ => "solid",
        };
        if style == "none" {
            ans = Some("none".to_string());
        } else {
            let mut decoration = format!("underline {style}");
            if let Some(color) = ns
                .get(u, "w:color")
                .filter(|c| !c.is_empty() && *c != "auto")
            {
                decoration.push_str(" #");
                decoration.push_str(color);
            }
            ans = Some(decoration);
        }
    }
    ans
}

/// Port of the Python `read_lang`. A hex LCID resolves to a language
/// code; anything else is passed through as written.
fn read_lang(parent: Node, ns: &DocxNamespace) -> Option<String> {
    let mut ans = None;
    for elem in ns.children(parent, &["w:lang"]) {
        let Some(val) = ns.get(elem, "w:val").filter(|v| !v.is_empty()) else {
            continue;
        };
        match u32::from_str_radix(val, 16) {
            Ok(code) => {
                if let Some(lang) = language_for_lcid(code) {
                    ans = Some(lang.to_string());
                }
            }
            Err(_) => ans = Some(val.to_string()),
        }
    }
    ans
}

/// The resolved (or partially resolved) formatting of one run.
///
/// Port of the Python `RunStyle`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunStyle {
    // Toggle and on/off properties.
    pub b: Option<bool>,
    pub b_cs: Option<bool>,
    pub caps: Option<bool>,
    pub cs: Option<bool>,
    pub dstrike: Option<bool>,
    pub emboss: Option<bool>,
    pub i: Option<bool>,
    pub i_cs: Option<bool>,
    pub imprint: Option<bool>,
    pub rtl: Option<bool>,
    pub shadow: Option<bool>,
    pub small_caps: Option<bool>,
    pub strike: Option<bool>,
    pub vanish: Option<bool>,
    pub web_hidden: Option<bool>,

    pub border: TextBorder,
    pub padding: Option<f64>,
    pub color: Option<String>,
    pub highlight: Option<String>,
    pub background_color: Option<String>,
    pub letter_spacing: Option<f64>,
    pub font_size: Option<f64>,
    pub text_decoration: Option<String>,
    pub vert_align: Option<String>,
    pub lang: Option<String>,
    pub font_family: Option<String>,
    pub position: Option<f64>,
    pub cs_font_size: Option<f64>,
    pub cs_font_family: Option<String>,

    /// The `w:rStyle` this run links to, if any.
    pub linked_style: Option<String>,
}

impl RunStyle {
    /// The properties that XOR down the style chain rather than being
    /// overridden, per ECMA-376 §17.7.3.
    pub const TOGGLE_PROPERTIES: [&'static str; 11] = [
        "b",
        "bCs",
        "caps",
        "emboss",
        "i",
        "iCs",
        "imprint",
        "shadow",
        "smallCaps",
        "strike",
        "vanish",
    ];

    /// An all-inherit style.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a `w:rPr` element.
    pub fn from_rpr(rpr: Node, ns: &DocxNamespace) -> Self {
        let mut s = Self::new();
        s.b = binary_property(rpr, "b", ns);
        s.b_cs = binary_property(rpr, "bCs", ns);
        s.caps = binary_property(rpr, "caps", ns);
        s.cs = binary_property(rpr, "cs", ns);
        s.dstrike = binary_property(rpr, "dstrike", ns);
        s.emboss = binary_property(rpr, "emboss", ns);
        s.i = binary_property(rpr, "i", ns);
        s.i_cs = binary_property(rpr, "iCs", ns);
        s.imprint = binary_property(rpr, "imprint", ns);
        s.rtl = binary_property(rpr, "rtl", ns);
        s.shadow = binary_property(rpr, "shadow", ns);
        s.small_caps = binary_property(rpr, "smallCaps", ns);
        s.strike = binary_property(rpr, "strike", ns);
        s.vanish = binary_property(rpr, "vanish", ns);
        s.web_hidden = binary_property(rpr, "webHidden", ns);

        read_font(rpr, ns, &mut s);
        read_font_cs(rpr, ns, &mut s);

        let (border, padding) = read_text_border(rpr, ns);
        s.border = border;
        s.padding = padding;

        for col in ns.children(rpr, &["w:color"]) {
            if let Some(val) = ns.get(col, "w:val").filter(|v| !v.is_empty()) {
                s.color = Some(simple_color(Some(val), "currentColor"));
            }
        }
        for col in ns.children(rpr, &["w:highlight"]) {
            if let Some(val) = ns.get(col, "w:val").filter(|v| !v.is_empty()) {
                s.highlight = Some(if val == "none" {
                    "transparent".to_string()
                } else {
                    convert_highlight_color(val).to_string()
                });
            }
        }
        s.background_color = read_shd(rpr, ns);

        // `w:spacing` on a run is letter spacing, in twentieths of a
        // point — a different element from the paragraph-level one.
        for elem in ns.children(rpr, &["w:spacing"]) {
            if let Some(v) = simple_float(ns.get(elem, "w:val"), 0.05) {
                s.letter_spacing = Some(v);
            }
        }
        s.text_decoration = read_underline(rpr, ns);
        for elem in ns.children(rpr, &["w:vertAlign"]) {
            if let Some(val) = ns
                .get(elem, "w:val")
                .filter(|v| matches!(*v, "baseline" | "subscript" | "superscript"))
            {
                s.vert_align = Some(val.to_string());
            }
        }
        // `w:position` is in half-points.
        for elem in ns.children(rpr, &["w:position"]) {
            if let Some(v) = simple_float(ns.get(elem, "w:val"), 0.5) {
                s.position = Some(v);
            }
        }
        s.lang = read_lang(rpr, ns);

        for elem in ns.children(rpr, &["w:rStyle"]) {
            if let Some(val) = ns.get(elem, "w:val") {
                s.linked_style = Some(val.to_string());
            }
        }
        s
    }

    /// Overlay `other`'s specified properties onto this one.
    pub fn update(&mut self, other: &RunStyle) {
        macro_rules! overlay {
            ($($field:ident),* $(,)?) => {
                $(if other.$field.is_some() { self.$field.clone_from(&other.$field); })*
            };
        }
        overlay!(
            b,
            b_cs,
            caps,
            cs,
            dstrike,
            emboss,
            i,
            i_cs,
            imprint,
            rtl,
            shadow,
            small_caps,
            strike,
            vanish,
            web_hidden,
            padding,
            color,
            highlight,
            background_color,
            letter_spacing,
            font_size,
            text_decoration,
            vert_align,
            lang,
            font_family,
            position,
            cs_font_size,
            cs_font_family,
        );
        if !other.border.is_empty() {
            if other.border.color.is_some() {
                self.border.color.clone_from(&other.border.color);
            }
            if other.border.style.is_some() {
                self.border.style.clone_from(&other.border.style);
            }
            if other.border.width.is_some() {
                self.border.width = other.border.width;
            }
        }
        if other.linked_style.is_some() {
            self.linked_style.clone_from(&other.linked_style);
        }
    }

    /// Fill every inherited property from `parent`.
    pub fn resolve_based_on(&mut self, parent: &RunStyle) {
        macro_rules! inherit {
            ($($field:ident),* $(,)?) => {
                $(if self.$field.is_none() { self.$field.clone_from(&parent.$field); })*
            };
        }
        inherit!(
            b,
            b_cs,
            caps,
            cs,
            dstrike,
            emboss,
            i,
            i_cs,
            imprint,
            rtl,
            shadow,
            small_caps,
            strike,
            vanish,
            web_hidden,
            padding,
            color,
            highlight,
            background_color,
            letter_spacing,
            font_size,
            text_decoration,
            vert_align,
            lang,
            font_family,
            position,
            cs_font_size,
            cs_font_family,
        );
        if self.border.color.is_none() {
            self.border.color.clone_from(&parent.border.color);
        }
        if self.border.style.is_none() {
            self.border.style.clone_from(&parent.border.style);
        }
        if self.border.width.is_none() {
            self.border.width = parent.border.width;
        }
    }

    /// Emit the run's border declarations.
    ///
    /// Port of the Python `get_border_css`.
    pub fn border_css(&self, css: &mut Css) {
        if let Some(color) = &self.border.color {
            css.insert("border-color".to_string(), color.clone());
        }
        if let Some(style) = &self.border.style {
            css.insert("border-style".to_string(), style.clone());
        }
        if let Some(width) = self.border.width {
            css.insert(
                "border-width".to_string(),
                format!("{}pt", format_g3(width)),
            );
        }
    }

    /// Port of the Python `clear_border_css`.
    pub fn clear_border_css(&mut self) {
        self.border = TextBorder::default();
    }

    /// Whether two runs carry the same border, the test for merging
    /// adjacent runs.
    ///
    /// Port of the Python `same_border`.
    pub fn same_border(&self, other: &RunStyle) -> bool {
        self.border == other.border
    }

    /// The CSS for this run.
    ///
    /// Port of the Python `RunStyle.css`.
    pub fn css(&self) -> Css {
        let mut c = IndexMap::new();

        // Python accumulates the decorations in a `set`, so a run that
        // is both underlined and struck through gets them in arbitrary
        // order. Here the order is fixed: underline first.
        let mut decorations: Vec<&str> = Vec::new();
        if let Some(td) = &self.text_decoration {
            decorations.push(td);
        }
        if self.strike == Some(true) || self.dstrike == Some(true) {
            decorations.push("line-through");
        }
        if !decorations.is_empty() {
            c.insert("text-decoration".to_string(), decorations.join(" "));
        }

        if self.caps == Some(true) {
            c.insert("text-transform".to_string(), "uppercase".to_string());
        }
        if self.i == Some(true) {
            c.insert("font-style".to_string(), "italic".to_string());
        }
        if self.shadow == Some(true) {
            c.insert("text-shadow".to_string(), "2px 2px".to_string());
        }
        if self.small_caps == Some(true) {
            c.insert("font-variant".to_string(), "small-caps".to_string());
        }
        if self.vanish == Some(true) || self.web_hidden == Some(true) {
            c.insert("display".to_string(), "none".to_string());
        }

        self.border_css(&mut c);
        if let Some(padding) = self.padding {
            c.insert("padding".to_string(), format!("{}pt", format_g3(padding)));
        }
        if let Some(v) = &self.color {
            c.insert("color".to_string(), v.clone());
        }
        if let Some(v) = &self.background_color {
            c.insert("background-color".to_string(), v.clone());
        }
        if let Some(v) = self.letter_spacing {
            c.insert("letter-spacing".to_string(), format!("{}pt", format_g3(v)));
        }
        if let Some(v) = self.font_size {
            c.insert("font-size".to_string(), format!("{}pt", format_g3(v)));
        }
        if let Some(v) = self.position {
            c.insert("vertical-align".to_string(), format!("{}pt", format_g3(v)));
        }
        // A transparent highlight is Word's "no highlight" and would
        // otherwise clobber the shading colour set just above.
        if let Some(h) = self.highlight.as_deref().filter(|h| *h != "transparent") {
            c.insert("background-color".to_string(), h.to_string());
        }
        if self.b == Some(true) {
            c.insert("font-weight".to_string(), "bold".to_string());
        }
        if let Some(v) = &self.font_family {
            c.insert("font-family".to_string(), v.clone());
        }
        c
    }
}

/// Port of the Python `read_font`: the ASCII font and size.
fn read_font(parent: Node, ns: &DocxNamespace, dest: &mut RunStyle) {
    for col in ns.children(parent, &["w:rFonts"]) {
        // A theme reference is marked with pipes and resolved later
        // against the document theme.
        let val = match ns.get(col, "w:asciiTheme") {
            Some(theme) => Some(format!("|{theme}|")),
            None => ns.get(col, "w:ascii").map(str::to_string),
        };
        if let Some(v) = val.filter(|v| !v.is_empty()) {
            dest.font_family = Some(v);
        }
    }
    // `w:sz` is in half-points.
    for col in ns.children(parent, &["w:sz"]) {
        if let Some(v) = simple_float(ns.get(col, "w:val"), 0.5) {
            dest.font_size = Some(v);
            return;
        }
    }
}

/// Port of the Python `read_font_cs`: the complex-script font and size.
fn read_font_cs(parent: Node, ns: &DocxNamespace, dest: &mut RunStyle) {
    for col in ns.children(parent, &["w:rFonts"]) {
        let val = match ns.get(col, "w:csTheme") {
            Some(theme) => Some(format!("|{theme}|")),
            None => ns.get(col, "w:cs").map(str::to_string),
        };
        if let Some(v) = val.filter(|v| !v.is_empty()) {
            dest.cs_font_family = Some(v);
        }
    }
    for col in ns.children(parent, &["w:szCS"]) {
        if let Some(v) = simple_float(ns.get(col, "w:val"), 0.5) {
            // Calibre assigns `w:szCS` to `font_size`, not
            // `cs_font_size` — and since this runs after read_font, a
            // complex-script size wins over the ASCII one. Reproduced
            // as-is so output matches; see the module docs.
            dest.font_size = Some(v);
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    fn style_of(body: &str) -> RunStyle {
        let xml: &'static str = Box::leak(
            format!(
                r#"<w:rPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">{body}</w:rPr>"#
            )
            .into_boxed_str(),
        );
        let doc: &'static Document<'static> =
            Box::leak(Box::new(Document::parse(xml).expect("valid XML")));
        RunStyle::from_rpr(doc.root_element(), &DocxNamespace::default())
    }

    #[test]
    fn bold_and_italic_reach_the_css() {
        let s = style_of("<w:b/><w:i/>");
        let css = s.css();
        assert_eq!(css.get("font-weight").map(String::as_str), Some("bold"));
        assert_eq!(css.get("font-style").map(String::as_str), Some("italic"));

        // An explicit off is not inherit, and must not emit anything.
        let s = style_of(r#"<w:b w:val="0"/>"#);
        assert_eq!(s.b, Some(false));
        assert!(!s.css().contains_key("font-weight"));
    }

    #[test]
    fn font_size_is_in_half_points() {
        let s = style_of(r#"<w:sz w:val="24"/>"#);
        assert_eq!(s.font_size, Some(12.0));
        assert_eq!(s.css().get("font-size").map(String::as_str), Some("12pt"));
    }

    #[test]
    fn a_theme_font_is_marked_for_later_resolution() {
        let s = style_of(r#"<w:rFonts w:asciiTheme="minorHAnsi"/>"#);
        assert_eq!(s.font_family.as_deref(), Some("|minorHAnsi|"));

        let s = style_of(r#"<w:rFonts w:ascii="Georgia"/>"#);
        assert_eq!(s.font_family.as_deref(), Some("Georgia"));
    }

    #[test]
    fn complex_script_size_overrides_the_ascii_size() {
        // Calibre's read_font_cs writes to font_size; this pins that
        // behaviour so a future fix is a deliberate, visible change.
        let s = style_of(r#"<w:sz w:val="24"/><w:szCS w:val="40"/>"#);
        assert_eq!(s.font_size, Some(20.0));
        assert_eq!(s.cs_font_size, None);
    }

    #[test]
    fn underline_styles_map_onto_text_decoration() {
        assert_eq!(
            style_of(r#"<w:u w:val="single"/>"#)
                .text_decoration
                .as_deref(),
            Some("underline solid")
        );
        assert_eq!(
            style_of(r#"<w:u w:val="wave"/>"#)
                .text_decoration
                .as_deref(),
            Some("underline wavy")
        );
        assert_eq!(
            style_of(r#"<w:u w:val="none"/>"#)
                .text_decoration
                .as_deref(),
            Some("none")
        );
        assert_eq!(
            style_of(r#"<w:u w:val="single" w:color="FF0000"/>"#)
                .text_decoration
                .as_deref(),
            Some("underline solid #FF0000")
        );
        // `auto` is not a colour worth emitting.
        assert_eq!(
            style_of(r#"<w:u w:val="single" w:color="auto"/>"#)
                .text_decoration
                .as_deref(),
            Some("underline solid")
        );
    }

    #[test]
    fn strikethrough_joins_the_underline_decoration() {
        let s = style_of(r#"<w:u w:val="single"/><w:strike/>"#);
        assert_eq!(
            s.css().get("text-decoration").map(String::as_str),
            Some("underline solid line-through")
        );
        let s = style_of("<w:dstrike/>");
        assert_eq!(
            s.css().get("text-decoration").map(String::as_str),
            Some("line-through")
        );
    }

    #[test]
    fn highlight_colours_are_translated_and_beat_shading() {
        let s = style_of(r#"<w:shd w:fill="CCCCCC"/><w:highlight w:val="darkRed"/>"#);
        assert_eq!(s.background_color.as_deref(), Some("#CCCCCC"));
        assert_eq!(s.highlight.as_deref(), Some("#800000"));
        assert_eq!(
            s.css().get("background-color").map(String::as_str),
            Some("#800000")
        );

        // `none` becomes transparent, which is dropped from the CSS so
        // the shading colour survives.
        let s = style_of(r#"<w:shd w:fill="CCCCCC"/><w:highlight w:val="none"/>"#);
        assert_eq!(
            s.css().get("background-color").map(String::as_str),
            Some("#CCCCCC")
        );
    }

    #[test]
    fn a_known_highlight_keyword_passes_through() {
        let s = style_of(r#"<w:highlight w:val="yellow"/>"#);
        assert_eq!(s.highlight.as_deref(), Some("yellow"));
    }

    #[test]
    fn language_is_resolved_from_the_hex_lcid() {
        assert_eq!(
            style_of(r#"<w:lang w:val="0409"/>"#).lang.as_deref(),
            Some("en")
        );
        assert_eq!(
            style_of(r#"<w:lang w:val="040C"/>"#).lang.as_deref(),
            Some("fr")
        );
        // A non-hex value is a language tag already.
        assert_eq!(
            style_of(r#"<w:lang w:val="en-GB"/>"#).lang.as_deref(),
            Some("en-GB")
        );
        // A hex code that is not a known LCID leaves the property unset.
        assert_eq!(style_of(r#"<w:lang w:val="0FFF"/>"#).lang, None);
    }

    #[test]
    fn letter_spacing_and_position_use_their_own_scales() {
        // w:spacing on a run is twentieths of a point...
        let s = style_of(r#"<w:spacing w:val="20"/>"#);
        assert_eq!(s.letter_spacing, Some(1.0));
        assert_eq!(
            s.css().get("letter-spacing").map(String::as_str),
            Some("1pt")
        );
        // ...while w:position is half-points.
        let s = style_of(r#"<w:position w:val="6"/>"#);
        assert_eq!(s.position, Some(3.0));
        assert_eq!(
            s.css().get("vertical-align").map(String::as_str),
            Some("3pt")
        );
    }

    #[test]
    fn a_bare_bdr_establishes_a_default_border() {
        // Any attribute at all turns the border on, even one that says
        // nothing about how it looks.
        let s = style_of(r#"<w:bdr w:space="0"/>"#);
        assert_eq!(s.border.color.as_deref(), Some("currentColor"));
        assert_eq!(s.border.style.as_deref(), Some("none"));
        assert_eq!(s.border.width, Some(1.0));

        // A w:bdr with no attributes at all establishes nothing.
        let s = style_of("<w:bdr/>");
        assert_eq!(s.border, TextBorder::default());
    }

    #[test]
    fn run_border_width_is_floored_at_one_point() {
        let s = style_of(r#"<w:bdr w:val="single" w:sz="2"/>"#);
        assert_eq!(s.border.width, Some(1.0), "8 eighths is the floor");
        assert_eq!(s.css().get("border-width").map(String::as_str), Some("1pt"));
    }

    #[test]
    fn hidden_runs_are_display_none() {
        assert_eq!(
            style_of("<w:vanish/>")
                .css()
                .get("display")
                .map(String::as_str),
            Some("none")
        );
        assert_eq!(
            style_of("<w:webHidden/>")
                .css()
                .get("display")
                .map(String::as_str),
            Some("none")
        );
    }

    #[test]
    fn update_and_resolve_treat_none_as_inherit() {
        let mut base = style_of("<w:b/>");
        base.update(&style_of("<w:i/>"));
        assert_eq!(
            base.b,
            Some(true),
            "not overwritten by an unspecified value"
        );
        assert_eq!(base.i, Some(true));

        let mut child = style_of(r#"<w:sz w:val="24"/>"#);
        child.resolve_based_on(&style_of(r#"<w:sz w:val="40"/><w:b/>"#));
        assert_eq!(child.font_size, Some(12.0), "child's own size wins");
        assert_eq!(child.b, Some(true), "inherited");
    }

    #[test]
    fn toggle_properties_are_the_ones_the_spec_xors() {
        assert!(RunStyle::TOGGLE_PROPERTIES.contains(&"b"));
        assert!(RunStyle::TOGGLE_PROPERTIES.contains(&"smallCaps"));
        // `rtl`, `cs`, `dstrike` and `webHidden` are plain on/off.
        assert!(!RunStyle::TOGGLE_PROPERTIES.contains(&"rtl"));
        assert!(!RunStyle::TOGGLE_PROPERTIES.contains(&"webHidden"));
    }

    #[test]
    fn an_empty_rpr_inherits_everything() {
        let s = style_of("");
        assert_eq!(s, RunStyle::new());
        assert!(s.css().is_empty());
    }
}
