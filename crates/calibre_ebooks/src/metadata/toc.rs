use anyhow::{Context, Result};
use roxmltree::Document;

#[derive(Debug, Clone)]
pub struct TOCNode {
    pub title: String,
    pub src: String,
    pub children: Vec<TOCNode>,
}

#[derive(Debug, Clone)]
pub struct TOC {
    pub nodes: Vec<TOCNode>,
}

impl TOC {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    pub fn parse_ncx(raw: &str) -> Result<Self> {
        let doc = Document::parse(raw).context("Failed to parse NCX XML")?;
        let mut toc = TOC::new();

        let nav_map = doc
            .descendants()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("navMap"))
            .context("NCX missing navMap")?;

        for nav_point in nav_map.children() {
            if nav_point.tag_name().name().eq_ignore_ascii_case("navPoint") {
                if let Some(node) = parse_nav_point(nav_point) {
                    toc.nodes.push(node);
                }
            }
        }

        Ok(toc)
    }
}

fn parse_nav_point(node: roxmltree::Node) -> Option<TOCNode> {
    let mut title = String::new();
    let mut src = String::new();

    // Find navLabel/text
    if let Some(label) = node
        .children()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("navLabel"))
    {
        if let Some(text) = label
            .children()
            .find(|n| n.tag_name().name().eq_ignore_ascii_case("text"))
        {
            title = text.text().unwrap_or("").trim().to_string();
        }
    }

    // Find content
    if let Some(content) = node
        .children()
        .find(|n| n.tag_name().name().eq_ignore_ascii_case("content"))
    {
        src = content.attribute("src").unwrap_or("").to_string();
    }

    if title.is_empty() && src.is_empty() {
        return None;
    }

    let mut children = Vec::new();
    for child in node.children() {
        if child.tag_name().name().eq_ignore_ascii_case("navPoint") {
            if let Some(c) = parse_nav_point(child) {
                children.push(c);
            }
        }
    }

    Some(TOCNode {
        title,
        src,
        children,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ncx_parsing() -> Result<()> {
        let ncx = r#"
        <ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
            <navMap>
                <navPoint id="1" playOrder="1">
                    <navLabel><text>Chapter 1</text></navLabel>
                    <content src="c1.html"/>
                    <navPoint id="2" playOrder="2">
                        <navLabel><text>Section 1.1</text></navLabel>
                        <content src="c1.html#s1"/>
                    </navPoint>
                </navPoint>
                <navPoint id="3" playOrder="3">
                    <navLabel><text>Chapter 2</text></navLabel>
                    <content src="c2.html"/>
                </navPoint>
            </navMap>
        </ncx>
        "#;

        let toc = TOC::parse_ncx(ncx)?;
        assert_eq!(toc.nodes.len(), 2);
        assert_eq!(toc.nodes[0].title, "Chapter 1");
        assert_eq!(toc.nodes[0].children.len(), 1);
        assert_eq!(toc.nodes[0].children[0].title, "Section 1.1");
        assert_eq!(toc.nodes[1].title, "Chapter 2");

        Ok(())
    }
}
