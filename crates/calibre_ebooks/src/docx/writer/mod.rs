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
//! | `from_html.py` | [`from_html`] |
//! | `lists.py` | [`lists`] |
//! | `images.py` | [`images`] |
//! | `tables.py` | [`tables`] |
//!
//! **All six files issue #132 originally tracked (`links.py`/
//! `lists.py`/`styles.py`/`tables.py`/`images.py`/`from_html.py`) are
//! now FULLY ported.** [`from_html::convert`] (port of
//! `Convert.__init__` + `Convert.__call__`) is the real, recommended
//! entry point: give it a real `OEBBook`, and it runs the full
//! multi-item spine walk (per-item CSS cascade resolution reused from
//! [`crate::oeb::transforms::flatcss::resolve_document_styles`], since
//! [`crate::oeb::polish::cascade::resolve_styles`] needs a DIFFERENT,
//! filesystem-backed object model this crate's plain `OEBBook` isn't),
//! the pre-`write()` cleanup pass (`resolve_skipped`/`delete_block_at`/
//! `apply_page_break_after`/`resolve_language`/`styles_manager.finalize`/
//! `lists_manager.finalize`, the latter two correctly scoped PER SPINE
//! ITEM -- see [`lists::ListsManager::finalize`]'s own docs for why a
//! single shared call across the whole book would misresolve),
//! cover-image resolution (`oeb.metadata`'s `"cover"` term ->
//! [`images::ImagesManager::read_image`]/`::create_cover_markup`), and
//! [`from_html::write`] (`Convert.write`, the final assembly into a
//! real [`container::DocxWriter`], including
//! [`images::write_cover_block`] when there's a cover). A real
//! correctness question surfaced and resolved along the way: Python
//! shares ONE `document_relationships` object by reference across
//! every manager, but this port's managers each own a `Clone` --
//! [`from_html::convert`]'s own doc comment explains exactly how
//! relationship-id collisions between `LinksManager` and
//! `ImagesManager` are avoided without `Rc<RefCell<...>>`.
//!
//! **Deliberately NOT ported, tracked as the remainder of issue
//! #132**: SVG rasterization (this crate's
//! [`crate::oeb::transforms::rasterize::SvgRasterizer`] only has the
//! rasterization-cache half, issue #162, ported for a different
//! consumer) and font embedding (`fonts::FontsManager::serialize` is
//! itself fully ported, but needs used-family extraction from
//! `styles::StylesManager`'s interned styles plus manifest font-face
//! discovery, neither of which is wired up). Neither blocks producing
//! a real, valid `.docx` -- a document converted today just won't
//! rasterize SVGs or embed fonts yet.
//!
//! `from_html.py`'s `Blocks` has real table support: its own `tables`
//! stack of currently-open tables, `finish_tag`'s table-closing branch
//! (moving a finished table into its parent -- nesting it into a
//! still-open outer table, or appending it to `Blocks`' own top-level
//! `items`), a real `items: Vec<ItemId>` holding both `Block`s and
//! finished `Table`s, and [`from_html::Block`] gained a real
//! [`from_html::ItemContainer`] field (Python's `parent_items`,
//! finally meaningful now that a block can live in either `Blocks`'
//! own items or a table cell's). `Blocks::serialize` (see [`tables`]'s
//! module docs -- `tables.py` itself is FULLY ported, including every
//! `.serialize` method; `Tables::serialize_cell`/`::serialize_row`/
//! `::serialize_table` take a `serialize_block` callback for the one
//! piece they can't resolve on their own, a real `Block`'s content,
//! and `Blocks::serialize` is what builds that closure) walks
//! `self.items` and writes real content out. `add_block_tag`/
//! `add_inline_tag`'s `<img>`-tag handling is wired up too -- via
//! [`ProcessCtx`]'s `images_manager`/`names` fields (a second lifetime
//! parameter, `ProcessCtx<'a, 'b>`, since `ImagesManager<'b>`'s own
//! `&'b OEBBook` borrow is unrelated to `ProcessCtx`'s other `'a`
//! borrows). `links.py`'s `LinksManager` is fully ported, including
//! its TOC-serialization half, which is what gave [`xml::Element`] its
//! `insert`/`find_descendant_mut` methods -- #132's own "needs a real
//! OEB stylizer" framing was stale by the time it was filed:
//! [`crate::oeb::polish::cascade`] already had a real CSS cascade (a
//! different consumer, issue #164), and [`crate::oeb::polish::style`]
//! is the accessor seam these files actually need. [`styles`] is
//! FULLY ported: `TextStyle`/`BlockStyle`/`FloatSpec`/
//! `DescendantTextStyle` (CSS -> `w:rPr`/`w:pPr`/`w:framePr`, data
//! model and serialization), `StylesManager`'s `create_text_style`/
//! `create_block_style` dedup cache, `StylesManager::finalize`, and
//! `StylesManager::serialize`.
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
    convert, lang_for_tag, process_item, process_tag, write, Block, BlockId, Blocks, ItemContainer,
    ItemId, LinkTarget, ProcessCtx, ProcessState, TextRun,
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
