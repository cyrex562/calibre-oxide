//! Port of `old_src/src/calibre/ebooks/oeb/transforms/cover.py`.
//!
//! This is the conversion-pipeline cover manager: it synthesizes an
//! XHTML+SVG (or plain `<img>`) title page wrapping the book's cover
//! image for output formats that need one. Distinct from
//! `oeb::polish::cover` (issue #166), which edits an existing EPUB's
//! cover in place for the "Polish Book" tool -- different job, same
//! Python filename in different packages.

use crate::oeb::book::OEBBook;
use crate::oeb::polish::utils::guess_type;

/// Port of `CoverManager`.
#[derive(Default)]
pub struct CoverManager {
    pub no_default_cover: bool,
    pub no_svg_cover: bool,
    pub preserve_aspect_ratio: bool,
    pub fixed_size: Option<(String, String)>,
}

impl CoverManager {
    pub fn call(&self, oeb: &mut OEBBook) {
        self.insert_cover(oeb);
    }

    /// Port of `CoverManager.default_cover`: generate a generic
    /// title/author cover for books that don't have one.
    ///
    /// `calibre.ebooks.covers.create_cover` (font-rendered cover-image
    /// synthesis: title/author layout, embedded fonts, gradients) is a
    /// large, separate rendering subsystem with no port in this
    /// workspace and is out of scope for this batch. Python's own
    /// `default_cover` already wraps `create_cover` in
    /// `try/except Exception: log.exception(...); return None`, so
    /// "cover generation unavailable" is itself one of Python's real,
    /// intended outcomes here (just usually not the *common* one) --
    /// this always takes that path, which is why `insert_cover` below
    /// unconditionally handles a `None` result by giving up cleanly
    /// (matching Python's `if href is None: return`), never by
    /// panicking.
    fn default_cover(&self, oeb: &OEBBook) -> Option<String> {
        let _ = oeb;
        if self.no_default_cover {
            return None;
        }
        None
    }

    /// Port of `CoverManager.inspect_cover`: `(width, height)` of the
    /// manifest item at `href`, or `(-1, -1)` if it can't be read.
    fn inspect_cover(&self, oeb: &OEBBook, href: &str) -> (i64, i64) {
        let Some(item) = oeb.manifest.get_by_href(href) else {
            return (-1, -1);
        };
        let Ok(raw) = oeb.container.read(&item.href) else {
            return (-1, -1);
        };
        let (_, w, h) = calibre_utils::imghdr::identify(&raw);
        (w, h)
    }

