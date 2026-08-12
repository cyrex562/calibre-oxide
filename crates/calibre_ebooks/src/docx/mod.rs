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
//!
//! Still to come, tracked separately: `styles.py`, `numbering.py`,
//! `tables.py`, `fonts.py`, `images.py`, `fields.py`, `index.py`,
//! `toc.py` and `cleanup.py`, plus a real `to_html.py`. Those build and
//! mutate an HTML element tree, which this crate does not yet have.
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
pub mod footnotes;
pub mod lcid;
pub mod names;
pub mod settings;
pub mod theme;
pub mod to_html;
pub mod writer;

pub use block_styles::{Border, Borders, Css, Edge, Frame, ParagraphStyle};
pub use char_styles::RunStyle;
pub use container::{Docx, Relationships};
pub use error::DocxError;
pub use footnotes::{Footnotes, Note};
pub use names::DocxNamespace;
pub use settings::Settings;
pub use theme::Theme;
