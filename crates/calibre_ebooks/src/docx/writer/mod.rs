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
//! | `links.py` (partial) | [`links`] |
//! | `from_html.py` (partial) | [`from_html`] |
//! | `lists.py` | [`lists`] |
//! | `images.py` (partial) | [`images`] |
//! | `tables.py` (partial) | [`tables`] |
//!
//! Still to come, tracked as issue #132: `tables.py`'s `Cell`/`Row`/
//! `Table` themselves (see [`tables`]'s module docs -- only the
//! border/width foundation is ported so far), `images.py`'s
//! `ImagesManager` itself (see [`images`]'s module docs -- only its
//! self-contained utility layer is ported so far), `from_html.py`'s
//! `Convert.__call__`/`.write` and `Blocks`'
//! `Table`-related methods/`.serialize` (see [`from_html`]'s module
//! docs -- `Convert`'s core element walker itself is ported), plus
//! `styles.py`'s `CombinedStyle`/
//! `StylesManager.finalize`/`.serialize` and `links.py`'s
//! TOC-serialization half (`LinksManager.process_toc_node`/
//! `.process_toc_links`/`.serialize_toc` -- see [`links`]'s module
//! docs) -- all of which need those still-unported `from_html.py`
//! types. These walk a resolved-CSS HTML tree -- #132's own "needs a
//! real OEB stylizer" framing was stale by the time it was filed:
//! [`crate::oeb::polish::cascade`] already had a real CSS cascade (a
//! different consumer, issue #164), and
//! [`crate::oeb::polish::style`] is the accessor seam these files
//! actually need. [`styles`] is built against it: `TextStyle`/
//! `BlockStyle`/`FloatSpec`/`DescendantTextStyle` (CSS -> `w:rPr`/
//! `w:pPr`/`w:framePr`, data model and serialization) plus
//! `StylesManager`'s `create_text_style`/`create_block_style` dedup
//! cache are all ported; `StylesManager.finalize` (block/run-style
//! pairing, heading promotion, descendant-style dedup) and
//! `.serialize` (writing the final `w:styles` part) still need real
//! `Block`/`Run` objects from `from_html.py`.
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
pub mod from_html;
pub mod images;
pub mod links;
pub mod lists;
pub mod styles;
pub mod tables;
pub mod utils;
pub mod xml;

pub use container::{
    create_skeleton, DocumentRelationships, DocxWriter, Margins, PageOptions, Skeleton,
};
pub use fonts::{obfuscate_font_data, FontFace, FontsManager, Slot};
pub use from_html::{
    lang_for_tag, process_item, process_tag, Block, BlockId, Blocks, LinkTarget, ProcessCtx,
    ProcessState, TextRun,
};
pub use images::{create_docx_image_markup, create_filename, get_image_margins, ImageMargins};
pub use links::{LinksManager, TocItem};
pub use lists::ListsManager;
pub use styles::{BlockStyleId, StylesManager, TextStyleId};
pub use tables::{
    as_percent, border_style_weight, convert_width, read_css_block_borders, table_background_color,
    Border, EdgeBorders,
};
pub use utils::{convert_color, int_or_zero};
pub use xml::Element;
