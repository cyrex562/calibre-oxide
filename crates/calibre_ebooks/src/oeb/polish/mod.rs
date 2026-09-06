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
//! `cover.py`/`create.py`/`split.py` (issue #166, follow-up to #39/#162/
//! #163, and to `replace.py`'s `LinkRebaser`/`get_recommended_folders`):
//! EPUB cover detection/setting (`cover.rs`), empty-book creation
//! (`create.rs`), and real HTML split/merge (`split.rs`). The
//! AZW3-specific functions in `cover.py`/`create.py` (`set_azw3_cover`,
//! `get_azw3_raster_cover_name`, `mark_as_cover_azw3`, and `create_book`'s
//! `fmt == "azw3"` path) are `todo!()`, referencing the same
//! `Azw3Container`/`opf_to_azw3` gap `container.rs` (issue #161) already
//! documents -- `create_book`'s `docx` path looks like it needs the same
//! treatment but doesn't: `docx::writer::container::DocxWriter::new`
//! already starts from an empty document, so it's real, not stubbed.
//!
//! `check/` (issue #40, follow-up to #39/#161-#166): the "Check Book"
//! validator -- structural/XML/link/OPF/id/naming checks over an EPUB,
//! most with auto-fix support. See [`check`]'s module docs for its own
//! scope notes (its `base.py` port's one-struct-plus-closure design, and
//! its two genuine external-capability gaps: `check/css.py`'s stylelint/
//! QWebEngine linting step, and `check/images.py`'s Qt-based CMYK
//! auto-fix).
//!
//! The remaining feature files under
//! `old_src/src/calibre/ebooks/oeb/polish/` (Kobo conversion,
//! text-to-speech, jacket/report generation, ...) are out of scope for
//! this slice and are tracked as follow-up work.

pub mod cascade;
pub mod check;
pub mod container;
pub mod cover;
pub mod create;
pub mod css;
pub mod download;
pub mod embed;
pub mod errors;
pub mod fonts;
pub mod hyphenation;
pub mod images;
pub mod import_book;
pub mod jacket;
pub mod opf;
pub mod parsing;
pub mod pretty;
pub mod replace;
pub mod smartypants;
pub mod spell;
pub mod split;
pub mod stats;
pub mod style;
pub mod subset;
pub mod toc;
pub mod upgrade;
pub mod utils;
