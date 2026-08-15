//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/links.py`.
//!
//! # Scope note: `check_links`/`check_external_links` and the CSS
//! `iterlinks` gap
//!
//! [`Container::iterlinks`](super::super::container::Container::iterlinks)
//! (issue #161/#165) is real for the OPF, XHTML/HTML content, and NCX
//! cases, but `todo!()`s for stylesheets: rewriting/iterating `url(...)`
//! references inside CSS source needs a real CSS parser wired into that
//! specific method, which is [`crate::css`]'s existing, documented,
//! separate gap (`Container::replace_links`'s own docs). [`check_links`]
//! and [`check_external_links`] both iterate `OEB_STYLES` files via
//! `iterlinks` in Python; this port skips CSS files in that scan rather
//! than triggering the pre-existing gap. Concretely this means: links
//! *directly on an HTML/OPF/NCX element* (the overwhelming majority of
//! real book links) are checked exactly as in Python, but a `url(...)`
//! reference that exists *only* inside a `.css` file (e.g. an `@import`
//! chain, or a background-image only ever referenced from CSS) is not
//! discovered by this scan. Every other check in this file is fully
//! real.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use crate::mobi::dom::Dom;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES, XHTML_MIME};
use crate::oeb::polish::cover::get_raster_cover_name;
use crate::oeb::polish::parsing::parse_html5;
use crate::oeb::polish::replace::remove_links_to;
use crate::oeb::polish::utils::{
    actual_case_for_name, corrected_case_for_name, guess_type, OEB_FONTS,
};

use super::super::container::Container;
use super::base::{CheckError, Level};

// ===================================================================
// A minimal, lenient URL splitter (port of the handful of `urlparse`
// fields links.py reads: `.scheme`, `.path`, `.fragment`)
// ===================================================================

/// The `.scheme`/`.path`/`.fragment` fields of Python's `urlparse`
/// result that `links.py` actually reads. Deliberately not a full
/// RFC 3986 implementation (this crate already has [`url::Url`] for
/// strict parsing, used elsewhere for absolute URLs) -- `links.py` runs
/// this over arbitrary, possibly-relative, possibly-malformed `href`
/// text pulled straight out of book markup, which is exactly the shape
/// Python's very lenient `urlparse` (never requires a base, rarely
/// raises) is suited for and `url::Url::parse` (always requires either
/// an absolute URL or a base to resolve against) is not.
struct LooseUrl {
    scheme: String,
    path: String,
    fragment: String,
}

fn parse_url_loose(href: &str) -> Option<LooseUrl> {
    let (before_frag, fragment) = match href.split_once('#') {
        Some((b, f)) => (b, f.to_string()),
        None => (href, String::new()),
    };
    let before_query = before_frag.split('?').next().unwrap_or(before_frag);
    let scheme_end = before_query.find(':').filter(|&i| i > 0).filter(|&i| {
        let head = &before_query[..i];
        head.chars().next().unwrap().is_ascii_alphabetic()
            && head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
    });
    let (scheme, mut rest) = match scheme_end {
        Some(i) => (
            before_query[..i].to_ascii_lowercase(),
            &before_query[i + 1..],
        ),
        None => (String::new(), before_query),
    };
    if let Some(after_slashes) = rest.strip_prefix("//") {
        // A netloc is present. A malformed IPv6-literal-looking netloc
        // (unbalanced `[`/`]`) is the one case Python's `urlparse` can
        // raise `ValueError` for -- surfaced here as `None` so callers
        // report `MalformedURL`, matching that behavior without
        // attempting full RFC 3986 host validation.
        if after_slashes.starts_with('[') && !after_slashes[1..].contains(']') {
            return None;
        }
        rest = match after_slashes.find('/') {
            Some(idx) => &after_slashes[idx..],
            None => "",
        };
    }
    Some(LooseUrl {
        scheme,
        path: rest.to_string(),
        fragment,
    })
}

// ===================================================================
// Error constructors
// ===================================================================

pub fn bad_link(msg: &str, name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("BadLink", msg.to_string(), name)
        .at(lnum, None)
        .with_level(Level::Warn)
        .with_help(
            "The resource pointed to by this link does not exist. You should either fix, or \
             remove the link.",
        )
}

pub fn invalid_char_in_link(href: &str, name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "InvalidCharInLink",
        format!(
            "The link {href} contains a : character, this will cause errors on Windows computers"
        ),
        name,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(
        "Windows computers do not allow the : character in filenames. For maximum \
         compatibility it is best to not use these in filenames/links to files.",
    )
}

