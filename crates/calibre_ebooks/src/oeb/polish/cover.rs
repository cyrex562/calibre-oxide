//! Port of `old_src/src/calibre/ebooks/oeb/polish/cover.py`.
//!
//! Per issue #166: the EPUB-side cover-detection/cover-setting
//! machinery (`find_cover_image*`, `find_cover_page`,
//! `mark_as_cover*`/`mark_as_titlepage`, `create_epub_cover`,
//! `set_epub_cover`/`set_cover`, `clean_opf`, `has_epub_cover`) is ported
//! for real. The AZW3-specific functions (`set_azw3_cover`,
//! `get_azw3_raster_cover_name`, `mark_as_cover_azw3`) are given real
//! signatures with `todo!()` bodies: they need `Azw3Container`/
//! `opf_to_azw3`, which `container.rs` (issue #161) already left as a
//! documented gap (see [`super::container::Azw3Container::commit`]'s
//! docs) blocked on wiring `Plumber` + `mobi::writer2`/`writer8`
//! together -- the same shape of gap issue #157 tracks for joint
//! MOBI6+KF8 output. That gap is not attempted here either.
//!
//! # Design notes
//!
//! **No Python callable `cover_path`.** Python's `cover_path` parameter
//! is documented (`set_cover`'s own docstring) as "either the absolute
//! path to an image file or the canonical name of an image in the
//! book" -- a plain string either way. A handful of *internal* GUI call
//! sites additionally pass a callable (`cover_path('write_image', dest)`)
//! so cover bytes that are already in memory (e.g. freshly downloaded
//! metadata) can be streamed in without a round trip through a temp
//! file; that shape is GUI-specific plumbing, not part of the documented
//! library contract this port targets, so it is dropped -- `cover_path`
//! is always `&str` here, matching the docstring exactly.
//!
//! **`report`/`image_callback` as `FnMut` trait objects**, matching this
//! crate's existing convention for optional Python callables (see
//! `container.rs`'s `replace_links`).
//!
//! **`Container::spine_items`(the Python property) has no Rust
//! equivalent** -- [`super::container::Container`] only exposes
//! `spine_iter`/`spine_names`, which (correctly, for their own callers)
//! reorder non-linear items after linear ones. `cover.py`'s heuristics
//! need the *raw* `<spine>` document order instead, so this module has
//! its own small `spine_item_names` matching Python's `spine_items`
//! property (skipping its extra `abspath_to_name(path)` round trip --
//! resolving straight to names is equivalent and simpler).

use std::collections::{HashMap, HashSet};
use std::fs;

use anyhow::{Context, Result};

use crate::dom::{Dom, NodeId};
use crate::oeb::constants::{OEB_DOCS, OPF2_NS};

use super::container::Container;
use super::replace;
use super::toc;
use super::xmltree::XmlNodeId;

// ===================================================================
// Shared helpers
// ===================================================================

/// Port of `is_raster_image`.
pub fn is_raster_image(media_type: Option<&str>) -> bool {
    match media_type {
        Some(mt) => matches!(
            mt.to_lowercase().as_str(),
            "image/png" | "image/jpeg" | "image/jpg" | "image/gif"
        ),
        None => false,
    }
}

/// Port of `COVER_TYPES`.
const COVER_TYPES: &[&str] = &[
    "coverimagestandard",
    "other.ms-coverimage-standard",
    "other.ms-titleimage-standard",
    "other.ms-titleimage",
    "other.ms-coverimage",
    "other.ms-thumbimage-standard",
    "other.ms-thumbimage",
    "thumbimagestandard",
    "cover",
];

fn is_cover_type(lowercase_type: &str) -> bool {
    COVER_TYPES.contains(&lowercase_type)
}

/// Port of the `options` dict `set_cover`/`set_epub_cover`/
/// `create_epub_cover`/`set_azw3_cover` accept. `existing_image` is only
/// meaningful on [`set_cover`]/[`set_azw3_cover`]/[`set_epub_cover`]
/// (where it changes how `cover_path` is interpreted); `keep_aspect`/
/// `no_svg` are only meaningful on [`create_epub_cover`].
///
/// When `None` is passed where Python would fall back to
/// `load_defaults('epub_output')` (a persisted user config file this
/// port has no equivalent of), the conservative defaults `keep_aspect =
/// false, no_svg = false` are used instead -- the same defaults
/// `load_defaults` itself falls back to when the user has never touched
/// those settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct CoverOptions {
    pub existing_image: bool,
    pub keep_aspect: bool,
    pub no_svg: bool,
}

/// `set_epub_cover`'s optional callback, invoked once with `(cover_image,
/// wrapped_image)` right before the old cover is replaced -- port of
/// Python's `image_callback(cover_image, wrapped_image)`. Named so the
/// parameter type in [`set_epub_cover`]'s signature stays readable.
pub type ImageCallback<'a> = &'a mut dyn FnMut(Option<&str>, Option<&str>);

/// Python's `template % href` substitution (`%s` -> `href`, `%%` ->
/// literal `%`), used by [`SVG_TEMPLATE`]/[`NONSVG_TEMPLATE`], both of
/// which contain exactly one `%s`.
fn percent_format(template: &str, href: &str) -> String {
    let (before, after) = template.split_once("%s").unwrap_or((template, ""));
    format!(
        "{}{href}{}",
        before.replace("%%", "%"),
        after.replace("%%", "%")
    )
}

