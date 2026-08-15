//! Port of `old_src/src/calibre/ebooks/oeb/transforms/guide.py`.

use crate::oeb::book::OEBBook;

/// Guide reference types Clean leaves alone. `title-page` and
/// `copyright-page` are listed twice in the Python source (a harmless
/// duplicate in a set literal); listed once here.
const KNOWN_GUIDE_TYPES: &[&str] = &[
    "cover",
    "titlepage",
    "masthead",
    "toc",
    "title-page",
    "copyright-page",
    "text",
    "index",
    "glossary",
    "acknowledgements",
    "bibliography",
    "colophon",
    "dedication",
    "epigraph",
    "foreword",
    "loi",
    "lot",
    "notes",
    "preface",
];

/// Candidate guide reference types considered for the synthetic `cover`
/// entry, in Python's preference order (used only to break ties when
/// two candidates have equal byte size -- see [`Clean::call`]).
const COVER_CANDIDATES: &[&str] = &[
    "other.ms-coverimage-standard",
    "coverimagestandard",
    "other.ms-titleimage-standard",
    "other.ms-titleimage",
    "other.ms-coverimage",
    "other.ms-thumbimage-standard",
    "other.ms-thumbimage",
    "thumbimagestandard",
];

/// Port of `Clean`: clean up `oeb.guide`, leaving only known reference
/// types.
pub struct Clean;

impl Clean {
    pub fn call(&self, oeb: &mut OEBBook) {
        if !oeb.guide.references.contains_key("cover") {
            self.pick_cover(oeb);
        }

        if oeb.guide.references.contains_key("start") && !oeb.guide.references.contains_key("text")
        {
            // Prefer text to start as per the OPF 2.0 spec.
            let (title, href) = {
                let x = oeb.guide.references.get("start").expect("checked above");
                (x.title.clone(), x.href.clone())
            };
            oeb.guide.add("text", title, &href);
            oeb.guide.remove("start");
        }

        let types: Vec<String> = oeb.guide.types().cloned().collect();
        for x in types {
            if KNOWN_GUIDE_TYPES.contains(&x.to_lowercase().as_str()) {
                continue;
            }
            let title_is_start = oeb
                .guide
                .get(&x)
                .and_then(|item| item.title.as_deref())
                .map(|t| t.eq_ignore_ascii_case("start"))
                .unwrap_or(false);
            if title_is_start {
                continue;
            }
            oeb.guide.remove(&x);
        }
    }

    /// Choose the largest (by referenced file size) of the known
    /// Microsoft/legacy cover-image guide reference types and register
    /// it as `cover`. The losing entries are left in place -- the
    /// unknown-type cleanup pass below removes them, matching Python's
    /// two-pass behavior exactly.
    fn pick_cover(&self, oeb: &mut OEBBook) {
        let mut covers: Vec<(String, u64)> = Vec::new();
        for &x in COVER_CANDIDATES {
            let Some(href) = oeb.guide.get(x).map(|r| r.href.clone()) else {
                continue;
            };
            let Some(item) = oeb.manifest.get_by_href(&href) else {
                continue;
            };
            let size = oeb.container.read(&item.href).map(|d| d.len()).unwrap_or(0) as u64;
            covers.push((x.to_string(), size));
        }
        // Stable descending sort by size, matching Python's
        // `covers.sort(key=..., reverse=True)`.
        covers.sort_by_key(|c| std::cmp::Reverse(c.1));
        if let Some((winner_type, _)) = covers.first() {
            let (title, href) = {
                let r = oeb.guide.get(winner_type).expect("just found above");
                (r.title.clone(), r.href.clone())
            };
            oeb.guide.add("cover", title, &href);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn picks_the_largest_legacy_cover_candidate() {
        let mut oeb = Builder::new()
            .part("small.jpg", "image/jpeg", &[0u8; 10], false)
            .part("big.jpg", "image/jpeg", &[0u8; 1000], false)
            .build();
        oeb.guide
            .add("other.ms-thumbimage", Some("Thumb".into()), "small.jpg");
        oeb.guide
            .add("other.ms-coverimage", Some("Cover".into()), "big.jpg");
        Clean.call(&mut oeb);
        let cover = oeb.guide.get("cover").expect("cover added");
        assert_eq!(cover.href, "big.jpg");
        // The losing legacy entry is removed as an unknown guide type.
        assert!(oeb.guide.get("other.ms-thumbimage").is_none());
        assert!(oeb.guide.get("other.ms-coverimage").is_none());
    }

    #[test]
    fn start_becomes_text_when_no_text_reference_exists() {
        let mut oeb = Builder::new().build();
        oeb.guide.add("start", Some("Begin".into()), "ch1.html");
        Clean.call(&mut oeb);
        assert!(oeb.guide.get("start").is_none());
        let text = oeb.guide.get("text").expect("text added");
        assert_eq!(text.href, "ch1.html");
    }

    #[test]
    fn unknown_guide_types_are_removed_unless_titled_start() {
        let mut oeb = Builder::new().build();
        oeb.guide.add("random-junk", Some("Junk".into()), "x.html");
        oeb.guide
            .add("other.custom", Some("Start".into()), "y.html");
        Clean.call(&mut oeb);
        assert!(oeb.guide.get("random-junk").is_none());
        // Titled "start" (case-insensitively), so kept even though its
        // type isn't in the known set.
        assert!(oeb.guide.get("other.custom").is_some());
    }

    #[test]
    fn known_guide_types_survive() {
        let mut oeb = Builder::new().build();
        oeb.guide.add("toc", Some("TOC".into()), "toc.html");
        oeb.guide.add("index", Some("Index".into()), "idx.html");
        Clean.call(&mut oeb);
        assert!(oeb.guide.get("toc").is_some());
        assert!(oeb.guide.get("index").is_some());
    }
}
