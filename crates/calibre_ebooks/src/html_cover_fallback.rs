//! Port of `calibre.ebooks`'s cover-rasterization fallback chain
//! (`old_src/src/calibre/ebooks/__init__.py`'s `return_raster_image`,
//! `extract_cover_from_embedded_svg`, `extract_calibre_cover`, and
//! `render_html_svg_workaround`) -- issue #119, the achievable slice of
//! it. Used across 3 real call sites (EPUB conversion cover generation,
//! EPUB metadata cover extraction, OEB reader HTML-cover resolution) as
//! the last-resort extractor for a "cover page" that's HTML/XHTML
//! rather than already a plain raster image.
//!
//! # Ported for real: the two pure-parsing pre-fallbacks
//!
//! `extract_cover_from_embedded_svg` (an inline `<svg><image
//! xlink:href="..."/></svg>` cover, exactly one of each) and
//! `extract_calibre_cover` (a body with no heading/paragraph/text
//! content and exactly one `<img>`) need only HTML parsing and
//! attribute inspection -- no browser engine. Both are real, tested
//! ports here.
//!
//! # NOT ported: the final Qt-WebEngine rendering fallback
//!
//! Real `render_html_svg_workaround`'s own final fallback --
//! `render_html_data`, which forks a `calibre.ebooks.render_html`
//! subprocess to lay out the HTML with actual CSS via Qt WebEngine,
//! print it to PDF, and rasterize that PDF page -- needs real
//! browser-engine-grade CSS layout. Nothing in this crate provides
//! that: the wry/tao-based scraper worker (issue #58) has no
//! screenshot/print-to-PDF capability wired up (its own protocol is
//! `fetch`/`set_cookie`/`quit` only), `crate::pdf::render` is a PDF
//! *writer* (the wrong direction) whose own `Graphics` type is blocked
//! on a live Qt paint engine, and `crate::covers`/`covers_text` (issue
//! #116) is a narrowly-scoped generative cover-art renderer, not a
//! general HTML/CSS layout engine. Building this for real would mean
//! either extending the scraper worker with new native GTK/webkit2gtk
//! screenshot FFI (a project on the scale of #58 itself) or writing an
//! actual CSS layout engine from scratch -- both disproportionate to
//! this issue, and exactly the kind of "don't build a second browser
//! engine" call this project has made consistently elsewhere (see
//! `crate::css::selector`'s own module doc). [`render_html_svg_workaround`]
//! therefore returns `None` when neither pre-fallback applies, matching
//! `crate::pdf::html_writer`'s own already-disclosed precedent for the
//! identical underlying gap.
//!
//! Real `render_html_svg_workaround`'s `width`/`height`/`log` parameters
//! are dropped here -- they're only ever consumed by the unported final
//! fallback, so there's nothing in this port for them to do.
//!
//! # A resolver callback instead of a literal filesystem path
//!
//! Real upstream always operates on files already extracted to a temp
//! directory (every real caller extracts the whole EPUB to disk first).
//! [`extract_cover_from_embedded_svg`]/[`extract_calibre_cover`] instead
//! take a `resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>` callback --
//! "given a path relative to the cover page, return that resource's raw
//! bytes" -- so a caller that already has the book open as a zip archive
//! (this port's `metadata::epub::get_metadata`) can read the one
//! referenced image directly from the archive instead of extracting the
//! whole book to disk just to satisfy a path-based API. [`filesystem_resolver`]
//! provides the straightforward disk-backed implementation for callers
//! that do extract to disk, matching real upstream's own approach.
//!
//! # A real, pre-existing bug found and fixed while porting this
//!
//! `crate::metadata::epub::get_metadata` resolved a manifest cover item
//! by reading its raw bytes directly, with no check that the item was
//! actually a raster image -- if the cover manifest entry pointed at an
//! HTML/XHTML titlepage (a real, valid EPUB shape this whole fallback
//! chain exists to handle), the HTML source text was silently returned
//! as `cover_data`, mislabeled as image bytes. Fixed by routing through
//! this module's fallback chain when the resolved cover item isn't a
//! recognized raster format.

use crate::chardet::xml_to_unicode;
use crate::dom::{Dom, NodeId, NodeKind};
use std::path::{Path, PathBuf};

const SVG_NS: &str = "http://www.w3.org/2000/svg";

const MARKER_TAGS: &[&str] = &["h1", "h2", "h3", "h4", "h5", "h6", "p", "span", "font", "br"];

