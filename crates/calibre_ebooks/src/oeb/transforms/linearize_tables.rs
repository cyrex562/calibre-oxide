//! Port of `old_src/src/calibre/ebooks/oeb/transforms/linearize_tables.py`.

use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_DOCS;

const TABLE_TAGS: &[&str] = &[
    "table", "td", "tr", "th", "caption", "tbody", "tfoot", "thead", "colgroup", "col",
];

const STRIPPED_ATTRS: &[&str] = &[
    "style",
    "font",
    "valign",
    "colspan",
    "width",
    "height",
    "rowspan",
    "summary",
    "align",
    "cellspacing",
    "cellpadding",
    "frames",
    "rules",
    "border",
];

/// Port of `LinearizeTables`: turn every table-related element into a
/// plain `<div>`, dropping table-only presentational attributes.
pub struct LinearizeTables;

impl LinearizeTables {
    fn linearize(&self, dom: &mut crate::mobi::dom::Dom) {
        for tag in TABLE_TAGS {
            for el in dom.find_all_tag_global(tag) {
                dom.set_tag(el, "div");
                for attr in STRIPPED_ATTRS {
                    dom.node_mut(el).attrs.shift_remove(*attr);
                }
            }
        }
    }

    pub fn call(&self, oeb: &mut OEBBook) {
        let items: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in items {
            if !OEB_DOCS.contains(&media_type.as_str()) {
                continue;
            }
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = crate::mobi::dom::Dom::parse(&html);
            self.linearize(&mut dom);
            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(&href, &rendered);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn tables_become_divs_and_lose_presentational_attrs() {
        let mut oeb = Builder::new()
            .page(
                "a.html",
                r#"<table border="1" width="100%"><tr><td valign="top">x</td></tr></table>"#,
            )
            .build();
        LinearizeTables.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("<table"), "{html}");
        assert!(!html.contains("<td"), "{html}");
        assert!(!html.contains("border="), "{html}");
        assert!(!html.contains("valign="), "{html}");
        assert!(html.contains(">x<"), "{html}");
    }
}
