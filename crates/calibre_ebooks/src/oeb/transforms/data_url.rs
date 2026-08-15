//! Port of `old_src/src/calibre/ebooks/oeb/transforms/data_url.py`.

use base64::Engine;

use crate::oeb::book::OEBBook;

/// Port of `DataURL`: replace `data:` URI images embedded directly in
/// spine `<img src>` attributes with real manifest items.
pub struct DataURL;

impl DataURL {
    pub fn call(&self, oeb: &mut OEBBook) {
        let hrefs: Vec<String> = oeb.spine.iter().map(|s| s.idref.clone()).collect();
        for idref in hrefs {
            let Some(item) = oeb.manifest.get_by_id(&idref) else {
                continue;
            };
            let href = item.href.clone();
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = crate::mobi::dom::Dom::parse(&html);
            let mut changed = false;
            for img in dom.find_all_tag_global("img") {
                let src = dom.node(img).attrs.get("src").cloned().unwrap_or_default();
                if !src.starts_with("data:") {
                    continue;
                }
                let Some((header, data)) = src.split_once(',') else {
                    continue;
                };
                if !header.starts_with("data:image/") || data.is_empty() {
                    continue;
                }
                let bytes = if header.contains(";base64") {
                    let cleaned: String = data.chars().filter(|c| !c.is_whitespace()).collect();
                    match base64::engine::general_purpose::STANDARD.decode(&cleaned) {
                        Ok(b) => b,
                        // Invalid base64: ignore this image, matching
                        // Python's `except Exception: ... continue`.
                        // (`OEBBook` has no logger yet to report a
                        // warning through -- see the module doc note.)
                        Err(_) => continue,
                    }
                } else {
                    urlencoding::decode(data)
                        .map(|s| s.into_owned().into_bytes())
                        .unwrap_or_else(|_| data.as_bytes().to_vec())
                };
                // Unknown image format: ignore, matching Python's
                // `self.log.warn(...); continue`.
                let Some(fmt) = calibre_utils::imghdr::what(&bytes) else {
                    continue;
                };
                let (id, item_href) = oeb
                    .manifest
                    .generate("data-url-image", &format!("data-url-image.{fmt}"));
                let mime = crate::oeb::polish::utils::guess_type(&item_href);
                oeb.manifest.add(&id, &item_href, &mime);
                let _ = oeb.container.write(&item_href, &bytes);
                let rel = super::filenames::relhref(&href, &item_href);
                dom.node_mut(img).attrs.insert("src".to_string(), rel);
                changed = true;
            }
            if changed {
                let rendered = dom.serialize(dom.root).into_bytes();
                let _ = oeb.container.write(&href, &rendered);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn converts_base64_data_uri_image_to_manifest_item() {
        // A 1x1 transparent PNG.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1F, 0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9C, 0x62, 0x00, 0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let b64 = base64::engine::general_purpose::STANDARD.encode(png);
        let body = format!(r#"<img src="data:image/png;base64,{b64}"/>"#);
        let mut oeb = Builder::new().page("a.html", &body).build();
        DataURL.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(!html.contains("data:image"), "{html}");
        assert!(html.contains("data-url-image"), "{html}");
        let has_png_item = oeb
            .manifest
            .iter()
            .any(|i| i.media_type == "image/png" && i.href.starts_with("data-url-image"));
        assert!(has_png_item);
    }

    #[test]
    fn leaves_non_data_uri_images_alone() {
        let mut oeb = Builder::new()
            .page("a.html", r#"<img src="regular.png"/>"#)
            .build();
        DataURL.call(&mut oeb);
        let raw = oeb.container.read("a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains(r#"src="regular.png""#), "{html}");
    }
}
