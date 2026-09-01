//! Port of `old_src/src/calibre/translations/` (issue #61, 2 files):
//! compiling `.po` catalogs into the binary `.mo` format
//! ([`msgfmt`]) and looking translations up dynamically at runtime
//! ([`dynamic`]). [`mo_reader`] is new infrastructure this port
//! needed that upstream gets for free from CPython's own `gettext`
//! module -- see its own doc.

pub mod dynamic;
pub mod mo_reader;
pub mod msgfmt;
