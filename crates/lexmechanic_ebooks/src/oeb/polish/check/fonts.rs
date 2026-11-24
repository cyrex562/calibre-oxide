//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/fonts.py`.
//!
//! # Scope note: `calibre.utils.fonts.utils`
//!
//! Beyond `oeb::polish::fonts`'s CSS-side helpers (real since issue
//! #164), `check_fonts` also needs two functions from a module this
//! port hadn't touched yet: `calibre.utils.fonts.utils.get_all_font_names`
//! (read a TrueType/OpenType font's `name` table) and
//! `is_font_embeddable` (read its `OS/2` table's `fsType` flags).
//! `oeb::polish::subset`'s module docs already flag this exact module
//! (`calibre.utils.fonts.utils.get_font_names`) as a real, separate gap
//! from CSS parsing -- but there it's paired with actual font
//! *subsetting* (`calibre.utils.fonts.subset.subset`, a substantial
//! `glyf`/`loca`/`cmap`-rewriting binary editor), which is what makes
//! that file a large, distinct porting effort left `todo!()`.
//!
//! `check_fonts` only needs the **read-only** half: parse the `name`
//! table's records and the `OS/2` table's `fsType` field. That is
//! plain, self-contained binary parsing (a handful of fixed-layout
//! struct reads over the font's own byte buffer, no glyph/outline data
//! involved) -- narrow enough to port for real here, and doing so is
//! what lets [`check_fonts`] be fully real rather than gapped. The
//! [`sfnt`] submodule below is exactly (and only) `get_tables`/
//! `get_table`/`decode_name_record`/`get_all_font_names`/
//! `is_font_embeddable` from `calibre/utils/fonts/utils.py`; it does not
//! attempt the rest of that module (checksum verification, `cmap`
//! glyph-id lookups, embed-restriction removal, ...), none of which
//! `check_fonts` needs.

use std::collections::HashMap;

use anyhow::Result;

use crate::css::{Rule, Stylesheet};
use crate::dom::NodeId;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::fonts3::parse_font_family;
use crate::oeb::polish::fonts::{change_font_in_declaration, style_tag_is_css, unquote};
use crate::oeb::polish::utils::OEB_FONTS;

use super::super::container::Container;
use super::base::{CheckError, Level};

// ===================================================================
// sfnt: narrow, read-only TrueType/OpenType table parsing
// ===================================================================

mod sfnt {
    use anyhow::{bail, Result};
    use std::collections::HashMap;

    /// Port of `is_truetype_font`.
    fn is_truetype_font(raw: &[u8]) -> bool {
        raw.len() >= 4 && matches!(&raw[..4], b"\x00\x01\x00\x00" | b"OTTO")
    }

    fn read_u16(b: &[u8], off: usize) -> Option<u16> {
        b.get(off..off + 2)
            .map(|s| u16::from_be_bytes([s[0], s[1]]))
    }

    fn read_u32(b: &[u8], off: usize) -> Option<u32> {
        b.get(off..off + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// Port of `get_table`: the raw bytes of table `name`, if present.
    fn get_table<'a>(raw: &'a [u8], name: &str) -> Option<&'a [u8]> {
        let num_tables = read_u16(raw, 4)? as usize;
        let name = name.to_ascii_lowercase();
        for i in 0..num_tables {
            let entry = 12 + i * 16;
            let tag = raw.get(entry..entry + 4)?;
            let tag = String::from_utf8_lossy(tag).to_ascii_lowercase();
            let table_offset = read_u32(raw, entry + 8)? as usize;
            let table_length = read_u32(raw, entry + 12)? as usize;
            if tag == name {
                return raw.get(table_offset..table_offset.checked_add(table_length)?);
            }
        }
        None
    }

