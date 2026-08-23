//! Port of `old_src/src/calibre/ebooks/oeb/polish/upgrade.py`:
//! `epub_2_to_3`/`upgrade_book`, which upgrade an EPUB2 book (NCX-only
//! TOC, no nav doc, OPF2-shaped metadata) to EPUB3 shape.
//!
//! # `opf_2_to_3.upgrade_metadata` and `parse_utils.ensure_namespace_prefixes`
//!
//! Neither is ported elsewhere in this crate. Per this project's
//! established boundary (`opf.py` needing only one function out of
//! `writer8/exth.py` for issue #34; `manifest_items_with_property`
//! simplifying away `opf3.py`'s custom-prefix remapping), this file
//! implements the narrow slices these two functions actually need
//! rather than full ports of `opf3.py`/`opf_2_to_3.py`/`parse_utils.py`:
//!
//! - [`ensure_namespace_prefix_epub`] replaces
//!   `ensure_namespace_prefixes(root, {'epub': EPUB_NS})`: this crate's
//!   HTML5-tag-soup `Dom` (see `crate::dom`'s module docs) has no XML
//!   namespace-declaration concept at all, so "ensure the `epub:`
//!   prefix is declared" reduces to "ensure an `xmlns:epub` attribute is
//!   present on the document's root element".
//! - [`upgrade_metadata`] ports `opf_2_to_3.py`'s `upgrade_metadata`
//!   pipeline directly against this crate's [`crate::xmltree::Xml`]
//!   arena, with a deliberately bounded reimplementation of the small
//!   slice of `opf3.py` it depends on (`ensure_id`/`set_refines`/a
//!   calibre-prefix-only `ensure_prefix` -- see each helper's docs).
//!   **Not ported** (each individually a large, separate subsystem, and
//!   none of them structurally required for a book to be valid EPUB3):
//!   - `upgrade_custom` (custom-column user-metadata round-tripping --
//!     `read_user_metadata2`/`set_user_metadata3`/`encode_is_multiple`,
//!     a calibre-specific serialization format, not an EPUB3 shape
//!     requirement).
//!   - `set_last_modified` -- **not a gap**: `EpubContainer::commit`
//!     already calls the equivalent
//!     [`super::container::EpubContainer::update_modified_timestamp`]
//!     unconditionally for every version-3 book on commit (issue #161),
//!     so `epub_2_to_3` setting `version="3.0"` is sufficient to get a
//!     `dcterms:modified` meta for free without this file re-deriving
//!     it.
//!   - `upgrade_timestamp`'s `create_timestamp` half: the stale
//!     `calibre:timestamp` `<meta name>` is still removed (the
//!     behaviorally important half -- an EPUB3 reader must not see a
//!     dangling OPF2-only meta), but no OPF3-shaped replacement is
//!     written; the "date added to library" it recorded has no EPUB3
//!     metadata field of its own and is redundant with `dc:date` in
//!     practice.
//!   - `pretty_print_opf` (cosmetic re-indentation only; this crate's
//!     `Xml::serialize` already produces well-formed, readable output,
//!     just not byte-identical to `lxml`'s -- the same convention
//!     `xmltree`/`pretty` document elsewhere in this crate).

use std::collections::HashSet;

use anyhow::Result;

use crate::oeb::constants::{DC11_NS, EPUB_NS, OEB_DOCS, OPF2_NS};
use crate::oeb::polish::opf::get_book_language;
use crate::oeb::polish::toc::{
    commit_nav_toc, find_existing_ncx_toc, get_landmarks, get_toc, Landmark, Toc,
};
use crate::oeb::polish::utils::OEB_FONTS;
use calibre_utils::short_uuid::uuid4;

use super::container::{
    opf_namespaces, Container, EpubContainer, ADOBE_OBFUSCATION, IDPF_OBFUSCATION,
};
use crate::xmltree::{Xml, XmlNodeId};

const CALIBRE_PREFIX: &str = "https://calibre-ebook.com";

/// Port of `add_properties`: unions `props` into `item`'s `properties`
/// attribute (space-separated, sorted, deduplicated).
pub fn add_properties(xml: &mut Xml, item: XmlNodeId, props: &[&str]) {
    let mut existing: HashSet<String> = xml
        .get_attr(item, "properties")
        .unwrap_or("")
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    existing.extend(props.iter().map(|s| s.to_string()));
    let mut v: Vec<String> = existing.into_iter().collect();
    v.sort();
    xml.set_attr(item, "properties", v.join(" "));
}

