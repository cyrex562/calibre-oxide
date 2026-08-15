//! Port of `old_src/src/calibre/ebooks/oeb/polish/subset.py`.
//!
//! This file has two independent gaps, which this port keeps distinct
//! rather than conflating into one blanket "CSS parsing" excuse:
//!
//! - **CSS parsing** ([`remove_font_face_rules`]): needs
//!   `CSSStyleSheet.cssRules`/`@font-face` `src` property access from a
//!   real CSS parse -- the same, already-documented gap as
//!   `oeb::polish::utils::parse_css` (issues #34/#35/#36).
//! - **TrueType/OpenType font subsetting** ([`subset_all_fonts`]):
//!   Python's actual byte-level work is
//!   `calibre.utils.fonts.subset.subset` -- a substantial binary
//!   TrueType/OpenType table editor (glyph table rewriting, `cmap`
//!   reduction, `loca`/`glyf` repacking, ...) plus
//!   `calibre.utils.fonts.utils.get_font_names` (font name-table
//!   parsing). Neither exists in this crate and neither is something a
//!   narrow, in-scope helper can provide; this is a distinct, separately
//!   large porting effort (a real font-editing library), not a CSS gap.
//!
//! [`iter_subsettable_fonts`] needs neither: it is pure manifest
//! filtering and is ported for real.

use std::collections::HashSet;

use anyhow::Result;

use crate::oeb::polish::utils::OEB_FONTS;

use super::container::Container;

/// Port of `remove_font_face_rules`. Needs a real CSS parser to read
/// `@font-face` rules' `src` URIs and delete matched rules -- see the
/// module docs.
pub fn remove_font_face_rules(
    _container: &mut Container,
    _sheet_name: &str,
    _remove_names: &HashSet<String>,
) -> Result<bool> {
    todo!(
        "placeholder: needs a real CSS parser to enumerate @font-face rules \
         and their src URIs -- see oeb::polish::utils::parse_css's docs \
         (same gap as issues #34/#35/#36)"
    )
}

/// Port of `iter_subsettable_fonts`: every manifest entry that is (or
/// looks like, by extension) an embeddable TrueType/OpenType font.
pub fn iter_subsettable_fonts(container: &mut Container) -> Result<Vec<(String, String)>> {
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Ok(names
        .into_iter()
        .filter(|(name, mt)| {
            OEB_FONTS.contains(&mt.as_str())
                || matches!(
                    name.rsplit('.').next().map(|e| e.to_lowercase()).as_deref(),
                    Some("otf") | Some("ttf")
                )
        })
        .collect())
}

/// Port of `subset_all_fonts`: removes unused embedded fonts, and
/// shrinks used ones to only the glyphs the book actually references.
/// Needs `calibre.utils.fonts.subset.subset` and
/// `calibre.utils.fonts.utils.get_font_names` -- real TrueType/OpenType
/// binary editing this crate does not have. See the module docs; this is
/// a distinct gap from [`remove_font_face_rules`]'s CSS-parsing one.
pub fn subset_all_fonts(
    _container: &mut Container,
    _font_stats: &std::collections::HashMap<String, HashSet<char>>,
    _report: &mut dyn FnMut(&str),
) -> Result<bool> {
    todo!(
        "placeholder: needs a real TrueType/OpenType font subsetter \
         (calibre.utils.fonts.subset.subset: cmap/glyf/loca table \
         rewriting) plus font name-table parsing \
         (calibre.utils.fonts.utils.get_font_names) -- neither exists in \
         this crate. This is a font-editing-library gap, distinct from the \
         CSS-parsing gap in remove_font_face_rules."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(files: &[(&str, &str)]) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt) in files {
            fs::write(dir.path().join(name), b"x").unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine></spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn iter_subsettable_fonts_matches_font_mime_and_extension() {
        let (_dir, mut container) = make_container(&[
            ("a.ttf", "font/ttf"),
            ("b.otf", "font/otf"),
            ("c.jpg", "image/jpeg"),
            ("d.ttf", "application/octet-stream"),
        ]);
        let mut fonts: Vec<String> = iter_subsettable_fonts(&mut container)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        fonts.sort();
        assert_eq!(
            fonts,
            vec![
                "a.ttf".to_string(),
                "b.otf".to_string(),
                "d.ttf".to_string()
            ]
        );
    }
}
