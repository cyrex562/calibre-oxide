//! Minimal OPF/NCX package writer, shared across every output plugin
//! that needs to hand-write a package rather than build one via
//! `crate::oeb`'s in-memory `OEBBook` (currently [`crate::mobi::mobi6`]/
//! [`crate::mobi::mobi8`] and `docx::to_html`'s not-yet-wired-up
//! `write` step, issue #130/#288).
//!
//! Python's readers build these documents via
//! `calibre.ebooks.metadata.opf2.OPFCreator`/`Guide` and
//! `calibre.ebooks.metadata.toc.TOC`. This crate has no equivalent OPF
//! *writer* yet (`crate::opf` only parses), so rather than duplicate a
//! general-purpose OPF writer inside each unrelated reader/converter
//! module, this file provides the narrow slice they actually need: an
//! OPF 2.0 package document (metadata + manifest + spine + guide), an
//! NCX 2005-1 document built from `crate::metadata::toc::TOC`, and
//! ([`scan_directory_manifest`]) a manifest built by walking a
//! directory of already-written files -- Python's
//! `OPFCreator.create_manifest_from_files_in`.
//!
//! Relocated here (originally `mobi/opf_writer.rs`) once a second,
//! unrelated module needed it -- same rationale as `crate::dom`'s and
//! `crate::xmltree`'s own moves out of `mobi::`: don't build a second
//! copy of a type/module that's already general-purpose, just widen
//! its home.

use std::path::Path;

use crate::metadata::toc::{TOCNode, TOC};
use crate::metadata::MetaInformation;

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
}

