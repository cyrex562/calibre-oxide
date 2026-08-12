//! The document theme's major/minor latin fonts.
//!
//! Port of `old_src/src/calibre/ebooks/docx/theme.py`.
//!
//! Runs can name a font indirectly — `<w:rFonts w:asciiTheme="minorHAnsi"/>`
//! means "whatever the theme calls the minor font". [`super::char_styles`]
//! records those as `|minorHAnsi|`, and [`Theme::resolve_font_family`]
//! turns them into real family names.

use roxmltree::Node;

use super::names::DocxNamespace;

/// The theme's two latin font choices.
///
/// Port of the Python `Theme` class. The defaults are Word's own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    pub major_latin_font: String,
    pub minor_latin_font: String,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            major_latin_font: "Cambria".to_string(),
            minor_latin_font: "Calibri".to_string(),
        }
    }
}

impl Theme {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a `theme1.xml` root element.
    ///
    /// Port of the Python `Theme.__call__`.
    pub fn read(&mut self, root: Node, ns: &DocxNamespace) {
        for fs in ns.descendants(root, &["a:fontScheme"]) {
            for major in ns.children(fs, &["a:majorFont"]) {
                for latin in ns.children(major, &["a:latin"]) {
                    if let Some(t) = latin.attribute("typeface") {
                        self.major_latin_font = t.to_string();
                    }
                }
            }
            for minor in ns.children(fs, &["a:minorFont"]) {
                for latin in ns.children(minor, &["a:latin"]) {
                    if let Some(t) = latin.attribute("typeface") {
                        self.minor_latin_font = t.to_string();
                    }
                }
            }
        }
    }

    /// Resolve a possibly theme-referencing font family.
    ///
    /// Port of the Python `resolve_font_family`. A name wrapped in
    /// pipes is a theme reference; anything else passes through.
    pub fn resolve_font_family(&self, ff: &str) -> String {
        let Some(inner) = ff.strip_prefix('|') else {
            return ff.to_string();
        };
        // The Python slices off the last character unconditionally,
        // trailing pipe or not.
        let mut chars = inner.chars();
        chars.next_back();
        let inner = chars.as_str();
        if inner.starts_with("major") {
            self.major_latin_font.clone()
        } else {
            self.minor_latin_font.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const THEME: &str = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <a:themeElements>
    <a:fontScheme name="Office">
      <a:majorFont><a:latin typeface="Georgia"/></a:majorFont>
      <a:minorFont><a:latin typeface="Verdana"/></a:minorFont>
    </a:fontScheme>
  </a:themeElements>
</a:theme>"#;

    fn theme_from(xml: &str) -> Theme {
        let doc = Document::parse(xml).expect("valid XML");
        let mut theme = Theme::new();
        theme.read(doc.root_element(), &DocxNamespace::default());
        theme
    }

    #[test]
    fn defaults_are_words_own() {
        let t = Theme::new();
        assert_eq!(t.major_latin_font, "Cambria");
        assert_eq!(t.minor_latin_font, "Calibri");
    }

    #[test]
    fn reads_both_latin_faces() {
        let t = theme_from(THEME);
        assert_eq!(t.major_latin_font, "Georgia");
        assert_eq!(t.minor_latin_font, "Verdana");
    }

    #[test]
    fn a_theme_without_a_font_scheme_keeps_the_defaults() {
        let t = theme_from(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"/>"#,
        );
        assert_eq!(t, Theme::new());
    }

    #[test]
    fn theme_references_resolve_and_plain_names_pass_through() {
        let t = theme_from(THEME);
        assert_eq!(t.resolve_font_family("|majorHAnsi|"), "Georgia");
        assert_eq!(t.resolve_font_family("|minorHAnsi|"), "Verdana");
        // Anything not starting with `major` is the minor font.
        assert_eq!(t.resolve_font_family("|minorEastAsia|"), "Verdana");
        assert_eq!(t.resolve_font_family("Times New Roman"), "Times New Roman");
        // A family that merely contains a pipe is not a reference.
        assert_eq!(t.resolve_font_family("Foo|Bar"), "Foo|Bar");
    }
}