/// Port of `return_raster_image`'s own `imghdr.what(None, raw) not in
/// (None, 'svg')` check. `calibre_utils::imghdr::what` never returns
/// `"svg"` (real Python's `imghdr` doesn't either, in practice -- that
/// branch is defensive/vestigial upstream too), so this collapses to a
/// plain "is this a recognized raster format" check.
fn raster_or_none(bytes: Vec<u8>) -> Option<Vec<u8>> {
    calibre_utils::imghdr::what(&bytes)?;
    Some(bytes)
}

/// Port of `os.path.join(base, *href.split('/'))`: splits on a literal
/// `/` (not the OS separator) before joining, matching how these hrefs
/// are always written (HTML/OPF paths, never OS-native).
fn join_relative(base: &Path, href: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in href.split('/') {
        p.push(part);
    }
    p
}

/// A disk-backed [`extract_cover_from_embedded_svg`]/
/// [`extract_calibre_cover`] resolver: resolves a relative path against
/// `base` and reads it from the filesystem. Matches real upstream's own
/// approach (every real caller already extracts to a temp directory).
pub fn filesystem_resolver(base: PathBuf) -> impl FnMut(&str) -> Option<Vec<u8>> {
    move |rel: &str| std::fs::read(join_relative(&base, rel)).ok()
}

fn element_children(dom: &Dom, id: NodeId) -> Vec<NodeId> {
    dom.children(id).into_iter().filter(|&c| matches!(dom.node(c).kind, NodeKind::Element(_))).collect()
}

/// Port of `extract_cover_from_embedded_svg`: an HTML page consisting
/// of exactly one inline `<svg>` whose only child is an `<image>`
/// referencing a raster file.
pub fn extract_cover_from_embedded_svg(raw: &str, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let dom = Dom::parse(raw);
    let svgs = dom.find_all_tag_global("svg");
    if svgs.len() != 1 {
        return None;
    }
    let children = element_children(&dom, svgs[0]);
    if children.len() != 1 {
        return None;
    }
    let image = children[0];
    if dom.tag(image) != Some("image") {
        return None;
    }
    let href = dom.node(image).attrs.get("xlink:href").or_else(|| dom.node(image).attrs.get("href"))?.clone();
    raster_or_none(resolve(&href)?)
}

/// Port of `extract_calibre_cover`: a body with (a) no heading/
/// paragraph/span/font/br tags anywhere and exactly one document-wide
/// `<img alt="cover">`, or (b) no such tags and a body with no text
/// content and exactly one `<img>` inside it.
pub fn extract_calibre_cover(raw: &str, resolve: &mut dyn FnMut(&str) -> Option<Vec<u8>>) -> Option<Vec<u8>> {
    let dom = Dom::parse(raw);
    let has_marker_tag = MARKER_TAGS.iter().any(|t| !dom.find_all_tag_global(t).is_empty());
    if has_marker_tag {
        return None;
    }

    let all_images: Vec<NodeId> = dom.find_all_tag_global("img").into_iter().filter(|&id| dom.node(id).attrs.contains_key("src")).collect();
    if all_images.len() == 1 {
        let alt = dom.node(all_images[0]).attrs.get("alt").map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        if alt == "cover" {
            let src = dom.node(all_images[0]).attrs.get("src").cloned();
            if let Some(bytes) = src.and_then(|s| resolve(&s)).and_then(raster_or_none) {
                return Some(bytes);
            }
        }
    }

    let body = dom.find_first_tag_global("body")?;
    if !dom.text_content(body).trim().is_empty() {
        return None;
    }
    let body_images: Vec<NodeId> = dom.find_all_tag(body, "img").into_iter().filter(|&id| dom.node(id).attrs.contains_key("src")).collect();
    if body_images.len() == 1 {
        let src = dom.node(body_images[0]).attrs.get("src").cloned()?;
        return resolve(&src).and_then(raster_or_none);
    }
    None
}

