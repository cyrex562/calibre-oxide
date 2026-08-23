//! Port of `old_src/src/calibre/ebooks/oeb/transforms/htmltoc.py`.

use crate::oeb::book::OEBBook;
use crate::oeb::constants::{CSS_MIME, XHTML_MIME};
use crate::oeb::toc::TOCNode;
use crate::oeb::transforms::filenames::urlnormalize;

pub const DEFAULT_TITLE: &str = "Table of Contents";

const NESTED_CSS: &str = r#"
.calibre_toc_header {
  text-align: center;
}
.calibre_toc_block {
  margin-left: 1.2em;
  text-indent: -1.2em;
}
.calibre_toc_block .calibre_toc_block {
  margin-left: 2.4em;
}
.calibre_toc_block .calibre_toc_block .calibre_toc_block {
  margin-left: 3.6em;
}
"#;

const CENTERED_CSS: &str = r#"
.calibre_toc_header {
  text-align: center;
}
.calibre_toc_block {
  text-align: center;
}
body > .calibre_toc_block {
  margin-top: 1.2em;
}
"#;

fn style_css(style: &str) -> &'static str {
    match style {
        "centered" => CENTERED_CSS,
        _ => NESTED_CSS,
    }
}

/// Port of `HTMLTOCAdder`: ensure the book has a navigable HTML table of
/// contents, generating one from `oeb.toc` when there isn't already a
/// usable `guide[toc]` document.
pub struct HTMLTOCAdder {
    pub title: Option<String>,
    /// `"nested"` or `"centered"`; unrecognized values fall back to
    /// `"nested"`, matching Python.
    pub style: String,
    /// `"end"` or `"start"`.
    pub position: String,
}

impl Default for HTMLTOCAdder {
    fn default() -> Self {
        HTMLTOCAdder {
            title: None,
            style: "nested".to_string(),
            position: "end".to_string(),
        }
    }
}

impl HTMLTOCAdder {
    pub fn call(&self, oeb: &mut OEBBook) {
        let has_toc = !oeb.toc.root.children.is_empty();

        if let Some(toc_ref) = oeb.guide.get("toc") {
            let href = urlnormalize(&toc_ref.href);
            let (path, _) = href.split_once('#').unwrap_or((&href, ""));
            let path = path.to_string();
            if let Some(item) = oeb.manifest.get_by_href(&path) {
                let item_href = item.href.clone();
                let media_type = item.media_type.clone();
                let is_xml_like = media_type == XHTML_MIME
                    || media_type == "text/html"
                    || media_type.ends_with("/xml")
                    || media_type.ends_with("+xml");
                let has_links = is_xml_like
                    && oeb
                        .container
                        .read(&item_href)
                        .ok()
                        .map(|raw| {
                            let html = String::from_utf8_lossy(&raw);
                            let dom = crate::dom::Dom::parse(&html);
                            dom.find_all_tag_global("a")
                                .into_iter()
                                .any(|a| dom.node(a).attrs.contains_key("href"))
                        })
                        .unwrap_or(false);
                if has_links {
                    let id = oeb.manifest.get_by_href(&item_href).unwrap().id.clone();
                    if oeb.spine.index_of(&id).is_none() {
                        if self.position == "end" {
                            oeb.spine.add(&id, false);
                        } else {
                            oeb.spine.insert(0, &id, true);
                        }
                    }
                    return;
                } else if has_toc {
                    oeb.guide.remove("toc");
                }
            } else {
                oeb.guide.remove("toc");
            }
        }
        if !has_toc {
            return;
        }

        let title = self
            .title
            .clone()
            .unwrap_or_else(|| DEFAULT_TITLE.to_string());
        let style = if style_css_is_known(&self.style) {
            self.style.clone()
        } else {
            "nested".to_string()
        };
        let (id, css_href) = oeb.manifest.generate("tocstyle", "tocstyle.css");
        oeb.manifest.add(&id, &css_href, CSS_MIME);
        let _ = oeb.container.write(&css_href, style_css(&style).as_bytes());

        let language = oeb
            .metadata
            .first("language")
            .map(|i| i.value.clone())
            .unwrap_or_else(|| "en".to_string());

        let mut body = String::new();
        body.push_str(&format!(
            "<h2 class=\"calibre_toc_header\">{}</h2>",
            escape(&title)
        ));
        render_toc_level(&oeb.toc.root, &mut body);
        let contents = format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"{lang}\"><head><title>{title}</title>\
             <link rel=\"stylesheet\" type=\"{css_mime}\" href=\"{css_href}\" /></head>\
             <body class=\"calibre_toc\">{body}</body></html>",
            lang = escape(&language),
            title = escape(&title),
            css_mime = CSS_MIME,
        );
        let (id, href) = oeb.manifest.generate("contents", "contents.xhtml");
        oeb.manifest.add(&id, &href, XHTML_MIME);
        let _ = oeb.container.write(&href, contents.as_bytes());
        if self.position == "end" {
            oeb.spine.add(&id, false);
        } else {
            oeb.spine.insert(0, &id, true);
        }
        oeb.guide
            .add("toc", Some("Table of Contents".to_string()), &href);
    }
}

fn style_css_is_known(style: &str) -> bool {
    matches!(style, "nested" | "centered")
}

fn render_toc_level(toc: &TOCNode, out: &mut String) {
    for node in &toc.children {
        out.push_str("<div class=\"calibre_toc_block\">");
        out.push_str(&format!(
            "<a class=\"calibre_toc_line\" href=\"{}\">{}</a>",
            escape(node.href.as_deref().unwrap_or("")),
            escape(node.title.as_deref().unwrap_or(""))
        ));
        render_toc_level(node, out);
        out.push_str("</div>");
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::toc::TOCNode;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn existing_toc_page_with_links_is_added_to_spine_not_regenerated() {
        let mut oeb = Builder::new()
            .part(
                "toc.html",
                XHTML_MIME,
                br#"<html><body><a href="a.html">A</a></body></html>"#,
                false,
            )
            .build();
        oeb.guide.add("toc", Some("TOC".into()), "toc.html");
        HTMLTOCAdder::default().call(&mut oeb);
        let id = oeb.manifest.get_by_href("toc.html").unwrap().id.clone();
        assert!(oeb.spine.index_of(&id).is_some());
        // No new "contents.xhtml" generated.
        assert!(oeb.manifest.get_by_href("contents.xhtml").is_none());
    }

    #[test]
    fn generates_inline_toc_when_no_usable_guide_toc() {
        let mut oeb = Builder::new().build();
        let mut chapter1 = TOCNode::new(Some("Chapter 1".into()), Some("a.html".into()));
        chapter1.add(TOCNode::new(
            Some("Section 1.1".into()),
            Some("a.html#s1".into()),
        ));
        oeb.toc.root.add(chapter1);
        HTMLTOCAdder::default().call(&mut oeb);
        let href = oeb
            .guide
            .get("toc")
            .expect("toc guide ref added")
            .href
            .clone();
        let raw = oeb.container.read(&href).unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("Chapter 1"), "{html}");
        assert!(html.contains("Section 1.1"), "{html}");
        assert!(html.contains("calibre_toc_block"), "{html}");
    }
}