/// Port of `fix_font_mime_types`.
pub fn fix_font_mime_types(container: &mut Container) -> Result<bool> {
    let opf_name = container.opf_name.clone();
    let items = container.opf_xpath("//opf:manifest/opf:item[@href and @media-type]")?;
    let mut candidates = Vec::new();
    {
        let xml = container.get_xml(&opf_name)?;
        for &item in &items {
            let mt = xml
                .get_attr(item, "media-type")
                .unwrap_or("")
                .to_lowercase();
            if OEB_FONTS.iter().any(|f| f.eq_ignore_ascii_case(&mt)) {
                if let Some(href) = xml.get_attr(item, "href") {
                    candidates.push((item, href.to_string()));
                }
            }
        }
    }
    let mut changed = false;
    for (item, href) in candidates {
        if let Some(name) = container.href_to_name(&href, Some(&opf_name)) {
            let mt = container.guess_type(&name);
            let xml = container.get_xml_mut(&opf_name)?;
            xml.set_attr(item, "media-type", mt);
            changed = true;
        }
    }
    Ok(changed)
}

/// Port of `migrate_obfuscated_fonts`. Re-obfuscates any font this
/// container deobfuscated on open (see
/// [`EpubContainer::process_encryption`]) using the IDPF algorithm
/// (which needs no book-specific key beyond the unique identifier, so
/// it survives edits that don't touch the identifier), rewriting
/// `META-INF/encryption.xml` to match.
pub fn migrate_obfuscated_fonts(container: &mut EpubContainer) -> Result<()> {
    if container.obfuscated_fonts.is_empty() {
        return Ok(());
    }
    // `(EncryptionMethod node, CipherReference node)` per obfuscated
    // font name -- Python's `iter_encryption_entries` returns the
    // CipherReference *element* (`cr`) so its `URI` can be rewritten in
    // place; this crate's `EpubContainer::iter_encryption_entries`
    // returns just the resolved URI *string* (real for its one existing
    // caller, `process_encryption`, which never needs to write it back),
    // so this walks `META-INF/encryption.xml` directly to also keep the
    // CipherReference node id.
    let mut name_to_nodes: std::collections::HashMap<String, (XmlNodeId, XmlNodeId)> =
        std::collections::HashMap::new();
    if container
        .name_path_map
        .contains_key("META-INF/encryption.xml")
    {
        container.ensure_parsed("META-INF/encryption.xml")?;
        let empty_ns = std::collections::HashMap::new();
        let ems = {
            let xml = container.get_xml("META-INF/encryption.xml")?;
            xml.opf_xpath("//*[@Algorithm]", &empty_ns)
        };
        for em in ems {
            let (alg, cr) = {
                let xml = container.get_xml("META-INF/encryption.xml")?;
                let alg = xml.get_attr(em, "Algorithm").unwrap_or("").to_string();
                let cr = xml.parent(em).and_then(|parent| {
                    xml.children(parent)
                        .iter()
                        .find(|&&c| xml.has_attr(c, "URI"))
                        .copied()
                });
                (alg, cr)
            };
            let Some(cr) = cr else { continue };
            if alg != ADOBE_OBFUSCATION && alg != IDPF_OBFUSCATION {
                continue;
            }
            let uri = {
                let xml = container.get_xml("META-INF/encryption.xml")?;
                xml.get_attr(cr, "URI").unwrap_or("").to_string()
            };
            if let Some(name) = container.href_to_name(&uri, None) {
                name_to_nodes.insert(name, (em, cr));
            }
        }
    }

    let (package_id, _raw, idpf_key) = container.read_raw_unique_identifier()?;
    let idpf_key = if idpf_key.is_none() {
        if package_id.is_none() {
            let pid = uuid4();
            let root = container.opf_root()?;
            let opf_name = container.opf_name.clone();
            container
                .get_xml_mut(&opf_name)?
                .set_attr(root, "unique-identifier", pid);
        }
        let opf_name = container.opf_name.clone();
        let metadata_node = {
            let ns = opf_namespaces();
            let xml = container.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:metadata", &ns).first().copied()
        };
        if let Some(metadata) = metadata_node {
            let xml = container.get_xml_mut(&opf_name)?;
            let ident = xml.new_element("identifier", Some(DC11_NS));
            xml.set_element_text(ident, uuid4());
            xml.insert_element(metadata, ident, None);
        }
        let (_pid2, _raw2, key2) = container.read_raw_unique_identifier()?;
        key2
    } else {
        idpf_key
    };

    for name in container
        .obfuscated_fonts
        .keys()
        .cloned()
        .collect::<Vec<_>>()
    {
        let Some(&(em, cr)) = name_to_nodes.get(&name) else {
            container.obfuscated_fonts.remove(&name);
            continue;
        };
        let new_href = container.name_to_href(&name, None);
        let xml = container.get_xml_mut("META-INF/encryption.xml")?;
        xml.set_attr(em, "Algorithm", IDPF_OBFUSCATION);
        xml.set_attr(cr, "URI", new_href);
        if let Some(key) = idpf_key.clone() {
            container
                .obfuscated_fonts
                .insert(name, (IDPF_OBFUSCATION.to_string(), key));
        }
    }
    container.commit_item("META-INF/encryption.xml", false)
}

