//! The ebook-viewer's iteration support.
//!
//! Port of `old_src/src/calibre/ebooks/oeb/iterator/` -- extracting any
//! supported book format into an exploded OEB directory, building a
//! paginated spine with anchor/link indexing, and reading/writing
//! bookmarks stored inside the book file itself.

pub mod book;
pub mod bookmarks;
pub mod spine;