/// Port of `CoverManager.SVG_TEMPLATE` (`old_src/.../oeb/transforms/cover.py`).
const SVG_TEMPLATE: &str = "\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">
    <head>
        <meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />
        <meta name=\"calibre:cover\" content=\"true\" />
        <title>Cover</title>
        <style type=\"text/css\" title=\"override_css\">
            @page {padding: 0pt; margin:0pt}
            body { text-align: center; padding:0pt; margin: 0pt; }
        </style>
    </head>
    <body>
        <div>
            <svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\"
                xmlns:xlink=\"http://www.w3.org/1999/xlink\"
                width=\"100%%\" height=\"100%%\" viewBox=\"__viewbox__\"
                preserveAspectRatio=\"__ar__\">
                <image width=\"__width__\" height=\"__height__\" xlink:href=\"%s\"/>
            </svg>
        </div>
    </body>
</html>
";

/// Port of `CoverManager.NONSVG_TEMPLATE`.
const NONSVG_TEMPLATE: &str = "\
<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">
    <head>
        <meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />
        <meta name=\"calibre:cover\" content=\"true\" />
        <title>Cover</title>
        <style type=\"text/css\" title=\"override_css\">
            @page {padding: 0pt; margin:0pt}
            body { text-align: center; padding:0pt; margin: 0pt }
            div { padding:0pt; margin: 0pt }
            img { padding:0pt; margin: 0pt }
        </style>
    </head>
    <body>
        <div>
            <img src=\"%s\" alt=\"cover\" __style__ />
        </div>
    </body>
</html>
";

/// Port of `Container.spine_items` (a Python property `container.py`
/// doesn't expose in this port's `Container` -- see the module docs).
fn spine_item_names(container: &mut Container) -> Result<Vec<String>> {
    let manifest_id_map = container.manifest_id_map()?;
    let items = container.opf_xpath("//opf:spine/opf:itemref[@idref]")?;
    let opf_name = container.opf_name.clone();
    let xml = container.get_xml(&opf_name)?;
    let mut out = Vec::new();
    for item in items {
        if let Some(idref) = xml.get_attr(item, "idref") {
            if let Some(name) = manifest_id_map.get(idref) {
                if container.name_path_map.contains_key(name) {
                    out.push(name.clone());
                }
            }
        }
    }
    Ok(out)
}

/// Port of `get_guides`. Python returns the (possibly multi-element)
/// list of `<guide>` matches; every real call site only ever inserts
/// into all of them identically, and a well-formed OPF never has more
/// than one, so this returns the first (or newly created) `<guide>`
/// directly rather than a `Vec`.
fn get_guides(container: &mut Container) -> Result<XmlNodeId> {
    let existing = container.opf_xpath("//opf:guide")?;
    if let Some(&g) = existing.first() {
        return Ok(g);
    }
    let package = container.opf_root()?;
    let opf_name = container.opf_name.clone();
    let xml = container.get_xml_mut(&opf_name)?;
    let guide = xml.new_element("guide", Some(OPF2_NS));
    xml.insert_element(package, guide, None);
    Ok(guide)
}

fn has_svg_ancestor(dom: &Dom, id: NodeId) -> bool {
    let mut cur = dom.parent(id);
    while let Some(p) = cur {
        if dom.tag(p) == Some("svg") {
            return true;
        }
        cur = dom.parent(p);
    }
    false
}

// ===================================================================
// AZW3 cover handling -- out of scope, see module docs.
// ===================================================================

/// Port of `set_azw3_cover`. See the module docs: blocked on
/// `Azw3Container`/`opf_to_azw3` (`container.rs`, issue #161's
/// documented gap; same shape of gap as issue #157). Not attempted here.
pub fn set_azw3_cover(
    _container: &mut Container,
    _cover_path: &str,
    _report: &mut dyn FnMut(&str),
    _options: Option<&CoverOptions>,
) -> Result<()> {
    todo!(
        "placeholder: AZW3 cover-setting needs Azw3Container/opf_to_azw3 -- \
         see container.rs's Azw3Container::commit docs for the tracked gap \
         (issue #161/#157)"
    )
}

/// Port of `get_azw3_raster_cover_name`. See the module docs.
pub fn get_azw3_raster_cover_name(_container: &mut Container) -> Result<Option<String>> {
    todo!(
        "placeholder: AZW3 cover reading needs Azw3Container/opf_to_azw3 -- \
         see container.rs's Azw3Container::commit docs for the tracked gap \
         (issue #161/#157)"
    )
}

/// Port of `mark_as_cover_azw3`. See the module docs.
pub fn mark_as_cover_azw3(_container: &mut Container, _name: &str) -> Result<()> {
    todo!(
        "placeholder: AZW3 cover marking needs Azw3Container/opf_to_azw3 -- \
         see container.rs's Azw3Container::commit docs for the tracked gap \
         (issue #161/#157)"
    )
}

// ===================================================================
// Format dispatch
// ===================================================================

/// Port of `get_raster_cover_name`.
pub fn get_raster_cover_name(container: &mut Container) -> Result<Option<String>> {
    if container.book_type() == "azw3" {
        return get_azw3_raster_cover_name(container);
    }
    find_cover_image(container, true)
}

/// Port of `get_cover_page_name`.
pub fn get_cover_page_name(container: &mut Container) -> Result<Option<String>> {
    if container.book_type() == "azw3" {
        return Ok(None);
    }
    find_cover_page(container)
}

