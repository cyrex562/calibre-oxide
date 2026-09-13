//! Port of `calibre.utils.fonts.sfnt.cff` (issue #554, split from #65),
//! the CFF (Compact Format) table reader/writer used by OpenType/
//! PostScript-flavored (`.otf`) fonts. Fully real as of issue #565:
//! [`writer::Subset`] plus [`super::subset::subset_cff`] wire
//! CFF-flavored fonts into `sfnt::subset::subset` alongside the
//! TrueType path (#553).
//!
//! Split further (comment on #554 has the full rationale): #563
//! (`constants`/`dict_data`, the DICT byte-code codec foundation),
//! #564 (`table.py`'s `Index`/`Strings`/`Charset`/`CFF` table reader,
//! depends on #563), #565 (`writer.py`'s real subsetting writer +
//! wiring into `sfnt::subset`'s CFF branch, depends on #564) -- all
//! closed.

pub mod constants;
pub mod dict_data;
pub mod table;
pub mod writer;
