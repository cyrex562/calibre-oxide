//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/opf.py`.
//!
//! # Scope note: `book_type`
//!
//! Python's `container.book_type` is a dynamically-dispatched property:
//! whatever concrete subclass `container` actually is (`EpubContainer`,
//! `AZW3Container`, ...) answers it, regardless of what static type the
//! calling code declares. Rust has no equivalent -- [`Container::book_type`]
//! is a plain inherent method, and [`super::super::container::EpubContainer`]
//! *shadows* it rather than overriding a trait method, so a function
//! written against `&mut Container` would always see `"oeb"` even when
//! called (via `Deref` coercion) on a real `EpubContainer`. [`check_opf`]
//! therefore takes `book_type` as an explicit parameter -- the caller
//! (which does still know the concrete container type) passes it in --
//! rather than silently always answering `"oeb"`.

use std::collections::HashMap;

use anyhow::Result;

use crate::oeb::constants::{DC11_NS, OPF2_NS};
use crate::oeb::polish::toc::{find_existing_nav_toc, parse_nav};
use crate::oeb::polish::utils::guess_type;
use crate::oeb::polish::xmltree::XmlNodeId;
use crate::xml_util::prepare_string_for_xml as xml;

use super::super::container::Container;
use super::base::{CheckError, Level};

// ===================================================================
// Error constructors
// ===================================================================

pub fn missing_section(name: &str, section_name: &str) -> CheckError {
    CheckError::new(
        "MissingSection",
        format!("The <{section_name}> section is missing from the OPF"),
        name,
    )
    .with_help(xml(
        &format!(
            "The <{section_name}> section is required in the OPF file. You have to create one."
        ),
        false,
    ))
}

pub fn empty_id(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("EmptyID", "Empty id attributes are invalid", name)
        .at(lnum, None)
        .with_help(xml("Empty ID attributes are invalid in OPF files.", false))
}

pub fn incorrect_idref(name: &str, idref: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "IncorrectIdref",
        format!("idref=\"{idref}\" points to unknown id"),
        name,
    )
    .at(lnum, None)
    .with_help(xml(
        &format!("The idref=\"{idref}\" points to an id that does not exist in the OPF"),
        false,
    ))
}

pub fn incorrect_cover(name: &str, lnum: Option<u32>, cover: &str) -> CheckError {
    CheckError::new(
        "IncorrectCover",
        "The meta cover tag points to an non-existent item",
        name,
    )
    .at(lnum, None)
    .with_help(xml(
        &format!("The meta cover tag points to an item with id=\"{cover}\" which does not exist in the manifest"),
        false,
    ))
}

pub fn nook_cover(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "NookCover",
        "The meta cover tag has content before name",
        name,
    )
    .at(lnum, None)
    .with_help(
        "Some e-book readers such as the Nook fail to recognize covers if the content \
         attribute comes before the name attribute. For maximum compatibility move the name \
         attribute before the content attribute.",
    )
    .with_fix(
        "Move the name attribute before the content attribute",
        move |container| {
            let covers = container.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#)?;
            let opf_name = container.opf_name.clone();
            let xml_tree = container.opf_mut()?;
            for cover in &covers {
                if let Some(v) = xml_tree.get_attr(*cover, "content").map(|s| s.to_string()) {
                    xml_tree.remove_attr(*cover, "content");
                    xml_tree.set_attr(*cover, "content", v);
                }
            }
            container.dirty(&opf_name);
            Ok(true)
        },
    )
}

pub fn incorrect_toc(
    name: &str,
    lnum: Option<u32>,
    bad_idref: Option<&str>,
    bad_mimetype: Option<&str>,
) -> CheckError {
    let (msg, help) = if let Some(bad_idref) = bad_idref {
        (
            format!("The item identified as the Table of Contents ({bad_idref}) does not exist"),
            format!("There is no item with id=\"{bad_idref}\" in the manifest."),
        )
    } else {
        (
            format!(
                "The item identified as the Table of Contents has an incorrect media-type ({})",
                bad_mimetype.unwrap_or_default()
            ),
            format!(
                "The media type for the Table of Contents must be {}",
                guess_type("a.ncx")
            ),
        )
    };
    CheckError::new("IncorrectToc", msg, name)
        .at(lnum, None)
        .with_help(help)
}

