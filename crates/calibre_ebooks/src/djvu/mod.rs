//! DjVu support — container parsing, text extraction, and the BZZ
//! decompressor DjVu uses for its text layer.
//!
//! Port of `old_src/src/calibre/ebooks/djvu/`:
//!
//! | Python / C                | Rust             |
//! | ------------------------- | ---------------- |
//! | `djvu.py`                 | [`file`]         |
//! | `djvubzzdec.py`           | [`bzz`]          |
//! | `bzzdecoder.c`            | [`bzz`]          |
//! | `__init__.py` (docstring) | this module doc  |
//!
//! Calibre ships the BZZ decoder twice — a pure-Python implementation
//! and a C extension that is what actually runs. The Rust port is a
//! single implementation covering both; where they disagree it follows
//! the C one. See the [`bzz`] module docs.
//!
//! ```no_run
//! use calibre_ebooks::djvu::DjvuFile;
//!
//! let book = DjvuFile::open("scan.djvu")?;
//! let text = book.text()?;
//! # Ok::<(), calibre_ebooks::djvu::DjvuError>(())
//! ```

pub mod bzz;
pub mod file;

pub use bzz::{decompress, BzzDecoder, BzzError};
pub use file::{DjvuChunk, DjvuError, DjvuFile, MAGIC, TEXT_SEPARATOR};