/// Port of `ensure_namespace_prefixes(root, {'epub': EPUB_NS})`. See the
/// module docs for why this reduces to "ensure `xmlns:epub` is present
/// on the root element" for this crate's HTML5-tag-soup `Dom`. Returns
/// whether the attribute was newly added (callers use this to decide
/// whether to mark the document dirty, rather than Python's
/// unconditional dirty-on-every-call -- see [`collect_properties`]'s
/// docs for that divergence).
pub fn ensure_namespace_prefix_epub(dom: &mut crate::dom::Dom) -> bool {
    let root = dom.root;
    let html = dom
        .children(root)
        .into_iter()
        .find(|&c| dom.tag(c).is_some())
        .unwrap_or(root);
    if dom.node(html).attrs.get("xmlns:epub").map(|s| s.as_str()) == Some(EPUB_NS) {
        return false;
    }
    dom.node_mut(html)
        .attrs
        .insert("xmlns:epub".to_string(), EPUB_NS.to_string());
    true
}

/// Port of `collect_properties`. Unlike Python (whose
/// `container.replace(name, root)` -- "Ensure entities are converted" --
/// unconditionally dirties every content document regardless of whether
/// anything actually changed), this only dirties a document when
/// `ensure_namespace_prefix_epub` or a detected property actually
/// changed it: this crate decodes entities once at parse time (see
/// `container.rs`'s `ContainerBase::parse_xhtml` docs), so there is no
/// "re-encode entities" side effect to force through a write.
pub fn collect_properties(container: &mut Container) -> Result<()> {
    let opf_name = container.opf_name.clone();
    let items = container.opf_xpath("//opf:manifest/opf:item[@href and @media-type]")?;
    let mut candidates = Vec::new();
    {
        let xml = container.get_xml(&opf_name)?;
        for &item in &items {
            let mt = xml
                .get_attr(item, "media-type")
                .unwrap_or("")
                .to_lowercase();
            if !OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(&mt)) {
                continue;
            }
            if let Some(href) = xml.get_attr(item, "href") {
                candidates.push((item, href.to_string()));
            }
        }
    }
    for (item, href) in candidates {
        let Some(name) = container.href_to_name(&href, Some(&opf_name)) else {
            continue;
        };
        if container.ensure_parsed(&name).is_err() {
            continue;
        }
        let mut doc_changed = false;
        {
            let dom = container.get_xhtml_mut(&name)?;
            if ensure_namespace_prefix_epub(dom) {
                doc_changed = true;
            }
        }
        let dom = container.get_xhtml(&name)?;
        let mut properties: Vec<&str> = Vec::new();
        if dom.find_first_tag_global("svg").is_some() {
            properties.push("svg");
        }
        if dom.find_first_tag_global("script").is_some() {
            properties.push("scripted");
        }
        if dom.find_first_tag_global("math").is_some() {
            properties.push("mathml");
        }
        if dom.find_first_tag_global("epub:switch").is_some() {
            properties.push("switch");
        }
        if doc_changed {
            container.dirty(&name);
        }
        if !properties.is_empty() {
            let xml = container.get_xml_mut(&opf_name)?;
            add_properties(xml, item, &properties);
            container.dirty(&opf_name);
        }
    }
    Ok(())
}