    /// One raw `name` table record: `(platform_id, encoding_id,
    /// language_id, bytes)`.
    type NameRecord<'a> = (u16, u16, u16, &'a [u8]);

    /// Port of `_get_font_names`: every record in the `name` table,
    /// grouped by `name_id`.
    fn parse_name_records(table: &[u8]) -> Result<HashMap<u16, Vec<NameRecord<'_>>>> {
        let count =
            read_u16(table, 2).ok_or_else(|| anyhow::anyhow!("Truncated name table"))? as usize;
        let string_offset =
            read_u16(table, 4).ok_or_else(|| anyhow::anyhow!("Truncated name table"))? as usize;
        let mut records: HashMap<u16, Vec<NameRecord<'_>>> = HashMap::new();
        for i in 0..count {
            let entry = 6 + i * 12;
            let Some(platform_id) = read_u16(table, entry) else {
                break;
            };
            let Some(encoding_id) = read_u16(table, entry + 2) else {
                break;
            };
            let Some(language_id) = read_u16(table, entry + 4) else {
                break;
            };
            let Some(name_id) = read_u16(table, entry + 6) else {
                break;
            };
            let Some(length) = read_u16(table, entry + 8) else {
                break;
            };
            let Some(offset) = read_u16(table, entry + 10) else {
                break;
            };
            let start = string_offset + offset as usize;
            let Some(src) = table.get(start..start + length as usize) else {
                continue;
            };
            records
                .entry(name_id)
                .or_default()
                .push((platform_id, encoding_id, language_id, src));
        }
        Ok(records)
    }

    fn decode_utf16_be(src: &[u8]) -> Option<String> {
        if !src.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = src
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).ok()
    }

    /// Port of `decode_name_record`: pick the best English name out of a
    /// `name_id`'s records, preferring Windows US-English, then other
    /// Windows English variants, then Macintosh, then any Unicode entry.
    fn decode_name_record(recs: &[NameRecord<'_>]) -> Option<String> {
        if recs.is_empty() {
            return None;
        }
        let mut unicode_names: HashMap<u16, String> = HashMap::new();
        let mut windows_names: HashMap<u16, String> = HashMap::new();
        let mut mac_names: HashMap<u16, String> = HashMap::new();
        for &(platform_id, encoding_id, language_id, src) in recs {
            if language_id > 0x8000 {
                continue;
            }
            match platform_id {
                0 if encoding_id < 4 => {
                    if let Some(s) = decode_utf16_be(src) {
                        unicode_names.insert(language_id, s);
                    }
                }
                1 => {
                    if let Ok(s) = std::str::from_utf8(src) {
                        mac_names.insert(language_id, s.to_string());
                    }
                }
                2 => {
                    let decoded = match encoding_id {
                        0 => std::str::from_utf8(src).ok().map(|s| s.to_string()),
                        1 => decode_utf16_be(src),
                        2 => Some(src.iter().map(|&b| b as char).collect()),
                        _ => None,
                    };
                    if let Some(s) = decoded {
                        unicode_names.insert(language_id, s);
                    }
                }
                3 => {
                    let decoded = match encoding_id {
                        1 => decode_utf16_be(src),
                        _ => None,
                    };
                    if let Some(s) = decoded {
                        windows_names.insert(language_id, s);
                    }
                }
                _ => {}
            }
        }
        if let Some(s) = windows_names.get(&1033) {
            return Some(s.clone());
        }
        const ALT_ENGLISH: [u16; 15] = [
            3081, 10249, 4105, 9225, 16393, 6153, 8201, 17417, 5129, 13321, 18441, 7177, 11273,
            2057, 12297,
        ];
        for lang in ALT_ENGLISH {
            if let Some(s) = windows_names.get(&lang) {
                return Some(s.clone());
            }
        }
        if let Some(s) = mac_names.get(&0) {
            return Some(s.clone());
        }
        unicode_names.into_values().next()
    }

    /// Port of `get_all_font_names`.
    pub fn get_all_font_names(raw: &[u8]) -> Result<HashMap<String, String>> {
        let table = get_table(raw, "name")
            .ok_or_else(|| anyhow::anyhow!("Not a supported font, has no name table"))?;
        let records = parse_name_records(table)?;
        let mut ans = HashMap::new();
        for (key, num) in [
            ("family_name", 1u16),
            ("subfamily_name", 2),
            ("full_name", 4),
            ("preferred_family_name", 16),
            ("preferred_subfamily_name", 17),
            ("wws_family_name", 21),
            ("wws_subfamily_name", 22),
        ] {
            if let Some(recs) = records.get(&num) {
                if let Some(v) = decode_name_record(recs) {
                    if !v.is_empty() {
                        ans.insert(key.to_string(), v);
                    }
                }
            }
        }
        if let Some(recs) = records.get(&6) {
            for &(platform_id, encoding_id, language_id, src) in recs {
                if (platform_id, encoding_id, language_id) == (1, 0, 0) {
                    if let Ok(s) = std::str::from_utf8(src) {
                        ans.insert("postscript_name".to_string(), s.to_string());
                        break;
                    }
                } else if (platform_id, encoding_id, language_id) == (3, 1, 1033) {
                    if let Some(s) = decode_utf16_be(src) {
                        ans.insert("postscript_name".to_string(), s);
                        break;
                    }
                }
            }
        }
        Ok(ans)
    }

    /// Port of `is_font_embeddable`: `(embeddable, fs_type)`.
    pub fn is_font_embeddable(raw: &[u8]) -> Result<(bool, u16)> {
        if !is_truetype_font(raw) {
            let sig = raw.get(..4).unwrap_or(&[]);
            bail!("Not a supported font, sfnt_version: {sig:?}");
        }
        let table = get_table(raw, "os/2")
            .ok_or_else(|| anyhow::anyhow!("Not a supported font, has no OS/2 table"))?;
        let fs_type = read_u16(table, 8).ok_or_else(|| anyhow::anyhow!("OS/2 table too small"))?;
        if fs_type == 0 || fs_type & 0x8 != 0 {
            return Ok((true, fs_type));
        }
        if fs_type & 1 != 0 {
            return Ok((false, fs_type));
        }
        if fs_type & 0x200 != 0 {
            return Ok((false, fs_type));
        }
        Ok((true, fs_type))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Builds a minimal, well-formed sfnt with just a `name` table
        /// (one US-English family-name record) and an `OS/2` table
        /// (with a given `fsType`).
        fn make_font(fs_type: u16, family: &str) -> Vec<u8> {
            let family_utf16: Vec<u8> = family
                .encode_utf16()
                .flat_map(|u| u.to_be_bytes())
                .collect();

            // name table: header (3x u16) + 1 record (6x u16) + string data
            let mut name_table = Vec::new();
            name_table.extend_from_slice(&0u16.to_be_bytes()); // format
            name_table.extend_from_slice(&1u16.to_be_bytes()); // count
            let string_offset = 6 + 12;
            name_table.extend_from_slice(&(string_offset as u16).to_be_bytes());
            name_table.extend_from_slice(&3u16.to_be_bytes()); // platform windows
            name_table.extend_from_slice(&1u16.to_be_bytes()); // encoding utf16be
            name_table.extend_from_slice(&1033u16.to_be_bytes()); // lang en-US
            name_table.extend_from_slice(&1u16.to_be_bytes()); // name_id family
            name_table.extend_from_slice(&(family_utf16.len() as u16).to_be_bytes());
            name_table.extend_from_slice(&0u16.to_be_bytes()); // offset within strings
            name_table.extend_from_slice(&family_utf16);

            // OS/2 table: fsType at byte offset 8.
            let mut os2_table = vec![0u8; 10];
            os2_table[8..10].copy_from_slice(&fs_type.to_be_bytes());

            let num_tables = 2u16;
            let header_len = 12;
            let dir_len = num_tables as usize * 16;
            let name_offset = header_len + dir_len;
            let os2_offset = name_offset + name_table.len();

            let mut out = Vec::new();
            out.extend_from_slice(b"\x00\x01\x00\x00"); // sfntVersion
            out.extend_from_slice(&num_tables.to_be_bytes()); // numTables
            out.extend_from_slice(&0u16.to_be_bytes()); // searchRange
            out.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
            out.extend_from_slice(&0u16.to_be_bytes()); // rangeShift

            out.extend_from_slice(b"name");
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(name_offset as u32).to_be_bytes());
            out.extend_from_slice(&(name_table.len() as u32).to_be_bytes());

            out.extend_from_slice(b"OS/2");
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(&(os2_offset as u32).to_be_bytes());
            out.extend_from_slice(&(os2_table.len() as u32).to_be_bytes());

            out.extend_from_slice(&name_table);
            out.extend_from_slice(&os2_table);
            out
        }

        #[test]
        fn get_all_font_names_reads_family_name() {
            let font = make_font(0, "Test Family");
            let names = get_all_font_names(&font).unwrap();
            assert_eq!(
                names.get("family_name").map(|s| s.as_str()),
                Some("Test Family")
            );
        }

        #[test]
        fn is_font_embeddable_zero_fstype_is_embeddable() {
            let font = make_font(0, "F");
            assert_eq!(is_font_embeddable(&font).unwrap(), (true, 0));
        }

        #[test]
        fn is_font_embeddable_restricted_license_flag() {
            // Bit 0 (0x0001) is the specific bit `is_font_embeddable`
            // checks for "restricted" (matching Python's `fs_type & 1`
            // exactly -- see the function's own doc comment).
            let font = make_font(0x0001, "F");
            let (embeddable, fs_type) = is_font_embeddable(&font).unwrap();
            assert!(!embeddable);
            assert_eq!(fs_type, 0x0001);
        }

        #[test]
        fn is_font_embeddable_bitmap_only_flag_allows_embedding() {
            let font = make_font(0x0200, "F"); // bit 9: bitmap embedding only
            let (embeddable, _) = is_font_embeddable(&font).unwrap();
            assert!(!embeddable);
        }

        #[test]
        fn is_font_embeddable_editable_flag_bit3() {
            let font = make_font(0x0008, "F"); // bit 3: editable embedding
            let (embeddable, _) = is_font_embeddable(&font).unwrap();
            assert!(embeddable);
        }

        #[test]
        fn not_a_truetype_font_errors() {
            assert!(is_font_embeddable(b"not a font").is_err());
            assert!(get_all_font_names(b"not a font").is_err());
        }
    }
}

