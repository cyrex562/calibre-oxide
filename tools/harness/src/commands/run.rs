//! `harness run` — the porting iteration loop.
//!
//! **Deliberately not implemented, by user decision (issue #91) — not a
//! bootstrap-scope deferral anymore.** `docs/HARNESS.md`'s own
//! §Iteration loop design has this command spawn `claude` subprocesses
//! to plan/implement/judge each port, then `gh pr merge --squash --auto`
//! on a passing AI-judge verdict with **no human review** — a
//! materially different, higher-risk shape than how every real port in
//! this repo has actually landed: an interactive Claude Code session
//! (via `/loop` and friends) doing the work directly, with a human
//! present at every merge decision. That's not an accident of bootstrap
//! sequencing; it's the tool that's actually been used, and it already
//! covers everything this command would have done (claim/plan/implement
//! /judge/merge), just with a human in the loop instead of an
//! unsupervised second AI judge. Building this for real would mean
//! adding unsupervised autonomous merging to a repo that has
//! specifically not wanted that. Superseded, not pending -- see
//! `docs/HARNESS.md`'s own updated note.
//!
//! The CLI surface stays (so `harness run ...` fails with a clear
//! message instead of "unknown subcommand"), but there is no follow-up
//! issue tracking a real implementation.

use anyhow::Result;
use std::path::Path;

#[derive(clap::Args)]
pub struct Args {
    /// Comma-separated issue numbers to work.
    #[arg(long, value_delimiter = ',')]
    pub issues: Vec<u64>,

    /// Cluster label to filter by (e.g. `cluster:db`).
    #[arg(long)]
    pub cluster: Option<String>,

    /// Auto-pick up to N unblocked, unassigned issues.
    #[arg(long, conflicts_with_all = ["issues", "cluster"])]
    pub auto: bool,

    /// Cap on concurrent in-flight branches.
    #[arg(long, default_value_t = 3)]
    pub max_concurrent: usize,

    /// Cap total issues processed in this invocation (auto mode).
    #[arg(long, default_value_t = 5)]
    pub max_issues: usize,
}

pub fn run(_repo: &Path, args: Args) -> Result<()> {
    eprintln!("harness run: not implemented (deliberately -- see this file's own module doc)");
    eprintln!("  planned inputs: issues={:?}, cluster={:?}, auto={}, max_concurrent={}, max_issues={}",
        args.issues, args.cluster, args.auto, args.max_concurrent, args.max_issues);
    eprintln!();
    eprintln!("The autonomous plan/implement/judge/auto-merge pipeline this command");
    eprintln!("would run is superseded by interactive Claude Code sessions (/loop and");
    eprintln!("friends), which is how every real port in this repo has actually landed.");
    eprintln!("There is no follow-up issue tracking a real implementation of this command.");
    Err(anyhow::anyhow!("harness run is deliberately unimplemented — use an interactive Claude Code session instead"))
}
