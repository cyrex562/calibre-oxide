//! Collect and encode image/font manifest items into MOBI image/FONT
//! records.
//!
//! Port of `calibre.ebooks.mobi.writer2.resources.Resources`.
//!
//! # Scope gap: real image transcoding
//!
//! `mobify_image`/`rescale_image` (`crate::mobi::utils`) are pre-existing
//! stubs from issue #33's port (no image-decoding crate is a dependency
//! of this crate yet): `mobify_image` is a passthrough and `rescale_image`
//! currently returns an empty buffer. This module calls them exactly as
//! `resources.py` does (so the *structural* logic here -- which images
//! get embedded, in what order, cover/thumbnail/masthead bookkeeping,
//! placeholder substitution for unused images -- is real and load-bearing)
//! but inherits their pixel-level limitation. `convert_webp` similarly has
//! no real WebP decoder to call and is a documented passthrough.
//! `generate_default_masthead` (the Python `calibre.ebooks.generate_masthead`,
//! which rasterizes a title string with PIL) has no text-rendering
//! equivalent available either, so a periodical with no explicit masthead
//! image gets a minimal placeholder GIF instead of a rendered title card.

use std::collections::HashSet;

use crate::mobi::utils::write_font_record;
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_RASTER_IMAGES;

/// A 1x1 transparent GIF, written into any image record slot whose image
/// turned out to be unused by the serialized text. `PLACEHOLDER_GIF` in
/// `resources.py`.
pub const PLACEHOLDER_GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\xf0\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00!\xfe calibre-placeholder-gif-for-azw3\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

/// Options `Resources::add_resources`/`process_image` read from Python's
/// `opts`. Only the fields `resources.py` actually consults.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceOpts {
    /// `opts.mobi_keep_original_images`: skip rescaling, only run the
    /// (currently passthrough) `mobify_image` PNG->GIF conversion.
    pub mobi_keep_original_images: bool,
}

/// Builds a "default" masthead placeholder for a periodical that has no
/// explicit `<guide type="masthead">` item.
///
/// Stand-in for `calibre.ebooks.generate_masthead(title)`, which
/// rasterizes `title` as an image with PIL -- see the module-level scope
/// note. Real title text is not rendered.
pub fn generate_default_masthead(_title: &str) -> Vec<u8> {
    PLACEHOLDER_GIF.to_vec()
}

/// Convert a JPEG/PNG/GIF (nominally -- see the module scope note, this
/// is currently identity) through calibre's Amazon-compatibility and
/// resize passes.
fn process_image(data: &[u8], opts: ResourceOpts) -> Vec<u8> {
    if opts.mobi_keep_original_images {
        crate::mobi::utils::mobify_image(data).unwrap_or_else(|_| data.to_vec())
    } else {
        crate::mobi::utils::rescale_image(data, super::PALM_MAX_IMAGE_SIZE, None)
            .unwrap_or_else(|_| data.to_vec())
    }
}

/// Collects image (and, optionally, font) manifest items into the flat
/// record list the MOBI writer appends after text records.
///
/// Port of `resources.py`'s `Resources` class.
#[derive(Debug, Default)]
pub struct Resources {
    /// href (as it appears in the manifest) -> 1-based image record
    /// index. `item_map` in Python; also used for font hrefs when
    /// `add_fonts` is set.
    pub item_map: std::collections::HashMap<String, usize>,
    /// The record payloads, in final emission order. Index 0 here is
    /// the masthead slot when one exists (matching `records[0]` in
    /// Python).
    pub records: Vec<Option<Vec<u8>>>,
    /// href -> MIME type of each embedded image (`image/gif` etc, as
    /// sniffed from the processed bytes -- currently always the input's
    /// own extension guess since no real decode happens, see the module
    /// scope note).
    pub mime_map: std::collections::HashMap<String, String>,
    /// 0-based index of the masthead record within `records`, if any
    /// (always 0 when present, matching Python's fixed slot 0).
    pub masthead_offset: usize,
    /// 0-based indices of records that must always be kept (masthead,
    /// cover, thumbnail) even if nothing in the serialized text
    /// references them.
    pub used_image_indices: HashSet<usize>,
    /// 0-based indices of every image record (as opposed to font
    /// records), used by `serialize` to know which unreferenced slots
    /// to blank out with [`PLACEHOLDER_GIF`].
    pub image_indices: HashSet<usize>,
    pub cover_offset: Option<usize>,
    pub thumbnail_offset: Option<usize>,
    pub has_fonts: bool,
}