/// Port of `set_cover`. Sets the cover of the book to the image pointed
/// to by `cover_path`: either the absolute path to an image file, or
/// (when `options.existing_image` is set) the canonical name of an image
/// already in the book.
pub fn set_cover(
    container: &mut Container,
    cover_path: &str,
    mut report: Option<&mut dyn FnMut(&str)>,
    options: Option<&CoverOptions>,
) -> Result<()> {
    let mut noop = |_: &str| {};
    let report: &mut dyn FnMut(&str) = match report.take() {
        Some(r) => r,
        None => &mut noop,
    };
    if container.book_type() == "azw3" {
        set_azw3_cover(container, cover_path, report, options)
    } else {
        set_epub_cover(container, cover_path, report, options, None).map(|_| ())
    }
}

/// Port of `mark_as_cover`: marks the specified image as the cover
/// image.
pub fn mark_as_cover(container: &mut Container, name: &str) -> Result<()> {
    let mt = container
        .base
        .mime_map
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Cannot mark {name} as cover as it does not exist"))?;
    if !is_raster_image(Some(&mt)) {
        anyhow::bail!("Cannot mark {name} as the cover image as it is not a raster image");
    }
    if container.book_type() == "azw3" {
        mark_as_cover_azw3(container, name)
    } else {
        mark_as_cover_epub(container, name)
    }
}

// ===================================================================
// The delightful EPUB cover processing
// ===================================================================

/// Port of `find_cover_image2` (OPF2, `<meta name="cover">`/`<guide>`
/// heuristics).
pub fn find_cover_image2(container: &mut Container, strict: bool) -> Result<Option<String>> {
    let manifest_id_map = container.manifest_id_map()?;
    let metas = container.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#)?;
    {
        let opf_name = container.opf_name.clone();
        let xml = container.get_xml(&opf_name)?;
        for meta in &metas {
            if let Some(item_id) = xml.get_attr(*meta, "content") {
                if let Some(name) = manifest_id_map.get(item_id) {
                    let mt = container.base.mime_map.get(name).map(|s| s.as_str());
                    if is_raster_image(mt) {
                        return Ok(Some(name.clone()));
                    }
                }
            }
        }
    }

    let guide_type_map = container.guide_type_map()?;
    for (ref_type, name) in &guide_type_map {
        if ref_type.eq_ignore_ascii_case("cover") {
            let mt = container.base.mime_map.get(name).map(|s| s.as_str());
            if is_raster_image(mt) {
                return Ok(Some(name.clone()));
            }
        }
    }

    if strict {
        return Ok(None);
    }

    let mut largest: Option<(String, u64)> = None;
    for (ref_type, name) in &guide_type_map {
        if !is_cover_type(&ref_type.to_lowercase()) {
            continue;
        }
        let mt = container.base.mime_map.get(name).map(|s| s.as_str());
        if !is_raster_image(mt) {
            continue;
        }
        if let Some(path) = container.name_path_map.get(name) {
            if let Ok(meta) = fs::metadata(path) {
                let sz = meta.len();
                let is_larger = largest.as_ref().map(|(_, s)| sz > *s).unwrap_or(true);
                if is_larger {
                    largest = Some((name.clone(), sz));
                }
            }
        }
    }
    Ok(largest.map(|(n, _)| n))
}

/// Port of `find_cover_image3` (OPF3, `properties="cover-image"`).
pub fn find_cover_image3(container: &mut Container) -> Result<Option<String>> {
    if let Some(name) = container
        .manifest_items_with_property("cover-image")?
        .into_iter()
        .next()
    {
        return Ok(Some(name));
    }
    let manifest_id_map = container.manifest_id_map()?;
    let metas = container.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#)?;
    let opf_name = container.opf_name.clone();
    let xml = container.get_xml(&opf_name)?;
    for meta in metas {
        if let Some(item_id) = xml.get_attr(meta, "content") {
            if let Some(name) = manifest_id_map.get(item_id) {
                let mt = container.base.mime_map.get(name).map(|s| s.as_str());
                if is_raster_image(mt) {
                    return Ok(Some(name.clone()));
                }
            }
        }
    }
    Ok(None)
}

/// Port of `find_cover_image`: finds a raster image marked as a cover in
/// the OPF.
pub fn find_cover_image(container: &mut Container, strict: bool) -> Result<Option<String>> {
    let (major, _minor) = container.opf_version_parsed()?;
    if major < 3 {
        find_cover_image2(container, strict)
    } else {
        find_cover_image3(container)
    }
}

