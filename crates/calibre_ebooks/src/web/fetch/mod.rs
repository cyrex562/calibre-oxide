//! Port of `old_src/src/calibre/web/fetch/` (issue #83).
//!
//! [`utils`] (`utils.py`) is a real port -- see its own doc.
//!
//! `simple.py`'s `RecursiveFetcher` (a ~500-line full recursive web
//! crawler: link rewriting, image downloading, HTML rewriting, depth
//! limits) was large enough to need its own scoping/splitting pass
//! (docs/AGENT_PORTING_GUIDE.md §5a, issue #455 epic -> #630-#633).
//! [`simple`] covers #630 (the fetch primitive + link filtering); the
//! rest of the crawler isn't here yet -- see [`simple`]'s own doc.

pub mod simple;
pub mod utils;