/// Port of `guide_epubtype_map`.
pub fn guide_epubtype_map(guide_type: &str) -> Option<&'static str> {
    let key = guide_type.to_lowercase();
    Some(match key.as_str() {
        "acknowledgements" => "acknowledgments",
        "other.afterword" => "afterword",
        "other.appendix" => "appendix",
        "other.backmatter" => "backmatter",
        "bibliography" => "bibliography",
        "text" => "bodymatter",
        "other.chapter" => "chapter",
        "colophon" => "colophon",
        "other.conclusion" => "conclusion",
        "other.contributors" => "contributors",
        "copyright-page" => "copyright-page",
        "cover" => "cover",
        "dedication" => "dedication",
        "other.division" => "division",
        "epigraph" => "epigraph",
        "other.epilogue" => "epilogue",
        "other.errata" => "errata",
        "other.footnotes" => "footnotes",
        "foreword" => "foreword",
        "other.frontmatter" => "frontmatter",
        "glossary" => "glossary",
        "other.halftitlepage" => "halftitlepage",
        "other.imprint" => "imprint",
        "other.imprimatur" => "imprimatur",
        "index" => "index",
        "other.introduction" => "introduction",
        "other.landmarks" => "landmarks",
        "other.loa" => "loa",
        "loi" => "loi",
        "lot" => "lot",
        "other.lov" => "lov",
        "notes" => "",
        "other.notice" => "notice",
        "other.other-credits" => "other-credits",
        "other.part" => "part",
        "other.preamble" => "preamble",
        "preface" => "preface",
        "other.prologue" => "prologue",
        "other.rearnotes" => "rearnotes",
        "other.subchapter" => "subchapter",
        "title-page" => "titlepage",
        "toc" => "toc",
        "other.volume" => "volume",
        "other.warning" => "warning",
        _ => return None,
    })
}

/// Port of `create_nav`.
pub fn create_nav(
    container: &mut Container,
    toc: &Toc,
    mut landmarks: Vec<Landmark>,
    previous_nav: Option<(String, crate::dom::Dom)>,
) -> Result<()> {
    let mut lang = get_book_language(container)?;
    if lang.as_deref() == Some("und") {
        lang = None;
    }
    for entry in &mut landmarks {
        let mapped = guide_epubtype_map(&entry.r#type).unwrap_or("").to_string();
        let is_cover = mapped == "cover";
        entry.r#type = mapped;
        if is_cover {
            let is_doc = container
                .base
                .mime_map
                .get(&entry.dest)
                .map(|mt| OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(mt)))
                .unwrap_or(false);
            if is_doc {
                let dest = entry.dest.clone();
                container.apply_unique_properties(Some(&dest), &["calibre:title-page"])?;
            }
        }
    }
    commit_nav_toc(
        container,
        toc,
        lang.as_deref(),
        Some(&landmarks),
        previous_nav,
    )
}

/// Port of `epub_2_to_3`.
pub fn epub_2_to_3(
    container: &mut EpubContainer,
    mut report: impl FnMut(&str),
    previous_nav: Option<(String, crate::dom::Dom)>,
    remove_ncx: bool,
) -> Result<()> {
    {
        let opf_name = container.opf_name.clone();
        let root = container.opf_root()?;
        let xml = container.get_xml_mut(&opf_name)?;
        upgrade_metadata(xml, root);
    }
    collect_properties(container)?;
    let toc = get_toc(container, false)?;
    let toc_name = find_existing_ncx_toc(container)?;
    if let Some(toc_name) = &toc_name {
        if remove_ncx {
            container.remove_item(toc_name, true)?;
            let spines = container.opf_xpath("//opf:spine")?;
            if let Some(&spine) = spines.first() {
                let opf_name = container.opf_name.clone();
                container.get_xml_mut(&opf_name)?.remove_attr(spine, "toc");
            }
        }
    }
    let landmarks = get_landmarks(container)?;
    let guides = container.opf_xpath("//opf:guide")?;
    if !guides.is_empty() {
        let opf_name = container.opf_name.clone();
        let xml = container.get_xml_mut(&opf_name)?;
        for g in guides {
            xml.detach(g);
        }
        container.dirty(&opf_name);
    }
    create_nav(container, &toc, landmarks, previous_nav)?;
    {
        let opf_name = container.opf_name.clone();
        let root = container.opf_root()?;
        container
            .get_xml_mut(&opf_name)?
            .set_attr(root, "version", "3.0");
    }
    if fix_font_mime_types(container)? {
        container.refresh_mime_map()?;
    }
    migrate_obfuscated_fonts(container)?;
    let opf_name = container.opf_name.clone();
    container.dirty(&opf_name);
    report("");
    Ok(())
}

/// Port of `upgrade_book`.
pub fn upgrade_book(
    container: &mut EpubContainer,
    mut report: impl FnMut(&str),
    remove_ncx: bool,
) -> Result<bool> {
    let book_type = container.book_type();
    let (major, _) = container.opf_version_parsed()?;
    if (book_type != "epub" && book_type != "kepub") || major >= 3 {
        report("No upgrade needed");
        return Ok(false);
    }
    epub_2_to_3(container, &mut report, None, remove_ncx)?;
    report("Updated EPUB from version 2 to 3");
    Ok(true)
}

// ===================================================================
// `opf_2_to_3.upgrade_metadata` -- narrow, direct port. See the module
// docs for exactly what is/isn't included.
// ===================================================================

