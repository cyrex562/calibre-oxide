//! Port of `old_src/src/calibre/db/cli/cmd_fts_search.py` (issue
//! #226). The real file is named `cmd_fts_search.py` -- this crate's
//! file was previously named `cmd_fits_search.rs` ("fits" is a typo
//! for "fts", Full-Text Search), renamed as part of this pass.
//!
//! # Scope of this pass
//!
//! Real: the `--restrict-to ids:.../search:...` split (`search:`
//! delegates to [`crate::Library::search`], the real query-syntax
//! engine from #210), `--include-snippets`/`--match-start-marker`/
//! `--match-end-marker`/`--do-not-match-on-related-words` (stemming
//! toggle) all wired straight through to
//! [`crate::fts::connection::FtsConnection::search`], `--output-format
//! text/json`, and `--indexing-threshold` (aborts if too much of the
//! library is still unindexed, matching upstream's `l/t >
//! (1-threshold)` check).
//!
//! Not ported: upstream's `text` output groups consecutive identical
//! snippets across formats of the same book into one printed block
//! (`current_text_q`/`current_formats` dedup in
//! `output_results_as_text`) -- this prints one line per hit instead,
//! a real but disclosed simplification since result ordering here
//! isn't guaranteed to put a book's formats adjacent to each other the
//! way upstream's single connection/cursor iteration does.

use anyhow::{bail, Result};
use clap::Parser;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Parser)]
pub struct RunArgs {
    /// Search expression (joined if given as multiple words)
    #[arg(required = true, trailing_var_arg = true)]
    pub query: Vec<String>,

    #[clap(long)]
    pub include_snippets: bool,

    #[clap(long, default_value = "\x1b[31m")]
    pub match_start_marker: String,

    #[clap(long, default_value = "\x1b[m")]
    pub match_end_marker: String,

    /// Only match on exact words, not stemmed/related ones.
    #[clap(long)]
    pub do_not_match_on_related_words: bool,

    /// `ids:1,2,3` or `search:tag:foo`.
    #[clap(long, default_value = "")]
    pub restrict_to: String,

    #[clap(long, default_value = "text")]
    pub output_format: String,

    #[clap(long, default_value_t = 90.0)]
    pub indexing_threshold: f64,
}

pub struct CmdFtsSearch;

impl Default for CmdFtsSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl CmdFtsSearch {
    pub fn new() -> Self {
        CmdFtsSearch
    }

    pub fn run(&self, db: &crate::Library, args: &RunArgs) -> Result<()> {
        if !db.is_fts_enabled()? {
            bail!(
                "Full text searching is not enabled on this library. Use the calibredb fts_index enable command to enable it"
            );
        }
        let (left, total) = db.fts_indexing_progress()?;
        let threshold = args.indexing_threshold.clamp(0.0, 100.0) / 100.0;
        if total > 0 && (left as f64 / total as f64) > (1.0 - threshold) {
            bail!(
                "{} files out of {} are not yet indexed, searching is disabled",
                left,
                total
            );
        }

        let restrict_to: Option<HashSet<i32>> = if args.restrict_to.is_empty() {
            None
        } else if let Some(ids) = args.restrict_to.strip_prefix("ids:") {
            Some(
                ids.split(',')
                    .filter_map(|s| s.trim().parse::<i32>().ok())
                    .collect(),
            )
        } else if let Some(query) = args.restrict_to.strip_prefix("search:") {
            Some(db.search(query)?.into_iter().collect())
        } else {
            bail!("The --restrict-to option must start with either ids: or search:");
        };

        let query = args.query.join(" ");
        let fts = db.fts();
        let results = fts.search(
            &query,
            !args.do_not_match_on_related_words,
            Some((&args.match_start_marker, &args.match_end_marker)),
            Some(64),
            restrict_to.as_ref(),
            args.include_snippets,
        )?;

        let mut metadata_cache: HashMap<i32, (String, Vec<String>)> = HashMap::new();
        for r in &results {
            metadata_cache.entry(r.book_id).or_insert_with(|| {
                let title = db
                    .get_book(r.book_id)
                    .ok()
                    .flatten()
                    .map(|b| b.title)
                    .unwrap_or_default();
                let authors = db.get_authors(r.book_id).unwrap_or_default();
                (title, authors)
            });
        }

        if args.output_format == "json" {
            let json: Vec<serde_json::Value> = results
                .iter()
                .map(|r| {
                    let (title, authors) =
                        metadata_cache.get(&r.book_id).cloned().unwrap_or_default();
                    serde_json::json!({
                        "book_id": r.book_id,
                        "format": r.format,
                        "text": if args.include_snippets { r.text.clone() } else { None },
                        "title": title,
                        "authors": authors,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
        } else {
            for r in &results {
                let (title, authors) = metadata_cache.get(&r.book_id).cloned().unwrap_or_default();
                println!("{} by {}", title, authors.join(" & "));
                println!("Book id: {} Format: {}", r.book_id, r.format);
                if args.include_snippets {
                    if let Some(text) = &r.text {
                        println!("{text}");
                    }
                }
                println!("{}", "-".repeat(40));
            }
        }
        Ok(())
    }
}
