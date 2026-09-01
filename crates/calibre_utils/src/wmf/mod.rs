//! Port of `old_src/src/calibre/utils/wmf/` (issue #80): parsing
//! legacy Windows metafile formats (WMF and EMF) far enough to
//! extract an embedded raster image and re-encode it as PNG --
//! [`parse::wmf_unwrap`]/[`emf::emf_unwrap`]'s whole job, and the only
//! thing any real caller in this codebase needs (RTF/DOCX image
//! extraction pulling an embedded bitmap out of a metafile-wrapped OLE
//! object -- `calibre_ebooks::docx::images` disclosed this exact gap
//! in its own module doc before this port existed).
//!
//! # Scope
//!
//! Real: [`dib::DibHeader`]/[`dib::create_bmp_from_dib`] (raw DIB ->
//! standalone `.bmp` reconstruction, `__init__.py`), the WMF and EMF
//! record-stream parsers ([`parse::Wmf`]/[`emf::Emf`]) walking every
//! record to find `DibStretchBlt`/`EMR_STRETCHDIBITS` raster payloads,
//! and [`dib::bmp_to_png`] (real BMP decode + PNG encode via the
//! `image` crate, replacing upstream's Qt-based `to_png`).
//!
//! Narrowed: neither parser does anything with the vector-drawing
//! records (`LineTo`, `Polygon`, `TextOut`, etc.) beyond skipping over
//! them to find the next record -- exactly like upstream, which only
//! defines handlers for the handful of record types it actually needs
//! and silently ignores the rest.

pub mod dib;
pub mod emf;
pub mod parse;

pub use dib::{DibError, DibHeader};
pub use emf::{emf_unwrap, Emf, EmfError};
pub use parse::{wmf_unwrap, Wmf, WmfError};
