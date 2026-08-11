//! calibre-oxide iterative-porting harness.
//!
//! See `docs/HARNESS.md` for architecture. This binary is invoked by the
//! developer to seed GitHub issues, scan the codebase for placeholders,
//! kick off porting iterations, and sweep merge-ready PRs.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;
mod gh;
mod git;
mod paths;
mod placeholders;
mod state;

#[derive(Parser)]
#[command(name = "harness")]
#[command(about = "calibre-oxide porting harness", long_about = None)]
struct Cli {
    /// Path to the repo root. Defaults to CARGO_MANIFEST_DIR/../../ resolved
    /// or the current working directory.
    #[arg(long, global = true)]
    repo: Option<std::path::PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Walk crates/ for placeholder markers and rewrite docs/placeholders.jsonl.
    ScanPlaceholders(commands::scan_placeholders::Args),
    /// Populate GitHub issues from docs/modules_to_port.md and placeholders.jsonl.
    SeedIssues(commands::seed_issues::Args),
    /// Print in-flight issues, open PRs, and last-sweep state.
    Status(commands::status::Args),
    /// Run one or more porting iterations.
    Run(commands::run::Args),
    /// Merge PRs that the harness has already marked green-and-judged.
    Sweep(commands::sweep::Args),
    /// Emit a checklist for user playtesting after a batch of merges.
    PlaytestReady(commands::playtest::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let repo = paths::resolve_repo_root(cli.repo.as_deref())?;

    match cli.command {
        Command::ScanPlaceholders(a) => commands::scan_placeholders::run(&repo, a),
        Command::SeedIssues(a) => commands::seed_issues::run(&repo, a),
        Command::Status(a) => commands::status::run(&repo, a),
        Command::Run(a) => commands::run::run(&repo, a),
        Command::Sweep(a) => commands::sweep::run(&repo, a),
        Command::PlaytestReady(a) => commands::playtest::run(&repo, a),
    }
}
