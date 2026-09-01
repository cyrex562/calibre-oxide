//! Port of `old_src/src/calibre/web/fetch/` (issue #83).
//!
//! [`utils`] (`utils.py`) is a real port -- see its own doc.
//!
//! `simple.py`'s `RecursiveFetcher` (a ~500-line full recursive web
//! crawler: link rewriting, image downloading, HTML rewriting, depth
//! limits) is not ported here -- it's large enough to need its own
//! scoping/splitting pass (docs/AGENT_PORTING_GUIDE.md §5a) rather
//! than being folded into this file's real, narrow deliverable.

pub mod utils;
