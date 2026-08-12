//! The font table and embedded-font obfuscation.
//!
//! Port of `old_src/src/calibre/ebooks/docx/writer/fonts.py`.
//!
//! Fonts embedded in a DOCX are "obfuscated": the first 32 bytes are
//! XORed with the 16 bytes of a GUID written into the markup, which is
//! the whole of the protection. See ECMA-376 Part 1, §15.2.13. The
//! algorithm is its own inverse, so [`obfuscate_font_data`] both
//! obfuscates and recovers.
//!
//! One deviation: the Python discovers embedded fonts by scanning the
//! book's stylesheets for `@font-face` rules
//! (`oeb.transforms.subset.find_font_face_rules`). That scan belongs to
//! the OEB pipeline, so this port takes the already-discovered faces as
//! [`FontFace`] values, leaving the caller to produce them.

use super::xml::Element;
use crate::docx::names::DocxNamespace;

/// A `@font-face` rule that the book embeds.
///
/// Port of the dicts `find_font_face_rules` yields, reduced to the
/// fields the writer reads.
#[derive(Debug, Clone, PartialEq)]
pub struct FontFace {
    /// The first family named by the rule.
    pub family: String,
    /// CSS `font-weight`, numeric.
    pub weight: u32,
    /// CSS `font-style`.
    pub style: String,
    /// The font file's bytes.
    pub data: Vec<u8>,
    /// A key identifying the source file, so two faces from one file
    /// share a relationship.
    pub source: String,
}

impl FontFace {
    /// Which of Word's four slots this face fills.
    ///
    /// Port of the Python's `tag` calculation.
    pub fn slot(&self) -> Slot {
        let bold = self.weight > 400;
        let italic = self.style != "normal";
        match (bold, italic) {
            (true, true) => Slot::BoldItalic,
            (true, false) => Slot::Bold,
            (false, true) => Slot::Italic,
            (false, false) => Slot::Regular,
        }
    }
}

/// The four font slots Word recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    Regular,
    Bold,
    Italic,
    BoldItalic,
}

impl Slot {
    /// The element name Word expects: `w:embedRegular` and friends.
    pub fn element_name(self) -> &'static str {
        match self {
            Slot::Regular => "w:embedRegular",
            Slot::Bold => "w:embedBold",
            Slot::Italic => "w:embedItalic",
            Slot::BoldItalic => "w:embedBoldItalic",
        }
    }
}

/// XOR the first 32 bytes of a font with the reversed bytes of `key`.
///
/// Port of the Python `obfuscate_font_data`. `key` is the raw 16 bytes
/// of the GUID that goes into `w:fontKey`. Fonts shorter than 32 bytes
/// are handled by obfuscating what is there — the Python would too,
/// since it slices.
///
/// The operation is an involution: applying it twice with the same key
/// returns the original bytes.
pub fn obfuscate_font_data(data: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let mut out = data.to_vec();
    let reversed: Vec<u8> = key.iter().rev().copied().collect();
    for (i, byte) in out.iter_mut().take(32).enumerate() {
        *byte ^= reversed[i % reversed.len()];
    }
    out
}

/// Format a GUID as `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, which is
/// how `w:fontKey` spells it.
pub fn format_font_key(key: &[u8; 16]) -> String {
    let h = |r: std::ops::Range<usize>| -> String {
        key[r].iter().map(|b| format!("{b:02X}")).collect()
    };
    format!(
        "{{{}-{}-{}-{}-{}}}",
        h(0..4),
        h(4..6),
        h(6..8),
        h(8..10),
        h(10..16)
    )
}

/// Builds `word/fontTable.xml` and its relationships.
///
/// Port of the Python `FontsManager`.
#[derive(Debug, Default)]
pub struct FontsManager {
    /// Whether embedded fonts were subsetted, written into
    /// `w:subsetted`.
    pub subset_embedded_fonts: bool,
}

/// What [`FontsManager::serialize`] produced.
#[derive(Debug, Default, PartialEq)]
pub struct SerializedFonts {
    /// Part name → obfuscated font bytes, to add to the package.
    pub font_data: Vec<(String, Vec<u8>)>,
}

impl FontsManager {
    pub fn new(subset_embedded_fonts: bool) -> Self {
        Self {
            subset_embedded_fonts,
        }
    }

