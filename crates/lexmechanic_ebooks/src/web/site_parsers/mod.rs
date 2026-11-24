//! Port of `old_src/src/calibre/web/site_parsers/` (issue #84):
//! per-site article-HTML extractors for the news-recipe pipeline,
//! each pulling structured content out of a page's embedded JSON
//! blob rather than scraping rendered markup.

pub mod natgeo;
pub mod nytimes;
