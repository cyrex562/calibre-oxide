//! The MOBI 6 writer: OEB -> `.mobi` bytes.
//!
//! Port of `calibre.ebooks.mobi.writer2`, i.e. the four files
//! `indexer.py`, `main.py`, `resources.py`, `serializer.py` plus the
//! four constants that used to live in `writer2/__init__.py`.
//!
//! This is the *encoder* side of the format the reader half of this
//! crate (`crate::mobi::mobi6`/`mobi8`, `crate::mobi::index`) already
//! decodes. The wire format (SKEL/SECT/NCX/GUIDE INDX trees, TAGX tags,
//! trailing byte sequences) mirrors `crate::mobi::index`'s decoder --
//! `indexer.rs` here is deliberately read as index.rs's inverse rather
//! than re-derived independently.
//!
//! The joint MOBI6+KF8 (`.azw3`) output path (`main.py`'s `kf8`-present
//! branches) is real (issue #157): `main::MobiWriter::write_joint`
//! interleaves this writer's own text/index records with a sibling
//! `crate::mobi::writer8::mobi::KF8Book`'s, sharing one `Resources`
//! block. No output plugin drives it yet (`output::mobi_output::MOBIOutput`
//! only ever calls the standalone [`main::MobiWriter::write`]; there is
//! no `AZW3Output` plugin in this crate at all) -- that dispatch (real
//! upstream's own old/new/both `mobi_output_format` option) is separate,
//! unstarted scope.

pub mod indexer;
pub mod main;
pub mod resources;
pub mod serializer;

/// PalmDOC record compression disabled. `UNCOMPRESSED` in
/// `mobi/writer2/__init__.py`.
pub const UNCOMPRESSED: u16 = 1;
/// PalmDOC LZ77 compression. `PALMDOC` in `mobi/writer2/__init__.py`.
pub const PALMDOC: u16 = 2;
/// HUFF/CDIC compression -- calibre's writer never produces this, only
/// its reader decodes it. `HUFFDIC` in `mobi/writer2/__init__.py`.
pub const HUFFDIC: u32 = 17480;
/// Largest image calibre will embed unmodified into a MOBI 6 file
/// before it must be rescaled. `PALM_MAX_IMAGE_SIZE` in
/// `mobi/writer2/__init__.py`.
pub const PALM_MAX_IMAGE_SIZE: usize = 63 * 1024;

/// Largest cover thumbnail (bytes). `MAX_THUMB_SIZE` in `mobi/__init__.py`.
pub const MAX_THUMB_SIZE: usize = 16 * 1024;
/// Cover thumbnail target dimensions (w, h). `MAX_THUMB_DIMEN` in
/// `mobi/__init__.py`.
pub const MAX_THUMB_DIMEN: (u32, u32) = (180, 240);