pub fn no_href(name: &str, item_id: Option<&str>, lnum: Option<u32>) -> CheckError {
    let owned_item_id = item_id.map(|s| s.to_string());
    CheckError::new("NoHref", "Item in manifest has no href attribute", name)
        .at(lnum, None)
        .with_help(
            "This manifest entry has no href attribute. Either add the href attribute or \
             remove the entry.",
        )
        .with_fix("Remove this manifest entry", move |container| {
            let items = container.opf_xpath("//opf:manifest/opf:item")?;
            let opf_name = container.opf_name.clone();
            let mut changed = false;
            let xml_tree = container.opf()?;
            let mut to_remove = Vec::new();
            for item in &items {
                if xml_tree.get_attr(*item, "id").map(|s| s.to_string()) == owned_item_id {
                    to_remove.push(*item);
                }
            }
            for item in to_remove {
                container.remove_from_xml(&opf_name, item)?;
                changed = true;
            }
            if changed {
                container.dirty(&opf_name);
            }
            Ok(changed)
        })
}

pub fn missing_ncx_ref(name: &str, lnum: Option<u32>, ncx_id: &str) -> CheckError {
    let owned_ncx_id = ncx_id.to_string();
    CheckError::new(
        "MissingNCXRef",
        "Missing reference to the NCX Table of Contents",
        name,
    )
    .at(lnum, None)
    .with_help(
        "The <spine> tag has no reference to the NCX table of contents file. Without this \
         reference, the table of contents will not work in most readers. The reference should \
         look like <spine toc=\"id of manifest item for the ncx file\">.",
    )
    .with_fix("Add the reference to the NCX file", move |container| {
        let spines = container.opf_xpath("//opf:spine")?;
        let opf_name = container.opf_name.clone();
        let mut changed = false;
        let xml_tree = container.opf_mut()?;
        for spine in spines {
            if xml_tree.get_attr(spine, "toc").is_none() {
                xml_tree.set_attr(spine, "toc", owned_ncx_id.clone());
                changed = true;
            }
        }
        if changed {
            container.dirty(&opf_name);
        }
        Ok(changed)
    })
}

pub fn missing_nav(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("MissingNav", "Missing navigation document", name)
        .at(lnum, None)
        .with_help(
            "This book has no Navigation document. According to the EPUB 3 specification, a \
             navigation document is required. The Navigation document contains the Table of \
             Contents. Use the Table of Contents tool to add a Table of Contents to this book.",
        )
}

pub fn empty_nav(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("EmptyNav", "Missing ToC in navigation document", name)
        .at(lnum, None)
        .with_level(Level::Warn)
        .with_help(
            "The nav document for this book contains no table of contents, or an empty table \
             of contents. Use the Table of Contents tool to add a Table of Contents to this \
             book.",
        )
}

pub fn missing_href(name: &str, href: &str, lnum: Option<u32>) -> CheckError {
    let owned_href = href.to_string();
    CheckError::new(
        "MissingHref",
        format!("Item ({href}) in manifest is missing"),
        name,
    )
    .at(lnum, None)
    .with_help(
        "A file listed in the manifest is missing, you should either remove it from the \
             manifest or add the missing file to the book.",
    )
    .with_fix(
        format!("Remove the entry for {href} from the manifest"),
        move |container| {
            let items = container.opf_xpath("//opf:manifest/opf:item[@href]")?;
            let opf_name = container.opf_name.clone();
            let mut to_remove = Vec::new();
            {
                let xml_tree = container.opf()?;
                for item in &items {
                    if xml_tree.get_attr(*item, "href") == Some(owned_href.as_str()) {
                        to_remove.push(*item);
                    }
                }
            }
            for item in to_remove {
                container.remove_from_xml(&opf_name, item)?;
            }
            container.dirty(&opf_name);
            Ok(true)
        },
    )
}