impl ManifestItem {
    pub fn new(
        id: impl Into<String>,
        href: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Self {
        ManifestItem {
            id: id.into(),
            href: href.into(),
            media_type: media_type.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GuideRef {
    pub type_: String,
    pub title: String,
    pub href: String,
}

/// Assigns sequential manifest ids (`id1`, `id2`, ...) to a set of
/// `(href, media_type)` pairs, matching `OPFCreator.create_manifest`'s
/// auto-id behavior closely enough for our purposes (ids only need to be
/// unique within the document).
pub fn auto_manifest(items: &[(String, String)]) -> Vec<ManifestItem> {
    items
        .iter()
        .enumerate()
        .map(|(i, (href, mt))| ManifestItem::new(format!("id{}", i + 1), href.clone(), mt.clone()))
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn write_opf(
    mi: &MetaInformation,
    manifest: &[ManifestItem],
    spine_idrefs: &[String],
    guide: &[GuideRef],
    ncx_manifest_id: Option<&str>,
    cover_href: Option<&str>,
    page_progression_direction: Option<&str>,
    primary_writing_mode: Option<&str>,
) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    let mut package_attrs = String::from(
        "<package xmlns=\"http://www.idpf.org/2007/opf\" unique-identifier=\"uuid_id\" version=\"2.0\"",
    );
    if let Some(ppd) = page_progression_direction {
        package_attrs.push_str(&format!(
            " page-progression-direction=\"{}\"",
            xml_escape(ppd)
        ));
    }
    if let Some(pwm) = primary_writing_mode {
        package_attrs.push_str(&format!(" primary-writing-mode=\"{}\"", xml_escape(pwm)));
    }
    package_attrs.push('>');
    out.push_str(&package_attrs);
    out.push('\n');

    out.push_str("  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\n");
    out.push_str(&format!(
        "    <dc:title>{}</dc:title>\n",
        xml_escape(&mi.title)
    ));
    for author in &mi.authors {
        let sort = mi
            .author_sort_map
            .get(author)
            .cloned()
            .unwrap_or_else(|| author.clone());
        out.push_str(&format!(
            "    <dc:creator opf:role=\"aut\" opf:file-as=\"{}\">{}</dc:creator>\n",
            xml_escape(&sort),
            xml_escape(author)
        ));
    }
    for lang in &mi.languages {
        out.push_str(&format!(
            "    <dc:language>{}</dc:language>\n",
            xml_escape(lang)
        ));
    }
    if let Some(uuid) = &mi.uuid {
        out.push_str(&format!(
            "    <dc:identifier id=\"uuid_id\" opf:scheme=\"uuid\">{}</dc:identifier>\n",
            xml_escape(uuid)
        ));
    }
    for (scheme, value) in &mi.identifiers {
        out.push_str(&format!(
            "    <dc:identifier opf:scheme=\"{}\">{}</dc:identifier>\n",
            xml_escape(scheme),
            xml_escape(value)
        ));
    }
    if let Some(desc) = &mi.comments {
        out.push_str(&format!(
            "    <dc:description>{}</dc:description>\n",
            xml_escape(desc)
        ));
    }
    if let Some(publ) = &mi.publisher {
        out.push_str(&format!(
            "    <dc:publisher>{}</dc:publisher>\n",
            xml_escape(publ)
        ));
    }
    for tag in &mi.tags {
        out.push_str(&format!(
            "    <dc:subject>{}</dc:subject>\n",
            xml_escape(tag)
        ));
    }
    if let Some(pd) = &mi.pubdate {
        out.push_str(&format!(
            "    <dc:date opf:event=\"publication\">{}</dc:date>\n",
            pd.to_rfc3339()
        ));
    }
    if let Some(href) = cover_href {
        out.push_str(&format!(
            "    <meta name=\"cover\" content=\"{}\"/>\n",
            xml_escape(href)
        ));
    }
    out.push_str("  </metadata>\n");

    out.push_str("  <manifest>\n");
    for item in manifest {
        out.push_str(&format!(
            "    <item id=\"{}\" href=\"{}\" media-type=\"{}\"/>\n",
            xml_escape(&item.id),
            xml_escape(&item.href),
            xml_escape(&item.media_type)
        ));
    }
    if let Some(id) = ncx_manifest_id {
        out.push_str(&format!(
            "    <item id=\"{}\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/>\n",
            xml_escape(id)
        ));
    }
    out.push_str("  </manifest>\n");

    out.push_str(&format!(
        "  <spine{}>\n",
        match ncx_manifest_id {
            Some(id) => format!(" toc=\"{}\"", xml_escape(id)),
            None => String::new(),
        }
    ));
    for idref in spine_idrefs {
        out.push_str(&format!("    <itemref idref=\"{}\"/>\n", xml_escape(idref)));
    }
    out.push_str("  </spine>\n");

    if !guide.is_empty() {
        out.push_str("  <guide>\n");
        for g in guide {
            out.push_str(&format!(
                "    <reference type=\"{}\" title=\"{}\" href=\"{}\"/>\n",
                xml_escape(&g.type_),
                xml_escape(&g.title),
                xml_escape(&g.href)
            ));
        }
        out.push_str("  </guide>\n");
    }

    out.push_str("</package>\n");
    out
}

/// Renders `toc` as an NCX 2005-1 document (`toc.ncx`).
pub fn write_ncx(toc: &TOC, uid: &str, doc_title: &str) -> String {
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
    out.push_str("<!DOCTYPE ncx PUBLIC \"-//NISO//DTD ncx 2005-1//EN\" \"http://www.daisy.org/z3986/2005/ncx-2005-1.dtd\">\n");
    out.push_str("<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\n");
    out.push_str("  <head>\n");
    out.push_str(&format!(
        "    <meta name=\"dtb:uid\" content=\"{}\"/>\n",
        xml_escape(uid)
    ));
    out.push_str("    <meta name=\"dtb:depth\" content=\"1\"/>\n");
    out.push_str("    <meta name=\"dtb:totalPageCount\" content=\"0\"/>\n");
    out.push_str("    <meta name=\"dtb:maxPageNumber\" content=\"0\"/>\n");
    out.push_str("  </head>\n");
    out.push_str(&format!(
        "  <docTitle><text>{}</text></docTitle>\n",
        xml_escape(doc_title)
    ));
    out.push_str("  <navMap>\n");
    let mut play_order = 1;
    for node in &toc.nodes {
        write_nav_point(&mut out, node, &mut play_order, 2);
    }
    out.push_str("  </navMap>\n");
    out.push_str("</ncx>\n");
    out
}

fn write_nav_point(out: &mut String, node: &TOCNode, play_order: &mut usize, indent: usize) {
    let pad = "  ".repeat(indent);
    let id = format!("np{}", *play_order);
    out.push_str(&format!(
        "{pad}<navPoint id=\"{}\" playOrder=\"{}\">\n",
        xml_escape(&id),
        play_order
    ));
    *play_order += 1;
    out.push_str(&format!(
        "{pad}  <navLabel><text>{}</text></navLabel>\n",
        xml_escape(&node.title)
    ));
    out.push_str(&format!(
        "{pad}  <content src=\"{}\"/>\n",
        xml_escape(&node.src)
    ));
    for child in &node.children {
        write_nav_point(out, child, play_order, indent + 1);
    }
    out.push_str(&format!("{pad}</navPoint>\n"));
}

/// Walks `dir` for every already-written file, returning `(relative
/// href, media type)` pairs -- pass to [`auto_manifest`] for a full
/// [`ManifestItem`] list. `skip` is matched against the file's path
/// relative to `dir` with `/`-separators (e.g. `"metadata.opf"`),
/// letting a caller exclude package files it's about to write itself
/// (this function doesn't know their names) or anything else that
/// shouldn't be in the manifest.
///
/// Media types come from the file extension (falling back to
/// `application/octet-stream`), with the same handful of remappings
/// `crate::mobi::mobi8`'s own directory scan already used: `text/html`
/// -> `application/xhtml+xml` (matches Python's own
/// `guess_type('a.xhtml')` swap in both `mobi8.py` and `docx/to_html.py`'s
/// `write`), and the three TrueType/OpenType/WOFF font types
/// `mime_guess` names differently than EPUB/OPF readers expect.
///
/// Port of `OPFCreator.create_manifest_from_files_in`.
pub fn scan_directory_manifest(dir: &Path, skip: &[&str]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry.path().strip_prefix(dir).unwrap_or(entry.path());
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if skip.contains(&rel_str.as_str()) {
            continue;
        }
        let mut mt = mime_guess::from_path(&rel_str)
            .first()
            .map(|m| m.to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());
        mt = match mt.as_str() {
            "text/html" => "application/xhtml+xml".to_string(),
            "font/ttf" => "application/x-font-truetype".to_string(),
            "font/otf" => "application/vnd.ms-opentype".to_string(),
            "font/woff" => "application/font-woff".to_string(),
            other => other.to_string(),
        };
        pairs.push((rel_str, mt));
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_opf_basic_structure() {
        let mut mi = MetaInformation::new("My Title", vec!["Author One".to_string()]);
        mi.publisher = Some("Pub Co".to_string());
        let manifest = auto_manifest(&[
            (
                "index.html".to_string(),
                "application/xhtml+xml".to_string(),
            ),
            ("styles.css".to_string(), "text/css".to_string()),
        ]);
        let guide = vec![GuideRef {
            type_: "cover".to_string(),
            title: "Cover".to_string(),
            href: "index.html#cover".to_string(),
        }];
        let xml = write_opf(
            &mi,
            &manifest,
            &["id1".to_string()],
            &guide,
            Some("ncx"),
            Some("images/00001.jpg"),
            Some("ltr"),
            None,
        );

        assert!(xml.contains("<dc:title>My Title</dc:title>"), "{xml}");
        assert!(xml.contains("<dc:creator"), "{xml}");
        assert!(xml.contains("Author One"), "{xml}");
        assert!(xml.contains("<dc:publisher>Pub Co</dc:publisher>"), "{xml}");
        assert!(xml.contains("page-progression-direction=\"ltr\""), "{xml}");
        assert!(xml.contains("<itemref idref=\"id1\"/>"), "{xml}");
        assert!(xml.contains("toc.ncx"), "{xml}");
        assert!(xml.contains("<reference type=\"cover\""), "{xml}");
        assert!(xml.contains("meta name=\"cover\""), "{xml}");
    }

    #[test]
    fn test_write_opf_escapes_special_characters() {
        let mi = MetaInformation::new("A & B <Title>", vec!["Author \"Q\"".to_string()]);
        let xml = write_opf(&mi, &[], &[], &[], None, None, None, None);
        assert!(xml.contains("A &amp; B &lt;Title&gt;"), "{xml}");
        assert!(!xml.contains("<Title>"), "{xml}");
    }

    #[test]
    fn test_write_ncx_nested_toc() {
        let toc = TOC {
            nodes: vec![TOCNode {
                title: "Chapter 1".to_string(),
                src: "text/part0001.html".to_string(),
                children: vec![TOCNode {
                    title: "Section 1.1".to_string(),
                    src: "text/part0001.html#s1".to_string(),
                    children: Vec::new(),
                }],
            }],
        };
        let ncx = write_ncx(&toc, "urn:uuid:1234", "My Book");
        assert!(
            ncx.contains("<docTitle><text>My Book</text></docTitle>"),
            "{ncx}"
        );
        assert!(ncx.contains("Chapter 1"), "{ncx}");
        assert!(ncx.contains("Section 1.1"), "{ncx}");
        assert!(ncx.contains("text/part0001.html#s1"), "{ncx}");
        // Nested navPoint should appear inside the parent's.
        let parent_pos = ncx.find("Chapter 1").unwrap();
        let child_pos = ncx.find("Section 1.1").unwrap();
        let parent_close = ncx[parent_pos..].find("</navPoint>").unwrap() + parent_pos;
        assert!(
            child_pos < parent_close,
            "child navPoint should nest inside parent"
        );
    }

    #[test]
    fn test_auto_manifest_assigns_unique_ids() {
        let items = auto_manifest(&[
            ("a.html".to_string(), "application/xhtml+xml".to_string()),
            ("b.html".to_string(), "application/xhtml+xml".to_string()),
        ]);
        assert_eq!(items[0].id, "id1");
        assert_eq!(items[1].id, "id2");
        assert_ne!(items[0].id, items[1].id);
    }

    mod scan_directory_manifest_tests {
        use super::*;

        #[test]
        fn finds_every_file_and_maps_html_to_xhtml() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("index.html"), "<html/>").unwrap();
            std::fs::write(dir.path().join("style.css"), "").unwrap();

            let mut pairs = scan_directory_manifest(dir.path(), &[]);
            pairs.sort();

            assert_eq!(
                pairs,
                vec![
                    (
                        "index.html".to_string(),
                        "application/xhtml+xml".to_string()
                    ),
                    ("style.css".to_string(), "text/css".to_string()),
                ]
            );
        }

        #[test]
        fn skipped_names_are_excluded() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("index.html"), "<html/>").unwrap();
            std::fs::write(dir.path().join("metadata.opf"), "").unwrap();
            std::fs::write(dir.path().join("toc.ncx"), "").unwrap();

            let pairs = scan_directory_manifest(dir.path(), &["metadata.opf", "toc.ncx"]);

            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].0, "index.html");
        }

        #[test]
        fn nested_files_get_a_relative_forward_slash_path() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("images")).unwrap();
            std::fs::write(dir.path().join("images/cover.jpg"), b"\xff\xd8").unwrap();

            let pairs = scan_directory_manifest(dir.path(), &[]);

            assert_eq!(pairs.len(), 1);
            assert_eq!(pairs[0].0, "images/cover.jpg");
            assert_eq!(pairs[0].1, "image/jpeg");
        }

        #[test]
        fn an_unrecognized_extension_falls_back_to_octet_stream() {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("data.bin"), b"\x00\x01").unwrap();

            let pairs = scan_directory_manifest(dir.path(), &[]);

            assert_eq!(pairs[0].1, "application/octet-stream");
        }
    }
}
