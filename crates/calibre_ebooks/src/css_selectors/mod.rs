//! Port of the top-level `css_selectors` package (issue #85, split per
//! `docs/AGENT_PORTING_GUIDE.md` §5a into #451 `errors.py` +
//! `ordered_set.py`, #452 `parser.py`, #453 `select.py`): a real CSS
//! Selectors Level 3 parser + tree-matching engine, ~2500 lines across
//! 5 files (`tests.py` is upstream's own test suite, not ported
//! line-by-line -- each sub-issue writes real Rust tests covering the
//! same grammar/matching surface instead).
//!
//! # Not a replacement for `crate::css::selector`/`crate::css::matcher`
//!
//! This crate already has a real, working, deliberately **narrower**
//! CSS selector engine at [`crate::css::selector`]/[`crate::css::matcher`],
//! built for issue #164 (`css.py`/`cascade.py`'s actual need:
//! `Select(root).has_matches(selector_text)` plus specificity
//! ordering). That module's own doc explains this is a deliberate
//! substitute, not a stopgap -- it doesn't support sibling combinators,
//! structural pseudo-classes (`:nth-child()`, etc.), `:lang()`, or a
//! tree-agnostic `Element` trait usable beyond [`crate::dom::Dom`]/
//! [`crate::xmltree::Xml`]. This module is the FULL grammar upstream's
//! own `css_selectors` package implements, built as its own standalone
//! port (mirroring the real Python package's own module boundaries)
//! rather than widening `css::selector` in place -- the two serve
//! different real callers (`cascade.rs`'s narrower need vs. any future
//! caller needing the fuller CSS Selectors Level 3 surface), matching
//! how real upstream itself keeps them as two separate, independently
//! useful libraries in the same codebase.
//!
//! This tracking issue (#85) was flagged across multiple prior sessions
//! as "no concrete caller needs the extra surface yet, maintainer's
//! call" -- ported for real this session per direct instruction.

pub mod errors;
pub mod ordered_set;
pub mod parser;