/// Port of `mark_as_cover_epub`.
pub fn mark_as_cover_epub(container: &mut Container, name: &str) -> Result<()> {
    let manifest_id_map = container.manifest_id_map()?;
    let mid = manifest_id_map
        .iter()
        .find(|(_, n)| n.as_str() == name)
        .map(|(id, _)| id.clone())
        .ok_or_else(|| anyhow::anyhow!("Cannot mark {name} as cover as it is not in manifest"))?;
    let (major, _minor) = container.opf_version_parsed()?;
    let opf_name = container.opf_name.clone();

    let metas = container.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#)?;
    for meta in metas {
        container.remove_from_xml(&opf_name, meta)?;
    }

    let refs = container.opf_xpath("//opf:guide/opf:reference[@href and @type]")?;
    let mut to_check = Vec::new();
    {
        let xml = container.get_xml(&opf_name)?;
        for r in refs {
            let typ = xml.get_attr(r, "type").unwrap_or("").to_lowercase();
            if !is_cover_type(&typ) {
                continue;
            }
            let href = xml.get_attr(r, "href").unwrap_or("").to_string();
            to_check.push((r, href));
        }
    }
    for (r, href) in to_check {
        let rname = container.href_to_name(&href, Some(&opf_name));
        let mt = rname
            .as_ref()
            .and_then(|n| container.base.mime_map.get(n))
            .map(|s| s.as_str());
        if is_raster_image(mt) {
            container.remove_from_xml(&opf_name, r)?;
        }
    }

    if major < 3 {
        let metadata_nodes = container.opf_xpath("//opf:metadata")?;
        {
            let xml = container.get_xml_mut(&opf_name)?;
            for metadata in metadata_nodes {
                let m = xml.new_element("meta", Some(OPF2_NS));
                xml.set_attr(m, "name", "cover");
                xml.set_attr(m, "content", mid.clone());
                xml.insert_element(metadata, m, None);
            }
        }
        let has_cover_ref = !container
            .opf_xpath(r#"//opf:guide/opf:reference[@type="cover"]"#)?
            .is_empty();
        if !has_cover_ref {
            let href = container.name_to_href(name, Some(&opf_name));
            let guide = get_guides(container)?;
            let xml = container.get_xml_mut(&opf_name)?;
            let r = xml.new_element("reference", Some(OPF2_NS));
            xml.set_attr(r, "type", "cover");
            xml.set_attr(r, "href", href);
            xml.insert_element(guide, r, None);
        }
    } else {
        container.apply_unique_properties(Some(name), &["cover-image"])?;
    }

    container.dirty(&opf_name);
    Ok(())
}

/// Port of `mark_as_titlepage`: marks the specified HTML file as the
/// titlepage of the EPUB. If `move_to_start` the HTML file is moved to
/// the start of the spine.
pub fn mark_as_titlepage(container: &mut Container, name: &str, move_to_start: bool) -> Result<()> {
    let (major, _minor) = container.opf_version_parsed()?;
    let opf_name = container.opf_name.clone();
    if move_to_start {
        let spine = container.spine_iter()?;
        let (item, _, linear) = spine
            .into_iter()
            .find(|(_, q, _)| q == name)
            .ok_or_else(|| anyhow::anyhow!("{name} is not in the spine"))?;
        if !linear {
            let xml = container.get_xml_mut(&opf_name)?;
            xml.set_attr(item, "linear", "yes");
        }
        let spine_node = container
            .get_xml(&opf_name)?
            .parent(item)
            .ok_or_else(|| anyhow::anyhow!("spine itemref has no parent"))?;
        let is_first = container
            .get_xml(&opf_name)?
            .element_children(spine_node)
            .first()
            == Some(&item);
        if !is_first {
            container.insert_into_xml(&opf_name, spine_node, item, Some(0))?;
        }
    }
    if major < 3 {
        let refs = container.opf_xpath(r#"//opf:guide/opf:reference[@type="cover"]"#)?;
        for r in refs {
            container.remove_from_xml(&opf_name, r)?;
        }
        let href = container.name_to_href(name, Some(&opf_name));
        let guide = get_guides(container)?;
        let xml = container.get_xml_mut(&opf_name)?;
        let r = xml.new_element("reference", Some(OPF2_NS));
        xml.set_attr(r, "type", "cover");
        xml.set_attr(r, "href", href);
        xml.insert_element(guide, r, None);
    } else {
        container.apply_unique_properties(Some(name), &["calibre:title-page"])?;
    }
    container.dirty(&opf_name);
    Ok(())
}

/// Port of `find_cover_page`: finds a document marked as a cover in the
/// OPF.
pub fn find_cover_page(container: &mut Container) -> Result<Option<String>> {
    let (major, _minor) = container.opf_version_parsed()?;
    if major < 3 {
        let guide_type_map = container.guide_type_map()?;
        for (ref_type, name) in &guide_type_map {
            if ref_type.eq_ignore_ascii_case("cover") {
                let mt = container
                    .base
                    .mime_map
                    .get(name)
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if OEB_DOCS.contains(&mt.as_str()) {
                    return Ok(Some(name.clone()));
                }
            }
        }
        Ok(None)
    } else {
        if let Some(name) = container
            .manifest_items_with_property("calibre:title-page")?
            .into_iter()
            .next()
        {
            return Ok(Some(name));
        }
        for landmark in toc::get_landmarks(container)? {
            if landmark.r#type == "cover" {
                let mt = container
                    .base
                    .mime_map
                    .get(&landmark.dest)
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                if OEB_DOCS.contains(&mt.as_str()) {
                    return Ok(Some(landmark.dest));
                }
            }
        }
        Ok(None)
    }
}

/// Port of `fix_conversion_titlepage_links_in_nav`.
pub fn fix_conversion_titlepage_links_in_nav(container: &mut Container) -> Result<()> {
    let Some(cover_page_name) = find_cover_page(container)? else {
        return Ok(());
    };
    let Some(nav_page_name) = toc::find_existing_nav_toc(container)? else {
        return Ok(());
    };
    container.ensure_parsed(&nav_page_name)?;
    let matches: Vec<NodeId> = {
        let dom = container.get_xhtml(&nav_page_name)?;
        dom.preorder_elements(dom.root)
            .into_iter()
            .filter(|&e| {
                dom.node(e)
                    .attrs
                    .contains_key("data-calibre-removed-titlepage")
            })
            .collect()
    };
    if matches.is_empty() {
        return Ok(());
    }
    let href = container.name_to_href(&cover_page_name, Some(&nav_page_name));
    let dom = container.get_xhtml_mut(&nav_page_name)?;
    for elem in matches {
        dom.node_mut(elem)
            .attrs
            .shift_remove("data-calibre-removed-titlepage");
        dom.node_mut(elem)
            .attrs
            .insert("href".to_string(), href.clone());
    }
    container.dirty(&nav_page_name);
    Ok(())
}

/// Port of `find_cover_image_in_page`.
pub fn find_cover_image_in_page(
    container: &mut Container,
    cover_page: &str,
) -> Result<Option<String>> {
    container.ensure_parsed(cover_page)?;
    let dom = container.get_xhtml(cover_page)?;
    let Some(body) = dom.find_first_tag_global("body") else {
        return Ok(None);
    };
    let mut images = Vec::new();
    for el in dom.preorder_elements(body) {
        if el == body {
            continue;
        }
        match dom.tag(el) {
            Some("img") if dom.node(el).attrs.contains_key("src") => images.push(el),
            Some("image") if has_svg_ancestor(dom, el) => images.push(el),
            _ => {}
        }
    }
    let text_is_empty = dom.text_content(body).chars().all(|c| c.is_whitespace());
    if !text_is_empty || images.len() > 1 {
        return Ok(None);
    }
    let Some(&first) = images.first() else {
        return Ok(None);
    };
    let href = dom
        .node(first)
        .attrs
        .get("src")
        .or_else(|| dom.node(first).attrs.get("href"))
        .cloned();
    Ok(href.and_then(|h| container.href_to_name(&h, Some(cover_page))))
}

/// Port of `clean_opf`: removes all references to covers from the OPF,
/// returning the names of manifest items that may now be orphaned.
pub fn clean_opf(container: &mut Container) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let manifest_id_map = container.manifest_id_map()?;
    let opf_name = container.opf_name.clone();

    let metas = container.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#)?;
    for meta in metas {
        let name = {
            let xml = container.get_xml(&opf_name)?;
            xml.get_attr(meta, "content")
                .and_then(|c| manifest_id_map.get(c))
                .cloned()
        };
        container.remove_from_xml(&opf_name, meta)?;
        if let Some(name) = name {
            if container.name_path_map.contains_key(&name) {
                removed.push(name);
            }
        }
    }

    let gtm = container.guide_type_map()?;
    let refs = container.opf_xpath("//opf:guide/opf:reference[@type]")?;
    for r in refs {
        let typ = {
            let xml = container.get_xml(&opf_name)?;
            xml.get_attr(r, "type").unwrap_or("").to_string()
        };
        if is_cover_type(&typ.to_lowercase()) {
            container.remove_from_xml(&opf_name, r)?;
            if let Some(name) = gtm.get(&typ) {
                if container.name_path_map.contains_key(name) {
                    removed.push(name.clone());
                }
            }
        }
    }

    let (major, _minor) = container.opf_version_parsed()?;
    if major > 2 {
        let (removed_names, _added) =
            container.apply_unique_properties(None, &["cover-image", "calibre:title-page"])?;
        removed.extend(removed_names);
    }
    container.dirty(&opf_name);
    Ok(removed)
}

