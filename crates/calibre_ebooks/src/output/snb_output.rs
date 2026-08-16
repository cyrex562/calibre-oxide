//! SNB output plugin wiring.
//!
//! This is a scoped-down stand-in for
//! `old_src/.../conversion/plugins/snb_output.py`'s `SNBOutput.convert`
//! -- that plugin (TOC-driven chapter splitting/merging across spine
//! items, SVG rasterization, per-image resizing for a target screen)
//! lives in a different `old_src` directory and is tracked as its own,
//! separate porting item in `docs/modules_to_port.md`; it is not part
//! of issue #52 (`ebooks/snb/{snbfile.py,snbml.py}`).
//!
//! What issue #52 *does* require is wiring this crate's real
//! [`crate::snb::writer::SnbWriter`] (container writer) and
//! [`crate::snb::snbml::SnbMlizer`] (markup converter) in for the
//! previously-stub `SnbOutput`, with real metadata from `book.metadata`
//! -- so this `convert` does that: one SNBC sub-document per spine
//! item (no cross-item chapter merging), images copied through
//! byte-for-byte (see [`crate::snb::snbml`]'s docs for why re-encoding
//! to JPEG -- a real thing `HandleImage` does in the unported plugin --
//! is out of scope here), and a real `snbf/book.snbf` metadata file.

use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::TagStylizer;
use crate::snb::snbml::{process_file_name, SnbMlizer, SnbOptions};
use crate::snb::writer::{SnbOutputFile, SnbWriter};
use crate::xml_util::prepare_string_for_xml;
use anyhow::{Context, Result};
use roxmltree::Document;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

pub struct SnbOutput;

impl SnbOutput {
    pub fn new() -> Self {
        SnbOutput
    }

    pub fn convert(&self, book: &OEBBook, output_path: &Path) -> Result<()> {
        let opts = SnbOptions::default();
        let mut files: Vec<SnbOutputFile> = Vec::new();

        files.push(SnbOutputFile::plain(
            "snbf/book.snbf",
            build_book_snbf(book).into_bytes(),
        ));

        let title = book
            .metadata
            .first("title")
            .map(|i| i.value.clone())
            .unwrap_or_default();

        let stylizer = TagStylizer;
        for spine_item in &book.spine.items {
            let Some(item) = book.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = book.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            let Ok(doc) = Document::parse(&content) else {
                continue;
            };
            let Some(body) = doc
                .descendants()
                .find(|n| n.is_element() && n.tag_name().name() == "body")
            else {
                continue;
            };

            let subitems = vec![(String::new(), title.clone())];
            let mut mlizer = SnbMlizer::new();
            let trees = mlizer
                .extract_content(body, &item.href, &subitems, &stylizer, &opts)
                .with_context(|| format!("Failed to convert {} to SNBC", item.href))?;
            let doc_xml = trees[""].to_xml();

            let snbc_name = format!("snbc/{}.snbc", process_file_name(&item.href));
            files.push(SnbOutputFile::plain(snbc_name, doc_xml.into_bytes()));
        }

        for item in book.manifest.iter() {
            if !item.media_type.starts_with("image/") {
                continue;
            }
            if let Ok(data) = book.container.read(&item.href) {
                let name = format!("snbc/images/{}", process_file_name(&item.href));
                files.push(SnbOutputFile::binary(name, data));
            }
        }

        let file = File::create(output_path).context("Failed to create SNB file")?;
        let mut writer = BufWriter::new(file);
        SnbWriter::new(files)
            .output(&mut writer)
            .context("Failed to write SNB container")?;

        Ok(())
    }
}

impl Default for SnbOutput {
    fn default() -> Self {
        Self::new()
    }
}

/// Port of the `book-snbf` metadata document `SNBOutput.convert` builds
/// (the subset that comes from `oeb_book.metadata` -- `rights`/
/// `created` are always emitted empty by the Python too, and `cover`
/// needs the guide/titlepage resolution the unported plugin does, so is
/// left empty here).
fn build_book_snbf(book: &OEBBook) -> String {
    let title = book
        .metadata
        .first("title")
        .map(|i| i.value.as_str())
        .unwrap_or("");
    let authors: Vec<&str> = book
        .metadata
        .get("creator")
        .iter()
        .map(|i| i.value.as_str())
        .collect();
    let language = book
        .metadata
        .first("language")
        .map(|i| i.value.to_uppercase())
        .unwrap_or_default();
    let publisher = book
        .metadata
        .first("publisher")
        .map(|i| i.value.as_str())
        .unwrap_or("");
    let description = book
        .metadata
        .first("description")
        .map(|i| i.value.as_str())
        .unwrap_or("");

    format!(
        "<?xml version='1.0' encoding='utf-8'?>\n\
         <book-snbf version=\"1.0\">\n\
         \x20 <head>\n\
         \x20   <name>{}</name>\n\
         \x20   <author>{}</author>\n\
         \x20   <language>{}</language>\n\
         \x20   <rights/>\n\
         \x20   <publisher>{}</publisher>\n\
         \x20   <generator>calibre-oxide</generator>\n\
         \x20   <created/>\n\
         \x20   <abstract>{}</abstract>\n\
         \x20   <cover/>\n\
         \x20 </head>\n\
         </book-snbf>\n",
        prepare_string_for_xml(title, false),
        prepare_string_for_xml(&authors.join(" "), false),
        prepare_string_for_xml(&language, false),
        prepare_string_for_xml(publisher, false),
        prepare_string_for_xml(description, false),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    #[test]
    fn writes_a_valid_container_with_real_metadata_and_content() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let output_path = tmp_dir.path().join("book.snb");
        let mut book = OEBBook::new(Box::new(DirContainer::new(tmp_dir.path())));
        book.manifest
            .add("item1", "index.html", "application/xhtml+xml");
        book.spine.add("item1", true);
        book.container
            .write(
                "index.html",
                b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Hello SNB</p></body></html>",
            )
            .unwrap();
        book.metadata.add("title", "My Book");
        book.metadata.add("creator", "Author One");

        let result = SnbOutput::new().convert(&book, &output_path);
        assert!(result.is_ok(), "{result:?}");
        assert!(output_path.exists());
        assert!(std::fs::metadata(&output_path).unwrap().len() > 44);
    }
}