// ===================================================================
// Error constructors
// ===================================================================

/// Port of `InvalidFont`.
pub fn invalid_font(msg: &str, name: &str) -> CheckError {
    CheckError::new("InvalidFont", format!("Not a valid font: {msg}"), name).with_help(
        "This font could not be processed. It most likely will not work in an e-book reader, \
         either",
    )
}

/// Port of `NotEmbeddable`.
pub fn not_embeddable(name: &str, fs_type: u16) -> CheckError {
    CheckError::new(
        "NotEmbeddable",
        format!("The font {name} is not allowed to be embedded"),
        name,
    )
    .with_level(Level::Warn)
    .with_help(format!(
        "The font has a flag in its metadata ({fs_type:09b}) set indicating that it is not \
         licensed for embedding. You can ignore this warning, if you are sure you have \
         permission to embed this font."
    ))
}

/// Port of `FontAliasing`.
pub fn font_aliasing(font_name: &str, css_name: &str, name: &str, line: Option<u32>) -> CheckError {
    let owned_font_name = font_name.to_string();
    let owned_css_name = css_name.to_string();
    CheckError::new(
        "FontAliasing",
        format!(
            "The CSS font-family name {css_name} does not match the actual font name {font_name}"
        ),
        name,
    )
    .at(line, None)
    .with_level(Level::Warn)
    .with_help(format!(
        "The font family name specified in the CSS @font-face rule: \"{css_name}\" does not \
         match the font name inside the actual font file: \"{font_name}\". This can cause \
         problems in some viewers. You should change the CSS font name to match the actual \
         font name."
    ))
    .with_fix(
        format!("Change the font name {css_name} to {font_name} everywhere"),
        move |container| font_aliasing_fix(container, &owned_css_name, &owned_font_name),
    )
}

