//! Port of `old_src/src/calibre/ebooks/oeb/transforms/metadata.py`.

use std::collections::HashMap;

use chrono::Utc;

use crate::metadata::meta::MetaInformation;
use crate::oeb::book::OEBBook;

/// Port of `meta_info_to_oeb_metadata`: copy every non-null field of
/// `mi` (a [`MetaInformation`], calibre's canonical book-metadata
/// struct) into `oeb.metadata`.
///
/// A few Python fields have no equivalent on this crate's
/// [`MetaInformation`] (`book_producer`, `rights`, `publication_type`)
/// and are skipped -- there is nowhere to read them from. If those
/// fields are ever added to `MetaInformation`, port their branches here
/// too (each is a simple `if let Some(...) { m.clear(...); m.add(...) }`
/// following the pattern already used for the fields below).
pub fn meta_info_to_oeb_metadata(
    mi: &MetaInformation,
    m: &mut crate::oeb::metadata::Metadata,
    override_input_metadata: bool,
) {
    if !mi.title.is_empty() {
        m.clear("title");
        m.add("title", &mi.title);
    }
    if let Some(title_sort) = mi.title_sort.as_deref().filter(|s| !s.is_empty()) {
        if m.first("title").is_none() {
            m.add("title", title_sort);
        }
        m.clear("title_sort");
        m.add("title_sort", title_sort);
    }
    if !mi.authors.is_empty() {
        m.filter("creator", |x| {
            let role = x
                .get_attribute("role")
                .map(|r| r.to_lowercase())
                .unwrap_or_default();
            role == "aut" || role.is_empty()
        });
        for a in &mi.authors {
            let mut attrib = HashMap::new();
            attrib.insert("role".to_string(), "aut".to_string());
            if let Some(author_sort) = mi.author_sort.as_deref().filter(|s| !s.is_empty()) {
                attrib.insert("file-as".to_string(), author_sort.to_string());
            }
            m.add_with_attrib("creator", a, attrib);
        }
    }
    if let Some(comments) = mi.comments.as_deref().filter(|s| !s.is_empty()) {
        m.clear("description");
        m.add("description", comments);
    } else if override_input_metadata {
        m.clear("description");
    }
    if let Some(publisher) = mi.publisher.as_deref().filter(|s| !s.is_empty()) {
        m.clear("publisher");
        m.add("publisher", publisher);
    } else if override_input_metadata {
        m.clear("publisher");
    }
    if let Some(series) = mi.series.as_deref().filter(|s| !s.is_empty()) {
        m.clear("series");
        m.add("series", series);
    } else if override_input_metadata {
        m.clear("series");
    }
    let mut set_isbn = false;
    for (typ, val) in &mi.identifiers {
        if typ.eq_ignore_ascii_case("isbn") {
            set_isbn = true;
        }
        let mut has = false;
        for item in m.iter_mut("identifier") {
            let scheme_matches = item
                .get_attribute("scheme")
                .map(|s| s.eq_ignore_ascii_case(typ))
                .unwrap_or(false);
            if scheme_matches {
                item.value = val.clone();
                has = true;
            }
        }
        if !has {
            let mut attrib = HashMap::new();
            attrib.insert("scheme".to_string(), typ.to_uppercase());
            m.add_with_attrib("identifier", val, attrib);
        }
    }
    if override_input_metadata && !set_isbn {
        m.filter("identifier", |x| {
            x.get_attribute("scheme")
                .map(|s| s.eq_ignore_ascii_case("isbn"))
                .unwrap_or(false)
        });
    }
    if !mi.languages.is_empty() {
        m.clear("language");
        for lang in &mi.languages {
            if !lang.is_empty() && !lang.eq_ignore_ascii_case("und") {
                m.add("language", lang);
            }
        }
    }
    // series_index: Python's `mi.is_null('series_index')` is true when
    // there is no series at all; our `MetaInformation.series_index` is a
    // plain `f64` (default 1.0), so mirror Python's actual gating: only
    // emit a series_index when there is a series.
    if mi.series.as_deref().filter(|s| !s.is_empty()).is_some() {
        m.clear("series_index");
        m.add("series_index", &format_series_index(mi.series_index));
    } else if override_input_metadata {
        m.clear("series_index");
    }
    if let Some(rating) = mi.rating {
        m.clear("rating");
        m.add("rating", &format!("{rating:.2}"));
    } else if override_input_metadata {
        m.clear("rating");
    }
    if !mi.tags.is_empty() {
        m.clear("subject");
        for t in &mi.tags {
            m.add("subject", t);
        }
    } else if override_input_metadata {
        m.clear("subject");
    }
    if let Some(pubdate) = mi.pubdate {
        m.clear("date");
        m.add("date", &pubdate.to_rfc3339());
    }
    if let Some(timestamp) = mi.timestamp {
        m.clear("timestamp");
        m.add("timestamp", &timestamp.to_rfc3339());
    }

    if m.first("timestamp").is_none() {
        m.add("timestamp", &Utc::now().to_rfc3339());
    }
}