/// Port of `render_html_svg_workaround`, minus the final Qt-WebEngine
/// fallback -- see the module doc.
pub fn render_html_svg_workaround(path_to_html: &Path) -> Option<Vec<u8>> {
    let file_bytes = std::fs::read(path_to_html).ok()?;
    let (raw, _encoding) = xml_to_unicode(&file_bytes, true, false);
    let base = path_to_html.parent().unwrap_or_else(|| Path::new(""));
    let mut resolve = filesystem_resolver(base.to_path_buf());

    let mut data = None;
    if raw.contains(SVG_NS) {
        data = extract_cover_from_embedded_svg(&raw, &mut resolve);
    }
    if data.is_none() {
        data = extract_calibre_cover(&raw, &mut resolve);
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    const JPEG_MAGIC: &[u8] = &[0xFF, 0xD8, 0xFF, 0xD9];

    #[test]
    fn extract_cover_from_embedded_svg_reads_the_referenced_raster_image() {
        let html = r#"<html xmlns:xlink="http://www.w3.org/1999/xlink"><body>
            <svg xmlns="http://www.w3.org/2000/svg"><image xlink:href="images/cover.jpg"/></svg>
        </body></html>"#;
        let mut resolve = |rel: &str| -> Option<Vec<u8>> {
            assert_eq!(rel, "images/cover.jpg");
            Some(JPEG_MAGIC.to_vec())
        };
        let result = extract_cover_from_embedded_svg(html, &mut resolve);
        assert_eq!(result.as_deref(), Some(JPEG_MAGIC));
    }

    #[test]
    fn extract_cover_from_embedded_svg_rejects_more_than_one_svg() {
        let html = r#"<html><body><svg><image href="a.jpg"/></svg><svg><image href="b.jpg"/></svg></body></html>"#;
        let mut resolve = |_: &str| -> Option<Vec<u8>> { Some(JPEG_MAGIC.to_vec()) };
        assert!(extract_cover_from_embedded_svg(html, &mut resolve).is_none());
    }

    #[test]
    fn extract_cover_from_embedded_svg_rejects_a_non_raster_referenced_file() {
        let html = r#"<html><body><svg><image href="cover.svg"/></svg></body></html>"#;
        let mut resolve = |_: &str| -> Option<Vec<u8>> { Some(b"<svg/>".to_vec()) };
        assert!(extract_cover_from_embedded_svg(html, &mut resolve).is_none());
    }

    #[test]
    fn extract_calibre_cover_accepts_a_single_alt_cover_image_anywhere() {
        let html = r#"<html><body><div><img src="c.jpg" alt="Cover"/></div></body></html>"#;
        let mut resolve = |rel: &str| -> Option<Vec<u8>> {
            assert_eq!(rel, "c.jpg");
            Some(JPEG_MAGIC.to_vec())
        };
        assert_eq!(extract_calibre_cover(html, &mut resolve).as_deref(), Some(JPEG_MAGIC));
    }

    #[test]
    fn extract_calibre_cover_rejects_when_a_marker_tag_is_present() {
        let html = r#"<html><body><p>Some text</p><img src="c.jpg" alt="cover"/></body></html>"#;
        let mut resolve = |_: &str| -> Option<Vec<u8>> { Some(JPEG_MAGIC.to_vec()) };
        assert!(extract_calibre_cover(html, &mut resolve).is_none());
    }

    #[test]
    fn extract_calibre_cover_accepts_a_lone_image_in_an_otherwise_textless_body() {
        let html = r#"<html><body>  <div><img src="c.png"/></div>  </body></html>"#;
        let mut resolve = |rel: &str| -> Option<Vec<u8>> {
            assert_eq!(rel, "c.png");
            Some(JPEG_MAGIC.to_vec())
        };
        assert_eq!(extract_calibre_cover(html, &mut resolve).as_deref(), Some(JPEG_MAGIC));
    }

    #[test]
    fn extract_calibre_cover_rejects_a_body_with_real_text() {
        let html = r#"<html><body>Some real text here<img src="c.png"/></body></html>"#;
        let mut resolve = |_: &str| -> Option<Vec<u8>> { Some(JPEG_MAGIC.to_vec()) };
        assert!(extract_calibre_cover(html, &mut resolve).is_none());
    }

    #[test]
    fn extract_calibre_cover_rejects_more_than_one_image_in_the_body() {
        let html = r#"<html><body><img src="a.png"/><img src="b.png"/></body></html>"#;
        let mut resolve = |_: &str| -> Option<Vec<u8>> { Some(JPEG_MAGIC.to_vec()) };
        assert!(extract_calibre_cover(html, &mut resolve).is_none());
    }

    #[test]
    fn render_html_svg_workaround_reads_a_real_file_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cover.jpg"), JPEG_MAGIC).unwrap();
        let html_path = tmp.path().join("titlepage.html");
        std::fs::write(&html_path, r#"<html><body><img src="cover.jpg" alt="cover"/></body></html>"#).unwrap();
        let result = render_html_svg_workaround(&html_path);
        assert_eq!(result.as_deref(), Some(JPEG_MAGIC));
    }

    #[test]
    fn render_html_svg_workaround_returns_none_when_no_pre_fallback_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let html_path = tmp.path().join("titlepage.html");
        // A real CSS-laid-out titlepage with actual text: neither
        // pre-fallback applies, and the Qt-WebEngine fallback isn't
        // ported -- this is the real, disclosed narrowing.
        std::fs::write(&html_path, r#"<html><body><h1>My Book</h1><p>by an author</p></body></html>"#).unwrap();
        assert!(render_html_svg_workaround(&html_path).is_none());
    }
}
