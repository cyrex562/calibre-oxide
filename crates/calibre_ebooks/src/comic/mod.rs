//! Comic-book format support (CBZ/CBR/CBT/CB7).
//!
//! Port of `old_src/src/calibre/ebooks/comic/`. Python's
//! `comic/input.py` mixed three responsibilities:
//!
//! 1. Archive extraction + filename sanitization (`extract_comic`).
//! 2. Page enumeration + natural sort (`find_pages`).
//! 3. Qt-based image rendering pipeline (`PageProcessor`,
//!    `render_pages`, `process_pages`).
//!
//! All three are real here: (1)/(2) in [`input`], (3) in
//! [`page_processor`] (issue #124) -- a rewrite off Qt onto this
//! workspace's own `image` crate plus the already-real
//! `calibre_utils::imageops`/`quantize` (issues #569-#571).

pub mod input;
pub mod page_processor;

pub use input::{comic_exts, extract_comic, find_pages, is_comic_page, numeric_sort_key};
pub use page_processor::{process_pages, render_page, render_pages, ComicPageOptions, PageTask, RenderOutcome};