/// Port of `create_epub_cover`. `existing_image`, when set, is the
/// canonical name of an already-in-book image to use instead of copying
/// `cover_path`'s bytes in as a new manifest item.
pub fn create_epub_cover(
    container: &mut Container,
    cover_path: &str,
    existing_image: Option<&str>,
    options: Option<&CoverOptions>,
) -> Result<(String, String)> {
    // Port of `cover_path.rpartition('.')[-1].lower()`: `rpartition`
    // never raises for a plain string, and yields the *whole* string
    // (not `'jpeg'`) when there's no `.` -- the `try/except` in Python
    // only ever fires for the callable form this port doesn't support
    // (see the module docs), so it's preserved here as a match, not a
    // fallback-on-error.
    let ext = match cover_path.rsplit_once('.') {
        Some((_, e)) => e.to_lowercase(),
        None => cover_path.to_lowercase(),
    };
    let mut cname = format!("cover.{ext}");
    let mut tname = "titlepage.xhtml".to_string();
    let recommended = replace::get_recommended_folders(container, &[cname.clone(), tname.clone()]);

    let (raster_cover_item, raster_cover) = if let Some(existing) = existing_image {
        let manifest_id_map = container.manifest_id_map()?;
        let manifest_id = manifest_id_map
            .iter()
            .find(|(_, n)| n.as_str() == existing)
            .map(|(id, _)| id.clone())
            .ok_or_else(|| anyhow::anyhow!("{existing} is not in the manifest"))?;
        let items = container.opf_xpath("//opf:manifest/opf:item")?;
        let opf_name = container.opf_name.clone();
        let item = {
            let xml = container.get_xml(&opf_name)?;
            items
                .into_iter()
                .find(|&it| xml.get_attr(it, "id") == Some(manifest_id.as_str()))
                .ok_or_else(|| anyhow::anyhow!("manifest item {manifest_id} not found"))?
        };
        (item, existing.to_string())
    } else {
        if let Some(folder) = recommended.get(&cname) {
            if !folder.is_empty() {
                cname = format!("{folder}/{cname}");
            }
        }
        let item = container.generate_item(&cname, "cover", None, true)?;
        let opf_name = container.opf_name.clone();
        let href = container
            .get_xml(&opf_name)?
            .get_attr(item, "href")
            .unwrap_or("")
            .to_string();
        let name = container
            .href_to_name(&href, Some(&opf_name))
            .ok_or_else(|| anyhow::anyhow!("failed to resolve cover item name"))?;
        let data = fs::read(cover_path)
            .with_context(|| format!("Failed to read cover image {cover_path}"))?;
        container.write_file(&name, &data)?;
        (item, name)
    };

    let (keep_aspect, no_svg) = options
        .map(|o| (o.keep_aspect, o.no_svg))
        .unwrap_or((false, false));

    let (templ, has_svg) = if no_svg {
        (
            NONSVG_TEMPLATE.replace("__style__", "style=\"height: 100%\""),
            false,
        )
    } else {
        let mut width: i64 = 600;
        let mut height: i64 = 800;
        let bytes = if let Some(existing) = existing_image {
            container.raw_data(existing, false).ok()
        } else {
            fs::read(cover_path).ok()
        };
        if let Some(bytes) = bytes {
            let (_, w, h) = calibre_utils::imghdr::identify(&bytes);
            width = w;
            height = h;
        }
        let ar = if keep_aspect { "xMidYMid meet" } else { "none" };
        let templ = SVG_TEMPLATE
            .replace("__ar__", ar)
            .replace("__viewbox__", &format!("0 0 {width} {height}"))
            .replace("__width__", &width.to_string())
            .replace("__height__", &height.to_string());
        (templ, true)
    };

    if let Some(folder) = recommended.get(&tname) {
        if !folder.is_empty() {
            tname = format!("{folder}/{tname}");
        }
    }
    let titlepage_item = container.generate_item(&tname, "titlepage", None, true)?;
    let opf_name = container.opf_name.clone();
    let titlepage_href = container
        .get_xml(&opf_name)?
        .get_attr(titlepage_item, "href")
        .unwrap_or("")
        .to_string();
    let titlepage = container
        .href_to_name(&titlepage_href, Some(&opf_name))
        .ok_or_else(|| anyhow::anyhow!("failed to resolve titlepage item name"))?;
    let href_for_template = container.name_to_href(&raster_cover, Some(&titlepage));
    let raw = percent_format(&templ, &href_for_template);
    container.write_file(&titlepage, raw.as_bytes())?;

    // We have to make sure the raster cover item has id="cover" for the
    // moron that wrote the Nook firmware.
    let cover_item_id = {
        let xml = container.get_xml(&opf_name)?;
        xml.get_attr(raster_cover_item, "id")
            .unwrap_or("")
            .to_string()
    };
    if cover_item_id != "cover" {
        let newid = toc::uuid_id();
        let all_elems = container.opf_xpath("//*")?;
        let mut fix_id = Vec::new();
        let mut fix_idref = Vec::new();
        {
            let xml = container.get_xml(&opf_name)?;
            for &e in &all_elems {
                if xml.get_attr(e, "id") == Some("cover") {
                    fix_id.push(e);
                }
                if xml.get_attr(e, "idref") == Some("cover") {
                    fix_idref.push(e);
                }
            }
        }
        let xml = container.get_xml_mut(&opf_name)?;
        for e in fix_id {
            xml.set_attr(e, "id", newid.clone());
        }
        for e in fix_idref {
            xml.set_attr(e, "idref", newid.clone());
        }
        xml.set_attr(raster_cover_item, "id", "cover");
    }

    let spine_node = container
        .opf_xpath("//opf:spine")?
        .first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("OPF has no <spine>"))?;
    let titlepage_id = {
        let xml = container.get_xml(&opf_name)?;
        xml.get_attr(titlepage_item, "id").unwrap_or("").to_string()
    };
    {
        let xml = container.get_xml_mut(&opf_name)?;
        let itemref = xml.new_element("itemref", Some(OPF2_NS));
        xml.set_attr(itemref, "idref", titlepage_id);
        xml.insert_element(spine_node, itemref, Some(0));
    }

    let (major, _minor) = container.opf_version_parsed()?;
    if major < 3 {
        let guide = container.opf_get_or_create("guide")?;
        let href = container.name_to_href(&titlepage, Some(&opf_name));
        {
            let xml = container.get_xml_mut(&opf_name)?;
            let r = xml.new_element("reference", Some(OPF2_NS));
            xml.set_attr(r, "type", "cover");
            xml.set_attr(r, "title", "Cover");
            xml.set_attr(r, "href", href);
            xml.insert_element(guide, r, None);
        }
        let metadata = container.opf_get_or_create("metadata")?;
        let xml = container.get_xml_mut(&opf_name)?;
        let meta = xml.new_element("meta", Some(OPF2_NS));
        xml.set_attr(meta, "name", "cover");
        xml.set_attr(meta, "content", "cover");
        xml.insert_element(metadata, meta, None);
    } else {
        container.apply_unique_properties(Some(&raster_cover), &["cover-image"])?;
        container.apply_unique_properties(Some(&titlepage), &["calibre:title-page"])?;
        if has_svg {
            container.add_properties(&titlepage, &["svg"])?;
        }
    }

    Ok((raster_cover, titlepage))
}