impl Resources {
    /// Port of `Resources.__init__` + `add_resources`.
    ///
    /// `add_fonts`: embed manifest `.ttf`/`.otf` items as FONT records
    /// (`kf8`-only in Python's caller, exposed here for completeness).
    pub fn new(oeb: &OEBBook, opts: ResourceOpts, is_periodical: bool, add_fonts: bool) -> Self {
        let mut this = Resources::default();
        this.add_resources(oeb, opts, is_periodical, add_fonts);
        this
    }

    fn add_resources(
        &mut self,
        oeb: &OEBBook,
        opts: ResourceOpts,
        is_periodical: bool,
        add_fonts: bool,
    ) {
        let mut index: usize = 1;

        let mh_href = oeb
            .guide
            .get("masthead")
            .filter(|r| !r.href.is_empty())
            .map(|r| r.href.clone());

        if let Some(_href) = &mh_href {
            self.records.push(None);
            index += 1;
            self.used_image_indices.insert(0);
            self.image_indices.insert(0);
        } else if is_periodical {
            let title = oeb
                .metadata
                .get("title")
                .first()
                .map(|i| i.value.clone())
                .unwrap_or_default();
            let data = generate_default_masthead(&title);
            self.records.push(Some(data));
            self.used_image_indices.insert(0);
            self.image_indices.insert(0);
            index += 1;
        }

        let cover_href: Option<String> = oeb
            .metadata
            .get("cover")
            .first()
            .and_then(|item| oeb.manifest.get_by_id(&item.value).map(|m| m.href.clone()));

        for item in oeb.manifest.iter() {
            if !OEB_RASTER_IMAGES
                .iter()
                .any(|mt| mt.eq_ignore_ascii_case(&item.media_type))
            {
                continue;
            }
            let mut data = item_data(oeb, item);
            if item.media_type.eq_ignore_ascii_case("image/webp") {
                data = convert_webp(data);
            }
            let data = process_image(&data, opts);

            if let Some(mh) = &mh_href {
                if &item.href == mh {
                    if let Some(slot) = self.records.first_mut() {
                        *slot = Some(data);
                    }
                    continue;
                }
            }

            self.image_indices.insert(self.records.len());
            self.records.push(Some(data.clone()));
            self.item_map.insert(item.href.clone(), index);
            self.mime_map
                .insert(item.href.clone(), format!("image/{}", sniff_ext(&data)));
            index += 1;

            if cover_href.as_deref() == Some(item.href.as_str()) {
                let cover_offset = self.item_map[&item.href] - 1;
                self.cover_offset = Some(cover_offset);
                self.used_image_indices.insert(cover_offset);
                match crate::mobi::utils::rescale_image(
                    &data,
                    super::MAX_THUMB_SIZE,
                    Some(super::MAX_THUMB_DIMEN),
                ) {
                    Ok(tdata) => {
                        self.image_indices.insert(self.records.len());
                        self.records.push(Some(tdata));
                        self.thumbnail_offset = Some(index - 1);
                        self.used_image_indices.insert(index - 1);
                        index += 1;
                    }
                    Err(_) => {
                        // Failed to generate thumbnail: warn-and-continue,
                        // matching Python's `log.warn` + fallthrough.
                    }
                }
            }
        }

        if add_fonts {
            for item in oeb.manifest.iter() {
                let ext = item
                    .href
                    .rsplit('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if ext != "ttf" && ext != "otf" {
                    continue;
                }
                let data = item_data(oeb, item);
                if data.is_empty() {
                    continue;
                }
                self.records
                    .push(Some(write_font_record(&data, true, true)));
                self.item_map.insert(item.href.clone(), self.records.len());
                self.has_fonts = true;
            }
        }
    }

