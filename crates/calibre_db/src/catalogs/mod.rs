//! Port of `old_src/src/calibre/library/catalogs/` (issue #57) -- calibre's
//! catalog generators, which render a library's book list out to BibTeX,
//! CSV/XML, or an EPUB/MOBI/AZW3 "browsable catalog" ebook.
//!
//! # What's here
//!
//! - [`utils`]: `NumberToText`, a small recursive number-to-English-words
//!   converter used by the EPUB/MOBI builder's series/genre sort text.
//!
//! The remaining files (`bibtex.py`, `csv_xml.py`, `epub_mobi.py`,
//! `epub_mobi_builder.py`) are follow-up work; each generator subclasses
//! `calibre.customize.CatalogPlugin`, itself a subclass of the generic
//! `calibre.customize.Plugin`. This crate doesn't yet have any plugin
//! *registration/discovery* system consuming a `CatalogPlugin`-shaped trait
//! object (`crates/calibre_db/src/cli/cmd_catalog.rs`, the one existing
//! catalog entry point, dispatches on file extension directly, not through
//! a plugin registry), so those generators will be ported as plain structs
//! with a `run` method rather than a trait implementation once their turn
//! comes, not as part of this file.

pub mod utils;
