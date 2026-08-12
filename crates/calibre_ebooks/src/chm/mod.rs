//! CHM (Compiled HTML Help) format support.
//!
//! Port of `old_src/src/calibre/ebooks/chm/`. Python delegates to
//! `pychm` (a Python binding to the C `chmlib`); we delegate to
//! `libchm`, a pure-Rust CHM reader — no C dependency.
//!
//! Coverage matches the Python module's public surface: file
//! extraction, home-topic resolution, title extraction. The full
//! HHC-based TOC parser (`CHMReader._parse_toc`) is deferred — it
//! depends on an HTML tree walker that will land with the
//! html5ever DOM integration (issue #121).

pub mod reader;

pub use reader::{ChmError, ChmReader, ChmSystemInfo};
