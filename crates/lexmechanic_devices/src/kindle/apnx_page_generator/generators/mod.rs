//! Concrete `IPageGenerator` implementations.
//!
//! - `FastPageGenerator`   — 2300-chars-per-page estimate.
//! - `ExactPageGenerator`  — divide text length by user-supplied real_count.
//! - `AccuratePageGenerator` — parse the HTML, count lines, 32 lines/page.
//! - `PagebreakPageGenerator` — anchor to `<*pagebreak*/>` markers.
//!
//! Each generator's `_generate` maps to a pure function that operates
//! on already-loaded data, so the algorithms are testable without a
//! real MOBI file.

pub mod accurate;
pub mod exact;
pub mod fast;
pub mod pagebreak;

pub use accurate::AccuratePageGenerator;
pub use exact::ExactPageGenerator;
pub use fast::FastPageGenerator;
pub use pagebreak::PagebreakPageGenerator;