fn collect_ids(xml: &Xml, id: XmlNodeId, out: &mut HashSet<String>) {
    if let Some(v) = xml.get_attr(id, "id") {
        out.insert(v.to_string());
    }
    for &c in xml.children(id) {
        collect_ids(xml, c, out);
    }
}

/// Port of `ensure_unique('id', ...)` + `ensure_id`: returns `elem`'s
/// `id` attribute, generating and setting a fresh `id`/`id-1`/`id-2`/...
/// if absent.
fn ensure_id(xml: &mut Xml, elem: XmlNodeId) -> String {
    if let Some(id) = xml.get_attr(elem, "id") {
        if !id.is_empty() {
            return id.to_string();
        }
    }
    let mut existing = HashSet::new();
    collect_ids(xml, xml.root, &mut existing);
    let mut candidate = "id".to_string();
    let mut c = 0u32;
    while existing.contains(&candidate) {
        c += 1;
        candidate = format!("id-{c}");
    }
    xml.set_attr(elem, "id", candidate.clone());
    candidate
}

/// Narrow port of `opf3.ensure_prefix(root, prefixes, 'calibre',
/// CALIBRE_PREFIX)`: the only prefix `upgrade_metadata`'s dependencies
/// (`create_rating`/`create_series`) ever register. Full prefix-map
/// parsing/rewriting (`opf3.read_prefixes`/general `ensure_prefix`) is
/// out of scope -- see the module docs.
fn ensure_prefix_calibre(xml: &mut Xml, root: XmlNodeId) {
    let existing = xml.get_attr(root, "prefix").unwrap_or("").to_string();
    if existing
        .split_whitespace()
        .collect::<Vec<_>>()
        .chunks(2)
        .any(|c| c.first() == Some(&"calibre:") && c.get(1) == Some(&CALIBRE_PREFIX))
    {
        return;
    }
    let mut new_val = existing;
    if !new_val.is_empty() {
        new_val.push(' ');
    }
    new_val.push_str(&format!("calibre: {CALIBRE_PREFIX}"));
    xml.set_attr(root, "prefix", new_val);
}

/// Narrow port of `opf3.set_refines`: inserts one `<opf:meta
/// refines="#<id-of-elem>" property="..." [scheme="..."]>` per `refs`
/// entry, immediately after `elem`, in `refs` order. Assumes `elem` has
/// no pre-existing refines to remove first (true for every call site in
/// this file: `upgrade_metadata` only ever runs once, against an OPF2
/// document that cannot yet have OPF3 refines).
fn set_refines_after(xml: &mut Xml, elem: XmlNodeId, refs: &[(&str, String, Option<&str>)]) {
    let eid = ensure_id(xml, elem);
    let Some(parent) = xml.parent(elem) else {
        return;
    };
    let mut insert_index = xml
        .element_children(parent)
        .iter()
        .position(|&c| c == elem)
        .map(|i| i + 1);
    for (prop, val, scheme) in refs {
        let r = xml.new_element("meta", Some(OPF2_NS));
        xml.set_attr(r, "refines", format!("#{eid}"));
        xml.set_attr(r, "property", *prop);
        if let Some(s) = scheme {
            xml.set_attr(r, "scheme", *s);
        }
        xml.set_element_text(r, val.trim().to_string());
        xml.insert_element(parent, r, insert_index);
        insert_index = insert_index.map(|i| i + 1);
    }
}

/// Port of `upgrade_identifiers`.
fn upgrade_identifiers(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    for ident in xml.opf_xpath("//opf:metadata/dc:identifier", &ns) {
        let mut val = xml.element_text(ident).unwrap_or("").trim().to_string();
        let lval = val.to_lowercase();
        let mut scheme = xml.get_attr(ident, "scheme").map(|s| s.to_string());
        xml.remove_attr(ident, "scheme");
        if lval.starts_with("urn:") {
            if let Some((prefix, rest)) = val[4..].split_once(':') {
                if !prefix.is_empty() && !rest.is_empty() {
                    scheme = Some(prefix.to_string());
                    val = rest.to_string();
                }
            }
        }
        if let Some(s) = &scheme {
            if !val.is_empty() {
                xml.set_element_text(ident, format!("{s}:{val}"));
            }
        }
        let attrs: Vec<String> = xml.node(ident).attrs.keys().cloned().collect();
        for a in attrs {
            if a != "id" {
                xml.remove_attr(ident, &a);
            }
        }
    }
    let _ = root;
}

