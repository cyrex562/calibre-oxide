//! Port of `old_src/src/calibre/srv/render_book.py`'s `extract_book`/
//! `process_exploded_book`/`render` (issue #481, the central
//! integration piece of #427's tracking epic): explode a book file
//! into a directory tree, build a container over it, compute TOC/
//! landmarks/cover/spine, run the per-file transforms (#478 `html_as_json`
//! via [`crate::reader_json`], #479 `url()` rewriting via
//! [`crate::css::url_rewrite`], #480 link virtualization via
//! [`crate::link_virtualize`], #488 CSS property-value semantic
//! rewrites -- font-size->rem, page-break fallback, writing-mode rename
//! -- via [`crate::css::property_transform`]) across every HTML/CSS
//! file, and write one `book_render_data` structure as
//! `calibre-book-manifest.json` alongside the transformed files on disk.
//!
//! # Scope: EPUB only, HTML/CSS only, sequential
//!
//! - **Format**: only EPUB (zip or already-exploded directory) is
//!   supported. Upstream dispatches through calibre's full
//!   input-plugin registry; this crate's own
//!   [`crate::oeb::polish::container::get_container`] already only
//!   really supports EPUB/KEPUB (`Azw3Container::open`/`commit` are
//!   `todo!()` stubs -- a pre-existing gap, not something this issue
//!   introduces or fixes). [`extract_book`] checks the extension
//!   itself and returns a real `Err` for anything unsupported rather
//!   than risking a panic by calling into the `todo!()` path.
//! - **File kinds**: only HTML content documents (`OEB_DOCS`) and CSS
//!   (`OEB_STYLES`) go through a real transform. Upstream also
//!   transforms standalone SVG images (`transform_svg_image`) and
//!   SMIL files (`transform_smil`) -- not ported here. Real reason,
//!   not just "ran out of time": standalone SVG parses into this
//!   crate's [`crate::xmltree::Xml`] tree (`ParsedItem::Xml`, used for
//!   any `+xml`/`/xml` mimetype), a completely different type from
//!   the [`crate::dom::Dom`] tree [`crate::link_virtualize`] and
//!   [`crate::reader_json`] are built on -- porting SVG virtualization
//!   for real means a second, `Xml`-based virtualizer, not reusing
//!   this one. SMIL (audio-narration sync files) is a narrow format
//!   real books rarely use. Both are left unrendered (present in the
//!   manifest's `files` map, but not processed into the JSON reader
//!   format) rather than silently mishandled.
//! - **Concurrency**: upstream processes files across a
//!   process/thread pool (`calculate_number_of_workers`,
//!   `forked_map`/`ThreadPoolExecutor`). This port processes files
//!   sequentially -- correct, just not as fast for a large book. A
//!   real, disclosed simplification, not a correctness gap.
//!
//! # Real prior art this builds on
//!
//! [`crate::oeb::polish::container`] (`Container`/`EpubContainer`/
//! `get_container`), [`crate::oeb::polish::toc`] (`get_toc`/
//! `get_landmarks`/`from_xpaths`), [`crate::oeb::polish::cover`]
//! (`find_cover_image`/`find_cover_page`/`find_cover_image_in_page`)
//! are all real, substantial, already-ported infrastructure this
//! module composes rather than rebuilds.
//!
//! # Real prior art now actually wired up
//!
//! [`crate::link_virtualize`] (#480) and [`crate::css::url_rewrite`]
//! (#479) were built as scheme-agnostic, container-agnostic
//! mechanisms precisely so #481 could supply the real decision logic
//! once a real container existed -- [`create_link_replacer`] is that
//! real decision logic (`render_book.py`'s own `create_link_replacer`,
//! resolving a relative href against this container's
//! `href_to_name`/`present_names`).
//!
//! # Not ported: everything past `process_exploded_book`
//!
//! `serialize_metadata`/`extract_annotations` (`render_for_viewer`'s
//! own extras: `calibre-book-metadata.json`, legacy-bookmark-format
//! migration via `get_stored_annotations`), `quicklook`/
//! `quicklook_service`/`viewer_main`/`develop`/`profile` (desktop
//! QuickLook integration and CLI dev tools -- no server relevance)
//! are all out of scope.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::css::property_transform::transform_properties;
use crate::css::url_rewrite::transform_urls;
use crate::dom::{Dom, NodeKind};
use crate::link_virtualize::{anchor_map, disable_non_stylesheet_links, encode_url, process_anchor_links, rewrite_link_attributes};
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::container::{href_to_name_at, AnyContainer, Container, EpubContainer, KepubContainer};
use crate::oeb::polish::cover::{find_cover_image, find_cover_image_in_page, find_cover_page};
use crate::oeb::polish::toc::{from_xpaths, get_landmarks, get_toc, Toc, TocNodeId};
use crate::reader_json::serialize_document;

