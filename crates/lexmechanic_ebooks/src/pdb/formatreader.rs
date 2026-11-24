//! Port of `calibre.ebooks.pdb.formatreader`: the interface every
//! PDB-format-specific reader (`ereader`, `plucker`, `ztxt`, ...)
//! implements to extract its content into an OEB directory.
//!
//! Python's `FormatReader.__init__(self, header, stream, log, options)`
//! is a constructor contract, not a behavior every caller dispatches
//! through polymorphically — each concrete reader in this crate
//! constructs itself directly (`SomeReader::new(header, stream)`, as
//! the already-ported `PdbReader` does), so it isn't part of this trait.
//! [`FormatReader::extract_content`] is the one method Python callers
//! actually invoke through the abstract interface.

use anyhow::Result;
use std::path::Path;

pub trait FormatReader {
    /// Extract this reader's content into `output_dir` as an exploded
    /// OEB (HTML + images + OPF).
    fn extract_content(&self, output_dir: &Path) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct RecordingReader {
        calls: RefCell<Vec<std::path::PathBuf>>,
    }

    impl FormatReader for RecordingReader {
        fn extract_content(&self, output_dir: &Path) -> Result<()> {
            self.calls.borrow_mut().push(output_dir.to_path_buf());
            Ok(())
        }
    }

    #[test]
    fn trait_object_dispatches_extract_content() {
        let reader = RecordingReader {
            calls: RefCell::new(Vec::new()),
        };
        let dyn_reader: &dyn FormatReader = &reader;
        dyn_reader.extract_content(Path::new("/tmp/out")).unwrap();
        assert_eq!(reader.calls.borrow()[0], Path::new("/tmp/out"));
    }
}