/// Port of `upgrade_title` (the refines-writing "title-type"/"file-as"
/// half; `data.refines` pre-scanning is unnecessary here -- see
/// `set_refines_after`'s docs).
fn upgrade_title(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    let mut first_title = None;
    for title in xml.opf_xpath("//opf:metadata/dc:title", &ns) {
        let has_text = xml
            .element_text(title)
            .map(|t| !t.trim().is_empty())
            .unwrap_or(false);
        if !has_text {
            xml.detach(title);
            continue;
        }
        if first_title.is_none() {
            first_title = Some(title);
        }
    }
    let mut title_sort = None;
    for m in xml.opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns) {
        if xml.get_attr(m, "name") == Some("calibre:title_sort") {
            if let Some(v) = xml.get_attr(m, "content") {
                title_sort = Some(v.to_string());
            }
            xml.detach(m);
        }
    }
    if let Some(title) = first_title {
        let mut refs = vec![("title-type", "main".to_string(), None)];
        if let Some(ts) = title_sort {
            refs.push(("file-as", ts, None));
        }
        set_refines_after(xml, title, &refs);
    }
    let _ = root;
}

/// Port of `upgrade_languages`.
fn upgrade_languages(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    let langs = xml.opf_xpath("//opf:metadata/dc:language", &ns);
    if !langs.is_empty() {
        for lang in langs {
            let attrs: Vec<String> = xml.node(lang).attrs.keys().cloned().collect();
            for a in attrs {
                xml.remove_attr(lang, &a);
            }
        }
        return;
    }
    let Some(&metadata) = xml.opf_xpath("//opf:metadata", &ns).first() else {
        return;
    };
    let l = xml.new_element("language", Some(DC11_NS));
    xml.set_element_text(l, "und");
    xml.insert_element(metadata, l, None);
    let _ = root;
}

/// Port of `upgrade_authors`.
fn upgrade_authors(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    for which in ["creator", "contributor"] {
        for elem in xml.opf_xpath(&format!("//opf:metadata/dc:{which}"), &ns) {
            let role = xml.get_attr(elem, "role").map(|s| s.to_string());
            let sort = xml.get_attr(elem, "file-as").map(|s| s.to_string());
            if role.is_none() && sort.is_none() {
                continue;
            }
            xml.remove_attr(elem, "role");
            xml.remove_attr(elem, "file-as");
            let mut refs = Vec::new();
            if let Some(r) = role {
                refs.push(("role", r, Some("marc:relators")));
            }
            if let Some(s) = sort {
                refs.push(("file-as", s, None));
            }
            set_refines_after(xml, elem, &refs);
        }
    }
    let _ = root;
}

/// Port of `upgrade_timestamp`'s removal half. See the module docs for
/// why the `create_timestamp` replacement is not written.
fn upgrade_timestamp(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    for meta in xml.opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns) {
        if xml.get_attr(meta, "name") == Some("calibre:timestamp") {
            xml.detach(meta);
        }
    }
    let _ = root;
}

/// Port of `upgrade_date`.
fn upgrade_date(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    let mut found = false;
    for date in xml.opf_xpath("//opf:metadata/dc:date", &ns) {
        let val = xml.element_text(date).map(|s| s.to_string());
        if val.as_deref().map(|v| v.is_empty()).unwrap_or(true) {
            xml.detach(date);
            continue;
        }
        if found {
            xml.detach(date);
        } else {
            found = true;
        }
    }
    let _ = root;
}

/// Port of `upgrade_rating` (removal + `create_rating`).
fn upgrade_rating(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    let mut rating = None;
    for meta in xml.opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns) {
        if xml.get_attr(meta, "name") == Some("calibre:rating") {
            if let Some(v) = xml.get_attr(meta, "content") {
                rating = Some(v.to_string());
            }
            xml.detach(meta);
        }
    }
    let Some(rating) = rating else { return };
    ensure_prefix_calibre(xml, root);
    let Some(&metadata) = xml.opf_xpath("//opf:metadata", &ns).first() else {
        return;
    };
    let d = xml.new_element("meta", Some(OPF2_NS));
    xml.set_attr(d, "property", "calibre:rating");
    xml.set_element_text(d, rating);
    xml.insert_element(metadata, d, None);
}

