//! Port of `calibre.ebooks.oeb.polish.jacket` (issue #168's `jacket.py`
//! half; `kepubify.py` is separate scope, split into issue #571 since
//! it also depends on the unported `tts.py`, issue #167).
//!
//! **This is the `oeb/polish/` version.** `oeb/transforms/jacket.py`
//! (already ported, `crate::oeb::transforms::jacket`) is a different
//! file that generates the jacket page's actual markup from an
//! `OEBBook`'s metadata; this file is the "Polish Book" editor's
//! feature for finding/replacing/removing an existing jacket page
//! inside an on-disk, already-built book via a
//! [`crate::oeb::polish::container::Container`].
//!
//! # Disclosed narrowings
//!
//! - Real upstream's `render_jacket(container, jacket)` reads a
//!   `page_setup` output-profile preference (`load_defaults('page_setup')`
//!   + `output_profiles()`) to pick which named output profile's
//!   ratings-character/kindle-specific-CSS quirks to use. This crate's
//!   own [`crate::oeb::transforms::jacket::render_jacket`] already
//!   collapsed that whole GUI-preferences-driven profile lookup into a
//!   [`crate::oeb::transforms::jacket::JacketOptions`] struct with the
//!   project's own documented defaults (see that module's own doc) --
//!   this file reuses `JacketOptions::default()` rather than
//!   reintroducing an output-profile registry.
//! - No logging infrastructure exists on [`Container`] yet -- real
//!   upstream's `container.log(...)` diagnostic call when embedding a
//!   referenced image is dropped rather than routed anywhere.

use anyhow::{Context, Result};

use crate::dom::{Dom, NodeId};
use crate::oeb::constants::OPF2_NS;
use crate::oeb::polish::container::{Container, ParsedItem};
use crate::oeb::polish::cover::find_cover_page;
use crate::oeb::transforms::jacket::{
    is_jacket_document, referenced_images, render_jacket as render_jacket_template, JacketOptions,
};

/// Port of `is_legacy_jacket`: an old-style jacket page, identified by
/// an `<h1>`/`<h2>` whose `class` starts with `calibrerescale` (a class
/// prefix only the jacket template's own old CSS used).
pub fn is_legacy_jacket(dom: &Dom) -> bool {
    for tag in ["h1", "h2"] {
        for node in dom.find_all_tag_global(tag) {
            if dom
                .node(node)
                .attrs
                .get("class")
                .is_some_and(|c| c.starts_with("calibrerescale"))
            {
                return true;
            }
        }
    }
    false
}

/// Port of `is_current_jacket`: the same real check
/// [`crate::oeb::transforms::jacket`]'s own `Jacket` transform already
/// implements (a `<meta name="calibre-content" content="jacket">`),
/// reused directly rather than duplicated.
pub fn is_current_jacket(dom: &Dom) -> bool {
    is_jacket_document(dom)
}

/// Port of `find_existing_jacket`: locates an already-inserted jacket
/// page in the spine, if any.
pub fn find_existing_jacket(container: &mut Container) -> Result<Option<String>> {
    let is_azw3 = container.book_type() == "azw3";
    for (name, _linear) in container.spine_names()? {
        container.ensure_parsed(&name)?;
        let Ok(dom) = container.get_xhtml(&name) else {
            continue;
        };
        if is_azw3 {
            if is_current_jacket(dom) {
                return Ok(Some(name));
            }
        } else {
            let base = name.rsplit('/').next().unwrap_or(&name);
            if base.starts_with("jacket") && name.ends_with(".xhtml") && (is_current_jacket(dom) || is_legacy_jacket(dom)) {
                return Ok(Some(name));
            }
        }
    }
    Ok(None)
}

/// Port of `remove_jacket_images`: drops every `<img>` a jacket page at
/// `name` references from the manifest (used before regenerating/
/// removing that page).
pub fn remove_jacket_images(container: &mut Container, name: &str) -> Result<()> {
    container.ensure_parsed(name)?;
    let srcs: Vec<String> = {
        let dom = container.get_xhtml(name)?;
        dom.find_all_tag_global("img")
            .into_iter()
            .filter_map(|img| dom.node(img).attrs.get("src").cloned())
            .collect()
    };
    for src in srcs {
        if let Some(iname) = container.href_to_name(&src, Some(name)) {
            if container.has_name(&iname) {
                container.remove_item(&iname, true)?;
            }
        }
    }
    Ok(())
}

/// Port of `render_jacket`: builds the jacket page's DOM from the
/// container's own metadata, embedding any local `file://`-referenced
/// images the template pulls in.
pub fn render_jacket(container: &mut Container, jacket: &str) -> Result<Dom> {
    let opf_xml = container.opf()?.serialize();
    let mi = crate::opf::parse_opf(&String::from_utf8_lossy(&opf_xml)).context("parsing the book's own OPF metadata")?;
    let opts = JacketOptions::default();
    let mut root = render_jacket_template(&mi, &opts, "", &[], &[], "", false).context("rendering the jacket template")?;

    let embeds: Vec<(NodeId, String)> = referenced_images(&root);
    for (img, path) in embeds {
        let ext = path.rsplit('.').next().unwrap_or("").to_string();
        let opf_name = container.opf_name.clone();
        let item_node = container.generate_item(&format!("jacket_image.{ext}"), "jacket_img", None, true)?;
        let href_attr = container.opf()?.get_attr(item_node, "href").unwrap_or("").to_string();
        let Some(name) = container.href_to_name(&href_attr, Some(&opf_name)) else {
            continue;
        };
        let data = std::fs::read(&path).with_context(|| format!("Failed to read referenced jacket image {path}"))?;
        container.base.parsed_cache.insert(name.clone(), ParsedItem::Raw(data));
        container.commit_item(&name, false)?;
        let href = container.name_to_href(&name, Some(jacket));
        root.node_mut(img).attrs.insert("src".to_string(), href);
    }
    Ok(root)
}

