//! Port of `old_src/src/calibre/ebooks/oeb/polish/subset.py`.
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
//! - **TrueType/OpenType font subsetting** ([`subset_all_fonts`]): real
//!   as of issue #553, via
//!   `calibre_utils::fonts::sfnt::subset::subset` (the hand-rolled
//!   glyph-reachability-closure TrueType subsetter the #548-552 fonts
//!   cluster built) and `calibre_utils::fonts::utils::get_font_names`
//!   (font-name reporting, #548). **This is a real, tested, cross-
//!   validated subsetter, but it is NOT a port of the CURRENT real
//!   `calibre.utils.fonts.subset.subset`** -- that function is a 2023
//!   rewrite delegating entirely to the third-party `fontTools`
//!   library's own layout-feature-aware `Subsetter`, which this port
//!   does not reimplement (disproportionate to this crate's scope; see
//!   `calibre_utils::fonts::sfnt::subset`'s own module doc for the full
//!   finding). What's wired in here is the OLDER, still-real,
//!   fully-hand-rolled `sfnt/subset.py` algorithm instead -- same
//!   observable goal (shrink an embedded font to only the glyphs a
//!   book's text needs), different implementation, not byte-identical
//!   output to current upstream. CFF-flavored (PostScript-outline)
//!   fonts aren't subsettable yet (issue #554); such a font is
//!   reported as unsupported and left untouched, matching how any
//!   other unsupported font is handled.
//!
//! [`iter_subsettable_fonts`] needs neither: it is pure manifest
//! filtering and is ported for real.

use std::collections::HashSet;

use anyhow::Result;

use crate::css::{Rule, Stylesheet};
use crate::dom::NodeId;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::fonts::{set_dom_element_text_only, style_tag_is_css, unquote};
use crate::oeb::polish::utils::OEB_FONTS;

use super::container::Container;

