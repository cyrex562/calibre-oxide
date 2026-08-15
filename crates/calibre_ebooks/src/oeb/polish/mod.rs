//! Port of `calibre.ebooks.oeb.polish` -- the "Polish Book" subsystem.
//!
//! Foundation (issue #39, merged in #161): `errors.py`, `parsing.py`,
//! `utils.py`, `container.py` (`ContainerBase`/`Container`/
//! `EpubContainer` for real; `KEPUBContainer`/`AZW3Container` partially,
//! see `container.rs`'s module docs), `opf.py`.
//!
//! Layer 1 (issue #162): `hyphenation.py`, `pretty.py`, `import_book.py`,
//! `cascade.py`, `embed.py`, `fonts.py`, `subset.py`, `download.py`,
//! `images.py`. `subset.py`'s actual font subsetting and `images.py`'s
//! actual image recompression are distinct, still-open gaps documented
//! in each module; `cascade.py`/`fonts.py`/`subset.py`'s CSS-parsing gaps
//! were closed for real by issue #164 (below).
//!
//! `toc.py` (issue #163, follow-up to #39/#162): the TOC editor -- NCX/
//! nav-document parsing, fallback TOC generation from spine headings/
//! links/files, landmarks, and NCX/nav/inline-HTML TOC commit. Depends
//! only on the foundation and `pretty.py` above.
//!
//! `css.py`/`stats.py` (issue #164): selector-usage tracking, unused-CSS
//! removal, rule merging, class renaming (`css.rs`), and per-language
//! character/font-usage statistics (`stats.rs`). This is also the issue
//! that added [`crate::css`] -- a real CSS tokenizer/object-model/
//! selector-matcher this crate previously did not have -- and used it to
//! retrofit the CSS-parsing-shaped `todo!()`s issue #162 had left in
//! `cascade.rs`/`fonts.rs`/`subset.rs`; see [`crate::css`]'s module docs
//! and each retrofitted function's own docs for what's real now and what
//! (narrower, still-documented) gaps remain.
//!
//! The remaining ~11 feature files under
//! `old_src/src/calibre/ebooks/oeb/polish/` (spell-check, cover editing,
//! split/merge, Kobo conversion, text-to-speech, link-checking, ...) are
//! out of scope for this slice and are tracked as follow-up work.

pub mod cascade;
pub mod container;
pub mod css;
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
pub mod replace;
pub mod smartypants;
pub mod spell;
pub mod stats;
pub mod subset;
pub mod toc;
pub mod upgrade;
pub mod utils;
pub mod xmltree;
