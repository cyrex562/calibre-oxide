//! Footnotes and endnotes.
//!
//! Port of `old_src/src/calibre/ebooks/docx/footnotes.py`.
//!
//! `footnotes.xml` and `endnotes.xml` each hold a flat list of notes
//! keyed by id. The document body refers to them with
//! `<w:footnoteReference w:id="2"/>`. Notes are numbered in the order
//! the body references them — not the order they appear in the part —
//! so [`Footnotes::get_ref`] both assigns the number and records the
//! note for later emission.
//!
//! Word reserves ids for separator notes (`w:type` of `separator` or
//! `continuationSeparator`); those carry the horizontal rule above the
//! note area rather than any content, and are skipped.

use std::rc::Rc;

use indexmap::IndexMap;
use roxmltree::Node;

use super::container::Relationships;
use super::names::DocxNamespace;

/// One footnote or endnote.
///
/// Port of the Python `Note` class.
#[derive(Debug, Clone)]
pub struct Note<'a, 'i> {
    /// The `w:type`, defaulting to `normal`.
    pub kind: String,
    /// The `w:footnote`/`w:endnote` element itself.
    pub parent: Node<'a, 'i>,
    /// The relationships of the part the note came from — footnotes
    /// live in their own part, so images inside one resolve against
    /// `footnotes.xml.rels` rather than the document's.
    pub rels: Rc<Relationships>,
}

impl<'a, 'i> Note<'a, 'i> {
    /// The note's block-level content, in document order.
    ///
    /// Port of the Python `Note.__iter__`.
    pub fn blocks(&self, ns: &DocxNamespace) -> Vec<Node<'a, 'i>> {
        ns.descendants(self.parent, &["w:p", "w:tbl"])
    }

    /// Whether this note carries content rather than being one of
    /// Word's separator notes.
    pub fn is_normal(&self) -> bool {
        self.kind == "normal"
    }
}

/// The notes of a document, and the numbering assigned as the body
/// references them.
///
/// Port of the Python `Footnotes` class.
#[derive(Debug, Default)]
pub struct Footnotes<'a, 'i> {
    footnotes: IndexMap<String, Note<'a, 'i>>,
    endnotes: IndexMap<String, Note<'a, 'i>>,
    counter: usize,
    /// anchor → (displayed number, note), in reference order.
    notes: IndexMap<String, (String, Note<'a, 'i>)>,
}

impl<'a, 'i> Footnotes<'a, 'i> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load the `footnotes.xml` and `endnotes.xml` roots, each with the
    /// relationships of its own part.
    ///
    /// Port of the Python `Footnotes.__call__`.
    pub fn load(
        &mut self,
        footnotes: Option<Node<'a, 'i>>,
        footnotes_rels: Rc<Relationships>,
        endnotes: Option<Node<'a, 'i>>,
        endnotes_rels: Rc<Relationships>,
        ns: &DocxNamespace,
    ) {
        if let Some(root) = footnotes {
            for note in ns.children(root, &["w:footnote"]) {
                if let Some(id) = ns.get(note, "w:id").filter(|v| !v.is_empty()) {
                    self.footnotes
                        .insert(id.to_string(), make_note(note, &footnotes_rels, ns));
                }
            }
        }
        if let Some(root) = endnotes {
            for note in ns.children(root, &["w:endnote"]) {
                if let Some(id) = ns.get(note, "w:id").filter(|v| !v.is_empty()) {
                    self.endnotes
                        .insert(id.to_string(), make_note(note, &endnotes_rels, ns));
                }
            }
        }
    }

    /// Resolve a `w:footnoteReference`/`w:endnoteReference` from the
    /// body, assigning it the next number.
    ///
    /// Returns the anchor and the displayed number, or `None` for a
    /// reference to an unknown or separator note.
    ///
    /// Port of the Python `get_ref`.
    pub fn get_ref(
        &mut self,
        reference: Node<'a, 'i>,
        ns: &DocxNamespace,
    ) -> Option<(String, String)> {
        let fid = ns.get(reference, "w:id")?;
        let is_footnote = reference.tag_name().name() == "footnoteReference";
        let note = if is_footnote {
            self.footnotes.get(fid)?
        } else {
            self.endnotes.get(fid)?
        };
        if !note.is_normal() {
            return None;
        }
        let note = note.clone();
        self.counter += 1;
        let anchor = format!("note_{}", self.counter);
        let number = self.counter.to_string();
        self.notes.insert(anchor.clone(), (number.clone(), note));
        Some((anchor, number))
    }

    /// The referenced notes, in reference order, as
    /// `(anchor, number, note)`.
    ///
    /// Port of the Python `Footnotes.__iter__`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, &Note<'a, 'i>)> {
        self.notes
            .iter()
            .map(|(anchor, (number, note))| (anchor.as_str(), number.as_str(), note))
    }

    /// Whether the body referenced any note at all.
    ///
    /// Port of the Python `has_notes`.
    pub fn has_notes(&self) -> bool {
        !self.notes.is_empty()
    }

    /// The notes available to be referenced, by id.
    pub fn footnotes(&self) -> &IndexMap<String, Note<'a, 'i>> {
        &self.footnotes
    }

    pub fn endnotes(&self) -> &IndexMap<String, Note<'a, 'i>> {
        &self.endnotes
    }
}

