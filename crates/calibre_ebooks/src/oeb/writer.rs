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
        let opf_content = self.write_opf(book)?;
        container.write("content.opf", opf_content.as_bytes())?;

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
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");

        // EPUB 2 requires `dc:title`, `dc:identifier` and `dc:language`,
        // and requires `package/@unique-identifier` to name the
        // identifier's `id`. None of the three was emitted, so every book
        // this produced was invalid -- a validating reader refuses it.
        // Worked out before the `<package>` tag because the attribute goes
        // on it.
        let identifier = Self::unique_identifier(book);
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
        out.push_str("  <spine>\n"); // toc attribute?
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