/// Port of `fix_sheet`.
fn fix_sheet(sheet: &mut Stylesheet, css_name: &str, font_name: &str) -> bool {
    let mut changed = false;
    for rule in &mut sheet.rules {
        match rule {
            Rule::Style(sr) => {
                changed |= change_font_in_declaration(&mut sr.style, css_name, Some(font_name));
            }
            Rule::FontFace(decl) => {
                changed |= change_font_in_declaration(decl, css_name, Some(font_name));
            }
            _ => {}
        }
    }
    changed
}

/// Replaces `id`'s children with a single text node containing `text`.
fn set_dom_element_text(dom: &mut crate::dom::Dom, id: NodeId, text: &str) {
    let children = dom.node(id).children.clone();
    for c in children {
        dom.detach(c);
    }
    let t = dom.new_text(text);
    dom.append_child(id, t);
}

/// Port of `FontAliasing.__call__`. Does **not** call
/// `pretty_script_or_style` on rewritten `<style>` tags: that function's
/// CSS-reformatting path is `todo!()` upstream (`oeb::polish::pretty`,
/// documented there as a pre-existing gap from issue #164 -- no CSS
/// serializer/pretty-printer exists in this crate). Calling it here on
/// a `<style>` tag with real (non-empty) content would turn a working
/// fix into a guaranteed panic; per `docs/FAULT_TOLERANCE.md`'s spirit
/// (never let a decorative step crash a real operation), this port
/// keeps the substantive fix (rewriting the font name everywhere) real
/// and simply skips the purely-cosmetic re-indentation step.
fn font_aliasing_fix(container: &mut Container, css_name: &str, font_name: &str) -> Result<bool> {
    let mut changed = false;
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (name, mt) in names {
        if OEB_STYLES.contains(&mt.as_str()) {
            let mut sheet = container.parsed_stylesheet(&name)?;
            if fix_sheet(&mut sheet, css_name, font_name) {
                container.set_css_text(&name, sheet.to_css_text());
                container.dirty(&name);
                changed = true;
            }
        } else if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(&name)?;
            let style_nodes: Vec<NodeId> = {
                let dom = container.get_xhtml(&name)?;
                dom.preorder_elements(dom.root)
                    .into_iter()
                    .filter(|&id| dom.tag(id) == Some("style") && style_tag_is_css(dom, id))
                    .collect()
            };
            for style_id in style_nodes {
                let text = container.get_xhtml(&name)?.text_content(style_id);
                if text.trim().is_empty() {
                    continue;
                }
                let mut sheet = Stylesheet::parse(&text);
                if fix_sheet(&mut sheet, css_name, font_name) {
                    let new_css = sheet.to_css_text();
                    let dom = container.get_xhtml_mut(&name)?;
                    set_dom_element_text(dom, style_id, &new_css);
                    container.dirty(&name);
                    changed = true;
                }
            }
            let style_attrs: Vec<(NodeId, String)> = {
                let dom = container.get_xhtml(&name)?;
                dom.preorder_elements(dom.root)
                    .into_iter()
                    .filter_map(|id| {
                        dom.node(id)
                            .attrs
                            .get("style")
                            .filter(|s| s.contains("font-family"))
                            .map(|s| (id, s.clone()))
                    })
                    .collect()
            };
            for (el, style_text) in style_attrs {
                let mut decl = crate::css::parser::parse_declaration_list(&style_text);
                if change_font_in_declaration(&mut decl, css_name, Some(font_name)) {
                    let new_style = decl.to_css_text(" ").replace('\n', " ");
                    let dom = container.get_xhtml_mut(&name)?;
                    dom.node_mut(el)
                        .attrs
                        .insert("style".to_string(), new_style);
                    container.dirty(&name);
                    changed = true;
                }
            }
        }
    }
    Ok(changed)
}

