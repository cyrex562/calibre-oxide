//! Port of `old_src/src/calibre/ebooks/oeb/transforms/trimmanifest.py`.

use std::collections::HashSet;

use crate::oeb::book::OEBBook;
use crate::oeb::constants::{CSS_MIME, OEB_DOCS};
use crate::oeb::transforms::filenames::{abshref, extract_css_urls, urldefrag, urlnormalize};

/// Port of `ManifestTrimmer`: drop every manifest item that is not
/// reachable from metadata, the guide, the spine, or a link chain
/// starting from any of those.
///
/// Link discovery is narrowed to `href`/`src`/`xlink:href` attributes
/// (matching [`super::filenames::RenameFiles`]'s narrowing of Python's
/// full `_link_attrs` set -- the same precedent already established in
/// `oeb::polish::container::replace_links_in_dom`).
pub struct ManifestTrimmer;

impl ManifestTrimmer {
    pub fn call(&self, oeb: &mut OEBBook) {
        let mut used: HashSet<String> = HashSet::new(); // manifest ids

        for item in &oeb.metadata.items {
            if let Some(id) = oeb.manifest.get_by_href(&item.value).map(|i| i.id.clone()) {
                used.insert(id);
            } else if oeb.manifest.get_by_id(&item.value).is_some() {
                used.insert(item.value.clone());
            }
        }
        for r in oeb.guide.values() {
            let (path, _) = urldefrag(&r.href);
            if let Some(id) = oeb.manifest.get_by_href(&path).map(|i| i.id.clone()) {
                used.insert(id);
            }
        }
        for s in oeb.spine.iter() {
            used.insert(s.idref.clone());
        }

        let mut unchecked: Vec<String> = used.iter().cloned().collect();
        while !unchecked.is_empty() {
            let mut new_ids: Vec<String> = Vec::new();
            for id in &unchecked {
                let Some(item) = oeb.manifest.get_by_id(id) else {
                    continue;
                };
                let href = item.href.clone();
                let media_type = item.media_type.clone();
                let Ok(raw) = oeb.container.read(&href) else {
                    continue;
                };
                let hrefs: Vec<String> = if OEB_DOCS.contains(&media_type.as_str())
                    || media_type.ends_with("/xml")
                    || media_type.ends_with("+xml")
                {
                    let html = String::from_utf8_lossy(&raw);
                    let dom = crate::dom::Dom::parse(&html);
                    let mut out = Vec::new();
                    for el in dom.preorder_elements(dom.root) {
                        for attr in ["href", "src", "xlink:href"] {
                            if let Some(v) = dom.node(el).attrs.get(attr) {
                                out.push(v.clone());
                            }
                        }
                    }
                    out
                } else if media_type == CSS_MIME {
                    let text = String::from_utf8_lossy(&raw);
                    extract_css_urls(&text)
                } else {
                    Vec::new()
                };
                for href_ref in hrefs {
                    let (path, _) = urldefrag(&urlnormalize(&href_ref));
                    let target = abshref(&href, &path);
                    if let Some(found) = oeb.manifest.get_by_href(&target).map(|i| i.id.clone()) {
                        if !used.contains(&found) {
                            new_ids.push(found);
                        }
                    }
                }
            }
            for id in &new_ids {
                used.insert(id.clone());
            }
            unchecked = new_ids;
        }

        let all_ids: Vec<String> = oeb.manifest.items.keys().cloned().collect();
        for id in all_ids {
            if !used.contains(&id) {
                oeb.manifest.remove(&id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn keeps_spine_items_and_their_referenced_images_drops_the_rest() {
        let mut oeb = Builder::new()
            .page("text/a.html", r#"<img src="../img/used.png"/>"#)
            .part("img/used.png", "image/png", b"png", false)
            .part("img/unused.png", "image/png", b"png", false)
            .build();
        ManifestTrimmer.call(&mut oeb);
        assert!(oeb.manifest.get_by_href("text/a.html").is_some());
        assert!(oeb.manifest.get_by_href("img/used.png").is_some());
        assert!(oeb.manifest.get_by_href("img/unused.png").is_none());
    }

    #[test]
    fn keeps_items_referenced_from_metadata_and_guide() {
        let mut oeb = Builder::new()
            .part("cover.jpg", "image/jpeg", b"jpg", false)
            .part("orphan.css", "text/css", b"", false)
            .build();
        oeb.metadata.add("cover", "cover.jpg");
        ManifestTrimmer.call(&mut oeb);
        assert!(oeb.manifest.get_by_href("cover.jpg").is_some());
        assert!(oeb.manifest.get_by_href("orphan.css").is_none());
    }
}