fn make_note<'a, 'i>(
    parent: Node<'a, 'i>,
    rels: &Rc<Relationships>,
    ns: &DocxNamespace,
) -> Note<'a, 'i> {
    Note {
        kind: ns.get_or(parent, "w:type", "normal").to_string(),
        parent,
        rels: Rc::clone(rels),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn footnotes_xml() -> String {
        format!(
            r#"<w:footnotes {W}>
                 <w:footnote w:id="-1" w:type="separator"><w:p><w:r><w:t>sep</w:t></w:r></w:p></w:footnote>
                 <w:footnote w:id="1"><w:p><w:r><w:t>First note</w:t></w:r></w:p></w:footnote>
                 <w:footnote w:id="2"><w:p><w:r><w:t>Second note</w:t></w:r></w:p><w:tbl/></w:footnote>
               </w:footnotes>"#
        )
    }

    fn endnotes_xml() -> String {
        format!(
            r#"<w:endnotes {W}>
                 <w:endnote w:id="5"><w:p><w:r><w:t>An endnote</w:t></w:r></w:p></w:endnote>
               </w:endnotes>"#
        )
    }

    fn body_xml() -> String {
        format!(
            r#"<w:body {W}>
                 <w:p><w:r><w:footnoteReference w:id="2"/></w:r></w:p>
                 <w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>
                 <w:p><w:r><w:endnoteReference w:id="5"/></w:r></w:p>
                 <w:p><w:r><w:footnoteReference w:id="-1"/></w:r></w:p>
                 <w:p><w:r><w:footnoteReference w:id="99"/></w:r></w:p>
               </w:body>"#
        )
    }

    #[test]
    fn notes_are_numbered_in_reference_order_not_document_order() {
        let fx = footnotes_xml();
        let ex = endnotes_xml();
        let bx = body_xml();
        let (fdoc, edoc, bdoc) = (
            Document::parse(&fx).unwrap(),
            Document::parse(&ex).unwrap(),
            Document::parse(&bx).unwrap(),
        );
        let ns = DocxNamespace::default();
        let rels = Rc::new(Relationships::default());

        let mut notes = Footnotes::new();
        notes.load(
            Some(fdoc.root_element()),
            Rc::clone(&rels),
            Some(edoc.root_element()),
            Rc::clone(&rels),
            &ns,
        );
        assert_eq!(
            notes.footnotes().len(),
            3,
            "separator notes are still loaded"
        );
        assert_eq!(notes.endnotes().len(), 1);
        assert!(!notes.has_notes(), "nothing referenced yet");

        let refs: Vec<Node> = bdoc
            .root_element()
            .descendants()
            .filter(|n| {
                matches!(
                    n.tag_name().name(),
                    "footnoteReference" | "endnoteReference"
                )
            })
            .collect();
        let resolved: Vec<Option<(String, String)>> =
            refs.iter().map(|r| notes.get_ref(*r, &ns)).collect();

        // The body references footnote 2 first, so it is note 1.
        assert_eq!(resolved[0], Some(("note_1".to_string(), "1".to_string())));
        assert_eq!(resolved[1], Some(("note_2".to_string(), "2".to_string())));
        // Endnotes share the numbering sequence with footnotes.
        assert_eq!(resolved[2], Some(("note_3".to_string(), "3".to_string())));
        // A separator note is not content and gets no number.
        assert_eq!(resolved[3], None);
        // Neither is a dangling reference.
        assert_eq!(resolved[4], None);

        assert!(notes.has_notes());
        let collected: Vec<(&str, &str)> = notes.iter().map(|(a, n, _)| (a, n)).collect();
        assert_eq!(
            collected,
            vec![("note_1", "1"), ("note_2", "2"), ("note_3", "3")]
        );

        // The first-referenced note is footnote id 2, whose text is
        // "Second note" — the ordering really is by reference.
        let (_, _, first) = notes.iter().next().unwrap();
        let text: String = first
            .parent
            .descendants()
            .filter(|n| n.is_text())
            .filter_map(|n| n.text())
            .collect();
        assert_eq!(text, "Second note");
        assert_eq!(first.blocks(&ns).len(), 2, "one w:p and one w:tbl");
    }

    #[test]
    fn a_document_with_no_note_parts_has_no_notes() {
        let ns = DocxNamespace::default();
        let mut notes = Footnotes::new();
        let rels = Rc::new(Relationships::default());
        notes.load(None, Rc::clone(&rels), None, rels, &ns);
        assert!(!notes.has_notes());
        assert_eq!(notes.iter().count(), 0);
    }

    #[test]
    fn each_note_carries_its_own_parts_relationships() {
        // A footnote's images resolve against footnotes.xml.rels, so
        // the two parts must not share one map.
        let fx = footnotes_xml();
        let ex = endnotes_xml();
        let (fdoc, edoc) = (Document::parse(&fx).unwrap(), Document::parse(&ex).unwrap());
        let ns = DocxNamespace::default();
        let mut frels = Relationships::default();
        frels
            .by_id
            .insert("rId1".to_string(), "word/media/foot.png".to_string());
        let mut erels = Relationships::default();
        erels
            .by_id
            .insert("rId1".to_string(), "word/media/end.png".to_string());

        let mut notes = Footnotes::new();
        notes.load(
            Some(fdoc.root_element()),
            Rc::new(frels),
            Some(edoc.root_element()),
            Rc::new(erels),
            &ns,
        );
        assert_eq!(
            notes.footnotes()["1"].rels.target("rId1"),
            Some("word/media/foot.png")
        );
        assert_eq!(
            notes.endnotes()["5"].rels.target("rId1"),
            Some("word/media/end.png")
        );
    }
}
