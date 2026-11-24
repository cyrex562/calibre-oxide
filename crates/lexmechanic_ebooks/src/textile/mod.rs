//! Port of `old_src/src/calibre/ebooks/textile/` -- PyTextile, a
//! Textile-markup-to-HTML converter vendored into calibre.
//!
//! - `functions.rs`: port of `functions.py` (the `Textile` class and
//!   the `textile`/`textile_restricted` free functions).
//! - `unsmarten.rs`: port of `unsmarten.py` (reverses HTML
//!   entities/Unicode typographic characters back into Textile's own
//!   escape notation).
//! - This file: port of `__init__.py`, which re-exports `Textile`,
//!   `textile`, and `textile_restricted` from `functions.py`.

pub mod functions;
pub mod unsmarten;

pub use functions::{textile, textile_restricted, Textile};
pub use unsmarten::unsmarten;