pub fn non_linear_items(name: &str, locs: &[Option<u32>]) -> CheckError {
    let mut sorted = locs.to_vec();
    sorted.sort();
    let all_locations = sorted
        .iter()
        .map(|l| (name.to_string(), *l, None))
        .collect();
    CheckError::new("NonLinearItems", "Non-linear items in the spine", name)
        .with_level(Level::Warn)
        .with_locations(all_locations)
        .with_help(xml(
            "There are items marked as non-linear in the <spine>. These will be displayed in \
             random order by different e-book readers. Some will ignore the non-linear \
             attribute, some will display them at the end or the beginning of the book and \
             some will fail to display them at all. Instead of using non-linear items simply \
             place the items in the order you want them to be displayed.",
            false,
        ))
        .with_fix("Mark all non-linear items as linear", move |container| {
            let items = container.opf_xpath("//opf:spine/opf:itemref[@linear]")?;
            let opf_name = container.opf_name.clone();
            let xml_tree = container.opf_mut()?;
            for item in items {
                xml_tree.remove_attr(item, "linear");
            }
            container.dirty(&opf_name);
            Ok(true)
        })
}

pub fn duplicate_href(name: &str, eid: &str, locs: &[Option<u32>], for_spine: bool) -> CheckError {
    let mut sorted = locs.to_vec();
    sorted.sort();
    let all_locations = sorted
        .iter()
        .map(|l| (name.to_string(), *l, None))
        .collect();
    let loc_word = if for_spine { "spine" } else { "manifest" };
    let owned_eid = eid.to_string();
    let xpath = format!(
        "//opf:{}",
        if for_spine {
            "spine/opf:itemref[@idref]"
        } else {
            "manifest/opf:item[@href]"
        }
    );
    let attr = if for_spine { "idref" } else { "href" };
    CheckError::new(
        "DuplicateHref",
        format!("Duplicate item in {loc_word}: {eid}"),
        name,
    )
    .with_locations(all_locations)
    .with_help(format!(
        "The item {eid} is present more than once in the {loc_word} in {name}. This is \
             not allowed."
    ))
    .with_fix(
        "Remove all but the first duplicate item",
        move |container| {
            let candidates = container.opf_xpath(&xpath)?;
            let opf_name = container.opf_name.clone();
            let mut matching = Vec::new();
            {
                let xml_tree = container.opf()?;
                for item in &candidates {
                    if xml_tree.get_attr(*item, attr) == Some(owned_eid.as_str()) {
                        matching.push(*item);
                    }
                }
            }
            for item in matching.into_iter().skip(1) {
                container.remove_from_xml(&opf_name, item)?;
            }
            container.dirty(&opf_name);
            Ok(true)
        },
    )
}

