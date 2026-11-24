//! SNB (Shanda Bambook) format support.
//!
//! Port of `old_src/src/calibre/ebooks/snb/{snbfile.py,snbml.py}`:
//! [`reader`]/[`writer`] handle the binary container format
//! (`SNBFile`), and [`snbml`] handles the OEB-XHTML-to-SNBC markup
//! conversion (`SNBMLizer`).

pub mod reader;
pub mod snbml;
pub mod writer;
