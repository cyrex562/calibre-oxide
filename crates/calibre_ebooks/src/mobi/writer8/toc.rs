//! Inject (or repair) an inline HTML table of contents into the spine.
//!
//! Port of `calibre.ebooks.mobi.writer8.toc.TOCAdder`. Kindle devices
//! have historically relied on an in-line, human-readable ToC page (as
//! opposed to the native NCX index alone) for reliable "Go To ->
//! Table of Contents" navigation, so `KF8Writer` generates one from
//! `oeb.toc` when the book doesn't already ship a usable one.
//!
//! # Scope note: `store_embed_font_rules`
//!
//! Python's template also injects a `body { font-family: ... }` rule
//! plus every embedded-font `@font-face` rule pulled from
//! `oeb.store_embed_font_rules` (set by an earlier `embed_fonts` output
//! plugin stage, which this crate's conversion pipeline doesn't wire up
//! yet). That styling is cosmetic (font choice for the generated ToC
//! page's own text) and not load-bearing for navigation or the
//! skeleton/index round trip this issue's tests exercise, so it's simply
//! omitted here rather than stubbed -- there is no equivalent field on
//! [`OEBBook`] to read it from.

use anyhow::{Context, Result};

use crate::mobi::dom::{Dom, NodeId};
use crate::mobi::writer2::serializer::urlnormalize;
use crate::oeb::book::OEBBook;
use crate::oeb::constants::XHTML_MIME;
use crate::oeb::toc::TOCNode;

/// `DEFAULT_TITLE` in `toc.py`.
pub const DEFAULT_TITLE: &str = "Table of Contents";

/// Fields `TOCAdder` reads off Python's `opts`.
#[derive(Debug, Clone, Default)]
pub struct TocOpts {
    pub toc_title: Option<String>,
    pub mobi_toc_at_start: bool,
    pub no_inline_toc: bool,
    pub mobi_passthrough: bool,
    pub extra_css: Option<String>,
}

/// Port of `find_previous_calibre_inline_toc`.
fn find_previous_calibre_inline_toc(oeb: &OEBBook) -> Option<String> {
    let guide_toc = oeb.guide.get("toc")?;
    let href = urlnormalize(guide_toc.href.split('#').next().unwrap_or(&guide_toc.href));
    let item = oeb.manifest.get_by_href(&href)?;
    let raw = oeb.container.read(&item.href).ok()?;
    let html = String::from_utf8_lossy(&raw);
    let dom = Dom::parse(&html);
    let body = dom.find_first_tag_global("body")?;
    if dom.node(body).attrs.get("id").map(String::as_str) == Some("calibre_generated_inline_toc") {
        Some(item.id.clone())
    } else {
        None
    }
}

/// Resolve `target` (a book-internal href, possibly with a `#fragment`)
/// relative to `base_href`'s directory. Port of `Item.relhref`.
fn relhref(base_href: &str, target: &str) -> String {
    let (target_path, frag) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (target, None),
    };
    if target_path.is_empty() {
        return match frag {
            Some(f) => format!("#{f}"),
            None => String::new(),
        };
    }
    let base_dir: Vec<&str> = match base_href.rfind('/') {
        Some(i) => base_href[..i]
            .split('/')
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    };
    let target_segs: Vec<&str> = target_path.split('/').collect();
    let mut common = 0usize;
    while common < base_dir.len()
        && common + 1 < target_segs.len()
        && base_dir[common] == target_segs[common]
    {
        common += 1;
    }
    let ups = base_dir.len() - common;
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    parts.extend(target_segs[common..].iter().map(|s| (*s).to_string()));
    let rel = if parts.is_empty() {
        target_segs.last().copied().unwrap_or("").to_string()
    } else {
        parts.join("/")
    };
    match frag {
        Some(f) => format!("{rel}#{f}"),
        None => rel,
    }
}

