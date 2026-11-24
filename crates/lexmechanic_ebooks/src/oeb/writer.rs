use crate::oeb::book::OEBBook;
use crate::oeb::constants::*;
use crate::oeb::container::{Container, DirContainer};
use crate::oeb::parse_utils::escape_xml;
use anyhow::Result;
use std::path::Path;

pub struct OEBWriter {
    pub pretty_print: bool,
}

/// Which identifier `package/@unique-identifier` names, and whether it had
/// to be invented.
struct UniqueIdentifier {
    id: String,
    value: String,
    /// True when the book carried no identifier and one was generated, in
    /// which case the element itself has to be written too.
    generated: bool,
}

impl OEBWriter {
    pub fn new() -> Self {
        Self { pretty_print: true }
    }

    pub fn write_book(&self, book: &mut OEBBook, output_path: &Path) -> Result<()> {
        let mut container = DirContainer::new(output_path);

        // 1. Write Manifest Items (Content)
        // We assume book.manifest.items has the data or we read it from book.container
        // Wait, OEBBook items don't store data directly unless loaded?
        // In the original design, items are just references.
        // But for a read-write cycle, we need to copy data from source container to destination.
        // OEBBook holds a 'container' which is the source.

        // Loop through everything in the source container and copy it?
        // Or loop through manifest items and copy them?
        // Manifest items are what matters for the book.
        for item in book.manifest.items.values() {
            // We need to read from source and write to dest
            // But checking if item has data... The current Item struct in manifest.rs doesn't store data bytes.
            // It only stores href.
            // So we read from book.container using item.href
            if let Ok(data) = book.container.read(&item.href) {
                container.write(&item.href, &data)?;
            } else {
                // Warning: item in manifest but file missing?
                eprintln!(
                    "Warning: Manifest item {} missing from source container",
                    item.href
                );
            }
        }

        // 2. Generate and Write OPF
        //
        // The identifier is settled once and shared with the NCX below, so
        // the NCX's `dtb:uid` matches the OPF's `dc:identifier`. Computing
        // it twice would generate two uuids for a book that had none.
        let identifier = Self::unique_identifier(book);
        let opf_content = self.write_opf_with(book, &identifier)?;
        container.write("content.opf", opf_content.as_bytes())?;

        // 3. Generate and write the NCX, unless the book brought its own.
        if Self::existing_ncx_id(book).is_none() {
            // Each `get` returns a temporary Vec, so the value is cloned
            // out of the first match rather than chained across both.
            let title = ["title", "dc:title"].iter().find_map(|term| book.metadata.get(term).first().map(|i| i.value.clone())).unwrap_or_default();
            let ncx = crate::opf_writer::write_ncx(&Self::to_ncx_toc(&book.toc), &identifier.value, &title);
            container.write("toc.ncx", ncx.as_bytes())?;
        }

        Ok(())
    }

/// The `dc:`-prefixed tag for a metadata term, or `None` if the term is
/// not a Dublin Core element and belongs in a `<meta name=... content=...>`.
///
/// This used to be `term.starts_with("dc:")`, which nothing satisfied:
/// every producer in this crate adds bare terms (`m.add("title", ..)` in
/// `meta_info_to_oeb_metadata`, `html_input`, and the rest). So **every
/// OPF this writer produced had no `dc:title`, `dc:creator` or
/// `dc:language`** -- only `<meta name="title" content="..."/>`, which no
/// reader looks at and which makes the EPUB invalid, since EPUB 2 requires
/// those elements. It is also why a converted book's title read back as
/// "Unknown".
/// Converts an [`crate::oeb::toc::TOC`] into the shape
/// [`crate::opf_writer::write_ncx`] takes.
///
/// The two `TOC` types are distinct: the `metadata` one was built for the
/// news/periodical writers (#622) and is what the NCX serializer accepts,
/// while `OEBBook` carries the `oeb` one. That mismatch is most of why the
/// serializer had no caller here despite an NCX being required.
///
/// Nodes with no `href` are dropped: an NCX `navPoint` must point
/// somewhere, and a title with no target is not something a reader can
/// navigate to. Their children are kept, promoted to the parent's level,
/// so a purely structural grouping node does not take its articles with it.
fn to_ncx_toc(toc: &crate::oeb::toc::TOC) -> crate::metadata::toc::TOC {
    fn convert(nodes: &[crate::oeb::toc::TOCNode], out: &mut Vec<crate::metadata::toc::TOCNode>) {
        for node in nodes {
            match node.href.as_deref().filter(|h| !h.is_empty()) {
                Some(href) => {
                    let mut children = Vec::new();
                    convert(&node.children, &mut children);
                    out.push(crate::metadata::toc::TOCNode {
                        title: node.title.clone().unwrap_or_default(),
                        src: href.to_string(),
                        children,
                        // `0` is this type's "not set" -- `write_ncx`
                        // auto-numbers in traversal order for `None`, which
                        // is what an unset order should mean.
                        play_order: if node.play_order > 0 { Some(node.play_order as u32) } else { None },
                        author: node.author.clone(),
                        description: node.description.clone(),
                        toc_thumbnail: None,
                    });
                }
                None => convert(&node.children, out),
            }
        }
    }

    let mut nodes = Vec::new();
    convert(&toc.root.children, &mut nodes);
    crate::metadata::toc::TOC { nodes }
}

/// The manifest id of an NCX the book already carries, if any.
///
/// An EPUB -> EPUB round trip brings its source's NCX along in the
/// manifest; emitting a second one would make the file invalid in a
/// different way.
fn existing_ncx_id(book: &OEBBook) -> Option<String> {
    book.manifest
        .items
        .values()
        .find(|item| item.media_type == "application/x-dtbncx+xml" || item.href.eq_ignore_ascii_case("toc.ncx"))
        .map(|item| item.id.clone())
}

fn unique_identifier(book: &OEBBook) -> UniqueIdentifier {
    for term in ["identifier", "dc:identifier"] {
        for item in book.metadata.get(term) {
            if let Some(id) = item.get_attribute("id").filter(|id| !id.is_empty()) {
                return UniqueIdentifier { id: id.to_string(), value: item.value.clone(), generated: false };
            }
        }
    }
    // A book with no identifier cannot be a valid EPUB, and inventing a
    // uuid is what every producer does in that case.
    UniqueIdentifier { id: "uuid_id".to_string(), value: format!("urn:uuid:{}", uuid::Uuid::new_v4()), generated: true }
}

fn dublin_core_tag(term: &str) -> Option<String> {
    // Already namespaced by the caller.
    if let Some(rest) = term.strip_prefix("dc:") {
        return Some(format!("dc:{rest}"));
    }
    // The Dublin Core 1.1 element set, which is exactly what OPF 2.0
    // permits inside `<metadata>`. Anything else is a `<meta>`.
    const DC_ELEMENTS: [&str; 15] = [
        "title",
        "creator",
        "contributor",
        "subject",
        "description",
        "publisher",
        "date",
        "type",
        "format",
        "identifier",
        "source",
        "language",
        "relation",
        "coverage",
        "rights",
    ];
    let lower = term.to_ascii_lowercase();
    if DC_ELEMENTS.contains(&lower.as_str()) {
        return Some(format!("dc:{lower}"));
    }
    None
}