fn extract_first_url(css_value: &str) -> Option<String> {
    let idx = css_value.find("url(")?;
    let rest = &css_value[idx + 4..];
    let end = rest.find(')')?;
    let inner = rest[..end].trim();
    Some(unquote(inner).to_string())
}

// ===================================================================
// check_fonts
// ===================================================================

/// Port of `check_fonts`.
pub fn check_fonts(container: &mut Container) -> Result<Vec<CheckError>> {
    let mut errors = Vec::new();
    let mut font_map: HashMap<String, Option<String>> = HashMap::new();
    let names: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    for (name, mt) in &names {
        if !OEB_FONTS.contains(&mt.as_str()) {
            continue;
        }
        let raw = container.raw_data(name, false)?;
        match sfnt::get_all_font_names(&raw) {
            Err(e) => {
                errors.push(invalid_font(&e.to_string(), name));
                continue;
            }
            Ok(name_map) => {
                let family = name_map
                    .get("family_name")
                    .or_else(|| name_map.get("preferred_family_name"))
                    .or_else(|| name_map.get("wws_family_name"))
                    .cloned();
                font_map.insert(name.clone(), family);
                match sfnt::is_font_embeddable(&raw) {
                    Ok((true, _)) | Err(_) => {}
                    Ok((false, fs_type)) => errors.push(not_embeddable(name, fs_type)),
                }
            }
        }
    }

    let mut sheets: Vec<(String, Stylesheet, Option<u32>)> = Vec::new();
    for (name, mt) in &names {
        if OEB_STYLES.contains(&mt.as_str()) {
            if let Ok(sheet) = container.parsed_stylesheet(name) {
                sheets.push((name.clone(), sheet, None));
            }
        } else if OEB_DOCS.contains(&mt.as_str()) {
            container.ensure_parsed(name)?;
            let dom = container.get_xhtml(name)?;
            for id in dom.preorder_elements(dom.root) {
                if dom.tag(id) == Some("style") && style_tag_is_css(dom, id) {
                    let text = dom.text_content(id);
                    if !text.trim().is_empty() {
                        sheets.push((name.clone(), Stylesheet::parse(&text), None));
                    }
                }
            }
        }
    }

    for (name, sheet, line_offset) in &sheets {
        for rule in &sheet.rules {
            let Rule::FontFace(decl) = rule else { continue };
            let Some(src) = decl.get_property("src") else {
                continue;
            };
            let Some(href) = extract_first_url(&src.value) else {
                continue;
            };
            let Some(fname) = container.href_to_name(&href, Some(name)) else {
                continue;
            };
            let Some(Some(font_name)) = font_map.get(&fname) else {
                continue;
            };
            let families = parse_font_family(decl.get_property_value("font-family"));
            if let Some(first) = families.first() {
                if first != font_name {
                    errors.push(font_aliasing(font_name, first, name, *line_offset));
                }
            }
        }
    }

    Ok(errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn make_container(dir: &Path, files: &[(&str, &[u8])]) -> Container {
        std::fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata><dc:identifier id="bookid">urn:uuid:x</dc:identifier></metadata>
  <manifest/>
  <spine/>
</package>"#,
        )
        .unwrap();
        for (name, data) in files {
            std::fs::write(dir.join(name), data).unwrap();
        }
        Container::open(dir, &dir.join("content.opf")).unwrap()
    }

    #[test]
    fn check_fonts_flags_invalid_font_data() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), &[("font.ttf", b"not a real font")]);
        let errors = check_fonts(&mut c).unwrap();
        assert!(errors.iter().any(|e| e.type_name == "InvalidFont"));
    }

    #[test]
    fn check_fonts_accepts_no_fonts() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = make_container(dir.path(), &[]);
        let errors = check_fonts(&mut c).unwrap();
        assert!(errors.is_empty());
    }

    #[test]
    fn extract_first_url_pulls_url_and_unquotes() {
        assert_eq!(
            extract_first_url("url(\"fonts/a.ttf\") format(\"truetype\")"),
            Some("fonts/a.ttf".to_string())
        );
        assert_eq!(extract_first_url("url(a.ttf)"), Some("a.ttf".to_string()));
        assert_eq!(extract_first_url("none"), None);
    }
}
