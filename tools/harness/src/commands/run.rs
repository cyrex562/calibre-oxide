//! `harness run` — the porting iteration loop.
//!
//! This command is intentionally not implemented in the bootstrap PR.
//! Implementing it requires: worktree management, subprocess control for
//! `claude` calls, judge-rubric machinery, and PR/merge orchestration.
//! Each of those is a substantial follow-up issue, and doing them all in
//! one PR would make review impossible.
//!
//! The bootstrap PR ships `scan-placeholders`, `seed-issues`, and
//! `status` fully working, plus the CLI surface for `run` / `sweep` /
//! `playtest-ready` so downstream work can flesh them in without
//! restructuring the crate.

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
    eprintln!("harness run: NOT YET IMPLEMENTED");
    eprintln!("  planned inputs: issues={:?}, cluster={:?}, auto={}, max_concurrent={}, max_issues={}",
        args.issues, args.cluster, args.auto, args.max_concurrent, args.max_issues);
    eprintln!();
    eprintln!("This is intentional. The bootstrap PR ships the CLI surface and");
    eprintln!("the placeholder/seed/status commands. The orchestration loop is");
    eprintln!("tracked as a follow-up harness issue and will land in a separate PR.");
    // PLACEHOLDER: implement iteration loop — see docs/HARNESS.md §Iteration loop.
    Err(anyhow::anyhow!("harness run not implemented in bootstrap"))
}
