//! LIT input plugin.
//!
//! Port of the reading half of `src/calibre/ebooks/lit/input.py`: open
//! the LIT container, write everything it holds out as a directory, and
//! hand back an [`OEBBook`] over it.

use crate::lit::reader::LitContainer;
use crate::oeb::book::OEBBook;
use crate::oeb::container::DirContainer;
use crate::oeb::manifest::ManifestItem;
use anyhow::{Context, Result};
use std::fs;
use std::io::BufReader;
use std::path::Path;

/// The LIT input conversion plugin.
pub struct LitInput;

impl Default for LitInput {
    fn default() -> Self {
        Self::new()
    }
}

impl LitInput {
    /// Build the plugin.
    pub fn new() -> Self {
        LitInput
    }

    /// Extract `input_path` into `output_dir` and describe it as an
    /// OEB book.
    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        let file = fs::File::open(input_path).context("Failed to open LIT file")?;
        let reader = BufReader::new(file);
        let name = input_path.file_name().and_then(|n| n.to_str());
        let mut container =
            LitContainer::new(reader, name).context("Failed to parse LIT container")?;

        fs::create_dir_all(output_dir)?;

        // The OPF is reconstructed from the tokenised `/meta` entry.
        let opf = container
            .get_metadata()
            .context("Failed to read LIT metadata")?;
        let opf_path = container.litfile.opf_path.clone();
        fs::write(output_dir.join(&opf_path), opf.as_bytes())?;

        let mut book = OEBBook::new(Box::new(DirContainer::new(output_dir)));

        // Manifest order is not meaningful, but a stable one keeps the
        // extracted directory reproducible.
        let mut internals: Vec<String> = container.litfile.manifest.keys().cloned().collect();
        internals.sort();

        for internal in internals {
            let item = container.litfile.manifest[&internal].clone();
            let data = container
                .read(&item.path)
                .with_context(|| format!("Failed to read {} from LIT file", item.path))?;

            let target = output_dir.join(&item.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, &data)?;

            let media_type = if item.state.contains("spine") {
                "application/xhtml+xml"
            } else {
                item.mime_type.as_str()
            };
            book.manifest.items.insert(
                item.internal.clone(),
                ManifestItem::new(&item.internal, &item.path, media_type),
            );
            book.manifest
                .hrefs
                .insert(item.path.clone(), item.internal.clone());
            if item.state == "spine" {
                book.spine.add(&item.internal, true);
            } else if item.state == "not spine" {
                book.spine.add(&item.internal, false);
            }
        }

        Ok(book)
    }
}
