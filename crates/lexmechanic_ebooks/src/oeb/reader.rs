use crate::oeb::book::OEBBook;
use crate::oeb::constants::*;
use anyhow::{bail, Result};
use roxmltree::Document;

pub struct OEBReader;

impl OEBReader {
    pub fn new() -> Self {
        OEBReader
    }

    pub fn read_opf(&self, book: &mut OEBBook, opf_path: &str) -> Result<()> {
        let data = book.container.read(opf_path)?;
        let text = String::from_utf8_lossy(&data); // Basic UTF-8 handling for now

        // roxmltree parses namespaces automatically.
        let doc = Document::parse(&text).map_err(|e| anyhow::anyhow!("XML Parse Error: {}", e))?;
        let root = doc.root_element();

        // Check root element local name
        if root.tag_name().name() != "package" {
            bail!("Root element is not package");
        }

        // The OPF spec defines every manifest/guide href as relative to
        // the OPF file's own location, not the container root -- e.g.
        // an OPF at "OEBPS/content.opf" with an item `href="page.html"`
        // refers to "OEBPS/page.html".
        let opf_dir = match opf_path.rsplit_once('/') {
            Some((dir, _)) => dir,
            None => "",
        };

        self.metadata_from_opf(book, &root)?;
        self.manifest_from_opf(book, &root, opf_dir)?;
        self.spine_from_opf(book, &root)?;
        self.guide_from_opf(book, &root, opf_dir)?;

        Ok(())
    }

    fn metadata_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        let metadata_nodes: Vec<_> = root
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "metadata")
            .collect();

        if let Some(metadata_node) = metadata_nodes.first() {
            for child in metadata_node.children().filter(|n| n.is_element()) {
                let tag_name = child.tag_name().name();
                let ns = child.tag_name().namespace().unwrap_or("");
                let text = child.text().unwrap_or("").trim();

                // Dublin Core check: Namespace is DC11_NS or tag starts with dc: (if ns parsing failed or different)
                // roxmltree handles standard namespaces well.
                if ns == DC11_NS {
                    book.metadata.add(tag_name, text);
                } else if tag_name == "meta" {
                    // Handle <meta name="..." content="...">
                    if let Some(name) = child.attribute("name") {
                        if let Some(content) = child.attribute("content") {
                            book.metadata.add(name, content);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn manifest_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node, opf_dir: &str) -> Result<()> {
        if let Some(manifest_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "manifest")
        {
            for child in manifest_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "item")
            {
                let id = child.attribute("id");
                let href = child.attribute("href");
                let media_type = child.attribute("media-type");

                if let (Some(id), Some(href), Some(media_type)) = (id, href, media_type) {
                    let href = resolve_opf_relative_href(opf_dir, href);
                    book.manifest.add(id, &href, media_type);
                }
            }
        }
        Ok(())
    }

    fn spine_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node) -> Result<()> {
        if let Some(spine_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "spine")
        {
            for child in spine_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "itemref")
            {
                if let Some(idref) = child.attribute("idref") {
                    let linear = child.attribute("linear").unwrap_or("yes") != "no";
                    book.spine.add(idref, linear);
                }
            }
        }
        Ok(())
    }

    fn guide_from_opf(&self, book: &mut OEBBook, root: &roxmltree::Node, opf_dir: &str) -> Result<()> {
        if let Some(guide_node) = root
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == "guide")
        {
            for child in guide_node
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "reference")
            {
                let type_ = child.attribute("type");
                let href = child.attribute("href");
                let title = child.attribute("title").map(|s| s.to_string());

                if let (Some(type_), Some(href)) = (type_, href) {
                    let href = resolve_opf_relative_href(opf_dir, href);
                    book.guide.add(type_, title, &href);
                }
            }
        }
        Ok(())
    }
}

/// Resolves an OPF manifest/guide `href` (relative to the OPF file's
/// own location, per spec) into a path relative to the container
/// root -- e.g. `resolve_opf_relative_href("OEBPS", "page.html")` ->
/// `"OEBPS/page.html"`. Any `#fragment` is preserved untouched. Purely
/// lexical (`.`/`..` normalization), matching how zip container paths
/// are looked up -- no filesystem access.
fn resolve_opf_relative_href(opf_dir: &str, href: &str) -> String {
    // Skip absolute/external references (e.g. "http://...", "/abs/path").
    if href.contains("://") || href.starts_with('/') {
        return href.to_string();
    }

    let (path, fragment) = match href.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (href, None),
    };

    let joined = if opf_dir.is_empty() { path.to_string() } else { format!("{opf_dir}/{path}") };

    let mut parts: Vec<&str> = Vec::new();
    for comp in joined.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    let resolved = parts.join("/");

    match fragment {
        Some(f) => format!("{resolved}#{f}"),
        None => resolved,
    }
}

#[cfg(test)]
mod resolve_opf_relative_href_tests {
    use super::resolve_opf_relative_href;

    #[test]
    fn joins_a_relative_href_under_the_opf_directory() {
        assert_eq!(resolve_opf_relative_href("OEBPS", "page.html"), "OEBPS/page.html");
    }

    #[test]
    fn leaves_the_href_alone_when_the_opf_is_at_the_container_root() {
        assert_eq!(resolve_opf_relative_href("", "page.html"), "page.html");
    }

    #[test]
    fn normalizes_dot_dot_segments() {
        assert_eq!(resolve_opf_relative_href("OEBPS/text", "../images/cover.png"), "OEBPS/images/cover.png");
    }

    #[test]
    fn preserves_a_fragment() {
        assert_eq!(resolve_opf_relative_href("OEBPS", "toc.html#start"), "OEBPS/toc.html#start");
    }

    #[test]
    fn leaves_absolute_and_external_references_untouched() {
        assert_eq!(resolve_opf_relative_href("OEBPS", "/abs/path.html"), "/abs/path.html");
        assert_eq!(resolve_opf_relative_href("OEBPS", "http://example.com/x.html"), "http://example.com/x.html");
    }
}