/// Bumped whenever this render pipeline's own output shape changes,
/// to invalidate every existing cache entry -- matches upstream's
/// `render_book.RENDER_VERSION`. `calibre_srv::books_cache` (issue
/// #482) references this same constant rather than defining its own.
pub const RENDER_VERSION: u32 = 1;

const COVER_PAGE_TEMPLATE: &str = r#"
<html xmlns="http://www.w3.org/1999/xhtml" xml:lang="en">
<head><style>
html, body, img { height: 100vh; display: block; margin: 0; padding: 0; border-width: 0; }
img {
    width: 100%; height: 100%;
    object-fit: contain;
    margin-left: auto; margin-right: auto;
    max-width: 100vw; max-height: 100vh;
    top: 50vh; transform: translateY(-50%);
    position: relative;
}
body.cover-fill img { object-fit: fill; }
</style></head><body><img src="{}"/></body></html>
"#;

/// Book has no content in its spine -- matches upstream's `Spineless`.
#[derive(Debug, thiserror::Error)]
#[error("Book is empty, no content in spine")]
pub struct Spineless;

/// Port of `extract_book`: opens `path` (a `.epub`/`.kepub` file, or
/// an already-exploded directory) into `output_dir`, returning the
/// opened container and its format info. See this module's own doc
/// for why only EPUB/KEPUB are supported.
pub fn extract_book(path: &Path, output_dir: &Path) -> Result<(AnyContainer, String, String)> {
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    let is_dir = path.is_dir();
    if !is_dir && matches!(ext.as_str(), "azw3" | "mobi" | "original_azw3" | "original_mobi") {
        anyhow::bail!("The format {ext} is not yet supported by this render pipeline (AZW3/MOBI explosion is an existing, separate unported gap in oeb::polish::container::Azw3Container)");
    }
    let any = if !is_dir && matches!(ext.as_str(), "kepub" | "original_kepub") {
        AnyContainer::Kepub(KepubContainer::open_zip(path, output_dir)?)
    } else if is_dir {
        AnyContainer::Epub(EpubContainer::open_dir(path, output_dir)?)
    } else {
        AnyContainer::Epub(EpubContainer::open_zip(path, output_dir)?)
    };
    let input_fmt = if ext.is_empty() { "epub".to_string() } else { ext };
    let book_fmt = input_fmt.to_uppercase();
    Ok((any, book_fmt, input_fmt))
}

/// Port of `create_link_replacer`: resolves `href` (found on document
/// `base`) against the real manifest, returning the
/// `link_uid|base64(name)#frag|`-virtualized form for a same-page
/// fragment or a present resource, `missing:name` for a resolvable
/// but absent resource, or `None` to leave `href` unchanged (external,
/// has a query/netloc, non-`file` scheme, or an absolute path).
/// `changed` is populated with every `base` this call actually
/// virtualizes something for, matching upstream's own `changed.add(base)`
/// bookkeeping.
///
/// Takes `root` (a plain path) rather than a live `&Container`
/// borrow: callers need this closure alive at the same time as a
/// `&mut Container` borrow elsewhere (mutating a parsed document
/// while resolving its own links), and [`href_to_name_at`] -- the
/// same free function `Container::href_to_name` itself delegates to
/// -- only ever needed `root` anyway.
///
/// Runs its own guards (scheme/netloc/query/absolute-path) *before*
/// calling [`href_to_name_at`], rather than relying on that
/// function's own leniency here: `href_to_name_at` treats a
/// leading `/` as if it were relative (silently skips the leading
/// empty path segment) and doesn't reject a query string at all,
/// both different from upstream's own explicit "leave alone" guards
/// for those cases -- a real, narrow difference from
/// `Container::href_to_name`'s existing behavior elsewhere in this
/// crate, not a bug in it (nothing else needs the stricter guards).
fn create_link_replacer<'a>(root: &'a Path, link_uid: &'a str, present_names: &'a HashSet<String>, changed: &'a mut HashSet<String>) -> impl FnMut(&str, &str) -> Option<String> + 'a {
    move |base: &str, href: &str| {
        if let Some(frag) = href.strip_prefix('#') {
            changed.insert(base.to_string());
            let frag = crate::lit::urlunquote(frag);
            if frag.is_empty() {
                return Some(link_uid.to_string());
            }
            return Some(format!("{link_uid}|{}|", encode_url(base, &frag)));
        }

        let (scheme, rest) = split_scheme(href);
        if let Some(scheme) = scheme {
            if !scheme.eq_ignore_ascii_case("file") {
                return None;
            }
        }
        if rest.starts_with("//") {
            return None; // netloc: protocol-relative or scheme://host/...
        }
        let (before_frag, frag) = match rest.split_once('#') {
            Some((p, f)) => (p, f),
            None => (rest, ""),
        };
        let (path_part, has_query) = match before_frag.split_once('?') {
            Some((p, _)) => (p, true),
            None => (before_frag, false),
        };
        if has_query || path_part.is_empty() || path_part.starts_with('/') {
            return None;
        }

        let name = href_to_name_at(href, root, Some(base));
        changed.insert(base.to_string());
        match name {
            Some(name) if present_names.contains(&name) => {
                let frag = crate::lit::urlunquote(frag);
                Some(format!("{link_uid}|{}|", encode_url(&name, &frag)))
            }
            Some(name) => Some(format!("missing:{name}")),
            None => None,
        }
    }
}

