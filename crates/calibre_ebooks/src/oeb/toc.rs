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

    /// The maximum depth of the navigation tree rooted at this node. A
    /// leaf is depth 1. Port of `TOC.depth` in `oeb/base.py`.
    pub fn depth(&self) -> usize {
        self.children
            .iter()
            .map(|c| c.depth())
            .max()
            .map(|m| m + 1)
            .unwrap_or(1)
    }

    /// This node followed by all of its descendants, depth-first
    /// pre-order. Port of `TOC.iter` in `oeb/base.py`.
    pub fn iter(&self) -> Vec<&TOCNode> {
        let mut out = vec![self];
        for c in &self.children {
            out.extend(c.iter());
        }
        out
    }

    /// All descendants of this node (not including itself). Port of
    /// `TOC.iterdescendants` in `oeb/base.py`. `breadth_first` matches
    /// the Python flag exactly, including its "yield my children, then
    /// recurse into each child's own `iterdescendants`" shape, which is
    /// not a textbook global BFS but is what `CNCX`/`create_periodical_index`
    /// rely on for consistent ordering.
    pub fn iter_descendants(&self, breadth_first: bool) -> Vec<&TOCNode> {
        let mut out = Vec::new();
        if breadth_first {
            for c in &self.children {
                out.push(c);
            }
            for c in &self.children {
                out.extend(c.iter_descendants(true));
            }
        } else {
            for c in &self.children {
                out.extend(c.iter());
            }
        }
        out
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

    /// Total number of nodes in the tree, not counting the (synthetic)
    /// root. Port of `TOC.count` in `oeb/base.py`.
    pub fn count(&self) -> usize {
        self.root.iter().len().saturating_sub(1)
    }

    /// All descendants of the root, i.e. every real node in the tree.
    /// Port of `oeb.toc.iterdescendants(...)` call sites (`oeb.toc` in
    /// Python *is* the root node).
    pub fn iter_descendants(&self, breadth_first: bool) -> Vec<&TOCNode> {
        self.root.iter_descendants(breadth_first)
    }

    /// The first top-level node, i.e. `next(iter(oeb.toc))` in Python.
    pub fn first(&self) -> Option<&TOCNode> {
        self.root.children.first()
    }

    /// The top-level node at `idx`, i.e. `oeb.toc[idx]` in Python.
    pub fn get(&self, idx: usize) -> Option<&TOCNode> {
        self.root.children.get(idx)
    }
}
