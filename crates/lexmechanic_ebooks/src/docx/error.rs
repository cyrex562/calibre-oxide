//! Errors raised while reading a DOCX package.
//!
//! The Python raises `InvalidDOCX` (a `ValueError` subclass, defined in
//! `old_src/src/calibre/ebooks/docx/__init__.py`) for structural
//! problems and lets zip/XML exceptions escape as themselves. This
//! enum keeps that distinction while naming the part at fault, since
//! "which file inside the package was wrong" is the first question
//! anyone asks of a broken DOCX.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DocxError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("XML parse error: {0}")]
    Xml(#[from] roxmltree::Error),
    /// A structural problem with the package. Port of the Python
    /// `InvalidDOCX`.
    #[error("invalid DOCX: {0}")]
    InvalidDocx(String),
    /// A part the caller asked for is not in the package.
    #[error("no such part in the DOCX package: {0}")]
    MissingPart(String),
}