/// A minimal `scheme:rest` splitter (RFC 3986 `scheme` grammar:
/// letter, then letters/digits/`+`/`.`/`-`) -- `href_to_name_at`'s own
/// equivalent (`split_scheme`) isn't `pub`, so this is a small,
/// separate copy rather than a shared dependency on container.rs's
/// private internals.
fn split_scheme(href: &str) -> (Option<&str>, &str) {
    let bytes = href.as_bytes();
    if bytes.is_empty() || !bytes[0].is_ascii_alphabetic() {
        return (None, href);
    }
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b':' if i > 0 => return (Some(&href[..i]), &href[i + 1..]),
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'.' | b'-' => continue,
            _ => break,
        }
    }
    (None, href)
}

fn check_for_maths(dom: &crate::dom::Dom) -> bool {
    if !dom.find_all_tag_global("math").is_empty() {
        return true;
    }
    dom.find_all_tag_global("script").iter().any(|&id| dom.node(id).attrs.get("type").map(|t| t == "text/x-mathjax-config").unwrap_or(false))
}

/// Port of `get_length`, simplified: upstream sums each descendant's
/// own `.text`/`.tail` string lengths, which (since every text node
/// in a subtree is exactly one element's `.text` or exactly one
/// element's `.tail`) is equivalent to the total character count of
/// every text node under `<body>` -- [`crate::dom::Dom::text_content`]
/// already computes exactly that.
fn get_length(dom: &crate::dom::Dom, body: crate::dom::NodeId) -> usize {
    dom.text_content(body).chars().count()
}

fn find_body(dom: &crate::dom::Dom) -> Option<crate::dom::NodeId> {
    dom.find_first_tag_global("body")
}

/// Port of `find_epub_cover`.
fn find_epub_cover(container: &mut Container) -> Result<(Option<String>, Option<String>)> {
    let cover_image = find_cover_image(container, false)?;
    let marked_title_page = find_cover_page(container)?;
    let mut cover_image_in_first_page = None;
    let Some((first_page_name, _)) = container.spine_names()?.into_iter().next() else {
        return Ok((None, None));
    };
    if marked_title_page.is_none() {
        cover_image_in_first_page = find_cover_image_in_page(container, &first_page_name)?;
    }

    let has_epub_cover = cover_image.is_some() || marked_title_page.is_some() || cover_image_in_first_page.is_some();
    if !has_epub_cover {
        return Ok((None, None));
    }
    if let (Some(mtp), Some(ci)) = (&marked_title_page, &cover_image) {
        return Ok((Some(mtp.clone()), Some(ci.clone())));
    }
    if let Some(mtp) = marked_title_page {
        if let Some(ci) = cover_image {
            return Ok((Some(mtp), Some(ci)));
        }
        let ci = find_cover_image_in_page(container, &mtp)?;
        return Ok(if ci.is_some() { (Some(mtp), ci) } else { (None, None) });
    }
    if let Some(ci) = cover_image_in_first_page {
        return Ok((Some(first_page_name), Some(ci)));
    }
    Ok((None, None))
}

/// Port of `create_cover_page`'s EPUB branch (see this module's own
/// doc for the non-EPUB branch, not ported -- format detection
/// upstream needs for it, comic-collection input, doesn't exist in
/// this port yet).
fn create_cover_page(container: &mut Container) -> Result<(Option<String>, Option<String>)> {
    let (titlepage_name, raster_cover_name) = find_epub_cover(container)?;
    if let (Some(raster), Some(titlepage)) = (&raster_cover_name, &titlepage_name) {
        let href = container.name_to_href(raster, Some(titlepage));
        let raw = COVER_PAGE_TEMPLATE.replace("{}", &html_escape::encode_double_quoted_attribute(&href));
        container.write_file(titlepage, raw.as_bytes())?;
    }
    Ok((raster_cover_name, titlepage_name))
}