    /// Append this resource's records to `records`, blanking out any
    /// image record that ended up unused by the serialized text (its
    /// href never appeared in an `<img src>`/`recindex`) with
    /// [`PLACEHOLDER_GIF`].
    ///
    /// Port of `Resources.serialize`.
    pub fn serialize(&mut self, records: &mut Vec<Vec<u8>>, used_images: &HashSet<String>) {
        let mut used_image_indices = self.used_image_indices.clone();
        for (href, &idx) in &self.item_map {
            if used_images.contains(href) {
                used_image_indices.insert(idx - 1);
            }
        }
        for &i in self.image_indices.difference(&used_image_indices) {
            if let Some(slot) = self.records.get_mut(i) {
                *slot = Some(PLACEHOLDER_GIF.to_vec());
            }
        }
        for rec in &self.records {
            records.push(rec.clone().unwrap_or_default());
        }
    }

    /// `Resources.__bool__`.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

fn item_data(oeb: &OEBBook, item: &crate::oeb::manifest::ManifestItem) -> Vec<u8> {
    oeb.container.read(&item.href).unwrap_or_default()
}

/// No real WebP decoder is available (see module scope note); returns
/// the input unchanged rather than failing the whole resource pass.
fn convert_webp(data: Vec<u8>) -> Vec<u8> {
    data
}

/// Best-effort image format sniff from magic bytes, used only to label
/// `mime_map` entries (`image/{fmt}` in Python, via `imghdr.what`).
fn sniff_ext(data: &[u8]) -> &'static str {
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        "gif"
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if data.starts_with(b"\xff\xd8\xff") {
        "jpeg"
    } else if data.len() >= 12 && &data[8..12] == b"WEBP" {
        "webp"
    } else {
        "gif"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
        v.extend_from_slice(&[0u8; 16]);
        v
    }

    fn book_with_one_image(dir: &std::path::Path) -> OEBBook {
        std::fs::write(dir.join("cover.png"), png_bytes()).unwrap();
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
        oeb.manifest.add("cover-img", "cover.png", "image/png");
        oeb.metadata.add("cover", "cover-img");
        oeb.metadata.add("title", "A Book");
        oeb
    }

    #[test]
    fn collects_a_cover_image_and_marks_it_used() {
        let dir = tempfile::tempdir().unwrap();
        let oeb = book_with_one_image(dir.path());
        let opts = ResourceOpts {
            mobi_keep_original_images: true,
        };
        let res = Resources::new(&oeb, opts, false, false);
        assert_eq!(res.item_map.get("cover.png"), Some(&1));
        assert_eq!(res.cover_offset, Some(0));
        assert!(res.used_image_indices.contains(&0));
        assert!(!res.is_empty());
    }

    #[test]
    fn periodical_without_masthead_gets_a_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let oeb = book_with_one_image(dir.path());
        let opts = ResourceOpts {
            mobi_keep_original_images: true,
        };
        let res = Resources::new(&oeb, opts, true, false);
        assert_eq!(res.masthead_offset, 0);
        assert!(res.records[0].is_some());
    }

    #[test]
    fn serialize_blanks_unused_images() {
        let dir = tempfile::tempdir().unwrap();
        let oeb = book_with_one_image(dir.path());
        let opts = ResourceOpts {
            mobi_keep_original_images: true,
        };
        let mut res = Resources::new(&oeb, opts, false, false);
        let mut out = Vec::new();
        // Nothing in `used_images` references cover.png, but it's still
        // pinned via `used_image_indices` because it's the cover. A
        // thumbnail record follows it (the cover always attempts one --
        // it comes out empty here because `rescale_image` is currently
        // a stub, see the module scope note, but the record slot is
        // real).
        res.serialize(&mut out, &HashSet::new());
        assert_eq!(out.len(), 2);
        assert_ne!(out[0], PLACEHOLDER_GIF);
    }

    #[test]
    fn an_unreferenced_non_cover_image_is_replaced_with_the_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("extra.png"), png_bytes()).unwrap();
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
        oeb.manifest.add("extra-img", "extra.png", "image/png");
        let opts = ResourceOpts {
            mobi_keep_original_images: true,
        };
        let mut res = Resources::new(&oeb, opts, false, false);
        let mut out = Vec::new();
        res.serialize(&mut out, &HashSet::new());
        assert_eq!(out[0], PLACEHOLDER_GIF);
    }
}
