//! Port of a subset of `calibre.ebooks.conversion` -- the
//! `ebook-convert` CLI's argument-handling/config layer (issue #20:
//! `cli.rs`/`config.rs`).
//!
//! # Not a second conversion engine (issue #476)
//!
//! This crate originally also had its own `OebBook`/`InputPlugin`/
//! `OutputPlugin`/`ConversionPipeline`/`plugins::{epub_input,epub_output}`
//! -- a second, much thinner conversion architecture, parallel to but
//! disconnected from `calibre_ebooks`'s real one
//! (`calibre_ebooks::oeb::book::OEBBook`, its ~24 real per-format
//! `input`/`output` modules, and `calibre_ebooks::conversion::plumber`,
//! the actual format-dispatch table already wired to all of them --
//! reused by `oeb::iterator::book::extract_book`, issue #38). That
//! scaffold never had its own tests, its `OebBook` stored content in a
//! deliberately-leaked temp dir rather than a real container, and its
//! `epub_output`'s OPF generation was a hand-rolled `format!` string
//! with no NCX/TOC support -- strictly worse than what already existed
//! one crate over. It's been removed rather than wired up or adapted:
//! `bin/ebook_convert.rs` now calls `calibre_ebooks::conversion::plumber::Plumber`
//! directly, the same real dispatch table
//! `calibre_ebooks::bin::ebook-convert` (a second, separate binary)
//! already used. See issue #476 for the full investigation.
//!
//! What's still real and kept: [`cli_helpers`] (argument/path
//! validation matching upstream's `cli.py` exactly, including the
//! `.EXT` output-shorthand and `.recipe` readability exemption) and
//! [`config`] (the input/output format option-recommendation
//! registry) -- both already written with "dispatch to the plumber"
//! as their explicit target (see `cli_helpers`'s own module doc),
//! neither depends on anything just removed. [`config`]'s registry
//! isn't wired into `Plumber` yet (`Plumber::run` takes no options at
//! all) -- a real, separate follow-up, not part of #476's own finding.

pub mod cli_helpers;
pub mod config;
