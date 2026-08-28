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
//! # Early structure & preamble passes
//!
//! Issue #187's follow-up: the first 18 of the ~35 later-stage
//! file-to-file passes, in `ParseRtf.parse_rtf()`'s actual call order.
//! Each operates on -- and (mostly) returns -- the intermediate-format
//! text described in [`process_tokens`]'s docs, taking `&str` content
//! and returning transformed content/structs directly rather than
//! reopening files, matching [`check_brackets`]'s convention (the
//! temp-file / [`copy`] / rename dance around each pass in the real
//! pipeline is plumbing, not ported at this layer -- see
//! [`process_tokens::process_tokens`]'s own doc for the same call).
//!
//! - [`delete_info`]: drop the `\info` document-info group.
//! - [`info`]: closely related to `delete_info` (both handle the same
//!   `\info` group; `info` extracts/renames its fields instead of
//!   deleting them wholesale) -- see that module's docs for exactly
//!   how the two relate in the real pipeline.
//! - [`pict`]: extract embedded picture (`\pict`) sub-groups' raw RTF
//!   text out to a sibling `.rtf` file, leaving placeholder marker tags
//!   behind in the main stream.
//! - [`combine_borders`]: merge a border's `cw<bd<...` opening line and
//!   its following `cw<bt<...` sub-attribute lines into one combined
//!   `cw<bd<...` line.
//! - [`footnote`]: separates footnote text out of the main body stream
//!   (`separate_footnotes`), and -- much later in the real pipeline,
//!   after passes out of scope here -- reinserts it (`join_footnotes`).
//! - [`header`]: the same separate/rejoin shape as [`footnote`], for
//!   headers/footers.
//! - [`list_numbers`]: resolve list/bullet numbering tokens.
//! - [`preamble_div`]: carve the preamble into its named divisions
//!   (font table, color table, style sheet, etc).
//! - [`hex_2_utf8`]: resolve `\'HH` hex bytes (and, in the body pass
//!   out of scope here, symbol-font code points) to UTF-8/entities.
//! - [`fonts`]: parse the font table into per-font attributes.
//! - [`colors`]: parse the color table into per-color attributes.
//! - [`styles`]: parse the style sheet into per-style attributes.
//! - [`preamble_rest`]: catch-all cleanup pass for whatever preamble
//!   material the more specific passes above didn't already consume.
//! - [`old_rtf`]: compatibility shims for older/nonstandard RTF
//!   producers, run only at higher run levels.
//! - [`sections`]: parse section-break/section-property tokens.
//! - [`paragraphs`]: mark paragraph boundaries.
//! - [`paragraph_def`]: assemble per-paragraph attribute dictionaries
//!   into `paragraph-definition` tags, returning the body-style strings
//!   [`body_styles`] threads back in.
//! - [`body_styles`]: insert [`paragraph_def`]'s collected body-style
//!   strings after the style table.
//! - [`add_brackets`]: wrap "bare" character-formatting control words
//!   (the same list [`old_rtf::ALLOWABLE`] flags) that appear directly
//!   in running text -- with no bracket group of their own, as older
//!   RTF producers sometimes emit them -- in a synthetic bracket group,
//!   so every later pass can rely on the group-scoping invariant
//!   holding everywhere.
//! - [`fields_small`]: wrap bookmark, index-entry, and toc-entry field
//!   markers with `mi<mk<inline-fld` field-instruction tags (doesn't
//!   handle index/toc tables).
//!
//! Three more follow-up issues cover the rest of the ~35 passes
//! (fields/tables, lists/grouping/tag-conversion, and the
//! `ParseRtf.py` orchestrator itself), built on top of the shapes
//! established here and in the foundation below.
//!
//! # Not here
//!
//! Everything else in `old_src/src/calibre/ebooks/rtf2xml/`:
//! `ParseRtf.py` and the remaining later-stage transformation passes
//! (`border_parse` (except as a private, non-`pub` dependency
//! independently inlined into both [`paragraph_def`] and [`styles`] --
//! see those modules' own docs), `convert_to_tags`, `field_strings`,
//! `fields_large`, `group_borders`, `headings_to_sections`, `inline`,
//! `list_*` (besides
//! [`list_numbers`]), `make_lists`, `output`, `table*`, and the rest).
//! Those are tracked by this crate's follow-up rtf2xml issues, built on
//! top of the shapes established here -- most importantly
//! [`process_tokens`]'s intermediate format.

pub mod add_brackets;
pub mod body_styles;
pub mod char_set;
pub mod check_brackets;
pub mod check_encoding;
pub mod colors;
pub mod combine_borders;
pub mod copy;
pub mod default_encoding;
pub mod delete_info;
pub mod fields_small;
pub mod fonts;
pub mod footnote;
pub mod get_char_map;
pub mod header;
pub mod hex_2_utf8;
pub mod info;
pub mod line_endings;
pub mod list_numbers;
pub mod old_rtf;
pub mod paragraph_def;
pub mod paragraphs;
pub mod pict;
pub mod preamble_div;
pub mod preamble_rest;
pub mod process_tokens;
pub mod replace_illegals;
pub mod sections;
pub mod styles;
pub mod tokenize;
