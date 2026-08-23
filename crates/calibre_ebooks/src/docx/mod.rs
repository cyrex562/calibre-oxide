//! DOCX (WordprocessingML) reading.
//!
//! Port of `old_src/src/calibre/ebooks/docx/`. The Python module has
//! two halves: one reads the package and resolves Word's formatting
//! model into CSS, the other walks the document body building an HTML
//! tree. This is the first half.
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` (`InvalidDOCX`) | [`error`] |
//! | `names.py` | [`names`] |
//! | `lcid.py` | [`lcid`] |
//! | `container.py` | [`container`] |
//! | `block_styles.py` | [`block_styles`] |
//! | `char_styles.py` | [`char_styles`] |
//! | `theme.py` | [`theme`] |
//! | `settings.py` | [`settings`] |
//! | `footnotes.py` | [`footnotes`] |
//! | `dump.py` | [`dump`] |
//! | `numbering.py` (partial) | [`numbering`] |
//! | `tables.py` (partial) | [`tables`] |
//! | `styles.py` (partial) | [`styles`] |
//! | `fonts.py` (partial) | [`fonts`] |
//! | `toc.py` | [`toc`] |
//! | `images.py` (partial) | [`images`] |
//!
//! The four "partial" rows above port everything except HTML-markup
//! construction: `numbering.rs` has `Level`/`NumberingDefinition`/
//! `Numbering`'s full reading half (not `apply_markup`); `tables.rs`
//! has `RowStyle`/`CellStyle`/`TableStyle` *and* the full `Table`/
//! `Tables` row/cell/paragraph style resolution and merged-cell
//! bookkeeping (not `apply_markup`) -- merged-cell removal is a
//! tracked exclusion set rather than source-tree mutation, see
//! `tables`'s module docs; `styles.rs` has `PageProperties`/`Style`
//! *and* the full `Styles` paragraph/run cascade orchestrator (not
//! `Styles::cascade` itself, nor `generate_css`) -- see `styles`'s
//! module docs for why those two, specifically, are still blocked
//! (an `is-link`/`layers` concept only `to_html.rs` can produce, and
//! `fonts.py`'s system font matching, respectively); `fonts.rs` has
//! `is_symbol_font`/`map_symbol_text` (pure glyph-table lookups, no
//! system dependency) but not the `Fonts` class itself (embedded-font
//! extraction, `family_for`'s system-installed-font matching), which
//! needs a font scanner with no Rust counterpart.
//! `to_html.py`'s own orchestration, and the remaining unported files
//! (`fields.py`, `index.py`, `cleanup.py`) are tracked separately in
//! issue #130 -- they build and mutate an HTML element tree
//! ([`crate::dom`] as of that issue), and (for `fields.py`/`index.py`'s
//! synthetic-element insertion) a mutable *source*-document tree
//! ([`crate::xmltree`], relocated for this reason but not yet wired
//! into the docx module). `toc.py` is ported ([`toc`]) but, like
//! everything above, not yet wired into a `to_html.py` orchestrator --
//! see issue #288. `images.py`'s pure geometry/CSS half is ported
//! ([`images`], issue #289); the `Images` struct itself (embedded-image
//! extraction, resizing, `w:drawing`/`w:pict` -> `<img>` markup) is
//! still a separate follow-up.
//!
//! The output half of the module — `writer/` — is issue #23 and lives
//! in [`writer`].
//!
//! [`to_html`] is **not** part of that port: it is the pre-existing
//! sketch — paragraphs, runs, and images, no style resolution at all —
//! left in place so the DOCX input plugin keeps working until the real
//! conversion lands. Its module docs say so too.
//!
//! ```no_run
//! use calibre_ebooks::docx::container::Docx;
//!
//! let mut docx = Docx::open("book.docx")?;
//! let mi = docx.metadata();
//! println!("{} by {}", mi.title, mi.authors.join(" & "));
//! # Ok::<(), calibre_ebooks::docx::error::DocxError>(())
//! ```

pub mod block_styles;
pub mod char_styles;
pub mod container;
pub mod dump;
pub mod error;
pub mod fonts;
pub mod footnotes;
pub mod images;
pub mod lcid;
pub mod names;
pub mod numbering;
pub mod settings;
pub mod styles;
pub mod tables;
pub mod theme;
pub mod to_html;
pub mod toc;
pub mod writer;

pub use block_styles::{Border, Borders, Css, Edge, Frame, ParagraphStyle};
pub use char_styles::RunStyle;
pub use container::{Docx, Relationships};
pub use error::DocxError;
pub use fonts::{is_symbol_font, map_symbol_text};
pub use footnotes::{Footnotes, Note};
pub use names::DocxNamespace;
pub use numbering::{Level, Numbering, NumberingDefinition};
pub use settings::Settings;
pub use styles::{PageProperties, Style, Styles};
pub use tables::{CellStyle, RowStyle, TableStyle};
pub use theme::Theme;