/// Port of `upgrade_series` (removal + `create_series`).
fn upgrade_series(xml: &mut Xml, _root: XmlNodeId) {
    let ns = opf_namespaces();
    let mut series = None;
    let mut series_index = "1.0".to_string();
    for meta in xml.opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns) {
        match xml.get_attr(meta, "name") {
            Some("calibre:series") => {
                if let Some(v) = xml.get_attr(meta, "content") {
                    series = Some(v.to_string());
                }
                xml.detach(meta);
            }
            Some("calibre:series_index") => {
                if let Some(v) = xml.get_attr(meta, "content") {
                    series_index = v.to_string();
                }
                xml.detach(meta);
            }
            _ => {}
        }
    }
    let Some(series) = series else { return };
    let Some(&metadata) = xml.opf_xpath("//opf:metadata", &ns).first() else {
        return;
    };
    let d = xml.new_element("meta", Some(OPF2_NS));
    xml.set_attr(d, "property", "belongs-to-collection");
    xml.set_element_text(d, series);
    xml.insert_element(metadata, d, None);
    set_refines_after(
        xml,
        d,
        &[
            ("collection-type", "series".to_string(), None),
            ("group-position", series_index, None),
        ],
    );
}

/// Port of `upgrade_meta` (the `rendition:*` reflow properties).
fn upgrade_meta(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    for meta in xml.opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns) {
        let Some(name) = xml.get_attr(meta, "name").map(|s| s.to_string()) else {
            continue;
        };
        let content = xml.get_attr(meta, "content").unwrap_or("").to_string();
        let name = name
            .strip_prefix("rendition:")
            .map(|s| s.to_string())
            .unwrap_or(name);
        let prop_and_content: Option<(String, String)> = match name.as_str() {
            "orientation" | "layout" | "spread" => Some((format!("rendition:{name}"), content)),
            "fixed-layout" => Some((
                "rendition:layout".to_string(),
                if content.eq_ignore_ascii_case("true") {
                    "pre-paginated".to_string()
                } else {
                    "reflowable".to_string()
                },
            )),
            "orientation-lock" => Some((
                "rendition:orientation".to_string(),
                match content.to_lowercase().as_str() {
                    "portrait" => "portrait".to_string(),
                    "landscape" => "landscape".to_string(),
                    _ => "auto".to_string(),
                },
            )),
            _ => None,
        };
        if let Some((prop, new_content)) = prop_and_content {
            xml.remove_attr(meta, "name");
            xml.remove_attr(meta, "content");
            xml.set_attr(meta, "property", prop);
            xml.set_element_text(meta, new_content);
        }
    }
    let _ = root;
}

/// Port of `upgrade_cover`.
fn upgrade_cover(xml: &mut Xml, root: XmlNodeId) {
    let ns = opf_namespaces();
    let cover_ids: Vec<String> = xml
        .opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns)
        .into_iter()
        .filter(|&m| xml.get_attr(m, "name") == Some("cover"))
        .filter_map(|m| xml.get_attr(m, "content").map(|s| s.to_string()))
        .collect();
    if cover_ids.is_empty() {
        return;
    }
    for item in xml.opf_xpath(
        "//opf:manifest/opf:item[@id and @href and @media-type]",
        &ns,
    ) {
        let Some(id) = xml.get_attr(item, "id") else {
            continue;
        };
        if !cover_ids.iter().any(|c| c == id) {
            continue;
        }
        let mt = xml
            .get_attr(item, "media-type")
            .unwrap_or("")
            .to_lowercase();
        if mt.is_empty() || mt.contains("xml") || mt.contains("html") {
            continue;
        }
        let mut props = xml.get_attr(item, "properties").unwrap_or("").to_string();
        if !props.split_whitespace().any(|p| p == "cover-image") {
            if !props.is_empty() {
                props.push(' ');
            }
            props.push_str("cover-image");
            xml.set_attr(item, "properties", props);
        }
    }
    let _ = root;
}

/// Port of `remove_invalid_attrs_in_dc_metadata`.
fn remove_invalid_attrs_in_dc_metadata(xml: &mut Xml, root: XmlNodeId) {
    fn walk(xml: &Xml, id: XmlNodeId, out: &mut Vec<XmlNodeId>) {
        if xml.namespace(id) == Some(DC11_NS) {
            out.push(id);
        }
        for c in xml.children(id).to_vec() {
            walk(xml, c, out);
        }
    }
    let mut dc_elems = Vec::new();
    walk(xml, root, &mut dc_elems);
    for elem in dc_elems {
        let attrs: Vec<String> = xml.node(elem).attrs.keys().cloned().collect();
        for a in attrs {
            if a != "id" {
                xml.remove_attr(elem, &a);
            }
        }
    }
}

