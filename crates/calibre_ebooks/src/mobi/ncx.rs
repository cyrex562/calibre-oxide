use crate::mobi::headers::NULL_INDEX;
use crate::mobi::index::{read_index, CNCXReader};
use anyhow::Result;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct NCXEntry {
    pub pos: i64,
    pub len: u64,
    pub noffs: i64,
    pub text: String,
    pub hlvl: i64,
    pub kind: String,
    pub pos_fid: Option<(u64, u64)>, // tuple of 2
    pub parent: i64,
    pub child1: i64,
    pub childn: i64,
    pub description: Option<String>,
    pub author: Option<String>,
    pub image_caption: Option<String>,
    pub image_attribution: Option<String>,
    pub name: String,
    pub num: usize,
    // extra fields populated later
    pub href: Option<String>,
    pub idtag: Option<String>,
}

impl Default for NCXEntry {
    fn default() -> Self {
        NCXEntry {
            pos: -1,
            len: 0,
            noffs: -1,
            text: "Unknown Text".to_string(),
            hlvl: -1,
            kind: "Unknown Class".to_string(),
            pos_fid: None,
            parent: -1,
            child1: -1,
            childn: -1,
            description: None,
            author: None,
            image_caption: None,
            image_attribution: None,
            name: "".to_string(),
            num: 0,
            href: None,
            idtag: None,
        }
    }
}

pub fn parse_ncx_from_index(
    table: &BTreeMap<String, BTreeMap<u8, Vec<u64>>>,
    cncx: &CNCXReader,
) -> Vec<NCXEntry> {
    let mut index_entries = Vec::new();

    for (num, (text, tag_map)) in table.iter().enumerate() {
        let mut entry = NCXEntry::default();
        entry.name = text.clone();
        entry.num = num;

        if let Some(vals) = tag_map.get(&1) {
            entry.pos = vals[0] as i64;
        }
        if let Some(vals) = tag_map.get(&2) {
            entry.len = vals[0];
        }
        if let Some(vals) = tag_map.get(&3) {
            let off = vals[0] as usize;
            entry.noffs = off as i64;
            if let Some(s) = cncx.get(off) {
                entry.text = s.clone();
            }
        }
        if let Some(vals) = tag_map.get(&4) {
            entry.hlvl = vals[0] as i64;
        }
        if let Some(vals) = tag_map.get(&5) {
            let off = vals[0] as usize;
            if let Some(s) = cncx.get(off) {
                entry.kind = s.clone();
            }
        }
        if let Some(vals) = tag_map.get(&6) {
            if vals.len() >= 2 {
                entry.pos_fid = Some((vals[0], vals[1]));
            }
        }
        if let Some(vals) = tag_map.get(&21) {
            entry.parent = vals[0] as i64;
        }
        if let Some(vals) = tag_map.get(&22) {
            entry.child1 = vals[0] as i64;
        }
        if let Some(vals) = tag_map.get(&23) {
            entry.childn = vals[0] as i64;
        }

        if let Some(vals) = tag_map.get(&70) {
            let off = vals[0] as usize;
            if let Some(s) = cncx.get(off) {
                entry.description = Some(s.clone());
            }
        }
        if let Some(vals) = tag_map.get(&71) {
            let off = vals[0] as usize;
            if let Some(s) = cncx.get(off) {
                entry.author = Some(s.clone());
            }
        }
        if let Some(vals) = tag_map.get(&72) {
            let off = vals[0] as usize;
            if let Some(s) = cncx.get(off) {
                entry.image_caption = Some(s.clone());
            }
        }
        if let Some(vals) = tag_map.get(&73) {
            let off = vals[0] as usize;
            if let Some(s) = cncx.get(off) {
                entry.image_attribution = Some(s.clone());
            }
        }

        index_entries.push(entry);
    }
    index_entries
}

pub fn read_ncx(
    sections: &[(Vec<u8>, (u32, u32, u32, u32, u32))],
    index: u32,
    codec: &str,
) -> Result<Vec<NCXEntry>> {
    if index == NULL_INDEX {
        return Ok(Vec::new());
    }
    let (table, cncx) = read_index(sections, index as usize, codec)?;
    Ok(parse_ncx_from_index(&table, &cncx))
}

use crate::metadata::toc::{TOCNode, TOC};

struct NodeBuilder {
    title: String,
    src: String,
    children: Vec<Rc<RefCell<NodeBuilder>>>,
    // id: usize, // Unused
}

enum Parent {
    Root(Rc<RefCell<Vec<Rc<RefCell<NodeBuilder>>>>>),
    Node(Rc<RefCell<NodeBuilder>>),
}

impl Parent {
    fn add_child(&self, child: Rc<RefCell<NodeBuilder>>) {
        match self {
            Parent::Root(v) => v.borrow_mut().push(child),
            Parent::Node(n) => n.borrow_mut().children.push(child),
        }
    }
}

pub fn build_toc(index_entries: Vec<NCXEntry>) -> TOC {
    let mut toc = TOC::new();

    let root_children: Rc<RefCell<Vec<Rc<RefCell<NodeBuilder>>>>> =
        Rc::new(RefCell::new(Vec::new()));

    let mut num_map: HashMap<i64, Parent> = HashMap::new();
    num_map.insert(-1, Parent::Root(root_children.clone()));

    let mut levels: Vec<i64> = index_entries.iter().map(|e| e.hlvl).collect();
    levels.sort();
    levels.dedup();

    for lvl in levels {
        let items: Vec<&NCXEntry> = index_entries.iter().filter(|e| e.hlvl == lvl).collect();
        for item in items {
            let parent_id = item.parent;
            if let Some(parent) = num_map.get(&parent_id) {
                let node = Rc::new(RefCell::new(NodeBuilder {
                    title: item.text.clone(),
                    src: item.href.clone().unwrap_or_default(),
                    children: Vec::new(),
                    // id: item.num,
                }));

                parent.add_child(node.clone());
                num_map.insert(item.num as i64, Parent::Node(node));
            } else {
                eprintln!("Orphan NCX node: {:?}", item);
            }
        }
    }

    fn convert(builders: Vec<Rc<RefCell<NodeBuilder>>>) -> Vec<TOCNode> {
        let mut nodes = Vec::new();
        for b in builders {
            let b = b.borrow();
            nodes.push(TOCNode {
                title: b.title.clone(),
                src: b.src.clone(),
                children: convert(b.children.clone()),
            });
        }
        nodes
    }

    toc.nodes = convert(root_children.borrow().clone());
    toc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_parse_ncx_from_index() {
        let mut table = BTreeMap::new();
        let mut tag_map = BTreeMap::new();
        // text maps to tag 3 (offset index in cncx)
        tag_map.insert(3, vec![0]);
        // pos maps to tag 1
        tag_map.insert(1, vec![100]);
        // hlvl maps to tag 4
        tag_map.insert(4, vec![0]);

        table.insert("entry1".to_string(), tag_map);

        let mut cncx_records = BTreeMap::new();
        cncx_records.insert(0, "Chapter 1".to_string());
        let cncx = CNCXReader {
            records: cncx_records,
        };

        let entries = parse_ncx_from_index(&table, &cncx);

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].text, "Chapter 1");
        assert_eq!(entries[0].pos, 100);
        assert_eq!(entries[0].hlvl, 0);
    }
}
