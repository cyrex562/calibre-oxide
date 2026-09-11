//! Port of `old_src/src/calibre/web/fetch/` (issue #83).
//!
//! [`utils`] (`utils.py`) is a real port -- see its own doc.
//!
//! `simple.py`'s `RecursiveFetcher` (a ~500-line full recursive web
//! crawler: link rewriting, image downloading, HTML rewriting, depth
//! limits) was large enough to need its own scoping/splitting pass
//! (docs/AGENT_PORTING_GUIDE.md §5a, issue #455 epic -> #630-#633).
//! [`simple`] covers #630 (the fetch primitive + link filtering);
//! [`get_soup`] covers #631 (HTML preprocessing + tag-selector
//! matching); [`media`] covers #632 (image/stylesheet download +
//! rewrite); the recursive crawl engine itself (#633) isn't here yet
//! -- see each module's own doc.

pub mod get_soup;
pub mod media;
pub mod simple;
pub mod utils;
