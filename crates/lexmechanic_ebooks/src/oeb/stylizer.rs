use crate::oeb::normalize_css::DEFAULTS;
use roxmltree::Node;
use std::collections::HashMap;

pub struct Stylizer {
    pub dpi: f32,
    pub font_base: f32, // in pts
}

impl Stylizer {
    pub fn new(dpi: f32, font_base: f32) -> Self {
        Self { dpi, font_base }
    }

    pub fn style<'a, 'input>(&self, node: &Node<'a, 'input>) -> Style<'a, 'input, '_> {
        Style {
            node: *node,
            stylizer: self,
        }
    }
}

pub struct Style<'a, 'input, 'b> {
    node: Node<'a, 'input>,
    stylizer: &'b Stylizer,
}

impl<'a, 'input, 'b> Style<'a, 'input, 'b> {
    pub fn get(&self, property: &str) -> String {
        // 1. Check inline style
        if let Some(val) = self.get_inline_style(property) {
            return val;
        }

        // 2. Check inheritance
        // List of inherited properties (simplified)
        let inherited = [
            "color",
            "font-family",
            "font-size",
            "font-style",
            "font-weight",
            "text-align",
            "line-height",
        ];

        if inherited.contains(&property) {
            if let Some(parent) = self.node.parent() {
                if parent.is_element() {
                    return self.stylizer.style(&parent).get(property);
                }
            }
        }

        // 3. Return default
        DEFAULTS.get(property).cloned().unwrap_or("").to_string()
    }

    /// The value actually declared for a property, by this element or
    /// by an ancestor it inherits from — with no fallback to the CSS
    /// initial value.
    ///
    /// [`Style::get`] falls back to `DEFAULTS`, which is right for a
    /// caller that wants a value no matter what, but wrong for one
    /// merging these over the HTML default stylesheet: `display`'s
    /// initial value is `inline`, and returning it would override the
    /// `block` that a `<p>` gets from being a `<p>`.
    pub fn specified(&self, property: &str) -> Option<String> {
        if let Some(val) = self.get_inline_style(property) {
            return Some(val);
        }
        let inherited = [
            "color",
            "font-family",
            "font-size",
            "font-style",
            "font-weight",
            "text-align",
            "line-height",
        ];
        if inherited.contains(&property) {
            if let Some(parent) = self.node.parent() {
                if parent.is_element() {
                    return self.stylizer.style(&parent).specified(property);
                }
            }
        }
        None
    }

    fn get_inline_style(&self, property: &str) -> Option<String> {
        if let Some(style_attr) = self.node.attribute("style") {
            // Simple CSS parser: split by ; then :
            for decl in style_attr.split(';') {
                let parts: Vec<&str> = decl.splitn(2, ':').collect();
                if parts.len() == 2 {
                    let prop = parts[0].trim();
                    let val = parts[1].trim();
                    if prop == property {
                        return Some(val.to_string());
                    }
                    // Handle edge expansion? E.g. simple cases if needed
                }
            }
        }
        None
    }

    pub fn font_size(&self) -> f32 {
        let val = self.get("font-size");
        // Naive conversion
        if val.ends_with("pt") {
            val.trim_end_matches("pt")
                .parse()
                .unwrap_or(self.stylizer.font_base)
        } else if val.ends_with("px") {
            let px = val.trim_end_matches("px").parse().unwrap_or(0.0);
            (px * 72.0) / self.stylizer.dpi
        } else if val == "medium" || val == "initial" {
            self.stylizer.font_base
        } else if val.ends_with("em") {
            let em = val.trim_end_matches("em").parse().unwrap_or(1.0);
            // Get parent font size
            if let Some(parent) = self.node.parent() {
                if parent.is_element() {
                    return self.stylizer.style(&parent).font_size() * em;
                }
            }
            self.stylizer.font_base * em
        } else {
            // Default fallthrough
            self.stylizer.font_base
        }
    }

    pub fn color(&self) -> String {
        self.get("color")
    }
}

