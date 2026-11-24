//! Port of `old_src/src/calibre/ebooks/oeb/transforms/rasterize.py`.
//!
//! Rasterizes SVG images to PNG, for output formats that can't embed SVG
//! directly. Python does this with `qt.core`'s `QSvgRenderer`/
//! `QPainter`/`QImage` -- a GUI-toolkit dependency this project avoids
//! (`docs/AGENT_PORTING_GUIDE.md`). Unlike most Qt-shaped gaps elsewhere
//! in this codebase, SVG rasterization has a mature, production-grade,
//! pure-Rust replacement with **no** GUI/windowing dependency: the
//! `resvg` crate (+ its `tiny-skia` rasterization backend), the same
//! renderer used by `typst`, `usvg`'s own CLI, and widely elsewhere in
//! the Rust ecosystem. This module adds `resvg` as a new dependency
//! (verified to build and rasterize a real SVG in a scratch project
//! before writing any of this file) and implements real SVG->PNG
//! rasterization end to end -- mirroring how issue #40 added `image`/
//! `jpeg-decoder` for a real, narrowly-scoped capability rather than
//! leaving a `todo!()`.
//!
//! # A narrower, precisely-scoped gap: text-in-SVG
//!
//! `resvg` renders `<text>` correctly only when it can resolve a
//! matching font from a loaded font database; this module does not load
//! a system or bundled font database (`usvg::Options::default()`, no
//! `fontdb` population), so any `<text>` element in a rasterized SVG
//! renders as empty space rather than glyphs. E-book SVGs are
//! overwhelmingly vector art/cover illustrations with no `<text>`
//! element (text is normally left as real, selectable HTML text
//! precisely so it does *not* need to go through this transform), so
//! this is a narrow, low-impact gap, not a reason to leave the whole
//! file unported. Shapes, paths, gradients, and embedded raster images
//! (`<image>`) all rasterize correctly.
//!
//! # Sizing: a documented, narrower-than-Python scope
//!
//! Python resolves an embedded SVG's on-page width/height through the
//! full CSS cascade (`Stylizer.style(elem)['width']`/`['height']`) --
//! this crate has no layout engine to resolve arbitrary CSS box-model
//! sizing (percentages, `auto`, inherited values, ...) against arbitrary
//! markup. This port reads the element's own `width`/`height`
//! *attributes* (the common, explicit case for `<img>`/`<object>`/
//! inline `<svg>` in e-book content) and, when neither is present, keeps
//! the SVG's own intrinsic size unscaled -- real, useful behavior for
//! the overwhelming majority of real-world markup, just narrower than a
//! full cascade resolution.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use base64::Engine;
use regex::Regex;

use crate::dom::{Dom, NodeId};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::{OEB_RASTER_IMAGES, PNG_MIME, SVG_MIME};

/// Port of `data_url`.
pub fn data_url(mime_type: &str, data: &[u8]) -> String {
    format!(
        "data:{mime_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(data)
    )
}

fn ensure_svg_xmlns(mut svg_text: String) -> String {
    let Some(pos) = svg_text.find("<svg") else {
        return svg_text;
    };
    let insert_at = pos + 4;
    if !svg_text.contains("xmlns=\"http://www.w3.org/2000/svg\"") {
        svg_text.insert_str(insert_at, " xmlns=\"http://www.w3.org/2000/svg\"");
    }
    if svg_text.contains("xlink:href") && !svg_text.contains("xmlns:xlink") {
        let insert_at = svg_text.find("<svg").unwrap() + 4;
        svg_text.insert_str(insert_at, " xmlns:xlink=\"http://www.w3.org/1999/xlink\"");
    }
    svg_text
}