/// Port of `MetaInformation.format_series_index`: an integer-valued
/// index (`3.0`) is rendered without a decimal point, matching Python's
/// `'%d'%x if int(x)==x else nn(x)` behavior.
fn format_series_index(idx: f64) -> String {
    if idx == idx.trunc() {
        format!("{}", idx as i64)
    } else {
        let s = format!("{idx:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Port of `MergeMetadata`: merge user-specified metadata, including the
/// cover, into `oeb`.
pub struct MergeMetadata;

impl MergeMetadata {
    /// `prefer_metadata_cover` mirrors `opts.prefer_metadata_cover`;
    /// `plumber_output_format` mirrors `oeb.plumber_output_format`
    /// (checked against `mobi`/`azw3` to decide whether to drop an
    /// HTML title page).
    pub fn call(
        &self,
        oeb: &mut OEBBook,
        mi: &MetaInformation,
        prefer_metadata_cover: bool,
        plumber_output_format: &str,
        override_input_metadata: bool,
    ) {
        meta_info_to_oeb_metadata(mi, &mut oeb.metadata, override_input_metadata);
        let cover_id = self.set_cover(oeb, mi, prefer_metadata_cover, plumber_output_format);
        oeb.metadata.clear("cover");
        if let Some(cover_id) = cover_id {
            oeb.metadata.add("cover", &cover_id);
        }
        if let Some(uuid) = mi.uuid.as_deref() {
            oeb.metadata.filter("identifier", |x| {
                x.get_attribute("id")
                    .map(|i| i == "uuid_id")
                    .unwrap_or(false)
            });
            let mut attrib = HashMap::new();
            attrib.insert("id".to_string(), "uuid_id".to_string());
            attrib.insert("scheme".to_string(), "uuid".to_string());
            oeb.metadata.add_with_attrib("identifier", uuid, attrib);
            oeb.uid = Some(uuid.to_string());
        }
    }

    /// Returns the manifest id of the resulting cover image item, if
    /// any. Port of `MergeMetadata.set_cover`.
    fn set_cover(
        &self,
        oeb: &mut OEBBook,
        mi: &MetaInformation,
        prefer_metadata_cover: bool,
        plumber_output_format: &str,
    ) -> Option<String> {
        let (mut cdata, mut ext): (Vec<u8>, String) = (Vec::new(), "jpg".to_string());
        if !mi.cover_data.1.is_empty() {
            cdata = mi.cover_data.1.clone();
            ext = mi
                .cover_data
                .0
                .clone()
                .unwrap_or_else(|| "jpg".to_string())
                .to_lowercase();
        }
        if !matches!(ext.as_str(), "png" | "jpg" | "jpeg") {
            ext = "jpg".to_string();
        }

        let old_cover_href = oeb.guide.get("cover").map(|r| r.href.clone());
        if prefer_metadata_cover && old_cover_href.is_some() {
            cdata.clear();
        }
        if !cdata.is_empty() {
            oeb.guide.remove("cover");
            oeb.guide.remove("titlepage");
        } else if matches!(plumber_output_format, "mobi" | "azw3") && old_cover_href.is_some() {
            // The amazon formats don't support html cover pages, so
            // remove them even if no cover was specified.
            oeb.guide.remove("titlepage");
        }

        let mut do_remove_old_cover = false;
        let mut old_cover_item_id: Option<String> = None;
        let mut old_cover_item_href: Option<String> = None;
        if let Some(ref href) = old_cover_href {
            if let Some(item) = oeb.manifest.get_by_href(href) {
                if cdata.is_empty() {
                    return Some(item.id.clone());
                }
                old_cover_item_id = Some(item.id.clone());
                old_cover_item_href = Some(item.href.clone());
                do_remove_old_cover = true;
            } else if cdata.is_empty() {
                let (id, href2) = oeb.manifest.generate("cover", href);
                oeb.manifest.add(&id, &href2, "image/jpeg");
                return Some(id);
            }
        }

        let mut new_cover_href: Option<String> = None;
        let mut new_cover_id: Option<String> = None;
        if !cdata.is_empty() {
            let (id, href) = oeb.manifest.generate("cover", &format!("cover.{ext}"));
            let mime = crate::oeb::polish::utils::guess_type(&format!("cover.{ext}"));
            oeb.manifest.add(&id, &href, &mime);
            let _ = oeb.container.write(&href, &cdata);
            oeb.guide.add("cover", Some("Cover".to_string()), &href);
            new_cover_href = Some(href);
            new_cover_id = Some(id);
        }

        if do_remove_old_cover {
            if let (Some(old_id), Some(old_href)) = (old_cover_item_id, old_cover_item_href) {
                self.remove_old_cover(oeb, &old_id, &old_href, new_cover_href.as_deref());
            }
        }
        new_cover_id
    }

    /// Port of `MergeMetadata.remove_old_cover`: drop the superseded
    /// cover image from the manifest and rewrite (or remove) references
    /// to it from spine `<img src>`/`<image xlink:href>` elements,
    /// dropping any spine document that becomes an empty wrapper as a
    /// result.
    fn remove_old_cover(
        &self,
        oeb: &mut OEBBook,
        cover_item_id: &str,
        cover_href: &str,
        new_cover_href: Option<&str>,
    ) {
        oeb.manifest.remove(cover_item_id);

        let idrefs: Vec<String> = oeb.spine.iter().map(|s| s.idref.clone()).collect();
        let mut items_to_drop: Vec<(String, String)> = Vec::new(); // (idref, href)
        for idref in idrefs {
            let Some(href) = oeb.manifest.get_by_id(&idref).map(|i| i.href.clone()) else {
                continue;
            };
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            let html = String::from_utf8_lossy(&raw);
            let mut dom = crate::mobi::dom::Dom::parse(&html);
            let mut removed = false;
            for img in dom.find_all_tag_global("img") {
                let src = dom.node(img).attrs.get("src").cloned();
                if let Some(src) = src {
                    if crate::oeb::transforms::filenames::abshref(&href, &src) == cover_href {
                        if let Some(new_href) = new_cover_href {
                            let rel = crate::oeb::transforms::filenames::relhref(&href, new_href);
                            dom.node_mut(img).attrs.insert("src".to_string(), rel);
                        } else {
                            dom.remove_promoting_children(img);
                            removed = true;
                        }
                    }
                }
            }
            for image in dom.find_all_tag_global("image") {
                let xh = dom.node(image).attrs.get("xlink:href").cloned();
                if let Some(xh) = xh {
                    if crate::oeb::transforms::filenames::abshref(&href, &xh) == cover_href {
                        if let Some(new_href) = new_cover_href {
                            let rel = crate::oeb::transforms::filenames::relhref(&href, new_href);
                            dom.node_mut(image)
                                .attrs
                                .insert("xlink:href".to_string(), rel);
                        } else if let Some(svg) = dom.parent(image) {
                            dom.remove_promoting_children(svg);
                            removed = true;
                        }
                    }
                }
            }
            if removed {
                let has_img_or_svg = !dom.find_all_tag_global("img").is_empty()
                    || !dom.find_all_tag_global("svg").is_empty();
                let text_empty = dom
                    .find_first_tag_global("body")
                    .map(|b| dom.text_content(b).chars().all(|c| c.is_whitespace()))
                    .unwrap_or(true);
                if text_empty && !has_img_or_svg {
                    items_to_drop.push((idref, href));
                    continue;
                }
            }
            let rendered = dom.serialize(dom.root).into_bytes();
            let _ = oeb.container.write(&href, &rendered);
        }
        for (idref, href) in items_to_drop {
            oeb.spine.remove_by_idref(&idref);
            oeb.manifest.remove(&idref);
            oeb.guide.remove_by_href(&href);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    fn mi() -> MetaInformation {
        MetaInformation::new("A Title", vec!["Jane Doe".to_string()])
    }

    #[test]
    fn merges_title_and_author() {
        let mut m = crate::oeb::metadata::Metadata::new();
        let mi = mi();
        meta_info_to_oeb_metadata(&mi, &mut m, false);
        assert_eq!(m.first("title").unwrap().value, "A Title");
        assert_eq!(m.first("creator").unwrap().value, "Jane Doe");
        assert_eq!(
            m.first("creator").unwrap().get_attribute("role").unwrap(),
            "aut"
        );
    }

    #[test]
    fn identifier_scheme_updates_existing_value_in_place() {
        let mut m = crate::oeb::metadata::Metadata::new();
        let mut attrib = HashMap::new();
        attrib.insert("scheme".to_string(), "ISBN".to_string());
        m.add_with_attrib("identifier", "old-isbn", attrib);
        let mut mi = mi();
        mi.identifiers
            .insert("isbn".to_string(), "978-0-00-000000-0".to_string());
        meta_info_to_oeb_metadata(&mi, &mut m, false);
        let ids = m.get("identifier");
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].value, "978-0-00-000000-0");
    }

    #[test]
    fn merge_metadata_sets_new_cover_and_rewrites_wrapper_page_img() {
        let mut oeb = Builder::new()
            .part("old-cover.jpg", "image/jpeg", b"old", false)
            .page(
                "cover-page.xhtml",
                r#"<div><img src="old-cover.jpg"/></div>"#,
            )
            .build();
        oeb.guide
            .add("cover", Some("Cover".into()), "old-cover.jpg");

        let mut mi = mi();
        mi.cover_data = (Some("jpg".to_string()), b"newdata".to_vec());
        MergeMetadata.call(&mut oeb, &mi, false, "epub", false);

        // Old cover item gone, new one present.
        assert!(oeb.manifest.get_by_href("old-cover.jpg").is_none());
        let cover_ref = oeb.guide.get("cover").expect("cover guide ref kept");
        assert!(oeb.manifest.get_by_href(&cover_ref.href).is_some());
        // A new cover image was supplied, so the wrapper page's <img>
        // is rewritten to point at it (not removed) and the page
        // survives.
        let raw = oeb.container.read("cover-page.xhtml").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains(&cover_ref.href), "{html}");
        assert!(!html.contains("old-cover.jpg"), "{html}");
    }

    #[test]
    fn remove_old_cover_with_no_replacement_drops_now_empty_wrapper_page() {
        // `MergeMetadata::set_cover` only ever calls the private
        // `remove_old_cover` with a `Some` replacement href (a `cdata`
        // that triggers `do_remove_old_cover` always also creates a new
        // cover item first) -- matching Python, where the only call
        // site (`self.remove_old_cover(item, new_cover_item.href)`)
        // never passes `None` either, even though the method supports
        // it. Exercise that still-real "no replacement" branch directly.
        let mut oeb = Builder::new()
            .part("old-cover.jpg", "image/jpeg", b"old", false)
            .page(
                "cover-page.xhtml",
                r#"<div><img src="old-cover.jpg"/></div>"#,
            )
            .build();
        let cover_id = oeb
            .manifest
            .get_by_href("old-cover.jpg")
            .unwrap()
            .id
            .clone();
        MergeMetadata.remove_old_cover(&mut oeb, &cover_id, "old-cover.jpg", None);

        assert!(oeb.manifest.get_by_href("old-cover.jpg").is_none());
        assert!(oeb.manifest.get_by_href("cover-page.xhtml").is_none());
    }
}