fn render_template(title: &str, extra_css: &str) -> String {
    format!(
        "<html xmlns=\"http://www.w3.org/1999/xhtml\"><head><title>{title}</title>\
         <style type=\"text/css\">li {{ list-style-type: none }} a {{ text-decoration: none }} \
         a:hover {{ color: red }} {extra_css}</style></head>\
         <body id=\"calibre_generated_inline_toc\"><h2>{title}</h2><ul></ul></body></html>"
    )
}

/// Port of `TOCAdder.process_toc_node`.
fn process_toc_node(dom: &mut Dom, parent: NodeId, node: &TOCNode, tocitem_href: Option<&str>) {
    let li = dom.new_element("li");
    dom.append_child(parent, li);

    let mut href = node.href.clone().unwrap_or_default();
    if let Some(base) = tocitem_href {
        if !href.is_empty() {
            href = relhref(base, &href);
        }
    }
    let a = dom.new_element("a");
    dom.node_mut(a).attrs.insert(
        "href".to_string(),
        if href.is_empty() {
            "#".to_string()
        } else {
            href
        },
    );
    let text = dom.new_text(node.title.as_deref().unwrap_or(""));
    dom.append_child(a, text);
    dom.append_child(li, a);

    if !node.children.is_empty() {
        let ul = dom.new_element("ul");
        dom.append_child(li, ul);
        for child in &node.children {
            process_toc_node(dom, ul, child, tocitem_href);
        }
    }
}

/// Port of `TOCAdder`.
pub struct TocAdder {
    pub generated_item_id: Option<String>,
    pub added_toc_guide_entry: bool,
    pub has_toc: bool,
    tocitem_id: Option<String>,
}

impl TocAdder {
    /// Port of `TOCAdder.__init__`.
    pub fn new(
        oeb: &mut OEBBook,
        opts: &TocOpts,
        replace_previous_inline_toc: bool,
        ignore_existing_toc: bool,
    ) -> Result<Self> {
        let title = opts
            .toc_title
            .clone()
            .unwrap_or_else(|| DEFAULT_TITLE.to_string());
        let has_toc = oeb.toc.count() > 1;
        let mut this = TocAdder {
            generated_item_id: None,
            added_toc_guide_entry: false,
            has_toc,
            tocitem_id: None,
        };

        if replace_previous_inline_toc {
            this.tocitem_id = find_previous_calibre_inline_toc(oeb);
        }
        if ignore_existing_toc && oeb.guide.get("toc").is_some() {
            oeb.guide.remove("toc");
        }

        if let Some(guide_toc) = oeb.guide.get("toc").cloned() {
            let href = urlnormalize(guide_toc.href.split('#').next().unwrap_or(&guide_toc.href));
            match oeb.manifest.get_by_href(&href).cloned() {
                Some(item) => {
                    let raw = oeb.container.read(&item.href).unwrap_or_default();
                    let html = String::from_utf8_lossy(&raw);
                    let dom = Dom::parse(&html);
                    let has_link = dom
                        .find_all_tag_global("a")
                        .into_iter()
                        .any(|a| dom.node(a).attrs.contains_key("href"));
                    if has_link {
                        if oeb.spine.index_of(&item.id).is_none() {
                            oeb.spine.add(&item.id, false);
                        }
                        return Ok(this);
                    } else if this.has_toc {
                        oeb.guide.remove("toc");
                    }
                }
                None => oeb.guide.remove("toc"),
            }
        }

        if !this.has_toc
            || oeb.guide.get("toc").is_some()
            || opts.no_inline_toc
            || opts.mobi_passthrough
        {
            return Ok(this);
        }

        let mut dom = Dom::parse(&render_template(
            &title,
            opts.extra_css.as_deref().unwrap_or(""),
        ));
        let ul = dom
            .find_first_tag_global("ul")
            .context("inline ToC template is missing its <ul>")?;
        let tocitem_href = this
            .tocitem_id
            .as_deref()
            .and_then(|id| oeb.manifest.get_by_id(id))
            .map(|m| m.href.clone());
        for child in &oeb.toc.root.children {
            process_toc_node(&mut dom, ul, child, tocitem_href.as_deref());
        }
        let rendered = dom.serialize(dom.root).into_bytes();

        let target_href = if let Some(tocitem_id) = this.tocitem_id.clone() {
            let href = oeb
                .manifest
                .get_by_id(&tocitem_id)
                .map(|m| m.href.clone())
                .context("previous inline ToC item vanished from the manifest")?;
            oeb.spine.remove_by_idref(&tocitem_id);
            oeb.container.write(&href, &rendered)?;
            if opts.mobi_toc_at_start {
                oeb.spine.insert(0, &tocitem_id, true);
            } else {
                oeb.spine.add(&tocitem_id, false);
            }
            href
        } else {
            let (id, href) = oeb.manifest.generate("contents", "contents.xhtml");
            oeb.manifest.add(&id, &href, XHTML_MIME);
            oeb.container.write(&href, &rendered)?;
            this.generated_item_id = Some(id.clone());
            if opts.mobi_toc_at_start {
                oeb.spine.insert(0, &id, true);
            } else {
                oeb.spine.add(&id, false);
            }
            href
        };

        oeb.guide
            .add("toc", Some("Table of Contents".to_string()), &target_href);
        this.added_toc_guide_entry = true;

        Ok(this)
    }

