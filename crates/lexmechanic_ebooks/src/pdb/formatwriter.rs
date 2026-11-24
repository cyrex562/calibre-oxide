//! Port of `calibre.ebooks.pdb.formatwriter`: the interface every
//! PDB-format-specific writer (`ereader`, `plucker`, `ztxt`, ...)
//! implements to serialize an `OEBBook` into that format's bytes.
//!
//! Python's `FormatWriter.__init__(self, opts, log)` is a constructor
//! contract, not a behavior dispatched polymorphically -- see
//! [`super::formatreader`]'s module docs for why constructors aren't
//! part of this trait. [`FormatWriter::write_content`] is the one method
//! Python callers actually invoke through the abstract interface.

use crate::metadata::MetaInformation;
use crate::oeb::book::OEBBook;
use anyhow::Result;
use std::io::Write;

pub trait FormatWriter {
    /// Serialize `oeb_book` to `output_stream` in this writer's format.
    /// `metadata` overrides `oeb_book`'s own metadata when present,
    /// matching Python's `metadata=None` default (fall back to the
    /// book's metadata).
    fn write_content(
        &self,
        oeb_book: &OEBBook,
        output_stream: &mut dyn Write,
        metadata: Option<&MetaInformation>,
    ) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::NullContainer;

    struct RecordingWriter;

    impl FormatWriter for RecordingWriter {
        fn write_content(
            &self,
            _oeb_book: &OEBBook,
            output_stream: &mut dyn Write,
            metadata: Option<&MetaInformation>,
        ) -> Result<()> {
            let title = metadata.map(|m| m.title.as_str()).unwrap_or("untitled");
            output_stream.write_all(title.as_bytes())?;
            Ok(())
        }
    }

    #[test]
    fn trait_object_dispatches_write_content() {
        let writer = RecordingWriter;
        let book = OEBBook::new(Box::new(NullContainer::new()));
        let mi = MetaInformation {
            title: "Test Book".to_string(),
            ..Default::default()
        };
        let mut out = Vec::new();
        let dyn_writer: &dyn FormatWriter = &writer;
        dyn_writer
            .write_content(&book, &mut out, Some(&mi))
            .unwrap();
        assert_eq!(out, b"Test Book");
    }
}
