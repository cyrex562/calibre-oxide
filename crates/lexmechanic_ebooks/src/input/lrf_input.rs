//! LRF input: not supported, and it says so.
//!
//! This used to write a page reading "LRF Content Not Supported Yet" and
//! return it as the book. It is registered, so `ebook-convert book.lrf
//! out.epub` reported **success** and handed the user that page -- a
//! conversion that looks done and produced nothing.
//!
//! LRF is a discontinued Sony format whose content is a tree of binary
//! objects (`old_src/.../lrf/{lrfparser,objects,tags}.py`). There is no
//! parser for it in this tree, and unlike TCR (#943) it is not a small
//! port. So this refuses, with a message that says what is wrong and
//! distinguishes "not implemented" from "your file is broken".
//!
//! Found while auditing the registered input plugins for placeholders.
//! Two of the four claimed something could not be done that could
//! (#942 CHM, #943 TCR); this one is accurate about the obstacle, and the
//! defect was only that it reported success anyway.

use crate::oeb::book::OEBBook;
use anyhow::{bail, Result};
use std::path::Path;

pub struct LRFInput;

impl LRFInput {
    pub fn new() -> Self {
        LRFInput
    }

    pub fn convert(&self, input_path: &Path, _output_dir: &Path) -> Result<OEBBook> {
        bail!(
            "cannot convert {}: LRF content extraction is not implemented. \
             LRF stores its text as a tree of binary objects and no parser for it exists in this build",
            input_path.display()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect was not the missing feature -- it was reporting success.
    #[test]
    fn conversion_fails_instead_of_returning_a_fabricated_book() {
        let dir = tempfile::tempdir().unwrap();
        let book = dir.path().join("book.lrf");
        std::fs::write(&book, b"LRF bytes").unwrap();
        let out = dir.path().join("out");

        let message = match LRFInput::new().convert(&book, &out) {
            Ok(_) => panic!("an unimplemented format must not convert successfully"),
            Err(e) => format!("{e:#}"),
        };
        assert!(message.contains("not implemented"), "the error should say what is missing: {message}");
        assert!(message.contains("book.lrf"), "the error should name the file: {message}");
        assert!(!out.exists(), "nothing should have been written");
    }
}
