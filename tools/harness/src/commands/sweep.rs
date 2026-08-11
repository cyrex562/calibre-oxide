//! `harness sweep` — merge PRs the harness has marked green-and-judged.
//!
//! Not implemented in bootstrap PR. Tracked as a follow-up issue.

use anyhow::Result;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(_repo: &Path, _args: Args) -> Result<()> {
    eprintln!("harness sweep: NOT YET IMPLEMENTED (bootstrap PR ships CLI surface only)");
    // PLACEHOLDER: iterate state.green_judged_prs, re-verify each, gh pr merge --squash --auto.
    Err(anyhow::anyhow!("harness sweep not implemented in bootstrap"))
}