    pub fn write_opf(&self, book: &OEBBook) -> Result<String> {
        self.write_opf_with(book, &Self::unique_identifier(book))
    }

    /// The OPF, using an identifier the caller has already settled on.
    ///
    /// Split out because [`Self::unique_identifier`] *generates* one when
    /// the book has none, so calling it twice yields two different uuids --
    /// and the NCX's `dtb:uid` has to equal the OPF's identifier. Sharing
    /// one value is what keeps them agreeing.
    fn write_opf_with(&self, book: &OEBBook, identifier: &UniqueIdentifier) -> Result<String> {
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");

        // EPUB 2 requires `dc:title`, `dc:identifier` and `dc:language`,
        // and requires `package/@unique-identifier` to name the
        // identifier's `id`. None of the three was emitted, so every book
        // this produced was invalid -- a validating reader refuses it.
        // Worked out before the `<package>` tag because the attribute goes
        // on it.
        out.push_str(&format!(
            r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="{}">"#,
            escape_xml(&identifier.id)
        ));
        out.push('\n');

        // Metadata
        out.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");

        // Written when the book carries none of its own, so the required
        // elements are present even for an input that had no metadata at
        // all -- a bare HTML file, say.
        if identifier.generated {
            out.push_str(&format!(
                "    <dc:identifier id=\"{}\" opf:scheme=\"uuid\">{}</dc:identifier>\n",
                escape_xml(&identifier.id),
                escape_xml(&identifier.value)
            ));
        }
        if book.metadata.get("language").is_empty() && book.metadata.get("dc:language").is_empty() {
            // `und` is BCP-47's own "undetermined" -- the honest value for
            // a book whose language nothing stated, and a valid one,
            // unlike omitting the element.
            out.push_str("    <dc:language>und</dc:language>\n");
        }
        for item in &book.metadata.items {
            if let Some(tag) = Self::dublin_core_tag(&item.term) {
                // `<dc:title>Value</dc:title>`, with any `opf:role` /
                // `opf:file-as` attributes the item carries.
                let mut attrs_str = String::new();
                for (k, v) in &item.attrib {
                    attrs_str.push_str(&format!(" {}=\"{}\"", k, escape_xml(v)));
                }

                out.push_str(&format!(
                    "    <{}{}>{}</{}>\n",
                    tag,
                    attrs_str,
                    escape_xml(&item.value),
                    tag
                ));
            } else {
                // <meta name="..." content="..." />
                // Or potentially other tags.
                // Assuming "meta" logic:
                out.push_str(&format!(
                    "    <meta name=\"{}\" content=\"{}\" />\n",
                    escape_xml(&item.term),
                    escape_xml(&item.value)
                ));
            }
        }
        out.push_str("  </metadata>\n");

        // Manifest
        out.push_str("  <manifest>\n");

        // EPUB 2 requires an NCX in the manifest and `spine/@toc` naming
        // it. Neither was emitted, so every book this produced had no
        // table of contents at all -- `opf_writer::write_ncx` existed the
        // whole time with no caller.
        let ncx_id = Self::existing_ncx_id(book);
        if ncx_id.is_none() {
            out.push_str("    <item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\" />\n");
        }
        for item in book.manifest.items.values() {
            out.push_str(&format!(
                "    <item id=\"{}\" href=\"{}\" media-type=\"{}\" />\n",
                escape_xml(&item.id),
                escape_xml(&item.href),
                escape_xml(&item.media_type)
            ));
        }
        out.push_str("  </manifest>\n");

        // Spine
        out.push_str(&format!("  <spine toc=\"{}\">\n", escape_xml(ncx_id.as_deref().unwrap_or("ncx")))); // toc attribute?
        for item in &book.spine.items {
            let linear = if item.linear { "yes" } else { "no" };
            out.push_str(&format!(
                "    <itemref idref=\"{}\" linear=\"{}\" />\n",
                escape_xml(&item.idref),
                linear
            ));
        }
        out.push_str("  </spine>\n");

        // Guide
        if !book.guide.references.is_empty() {
            out.push_str("  <guide>\n");
            for refs in book.guide.references.values() {
                let title = refs.title.as_deref().unwrap_or("");
                out.push_str(&format!(
                    "    <reference type=\"{}\" title=\"{}\" href=\"{}\" />\n",
                    escape_xml(&refs.type_),
                    escape_xml(title),
                    escape_xml(&refs.href)
                ));
            }
            out.push_str("  </guide>\n");
        }

        out.push_str("</package>");
        Ok(out)
    }
}

