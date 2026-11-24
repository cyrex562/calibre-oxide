//! Port of `old_src/src/calibre/ebooks/oeb/polish/check/` -- calibre's
//! "Check Book" validator (issue #40, follow-up to the `polish/`
//! foundation ported in issues #39/#161-#166).
//!
//! `base.py` (-> [`base`]) defines the shared error/warning record type
//! every other file's checks build on -- see its module docs for why
//! this port uses one struct plus an optional fixer closure rather than
//! Python's ~40 `BaseError` subclasses. `parsing.py` (-> [`parsing`]),
//! `opf.py` (-> [`opf`]), `fonts.py` (-> [`fonts`]), and `links.py`
//! (-> [`links`]) are fully real: XML/HTML well-formedness, filename/id
//! validation, OPF structural checks, font name-table/embeddability
//! parsing (a narrow, real slice of `calibre.utils.fonts.utils` --
//! see `fonts`'s module docs), and link/reference validation including
//! live external-link checking (reusing `oeb::polish::download`'s
//! `reqwest` pattern).
//!
//! `images.py` (-> [`images`]) is real for detection (decode + CMYK
//! JPEG inspection, using the `image`/`jpeg-decoder` crates) but its
//! `CMYKImage` auto-fix is `todo!()` -- a genuine Qt-`QImage` dependency
//! gap, see its module docs. `css.py` (-> [`css`]) is real for its error
//! *types* and `message_to_error`'s pure mapping logic, but actually
//! running a linter is `todo!()` -- a genuine stylelint/QWebEngine
//! dependency gap with no local equivalent in this workspace, see its
//! module docs. `main.py` (-> [`main`]) wires every real checker into
//! [`main::run_checks`]/[`main::fix_errors`], skipping (not calling
//! into) the two CSS-linting sub-steps that would otherwise panic on any
//! ordinary book -- see its module docs for exactly which two steps and
//! why.

pub mod base;
pub mod css;
pub mod fonts;
pub mod images;
pub mod links;
pub mod main;
pub mod opf;
pub mod parsing;
