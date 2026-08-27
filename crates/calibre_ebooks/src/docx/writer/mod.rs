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
//! | `styles.py` | [`styles`] |
//! | `links.py` | [`links`] |
//! | `from_html.py` (partial) | [`from_html`] |
//! | `lists.py` | [`lists`] |
//! | `images.py` (partial) | [`images`] |
//! | `tables.py` (partial) | [`tables`] |
//!
//! Still to come, tracked as issue #132: every `.serialize` method
//! across `tables.py`/`from_html.py` (`Cell`/`Row`/`Table`/`Blocks`)
//! -- `tables.py`'s own side of its HTML-walk integration is now
//! FULLY ported (see [`tables`]'s module docs), and `from_html.py`'s
//! `Blocks` now has real table support too: its own `tables` stack of
//! currently-open tables, `finish_tag`'s table-closing branch (moving
//! a finished table into its parent -- nesting it into a still-open
//! outer table, or appending it to `Blocks`' own top-level `items`),
//! a real `items: Vec<ItemId>` holding both `Block`s and finished
//! `Table`s, and [`from_html::Block`] gained a real
//! [`from_html::ItemContainer`] field (Python's `parent_items`,
//! finally meaningful now that a block can live in either `Blocks`'
//! own items or a table cell's). `images.py`'s
//! `create_image_markup`/`add_image`/`serialize`/cover-image methods
//! (see [`images`]'s module docs -- its self-contained utility layer
//! AND [`images::ImagesManager`]'s data-source half, `read_image`/
//! `read_svg`, are now ported: the real image-content-source design
//! question resolved to an existing crate-wide idiom,
//! `OEBBook.container.read(href)`, no new abstraction needed), and
//! `from_html.py`'s `Convert.__call__`/`.write` (see [`from_html`]'s
//! module docs -- `Convert`'s core element walker AND `Blocks`'
//! `Table`-related methods are both ported now; only the top-level
//! orchestration itself remains) --
//! `links.py`'s `LinksManager` is now fully ported, including its
//! TOC-serialization half, which is what gave [`xml::Element`] its
//! `insert`/`find_descendant_mut` methods. These walk a resolved-CSS
//! HTML tree -- #132's own "needs a real OEB stylizer" framing was
//! stale by the time it was filed:
//! [`crate::oeb::polish::cascade`] already had a real CSS cascade (a
//! different consumer, issue #164), and
//! [`crate::oeb::polish::style`] is the accessor seam these files
//! actually need. [`styles`] is now FULLY ported: `TextStyle`/
//! `BlockStyle`/`FloatSpec`/`DescendantTextStyle` (CSS -> `w:rPr`/
//! `w:pPr`/`w:framePr`, data model and serialization),
//! `StylesManager`'s `create_text_style`/`create_block_style` dedup
//! cache, `StylesManager::finalize` (block/run-style pairing, heading
//! promotion, descendant-style dedup, writing back onto `from_html`'s
//! `Block`/`TextRun`), and `StylesManager::serialize` (writing every
//! combined/descendant/pure-block style into a real `<w:styles>`
//! element). `links.py`/`lists.py`/`styles.py` are the three files of
//! the original six this issue tracks that are now closed out
//! completely.
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
    lang_for_tag, process_item, process_tag, Block, BlockId, Blocks, ItemContainer, ItemId,
    LinkTarget, ProcessCtx, ProcessState, TextRun,
};
pub use images::{
    create_docx_image_markup, create_filename, get_image_margins, Image, ImageMargins,
    ImagesManager,
};
pub use links::{LinksManager, TocItem};
pub use lists::ListsManager;
pub use styles::{BlockStyleId, CombinedStyle, StylesManager, TextStyleId};
pub use tables::{
    as_percent, border_style_weight, convert_width, read_css_block_borders, table_background_color,
    Border, Cell, CellId, CellSlot, EdgeBorders, ResolvedBorders, Row, RowId, SpannedCell, Table,
    TableId, Tables,
};
pub use utils::{convert_color, int_or_zero};
pub use xml::Element;