#[cfg(test)]
mod ncx_tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    fn a_book(dir: &std::path::Path) -> OEBBook {
        std::fs::write(dir.join("c1.html"), "<html><body><h1>One</h1></body></html>").unwrap();
        std::fs::write(dir.join("c2.html"), "<html><body><h1>Two</h1></body></html>").unwrap();
        let mut book = OEBBook::new(Box::new(DirContainer::new(dir)));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.manifest.add("c2", "c2.html", "application/xhtml+xml");
        book.spine.add("c1", true);
        book.spine.add("c2", true);
        book.metadata.add("title", "A Book");
        book
    }

    fn node(title: &str, href: Option<&str>, children: Vec<crate::oeb::toc::TOCNode>) -> crate::oeb::toc::TOCNode {
        crate::oeb::toc::TOCNode {
            title: Some(title.to_string()),
            href: href.map(|h| h.to_string()),
            id: None,
            klass: None,
            play_order: 0,
            description: None,
            author: None,
            children,
        }
    }

    /// EPUB 2 requires an NCX in the manifest and `spine/@toc` naming it.
    /// Neither was emitted, so every book this produced had no table of
    /// contents -- `opf_writer::write_ncx` existed the whole time with no
    /// caller, because it takes the *other* `TOC` type.
    #[test]
    fn write_book_produces_an_ncx_referenced_from_the_manifest_and_spine() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut book = a_book(src.path());
        book.toc.root.children.push(node("Chapter One", Some("c1.html"), vec![]));

        OEBWriter::new().write_book(&mut book, out.path()).unwrap();

        let opf = std::fs::read_to_string(out.path().join("content.opf")).unwrap();
        assert!(opf.contains(r#"media-type="application/x-dtbncx+xml""#), "no NCX in the manifest:\n{opf}");
        assert!(opf.contains(r#"<spine toc="ncx">"#), "the spine does not name the NCX:\n{opf}");
        assert!(out.path().join("toc.ncx").is_file(), "no toc.ncx was written");
    }

    /// The NCX's `dtb:uid` must equal the OPF's identifier. They are
    /// produced by separate calls, and `unique_identifier` *generates* a
    /// uuid when the book has none -- so computing it twice would give two
    /// different values and a book whose two halves disagree about its
    /// identity.
    #[test]
    fn the_ncx_uid_matches_the_opf_identifier() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut book = a_book(src.path());

        OEBWriter::new().write_book(&mut book, out.path()).unwrap();

        let opf = std::fs::read_to_string(out.path().join("content.opf")).unwrap();
        let ncx = std::fs::read_to_string(out.path().join("toc.ncx")).unwrap();

        let uuid = opf.split("opf:scheme=\"uuid\">").nth(1).and_then(|r| r.split('<').next()).expect("no identifier in the OPF").to_string();
        assert!(!uuid.is_empty());
        assert!(ncx.contains(&format!(r#"content="{uuid}""#)), "the NCX's dtb:uid does not match the OPF identifier {uuid:?}:\n{ncx}");
    }

    #[test]
    fn toc_entries_become_navpoints() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut book = a_book(src.path());
        book.toc.root.children.push(node("Chapter One", Some("c1.html"), vec![]));
        book.toc.root.children.push(node("Chapter Two", Some("c2.html"), vec![]));

        OEBWriter::new().write_book(&mut book, out.path()).unwrap();

        let ncx = std::fs::read_to_string(out.path().join("toc.ncx")).unwrap();
        assert_eq!(ncx.matches("<navPoint").count(), 2, "both chapters should be navigable:\n{ncx}");
        assert!(ncx.contains("Chapter One") && ncx.contains("Chapter Two"), "{ncx}");
    }

    /// A `navPoint` must point somewhere. A grouping node with no href is
    /// dropped, but its children are promoted rather than lost with it --
    /// otherwise a purely structural node would take its chapters away.
    #[test]
    fn a_node_with_no_href_is_dropped_but_keeps_its_children() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let mut book = a_book(src.path());
        book.toc.root.children.push(node("Part One", None, vec![node("Chapter One", Some("c1.html"), vec![])]));

        OEBWriter::new().write_book(&mut book, out.path()).unwrap();

        let ncx = std::fs::read_to_string(out.path().join("toc.ncx")).unwrap();
        assert!(ncx.contains("Chapter One"), "the child was lost with its parent:\n{ncx}");
        assert!(!ncx.contains("Part One"), "a node with no target should not become a navPoint:\n{ncx}");
    }

    /// A book that already has an NCX -- an EPUB round trip -- must not be
    /// given a second one.
    #[test]
    fn a_book_that_already_has_an_ncx_does_not_get_another() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        std::fs::write(src.path().join("existing.ncx"), "<ncx/>").unwrap();
        let mut book = a_book(src.path());
        book.manifest.add("theirs", "existing.ncx", "application/x-dtbncx+xml");

        OEBWriter::new().write_book(&mut book, out.path()).unwrap();

        let opf = std::fs::read_to_string(out.path().join("content.opf")).unwrap();
        assert_eq!(opf.matches("application/x-dtbncx+xml").count(), 1, "a second NCX was added:\n{opf}");
        assert!(opf.contains(r#"<spine toc="theirs">"#), "the spine should name the book's own NCX:\n{opf}");
    }
}