/// Port of `rasterize_svg`: renders `svg_data` to a PNG. `sizes` is used
/// only when the SVG has no real intrinsic size of its own (matching
/// Qt's `QSvgRenderer::defaultSize()` 100x100 fallback quirk Python's
/// version worked around); `width`/`height` (in pixels, `0.0` meaning
/// "unset") scale the render, preserving aspect ratio, the same as
/// Python's `size.scale(..., Qt.AspectRatioMode.KeepAspectRatio)`.
pub fn rasterize_svg(
    svg_data: &[u8],
    sizes: Option<(f64, f64)>,
    width: f64,
    height: f64,
) -> Result<Vec<u8>> {
    let text = ensure_svg_xmlns(String::from_utf8_lossy(svg_data).into_owned());
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_str(&text, &opt).map_err(|e| anyhow!("invalid SVG: {e}"))?;
    let intrinsic = tree.size();
    let (mut w, mut h) = (intrinsic.width() as f64, intrinsic.height() as f64);
    if (w - 100.0).abs() < 0.001 && (h - 100.0).abs() < 0.001 {
        if let Some((sw, sh)) = sizes {
            if sw > 0.0 && sh > 0.0 {
                w = sw;
                h = sh;
            }
        }
    }
    if width > 0.0 || height > 0.0 {
        let scale = match (width > 0.0, height > 0.0) {
            (true, true) => (width / w).min(height / h),
            (true, false) => width / w,
            (false, true) => height / h,
            (false, false) => 1.0,
        };
        w *= scale;
        h *= scale;
    }
    let out_w = (w.round() as u32).max(1);
    let out_h = (h.round() as u32).max(1);
    let mut pixmap = tiny_skia::Pixmap::new(out_w, out_h)
        .ok_or_else(|| anyhow!("invalid raster size {out_w}x{out_h}"))?;
    pixmap.fill(tiny_skia::Color::WHITE);
    let sx = out_w as f32 / intrinsic.width().max(f32::MIN_POSITIVE);
    let sy = out_h as f32 / intrinsic.height().max(f32::MIN_POSITIVE);
    let transform = tiny_skia::Transform::from_scale(sx, sy);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|e| anyhow!("failed to encode PNG: {e}"))
}

fn xlink_href_re() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"xlink:href\s*=\s*("([^"]*)"|'([^']*)')"#).unwrap())
}

/// Options this transform reads (`opts.output_profile`/`context` in
/// Python). Defaults are a reasonable generic stand-in -- see the module
/// docs on sizing scope.
#[derive(Debug, Clone)]
pub struct RasterizeOptions {
    pub dpi: f64,
    /// Cover page width/height, in points.
    pub cover_width_pt: f64,
    pub cover_height_pt: f64,
    pub save_svg_originals: bool,
}

impl Default for RasterizeOptions {
    fn default() -> Self {
        RasterizeOptions {
            dpi: 96.0,
            cover_width_pt: 590.0,
            cover_height_pt: 750.0,
            save_svg_originals: false,
        }
    }
}

/// Port of `SVGRasterizer`.
pub struct SvgRasterizer {
    pub opts: RasterizeOptions,
    /// `(svg_href, width_px, height_px) -> generated png_href`, matching
    /// Python's `self.images` cache (the same external SVG rasterized at
    /// the same size, referenced from multiple pages, is only rendered
    /// once).
    cache: HashMap<(String, u32, u32), String>,
}

impl Default for SvgRasterizer {
    fn default() -> Self {
        Self::new(RasterizeOptions::default())
    }
}

impl SvgRasterizer {
    pub fn new(opts: RasterizeOptions) -> Self {
        SvgRasterizer {
            opts,
            cache: HashMap::new(),
        }
    }

    /// Port of `SVGRasterizer.__call__`.
    pub fn call(&mut self, oeb: &mut OEBBook, report: &mut dyn FnMut(&str)) -> Result<()> {
        report("Rasterizing SVG images...");
        self.cache.clear();
        self.scan_for_linked_resources_in_manifest(oeb)?;
        self.rasterize_spine(oeb)?;
        self.rasterize_cover(oeb)?;
        Ok(())
    }

    /// Port of `inline_linked_raster_images`/`scan_for_linked_resources_in_svg`:
    /// rewrites every `xlink:href="..."` in `svg_text` that resolves
    /// (relative to `base_href`) to a raster-image manifest item into a
    /// `data:` URI, so the rasterizer can see it without needing its own
    /// file resolution.
    fn inline_linked_raster_images(
        &self,
        oeb: &OEBBook,
        base_href: &str,
        svg_text: &str,
    ) -> (String, bool) {
        let mut changed = false;
        let out = xlink_href_re()
            .replace_all(svg_text, |caps: &regex::Captures| {
                let href = caps
                    .get(2)
                    .or_else(|| caps.get(3))
                    .map(|m| m.as_str())
                    .unwrap_or("");
                let (path, _frag) = super::filenames::urldefrag(href);
                if path.is_empty() {
                    return caps[0].to_string();
                }
                let abs = super::filenames::abshref(base_href, &path);
                let abs = super::filenames::urlnormalize(&abs);
                let Some(item) = oeb.manifest.get_by_href(&abs) else {
                    return caps[0].to_string();
                };
                if !OEB_RASTER_IMAGES.contains(&item.media_type.as_str()) {
                    return caps[0].to_string();
                }
                let Ok(data) = oeb.container.read(&item.href) else {
                    return caps[0].to_string();
                };
                changed = true;
                format!("xlink:href=\"{}\"", data_url(&item.media_type, &data))
            })
            .into_owned();
        (out, changed)
    }