/// Shared retain-logic between [`remove_font_face_rules`] (a real
/// manifest CSS file) and [`subset_all_fonts`]'s inline-`<style>`-tag
/// handling (an in-memory `Stylesheet` with no manifest name of its
/// own to look itself up by) -- both need the exact same "does this
/// `@font-face` rule's `src` resolve to a removed font" test.
fn retain_non_removed_font_face_rules(container: &Container, sheet: &mut Stylesheet, remove_names: &HashSet<String>, base_name: &str) -> bool {
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
        let Some(name) = container.href_to_name(uri, Some(base_name)) else {
            return true;
        };
        if remove_names.contains(&name) {
            changed = true;
            false
        } else {
            true
        }
    });
    changed
}

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
    let changed = retain_non_removed_font_face_rules(container, &mut sheet, remove_names, sheet_name);
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
/// Uses `calibre_utils::fonts::sfnt::subset::subset` (issue #553) for
/// the real TrueType glyph-set reduction and
/// `calibre_utils::fonts::utils::get_font_names` (issue #548) for
/// font-name reporting. See the module docs for why this hand-rolled
/// `sfnt`-based subsetter is what's wired in here, rather than a port
/// of the *current* real `calibre.utils.fonts.subset.subset` (which
/// wraps the third-party `fontTools` library and isn't reimplemented).
pub fn subset_all_fonts(
    container: &mut Container,
    font_stats: &std::collections::HashMap<String, HashSet<char>>,
    report: &mut dyn FnMut(&str),
) -> Result<bool> {
    let mut remove: HashSet<String> = HashSet::new();
    let mut total_old: u64 = 0;
    let mut total_new: u64 = 0;
    let mut changed = false;

    for (name, _mt) in iter_subsettable_fonts(container)? {
        let chars = font_stats.get(&name).cloned().unwrap_or_default();
        let font_size = container.filesize(&name)?;
        if chars.is_empty() {
            remove.insert(name.clone());
            report(&format!("Removed unused font: {name}"));
            continue;
        }

        let raw = container.raw_data(&name, false)?;
        let font_name = match calibre_utils::fonts::utils::get_font_names(&raw) {
            Ok((_, _, full_name)) => full_name,
            Err(e) => {
                report(&format!("Corrupted font: {name}, ignoring.  Error: {e}"));
                continue;
            }
        };
        report(&format!("Subsetting font: {}", font_name.clone().unwrap_or_else(|| name.clone())));

        let (nraw, warnings) = match calibre_utils::fonts::sfnt::subset::subset(&raw, &chars) {
            Ok((nraw, _old_sizes, _new_sizes, warnings)) => (nraw, warnings),
            Err(e) => {
                report(&format!("Unsupported font: {name}, ignoring. Error: {e}"));
                continue;
            }
        };

        total_old += font_size;
        for w in &warnings {
            report(w);
        }
        let (olen, nlen) = (raw.len(), nraw.len());
        total_new += nlen as u64;
        let display_name = font_name.unwrap_or_else(|| name.clone());
        if nlen == olen {
            report(&format!("The font {display_name} was already subset"));
        } else {
            report(&format!("Decreased the font {display_name} to {:.1}% of its original size", nlen as f64 / olen as f64 * 100.0));
            changed = true;
        }
        container.write_file(&name, &nraw)?;
    }

    for name in &remove {
        container.remove_item(name, true)?;
        changed = true;
    }

    if !remove.is_empty() {
        let names: Vec<(String, String)> = container.base.mime_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (name, mt) in names {
            if OEB_STYLES.contains(&mt.as_str()) {
                if remove_font_face_rules(container, &name, &remove)? {
                    container.dirty(&name);
                }
            } else if OEB_DOCS.contains(&mt.as_str()) {
                container.ensure_parsed(&name)?;
                let style_tags: Vec<(NodeId, String)> = {
                    let dom = container.get_xhtml(&name)?;
                    dom.preorder_elements(dom.root)
                        .into_iter()
                        .filter(|&id| dom.tag(id) == Some("style") && style_tag_is_css(dom, id))
                        .filter_map(|id| {
                            let text = dom.text_content(id);
                            if text.trim().is_empty() { None } else { Some((id, text)) }
                        })
                        .collect()
                };
                let mut style_tag_updates = Vec::new();
                for (id, text) in style_tags {
                    let mut sheet = Stylesheet::parse(&text);
                    if retain_non_removed_font_face_rules(container, &mut sheet, &remove, &name) {
                        style_tag_updates.push((id, sheet.to_css_text()));
                    }
                }
                if !style_tag_updates.is_empty() {
                    let dom = container.get_xhtml_mut(&name)?;
                    for (id, text) in style_tag_updates {
                        set_dom_element_text_only(dom, id, &text);
                    }
                    container.dirty(&name);
                }
            }
        }
    }

    if total_old > 0 {
        report(&format!("Reduced total font size to {:.1}% of original", total_new as f64 / total_old as f64 * 100.0));
    } else {
        report("No embedded fonts found");
    }

    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
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

    /// Builds a real, minimal, valid TrueType font (glyph 0 `.notdef`
    /// plus one simple glyph per entry in `mappings`) using
    /// `calibre_utils`'s own real, already-tested `sfnt` table types
    /// directly -- exercising the real public API surface this file
    /// depends on, rather than hand-writing a fourth copy of the same
    /// binary layout.
    fn build_font_bytes(mappings: &[(char, u16)]) -> Vec<u8> {
        use calibre_utils::fonts::sfnt::cmap::CmapTable;
        use calibre_utils::fonts::sfnt::glyf::GlyfTable;
        use calibre_utils::fonts::sfnt::head::HeadTable;
        use calibre_utils::fonts::sfnt::loca::LocaTable;
        use calibre_utils::fonts::sfnt::maxp::MaxpTable;
        use calibre_utils::fonts::sfnt::max_power_of_two;
        use calibre_utils::fonts::utils::checksum_of_block;

        let mut glyph_bytes: Vec<(usize, Vec<u8>)> = Vec::new();
        glyph_bytes.push((0, {
            let mut g = 1i16.to_be_bytes().to_vec();
            g.extend_from_slice(&[0u8; 8]);
            g.push(0xAA);
            g
        }));
        for (i, &(_, glyph_id)) in mappings.iter().enumerate() {
            let mut g = 1i16.to_be_bytes().to_vec();
            g.extend_from_slice(&[0u8; 8]);
            g.push(0xB0 + i as u8);
            glyph_bytes.push((glyph_id as usize, g));
        }
        glyph_bytes.sort_by_key(|&(id, _)| id);

        let mut glyf = GlyfTable::new(Vec::new());
        let offsets = glyf.update(&glyph_bytes);
        let mut loca = LocaTable::default();
        loca.update(&offsets);

        let head = HeadTable {
            version_number: 1 << 16,
            font_revision: 0,
            checksum_adjustment: 0,
            magic_number: 0x5f0f3cf5,
            flags: 0,
            units_per_em: 1000,
            created: 0,
            modified: 0,
            x_min: 0,
            y_min: 0,
            x_max: 1000,
            y_max: 1000,
            mac_style: 0,
            lowest_rec_ppem: 9,
            font_direction_hint: 2,
            index_to_loc_format: if loca.is_long_format { 1 } else { 0 },
            glyph_data_format: 0,
        };
        let maxp = MaxpTable { version: 0x0000_5000, num_glyphs: (loca.offset_map.len() - 1) as u16, v1: None };

        let mut cmap = CmapTable::parse(vec![0, 0, 0, 0]).unwrap();
        let cmap_map: BTreeMap<u32, u32> = mappings.iter().map(|&(c, g)| (c as u32, g as u32)).collect();
        cmap.set_character_map(&cmap_map);

        // A minimal but valid `name` table (0 records) -- required so
        // `get_font_names` (called by `subset_all_fonts` for reporting)
        // finds a `name` table to look in at all, even though this
        // fixture doesn't care what it says.
        let mut name_table = Vec::new();
        name_table.extend_from_slice(&0u16.to_be_bytes()); // format
        name_table.extend_from_slice(&0u16.to_be_bytes()); // count
        name_table.extend_from_slice(&6u16.to_be_bytes()); // stringOffset

        let tables: [(&[u8; 4], Vec<u8>); 6] = [
            (b"head", head.to_bytes()),
            (b"maxp", maxp.to_bytes()),
            (b"loca", loca.to_bytes()),
            (b"glyf", glyf.raw.clone()),
            (b"cmap", cmap.raw.clone()),
            (b"name", name_table),
        ];

        let num_tables = tables.len() as u32;
        let ln2 = max_power_of_two(num_tables);
        let srange = (1u32 << ln2) * 16;
        let mut out = Vec::new();
        out.extend_from_slice(&[0x00, 0x01, 0x00, 0x00]);
        out.extend_from_slice(&(num_tables as u16).to_be_bytes());
        out.extend_from_slice(&(srange as u16).to_be_bytes());
        out.extend_from_slice(&(ln2 as u16).to_be_bytes());
        out.extend_from_slice(&((num_tables * 16).wrapping_sub(srange) as u16).to_be_bytes());

        let header_len = 12 + tables.len() * 16;
        let mut data_section = Vec::new();
        let mut records = Vec::new();
        let mut offset = header_len;
        for (tag, data) in &tables {
            let checksum = checksum_of_block(data);
            records.push((**tag, checksum, offset, data.len()));
            data_section.extend_from_slice(data);
            while data_section.len() % 4 != 0 {
                data_section.push(0);
            }
            offset = header_len + data_section.len();
        }
        for (tag, checksum, table_offset, table_length) in records {
            out.extend_from_slice(&tag);
            out.extend_from_slice(&checksum.to_be_bytes());
            out.extend_from_slice(&(table_offset as u32).to_be_bytes());
            out.extend_from_slice(&(table_length as u32).to_be_bytes());
        }
        out.extend_from_slice(&data_section);
        out
    }

    #[test]
    fn subset_all_fonts_subsets_used_fonts_and_removes_unused_ones_everywhere() {
        let font_a = build_font_bytes(&[('A', 1), ('B', 2), ('Z', 3)]);
        let font_b = build_font_bytes(&[('Q', 1)]);

        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        fs::write(dir.path().join("keep.ttf"), &font_a).unwrap();
        fs::write(dir.path().join("drop.ttf"), &font_b).unwrap();
        fs::write(
            dir.path().join("style.css"),
            b"@font-face { font-family: Keep; src: url(keep.ttf) } \
              @font-face { font-family: Drop; src: url(drop.ttf) }",
        )
        .unwrap();
        fs::write(
            dir.path().join("index.html"),
            b"<html><head><style>@font-face { font-family: Drop2; src: url(drop.ttf) }</style></head>\
              <body>x</body></html>",
        )
        .unwrap();
        let opf = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>
    <item id="keep" href="keep.ttf" media-type="font/ttf"/>
    <item id="drop" href="drop.ttf" media-type="font/ttf"/>
    <item id="style" href="style.css" media-type="text/css"/>
    <item id="index" href="index.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine></spine>
</package>"#;
        fs::write(&opf_path, opf).unwrap();
        let mut container = Container::open(dir.path(), &opf_path).unwrap();

        let mut font_stats = std::collections::HashMap::new();
        font_stats.insert("keep.ttf".to_string(), HashSet::from(['A', 'B']));
        // "drop.ttf" is deliberately absent from font_stats (unused -> removed).

        let mut messages = Vec::new();
        let mut report = |m: &str| messages.push(m.to_string());
        let changed = subset_all_fonts(&mut container, &font_stats, &mut report).unwrap();
        assert!(changed);
        assert!(messages.iter().any(|m| m.contains("Removed unused font: drop.ttf")));
        assert!(messages.iter().any(|m| m.starts_with("Decreased the font keep.ttf")));

        assert!(!container.base.mime_map.contains_key("drop.ttf"), "the unused font should be removed from the manifest");

        let new_keep = container.raw_data("keep.ttf", false).unwrap();
        let sfnt = calibre_utils::fonts::sfnt::container::Sfnt::parse(&new_keep).unwrap();
        let old_sfnt = calibre_utils::fonts::sfnt::container::Sfnt::parse(&font_a).unwrap();
        assert!(
            sfnt.sizes()[&*b"glyf"] < old_sfnt.sizes()[&*b"glyf"],
            "the glyf table should shrink once 'Z's glyph data is dropped (whole-font byte count can be dominated by fixed per-table alignment overhead at this toy scale, so compare the glyf table directly instead)"
        );
        let cmap = calibre_utils::fonts::sfnt::cmap::CmapTable::parse(sfnt.get(b"cmap").unwrap().clone()).unwrap();
        let map = cmap.get_character_map(&['A' as u32, 'Z' as u32]).unwrap();
        assert!(map.contains_key(&('A' as u32)), "'A' was requested and must still resolve");
        assert!(!map.contains_key(&('Z' as u32)), "'Z' was never requested and should have been dropped");

        let sheet = container.parsed_stylesheet("style.css").unwrap();
        let families: Vec<String> = sheet.font_face_rules().map(|d| d.get_property_value("font-family").to_string()).collect();
        assert_eq!(families, vec!["Keep".to_string()], "the @font-face rule for the removed font should be stripped from the CSS file");

        let dom = container.get_xhtml("index.html").unwrap();
        let style_tag = dom.find_first_tag_global("style").unwrap();
        let inline_sheet = Stylesheet::parse(&dom.text_content(style_tag));
        assert_eq!(inline_sheet.rules.len(), 0, "the inline <style> block's @font-face rule for the removed font should also be stripped");
        assert!(container.dirtied.contains("index.html"));
    }
}
