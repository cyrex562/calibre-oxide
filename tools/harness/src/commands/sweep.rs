//! `harness sweep` — merge PRs the harness has marked green-and-judged.
//!
//! **Deliberately not implemented, by user decision (issue #92).**
//! `state.green_judged_prs` is only ever populated by the also-not-
//! implemented `run` pipeline (see that command's own module doc for
//! the full rationale) -- there's nothing for this command to iterate
//! even if it were built. Every real PR in this repo merges via an
//! interactive Claude Code session (`gh pr merge` under direct
//! instruction), not an unsupervised sweep. Superseded, not pending.

use anyhow::Result;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(_repo: &Path, _args: Args) -> Result<()> {
    eprintln!("harness sweep: not implemented (deliberately -- see this file's own module doc)");
    eprintln!("state.green_judged_prs is only ever populated by `run`, which is also");
    eprintln!("deliberately unimplemented. Merges happen via an interactive Claude Code");
    eprintln!("session instead. There is no follow-up issue tracking a real implementation.");
    Err(anyhow::anyhow!("harness sweep is deliberately unimplemented — use an interactive Claude Code session instead"))
}
