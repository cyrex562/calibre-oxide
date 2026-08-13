//! LIT output plugin.
//!
//! Port of the writing half of `src/calibre/ebooks/lit/output.py`:
//! stamp on the Microsoft cover guide references, then hand the book to
//! [`LitWriter`].

use crate::lit::writer::{litize_oeb, LitWriter, ProviderStyles};
use crate::oeb::book::OEBBook;
use crate::oeb::stylizer::{StyleProvider, TagStylizer};
use anyhow::{Context, Result};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

/// The LIT output conversion plugin.
pub struct LitOutput {
    /// Where `display` and the page-break properties come from.
    /// Defaults to the HTML defaults plus inline styles.
    styles: Box<dyn StyleProvider>,
}

impl Default for LitOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl LitOutput {
    /// Build the plugin with the tag-defaults style provider.
    pub fn new() -> Self {
        LitOutput {
            styles: Box::new(TagStylizer),
        }
    }

    /// Build the plugin with a caller-supplied style provider, for
    /// conversions that have a real cascade to offer.
    pub fn with_styles(styles: Box<dyn StyleProvider>) -> Self {
        LitOutput { styles }
    }

    /// Write `book` to `output_path`.
    ///
    /// Returns any non-fatal problems noticed on the way, which the
    /// Python logs.
    pub fn convert(&self, book: &mut OEBBook, output_path: &Path) -> Result<Vec<String>> {
        let mut warnings = litize_oeb(book);

        let file = File::create(output_path).context("Failed to create output LIT file")?;
        let mut stream = BufWriter::new(file);
        let lit_styles = ProviderStyles::new(self.styles.as_ref());
        let mut writer = LitWriter::new();
        writer
            .write(book, Some(&lit_styles), &mut stream)
            .context("Failed to write LIT file")?;
        warnings.extend(writer.warnings);
        Ok(warnings)
    }
}