// -- the resolved-style seam ------------------------------------------
//
// Converters (FB2, HTMLZ, and eventually the DOCX writer) need a
// handful of resolved CSS properties per element and do not care where
// they came from. This trait is that boundary: `Stylizer` above
// implements it from inline styles plus inheritance, `TagStylizer`
// implements it from the HTML defaults alone, and a full cascade can
// implement it later without any converter changing.

/// The resolved style of one element, reduced to what the format
/// converters ask of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedStyle {
    pub display: String,
    pub visibility: String,
    pub font_weight: String,
    pub font_style: String,
    pub text_decoration: String,
    /// Top margin in points.
    pub margin_top: f64,
    /// Font size in points.
    pub font_size: f64,
    /// The whole computed style as CSS text, for converters that
    /// inline it. Empty when the provider cannot produce one.
    pub css_text: String,
}

impl Default for ResolvedStyle {
    fn default() -> Self {
        Self {
            display: "inline".to_string(),
            visibility: "visible".to_string(),
            font_weight: "normal".to_string(),
            font_style: "normal".to_string(),
            text_decoration: "none".to_string(),
            margin_top: 0.0,
            font_size: 12.0,
            css_text: String::new(),
        }
    }
}

/// Supplies the resolved style of an element.
///
/// Stands in for `calibre.ebooks.oeb.stylizer.Stylizer`, which resolves
/// the full cascade. What implements it decides how much of that is
/// real.
pub trait StyleProvider {
    fn style(&self, node: Node) -> ResolvedStyle;
}

/// Elements that are blocks in the HTML default stylesheet.
const BLOCK_TAGS: &[&str] = &[
    "html",
    "body",
    "div",
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "ul",
    "ol",
    "li",
    "blockquote",
    "table",
    "tr",
    "td",
    "th",
    "section",
    "article",
    "header",
    "footer",
    "nav",
    "aside",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "pre",
    "hr",
    "form",
    "fieldset",
];

/// Elements the default stylesheet hides.
const HIDDEN_TAGS: &[&str] = &["head", "script", "style", "title", "meta", "link", "base"];

/// The style an element gets from the HTML defaults alone.
fn tag_defaults(node: Node) -> ResolvedStyle {
    let tag = node.tag_name().name();
    let mut style = ResolvedStyle::default();
    if BLOCK_TAGS.contains(&tag) {
        style.display = "block".to_string();
    }
    if HIDDEN_TAGS.contains(&tag) {
        style.display = "none".to_string();
    }
    if matches!(
        tag,
        "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th"
    ) {
        style.font_weight = "bold".to_string();
    }
    if matches!(tag, "i" | "em" | "cite" | "dfn" | "var" | "address") {
        style.font_style = "italic".to_string();
    }
    if matches!(tag, "del" | "s" | "strike") {
        style.text_decoration = "line-through".to_string();
    }
    if matches!(tag, "u" | "ins") {
        style.text_decoration = "underline".to_string();
    }
    style
}

/// A provider that knows only the HTML defaults: which elements are
/// blocks, which are hidden, and which imply bold, italic or a
/// decoration.
///
/// Enough for documents that carry their formatting in their markup.
#[derive(Debug, Default, Clone, Copy)]
pub struct TagStylizer;

impl StyleProvider for TagStylizer {
    fn style(&self, node: Node) -> ResolvedStyle {
        tag_defaults(node)
    }
}

