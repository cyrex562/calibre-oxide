//! HTMLZ: a book as one HTML file plus its resources, zipped.
//!
//! Port of `old_src/src/calibre/ebooks/htmlz/`:
//!
//! | Python | Rust |
//! | --- | --- |
//! | `__init__.py` (empty upstream) | this module |
//! | `oeb2html.py` | [`oeb2html`] |

pub mod oeb2html;

pub use oeb2html::{
    oeb2html_class_css, oeb2html_inline_css, oeb2html_no_css, Converted, CssMode, Oeb2Html,
};