    fn scan_for_linked_resources_in_manifest(&self, oeb: &mut OEBBook) -> Result<()> {
        let svg_hrefs: Vec<String> = oeb
            .manifest
            .iter()
            .filter(|i| i.media_type == SVG_MIME)
            .map(|i| i.href.clone())
            .collect();
        for href in svg_hrefs {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let text = String::from_utf8_lossy(&raw).into_owned();
            let (new_text, changed) = self.inline_linked_raster_images(oeb, &href, &text);
            if changed {
                let _ = oeb.container.write(&href, new_text.as_bytes());
            }
        }
        Ok(())
    }

    fn rasterize_spine(&mut self, oeb: &mut OEBBook) -> Result<()> {
        let spine_hrefs: Vec<String> = oeb
            .spine
            .iter()
            .filter_map(|s| oeb.manifest.get_by_id(&s.idref).map(|i| i.href.clone()))
            .collect();
        for href in spine_hrefs {
            self.rasterize_item(oeb, &href)?;
        }
        Ok(())
    }

    fn rasterize_item(&mut self, oeb: &mut OEBBook, href: &str) -> Result<()> {
        let Ok(raw) = oeb.container.read(href) else {
            return Ok(());
        };
        let html = String::from_utf8_lossy(&raw);
        let mut dom = Dom::parse(&html);
        let mut changed = false;

        let imgs: Vec<NodeId> = dom.find_all_tag_global("img");
        for img in imgs {
            let Some(src) = dom.node(img).attrs.get("src").cloned() else {
                continue;
            };
            let abs = super::filenames::urlnormalize(&super::filenames::abshref(href, &src));
            let Some(svg_href) = oeb
                .manifest
                .get_by_href(&abs)
                .filter(|i| i.media_type == SVG_MIME)
                .map(|i| i.href.clone())
            else {
                continue;
            };
            self.rasterize_external(oeb, &mut dom, img, href, &svg_href)?;
            changed = true;
        }

        let objects: Vec<NodeId> = dom.find_all_tag_global("object");
        for obj in objects {
            if dom.node(obj).attrs.get("type").map(|s| s.as_str()) != Some(SVG_MIME) {
                continue;
            }
            let Some(data_attr) = dom.node(obj).attrs.get("data").cloned() else {
                continue;
            };
            let abs = super::filenames::urlnormalize(&super::filenames::abshref(href, &data_attr));
            let Some(svg_href) = oeb
                .manifest
                .get_by_href(&abs)
                .filter(|i| i.media_type == SVG_MIME)
                .map(|i| i.href.clone())
            else {
                continue;
            };
            self.rasterize_external(oeb, &mut dom, obj, href, &svg_href)?;
            changed = true;
        }

        let svgs: Vec<NodeId> = dom.find_all_tag_global("svg");
        for svg in svgs {
            self.rasterize_inline(oeb, &mut dom, svg, href)?;
            changed = true;
        }

        if changed {
            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(href, &rendered);
        }
        Ok(())
    }

