//! Writing DOCX files.
//!
//! Port of `old_src/src/calibre/ebooks/docx/writer/` — the output half
//! of calibre's DOCX support, which turns a converted book into a
//! WordprocessingML package.
//!
//! | Python | Rust |
//! | --- | --- |
//! | `utils.py` (+ `tinycss.color3`) | [`utils`] |
//! | `container.py` | [`container`] |
//! | `fonts.py` | [`fonts`] |
//! | — (`lxml.builder`) | [`xml`] |
//! | `styles.py` (partial) | [`styles`] |
//!
//! Still to come, tracked as issue #132: the rest of `styles.py`
//! (`BlockStyle`/`FloatSpec`/`DescendantTextStyle`/`StylesManager`),
//! `from_html.py`, `tables.py`, `images.py`, `links.py`, `lists.py`.
//! These walk a resolved-CSS HTML tree -- #132's own "needs a real OEB
//! stylizer" framing was stale by the time it was filed:
//! [`crate::oeb::polish::cascade`] already had a real CSS cascade (a
//! different consumer, issue #164), and
//! [`crate::oeb::polish::style`] is the accessor seam these files
//! actually need. [`styles`] is the first piece built against it:
//! `TextStyle`, the CSS -> `w:rPr` run-property data model (not yet
//! its serialization or `StylesManager`'s deduplication pass).
//!
//! ```no_run
//! use calibre_ebooks::docx::writer::container::{DocxWriter, PageOptions};
//! use calibre_ebooks::metadata::meta::MetaInformation;
//!
//! let writer = DocxWriter::new(PageOptions::default());
//! let file = std::fs::File::create("out.docx")?;
//! writer.write(file, &MetaInformation::default())?;
//! # Ok::<(), calibre_ebooks::docx::error::DocxError>(())
//! ```

pub mod container;
pub mod fonts;
pub mod styles;
pub mod utils;
pub mod xml;

pub use container::{
    create_skeleton, DocumentRelationships, DocxWriter, Margins, PageOptions, Skeleton,
};
pub use fonts::{obfuscate_font_data, FontFace, FontsManager, Slot};
pub use utils::{convert_color, int_or_zero};
pub use xml::Element;