/// Port of `remove_cover_image_in_page`.
pub fn remove_cover_image_in_page(
    container: &mut Container,
    page: &str,
    cover_images: &HashSet<String>,
) -> Result<()> {
    container.ensure_parsed(page)?;
    let first_img = {
        let dom = container.get_xhtml(page)?;
        dom.preorder_elements(dom.root)
            .into_iter()
            .find(|&e| dom.tag(e) == Some("img") && dom.node(e).attrs.contains_key("src"))
    };
    let Some(img) = first_img else {
        return Ok(());
    };
    let href = {
        let dom = container.get_xhtml(page)?;
        dom.node(img).attrs.get("src").cloned()
    };
    let Some(href) = href else {
        return Ok(());
    };
    let Some(name) = container.href_to_name(&href, Some(page)) else {
        return Ok(());
    };
    if cover_images.contains(&name) {
        // Python's `remove_cover_image_in_page` mutates the parsed tree
        // in place without ever calling `container.dirty(page)` -- kept
        // faithfully here too (the caller, `set_epub_cover`, immediately
        // goes on to remove/replace the surrounding pages in every real
        // path that reaches this, so the omission is inert in practice).
        let dom = container.get_xhtml_mut(page)?;
        dom.detach(img);
    }
    Ok(())
}

/// Port of `has_epub_cover`.
pub fn has_epub_cover(container: &mut Container) -> Result<bool> {
    if find_cover_image(container, false)?.is_some() {
        return Ok(true);
    }
    if find_cover_page(container)?.is_some() {
        return Ok(true);
    }
    let spine = spine_item_names(container)?;
    if let Some(first) = spine.first() {
        if find_cover_image_in_page(container, first)?.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Port of `set_epub_cover`.
pub fn set_epub_cover(
    container: &mut Container,
    cover_path: &str,
    report: &mut dyn FnMut(&str),
    options: Option<&CoverOptions>,
    mut image_callback: Option<ImageCallback<'_>>,
) -> Result<(String, String)> {
    let existing_image_flag = options.map(|o| o.existing_image).unwrap_or(false);
    let existing_image: Option<String> = if existing_image_flag {
        Some(cover_path.to_string())
    } else {
        None
    };
    let cover_image = find_cover_image(container, false)?;
    let mut cover_page = find_cover_page(container)?;
    let mut wrapped_image: Option<String> = None;
    let mut extra_cover_page: Option<String> = None;
    let mut updated = false;

    // `clean_opf`'s return value (the names it may have orphaned) is
    // computed but, matching Python's own inline `TODO`, not acted on:
    //
    //   TODO: Handle possible_removals and also iterate over links in
    //   the removed pages and handle possibly removing stylesheets
    //   referred to by them.
    let _possible_removals: HashSet<String> = clean_opf(container)?.into_iter().collect();

    let mut image_callback_called = false;
    let mut spine_items = spine_item_names(container)?;
    if cover_page.is_none() {
        if let Some(first) = spine_items.first() {
            if find_cover_image_in_page(container, first)?.is_some() {
                cover_page = Some(first.clone());
            }
        }
    }

    if let Some(cp) = cover_page.clone() {
        wrapped_image = find_cover_image_in_page(container, &cp)?;

        if spine_items.len() > 1 {
            let c = spine_items[1].clone();
            if c != cp {
                let candidate = find_cover_image_in_page(container, &c)?;
                let matches_existing = candidate.as_deref().is_some_and(|cand| {
                    Some(cand) == wrapped_image.as_deref() || Some(cand) == cover_image.as_deref()
                });
                if matches_existing {
                    container.remove_item(&c, true)?;
                    extra_cover_page = Some(c);
                    spine_items.remove(1);
                } else if candidate.is_none() {
                    let mut targets = HashSet::new();
                    if let Some(w) = &wrapped_image {
                        targets.insert(w.clone());
                    }
                    if let Some(ci) = &cover_image {
                        targets.insert(ci.clone());
                    }
                    remove_cover_image_in_page(container, &c, &targets)?;
                }
            }
        }

        if let Some(wi) = wrapped_image.clone() {
            container.remove_item(&cp, true)?;
            if Some(wi.as_str()) != existing_image.as_deref() {
                if !image_callback_called {
                    if let Some(cb) = image_callback.as_mut() {
                        cb(cover_image.as_deref(), wrapped_image.as_deref());
                    }
                    image_callback_called = true;
                }
                container.remove_item(&wi, true)?;
            }
            updated = true;
        }
    }

    if !image_callback_called {
        if let Some(cb) = image_callback.as_mut() {
            cb(cover_image.as_deref(), wrapped_image.as_deref());
        }
    }
    if let Some(ci) = &cover_image {
        if Some(ci.as_str()) != wrapped_image.as_deref()
            && Some(ci.as_str()) != existing_image.as_deref()
        {
            container.remove_item(ci, true)?;
        }
    }

    let (raster_cover, titlepage) =
        create_epub_cover(container, cover_path, existing_image.as_deref(), options)?;

    report(if updated {
        "Cover updated"
    } else {
        "Cover inserted"
    });

    // Match Python's "build one dict (later keys overwrite earlier ones
    // for the same source name), then filter out no-op/None entries"
    // two-phase shape exactly -- see this function's module docs.
    let mut raw_subs: HashMap<String, String> = HashMap::new();
    for (s, d) in [
        (cover_page.clone(), titlepage.clone()),
        (wrapped_image.clone(), raster_cover.clone()),
        (cover_image.clone(), raster_cover.clone()),
        (extra_cover_page.clone(), titlepage.clone()),
    ] {
        if let Some(s) = s {
            raw_subs.insert(s, d);
        }
    }
    let link_sub: HashMap<String, String> = raw_subs.into_iter().filter(|(s, d)| s != d).collect();
    if !link_sub.is_empty() {
        replace::replace_links(container, &link_sub, &|_n, _f| String::new(), false)?;
    }

    Ok((raster_cover, titlepage))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const TINY_PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, // signature
        0, 0, 0, 13, b'I', b'H', b'D', b'R', // IHDR chunk, length 13
        0, 0, 0, 2, 0, 0, 0, 2, // 2x2
        8, 6, 0, 0, 0, // bit depth/color type/compression/filter/interlace
        0, 0, 0, 0, // crc (unused by our sniffing)
    ];

    fn write_v2_epub(dir: &Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
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
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1>Chapter One</h1></body></html>",
        )
        .unwrap();
    }

    fn write_v3_epub(dir: &Path) {
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book V3</dc:title>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
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
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1>Chapter One</h1></body></html>",
        )
        .unwrap();
    }

    #[test]
    fn set_epub_cover_v2_creates_titlepage_and_marks_cover() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        let cover_path = dir.path().join("src-cover.png");
        fs::write(&cover_path, TINY_PNG).unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();

        let mut messages = Vec::new();
        let mut report = |m: &str| messages.push(m.to_string());
        set_cover(
            &mut c,
            cover_path.to_str().unwrap(),
            Some(&mut report),
            None,
        )
        .unwrap();
        assert_eq!(messages, vec!["Cover inserted"]);

        let cover_image = find_cover_image(&mut c, true).unwrap();
        assert!(cover_image.is_some());
        assert!(has_epub_cover(&mut c).unwrap());
        let cover_page = find_cover_page(&mut c).unwrap();
        assert!(cover_page.is_some());

        c.commit(false).unwrap();
        // Round-trip: re-open and confirm the cover survives a fresh parse.
        let mut c2 = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(has_epub_cover(&mut c2).unwrap());
        assert!(find_cover_image(&mut c2, true).unwrap().is_some());
    }

    #[test]
    fn set_epub_cover_v3_uses_cover_image_property() {
        let dir = tempfile::tempdir().unwrap();
        write_v3_epub(dir.path());
        let cover_path = dir.path().join("src-cover.png");
        fs::write(&cover_path, TINY_PNG).unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        set_cover(&mut c, cover_path.to_str().unwrap(), None, None).unwrap();

        let cover_image = find_cover_image3(&mut c).unwrap();
        assert!(cover_image.is_some());
        let props = c.manifest_items_with_property("cover-image").unwrap();
        assert_eq!(props, vec![cover_image.unwrap()]);
    }

    #[test]
    fn mark_as_cover_rejects_non_raster_and_missing() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(mark_as_cover(&mut c, "does-not-exist.png").is_err());
        assert!(mark_as_cover(&mut c, "chap1.html").is_err());
    }

    #[test]
    fn mark_as_cover_epub_marks_existing_manifest_image() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.add_file("existing.png", TINY_PNG, Some("image/png"), None, false)
            .unwrap();
        mark_as_cover(&mut c, "existing.png").unwrap();
        assert_eq!(
            find_cover_image(&mut c, true).unwrap().as_deref(),
            Some("existing.png")
        );
    }

    #[test]
    fn find_cover_image_in_page_detects_simple_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        fs::write(
            dir.path().join("wrap.html"),
            b"<html><body><div><img src=\"cover.png\"/></div></body></html>",
        )
        .unwrap();
        fs::write(dir.path().join("cover.png"), TINY_PNG).unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let img = find_cover_image_in_page(&mut c, "wrap.html").unwrap();
        assert_eq!(img.as_deref(), Some("cover.png"));
    }

    #[test]
    fn find_cover_image_in_page_returns_none_with_extra_text() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        fs::write(
            dir.path().join("wrap.html"),
            b"<html><body><div><img src=\"cover.png\"/>some text</div></body></html>",
        )
        .unwrap();
        fs::write(dir.path().join("cover.png"), TINY_PNG).unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let img = find_cover_image_in_page(&mut c, "wrap.html").unwrap();
        assert!(img.is_none());
    }

    #[test]
    fn percent_format_substitutes_href_and_unescapes_percent() {
        let out = percent_format("width=\"100%%\" href=\"%s\"", "img.png");
        assert_eq!(out, "width=\"100%\" href=\"img.png\"");
    }

    #[test]
    fn clean_opf_removes_cover_metadata_and_guide_ref() {
        let dir = tempfile::tempdir().unwrap();
        write_v2_epub(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.add_file("cover.png", TINY_PNG, Some("image/png"), None, false)
            .unwrap();
        mark_as_cover(&mut c, "cover.png").unwrap();
        let removed = clean_opf(&mut c).unwrap();
        // `mark_as_cover_epub` (OPF2) records the cover both as a
        // `<meta name="cover">` and a `<guide><reference type="cover">`;
        // `clean_opf` yields the resolved name once per removal source,
        // so the same name legitimately appears twice here -- matching
        // Python's un-deduplicated generator exactly.
        assert_eq!(
            removed,
            vec!["cover.png".to_string(), "cover.png".to_string()]
        );
        assert!(find_cover_image(&mut c, true).unwrap().is_none());
    }
}
