//! Loose HTML as a book format.
//!
//! Port of `old_src/src/calibre/ebooks/html/`:
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` (docstring only) | this module |
//! | `input.py` | [`input`] |
//! | `meta.py` | [`meta`] |
//! | `to_zip.py` | [`to_zip`] |
//!
//! An HTML "book" is a starting file plus everything it links to, so
//! reading one means crawling it. [`input`] does that crawl; [`to_zip`]
//! is the import plugin that packages the result.

pub mod input;
pub mod meta;
pub mod to_zip;

pub use input::{get_filelist, traverse, HtmlFile, Link, Traversal};
pub use meta::EasyMeta;
pub use to_zip::{parse_settings, Settings};
