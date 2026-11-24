//! Port of `old_src/src/calibre/ebooks/readability/debug.py`.
//!
//! Nothing in `old_src` imports this module -- not even
//! `readability.py`, which carries its own near-duplicate `describe`
//! function inline (ported separately as
//! [`super::readability::describe`], which differs only in its default
//! `depth` and in not doing the per-node numbering this one does). It's
//! ported here anyway since it's one of the four files this issue lists,
//! and it's small.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::dom::{Dom, NodeId};

/// Port of `save_to_file`: writes `text` to `filename` as UTF-8-encoded
/// HTML, prefixed with a `Content-Type` meta tag (the Python opens the
/// file in binary mode and writes the meta tag's bytes followed by
/// `text.encode('utf-8')`).
pub fn save_to_file(text: &str, filename: &Path) -> io::Result<()> {
    let mut f = fs::File::create(filename)?;
    f.write_all(b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />")?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

/// Port of `describe`. The Python keys its `uids` numbering dict by
/// node object identity (`node not in uids`); [`NodeId`] is already a
/// stable per-node identity within a [`Dom`], so it's used as the key
/// directly instead of Python's `id(node)`-via-`__eq__`/`__hash__`
/// dance.
pub fn describe(
    dom: &Dom,
    node: NodeId,
    depth: usize,
    uids: &mut HashMap<NodeId, usize>,
) -> String {
    let Some(tag) = dom.tag(node) else {
        return "[non-element node]".to_string();
    };
    let mut name = tag.to_string();
    if let Some(id_attr) = dom.node(node).attrs.get("id") {
        if !id_attr.is_empty() {
            name.push('#');
            name.push_str(id_attr);
        }
    }
    if let Some(class_attr) = dom.node(node).attrs.get("class") {
        if !class_attr.is_empty() {
            name.push('.');
            name.push_str(&class_attr.replace(' ', "."));
        }
    }
    if name.starts_with("div#") || name.starts_with("div.") {
        name = name[3..].to_string();
    }
    if matches!(tag, "tr" | "td" | "div" | "p") {
        let next_uid = uids.len() + 1;
        let uid = *uids.entry(node).or_insert(next_uid);
        name.push_str(&format!("{uid:02}"));
    }
    if depth > 0 {
        if let Some(parent) = dom.parent(node) {
            if dom.tag(parent).is_some() {
                return format!("{name} - {}", describe(dom, parent, depth - 1, uids));
            }
        }
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn describe_includes_id_and_class() {
        let dom = Dom::parse("<html><body><div id=\"x\" class=\"a b\">hi</div></body></html>");
        let div = dom.find_first_tag_global("div").unwrap();
        let mut uids = HashMap::new();
        let d = describe(&dom, div, 2, &mut uids);
        // div#x.a.b prefix is shortened to "#x.a.b" (the div# stripping
        // rule), followed by a 2-digit uid, then " - " + parent chain.
        assert!(d.starts_with("#x.a.b"), "{d}");
    }

    #[test]
    fn describe_numbers_repeated_tags_stably() {
        let dom = Dom::parse("<html><body><div>a</div><div>b</div></body></html>");
        let divs = dom.find_all_tag_global("div");
        let mut uids = HashMap::new();
        let d0 = describe(&dom, divs[0], 0, &mut uids);
        let d1 = describe(&dom, divs[1], 0, &mut uids);
        assert_ne!(d0, d1);
        // Re-describing the first node again reuses its original uid.
        let d0_again = describe(&dom, divs[0], 0, &mut uids);
        assert_eq!(d0, d0_again);
    }

    #[test]
    fn save_to_file_writes_meta_and_text() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("out.html");
        save_to_file("<p>hi</p>", &path).expect("save");
        let contents = fs::read_to_string(&path).expect("read back");
        assert!(contents.starts_with("<meta http-equiv=\"Content-Type\""));
        assert!(contents.ends_with("<p>hi</p>"));
    }
}
