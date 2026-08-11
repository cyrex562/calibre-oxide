//! `harness playtest-ready` — write a per-cluster playtest checklist.
//!
//! Not implemented in bootstrap PR. Tracked as a follow-up issue.

use anyhow::Result;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Git ref to diff against (default: last playtest tag or origin/master).
    #[arg(long)]
    pub since: Option<String>,
}

pub fn run(_repo: &Path, _args: Args) -> Result<()> {
    eprintln!("harness playtest-ready: NOT YET IMPLEMENTED (bootstrap PR ships CLI surface only)");
    // PLACEHOLDER: walk merges since `since`, group by cluster, emit
    // .harness/playtest/<timestamp>.md with what changed + click-through checklist.
    Err(anyhow::anyhow!("harness playtest-ready not implemented in bootstrap"))
}
