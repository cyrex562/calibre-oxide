//! Port of `calibre.ebooks.oeb.polish` -- the "Polish Book" subsystem.
//!
//! Foundation (issue #39, merged in #161): `errors.py`, `parsing.py`,
//! `utils.py`, `container.py` (`ContainerBase`/`Container`/
//! `EpubContainer` for real; `KEPUBContainer`/`AZW3Container` partially,
//! see `container.rs`'s module docs), `opf.py`.
//!
//! Layer 1 (issue #162, this slice -- each depends only on the
//! foundation above, not on any other `polish/` file): `hyphenation.py`,
//! `pretty.py`, `import_book.py`, `cascade.py`, `embed.py`, `fonts.py`,
//! `subset.py`, `download.py`, `images.py`. `cascade.py`/`embed.py`/
//! `fonts.py`/`subset.py` each have a real CSS-parser-shaped gap (no
//! general CSS selector/declaration parser exists in this crate -- see
//! `cascade.rs`'s module docs); `subset.py`'s actual font subsetting and
//! `images.py`'s actual image recompression are further, distinct gaps
//! documented in each module.
//!
//! `toc.py` (issue #163, follow-up to #39/#162): the TOC editor -- NCX/
//! nav-document parsing, fallback TOC generation from spine headings/
//! links/files, landmarks, and NCX/nav/inline-HTML TOC commit. Depends
//! only on the foundation and `pretty.py` above.
//!
//! The remaining ~13 feature files under
//! `old_src/src/calibre/ebooks/oeb/polish/` (spell-check, cover editing,
//! split/merge, Kobo conversion, text-to-speech, link-checking, ...) are
//! out of scope for this slice and are tracked as follow-up work.

pub mod cascade;
pub mod container;
pub mod download;
pub mod embed;
pub mod errors;
pub mod fonts;
pub mod hyphenation;
pub mod images;
pub mod import_book;
pub mod opf;
pub mod parsing;
pub mod pretty;
pub mod subset;
pub mod toc;
pub mod utils;
pub mod xmltree;
