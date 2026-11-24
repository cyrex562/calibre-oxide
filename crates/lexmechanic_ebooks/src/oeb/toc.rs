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

    /// Port of `TOC.rationalize_play_orders`: ensure that all nodes
    /// with the same `play_order` have the same `href`, and nodes with
    /// different `play_order`s have different `href`s.
    ///
    /// Operates over a flat, path-indexed snapshot instead of Python's
    /// aliased-live-object mutation (which the borrow checker won't
    /// allow directly -- holding many simultaneous `&mut TOCNode`
    /// references into one tree isn't expressible safely). `href`
    /// never changes during this algorithm, only `play_order` does, so
    /// snapshotting both once and mutating only the `play_order` array
    /// in place, read-back-and-write-forward, reproduces the real
    /// algorithm's every-lookup-sees-prior-mutations semantics exactly
    /// -- including `next_play_order()`'s own real behavior of
    /// recomputing `max(all current play_orders) + 1` fresh on every
    /// call (not a monotonic counter), which this mirrors by taking
    /// `max` over the whole live `play_orders` array at each fallback.
    pub fn rationalize_play_orders(&mut self) {
        fn collect(node: &TOCNode, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, i32, Option<String>)>) {
            out.push((path.clone(), node.play_order, node.href.clone()));
            for (i, c) in node.children.iter().enumerate() {
                path.push(i);
                collect(c, path, out);
                path.pop();
            }
        }
        let mut entries: Vec<(Vec<usize>, i32, Option<String>)> = Vec::new();
        collect(&self.root, &mut Vec::new(), &mut entries);

        let mut play_orders: Vec<i32> = entries.iter().map(|(_, po, _)| *po).collect();
        let hrefs: Vec<Option<String>> = entries.iter().map(|(_, _, h)| h.clone()).collect();

        for i in 0..entries.len() {
            if let Some(y) = (0..i).find(|&j| play_orders[j] == play_orders[i]) {
                if hrefs[i] != hrefs[y] {
                    play_orders[i] = match (0..i).find(|&j| hrefs[j] == hrefs[i]) {
                        Some(h) => play_orders[h],
                        None => play_orders.iter().max().copied().unwrap_or(0) + 1,
                    };
                }
            }
            if let Some(y) = (0..i).find(|&j| hrefs[j] == hrefs[i]) {
                play_orders[i] = play_orders[y];
            }
        }

        fn set_at(node: &mut TOCNode, path: &[usize], value: i32) {
            match path.split_first() {
                None => node.play_order = value,
                Some((&i, rest)) => set_at(&mut node.children[i], rest, value),
            }
        }
        for (i, (path, _, _)) in entries.iter().enumerate() {
            set_at(&mut self.root, path, play_orders[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(href: &str, play_order: i32) -> TOCNode {
        let mut n = TOCNode::new(None, Some(href.to_string()));
        n.play_order = play_order;
        n
    }

    #[test]
    fn nodes_sharing_an_href_get_unified_onto_the_same_play_order() {
        let mut toc = TOC::new();
        toc.root.add(node("a.html", 1));
        toc.root.add(node("a.html", 2)); // same href, different play_order
        toc.rationalize_play_orders();
        let pos: Vec<i32> = toc.root.children.iter().map(|c| c.play_order).collect();
        assert_eq!(pos[0], pos[1], "same href must end up with the same play_order");
    }

    #[test]
    fn nodes_sharing_a_play_order_with_different_hrefs_get_reassigned() {
        let mut toc = TOC::new();
        toc.root.add(node("a.html", 1));
        toc.root.add(node("b.html", 1)); // same play_order, different href
        toc.rationalize_play_orders();
        let pos: Vec<i32> = toc.root.children.iter().map(|c| c.play_order).collect();
        assert_ne!(pos[0], pos[1], "different hrefs must not share a play_order");
    }

    #[test]
    fn already_rationalized_orders_are_left_unchanged() {
        let mut toc = TOC::new();
        toc.root.add(node("a.html", 1));
        toc.root.add(node("b.html", 2));
        toc.root.add(node("c.html", 3));
        toc.rationalize_play_orders();
        let pos: Vec<i32> = toc.root.children.iter().map(|c| c.play_order).collect();
        assert_eq!(pos, vec![1, 2, 3]);
    }
}