/// Port of `TOC.to_dict`: `{title, dest, frag, children: [...]}` plus
/// `dest_exists`/`dest_error` when set, and a post-order `id` (every
/// child's own `id` assigned before its parent's, matching upstream's
/// own `next(node_counter)` call ordering).
fn toc_to_dict(toc: &Toc, id: TocNodeId, counter: &mut i64) -> Value {
    let node = toc.node(id);
    let children: Vec<Value> = toc.children(id).iter().map(|&c| toc_to_dict(toc, c, counter)).collect();
    let mut obj = serde_json::Map::new();
    obj.insert("title".to_string(), json!(node.title));
    obj.insert("dest".to_string(), json!(node.dest));
    obj.insert("frag".to_string(), json!(node.frag));
    obj.insert("children".to_string(), Value::Array(children));
    if let Some(exists) = node.dest_exists {
        obj.insert("dest_exists".to_string(), json!(exists));
    }
    if let Some(err) = &node.dest_error {
        obj.insert("dest_error".to_string(), json!(err));
    }
    obj.insert("id".to_string(), json!(*counter));
    *counter += 1;
    Value::Object(obj)
}

/// Port of `toc_anchor_map`: `dest name -> [{id, frag}]` for every
/// node with a real `dest`, first occurrence per `(dest, id)` pair
/// only.
fn toc_anchor_map(toc_json: &Value) -> HashMap<String, Vec<Value>> {
    let mut ans: HashMap<String, Vec<Value>> = HashMap::new();
    let mut seen: HashSet<(String, Value)> = HashSet::new();
    fn walk(node: &Value, ans: &mut HashMap<String, Vec<Value>>, seen: &mut HashSet<(String, Value)>) {
        if let Some(name) = node["dest"].as_str() {
            let id = node["id"].clone();
            let key = (name.to_string(), id.clone());
            if seen.insert(key) {
                ans.entry(name.to_string()).or_default().push(json!({"id": id, "frag": node["frag"]}));
            }
        }
        if let Some(children) = node["children"].as_array() {
            for c in children {
                walk(c, ans, seen);
            }
        }
    }
    walk(toc_json, &mut ans, &mut seen);
    ans
}

/// Port of `pagelist_anchor_map`: `dest name -> [{id, pagenum, frag}]`,
/// `id` assigned as a real 1-based sequence number over `page_list`
/// (matching upstream's `enumerate(page_list)`), first occurrence per
/// `(dest, frag)` pair only.
fn pagelist_anchor_map(page_list: &[crate::oeb::polish::toc::PageListEntry]) -> (HashMap<String, Vec<Value>>, Value) {
    let mut ans: HashMap<String, Vec<Value>> = HashMap::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut out_list = Vec::new();
    for (i, entry) in page_list.iter().enumerate() {
        let id = (i + 1) as i64;
        let frag = entry.frag.clone().unwrap_or_default();
        out_list.push(json!({"dest": entry.dest, "pagenum": entry.pagenum, "frag": entry.frag, "id": id}));
        if let Some(name) = &entry.dest {
            let key = (name.clone(), frag.clone());
            if seen.insert(key) {
                ans.entry(name.clone()).or_default().push(json!({"id": id, "pagenum": entry.pagenum, "frag": entry.frag}));
            }
        }
    }
    (ans, Value::Array(out_list))
}

/// One file's real per-file transform result: whether it was
/// virtualized, and (for HTML) its content length/maths/anchor data.
struct FileResult {
    virtualized: bool,
    html_data: Option<(usize, bool, Vec<String>)>,
}