    fn svg_template(&self, href: &str, width: i64, height: i64) -> String {
        let ar = if self.preserve_aspect_ratio {
            "xMidYMid meet"
        } else {
            "none"
        };
        format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">\
             <head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\
             <meta name=\"calibre:cover\" content=\"true\" /><title>Cover</title>\
             <style type=\"text/css\" title=\"override_css\">@page {{padding: 0pt; margin:0pt}} \
             body {{ text-align: center; padding:0pt; margin: 0pt; }}</style></head>\
             <body><div><svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\" \
             xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"100%\" height=\"100%\" \
             viewBox=\"0 0 {width} {height}\" preserveAspectRatio=\"{ar}\">\
             <image width=\"{width}\" height=\"{height}\" xlink:href=\"{href}\"/></svg></div></body></html>"
        )
    }

    fn non_svg_template(&self, href: &str) -> String {
        let style = match &self.fixed_size {
            None => "style=\"height: 100%\"".to_string(),
            Some((width, height)) => format!("style=\"height: {height}; width: {width}\""),
        };
        format!(
            "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">\
             <head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\
             <meta name=\"calibre:cover\" content=\"true\" /><title>Cover</title>\
             <style type=\"text/css\" title=\"override_css\">@page {{padding: 0pt; margin:0pt}} \
             body {{ text-align: center; padding:0pt; margin: 0pt }} div {{ padding:0pt; margin: 0pt }} \
             img {{ padding:0pt; margin: 0pt }}</style></head>\
             <body><div><img src=\"{href}\" alt=\"cover\" {style} /></div></body></html>"
        )
    }

    /// Port of `CoverManager.insert_cover`.
    fn insert_cover(&self, oeb: &mut OEBBook) {
        let item_id: Option<String> = if !oeb.guide.references.contains_key("titlepage") {
            let href = if let Some(cover) = oeb.guide.get("cover") {
                Some(cover.href.clone())
            } else {
                self.default_cover(oeb)
            };
            let Some(href) = href else {
                return;
            };
            let (mut width, mut height) = self.inspect_cover(oeb, &href);
            if width == -1 || height == -1 {
                width = 600;
                height = 800;
            }
            let decoded_href = urlencoding::decode(&href)
                .map(|c| c.into_owned())
                .unwrap_or(href);
            let contents = if self.no_svg_cover {
                self.non_svg_template(&decoded_href)
            } else {
                self.svg_template(&decoded_href, width, height)
            };
            let (id, tp_href) = oeb.manifest.generate("titlepage", "titlepage.xhtml");
            let mime = guess_type("t.xhtml");
            oeb.manifest.add(&id, &tp_href, &mime);
            let _ = oeb.container.write(&tp_href, contents.as_bytes());
            Some(id)
        } else {
            let tp_href = oeb.guide.get("titlepage").unwrap().href.clone();
            let (path, _) = tp_href.split_once('#').unwrap_or((&tp_href, ""));
            oeb.manifest.get_by_href(path).map(|i| i.id.clone())
        };

        if let Some(id) = item_id {
            let item_href = oeb.manifest.get_by_id(&id).unwrap().href.clone();
            if oeb.spine.index_of(&id).is_some() {
                oeb.spine.remove_by_idref(&id);
            }
            oeb.spine.insert(0, &id, true);
            if !oeb.guide.references.contains_key("cover") {
                oeb.guide.add("cover", Some("Title page".to_string()), "a");
            }
            if let Some(r) = oeb.guide.references.get_mut("cover") {
                r.href = item_href.clone();
            }
            if oeb.guide.references.contains_key("titlepage") {
                if let Some(r) = oeb.guide.references.get_mut("titlepage") {
                    r.href = item_href;
                }
            }
            // Python also fixes up `oeb.toc.item_that_refers_to_cover`
            // (an attribute a couple of other, out-of-scope writers set
            // dynamically). This crate's `TOC` has no such concept, so
            // there is nothing to fix up here.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn wraps_existing_cover_image_in_a_titlepage_and_updates_guide_and_spine() {
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x0A, 0x00, 0x00, 0x00, 0x0A, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x8D, 0x32, 0xCF, 0xBD, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let mut oeb = Builder::new()
            .part("cover.png", "image/png", png, false)
            .page("first.html", "<p>chapter one</p>")
            .build();
        oeb.guide.add("cover", Some("Cover".into()), "cover.png");

        CoverManager::default().call(&mut oeb);

        let tp = oeb
            .manifest
            .get_by_href("titlepage.xhtml")
            .expect("titlepage generated");
        assert_eq!(oeb.spine.items.first().unwrap().idref, tp.id);
        let raw = oeb.container.read("titlepage.xhtml").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("cover.png"), "{html}");
        assert!(html.contains("viewBox=\"0 0 10 10\""), "{html}");
        assert_eq!(oeb.guide.get("cover").unwrap().href, "titlepage.xhtml");
    }

    #[test]
    fn does_nothing_when_no_cover_and_default_cover_disabled() {
        let mut oeb = Builder::new().page("first.html", "<p>x</p>").build();
        let mgr = CoverManager {
            no_default_cover: true,
            ..CoverManager::default()
        };
        mgr.call(&mut oeb);
        assert!(oeb.manifest.get_by_href("titlepage.xhtml").is_none());
    }
}
