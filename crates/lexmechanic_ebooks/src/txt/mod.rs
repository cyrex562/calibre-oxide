//! Port of `old_src/src/calibre/ebooks/txt/`: the TXT format's input
//! and output-side markup processing (issue #54).
//!
//! `__init__.py` (10 lines) is a docstring/license header only -- there
//! is no code in it to port.
//!
//! - `newlines.rs`: port of `newlines.py` (`TxtNewlines`,
//!   `specified_newlines`).
//! - `processor.rs`: port of `processor.py` -- the real input-side
//!   algorithm (cleaning, splitting, markdown/textile/basic conversion,
//!   paragraph/formatting-type detection).
//! - `markdownml.rs`: port of `markdownml.py` (`MarkdownMLizer`).
//! - `textileml.rs`: port of `textileml.py` (`TextileMLizer`).
//! - `txtml.rs`: port of `txtml.py` (`TXTMLizer`), the plain-text
//!   output flavor.

pub mod markdownml;
pub mod newlines;
pub mod processor;
pub mod textileml;
pub mod txtml;
