use anyhow::{Context, Result};
use roxmltree::Document;

#[derive(Debug, Clone, Default)]
pub struct TOCNode {
    pub title: String,
    pub src: String,
    pub children: Vec<TOCNode>,
    /// Port of `TOC.play_order`. `None` means "not explicitly set" --
    /// [`crate::opf_writer::write_ncx`] falls back to auto-numbering
    /// in traversal order for these, matching every pre-existing
    /// caller of this struct that never set it.
    pub play_order: Option<u32>,
    /// Port of `TOC.author`/`.description`/`.toc_thumbnail`: rendered
    /// as `<calibre:meta name="...">` children of the `<navPoint>`
    /// (real `calibre.ebooks.metadata.toc.TOC.render`'s own
    /// `CALIBRE_NS` extension elements). Used by periodical TOCs
    /// (issue #622); no other current caller sets these.
    pub author: Option<String>,
    pub description: Option<String>,
    pub toc_thumbnail: Option<String>,
}

impl TOCNode {
    /// Port of `TOC.add_item`, restricted to the explicit-`play_order`
    /// form -- every real call site that needs `add_item` on a
    /// specific node (as opposed to a "top of tree" root) already
    /// tracks its own play-order counter and passes a real value, the
    /// same way `BasicNewsRecipe.create_opf`'s own `feed_index`
    /// closure does.
    pub fn add_item(&mut self, href: impl Into<String>, title: impl Into<String>, play_order: u32, author: Option<String>, description: Option<String>, toc_thumbnail: Option<String>) -> &mut TOCNode {
        self.children.push(TOCNode {
            title: title.into(),
            src: href.into(),
            children: Vec::new(),
            play_order: Some(play_order),
            author,
            description,
            toc_thumbnail,
        });
        self.children.last_mut().expect("just pushed")
    }
}

#[derive(Debug, Clone, Default)]
pub struct TOC {
    pub nodes: Vec<TOCNode>,
}

impl TOC {
    pub fn new() -> Self {
        Self { nodes: Vec::new() }
    }

    /// Port of `TOC.add_item` at the tree root. See
    /// [`TOCNode::add_item`] for the per-node form.
    pub fn add_item(&mut self, href: impl Into<String>, title: impl Into<String>, play_order: u32, author: Option<String>, description: Option<String>, toc_thumbnail: Option<String>) -> &mut TOCNode {
        self.nodes.push(TOCNode {
            title: title.into(),
            src: href.into(),
            children: Vec::new(),
            play_order: Some(play_order),
            author,
            description,
            toc_thumbnail,
        });
        self.nodes.last_mut().expect("just pushed")
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
        ..Default::default()
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

    #[test]
    fn add_item_builds_a_nested_tree_with_explicit_play_order() {
        let mut toc = TOC::new();
        let article = toc.add_item("a.html", "Article", 1, Some("Jane".to_string()), Some("A summary".to_string()), Some("thumb.jpg".to_string()));
        article.add_item("a.html#s1", "Section One", 2, None, None, None);

        assert_eq!(toc.nodes.len(), 1);
        assert_eq!(toc.nodes[0].play_order, Some(1));
        assert_eq!(toc.nodes[0].author.as_deref(), Some("Jane"));
        assert_eq!(toc.nodes[0].description.as_deref(), Some("A summary"));
        assert_eq!(toc.nodes[0].toc_thumbnail.as_deref(), Some("thumb.jpg"));
        assert_eq!(toc.nodes[0].children.len(), 1);
        assert_eq!(toc.nodes[0].children[0].src, "a.html#s1");
        assert_eq!(toc.nodes[0].children[0].play_order, Some(2));
    }
}
