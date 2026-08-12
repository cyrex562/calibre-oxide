//! Document-level settings read from `settings.xml`.
//!
//! Port of `old_src/src/calibre/ebooks/docx/settings.py`.
//!
//! Only one setting is consumed by the conversion: the default tab
//! stop, which decides how wide a `<w:tab/>` renders when the
//! paragraph declares no stops of its own.

use roxmltree::Node;

use super::names::DocxNamespace;

/// Port of the Python `Settings` class.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// Default tab stop in points. Word's default is 720 twips.
    pub default_tab_stop: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            default_tab_stop: 720.0 / 20.0,
        }
    }
}

impl Settings {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read a `settings.xml` root element.
    ///
    /// Port of the Python `Settings.__call__`. A malformed value
    /// leaves the default in place rather than failing the
    /// conversion.
    pub fn read(&mut self, root: Node, ns: &DocxNamespace) {
        for dts in ns.descendants(root, &["w:defaultTabStop"]) {
            if let Some(v) = ns
                .get(dts, "w:val")
                .and_then(|v| v.trim().parse::<f64>().ok())
            {
                self.default_tab_stop = v / 20.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    fn settings_from(xml: &str) -> Settings {
        let doc = Document::parse(xml).expect("valid XML");
        let mut s = Settings::new();
        s.read(doc.root_element(), &DocxNamespace::default());
        s
    }

    const OPEN: &str =
        r#"<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#;

    #[test]
    fn the_default_is_half_an_inch() {
        assert_eq!(Settings::new().default_tab_stop, 36.0);
    }

    #[test]
    fn the_tab_stop_is_read_in_twips() {
        let s = settings_from(&format!(
            r#"{OPEN}<w:defaultTabStop w:val="1440"/></w:settings>"#
        ));
        assert_eq!(s.default_tab_stop, 72.0);
    }

    #[test]
    fn a_malformed_or_missing_value_keeps_the_default() {
        assert_eq!(
            settings_from(&format!("{OPEN}</w:settings>")),
            Settings::new()
        );
        let s = settings_from(&format!(
            r#"{OPEN}<w:defaultTabStop w:val="lots"/></w:settings>"#
        ));
        assert_eq!(s, Settings::new());
        let s = settings_from(&format!(r#"{OPEN}<w:defaultTabStop/></w:settings>"#));
        assert_eq!(s, Settings::new());
    }

    #[test]
    fn the_last_declaration_wins() {
        let s = settings_from(&format!(
            r#"{OPEN}<w:defaultTabStop w:val="1440"/><w:defaultTabStop w:val="360"/></w:settings>"#
        ));
        assert_eq!(s.default_tab_stop, 18.0);
    }
}
