//! Port of `old_src/src/calibre/ebooks/oeb/polish/subset.py`.
//!
//! This file has two independent gaps, which this port keeps distinct
//! rather than conflating into one blanket "CSS parsing" excuse:
//!
//! - **CSS parsing** ([`remove_font_face_rules`]): needed
//!   `CSSStyleSheet.cssRules`/`@font-face` `src` property access from a
//!   real CSS parse -- closed for real by issue #164's [`crate::css`].
//!   Python's version takes an already-parsed `sheet` object (the
//!   caller, `subset_all_fonts`, is responsible for writing it back);
//!   since this crate doesn't cache a structured `Stylesheet` the way
//!   Python's `container.parsed(name)` caches a live `CSSStyleSheet`
//!   (see [`super::container::ParsedItem::Css`]'s docs), this port's
//!   version takes the CSS file's `sheet_name` instead and is
//!   self-contained: it reads, mutates, writes back
//!   ([`super::container::Container::set_css_text`]) and marks the file
//!   dirty itself when it changes anything.
//! - **TrueType/OpenType font subsetting** ([`subset_all_fonts`]):
//!   Python's actual byte-level work is
//!   `calibre.utils.fonts.subset.subset` -- a substantial binary
//!   TrueType/OpenType table editor (glyph table rewriting, `cmap`
//!   reduction, `loca`/`glyf` repacking, ...) plus
//!   `calibre.utils.fonts.utils.get_font_names` (font name-table
//!   parsing). Neither exists in this crate and neither is something a
//!   narrow, in-scope helper can provide; this is a distinct, separately
//!   large porting effort (a real font-editing library), not a CSS gap.
//!   Still `todo!()`.
//!
//! [`iter_subsettable_fonts`] needs neither: it is pure manifest
//! filtering and is ported for real.

use std::collections::HashSet;

use anyhow::Result;

use crate::css::Rule;
use crate::oeb::polish::fonts::unquote;
use crate::oeb::polish::utils::OEB_FONTS;

use super::container::Container;

/// Port of `remove_font_face_rules`: removes every `@font-face` rule in
/// `sheet_name` whose `src` resolves (relative to `sheet_name`) to a
/// name in `remove_names`. Returns whether anything was removed; when it
/// is, the sheet's new text has already been written back via
/// [`Container::set_css_text`] and [`Container::dirty`] has already been
/// called -- see the module docs for why this differs from Python's
/// caller-writes-it-back shape.
pub fn remove_font_face_rules(
    container: &mut Container,
    sheet_name: &str,
    remove_names: &HashSet<String>,
) -> Result<bool> {
    let mut sheet = container.parsed_stylesheet(sheet_name)?;
    let mut changed = false;
    sheet.rules.retain(|rule| {
        let Rule::FontFace(decl) = rule else {
            return true;
        };
        let Some(src) = decl.get_property("src") else {
            return true;
        };
        let mut uri = src.value.clone();
        if let Some(rest) = uri.strip_prefix("url(") {
            uri = rest.strip_suffix(')').unwrap_or(rest).to_string();
        }
        let uri = unquote(&uri);
        let Some(name) = container.href_to_name(uri, Some(sheet_name)) else {
            return true;
        };
        if remove_names.contains(&name) {
            changed = true;
            false
        } else {
            true
        }
    });
    if changed {
        container.set_css_text(sheet_name, sheet.to_css_text());
        container.dirty(sheet_name);
    }
    Ok(changed)
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

    fn make_container_with_content(
        files: &[(&str, &str, &[u8])],
    ) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt, content) in files {
            fs::write(dir.path().join(name), content).unwrap();
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
    fn remove_font_face_rules_removes_matched_rule_and_keeps_others() {
        let (_dir, mut container) = make_container_with_content(&[(
            "style.css",
            "text/css",
            b"@font-face { font-family: A; src: url(a.otf) } \
              @font-face { font-family: B; src: url(b.otf) } \
              p { color: red }",
        )]);
        let mut remove = HashSet::new();
        remove.insert("a.otf".to_string());
        let changed = remove_font_face_rules(&mut container, "style.css", &remove).unwrap();
        assert!(changed);
        assert!(container.dirtied.contains("style.css"));
        let sheet = container.parsed_stylesheet("style.css").unwrap();
        assert_eq!(
            sheet.rules.len(),
            2,
            "keeps the B @font-face rule and the style rule"
        );
        let families: Vec<String> = sheet
            .font_face_rules()
            .map(|d| d.get_property_value("font-family").to_string())
            .collect();
        assert_eq!(families, vec!["B".to_string()]);
    }

    #[test]
    fn remove_font_face_rules_is_a_no_op_when_nothing_matches() {
        let (_dir, mut container) = make_container_with_content(&[(
            "style.css",
            "text/css",
            b"@font-face { font-family: A; src: url(a.otf) }",
        )]);
        let remove = HashSet::new();
        let changed = remove_font_face_rules(&mut container, "style.css", &remove).unwrap();
        assert!(!changed);
        assert!(!container.dirtied.contains("style.css"));
    }
}