    /// Fill in `fonts` (the `w:fonts` root) and `embed_relationships`
    /// (`word/_rels/fontTable.xml.rels`).
    ///
    /// `families` are the families actually used by the document's text
    /// styles; a face for any other family is skipped. `keys` supplies
    /// one GUID per embedded face, in order — passed in rather than
    /// generated so output is reproducible.
    ///
    /// Port of the Python `FontsManager.serialize`.
    pub fn serialize(
        &self,
        families: &[String],
        faces: &[FontFace],
        fonts: &mut Element,
        embed_relationships: &mut Element,
        keys: &mut dyn Iterator<Item = [u8; 16]>,
        ns: &DocxNamespace,
    ) -> SerializedFonts {
        // Families are deduplicated case-insensitively but written with
        // the casing of their first appearance, then sorted.
        let mut seen: Vec<String> = Vec::new();
        let mut unique: Vec<String> = Vec::new();
        for family in families.iter().filter(|f| !f.is_empty()) {
            let lower = family.to_lowercase();
            if !seen.contains(&lower) {
                seen.push(lower);
                unique.push(family.clone());
            }
        }
        unique.sort();

        let mut family_indices: Vec<(String, usize)> = Vec::new();
        for family in &unique {
            fonts.append(Element::new("w:font").attr("w:name", family));
            family_indices.push((family.clone(), fonts.child_count() - 1));
        }

        let mut out = SerializedFonts::default();
        let mut num = 0usize;
        // Which (family, slot) pairs are already embedded, and which
        // source files already have a relationship.
        let mut filled: Vec<(String, Slot)> = Vec::new();
        let mut rel_for_source: Vec<(String, String)> = Vec::new();

        for face in faces {
            if !unique.contains(&face.family) {
                continue;
            }
            num += 1;
            let slot = face.slot();
            if filled.contains(&(face.family.clone(), slot)) {
                continue;
            }
            filled.push((face.family.clone(), slot));

            let key = keys.next().unwrap_or([0u8; 16]);
            let rid = match rel_for_source.iter().find(|(s, _)| *s == face.source) {
                Some((_, rid)) => rid.clone(),
                None => {
                    let rid = format!("rId{num}");
                    let fname = format!("fonts/font{num}.odttf");
                    embed_relationships.append(
                        Element::new("Relationship")
                            .attr("Id", &rid)
                            .attr("Type", ns.name("EMBEDDED_FONT").unwrap_or_default())
                            .attr("Target", &fname),
                    );
                    out.font_data.push((
                        format!("word/{fname}"),
                        obfuscate_font_data(&face.data, &key),
                    ));
                    rel_for_source.push((face.source.clone(), rid.clone()));
                    rid
                }
            };

            let Some((_, index)) = family_indices.iter().find(|(f, _)| *f == face.family) else {
                continue;
            };
            if let Some(super::xml::Child::Element(font)) = fonts.children.get_mut(*index) {
                font.append(
                    Element::new(slot.element_name())
                        .attr("r:id", &rid)
                        .attr("w:fontKey", format_font_key(&key))
                        .attr(
                            "w:subsetted",
                            if self.subset_embedded_fonts {
                                "true"
                            } else {
                                "false"
                            },
                        ),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const KEY: [u8; 16] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ];

    fn face(family: &str, weight: u32, style: &str, source: &str) -> FontFace {
        FontFace {
            family: family.to_string(),
            weight,
            style: style.to_string(),
            data: (0..64u8).collect(),
            source: source.to_string(),
        }
    }

    #[test]
    fn obfuscation_is_its_own_inverse() {
        let data: Vec<u8> = (0..100u8).collect();
        let hidden = obfuscate_font_data(&data, &KEY);
        assert_ne!(hidden[..32], data[..32], "the head is scrambled");
        assert_eq!(hidden[32..], data[32..], "the tail is untouched");
        assert_eq!(obfuscate_font_data(&hidden, &KEY), data);
    }

    #[test]
    fn obfuscation_uses_the_key_reversed() {
        // ECMA-376 §15.2.13: the GUID's bytes are applied in reverse.
        let data = vec![0u8; 32];
        let hidden = obfuscate_font_data(&data, &KEY);
        let reversed: Vec<u8> = KEY.iter().rev().copied().collect();
        assert_eq!(hidden[..16], reversed[..]);
        assert_eq!(hidden[16..32], reversed[..]);
    }

    #[test]
    fn a_font_shorter_than_the_obfuscated_head_is_still_handled() {
        let data = vec![0xffu8; 8];
        let hidden = obfuscate_font_data(&data, &KEY);
        assert_eq!(hidden.len(), 8);
        assert_eq!(obfuscate_font_data(&hidden, &KEY), data);
        assert!(obfuscate_font_data(&[], &KEY).is_empty());
    }

    #[test]
    fn the_font_key_is_a_braced_guid() {
        assert_eq!(
            format_font_key(&KEY),
            "{01020304-0506-0708-090A-0B0C0D0E0F10}"
        );
    }

    #[test]
    fn weight_and_style_pick_the_slot() {
        assert_eq!(face("X", 400, "normal", "a").slot(), Slot::Regular);
        assert_eq!(face("X", 700, "normal", "a").slot(), Slot::Bold);
        assert_eq!(face("X", 400, "italic", "a").slot(), Slot::Italic);
        assert_eq!(face("X", 700, "oblique", "a").slot(), Slot::BoldItalic);
        // 400 is not bold; anything above it is.
        assert_eq!(face("X", 401, "normal", "a").slot(), Slot::Bold);
    }

    #[test]
    fn the_font_table_lists_used_families_sorted_and_deduplicated() {
        let ns = DocxNamespace::new(true);
        let mut fonts = Element::new("w:fonts");
        let mut rels = Element::new("Relationships");
        let manager = FontsManager::new(false);
        let families = vec![
            "Georgia".to_string(),
            "Candara".to_string(),
            "georgia".to_string(),
            String::new(),
        ];
        manager.serialize(
            &families,
            &[],
            &mut fonts,
            &mut rels,
            &mut std::iter::empty(),
            &ns,
        );
        let names: Vec<&str> = fonts
            .children_named("w:font")
            .filter_map(|f| f.get("w:name"))
            .collect();
        assert_eq!(names, vec!["Candara", "Georgia"]);
    }

    #[test]
    fn embedded_faces_get_a_relationship_and_a_key() {
        let ns = DocxNamespace::new(true);
        let mut fonts = Element::new("w:fonts")
            .ns("w", ns.namespace("w").unwrap())
            .ns("r", ns.namespace("r").unwrap());
        let mut rels = Element::new("Relationships");
        let manager = FontsManager::new(true);
        let faces = vec![
            face("Georgia", 400, "normal", "georgia.ttf"),
            face("Georgia", 700, "normal", "georgia-bold.ttf"),
            // A family the document never uses is skipped.
            face("Unused", 400, "normal", "unused.ttf"),
        ];
        let out = manager.serialize(
            &["Georgia".to_string()],
            &faces,
            &mut fonts,
            &mut rels,
            &mut [KEY, [0xAA; 16]].into_iter(),
            &ns,
        );

        assert_eq!(out.font_data.len(), 2);
        assert_eq!(out.font_data[0].0, "word/fonts/font1.odttf");
        assert_eq!(out.font_data[1].0, "word/fonts/font2.odttf");
        // The stored bytes really are obfuscated with the given key.
        assert_eq!(
            obfuscate_font_data(&out.font_data[0].1, &KEY),
            (0..64u8).collect::<Vec<u8>>()
        );

        let xml = fonts.to_xml();
        let doc = Document::parse(&xml).expect("parses");
        let embeds: Vec<&str> = doc
            .descendants()
            .filter(|n| n.tag_name().name().starts_with("embed"))
            .map(|n| n.tag_name().name())
            .collect();
        assert_eq!(embeds, vec!["embedRegular", "embedBold"]);
        assert!(
            xml.contains(r#"w:fontKey="{01020304-0506-0708-090A-0B0C0D0E0F10}""#),
            "{xml}"
        );
        assert!(xml.contains(r#"w:subsetted="true""#));
        assert_eq!(rels.children_named("Relationship").count(), 2);
    }

    #[test]
    fn two_faces_from_one_file_share_a_relationship() {
        let ns = DocxNamespace::new(true);
        let mut fonts = Element::new("w:fonts");
        let mut rels = Element::new("Relationships");
        let manager = FontsManager::new(false);
        // A variable font supplying both slots from a single file.
        let faces = vec![
            face("Georgia", 400, "normal", "georgia.ttf"),
            face("Georgia", 700, "normal", "georgia.ttf"),
        ];
        let out = manager.serialize(
            &["Georgia".to_string()],
            &faces,
            &mut fonts,
            &mut rels,
            &mut [KEY, [0xAA; 16]].into_iter(),
            &ns,
        );
        assert_eq!(out.font_data.len(), 1, "the file is stored once");
        assert_eq!(rels.children_named("Relationship").count(), 1);
        let xml = fonts.to_xml_fragment();
        assert_eq!(xml.matches(r#"r:id="rId1""#).count(), 2, "{xml}");
    }

    #[test]
    fn a_slot_is_only_filled_once() {
        let ns = DocxNamespace::new(true);
        let mut fonts = Element::new("w:fonts");
        let mut rels = Element::new("Relationships");
        let out = FontsManager::new(false).serialize(
            &["Georgia".to_string()],
            &[
                face("Georgia", 400, "normal", "one.ttf"),
                face("Georgia", 400, "normal", "two.ttf"),
            ],
            &mut fonts,
            &mut rels,
            &mut [KEY, [0xAA; 16]].into_iter(),
            &ns,
        );
        assert_eq!(out.font_data.len(), 1, "the second Regular is dropped");
    }
}
