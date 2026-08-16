//! Port of the foundation layer of `old_src/src/calibre/ebooks/rtf2xml/`
//! -- calibre's real RTF-to-XML parsing pipeline (distinct from, and
//! not sharing code with, `crate::rtf` -- issue #50's port of
//! `calibre.ebooks.rtf.preprocess`, a different, narrower-scoped RTF
//! tokenizer used elsewhere in this crate).
//!
//! The full upstream module is 48 files (~30,900 lines) architected as
//! a strict sequential pipeline of ~35 file-to-file transformation
//! passes orchestrated by `ParseRtf.parse_rtf()`. That orchestration
//! (`ParseRtf.py` itself, and the ~35 later-stage passes it calls) is
//! deliberately **out of scope** here -- this module ports only the
//! foundation: the raw-input normalization passes and the two core
//! parsing stages every later pass builds on. Four follow-up issues
//! cover the rest of the pipeline in dependency order.
//!
//! # What's here
//!
//! Raw-input normalization (run before tokenizing):
//! - [`line_endings`]: normalize `\r\n`/`\r` to `\n`.
//! - [`replace_illegals`]: strip illegal low-ASCII control characters.
//! - [`check_encoding`]: verify a byte stream decodes cleanly under a
//!   named encoding.
//! - [`default_encoding`]: determine the codepage/platform an RTF
//!   document implies when it doesn't declare one explicitly.
//!
//! Core parsing stages:
//! - [`tokenize`]: RTF source -> one token per line (rtf2xml's own
//!   tokenizer, not `crate::rtf::preprocess`'s).
//! - [`process_tokens`]: token stream -> the bracket-tagged
//!   intermediate format every later-stage pass (out of scope here)
//!   consumes -- see that module's own docs for the exact line shapes.
//! - [`check_brackets`]: validate the intermediate format's bracket
//!   nesting is balanced (used both as its own pass and internally by
//!   [`process_tokens`]).
//!
//! Character/codepage data + a small debug helper:
//! - [`char_set`] + [`get_char_map`]: the ~16,700-line RTF
//!   character/codepage/font-symbol lookup table and the parser that
//!   extracts one named section from it into a `key -> replacement`
//!   map.
//! - [`copy`]: the pipeline's debug-snapshot helper (not Python's
//!   standard library `copy` module -- see that module's own docs).
//!
//! # Not here
//!
//! Everything else in `old_src/src/calibre/ebooks/rtf2xml/`:
//! `ParseRtf.py` and the ~35 later-stage transformation passes
//! (`add_brackets`, `body_styles`, `border_parse`, `colors`,
//! `combine_borders`, `convert_to_tags`, `fields_*`, `fonts`,
//! `footnote`, `header`, `headings_to_sections`, `hex_2_utf8`,
//! `inline`, `list_*`, `make_lists`, `output`, `paragraph*`, `pict`,
//! `sections`, `styles`, `table*`, and the rest). Those are tracked by
//! this crate's follow-up rtf2xml issues, built on top of the shapes
//! established here -- most importantly [`process_tokens`]'s
//! intermediate format.

pub mod char_set;
pub mod check_brackets;
pub mod check_encoding;
pub mod copy;
pub mod default_encoding;
pub mod get_char_map;
pub mod line_endings;
pub mod process_tokens;
pub mod replace_illegals;
pub mod tokenize;