/// Port of `replace_jacket`: overwrites an already-inserted jacket
/// page's content with a freshly rendered one.
pub fn replace_jacket(container: &mut Container, name: &str) -> Result<()> {
    let root = render_jacket(container, name)?;
    container.base.parsed_cache.insert(name.to_string(), ParsedItem::Xhtml(root));
    container.dirty(name);
    Ok(())
}

/// Port of `remove_jacket`: removes an existing jacket page (and its
/// referenced images) if one exists. Returns `false` if none was found.
pub fn remove_jacket(container: &mut Container) -> Result<bool> {
    let Some(name) = find_existing_jacket(container)? else {
        return Ok(false);
    };
    remove_jacket_images(container, &name)?;
    container.remove_item(&name, true)?;
    Ok(true)
}

/// Port of `add_or_replace_jacket`: creates a new jacket from the
/// book's own metadata, or replaces an existing one. Returns `true` if
/// an existing jacket was replaced (matching real upstream's own return
/// value, which the GUI uses to decide which status message to show).
pub fn add_or_replace_jacket(container: &mut Container) -> Result<bool> {
    let existing = find_existing_jacket(container)?;
    let found = existing.is_some();
    let (name, new_item_node) = match existing {
        Some(name) => {
            remove_jacket_images(container, &name)?;
            (name, None)
        }
        None => {
            let item_node = container.generate_item("jacket.xhtml", "jacket", None, true)?;
            let opf_name = container.opf_name.clone();
            let href_attr = container.opf()?.get_attr(item_node, "href").unwrap_or("").to_string();
            let name = container
                .href_to_name(&href_attr, Some(&opf_name))
                .context("resolving the newly generated jacket item's own href")?;
            (name, Some(item_node))
        }
    };

    replace_jacket(container, &name)?;

    if let Some(item_node) = new_item_node {
        let mut index = 0usize;
        let spine = container.spine_names()?;
        if let Some((first_name, _)) = spine.first() {
            if Some(first_name.clone()) == find_cover_page(container)? {
                index = 1;
            }
        }
        let item_id = container.opf()?.get_attr(item_node, "id").unwrap_or("").to_string();
        let opf_name = container.opf_name.clone();
        let spine_node = container
            .opf_xpath("//opf:spine")?
            .first()
            .copied()
            .context("OPF has no <spine>")?;
        let itemref = {
            let xml = container.opf_mut()?;
            let itemref = xml.new_element("itemref", Some(OPF2_NS));
            xml.set_attr(itemref, "idref", item_id);
            itemref
        };
        container.insert_into_xml(&opf_name, spine_node, itemref, Some(index))?;
    }

    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_epub(dir: &std::path::Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>The Jacket Test</dc:title>
    <dc:creator>A. Writer</dc:creator>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(dir.join("chap1.html"), b"<html><body><h1>Chapter One</h1></body></html>").unwrap();
    }

    #[test]
    fn no_jacket_exists_in_a_freshly_written_book() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert_eq!(find_existing_jacket(&mut c).unwrap(), None);
        assert!(!remove_jacket(&mut c).unwrap());
    }

    #[test]
    fn adding_a_jacket_inserts_it_at_the_start_of_the_spine_with_real_metadata() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        let replaced = add_or_replace_jacket(&mut c).unwrap();
        assert!(!replaced, "no jacket existed yet");

        let name = find_existing_jacket(&mut c).unwrap().expect("a jacket should now exist");
        assert!(name.contains("jacket"), "{name}");

        // It's the first item in the spine.
        let spine = c.spine_names().unwrap();
        assert_eq!(spine[0].0, name);
        assert_eq!(spine[1].0, "chap1.html");

        // The rendered content carries the real book title/author.
        c.commit_item(&name, true).unwrap();
        let raw = fs::read_to_string(dir.path().join(&name)).unwrap();
        assert!(raw.contains("The Jacket Test"), "{raw}");
        assert!(raw.contains("A. Writer"), "{raw}");
        assert!(is_current_jacket(c.get_xhtml(&name).unwrap()));
    }

    #[test]
    fn adding_a_jacket_twice_replaces_rather_than_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        assert!(!add_or_replace_jacket(&mut c).unwrap());
        let replaced = add_or_replace_jacket(&mut c).unwrap();
        assert!(replaced, "the second call should find and replace the first jacket");

        let spine = c.spine_names().unwrap();
        let jacket_count = spine.iter().filter(|(n, _)| n.contains("jacket")).count();
        assert_eq!(jacket_count, 1, "{spine:?}");
    }

    #[test]
    fn removing_a_jacket_drops_it_from_the_spine() {
        let dir = tempfile::tempdir().unwrap();
        write_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        add_or_replace_jacket(&mut c).unwrap();
        assert!(find_existing_jacket(&mut c).unwrap().is_some());

        assert!(remove_jacket(&mut c).unwrap());
        assert_eq!(find_existing_jacket(&mut c).unwrap(), None);
        let spine = c.spine_names().unwrap();
        assert_eq!(spine.len(), 1);
        assert_eq!(spine[0].0, "chap1.html");
    }

    #[test]
    fn a_legacy_rescale_class_is_detected_as_a_jacket() {
        let dom = Dom::parse(r#"<html><body><h1 class="calibrerescale100">Title</h1></body></html>"#);
        assert!(is_legacy_jacket(&dom));
        assert!(!is_current_jacket(&dom));
    }
}