/// Port of `process_book_file`, HTML/CSS branches only (see this
/// module's own doc for SVG/SMIL). `root` is the container's own
/// root path, threaded through to [`create_link_replacer`] -- see
/// that function's own doc for why it takes a plain path rather than
/// a live `&Container` borrow.
fn process_book_file(container: &mut Container, name: &str, root: &Path, link_uid: &str, present_names: &HashSet<String>, link_to_map: &mut HashMap<String, HashMap<String, HashSet<String>>>) -> Result<FileResult> {
    let mime = container.base.mime_map.get(name).cloned().unwrap_or_default().to_lowercase();
    container.ensure_parsed(name)?;

    if OEB_DOCS.contains(&mime.as_str()) {
        // 1. Content stats, collected before any virtualization (matches upstream's own call order).
        let (length, has_maths) = {
            let dom = container.get_xhtml(name)?;
            let length = find_body(dom).map(|b| get_length(dom, b)).unwrap_or(0);
            (length, check_for_maths(dom))
        };

        // 2. anchor_map (mutates: promotes a bare <a name> to a real id) + non-stylesheet <link> suppression.
        let anchors = {
            let dom = container.get_xhtml_mut(name)?;
            let root_id = dom.root;
            let anchors = anchor_map(dom, root_id);
            disable_non_stylesheet_links(dom, root_id);
            anchors
        };

        // 3. Inline <style>/style="" -- url() rewriting (#479) with no
        //    real callback (upstream's own comment claims these get
        //    resolved in virtualize_html, but doesn't actually pass a
        //    real url_callback into transform_properties for them
        //    either -- matched here), plus the #488 property-value
        //    semantic rewrites, which upstream's own single combined
        //    transform_properties call always applies regardless of
        //    whether a real url_callback was given.
        transform_inline_styles(container.get_xhtml_mut(name)?);

        // 4. Link virtualization: rewrite href/src/etc, then the <a>/<area> post-processing pass.
        let mut changed = HashSet::new();
        {
            let mut replacer = create_link_replacer(root, link_uid, present_names, &mut changed);
            let dom = container.get_xhtml_mut(name)?;
            let root_id = dom.root;
            rewrite_link_attributes(dom, root_id, |href| replacer(name, href));
        }
        {
            let dom = container.get_xhtml_mut(name)?;
            let root_id = dom.root;
            process_anchor_links(dom, root_id, link_uid, name, link_to_map);
        }

        // 5. Serialize to the reader's JSON format, replacing this name's on-disk HTML content.
        let json_bytes = {
            let dom = container.get_xhtml(name)?;
            let html_root = dom.find_first_tag_global("html").unwrap_or(dom.root);
            serialize_document(dom, html_root).to_string().into_bytes()
        };
        container.write_file(name, &json_bytes)?;

        Ok(FileResult { virtualized: changed.contains(name), html_data: Some((length, has_maths, anchors)) })
    } else if OEB_STYLES.contains(&mime.as_str()) {
        let raw = container.get_css_text(name)?.to_string();
        let mut changed_names = HashSet::new();
        let after_urls = {
            let mut replacer = create_link_replacer(root, link_uid, present_names, &mut changed_names);
            transform_urls(&raw, |url| replacer(name, url))
        };
        let new_raw = transform_properties(&after_urls);
        let mut changed = new_raw != raw;
        let trimmed = new_raw.trim_start();
        let final_raw = if trimmed.starts_with("@charset") {
            trimmed.to_string()
        } else {
            changed = true;
            format!("@charset \"UTF-8\";\n{trimmed}")
        };
        if changed {
            container.write_file(name, final_raw.as_bytes())?;
        }
        Ok(FileResult { virtualized: changed_names.contains(name), html_data: None })
    } else {
        Ok(FileResult { virtualized: false, html_data: None })
    }
}

fn transform_inline_styles(dom: &mut Dom) {
    for id in dom.preorder_elements(dom.root) {
        if dom.tag(id) == Some("style") {
            if let Some(first_child) = dom.node(id).children.first().copied() {
                if let NodeKind::Text(text) = dom.node(first_child).kind.clone() {
                    let new_text = transform_properties(&transform_urls(&text, |_| None));
                    dom.node_mut(first_child).kind = NodeKind::Text(new_text);
                }
            }
        }
        if let Some(style) = dom.node(id).attrs.get("style").cloned() {
            let new_style = transform_properties(&transform_urls(&style, |_| None));
            dom.node_mut(id).attrs.insert("style".to_string(), new_style);
        }
    }
}

/// Everything `process_exploded_book` builds, matching
/// `book_render_data`'s real shape.
#[derive(Debug)]
pub struct BookRenderData {
    pub link_uid: String,
    pub json: Value,
}

