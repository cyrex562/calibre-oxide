use roxmltree::Document;

pub fn safe_xml_from_str(s: &str) -> Result<Document<'_>, roxmltree::Error> {
    Document::parse(s)
}

pub fn safe_html_from_str(s: &str) -> Result<Document<'_>, roxmltree::Error> {
    // For now, treat HTML as XML (roxmltree is strict XML).
    // Real HTML parsing requires html5ever or similar.
    // This is a placeholder/best-effort.
    Document::parse(s)
}