pub fn multiple_covers(name: &str, locs: &[Option<u32>]) -> CheckError {
    let mut sorted = locs.to_vec();
    sorted.sort();
    let all_locations = sorted
        .iter()
        .map(|l| (name.to_string(), *l, None))
        .collect();
    CheckError::new(
        "MultipleCovers",
        "There is more than one cover defined",
        name,
    )
    .with_locations(all_locations)
    .with_help(xml(
        "There is more than one <meta name=\"cover\"> tag defined. There should be only one.",
        false,
    ))
    .with_fix(
        "Remove all but the first meta cover tag",
        move |container| {
            let items = container.opf_xpath(r#"//opf:meta[@name="cover"]"#)?;
            let opf_name = container.opf_name.clone();
            for item in items.into_iter().skip(1) {
                container.remove_from_xml(&opf_name, item)?;
            }
            container.dirty(&opf_name);
            Ok(true)
        },
    )
}

pub fn no_uid(name: &str) -> CheckError {
    CheckError::new("NoUID", "The OPF has no unique identifier", name)
        .with_help(xml(
            "The OPF must have an unique identifier, i.e. a <dc:identifier> element whose id \
             is referenced by the <package> element",
            false,
        ))
        .with_fix("Auto-generate a unique identifier", move |container| {
            let uid = format!("id_{}", uuid::Uuid::new_v4().simple());
            let opf_name = container.opf_name.clone();
            let opf_root = container.opf_root()?;
            let metadata = {
                let m = container.opf_xpath("//opf:metadata")?;
                match m.into_iter().next() {
                    Some(id) => id,
                    None => {
                        let xml_tree = container.opf_mut()?;
                        let elem = xml_tree.new_element("metadata", Some(OPF2_NS));
                        xml_tree.ensure_namespace_declared(Some("dc"), DC11_NS);
                        container.insert_into_xml(&opf_name, opf_root, elem, Some(0))?;
                        elem
                    }
                }
            };
            let xml_tree = container.opf_mut()?;
            xml_tree.set_attr(opf_root, "unique-identifier", uid.clone());
            xml_tree.ensure_namespace_declared(Some("dc"), DC11_NS);
            let dc = xml_tree.new_element("identifier", Some(DC11_NS));
            xml_tree.set_attr(dc, "id", uid.clone());
            xml_tree.set_attr(dc, "scheme", "uuid");
            xml_tree.set_element_text(dc, uid);
            xml_tree.insert_element(metadata, dc, None);
            container.dirty(&opf_name);
            Ok(true)
        })
}

pub fn empty_identifier(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("EmptyIdentifier", "Empty identifier element", name)
        .at(lnum, None)
        .with_help(xml("The <dc:identifier> element must not be empty.", false))
        .with_fix("Remove empty identifiers", move |container| {
            let items = container.opf_xpath("//dc:identifier")?;
            let opf_name = container.opf_name.clone();
            let mut to_remove = Vec::new();
            {
                let xml_tree = container.opf()?;
                for item in &items {
                    let text = xml_tree.element_text(*item).unwrap_or("");
                    if text.trim().is_empty() {
                        to_remove.push(*item);
                    }
                }
            }
            for item in to_remove {
                container.remove_from_xml(&opf_name, item)?;
            }
            container.dirty(&opf_name);
            Ok(true)
        })
}

pub fn bad_spine_mime(
    name: &str,
    iid: Option<&str>,
    mt: &str,
    lnum: Option<u32>,
    opf_name: &str,
) -> CheckError {
    let owned_iid = iid.map(|s| s.to_string());
    let mut err = CheckError::new(
        "BadSpineMime",
        "Incorrect media-type for spine item",
        opf_name,
    )
    .at(lnum, None)
    .with_help(format!(
        "The item {name} present in the spine has the media-type {mt}. Most e-book software \
         cannot handle non-HTML spine items. If the item is actually HTML, you should change \
         its media-type to {xhtml_mime}. If it is not-HTML you should consider replacing it \
         with an HTML item, as it is unlikely to work in most readers.",
        xhtml_mime = crate::oeb::constants::XHTML_MIME
    ));
    if let Some(iid) = owned_iid {
        err = err.with_fix(
            format!(
                "Change the media-type to {}",
                crate::oeb::constants::XHTML_MIME
            ),
            move |container| {
                let items =
                    container.opf_xpath(&format!(r#"//opf:manifest/opf:item[@id="{iid}"]"#))?;
                let opf_name = container.opf_name.clone();
                if let Some(&item) = items.first() {
                    let xml_tree = container.opf_mut()?;
                    xml_tree.set_attr(item, "media-type", crate::oeb::constants::XHTML_MIME);
                }
                container.dirty(&opf_name);
                container.refresh_mime_map()?;
                Ok(true)
            },
        );
    }
    err
}

// ===================================================================
// check_opf
// ===================================================================

/// Port of `check_opf`. See the module docs for why `book_type` is an
/// explicit parameter.
pub fn check_opf(container: &mut Container, book_type: &str) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let opf_name = container.opf_name.clone();
    let opf_version = container.opf_version_parsed()?;
    let root = container.opf_root()?;

    let root_is_package = {
        let xml_tree = container.opf()?;
        xml_tree.local_name(root) == Some("package") && xml_tree.namespace(root) == Some(OPF2_NS)
    };
    if !root_is_package {
        let sourceline = container.opf()?.node(root).sourceline;
        let mut err = CheckError::new(
            "BaseError",
            "The OPF does not have the correct root element",
            &opf_name,
        )
        .at(sourceline, None);
        err.help = xml(
            &format!(
                "The OPF must have the root element <package> in namespace {OPF2_NS}, like \
                 this: <package xmlns=\"{OPF2_NS}\">"
            ),
            false,
        );
        errors.push(err);
    } else if container.opf()?.get_attr(root, "version").is_none() && book_type == "epub" {
        let sourceline = container.opf()?.node(root).sourceline;
        let mut err = CheckError::new("BaseError", "The OPF does not have a version", &opf_name)
            .at(sourceline, None);
        err.help = xml(
            "The <package> tag in the OPF must have a version attribute. This is usually \
             version=\"2.0\" for EPUB2 and AZW3 and version=\"3.0\" for EPUB3",
            false,
        );
        errors.push(err);
    }

    for tag in ["metadata", "manifest", "spine"] {
        if container.opf_xpath(&format!("//opf:{tag}"))?.is_empty() {
            errors.push(missing_section(&opf_name, tag));
        }
    }

    let (all_ids, empty_id_lines): (std::collections::HashSet<String>, Vec<Option<u32>>) = {
        let id_nodes = container.opf_xpath("//*[@id]")?;
        let xml_tree = container.opf()?;
        let mut ids = std::collections::HashSet::new();
        let mut empty_lines = Vec::new();
        for &n in &id_nodes {
            if let Some(v) = xml_tree.get_attr(n, "id") {
                ids.insert(v.to_string());
                if v.is_empty() {
                    empty_lines.push(xml_tree.node(n).sourceline);
                }
            }
        }
        (ids, empty_lines)
    };
    for lnum in empty_id_lines {
        errors.push(empty_id(&opf_name, lnum));
    }
    let all_ids: std::collections::HashSet<String> =
        all_ids.into_iter().filter(|s| !s.is_empty()).collect();

    {
        let idref_nodes = container.opf_xpath("//*[@idref]")?;
        let xml_tree = container.opf()?;
        for &n in &idref_nodes {
            if let Some(idref) = xml_tree.get_attr(n, "idref") {
                if !all_ids.contains(idref) {
                    errors.push(incorrect_idref(
                        &opf_name,
                        idref,
                        xml_tree.node(n).sourceline,
                    ));
                }
            }
        }
    }

    {
        let nl_nodes = container.opf_xpath(r#"//opf:spine/opf:itemref[@linear="no"]"#)?;
        if !nl_nodes.is_empty() {
            let xml_tree = container.opf()?;
            let locs: Vec<Option<u32>> = nl_nodes
                .iter()
                .map(|&n| xml_tree.node(n).sourceline)
                .collect();
            errors.push(non_linear_items(&opf_name, &locs));
        }
    }

    // Duplicate hrefs in the manifest, and MissingHref/NoHref.
    {
        let items = container.opf_xpath("//opf:manifest/opf:item")?;
        let mut seen: HashMap<String, Option<u32>> = HashMap::new();
        let mut dups: HashMap<String, Vec<Option<u32>>> = HashMap::new();
        let mut no_href_errors = Vec::new();
        let mut missing_href_errors = Vec::new();
        {
            // Snapshot (href, id, line) out of the tree first: resolving
            // a href to a name below needs `container.href_to_name`/
            // `container.exists`, which would otherwise overlap with
            // `xml_tree`'s borrow of `container`.
            let snapshot: Vec<(Option<String>, Option<String>, Option<u32>)> = {
                let xml_tree = container.opf()?;
                items
                    .iter()
                    .map(|&item| {
                        (
                            xml_tree.get_attr(item, "href").map(|s| s.to_string()),
                            xml_tree.get_attr(item, "id").map(|s| s.to_string()),
                            xml_tree.node(item).sourceline,
                        )
                    })
                    .collect()
            };
            for (href, id, lnum) in snapshot {
                match href {
                    None => {
                        no_href_errors.push((id, lnum));
                    }
                    Some(href) => {
                        let hname = container.href_to_name(&href, Some(&opf_name));
                        let missing = match &hname {
                            Some(n) => !container.exists(n),
                            None => true,
                        };
                        if missing {
                            missing_href_errors.push((href.clone(), lnum));
                        }
                        if let Some(&first) = seen.get(&href) {
                            dups.entry(href.clone())
                                .or_insert_with(|| vec![first])
                                .push(lnum);
                        } else {
                            seen.insert(href.clone(), lnum);
                        }
                    }
                }
            }
        }
        for (id, lnum) in no_href_errors {
            errors.push(no_href(&opf_name, id.as_deref(), lnum));
        }
        for (href, lnum) in missing_href_errors {
            errors.push(missing_href(&opf_name, &href, lnum));
        }
        let mut dup_hrefs: Vec<&String> = dups.keys().collect();
        dup_hrefs.sort();
        for href in dup_hrefs {
            errors.push(duplicate_href(&opf_name, href, &dups[href], false));
        }
    }

    // Duplicate idrefs in the spine.
    {
        let items = container.opf_xpath("//opf:spine/opf:itemref[@idref]")?;
        let mut seen: HashMap<String, Option<u32>> = HashMap::new();
        let mut dups: HashMap<String, Vec<Option<u32>>> = HashMap::new();
        {
            let xml_tree = container.opf()?;
            for &item in &items {
                if let Some(ridref) = xml_tree.get_attr(item, "idref") {
                    let lnum = xml_tree.node(item).sourceline;
                    if let Some(&first) = seen.get(ridref) {
                        dups.entry(ridref.to_string())
                            .or_insert_with(|| vec![first])
                            .push(lnum);
                    } else {
                        seen.insert(ridref.to_string(), lnum);
                    }
                }
            }
        }
        let mut dup_refs: Vec<&String> = dups.keys().collect();
        dup_refs.sort();
        for eid in dup_refs {
            errors.push(duplicate_href(&opf_name, eid, &dups[eid], true));
        }
    }

    // Spine `toc` attribute / NCX reference.
    {
        let spine_with_toc = container.opf_xpath("//opf:spine[@toc]")?;
        if let Some(&spine) = spine_with_toc.first() {
            let toc_id = container
                .opf()?
                .get_attr(spine, "toc")
                .map(|s| s.to_string());
            let mitems = container.opf_xpath("//opf:manifest/opf:item[@id]")?;
            let matching: Vec<XmlNodeId> = {
                let xml_tree = container.opf()?;
                mitems
                    .into_iter()
                    .filter(|&n| xml_tree.get_attr(n, "id").map(|s| s.to_string()) == toc_id)
                    .collect()
            };
            if let Some(&mitem) = matching.first() {
                let xml_tree = container.opf()?;
                let mt = xml_tree
                    .get_attr(mitem, "media-type")
                    .unwrap_or("")
                    .to_string();
                if mt != guess_type("a.ncx") {
                    errors.push(incorrect_toc(
                        &opf_name,
                        xml_tree.node(mitem).sourceline,
                        None,
                        Some(&mt),
                    ));
                }
            } else {
                let xml_tree = container.opf()?;
                errors.push(incorrect_toc(
                    &opf_name,
                    xml_tree.node(spine).sourceline,
                    toc_id.as_deref(),
                    None,
                ));
            }
        } else {
            let spine = container.opf_xpath("//opf:spine")?;
            if let Some(&spine) = spine.first() {
                let ncx_name = container
                    .manifest_type_map()?
                    .get(&guess_type("a.ncx"))
                    .and_then(|v| v.first().cloned());
                if let Some(ncx_name) = ncx_name {
                    let manifest_id_map = container.manifest_id_map()?;
                    let ncx_id = manifest_id_map
                        .iter()
                        .find(|(_, v)| **v == ncx_name)
                        .map(|(k, _)| k.clone());
                    if let Some(ncx_id) = ncx_id {
                        let lnum = container.opf()?.node(spine).sourceline;
                        errors.push(missing_ncx_ref(&opf_name, lnum, &ncx_id));
                    }
                }
            }
        }
    }

    if opf_version.0 > 2 {
        let existing_nav = find_existing_nav_toc(container)?;
        match existing_nav {
            None => errors.push(missing_nav(&opf_name, None)),
            Some(nav_name) => {
                let toc = parse_nav(container, &nav_name)?;
                if toc.len(toc.root) == 0 {
                    errors.push(empty_nav(&nav_name, None));
                }
            }
        }
    }

    // Cover metadata.
    {
        let covers = container.opf_xpath(r#"//opf:meta[@name="cover"]"#)?;
        if !covers.is_empty() {
            if covers.len() > 1 {
                let locs: Vec<Option<u32>> = {
                    let xml_tree = container.opf()?;
                    covers
                        .iter()
                        .map(|&c| xml_tree.node(c).sourceline)
                        .collect()
                };
                errors.push(multiple_covers(&opf_name, &locs));
            }
            let manifest_ids: std::collections::HashSet<String> = {
                let id_nodes = container.opf_xpath("//opf:manifest/opf:item[@id]")?;
                let xml_tree = container.opf()?;
                id_nodes
                    .into_iter()
                    .filter_map(|n| xml_tree.get_attr(n, "id").map(|s| s.to_string()))
                    .collect()
            };
            for &cover in &covers {
                let xml_tree = container.opf()?;
                let content = xml_tree.get_attr(cover, "content").map(|s| s.to_string());
                let lnum = xml_tree.node(cover).sourceline;
                let has_name_before_content = {
                    let attrs = &xml_tree.node(cover).attrs;
                    let name_pos = attrs.get_index_of("name");
                    let content_pos = attrs.get_index_of("content");
                    matches!((name_pos, content_pos), (Some(n), Some(c)) if c < n)
                };
                if content
                    .as_deref()
                    .map(|c| !manifest_ids.contains(c))
                    .unwrap_or(true)
                {
                    errors.push(incorrect_cover(
                        &opf_name,
                        lnum,
                        content.as_deref().unwrap_or(""),
                    ));
                }
                if has_name_before_content {
                    errors.push(nook_cover(&opf_name, lnum));
                }
            }
        }
    }

    // Unique identifier.
    {
        let uid = container
            .opf()?
            .get_attr(root, "unique-identifier")
            .map(|s| s.to_string());
        match uid {
            None => errors.push(no_uid(&opf_name)),
            Some(uid) => {
                let dcid = container.opf_xpath(&format!(r#"//dc:identifier[@id="{uid}"]"#))?;
                let xml_tree = container.opf()?;
                let has_text = dcid
                    .first()
                    .and_then(|&n| xml_tree.element_text(n))
                    .map(|t| !t.trim().is_empty())
                    .unwrap_or(false);
                if !has_text {
                    errors.push(no_uid(&opf_name));
                }
            }
        }
        let identifiers = container.opf_xpath("//dc:identifier")?;
        let xml_tree = container.opf()?;
        for &elem in &identifiers {
            let empty = xml_tree
                .element_text(elem)
                .map(|t| t.trim().is_empty())
                .unwrap_or(true);
            if empty {
                errors.push(empty_identifier(&opf_name, xml_tree.node(elem).sourceline));
            }
        }
    }

    // Spine item media types.
    {
        let spine_items = container.spine_iter()?;
        for (item, name, _linear) in spine_items {
            let mt = container
                .base
                .mime_map
                .get(&name)
                .cloned()
                .unwrap_or_default();
            if mt != crate::oeb::constants::XHTML_MIME {
                let iid = container
                    .opf()?
                    .get_attr(item, "idref")
                    .map(|s| s.to_string());
                let (iid, lnum) = match &iid {
                    Some(iid) => {
                        let mitems = container
                            .opf_xpath(&format!(r#"//opf:manifest/opf:item[@id="{iid}"]"#))?;
                        if let Some(&mitem) = mitems.first() {
                            (Some(iid.clone()), container.opf()?.node(mitem).sourceline)
                        } else {
                            (None, None)
                        }
                    }
                    None => (None, None),
                };
                errors.push(bad_spine_mime(&name, iid.as_deref(), &mt, lnum, &opf_name));
            }
        }
    }

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_container(dir: &Path, opf: &str) -> Container {
        std::fs::write(dir.join("content.opf"), opf).unwrap();
        std::fs::write(
            dir.join("chap1.html"),
            b"<html><body><p>hi</p></body></html>",
        )
        .unwrap();
        Container::open(dir, &dir.join("content.opf")).unwrap()
    }

    const GOOD_OPF: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test</dc:title>
    <dc:identifier id="bookid">urn:uuid:x</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

    #[test]
    fn check_opf_accepts_a_well_formed_opf() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), GOOD_OPF);
        let errors = check_opf(&mut c, "epub").unwrap();
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
    }

    #[test]
    fn check_opf_flags_missing_manifest_section() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <spine/>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("content.opf"), opf).unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let errors = check_opf(&mut c, "epub").unwrap();
        assert!(errors
            .iter()
            .any(|e| e.type_name == "MissingSection" && e.msg.contains("manifest")));
    }

    #[test]
    fn check_opf_flags_duplicate_href_and_missing_file() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="c2" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="c3" href="missing.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), opf);
        let errors = check_opf(&mut c, "epub").unwrap();
        assert!(errors.iter().any(|e| e.type_name == "DuplicateHref"));
        assert!(errors.iter().any(|e| e.type_name == "MissingHref"));
    }

    #[test]
    fn check_opf_flags_no_uid_and_incorrect_idref() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0">
  <metadata><dc:identifier>urn:uuid:x</dc:identifier></metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine toc="nonexistent"><itemref idref="c1"/></spine>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), opf);
        let errors = check_opf(&mut c, "epub").unwrap();
        assert!(errors.iter().any(|e| e.type_name == "NoUID"));
        assert!(errors.iter().any(|e| e.type_name == "IncorrectToc"));
    }

    #[test]
    fn check_opf_flags_non_linear_and_bad_spine_mime() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="im1" href="pic.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="c1" linear="no"/>
    <itemref idref="im1"/>
  </spine>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("content.opf"), opf).unwrap();
        std::fs::write(dir.path().join("chap1.html"), b"<html><body/></html>").unwrap();
        std::fs::write(dir.path().join("pic.png"), b"\x89PNG").unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let errors = check_opf(&mut c, "epub").unwrap();
        assert!(errors.iter().any(|e| e.type_name == "NonLinearItems"));
        assert!(errors.iter().any(|e| e.type_name == "BadSpineMime"));
    }

    #[test]
    fn nook_cover_fix_reorders_attributes() {
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:identifier id="bookid">urn:uuid:x</dc:identifier>
    <meta content="cover-img" name="cover"/>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="cover-img" href="cover.png" media-type="image/png"/>
  </manifest>
  <spine><itemref idref="c1"/></spine>
</package>"#;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("content.opf"), opf).unwrap();
        std::fs::write(dir.path().join("chap1.html"), b"<html><body/></html>").unwrap();
        std::fs::write(dir.path().join("cover.png"), b"\x89PNG").unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let errors = check_opf(&mut c, "epub").unwrap();
        let mut nook = errors
            .into_iter()
            .find(|e| e.type_name == "NookCover")
            .unwrap();
        assert!(nook.apply_fix(&mut c).unwrap());
        let covers = c.opf_xpath(r#"//opf:meta[@name="cover"]"#).unwrap();
        let xml_tree = c.opf().unwrap();
        let attrs = &xml_tree.node(covers[0]).attrs;
        assert!(attrs.get_index_of("name").unwrap() < attrs.get_index_of("content").unwrap());
    }
}
