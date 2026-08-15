//! Port of `calibre.ebooks.oeb.polish` -- the "Polish Book" subsystem's
//! foundational layer.
//!
//! Ported here (issue #39, foundation slice): `errors.py`, `parsing.py`,
//! `utils.py`, `container.py` (`ContainerBase`/`Container`/
//! `EpubContainer` for real; `KEPUBContainer`/`AZW3Container` partially,
//! see `container.rs`'s module docs), `opf.py`. The other ~23 feature
//! files under `old_src/src/calibre/ebooks/oeb/polish/` (font
//! subsetting, spell-check, cover editing, split/merge, TOC editing,
//! Kobo conversion, text-to-speech, link-checking, ...) are out of scope
//! for this slice and are tracked as follow-up work.

pub mod container;
pub mod errors;
pub mod opf;
pub mod parsing;
pub mod utils;
pub mod xmltree;