/// Port of `process_exploded_book`. See this module's own doc for
/// what's not ported (SVG/SMIL per-file transforms, parallelism).
pub fn process_exploded_book(container: &mut Container, book_fmt: &str, book_hash: Option<&str>, virtualize_resources: bool) -> Result<BookRenderData> {
    let mut excluded_names = HashSet::new();
    let mut present_names = HashSet::new();
    let opf_name = container.opf_name.clone();
    for (name, mt) in container.base.mime_map.clone() {
        if container.has_name_and_is_not_empty(&name) {
            present_names.insert(name.clone());
            if name == opf_name || mt == crate::oeb::constants::NCX_MIME || name.starts_with("META-INF/") || name == "mimetype" {
                excluded_names.insert(name);
            }
        } else {
            excluded_names.insert(name);
        }
    }

    let (raster_cover_name, title_page_name) = create_cover_page(container)?;

    let mut toc = get_toc(container, false)?;
    let page_list = toc.page_list.clone();
    let mut counter = 0i64;
    let mut toc_json = toc_to_dict(&toc, toc.root, &mut counter);
    if toc_json["children"].as_array().map(|a| a.is_empty()).unwrap_or(true) {
        toc = from_xpaths(container, &["//h:h1", "//h:h2", "//h:h3"], false)?;
        counter = 0;
        toc_json = toc_to_dict(&toc, toc.root, &mut counter);
    }

    let spine: Vec<String> = container.spine_names()?.into_iter().map(|(n, _)| n).collect();
    let spine_set: HashSet<String> = spine.iter().cloned().collect();
    if spine_set.is_empty() {
        return Err(Spineless.into());
    }
    let landmarks: Vec<_> = get_landmarks(container)?.into_iter().filter(|l| spine_set.contains(&l.dest)).collect();

    let page_progression_direction = {
        let spine_els = container.opf_xpath("//opf:spine")?;
        match spine_els.first() {
            Some(&id) => container.get_xml(&opf_name)?.get_attr(id, "page-progression-direction").map(str::to_string),
            None => None,
        }
    };

    let link_uid = uuid::Uuid::new_v4().to_string();
    let (page_list_anchor_map_val, page_list_json) = pagelist_anchor_map(&page_list);

    let mut link_to_map: HashMap<String, HashMap<String, HashSet<String>>> = HashMap::new();
    let mut html_lengths: HashMap<String, usize> = HashMap::new();
    let mut html_has_maths: HashMap<String, bool> = HashMap::new();
    let mut has_maths_any = false;
    let mut anchor_maps: HashMap<String, Vec<String>> = HashMap::new();
    let mut virtualized_names: HashSet<String> = HashSet::new();

    let names_that_need_work: Vec<String> = container
        .base
        .mime_map
        .iter()
        .filter(|(_, mt)| OEB_DOCS.contains(&mt.as_str()) || OEB_STYLES.contains(&mt.as_str()))
        .map(|(n, _)| n.clone())
        .collect();

    let mut total_length = 0usize;
    let mut spine_length = 0usize;
    let mut files = serde_json::Map::new();
    let root_path = container.root.clone();

    for name in &names_that_need_work {
        if !virtualize_resources {
            continue;
        }
        let result = process_book_file(container, name, &root_path, &link_uid, &present_names, &mut link_to_map)?;
        if result.virtualized {
            virtualized_names.insert(name.clone());
        }
        if let Some((length, has_maths, anchors)) = result.html_data {
            html_lengths.insert(name.clone(), length);
            html_has_maths.insert(name.clone(), has_maths);
            if has_maths {
                has_maths_any = true;
            }
            anchor_maps.insert(name.clone(), anchors);
            total_length += length;
            if spine_set.contains(name) {
                spine_length += length;
            }
        }
    }

    for name in container.name_path_map.keys().cloned().collect::<Vec<_>>() {
        if excluded_names.contains(&name) {
            continue;
        }
        let mt = container.base.mime_map.get(&name).cloned().unwrap_or_else(|| "application/octet-stream".to_string()).to_lowercase();
        let is_html = OEB_DOCS.contains(&mt.as_str());
        let size = std::fs::metadata(&container.name_path_map[&name]).map(|m| m.len()).unwrap_or(0);
        let mut entry = json!({
            "size": size,
            "is_virtualized": virtualized_names.contains(&name),
            "mimetype": mt,
            "is_html": is_html,
        });
        if is_html {
            entry["length"] = json!(html_lengths.get(&name).copied().unwrap_or(0));
            entry["has_maths"] = json!(html_has_maths.get(&name).copied().unwrap_or(false));
            entry["anchor_map"] = json!(anchor_maps.get(&name).cloned().unwrap_or_default());
        }
        files.insert(name, entry);
    }

    let link_to_map_json: serde_json::Map<String, Value> = link_to_map
        .into_iter()
        .map(|(name, frags)| {
            let frags_json: serde_json::Map<String, Value> = frags.into_iter().map(|(frag, referrers)| (frag, json!(referrers.into_iter().collect::<Vec<_>>()))).collect();
            (name, Value::Object(frags_json))
        })
        .collect();

    let book_render_data = json!({
        "version": RENDER_VERSION,
        "toc": toc_json,
        "book_format": book_fmt,
        "spine": spine,
        "link_uid": link_uid,
        "book_hash": book_hash,
        "is_comic": false,
        "raster_cover_name": raster_cover_name,
        "title_page_name": title_page_name,
        "has_maths": has_maths_any,
        "total_length": total_length,
        "spine_length": spine_length,
        "toc_anchor_map": toc_anchor_map(&toc_json),
        "landmarks": landmarks.iter().map(|l| json!({"dest": l.dest, "frag": l.frag, "title": l.title, "type": l.r#type})).collect::<Vec<_>>(),
        "link_to_map": link_to_map_json,
        "page_progression_direction": page_progression_direction,
        "page_list": page_list_json,
        "page_list_anchor_map": page_list_anchor_map_val,
        "has_smil": false,
        "files": files,
    });

    for name in &excluded_names {
        if let Some(path) = container.name_path_map.get(name) {
            let _ = std::fs::remove_file(path);
        }
    }

    let manifest_path = container.root.join("calibre-book-manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&book_render_data).context("serializing book_render_data")?)?;

    Ok(BookRenderData { link_uid, json: book_render_data })
}

/// Port of `render`, narrowed: no `serialize_metadata`/
/// `extract_annotations` (see this module's own doc).
pub fn render(path_to_ebook: &Path, output_dir: &Path, book_hash: Option<&str>, virtualize_resources: bool) -> Result<BookRenderData> {
    let (mut any, book_fmt, _input_fmt) = extract_book(path_to_ebook, output_dir)?;
    process_exploded_book(any.as_container_mut(), &book_fmt, book_hash, virtualize_resources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A small, real two-chapter EPUB directory: `chap1.xhtml` links
    /// to `chap2.xhtml#target` (internal), `missing.xhtml` (a broken
    /// link), and an external URL; both chapters pull in `style.css`,
    /// which itself references `cover.jpg` via `url()`. No EPUB3 nav
    /// document -- exercises the `from_xpaths` (`h1`/`h2`/`h3`)
    /// TOC-synthesis fallback, matching what most real-world EPUB2
    /// content (no real nav doc) hits in practice.
    fn write_test_book(dir: &Path) {
        fs::create_dir_all(dir.join("META-INF")).unwrap();
        fs::write(
            dir.join("META-INF/container.xml"),
            r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        fs::write(
            dir.join("content.opf"),
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="chap2.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="cover-img" href="cover.jpg" media-type="image/jpeg"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.xhtml"),
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" type="text/css" href="style.css"/></head><body>
<h1>Chapter One</h1>
<p>Some text <a href="chap2.xhtml#target">go to chapter two</a>.</p>
<p><a href="missing.xhtml">a broken link</a></p>
<p><a href="https://example.com">an external link</a></p>
</body></html>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap2.xhtml"),
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" type="text/css" href="style.css"/></head><body>
<h1 id="target">Chapter Two</h1>
<p>The end.</p>
<math><mi>x</mi></math>
</body></html>"#,
        )
        .unwrap();
        fs::write(dir.join("style.css"), r#".c { background: url(cover.jpg) }"#).unwrap();
        fs::write(dir.join("cover.jpg"), [0xFFu8, 0xD8, 0xFF, 0xE0]).unwrap();
    }

    fn open_test_book() -> (tempfile::TempDir, tempfile::TempDir, EpubContainer) {
        let src = tempfile::tempdir().unwrap();
        write_test_book(src.path());
        let tdir = tempfile::tempdir().unwrap();
        let epub = EpubContainer::open_dir(src.path(), tdir.path()).unwrap();
        (src, tdir, epub)
    }

    #[test]
    fn extract_book_rejects_azw3_cleanly_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.azw3");
        fs::write(&path, b"not a real azw3 file").unwrap();
        let out = tempfile::tempdir().unwrap();
        let result = extract_book(&path, out.path());
        let Err(err) = result else { panic!("expected an error for an azw3 path") };
        assert!(err.to_string().contains("azw3"), "{err}");
    }

    #[test]
    fn process_exploded_book_produces_a_real_manifest_with_spine_and_files() {
        let (_src, _tdir, mut epub) = open_test_book();
        let data = process_exploded_book(&mut epub.container, "EPUB", Some("deadbeef"), true).unwrap();

        assert_eq!(data.json["version"], RENDER_VERSION);
        assert_eq!(data.json["book_format"], "EPUB");
        assert_eq!(data.json["book_hash"], "deadbeef");
        assert_eq!(data.json["spine"], json!(["chap1.xhtml", "chap2.xhtml"]));
        assert_eq!(data.json["files"]["chap1.xhtml"]["is_html"], true);
        assert_eq!(data.json["files"]["style.css"]["is_html"], false);
        assert!(data.json["files"]["chap1.xhtml"]["is_virtualized"].as_bool().unwrap());
        assert!(data.json["total_length"].as_u64().unwrap() > 0);

        // Per-file has_maths (chap2 has a real <math> element, chap1
        // doesn't) is real, distinct data -- not just a copy of the
        // whole-book has_maths flag.
        assert_eq!(data.json["files"]["chap1.xhtml"]["has_maths"], false);
        assert_eq!(data.json["files"]["chap2.xhtml"]["has_maths"], true);
        assert_eq!(data.json["has_maths"], true, "the whole-book flag should be true if any file has maths");

        // The manifest file itself was really written to disk.
        let manifest = fs::read_to_string(epub.container.root.join("calibre-book-manifest.json")).unwrap();
        let reparsed: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(reparsed["book_hash"], "deadbeef");
    }

    #[test]
    fn falls_back_to_heading_derived_toc_when_there_is_no_real_nav_doc() {
        let (_src, _tdir, mut epub) = open_test_book();
        let data = process_exploded_book(&mut epub.container, "EPUB", None, true).unwrap();
        let children = data.json["toc"]["children"].as_array().unwrap();
        assert_eq!(children.len(), 2, "one entry per <h1>, from the from_xpaths fallback");
        assert_eq!(children[0]["title"], "Chapter One");
        assert_eq!(children[1]["title"], "Chapter Two");
    }

    #[test]
    fn an_internal_link_is_virtualized_and_recorded_in_link_to_map() {
        let (_src, tdir, mut epub) = open_test_book();
        let data = process_exploded_book(&mut epub.container, "EPUB", None, true).unwrap();
        let link_uid = data.json["link_uid"].as_str().unwrap();

        let ltm = &data.json["link_to_map"]["chap2.xhtml"];
        assert!(ltm.get("target").is_some(), "link_to_map: {ltm}");
        let referrers = ltm["target"].as_array().unwrap();
        assert!(referrers.iter().any(|v| v == "chap1.xhtml"));

        // The rewritten chap1.xhtml on disk is now JSON, containing a
        // real data-{link_uid} blob for the internal link.
        let raw = fs::read_to_string(tdir.path().join("chap1.xhtml")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(doc["version"], 1);
        let doc_str = doc.to_string();
        assert!(doc_str.contains(&format!("data-{link_uid}")), "{doc_str}");
        assert!(doc_str.contains("javascript:void(0)"), "{doc_str}");
    }

    #[test]
    fn a_broken_link_is_marked_missing_and_an_external_link_is_left_alone() {
        let (_src, tdir, mut epub) = open_test_book();
        let data = process_exploded_book(&mut epub.container, "EPUB", None, true).unwrap();
        let link_uid = data.json["link_uid"].as_str().unwrap();

        let raw = fs::read_to_string(tdir.path().join("chap1.xhtml")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        let anchors = find_anchors(&doc["tree"]);

        let missing = anchors.iter().find(|a| a["x"] == "a broken link").expect("the broken-link anchor");
        assert_eq!(node_attr(missing, "href"), Some("javascript:void(0)".to_string()));
        let data_attr = node_attr(missing, &format!("data-{link_uid}")).expect("a data-{link_uid} attribute");
        let missing_data: Value = serde_json::from_str(&data_attr).unwrap();
        assert_eq!(missing_data["missing"], true, "{missing_data}");
        assert_eq!(missing_data["name"], "missing.xhtml");

        let external = anchors.iter().find(|a| a["x"] == "an external link").expect("the external link anchor");
        assert_eq!(node_attr(external, "href"), Some("https://example.com".to_string()), "external links are left as real hrefs: {external}");
        assert_eq!(node_attr(external, "target"), Some("_blank".to_string()));
    }

    /// Walks a `reader_json`-shaped tree collecting every `<a>` node.
    fn find_anchors(node: &Value) -> Vec<&Value> {
        let mut out = Vec::new();
        if node["n"] == "a" {
            out.push(node);
        }
        if let Some(children) = node["c"].as_array() {
            for c in children {
                out.extend(find_anchors(c));
            }
        }
        out
    }

    /// Looks up `name`'s value in a `reader_json`-shaped node's `"a"`
    /// (attribute) array -- `[[name, value], ...]`.
    fn node_attr(node: &Value, name: &str) -> Option<String> {
        node["a"].as_array()?.iter().find(|pair| pair[0] == name)?[1].as_str().map(str::to_string)
    }

    #[test]
    fn inline_css_url_is_rewritten_and_gets_a_real_charset_prelude() {
        let (_src, tdir, mut epub) = open_test_book();
        process_exploded_book(&mut epub.container, "EPUB", None, true).unwrap();

        let raw = fs::read_to_string(tdir.path().join("style.css")).unwrap();
        assert!(raw.starts_with("@charset \"UTF-8\";"), "{raw}");
        assert!(raw.contains("url(\""), "the bare url(cover.jpg) should have been rewritten to a quoted form: {raw}");
        assert!(!raw.contains("url(cover.jpg)"), "the original unresolved form should be gone: {raw}");
    }

    #[test]
    fn a_book_with_an_empty_spine_is_a_real_error() {
        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("META-INF")).unwrap();
        fs::write(
            src.path().join("META-INF/container.xml"),
            r#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#,
        )
        .unwrap();
        fs::write(
            src.path().join("content.opf"),
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid"><metadata><dc:title>Empty</dc:title><dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier></metadata><manifest></manifest><spine></spine></package>"#,
        )
        .unwrap();
        let tdir = tempfile::tempdir().unwrap();
        let mut epub = EpubContainer::open_dir(src.path(), tdir.path()).unwrap();
        let err = process_exploded_book(&mut epub.container, "EPUB", None, true).unwrap_err();
        assert!(err.to_string().contains("Spineless") || err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn render_end_to_end_from_a_real_directory_path() {
        let src = tempfile::tempdir().unwrap();
        write_test_book(src.path());
        let out = tempfile::tempdir().unwrap();
        let data = render(src.path(), out.path(), Some("hash123"), true).unwrap();
        assert_eq!(data.json["book_hash"], "hash123");
        assert!(out.path().join("calibre-book-manifest.json").exists());
    }
}
