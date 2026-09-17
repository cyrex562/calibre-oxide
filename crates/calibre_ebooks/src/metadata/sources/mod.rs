//! Real online metadata-source clients (issue #750 epic).
//!
//! Real upstream calibre's own `Source` plugin architecture
//! (`old_src/src/calibre/ebooks/metadata/sources/`) fans out a search
//! across several installed sources and lets the user review/merge the
//! candidate results per field. This port takes a narrower, disclosed
//! first slice, per an explicit user architecture decision (2026-09-17,
//! issue #750): only Google Books ([`google_books`]) and Open Library
//! ([`open_library`]) -- both real, official, free, no-key public APIs,
//! no scraping. Goodreads was considered and excluded: its public API
//! was discontinued in December 2020 and its current ToS prohibits
//! scraping -- matching real upstream calibre's own choice (no
//! `goodreads.py` source plugin exists there either).
//!
//! There is no `Source` trait here -- with only two source clients,
//! each with its own query-precedence rules and a different partial set
//! of fields it can populate, a shared trait would either force an
//! artificial lowest-common-denominator query shape or need an
//! associated-type escape hatch with no real second use yet. Each
//! source module exposes its own `search`/`search_at` functions
//! returning `Vec<MetadataCandidate>` directly; a later sub-issue
//! (calibre_srv's search route) calls both concurrently and
//! concatenates the results.

pub mod google_books;
pub mod open_library;

use std::collections::BTreeMap;

/// One candidate metadata result from a real online source, already
/// normalized to a common shape so a caller (the eventual srv route,
/// the eventual per-field-picker UI) doesn't need to know which source
/// produced it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MetadataCandidate {
    /// Human-readable source name, e.g. `"Google Books"` -- shown to
    /// the user so they know where a candidate's fields came from.
    pub source: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    /// Long-form description/comments (Google Books' `description`,
    /// upstream calibre's `comments`).
    pub description: Option<String>,
    pub publisher: Option<String>,
    /// Free-form published-date string as the source returned it (e.g.
    /// `"2011"` or `"2011-08-01"`) -- not parsed/normalized here, same
    /// as upstream's own `Metadata.pubdate` handling for partial dates.
    pub pubdate: Option<String>,
    pub tags: Vec<String>,
    /// Lowercase identifier-type -> value, e.g. `"isbn" -> "9780441013593"`.
    pub identifiers: BTreeMap<String, String>,
    /// BCP-47-ish language code as the source returned it (e.g. `"en"`).
    pub language: Option<String>,
    /// URL of a cover image, if the source has one. Fetching the actual
    /// bytes is the srv sub-issue's cover-proxy route's job, not this
    /// module's.
    pub cover_url: Option<String>,
    pub rating: Option<f64>,
}
