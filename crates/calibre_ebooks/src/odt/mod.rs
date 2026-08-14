//! ODT (OpenDocument Text) support: a scoped-down ODT-content -\> XHTML
//! converter and the calibre-specific post-processing `input.py`'s
//! `Extract` class adds on top of it. See [`crate::input::odt_input`] for
//! the orchestration that ties these together into the actual input
//! plugin, and each submodule's docs for what's in vs. out of scope.
//!
//! This does *not* port `old_src/src/odf` (the vendored `odfpy` library
//! `input.py` subclasses in Python) -- that ~17,400-line package is
//! tracked separately in `docs/modules_to_port.md` under `## src/odf` and
//! is out of scope for the `ebooks/odt` port this module belongs to.

pub mod convert;
pub mod cover;
pub mod css;
pub mod fixup;
pub mod namespaces;
pub mod styles;
