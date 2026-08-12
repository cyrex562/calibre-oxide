#[derive(Debug, Clone, Default)]
pub struct TOCNode {
    pub title: Option<String>,
    pub href: Option<String>,
    pub id: Option<String>,
    pub klass: Option<String>,
    pub play_order: i32,
    /// Article summary. Set by news recipes and consumed by the
    /// periodical writers — see `crate::epub::periodical`.
    pub description: Option<String>,
    /// Article byline, likewise recipe-supplied.
    pub author: Option<String>,
    pub children: Vec<TOCNode>,
}

impl TOCNode {
    pub fn new(title: Option<String>, href: Option<String>) -> Self {
        TOCNode {
            title,
            href,
            id: None,
            klass: None,
            play_order: 0,
            description: None,
            author: None,
            children: Vec::new(),
        }
    }

    pub fn add(&mut self, node: TOCNode) {
        self.children.push(node);
    }
}

#[derive(Debug, Clone, Default)]
pub struct TOC {
    pub root: TOCNode,
}

impl TOC {
    pub fn new() -> Self {
        TOC {
            root: TOCNode::new(None, None),
        }
    }
}
