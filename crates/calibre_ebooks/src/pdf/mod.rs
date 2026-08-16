//! Port of `old_src/src/calibre/ebooks/pdf/` (issue #45: `develop.py`,
//! `html_writer.py`, `image_writer.py`, `pdftohtml.py`, `reflow.py`,
//! `utils.h`).
//!
//! | Python | Rust | Status |
//! | --- | --- | --- |
//! | `reflow.py` | [`reflow`] | Real port: pure geometry/statistics, no native dependency. |
//! | `pdftohtml.py` | [`pdftohtml`] | Real port: subprocess wrapper around the external `pdftohtml` binary. |
//! | `utils.h` | [`utils`] | Real port: XML-escape helper + exception type. |
//! | `html_writer.py` | [`html_writer`] | Documented gap: Qt `QWebEnginePage` HTML->PDF rendering core has no local equivalent; separable pure logic ported where found. |
//! | `image_writer.py` | [`image_writer`] | Documented gap: depends on the separately-tracked `pdf/render/` native PDF serializer + Qt page geometry; page-size/unit-conversion math ported for real. |
//! | `develop.py` | [`develop`] | Documented gap: Qt+podofo CLI dev tool, entirely dependent on `html_writer.py`'s rendering core. |
//!
//! `reflow.py` + `pdftohtml.py` together form calibre's "real" PDF-input
//! pipeline (shell out to poppler's `pdftohtml -xml`, then reflow the
//! resulting layout XML into clean HTML). This crate already has a
//! separate, much simpler PDF input path at
//! `crate::input::pdf_input::PDFInput` (a naive `lopdf`-based per-page
//! text dump) - the two are independent; this module does not modify
//! `pdf_input.rs`. See `crates/calibre_ebooks/src/pdf/reflow.rs`'s module
//! doc comment for the full class-by-class port notes.

pub mod develop;
pub mod html_writer;
pub mod image_writer;
pub mod pdftohtml;
pub mod reflow;
pub mod utils;
