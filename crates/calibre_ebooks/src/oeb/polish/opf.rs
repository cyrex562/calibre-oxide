//! Port of `old_src/src/calibre/ebooks/oeb/polish/opf.py`.

use anyhow::Result;

use crate::oeb::constants::OPF2_NS;

use super::container::Container;
use super::xmltree::XmlNodeId;

/// Narrow stand-in for `calibre.utils.localization.canonicalize_lang`
/// (a multi-thousand-entry ISO-639 table). A second, independent copy
/// of the same narrow stand-in already exists at
/// `crate::mobi::headers::canonicalize_lang` (private to that module,
/// used for MOBI EXTH language codes); that one is private and
/// MOBI-specific in spirit even though the current implementation is
/// generic, so this is a fresh, equally-narrow copy rather than
/// reaching into an unrelated module's private helper. Real ISO-639
/// name/code canonicalization is out of scope for this port -- both
/// call sites only need "best effort" normalization (lowercase + trim)
/// for storage in a `Metadata`-like `languages` field.
fn canonicalize_lang(raw: &str) -> Option<String> {
    let s = raw.trim().to_lowercase();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Port of `get_book_language`: the first non-empty, canonicalizable
/// `<dc:language>` value in the OPF.
pub fn get_book_language(container: &mut Container) -> Result<Option<String>> {
    let nodes = container.opf_xpath("//dc:language")?;
    let opf_name = container.opf_name.clone();
    let xml = container.get_xml(&opf_name)?;
    for node in nodes {
        if let Some(raw) = xml.element_text(node) {
            let first = raw.split(',').next().unwrap_or("").trim();
            if let Some(code) = canonicalize_lang(first) {
                return Ok(Some(code));
            }
        }
    }
    Ok(None)
}

/// Port of `set_guide_item`: sets (or, if `name` is `None`, removes) the
/// `<guide>` `<reference type="item_type">` entry pointing at `name`
/// (optionally with a `#frag` fragment), creating the `<guide>` element
/// itself if needed.
pub fn set_guide_item(
    container: &mut Container,
    item_type: &str,
    title: &str,
    name: Option<&str>,
    frag: Option<&str>,
) -> Result<()> {
    let opf_name = container.opf_name.clone();
    let href = name.map(|n| {
        let mut h = container.name_to_href(n, Some(&opf_name));
        if let Some(f) = frag {
            h.push('#');
            h.push_str(f);
        }
        h
    });

    let mut guides = container.opf_xpath("//opf:guide")?;
    if guides.is_empty() && href.is_some() {
        let root = container.opf_root()?;
        let xml = container.opf_mut()?;
        let g = xml.new_element("guide", Some(OPF2_NS));
        xml.insert_element(root, g, None);
        guides = vec![g];
    }

    let item_type_lower = item_type.to_lowercase();
    for guide in guides {
        let mut matches: Vec<XmlNodeId> = {
            let xml = container.get_xml(&opf_name)?;
            xml.element_children(guide)
                .into_iter()
                .filter(|&c| {
                    xml.local_name(c) == Some("reference")
                        && xml.get_attr(c, "type").map(|t| t.to_lowercase())
                            == Some(item_type_lower.clone())
                })
                .collect()
        };
        if matches.is_empty() && href.is_some() {
            let xml = container.get_xml_mut(&opf_name)?;
            let r = xml.new_element("reference", Some(OPF2_NS));
            xml.set_attr(r, "type", item_type);
            xml.insert_element(guide, r, None);
            matches.push(r);
        }
        let xml = container.get_xml_mut(&opf_name)?;
        for m in matches {
            match &href {
                Some(href) => {
                    xml.set_attr(m, "title", title);
                    xml.set_attr(m, "href", href.clone());
                    xml.set_attr(m, "type", item_type);
                }
                None => xml.detach(m),
            }
        }
    }
    container.dirty(&opf_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_book(dir: &std::path::Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test</dc:title>
    <dc:language>en-GB, fr</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="cover" href="cover.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(dir.join("chap1.html"), b"<html><body>hi</body></html>").unwrap();
        fs::write(dir.join("cover.html"), b"<html><body>cover</body></html>").unwrap();
    }

    #[test]
    fn get_book_language_takes_first_code_before_comma() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert_eq!(
            get_book_language(&mut c).unwrap(),
            Some("en-gb".to_string())
        );
    }

    #[test]
    fn set_guide_item_creates_guide_and_reference() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        set_guide_item(&mut c, "cover", "Cover", Some("cover.html"), None).unwrap();
        let gmap = c.guide_type_map().unwrap();
        assert_eq!(gmap.get("cover").unwrap(), "cover.html");
    }

    #[test]
    fn set_guide_item_updates_existing_reference() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        set_guide_item(&mut c, "cover", "Cover", Some("cover.html"), None).unwrap();
        set_guide_item(
            &mut c,
            "cover",
            "Cover Page",
            Some("chap1.html"),
            Some("top"),
        )
        .unwrap();
        let gmap = c.guide_type_map().unwrap();
        assert_eq!(gmap.get("cover").unwrap(), "chap1.html");
    }

    #[test]
    fn set_guide_item_with_no_name_removes_reference() {
        let dir = tempfile::tempdir().unwrap();
        write_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        set_guide_item(&mut c, "cover", "Cover", Some("cover.html"), None).unwrap();
        set_guide_item(&mut c, "cover", "Cover", None, None).unwrap();
        let gmap = c.guide_type_map().unwrap();
        assert!(!gmap.contains_key("cover"));
    }
}