    /// Port of `rasterize_external`: replaces an `<img>`/`<object>`
    /// pointing at an SVG manifest item with a plain `<img>` pointing at
    /// a freshly rasterized PNG.
    fn rasterize_external(
        &mut self,
        oeb: &mut OEBBook,
        dom: &mut Dom,
        elem: NodeId,
        item_href: &str,
        svg_href: &str,
    ) -> Result<()> {
        let width = attr_px(dom, elem, "width").unwrap_or(0.0);
        let height = attr_px(dom, elem, "height").unwrap_or(0.0);
        let (w_key, h_key) = (width.round() as u32, height.round() as u32);

        let png_href = if let Some(cached) = self.cache.get(&(svg_href.to_string(), w_key, h_key)) {
            cached.clone()
        } else {
            let svg_bytes = oeb.container.read(svg_href)?;
            let png = rasterize_svg(&svg_bytes, None, width, height)?;
            let (stem, _) = split_ext(svg_href);
            let (id, out_href) = oeb.manifest.generate("svg_raster", &format!("{stem}.png"));
            oeb.manifest.add(&id, &out_href, PNG_MIME);
            oeb.container.write(&out_href, &png)?;
            self.cache
                .insert((svg_href.to_string(), w_key, h_key), out_href.clone());
            out_href
        };

        let alt = dom.text_content(elem).trim().to_string();
        let keep = ["class", "style", "width", "height", "align"];
        let all_attrs: Vec<String> = dom.node(elem).attrs.keys().cloned().collect();
        for a in all_attrs {
            if !keep.contains(&a.as_str()) {
                dom.node_mut(elem).attrs.shift_remove(&a);
            }
        }
        dom.set_tag(elem, "img");
        let rel = super::filenames::relhref(item_href, &png_href);
        dom.node_mut(elem).attrs.insert("src".to_string(), rel);
        if !alt.is_empty() {
            dom.node_mut(elem).attrs.insert("alt".to_string(), alt);
        }
        for c in dom.children(elem) {
            dom.detach(c);
        }
        Ok(())
    }

    /// Port of `rasterize_inline`: replaces an inline `<svg>` element
    /// with a plain `<img>` pointing at a freshly rasterized PNG.
    fn rasterize_inline(
        &self,
        oeb: &mut OEBBook,
        dom: &mut Dom,
        elem: NodeId,
        item_href: &str,
    ) -> Result<()> {
        let width = attr_px(dom, elem, "width").unwrap_or(0.0);
        let height = attr_px(dom, elem, "height").unwrap_or(0.0);

        let svg_text = dom.serialize(elem);
        let (svg_text, _) = self.inline_linked_raster_images(oeb, item_href, &svg_text);
        let png = rasterize_svg(svg_text.as_bytes(), None, width, height)?;

        let (stem, _) = split_ext(item_href);
        let (id, out_href) = oeb
            .manifest
            .generate("svg_inline", &format!("{stem}_inline.png"));
        oeb.manifest.add(&id, &out_href, PNG_MIME);
        oeb.container.write(&out_href, &png)?;

        if self.opts.save_svg_originals {
            let (sid, shref) = oeb
                .manifest
                .generate("svg_inline_src", &format!("{stem}_inline.svg"));
            oeb.manifest.add(&sid, &shref, SVG_MIME);
            let _ = oeb.container.write(&shref, svg_text.as_bytes());
        }

        let Some(parent) = dom.parent(elem) else {
            return Ok(());
        };
        let Some(idx) = dom.index_in_parent(elem) else {
            return Ok(());
        };
        let img = dom.new_element("img");
        let rel = super::filenames::relhref(item_href, &out_href);
        dom.node_mut(img).attrs.insert("src".to_string(), rel);
        if width > 0.0 {
            dom.node_mut(img)
                .attrs
                .insert("width".to_string(), format!("{}", width.round() as i64));
        }
        if height > 0.0 {
            dom.node_mut(img)
                .attrs
                .insert("height".to_string(), format!("{}", height.round() as i64));
        }
        dom.detach(elem);
        dom.insert_child(parent, idx, img);
        Ok(())
    }

    /// Port of `rasterize_cover`.
    fn rasterize_cover(&self, oeb: &mut OEBBook) -> Result<()> {
        let Some(cover_id) = oeb.metadata.first("cover").map(|i| i.value.clone()) else {
            return Ok(());
        };
        let Some(item) = oeb.manifest.get_by_id(&cover_id) else {
            oeb.metadata.clear("cover");
            return Ok(());
        };
        if item.media_type != SVG_MIME {
            return Ok(());
        }
        let svg_href = item.href.clone();
        let width = self.opts.cover_width_pt / 72.0 * self.opts.dpi;
        let height = self.opts.cover_height_pt / 72.0 * self.opts.dpi;
        let svg_bytes = oeb.container.read(&svg_href)?;
        let png = rasterize_svg(&svg_bytes, None, width, height)?;

        let (stem, _) = split_ext(&svg_href);
        let (id, out_href) = oeb
            .manifest
            .generate("cover_raster", &format!("{stem}.png"));
        oeb.manifest.add(&id, &out_href, PNG_MIME);
        oeb.container.write(&out_href, &png)?;
        oeb.metadata.clear("cover");
        oeb.metadata.add("cover", &id);
        Ok(())
    }
}