pub fn malformed_url(href: &str, name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "MalformedURL",
        format!("The URL {href} could not be parsed"),
        name,
    )
    .at(lnum, None)
    .with_level(Level::Error)
    .with_help("This URL could not be parsed.")
}

pub fn empty_link(name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new("EmptyLink", "The link is empty", name)
        .at(lnum, None)
        .with_level(Level::Warn)
        .with_help(
            "This link is empty. This is almost always a mistake. Either fill in the link \
             destination or remove the link tag.",
        )
}

pub fn file_link(href: &str, name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "FileLink",
        format!("The link {href} is a file:// URL"),
        name,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(
        "This link uses the file:// URL scheme. This does not work with many e-book \
             readers. Remove the file:// prefix and make sure the link points to a file inside \
             the book.",
    )
}

pub fn local_link(href: &str, name: &str, lnum: Option<u32>) -> CheckError {
    CheckError::new(
        "LocalLink",
        format!("The link {href} points to a file outside the book"),
        name,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(
        "This link points to a file outside the book. It will not work if the book is read on \
         any computer other than the one it was created on. Either fix or remove the link.",
    )
}

pub fn bad_destination_type(
    link_source: &str,
    link_dest: &str,
    href: &str,
    lnum: Option<u32>,
) -> CheckError {
    CheckError::new(
        "BadDestinationType",
        "Link points to a file that is not a text document",
        link_source,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(format!(
        "The link \"{href}\" points to a file <i>{link_dest}</i> that is not a text (HTML) \
         document. Many e-book readers will be unable to follow such a link. You should \
         either remove the link or change it to point to a text document. For example, if it \
         points to an image, you can create small wrapper document that contains the image and \
         change the link to point to that."
    ))
}

pub fn bad_destination_fragment(
    link_source: &str,
    link_dest: &str,
    href: &str,
    lnum: Option<u32>,
    fragment: &str,
) -> CheckError {
    CheckError::new(
        "BadDestinationFragment",
        "Link points to a location not present in the target file",
        link_source,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(format!(
        "The link \"{href}\" points to a location <i>{fragment}</i> in the file {link_dest} \
         that does not exist. You should either remove the location so that the link points to \
         the top of the file, or change the link to point to the correct location."
    ))
}

/// Port of `UnreferencedResource`. Note (matching Python exactly): this
/// base class has no `INDIVIDUAL_FIX`/`__call__` of its own -- only its
/// `UnreferencedDoc` subclass ([`unreferenced_doc`]) is auto-fixable.
pub fn unreferenced_resource(name: &str) -> CheckError {
    CheckError::new(
        "UnreferencedResource",
        format!("The file {name} is not referenced"),
        name,
    )
    .with_level(Level::Warn)
    .with_help(
        "This file is included in the book but not referred to by any document in the \
             spine. This means that the file will not be viewable on most e-book readers. You \
             should probably remove this file from the book or add a link to it somewhere.",
    )
}

pub fn unreferenced_doc(name: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new(
        "UnreferencedDoc",
        format!("The file {name} is not referenced"),
        name,
    )
    .with_level(Level::Warn)
    .with_help(
        "This file is not in the book spine. All content documents must be in the spine. \
             You should probably add it to the spine.",
    )
    .with_fix("Append this file to the spine", move |container| {
        let rmap: HashMap<String, String> = container
            .manifest_id_map()?
            .into_iter()
            .map(|(id, n)| (n, id))
            .collect();
        let manifest_id = match rmap.get(&owned_name) {
            Some(id) => id.clone(),
            None => container.add_name_to_manifest(&owned_name, "")?,
        };
        let spine = container.opf_xpath("//opf:spine")?;
        let opf_name = container.opf_name.clone();
        if let Some(&spine) = spine.first() {
            let xml_tree = container.opf_mut()?;
            let item = xml_tree.new_element("itemref", Some(crate::oeb::constants::OPF2_NS));
            xml_tree.set_attr(item, "idref", manifest_id);
            xml_tree.insert_element(spine, item, None);
            container.dirty(&opf_name);
        }
        Ok(true)
    })
}

pub fn unmanifested(name: &str, unreferenced: Option<bool>) -> CheckError {
    let owned_name = name.to_string();
    let mut err = CheckError::new(
        "Unmanifested",
        format!("The file {name} is not listed in the manifest"),
        name,
    )
    .with_level(Level::Warn)
    .with_help(
        "This file is not listed in the book manifest. While not strictly necessary it is \
         good practice to list all files in the manifest. Either list this file in the \
         manifest or remove it from the book if it is an unnecessary file.",
    );
    if let Some(unreferenced) = unreferenced {
        let label = if unreferenced {
            format!("Remove {name} from the book")
        } else {
            format!("Add {name} to the manifest")
        };
        err = err.with_fix(label, move |container| {
            if unreferenced {
                container.remove_item(&owned_name, true)?;
            } else {
                let rmap: HashSet<String> = container.manifest_id_map()?.into_values().collect();
                if !rmap.contains(&owned_name) {
                    container.add_name_to_manifest(&owned_name, "")?;
                }
            }
            Ok(true)
        });
    }
    err
}

pub fn dangling_link(msg: &str, target_name: &str, name: &str, lnum: Option<u32>) -> CheckError {
    let owned_target = target_name.to_string();
    CheckError::new("DanglingLink", msg.to_string(), name)
        .at(lnum, None)
        .with_level(Level::Warn)
        .with_help(
            "The resource pointed to by this link does not exist. You should either fix, or \
             remove the link.",
        )
        .with_fix(
            format!("Remove all references to {target_name} from the HTML and CSS in the book"),
            move |container| {
                let changed = remove_links_to(
                    container,
                    &|n: Option<&str>, _href: &str, _frag: Option<&str>| {
                        n == Some(owned_target.as_str())
                    },
                )?;
                Ok(!changed.is_empty())
            },
        )
}

pub fn bookmarks(name: &str) -> CheckError {
    let owned_name = name.to_string();
    CheckError::new(
        "Bookmarks",
        "The bookmarks file used by the calibre E-book viewer is present",
        name,
    )
    .with_level(Level::Info)
    .with_help(
        "This file stores the bookmarks and last opened information from the calibre E-book \
         viewer. You can remove it if you do not need that information, or don't want to share \
         it with other people you send this book to.",
    )
    .with_fix("Remove this file", move |container| {
        container.remove_item(&owned_name, true)?;
        Ok(true)
    })
}

pub fn mimetype_mismatch(
    container: &mut Container,
    name: &str,
    opf_mt: &str,
    ext_mt: &str,
) -> Result<CheckError> {
    let ext = name.rsplit('.').next().unwrap_or(name);
    let spine_names: HashSet<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let is_spine_doc = OEB_DOCS.contains(&opf_mt) && spine_names.contains(name);
    let opf_name = container.opf_name.clone();
    let owned_name = name.to_string();
    let owned_opf_mt = opf_mt.to_string();
    let owned_ext_mt = ext_mt.to_string();
    let mut err = CheckError::new(
        "MimetypeMismatch",
        format!("The file {name} has a MIME type that does not match its extension"),
        &opf_name,
    )
    .with_level(Level::Warn)
    .with_help(format!(
        "The file {name} has its MIME type specified as {opf_mt} in the OPF file. The \
         recommended MIME type for files with the extension \"{ext}\" is {ext_mt}. You should \
         change either the file extension or the MIME type in the OPF."
    ));
    if is_spine_doc {
        err = err.with_fix("Change the file extension to .xhtml", move |container| {
            let base = owned_name
                .rsplit_once('.')
                .map(|(b, _)| b)
                .unwrap_or(&owned_name);
            let mut new_name = format!("{base}.xhtml");
            let mut c = 0u32;
            while container.has_name(&new_name) {
                c += 1;
                new_name = format!("{base}{c}.xhtml");
            }
            let mut file_map = HashMap::new();
            file_map.insert(owned_name.clone(), new_name);
            super::super::replace::rename_files(container, &file_map)?;
            Ok(true)
        });
    } else {
        err = err.with_fix(
            format!("Change the MIME type for this file in the OPF to {ext_mt}"),
            move |container| {
                let items = container.opf_xpath(&format!(
                    r#"//opf:manifest/opf:item[@href and @media-type="{owned_opf_mt}"]"#
                ))?;
                let mut changed = false;
                let mut to_update = Vec::new();
                {
                    let opf_name = container.opf_name.clone();
                    for item in &items {
                        let href = container
                            .opf()?
                            .get_attr(*item, "href")
                            .unwrap_or("")
                            .to_string();
                        if let Some(n) = container.href_to_name(&href, Some(&opf_name)) {
                            if n == owned_name {
                                to_update.push(*item);
                            }
                        }
                    }
                }
                for item in to_update {
                    let xml_tree = container.opf_mut()?;
                    xml_tree.set_attr(item, "media-type", owned_ext_mt.clone());
                    changed = true;
                }
                if changed {
                    container
                        .base
                        .mime_map
                        .insert(owned_name.clone(), owned_ext_mt.clone());
                    let opf_name = container.opf_name.clone();
                    container.dirty(&opf_name);
                }
                Ok(changed)
            },
        );
    }
    Ok(err)
}

// ===================================================================
// check_mimetypes / check_link_destinations / check_links
// ===================================================================

/// Port of `check_mimetypes`.
pub fn check_mimetypes(container: &mut Container) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();
    for (name, mt) in names {
        let gt = container.guess_type(&name);
        if mt != gt {
            if mt == "application/oebps-page-map+xml" && name.to_lowercase().ends_with(".xml") {
                continue;
            }
            errors.push(mimetype_mismatch(container, &name, &mt, &gt)?);
        }
    }
    Ok(errors)
}

fn collect_html_ids(dom: &Dom) -> HashSet<String> {
    let mut out = HashSet::new();
    for el in dom.preorder_elements(dom.root) {
        if let Some(v) = dom.node(el).attrs.get("id") {
            out.insert(v.clone());
        }
        if let Some(v) = dom.node(el).attrs.get("name") {
            out.insert(v.clone());
        }
    }
    out
}

fn check_link_destination(
    container: &mut Container,
    dest_map: &mut HashMap<String, HashSet<String>>,
    name: &str,
    href: &str,
    lnum: Option<u32>,
    errors: &mut Vec<CheckError>,
) -> Result<()> {
    let tname = if let Some(stripped) = href.strip_prefix('#') {
        let _ = stripped;
        Some(name.to_string())
    } else {
        container.href_to_name(href, Some(name))
    };
    let Some(tname) = tname else { return Ok(()) };
    let Some(mt) = container.base.mime_map.get(&tname).cloned() else {
        return Ok(());
    };
    if !OEB_DOCS.contains(&mt.as_str()) {
        errors.push(bad_destination_type(name, &tname, href, lnum));
        return Ok(());
    }
    container.ensure_parsed(&tname)?;
    if !dest_map.contains_key(&tname) {
        let ids = collect_html_ids(container.get_xhtml(&tname)?);
        dest_map.insert(tname.clone(), ids);
    }
    if let Some(purl) = parse_url_loose(href) {
        if !purl.fragment.is_empty() && !dest_map[&tname].contains(&purl.fragment) {
            errors.push(bad_destination_fragment(
                name,
                &tname,
                href,
                lnum,
                &purl.fragment,
            ));
        }
    }
    Ok(())
}

/// Port of `check_link_destinations`.
pub fn check_link_destinations(
    container: &mut Container,
    book_type: &str,
) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let mut dest_map = HashMap::new();
    let opf_type = guess_type("a.opf");
    let ncx_type = guess_type("a.ncx");
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    for (name, mt) in names {
        if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let anchors: Vec<(Option<u32>, String)> = {
                let dom = container.get_xhtml(&name)?;
                dom.preorder_elements(dom.root)
                    .into_iter()
                    .filter(|&el| {
                        dom.tag(el) == Some("a") && dom.node(el).attrs.contains_key("href")
                    })
                    .map(|el| {
                        (
                            None,
                            dom.node(el).attrs.get("href").cloned().unwrap_or_default(),
                        )
                    })
                    .collect()
            };
            for (lnum, href) in anchors {
                check_link_destination(container, &mut dest_map, &name, &href, lnum, &mut errors)?;
            }
        } else if mt == opf_type {
            let refs = container.opf_xpath("//opf:reference[@href]")?;
            let entries: Vec<(Option<u32>, String)> = {
                let xml_tree = container.opf()?;
                refs.iter()
                    .filter(|&&n| {
                        if book_type != "azw3" {
                            return true;
                        }
                        !matches!(
                            xml_tree.get_attr(n, "type"),
                            Some("cover")
                                | Some("other.ms-coverimage-standard")
                                | Some("other.ms-coverimage")
                        )
                    })
                    .map(|&n| {
                        (
                            xml_tree.node(n).sourceline,
                            xml_tree.get_attr(n, "href").unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            };
            for (lnum, href) in entries {
                check_link_destination(container, &mut dest_map, &name, &href, lnum, &mut errors)?;
            }
        } else if mt == ncx_type {
            container.ensure_parsed(&name)?;
            let entries: Vec<(Option<u32>, String)> = {
                let xml_tree = container.get_xml(&name)?;
                xml_tree
                    .opf_xpath("//*[@src]", &HashMap::new())
                    .into_iter()
                    .filter(|&n| xml_tree.local_name(n) == Some("content"))
                    .map(|n| {
                        (
                            xml_tree.node(n).sourceline,
                            xml_tree.get_attr(n, "src").unwrap_or("").to_string(),
                        )
                    })
                    .collect()
            };
            for (lnum, href) in entries {
                check_link_destination(container, &mut dest_map, &name, &href, lnum, &mut errors)?;
            }
        }
    }
    Ok(errors)
}

/// Port of `check_links`. See the module docs for the CSS-scanning
/// simplification.
pub fn check_links(container: &mut Container) -> Result<Vec<CheckError>> {
    let mut links_map: HashMap<String, HashSet<String>> = HashMap::new();
    let xml_types: HashSet<String> = [guess_type("a.opf"), guess_type("a.ncx")]
        .into_iter()
        .collect();
    let mut errors = Vec::new();

    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();

    for (name, mt) in &names {
        let scannable = OEB_DOCS.contains(&mt.as_str()) || xml_types.contains(mt);
        // OEB_STYLES intentionally excluded -- see the module docs.
        if !scannable {
            continue;
        }
        for (href, lnum, _col) in container.iterlinks(name)? {
            if href.is_empty() {
                errors.push(empty_link(name, lnum));
                continue;
            }
            let tname = container.href_to_name(&href, Some(name));
            match tname {
                Some(tname) => {
                    if container.exists(&tname) {
                        if container.base.mime_map.contains_key(&tname) {
                            links_map
                                .entry(name.clone())
                                .or_default()
                                .insert(tname.clone());
                        } else {
                            let apath = container.name_to_abspath(&tname);
                            if apath.is_dir() {
                                errors.push(bad_link(
                                    &format!("The linked resource {href} is a folder"),
                                    name,
                                    lnum,
                                ));
                            } else if let Ok(corrected) = actual_case_for_name(container, &tname) {
                                errors.push(case_mismatch_real(&href, &corrected, name, lnum));
                            }
                        }
                    } else if let Some(cname) = corrected_case_for_name(container, &tname) {
                        errors.push(case_mismatch_real(&href, &cname, name, lnum));
                    } else {
                        errors.push(dangling_link(
                            &format!("The linked resource {href} does not exist"),
                            &tname,
                            name,
                            lnum,
                        ));
                    }
                }
                None => match parse_url_loose(&href) {
                    None => errors.push(malformed_url(&href, name, lnum)),
                    Some(purl) => {
                        if purl.scheme == "file" {
                            errors.push(file_link(&href, name, lnum));
                        } else if !purl.path.is_empty()
                            && purl.path.starts_with('/')
                            && (purl.scheme.is_empty() || purl.scheme == "file")
                        {
                            errors.push(local_link(&href, name, lnum));
                        } else if !purl.path.is_empty()
                            && (purl.scheme.is_empty() || purl.scheme == "file")
                            && url_decode(&purl.path).contains(':')
                        {
                            errors.push(invalid_char_in_link(&href, name, lnum));
                        }
                    }
                },
            }
        }
    }

    let spine_docs: HashSet<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    let mut spine_styles: HashSet<String> = spine_docs
        .iter()
        .flat_map(|name| links_map.get(name).cloned().unwrap_or_default())
        .filter(|tname| {
            container
                .base
                .mime_map
                .get(tname)
                .map(|m| OEB_STYLES.contains(&m.as_str()))
                .unwrap_or(false)
        })
        .collect();
    loop {
        let before = spine_styles.len();
        let extra: HashSet<String> = spine_styles
            .iter()
            .flat_map(|name| links_map.get(name).cloned().unwrap_or_default())
            .filter(|tname| {
                container
                    .base
                    .mime_map
                    .get(tname)
                    .map(|m| OEB_STYLES.contains(&m.as_str()))
                    .unwrap_or(false)
            })
            .collect();
        spine_styles.extend(extra);
        if spine_styles.len() == before {
            break;
        }
    }
    let seen_types: HashSet<&str> = OEB_DOCS.iter().chain(OEB_STYLES.iter()).copied().collect();
    let spine_all: HashSet<&String> = spine_docs.iter().chain(spine_styles.iter()).collect();
    let spine_resources: HashSet<String> = spine_all
        .iter()
        .flat_map(|name| links_map.get(name.as_str()).cloned().unwrap_or_default())
        .filter(|tname| {
            container
                .base
                .mime_map
                .get(tname)
                .map(|m| !seen_types.contains(m.as_str()))
                .unwrap_or(false)
        })
        .collect();

    let cover_name = container.guide_type_map()?.get("cover").cloned();
    let nav_items: HashSet<String> = container
        .manifest_items_with_property("nav")?
        .into_iter()
        .collect();
    let raster_cover_name = get_raster_cover_name(container)?;

    let mut unreferenced: HashSet<String> = HashSet::new();
    for (name, mt) in &names {
        let flag = if OEB_STYLES.contains(&mt.as_str()) && !spine_styles.contains(name) {
            Some(true)
        } else if OEB_DOCS.contains(&mt.as_str())
            && !spine_docs.contains(name)
            && !nav_items.contains(name)
        {
            Some(false)
        } else if (OEB_FONTS.contains(&mt.as_str())
            || matches!(mt.split('/').next(), Some("image" | "audio" | "video")))
            && !spine_resources.contains(name)
            && Some(name) != cover_name.as_ref()
        {
            if mt.split('/').next() == Some("image") && Some(name) == raster_cover_name.as_ref() {
                None
            } else {
                Some(true)
            }
        } else {
            None
        };
        match flag {
            Some(true) => {
                errors.push(unreferenced_resource(name));
                unreferenced.insert(name.clone());
            }
            Some(false) => {
                errors.push(unreferenced_doc(name));
                unreferenced.insert(name.clone());
            }
            None => {}
        }
    }

    let manifest_names: HashSet<String> = container.manifest_id_map()?.into_values().collect();
    for (name, _mt) in &names {
        if !manifest_names.contains(name) && !container.ok_to_be_unmanifested(name) {
            errors.push(unmanifested(name, Some(unreferenced.contains(name))));
        }
        if name == "META-INF/calibre_bookmarks.txt" {
            errors.push(bookmarks(name));
        }
    }

    Ok(errors)
}

fn url_decode(s: &str) -> String {
    urlencoding::decode(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// Port of `CaseMismatch`. A plain constructor (rather than being built
/// inline at each of `check_links`'s two call sites) since both the
/// message and the fixer close over the same `href`/`corrected_name`/
/// `name` triple.
fn case_mismatch_real(
    href: &str,
    corrected_name: &str,
    name: &str,
    lnum: Option<u32>,
) -> CheckError {
    let owned_href = href.to_string();
    let owned_corrected = corrected_name.to_string();
    let owned_name = name.to_string();
    CheckError::new(
        "CaseMismatch",
        format!("The linked to resource {href} does not exist"),
        name,
    )
    .at(lnum, None)
    .with_level(Level::Warn)
    .with_help(format!(
        "The case of the link {href} and the case of the actual file it points to \
         {corrected_name} do not agree. You should change either the case of the link or \
         rename the file."
    ))
    .with_fix(
        "Change the case of the link to match the actual file",
        move |container| {
            let frag = parse_url_loose(&owned_href)
                .map(|u| u.fragment)
                .unwrap_or_default();
            let mut nhref = container.name_to_href(&owned_corrected, Some(&owned_name));
            if !frag.is_empty() {
                nhref.push('#');
                nhref.push_str(&frag);
            }
            let mut replaced = false;
            let target_href = owned_href.clone();
            container.replace_links(&owned_name, |url, _ft| {
                if url == target_href {
                    replaced = true;
                    Some(nhref.clone())
                } else {
                    None
                }
            })?;
            Ok(replaced)
        },
    )
}

// ===================================================================
// check_external_links
// ===================================================================

/// `(name, href, line, col)` for every place in the book a given URL
/// was referenced from.
type LinkLocations = Vec<(String, String, Option<u32>, usize)>;

/// One external URL that failed to fetch (or, when `check_anchors` is
/// set, whose HTML fragment could not be found). Port of the `(locations,
/// exception, full_href)` tuples Python's `check_external_links`
/// collects into `ans`.
pub struct ExternalLinkFailure {
    pub locations: LinkLocations,
    pub error: String,
    pub url: String,
}

fn get_html_ids(raw: &[u8]) -> HashSet<String> {
    let text = String::from_utf8_lossy(raw);
    let dom = parse_html5(&text, true, true);
    let mut ids = HashSet::new();
    for body in dom.find_all_tag_global("body") {
        for el in dom.preorder_elements(body) {
            if let Some(v) = dom.node(el).attrs.get("id") {
                ids.insert(v.clone());
            }
            if dom.tag(el) == Some("a") {
                if let Some(v) = dom.node(el).attrs.get("name") {
                    ids.insert(v.clone());
                }
            }
        }
    }
    ids
}

/// Port of `check_external_links`: fetches every `http(s)://` URL
/// referenced from an HTML/CSS document (CSS files skipped per the
/// module docs) with a bounded worker pool, per
/// `docs/FAULT_TOLERANCE.md` §6 ("bounded timeout, no silent infinite
/// retry, attempted exactly once"). `progress_callback(done, total)` is
/// invoked from whichever worker thread completes each fetch.
pub fn check_external_links(
    container: &mut Container,
    timeout: Duration,
    check_anchors: bool,
    mut progress_callback: impl FnMut(usize, usize) + Send,
) -> Result<Vec<ExternalLinkFailure>> {
    progress_callback(0, 0);
    let mut external_links: HashMap<String, LinkLocations> = HashMap::new();
    let mut names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    names.sort();
    for (name, mt) in &names {
        if !OEB_DOCS.contains(&mt.as_str()) {
            // OEB_STYLES intentionally excluded -- see the module docs.
            continue;
        }
        for (href, lnum, col) in container.iterlinks(name)? {
            if let Some(purl) = parse_url_loose(&href) {
                if purl.scheme == "http" || purl.scheme == "https" {
                    external_links.entry(href.clone()).or_default().push((
                        name.clone(),
                        href,
                        lnum,
                        col,
                    ));
                }
            }
        }
    }
    if external_links.is_empty() {
        return Ok(Vec::new());
    }
    progress_callback(0, external_links.len());

    let entries: Vec<(String, LinkLocations)> = external_links.into_iter().collect();
    let total = entries.len();
    let next_index = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let results = Mutex::new(Vec::new());
    let progress_callback = Mutex::new(&mut progress_callback);
    let num_workers = 10.min(total).max(1);

    std::thread::scope(|scope| {
        for _ in 0..num_workers {
            let entries = &entries;
            let next_index = &next_index;
            let done = &done;
            let results = &results;
            let progress_callback = &progress_callback;
            scope.spawn(move || {
                let client = reqwest::blocking::Client::builder().timeout(timeout).build().ok();
                loop {
                    let idx = next_index.fetch_add(1, Ordering::SeqCst);
                    if idx >= entries.len() {
                        break;
                    }
                    let (full_href, locations) = &entries[idx];
                    let (href, frag) = full_href.split_once('#').unwrap_or((full_href.as_str(), ""));
                    let outcome = match &client {
                        None => Some("Failed to build HTTP client".to_string()),
                        Some(client) => match client.get(href).send() {
                            Err(e) => Some(e.to_string()),
                            Ok(resp) => {
                                if !resp.status().is_success() {
                                    Some(format!("Server returned status {}", resp.status()))
                                } else if frag.is_empty() || !check_anchors {
                                    None
                                } else {
                                    let ct = resp
                                        .headers()
                                        .get(reqwest::header::CONTENT_TYPE)
                                        .and_then(|v| v.to_str().ok())
                                        .unwrap_or("")
                                        .split(';')
                                        .next()
                                        .unwrap_or("")
                                        .trim()
                                        .to_lowercase();
                                    if ct == "text/html" || ct == XHTML_MIME {
                                        match resp.bytes() {
                                            Ok(body) => {
                                                let ids = get_html_ids(&body);
                                                if ids.contains(frag) {
                                                    None
                                                } else {
                                                    Some(format!("HTML anchor {frag} not found on the page"))
                                                }
                                            }
                                            Err(e) => Some(e.to_string()),
                                        }
                                    } else {
                                        None
                                    }
                                }
                            }
                        },
                    };
                    if let Some(error) = outcome {
                        results.lock().unwrap().push(ExternalLinkFailure {
                            locations: locations.clone(),
                            error,
                            url: full_href.clone(),
                        });
                    }
                    let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                    (progress_callback.lock().unwrap())(n, total);
                }
            });
        }
    });

    Ok(results.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_container(dir: &Path, opf: &str, files: &[(&str, &[u8])]) -> Container {
        std::fs::write(dir.join("content.opf"), opf).unwrap();
        for (name, data) in files {
            let path = dir.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, data).unwrap();
        }
        Container::open(dir, &dir.join("content.opf")).unwrap()
    }

    const OPF_TEMPLATE_HEAD: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest>
"#;

    #[test]
    fn parse_url_loose_splits_scheme_path_fragment() {
        let u = parse_url_loose("http://example.com/a/b?q=1#frag").unwrap();
        assert_eq!(u.scheme, "http");
        assert_eq!(u.fragment, "frag");

        let u = parse_url_loose("../images/a.png").unwrap();
        assert_eq!(u.scheme, "");
        assert_eq!(u.path, "../images/a.png");

        let u = parse_url_loose("file:///tmp/a.html").unwrap();
        assert_eq!(u.scheme, "file");

        let u = parse_url_loose("/abs/path.html").unwrap();
        assert_eq!(u.scheme, "");
        assert_eq!(u.path, "/abs/path.html");

        assert!(parse_url_loose("http://[::1").is_none());
    }

    #[test]
    fn check_mimetypes_flags_extension_mismatch() {
        let opf = format!(
            "{OPF_TEMPLATE_HEAD}    <item id=\"c1\" href=\"chap1.html\" media-type=\"text/plain\"/>\n  </manifest>\n  <spine/>\n</package>"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), &opf, &[("chap1.html", b"<html/>")]);
        let errors = check_mimetypes(&mut c).unwrap();
        assert!(errors.iter().any(|e| e.type_name == "MimetypeMismatch"));
    }

    #[test]
    fn check_links_flags_dangling_link_and_empty_link() {
        let opf = format!(
            "{OPF_TEMPLATE_HEAD}    <item id=\"c1\" href=\"chap1.html\" media-type=\"application/xhtml+xml\"/>\n  </manifest>\n  <spine><itemref idref=\"c1\"/></spine>\n</package>"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            &opf,
            &[(
                "chap1.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><a href=\"missing.html\">x</a><a href=\"\">y</a></body></html>",
            )],
        );
        let errors = check_links(&mut c).unwrap();
        assert!(errors.iter().any(|e| e.type_name == "DanglingLink"));
        assert!(errors.iter().any(|e| e.type_name == "EmptyLink"));
    }

    #[test]
    fn check_links_flags_unreferenced_and_unmanifested_resources() {
        let opf = format!(
            "{OPF_TEMPLATE_HEAD}    <item id=\"c1\" href=\"chap1.html\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"im1\" href=\"pic.png\" media-type=\"image/png\"/>\n  </manifest>\n  <spine><itemref idref=\"c1\"/></spine>\n</package>"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            &opf,
            &[
                (
                    "chap1.html",
                    b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>hi</p></body></html>",
                ),
                ("pic.png", b"\x89PNG\r\n\x1a\n"),
                ("stray.txt", b"not manifested"),
                ("META-INF/calibre_bookmarks.txt", b"x"),
            ],
        );
        let errors = check_links(&mut c).unwrap();
        // pic.png is manifested but never linked from the spine -> unreferenced.
        assert!(errors
            .iter()
            .any(|e| e.type_name == "UnreferencedResource" && e.name == "pic.png"));
        // stray.txt is on disk but not in the manifest at all.
        assert!(errors
            .iter()
            .any(|e| e.type_name == "Unmanifested" && e.name == "stray.txt"));
        assert!(errors.iter().any(|e| e.type_name == "Bookmarks"));
    }

    #[test]
    fn unreferenced_doc_fix_appends_to_spine() {
        let opf = format!(
            "{OPF_TEMPLATE_HEAD}    <item id=\"c1\" href=\"chap1.html\" media-type=\"application/xhtml+xml\"/>\n    <item id=\"c2\" href=\"chap2.html\" media-type=\"application/xhtml+xml\"/>\n  </manifest>\n  <spine><itemref idref=\"c1\"/></spine>\n</package>"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            &opf,
            &[
                (
                    "chap1.html",
                    b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>hi</p></body></html>",
                ),
                (
                    "chap2.html",
                    b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>hi</p></body></html>",
                ),
            ],
        );
        let mut err = unreferenced_doc("chap2.html");
        assert!(err.apply_fix(&mut c).unwrap());
        let spine_names: Vec<String> = c
            .spine_names()
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(spine_names.contains(&"chap2.html".to_string()));
    }

    #[test]
    fn bookmarks_fix_removes_the_file() {
        let opf = format!("{OPF_TEMPLATE_HEAD}  </manifest>\n  <spine/>\n</package>");
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            &opf,
            &[("META-INF/calibre_bookmarks.txt", b"x")],
        );
        let mut err = bookmarks("META-INF/calibre_bookmarks.txt");
        assert!(err.apply_fix(&mut c).unwrap());
        assert!(!c.has_name("META-INF/calibre_bookmarks.txt"));
    }

    #[test]
    fn check_link_destinations_flags_bad_fragment() {
        let opf = format!(
            "{OPF_TEMPLATE_HEAD}    <item id=\"c1\" href=\"chap1.html\" media-type=\"application/xhtml+xml\"/>\n  </manifest>\n  <spine><itemref idref=\"c1\"/></spine>\n</package>"
        );
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(
            dir.path(),
            &opf,
            &[(
                "chap1.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><a href=\"#nope\">x</a><p id=\"real\">y</p></body></html>",
            )],
        );
        let errors = check_link_destinations(&mut c, "epub").unwrap();
        assert!(errors
            .iter()
            .any(|e| e.type_name == "BadDestinationFragment"));
    }
}
