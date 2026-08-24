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
//! | `images.py` | [`images`] |
//! | `fields.py` (partial) | [`fields`] |
//! | `index.py` (partial) | [`index`] |
//! | `cleanup.py` | [`cleanup`] |
//! | `to_html.py`'s `Convert.read_styles` (partial) | [`read_styles`] |
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
//! `to_html.py`'s own orchestration is tracked separately in issue
//! #130 -- it needs the real HTML element tree ([`crate::dom`] as of
//! that issue) fully wired into a `Convert`-equivalent orchestrator.
//! `toc.py` and `images.py` are both fully ported ([`toc`]/[`images`],
//! issue #289 for the latter -- geometry/CSS, real embedded-image
//! extraction/resizing, and the `w:drawing`/`w:pict` -> `<img>` markup
//! generators are all done) but, like everything below, not yet wired
//! into an orchestrator -- see issue #288. `fields.py`'s pure
//! field-instruction parsing half is ported ([`fields`], issue #290);
//! the `Fields` orchestrator itself (the source-tree field scanner,
//! plus `parse_xe`'s synthetic-bookmark insertion) is a separate
//! follow-up -- see `fields`'s module docs. `index.py`'s
//! `polish_index_markup` half (the HTML-DOM-mutating block-merge
//! algorithm) is ported ([`index`], issue #293); its `make_block`/
//! `add_xe`/`process_index` (source-tree-mutating) half is not,
//! needing the same `crate::xmltree`-vs-side-table decision
//! `fields.rs`'s `parse_xe` is blocked on -- see `index`'s module
//! docs. `cleanup.py` is fully ported ([`cleanup`], issue #291) --
//! the last of #130's originally-unported files with no remaining
//! blocker of its own. `to_html.rs`'s [`to_html::convert_document`]
//! (issue #288) wires the whole `to_html.rs` orchestration together
//! in `Convert.__call__`'s real order, given an already-populated
//! `Styles`/`Numbering`/`Footnotes`/`Theme`/`Settings` --
//! [`read_styles`] does exactly that populating (`Convert.read_styles`,
//! split into a real-I/O half and a wiring half; see its own module
//! docs for why). What's left for #288: `resolve_alternate_content`
//! (needs the same source-tree-mutation decision as #290/#293),
//! `fields.py`'s orchestrator (#290), images wired into `convert_run`,
//! and `write`'s OPF/NCX output (`mobi/opf_writer.rs` already has the
//! low-level writers, needs generalizing out of `mobi/`).
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
pub mod cleanup;
pub mod container;
pub mod dump;
pub mod error;
pub mod fields;
pub mod fonts;
pub mod footnotes;
pub mod images;
pub mod index;
pub mod lcid;
pub mod names;
pub mod numbering;
pub mod read_styles;
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