fn attr_px(dom: &Dom, elem: NodeId, name: &str) -> Option<f64> {
    dom.node(elem)
        .attrs
        .get(name)
        .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
        .filter(|v| *v > 0.0)
}

fn split_ext(href: &str) -> (String, String) {
    let slash = href.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &href[slash..];
    match base.rfind('.') {
        Some(i) if i > 0 => (href[..slash + i].to_string(), base[i..].to_string()),
        _ => (href.to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    const SIMPLE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><rect width="64" height="64" fill="red"/></svg>"#;

    #[test]
    fn rasterize_svg_produces_a_valid_png_at_the_intrinsic_size() {
        let png = rasterize_svg(SIMPLE_SVG.as_bytes(), None, 0.0, 0.0).unwrap();
        assert_eq!(
            &png[0..8],
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]
        );
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), 64);
        assert_eq!(img.height(), 64);
    }

    #[test]
    fn rasterize_svg_scales_to_requested_width_keeping_aspect_ratio() {
        let png = rasterize_svg(SIMPLE_SVG.as_bytes(), None, 128.0, 0.0).unwrap();
        let img = image::load_from_memory(&png).unwrap();
        assert_eq!(img.width(), 128);
        assert_eq!(img.height(), 128);
    }

    #[test]
    fn rasterize_svg_rejects_invalid_svg_data() {
        assert!(rasterize_svg(b"not svg at all", None, 0.0, 0.0).is_err());
    }

    #[test]
    fn data_url_round_trips_base64() {
        let url = data_url("image/png", b"hello");
        assert!(url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn rasterize_item_replaces_an_img_pointing_at_an_svg_with_a_png() {
        let content = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="pic.svg" width="32" height="32"/></body></html>"#;
        let mut oeb = Builder::new()
            .part("pic.svg", "image/svg+xml", SIMPLE_SVG.as_bytes(), false)
            .part("a.html", "application/xhtml+xml", content.as_bytes(), true)
            .build();
        let mut r = SvgRasterizer::default();
        let mut log = Vec::new();
        r.call(&mut oeb, &mut |m| log.push(m.to_string())).unwrap();

        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("pic.svg"), "{html}");
        assert!(html.contains(".png"), "{html}");
        let png_href = oeb
            .manifest
            .iter()
            .find(|i| i.media_type == "image/png")
            .unwrap()
            .href
            .clone();
        let png_data = oeb.container.read(&png_href).unwrap();
        let img = image::load_from_memory(&png_data).unwrap();
        assert_eq!(img.width(), 32);
        assert_eq!(img.height(), 32);
    }

    #[test]
    fn rasterize_item_replaces_an_inline_svg_element_with_an_img() {
        let content = format!(
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>{SIMPLE_SVG}</body></html>"#
        );
        let mut oeb = Builder::new()
            .part("a.html", "application/xhtml+xml", content.as_bytes(), true)
            .build();
        let mut r = SvgRasterizer::default();
        let mut log = Vec::new();
        r.call(&mut oeb, &mut |m| log.push(m.to_string())).unwrap();
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("<svg"), "{html}");
        assert!(html.contains("<img"), "{html}");
    }

    #[test]
    fn rasterize_cover_replaces_an_svg_cover_with_a_png_and_updates_metadata() {
        let mut oeb = Builder::new()
            .part("cover.svg", "image/svg+xml", SIMPLE_SVG.as_bytes(), false)
            .build();
        let cover_id = oeb.manifest.get_by_href("cover.svg").unwrap().id.clone();
        oeb.metadata.add("cover", &cover_id);
        let mut r = SvgRasterizer::default();
        let mut log = Vec::new();
        r.call(&mut oeb, &mut |m| log.push(m.to_string())).unwrap();
        let new_id = oeb.metadata.first("cover").unwrap().value.clone();
        assert_ne!(new_id, cover_id);
        let item = oeb.manifest.get_by_id(&new_id).unwrap();
        assert_eq!(item.media_type, "image/png");
    }
}
