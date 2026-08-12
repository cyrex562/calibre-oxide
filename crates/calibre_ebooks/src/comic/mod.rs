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
//! Rust ports (1) and (2) here — they're the semantic core the input
//! plugin (`crate::input::comic_input`) needs. (3) is a substantial
//! rewrite off Qt onto the `image` / `imageproc` crates and lands as
//! a dedicated follow-up issue.

pub mod input;

pub use input::{comic_exts, extract_comic, find_pages, is_comic_page, numeric_sort_key};
