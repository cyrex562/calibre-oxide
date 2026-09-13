//! Port of `calibre.utils.fonts.sfnt.cff` (issue #554, split from #65),
//! the CFF (Compact Font Format) table reader/writer used by OpenType/
//! PostScript-flavored (`.otf`) fonts -- `sfnt::subset`'s own module
//! doc already discloses that CFF-flavored fonts currently report as
//! `UnsupportedFont` pending this cluster.
//!
//! Split further (comment on #554 has the full rationale): #563 (this
//! module so far -- `constants`/`dict_data`, the DICT byte-code codec
//! foundation), #564 (`table.py`'s `Index`/`Strings`/`Charset`/`CFF`
//! table reader, depends on #563), #565 (`writer.py`'s real subsetting
//! writer + wiring into `sfnt::subset`'s CFF branch, depends on #564).

pub mod constants;
pub mod dict_data;
pub mod table;
pub mod writer;
