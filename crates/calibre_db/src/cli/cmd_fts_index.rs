//! Port of `old_src/src/calibre/db/cli/cmd_fts_index.py` (issue #226).
//! The real file is named `cmd_fts_index.py` -- this crate's file was
//! previously named `cmd_fits_index.rs` ("fits" is a typo for "fts",
//! Full-Text Search), renamed as part of this pass.
//!
//! # Scope of this pass
//!
//! Real: `status`/`enable`/`disable`/`reindex`, backed by
//! [`crate::Library::is_fts_enabled`]/[`crate::Library::set_fts_enabled`]/
//! [`crate::Library::fts`] (issue #226). `reindex` with no book ids
//! marks every format in the library dirty
//! ([`crate::fts::connection::FtsConnection::dirty_existing`]);
//! `book_id[:FMT,FMT]` specs mark just those.
//!
//! Not ported: `--wait-for-completion`/the `wait` action and indexing
//! *rate* reporting -- both are about a live background indexing
//! pipeline's progress, and this crate has no such pipeline (see
//! `fts/connection.rs`'s module doc). `--indexing-speed` is parsed but
//! has nothing to apply to for the same reason.

use anyhow::{bail, Result};
use clap::Parser;

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// enable, disable, status, or reindex
    pub action: String,

    /// For `reindex`: book ids to re-index, optionally
    /// `book_id:FMT,FMT` to restrict to specific formats. If none are
    /// given, the entire library is marked dirty.
    pub items: Vec<String>,
}

pub struct CmdFtsIndex;

impl Default for CmdFtsIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdFtsIndex {
    pub fn new() -> Self {
        CmdFtsIndex
    }

    pub fn run(&self, db: &mut crate::Library, args: &RunArgs) -> Result<()> {
        match args.action.as_str() {
            "status" => {
                if db.is_fts_enabled()? {
                    let (left, total) = db.fts_indexing_progress()?;
                    println!("FTS Indexing is enabled");
                    println!("{} of {} books files indexed", total - left, total);
                } else {
                    println!("FTS Indexing is disabled");
                    std::process::exit(2);
                }
            }
            "enable" => {
                if !db.is_fts_enabled()? {
                    db.set_fts_enabled(true)?;
                }
                let (left, total) = db.fts_indexing_progress()?;
                println!("FTS indexing has been enabled");
                println!("{} of {} books files indexed", total - left, total);
            }
            "disable" => {
                db.set_fts_enabled(false)?;
                println!("FTS indexing has been disabled");
            }
            "reindex" => {
                if !db.is_fts_enabled()? {
                    bail!("Full text indexing is not enabled on this library");
                }
                let fts = db.fts();
                if args.items.is_empty() {
                    fts.dirty_existing()?;
                } else {
                    for item in &args.items {
                        let (book_id_str, fmts_str) = match item.split_once(':') {
                            Some((id, fmts)) => (id, Some(fmts)),
                            None => (item.as_str(), None),
                        };
                        let book_id: i32 = book_id_str
                            .parse()
                            .map_err(|_| anyhow::anyhow!("Invalid book id: {}", book_id_str))?;
                        match fmts_str {
                            Some(fmts) => {
                                let fmts: Vec<&str> = fmts.split(',').collect();
                                fts.dirty_book(book_id, &fmts)?;
                            }
                            None => {
                                // `format_files` returns `(name, format)` pairs.
                                let formats = db.format_files(book_id).unwrap_or_default();
                                let fmts: Vec<&str> =
                                    formats.iter().map(|(_, fmt)| fmt.as_str()).collect();
                                fts.dirty_book(book_id, &fmts)?;
                            }
                        }
                    }
                }
                let (left, total) = db.fts_indexing_progress()?;
                println!("{} of {} books files indexed", total - left, total);
            }
            other => bail!("{} is not a known action", other),
        }
        Ok(())
    }
}