    /// Port of `TOCAdder.remove_generated_toc`: undo the inline ToC
    /// insertion, since `KF8Writer` generates its own and a MOBI 6
    /// sibling writer must not see it.
    pub fn remove_generated_toc(&mut self, oeb: &mut OEBBook) {
        if let Some(id) = self.generated_item_id.take() {
            oeb.manifest.remove(&id);
        }
        if self.added_toc_guide_entry {
            oeb.guide.remove("toc");
            self.added_toc_guide_entry = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn book_with_toc(dir: &std::path::Path) -> OEBBook {
        std::fs::write(
            dir.join("c1.html"),
            "<html><body><h1>One</h1></body></html>",
        )
        .unwrap();
        std::fs::write(
            dir.join("c2.html"),
            "<html><body><h1>Two</h1></body></html>",
        )
        .unwrap();
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
        oeb.manifest.add("c1", "c1.html", XHTML_MIME);
        oeb.manifest.add("c2", "c2.html", XHTML_MIME);
        oeb.spine.add("c1", true);
        oeb.spine.add("c2", true);
        oeb.toc
            .root
            .add(TOCNode::new(Some("One".into()), Some("c1.html".into())));
        oeb.toc
            .root
            .add(TOCNode::new(Some("Two".into()), Some("c2.html".into())));
        oeb
    }

    #[test]
    fn generates_an_inline_toc_and_a_guide_entry() {
        let dir = tempfile::tempdir().unwrap();
        let mut oeb = book_with_toc(dir.path());
        let opts = TocOpts::default();
        let mut adder = TocAdder::new(&mut oeb, &opts, true, false).unwrap();
        assert!(adder.generated_item_id.is_some());
        assert!(oeb.guide.get("toc").is_some());
        let id = adder.generated_item_id.clone().unwrap();
        let href = oeb.manifest.get_by_id(&id).unwrap().href.clone();
        let raw = oeb.container.read(&href).unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("One"));
        assert!(html.contains("Two"));

        adder.remove_generated_toc(&mut oeb);
        assert!(oeb.manifest.get_by_id(&id).is_none());
        assert!(oeb.guide.get("toc").is_none());
    }

    #[test]
    fn does_nothing_when_no_inline_toc_requested() {
        let dir = tempfile::tempdir().unwrap();
        let mut oeb = book_with_toc(dir.path());
        let opts = TocOpts {
            no_inline_toc: true,
            ..Default::default()
        };
        let adder = TocAdder::new(&mut oeb, &opts, true, false).unwrap();
        assert!(adder.generated_item_id.is_none());
        assert!(oeb.guide.get("toc").is_none());
    }
}
