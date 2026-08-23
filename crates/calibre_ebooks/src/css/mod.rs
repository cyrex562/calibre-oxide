//! A small, scoped CSS layer: a stylesheet object model plus a selector
//! matcher, built for the `oeb::polish` files that manipulate CSS
//! directly (`css.py`, `cascade.py`, `fonts.py`, `subset.py`).
//!
//! Five earlier ports (issues #34/#35/#36, #161, #162) each hit "this
//! crate has no CSS parser" and left the gap as a documented `todo!()`
//! because CSS parsing was incidental to those files' actual job (see
//! `oeb::polish::utils::parse_css`'s docs, `oeb::normalize_css`'s narrow
//! shorthand-only scope, and `oeb::stylizer`'s module docs for the
//! previously-existing, much narrower alternative). For issue #164
//! (`css.py`/`stats.py`) CSS parsing and selector matching are the
//! *actual subject matter*, not a side dependency, so this module exists
//! to close that gap for real -- the same reasoning that led issue #33 to
//! adopt `html5ever` for HTML instead of hand-rolling a parser.
//!
//! # What this is built on
//!
//! [`cssparser`] -- the Mozilla/Servo CSS Syntax Level 3
//! tokenizer/low-level parser that Stylo, Servo and `lightningcss` all
//! build on. `cssparser` gives a correct, spec-compliant *token stream*
//! (idents, hashes, strings, delimiters, balanced `{}`/`[]`/`()` blocks,
//! nesting-aware "stop at this delimiter" scanning) but, deliberately,
//! no stylesheet/rule/declaration/selector object model -- that is
//! exactly what this module adds on top of it, modeled closely enough
//! after Python's `css_parser`/`css_selectors` (`CSSStyleSheet`,
//! `CSSRule`, `CSSStyleDeclaration`, `Property`, `Select`) that
//! `css.py`/`cascade.py`/`stats.py`'s logic ports recognizably:
//!
//! - [`model`]: [`model::Stylesheet`] / [`model::Rule`] /
//!   [`model::StyleDeclarationBlock`] / [`model::Declaration`] -- the
//!   object model, plus a serializer (not byte-identical to `cssutils`'s
//!   output, matching this crate's established `xmltree`/`pretty`
//!   convention of "well-formed and structurally correct, not a
//!   byte-for-byte clone of the Python library's formatting").
//! - [`parser`]: turns CSS text into a [`model::Stylesheet`] /
//!   [`model::StyleDeclarationBlock`] using `cssparser`'s tokenizer to
//!   find rule/declaration/block boundaries (respecting nested
//!   `{}`/strings/comments correctly, which a naive brace-counter would
//!   not), then this module's own recursive-descent for what sits inside
//!   those boundaries.
//! - [`selector`]: [`selector::SelectorList`] / [`selector::Selector`] --
//!   a **deliberately scoped** selector grammar and matcher, *not* an
//!   adoption of Servo's `selectors` crate (which would require
//!   implementing its large `selectors::Element` trait -- pseudo-class
//!   queries, full sibling/ancestor iteration protocols, tree-mutation
//!   hooks -- across two DOM types, [`crate::xmltree::Xml`] and
//!   [`crate::dom::Dom`], that don't have that surface today; see
//!   [`selector`]'s module docs for exactly what selector syntax is and
//!   is not supported).
//! - [`matcher`]: the [`matcher::Element`] trait (implemented for
//!   [`crate::dom::Dom`] node references and
//!   [`crate::xmltree::Xml`] node references) plus
//!   [`matcher::Select`], a small per-document index mirroring Python's
//!   `css_selectors.Select` (`select.has_matches(selector_text)`).
//!
//! # What is out of scope
//!
//! CSS *value* parsing (colors, lengths, `calc()`, shorthand expansion
//! beyond what [`crate::oeb::normalize_css`] already does) is not part
//! of this layer -- declaration values are kept as their original,
//! trimmed source text (matching how [`crate::oeb::polish::cascade`]'s
//! `PropertyValue::css_text` already represents values, which is exactly
//! what this module's [`model::Declaration::value`] slots into). CSS3
//! media-query *evaluation* (`@media (min-width: ...)`) is also out of
//! scope -- [`model::Rule::Media`] stores the parsed media prelude as
//! text; `cascade.rs`'s `media_ok`/`media_allowed` remain `todo!()` for
//! that reason (see their docs).

pub mod matcher;
pub mod model;
pub mod parser;
pub mod selector;

pub use matcher::{DomElement, Element, Select, XmlElement};
pub use model::{
    Declaration, ImportRule, MediaRule, Rule, RuleType, StyleDeclarationBlock, StyleRule,
    Stylesheet,
};
pub use selector::{Selector, SelectorError, SelectorList};
