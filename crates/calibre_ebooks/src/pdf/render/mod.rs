//! Port of `old_src/src/calibre/ebooks/pdf/render/` (issue #46: `common.py`,
//! `fonts.py`, `gradients.py`, `graphics.py`, `links.py`, `serialize.py`;
//! 1,809 lines total).
//!
//! Calibre's own hand-rolled PDF *writer*/serializer - distinct from
//! `crate::pdf::reflow` (a PDF *reader*/layout-reconstructor, issue #45)
//! and from `crate::input::pdf_input` (a separate, much simpler `lopdf`-
//! based PDF text extractor). It works by having Qt's WebEngine render an
//! HTML page through a custom `QPaintEngine` subclass (`graphics.py`'s
//! `Graphics`), which intercepts each Qt paint call (draw path, draw
//! image, set brush/pen, clip, ...) and re-emits it as raw PDF
//! content-stream operators via `serialize.py`'s `PDFStream`.
//!
//! `crate::pdf::image_writer`'s `draw_image_page`/`convert` are
//! `todo!()`'d out specifically because they need this module (see that
//! file's module doc comment) - wiring them up to the code here is
//! out of scope for this port; see this module's own gaps below instead.
//!
//! | Python | Rust | Status |
//! | --- | --- | --- |
//! | `common.py` | [`common`] | Real port: PDF object model + `pdf_float`. |
//! | `links.py` | [`links`] | Real port: back-reference restructured to explicit params (see its doc comment). |
//! | `fonts.py` | [`fonts`] | Real port except `Font`'s glyph-subsetting call (needs the unported `calibre.utils.fonts.sfnt.subset`). |
//! | `gradients.py` | [`gradients`] | Real port: Qt gradient/matrix inputs replaced with plain-data stand-ins. |
//! | `graphics.py` | [`graphics`] | Split: `convert_path`/tiling-pattern/texture-pattern logic real; `Graphics`/`GraphicsState` (live `QPaintEngine` interception) documented gap. |
//! | `serialize.py` | [`serialize`] | Real port, including `add_image`'s alpha-blend-and-JPEG-encode path (via the `image` crate). |

pub mod common;
pub mod fonts;
pub mod gradients;
pub mod graphics;
pub mod links;
pub mod serialize;