impl StyleProvider for Stylizer {
    /// The tag defaults, overridden by anything the element's own
    /// `style` attribute sets and by the inherited properties this
    /// stylizer tracks.
    fn style(&self, node: Node) -> ResolvedStyle {
        let mut resolved = tag_defaults(node);
        let view = Stylizer::style(self, &node);
        for (property, slot) in [
            ("display", &mut resolved.display),
            ("visibility", &mut resolved.visibility),
            ("font-weight", &mut resolved.font_weight),
            ("font-style", &mut resolved.font_style),
            ("text-decoration", &mut resolved.text_decoration),
        ] {
            // Only a declared value wins; the CSS initial value must
            // not override the HTML default stylesheet.
            if let Some(value) = view.specified(property).filter(|v| !v.is_empty()) {
                *slot = value;
            }
        }
        resolved.font_size = view.font_size() as f64;
        resolved.margin_top = view
            .specified("margin-top")
            .map(|m| parse_length(&m, resolved.font_size))
            .unwrap_or(0.0);
        // The element's own declarations, which is as much of the
        // computed style as this stylizer can honestly produce.
        resolved.css_text = node.attribute("style").unwrap_or("").trim().to_string();
        resolved
    }
}

/// Parse a CSS length into points, treating `em` as relative to
/// `font_size`. Anything unrecognised is zero.
fn parse_length(value: &str, font_size: f64) -> f64 {
    let value = value.trim();
    let (number, unit) = match value.find(|c: char| c.is_alphabetic() || c == '%') {
        Some(i) => (&value[..i], &value[i..]),
        None => (value, ""),
    };
    let Ok(n) = number.trim().parse::<f64>() else {
        return 0.0;
    };
    match unit {
        "pt" | "" => n,
        "px" => n * 72.0 / 96.0,
        "em" | "rem" => n * font_size,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "pc" => n * 12.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod seam_tests {
    use super::*;
    use roxmltree::Document;

    /// Apply `f` to the first element with the given tag.
    fn with<T>(xml: &str, tag: &str, f: impl FnOnce(Node) -> T) -> T {
        let doc = Document::parse(xml).expect("valid XML");
        let node = doc
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == tag)
            .expect("element");
        f(node)
    }

    const SAMPLE: &str = "<html><body><p/><b/><i/><s/><u/><head/><span/></body></html>";

    #[test]
    fn tag_defaults_classify_blocks_and_emphasis() {
        let display = |tag| with(SAMPLE, tag, |n| TagStylizer.style(n).display);
        assert_eq!(display("p"), "block");
        assert_eq!(display("span"), "inline");
        assert_eq!(display("head"), "none");

        assert_eq!(
            with(SAMPLE, "b", |n| TagStylizer.style(n).font_weight),
            "bold"
        );
        assert_eq!(
            with(SAMPLE, "i", |n| TagStylizer.style(n).font_style),
            "italic"
        );
        assert_eq!(
            with(SAMPLE, "s", |n| TagStylizer.style(n).text_decoration),
            "line-through"
        );
        assert_eq!(
            with(SAMPLE, "u", |n| TagStylizer.style(n).text_decoration),
            "underline"
        );
    }

    #[test]
    fn an_inline_style_attribute_overrides_the_tag_default() {
        let xml = r#"<html><body><p style="display: none; font-weight: bold">x</p></body></html>"#;
        let resolved = with(xml, "p", |n| {
            StyleProvider::style(&Stylizer::new(96.0, 12.0), n)
        });
        assert_eq!(resolved.display, "none");
        assert_eq!(resolved.font_weight, "bold");
        assert_eq!(resolved.css_text, "display: none; font-weight: bold");
    }

    #[test]
    fn an_element_without_a_style_attribute_keeps_its_tag_default() {
        let xml = "<html><body><p>x</p></body></html>";
        let resolved = with(xml, "p", |n| {
            StyleProvider::style(&Stylizer::new(96.0, 12.0), n)
        });
        assert_eq!(resolved.display, "block");
        assert!(resolved.css_text.is_empty());
    }

    #[test]
    fn lengths_convert_to_points() {
        assert_eq!(parse_length("12pt", 12.0), 12.0);
        assert_eq!(parse_length("96px", 12.0), 72.0);
        assert_eq!(parse_length("2em", 10.0), 20.0);
        assert_eq!(parse_length("1in", 12.0), 72.0);
        assert_eq!(parse_length("", 12.0), 0.0);
        assert_eq!(parse_length("auto", 12.0), 0.0);
    }
}
