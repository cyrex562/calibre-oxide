//! EPUB Canonical Fragment Identifiers.
//!
//! Port of `old_src/src/calibre/ebooks/epub/cfi/`.
//!
//! | Python | Rust |
//! | --- | --- |
//! | `epubcfi.ebnf` | the grammar below |
//! | `parse.py` | [`parse`] |
//! | `tests.py` | the tests in [`parse`] |
//!
//! (The issue tracking this port spells the grammar file
//! `epublfi.ebnf`; the file on disk is `epubcfi.ebnf`.)
//!
//! A CFI addresses a position inside an EPUB — `epubcfi(/6/4!/4/10/2:3)`
//! is "the third character of a particular paragraph" — and is what
//! bookmarks, annotations and last-read positions are stored as. The
//! numbers are child indices, doubled: CFI counts text nodes as the odd
//! positions and elements as the even ones, so `/4` is the second
//! element child.
//!
//! # Grammar
//!
//! `epubcfi.ebnf` is a grako grammar that calibre keeps as the
//! specification but does not use — its parser is written by hand, and
//! so is this one. The grammar is reproduced here because it is the
//! authority on what the parser accepts, and a comment cannot go stale
//! the way a generated file can.
//!
//! Adapted by calibre from <http://www.idpf.org/epub/linking/cfi/epub-cfi.html>,
//! with two changes from the specification: a text location assertion
//! is only allowed after a text offset rather than after any offset,
//! and an offset may not immediately follow a redirect, since that
//! makes no sense.
//!
//! ```text
//! fragment        = "epubcfi(" parent:path [ "," start:path "," end:path ] ")";
//!
//! path            = steps:( { step }+ ) [ ( "!" redirect:path ) | offset:offset ];
//!
//! step            = "/" num:integer [ "[" id_assertion:characters "]" ];
//!
//! text_offset     = ":" char_offset:integer [ "[" text_assertion:text_assertion "]" ];
//!
//! spatial_offset  = "@" x:number ":" y:number;
//!
//! temporal_offset = "~" t:number;
//!
//! offset          = (text_offset:text_offset)
//!                 | (spatio_temporal_offset:(temporal_offset spatial_offset))
//!                 | (temporal_offset:temporal_offset)
//!                 | (spatial_offset:spatial_offset);
//!
//! text_assertion  = [ ( ( before:characters [ "," after:characters ] )
//!                     | ( "," after:characters ) ) ]
//!                   [ parameters:{parameter} ];
//!
//! parameter       = ";" name:characters_no_space "=" { value+:characters [","] }+;
//!
//! (* No leading zeros allowed in integers *)
//! integer         = /0|(?:[1-9][0-9]*)/;
//!
//! (* No leading zeros, except for numbers in (0, 1), and no trailing
//!    zeros for the fractional part *)
//! number          = /(?:[1-9][0-9]*(?:[.][0-9]*[1-9]){0,1})|(?:0[.][0-9]*[1-9])/;
//!
//! (* All valid unicode characters, except the special ones, which are
//!    preceded by a ^ *)
//! characters          = /(?:[-\u0009\u000a\u000d\u0020-\u0027\u002a\u002b
//!                          \u002e-\u003a\u003c\u003e-\u005a\u005c
//!                          \u005f-\ud7ff\ue000-\ufffd
//!                          \U00010000-\U0010FFFF]|(?:\^[[\](),;=^]))+/;
//! characters_no_space = /(?:[-\u0009\u000a\u000d\u0021-\u0027\u002a\u002b
//!                          \u002e-\u003a\u003c\u003e-\u005a\u005c
//!                          \u005f-\ud7ff\ue000-\ufffd
//!                          \U00010000-\U0010FFFF]|(?:\^[[\](),;=^]))+/;
//! ```
//!
//! Where the hand-written parser knowingly differs from the grammar —
//! it accepts trailing zeros in the fractional part, and accepts `^-`
//! because calibre used to write it — [`parse`] says so at the point of
//! difference.

pub mod parse;

pub use parse::{
    cfi_sort_key, decode_cfi, parse_epubcfi, parse_path, Cfi, CfiSortKey, Offsets, Path, Step,
    TextAssertion,
};