/// Port of `opf_2_to_3.upgrade_metadata`. See the module docs for what
/// is/isn't included.
pub fn upgrade_metadata(xml: &mut Xml, root: XmlNodeId) {
    upgrade_identifiers(xml, root);
    upgrade_title(xml, root);
    upgrade_languages(xml, root);
    upgrade_authors(xml, root);
    upgrade_timestamp(xml, root);
    upgrade_date(xml, root);
    upgrade_rating(xml, root);
    upgrade_series(xml, root);
    upgrade_meta(xml, root);
    upgrade_cover(xml, root);
    remove_invalid_attrs_in_dc_metadata(xml, root);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_epub2_book(dir: &std::path::Path) {
        fs::create_dir_all(dir.join("META-INF")).unwrap();
        fs::write(
            dir.join("META-INF/container.xml"),
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Old Book</dc:title>
    <dc:creator opf:role="aut" opf:file-as="Doe, Jane">Jane Doe</dc:creator>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid" opf:scheme="calibre">abc123</dc:identifier>
    <meta name="calibre:rating" content="8"/>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="c1"/>
  </spine>
  <guide>
    <reference type="toc" title="Table of Contents" href="chap1.html"/>
  </guide>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><h1>Chapter 1</h1><p>Hello</p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("toc.ncx"),
            br#"<?xml version="1.0"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head><meta name="dtb:uid" content="abc123"/></head>
  <docTitle><text>Old Book</text></docTitle>
  <navMap>
    <navPoint id="np1" playOrder="1"><navLabel><text>Chapter 1</text></navLabel><content src="chap1.html"/></navPoint>
  </navMap>
</ncx>"#,
        )
        .unwrap();
    }

    #[test]
    fn upgrade_metadata_reshapes_identifiers_language_and_rating() {
        let dir = tempfile::tempdir().unwrap();
        write_epub2_book(dir.path());
        let mut xml = crate::xmltree::Xml::parse(
            &fs::read_to_string(dir.path().join("content.opf")).unwrap(),
        )
        .unwrap();
        let root = xml.root_element().unwrap();
        upgrade_metadata(&mut xml, root);
        let ns = opf_namespaces();
        let idents = xml.opf_xpath("//opf:metadata/dc:identifier", &ns);
        assert_eq!(xml.element_text(idents[0]), Some("calibre:abc123"));
        // Rating meta was removed and re-created as a `property`-based
        // OPF3 meta with the calibre prefix declared.
        assert!(xml
            .opf_xpath(r#"//opf:metadata/opf:meta[@name]"#, &ns)
            .into_iter()
            .all(|m| xml.get_attr(m, "name") != Some("calibre:rating")));
        let rating_meta = xml
            .opf_xpath(r#"//opf:metadata/opf:meta[@property]"#, &ns)
            .into_iter()
            .find(|&m| xml.get_attr(m, "property") == Some("calibre:rating"));
        assert!(rating_meta.is_some());
        assert_eq!(
            xml.get_attr(root, "prefix"),
            Some("calibre: https://calibre-ebook.com")
        );
    }

    #[test]
    fn upgrade_book_converts_epub2_ncx_book_to_epub3_nav_book() {
        let src = tempfile::tempdir().unwrap();
        write_epub2_book(src.path());
        let tdir = tempfile::tempdir().unwrap();
        let mut epub = EpubContainer::open_dir(src.path(), tdir.path()).unwrap();
        let mut reports = Vec::new();
        let upgraded = upgrade_book(&mut epub, |m| reports.push(m.to_string()), true).unwrap();
        assert!(upgraded);
        assert_eq!(epub.opf_version_parsed().unwrap().0, 3);
        // NCX removed, replaced by a nav document referenced from the
        // manifest with properties="nav".
        assert!(!epub.exists("toc.ncx"));
        let nav_names = epub.manifest_items_with_property("nav").unwrap();
        assert_eq!(nav_names.len(), 1);
        assert!(epub.exists(&nav_names[0]));
    }

    #[test]
    fn upgrade_book_is_a_no_op_for_already_epub3_books() {
        let src = tempfile::tempdir().unwrap();
        write_epub2_book(src.path());
        let tdir = tempfile::tempdir().unwrap();
        let mut epub = EpubContainer::open_dir(src.path(), tdir.path()).unwrap();
        // Upgrade in-memory (no commit/reopen round trip needed) so the
        // container is already EPUB3-shaped for the second call.
        upgrade_book(&mut epub, |_| {}, true).unwrap();
        assert_eq!(epub.opf_version_parsed().unwrap().0, 3);

        let mut reports = Vec::new();
        let upgraded = upgrade_book(&mut epub, |m| reports.push(m.to_string()), true).unwrap();
        assert!(!upgraded);
        assert!(reports.iter().any(|m| m.contains("No upgrade needed")));
    }
}
