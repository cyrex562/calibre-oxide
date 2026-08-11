//! `harness scan-placeholders` — rewrite docs/placeholders.jsonl from source.

use anyhow::Result;
use std::path::Path;

use crate::placeholders;

#[derive(clap::Args)]
pub struct Args {
    /// Print the diff between the existing and freshly-scanned registry
    /// without writing anything.
    #[arg(long)]
    pub dry_run: bool,
}

pub fn run(repo: &Path, args: Args) -> Result<()> {
    let scanned = placeholders::scan(repo)?;
    let existing = placeholders::read_registry(repo)?;

    let added = count_diff(&scanned, &existing);
    let removed = count_diff(&existing, &scanned);

    println!(
        "scanned {} placeholders  (was {}; +{} added, -{} removed)",
        scanned.len(),
        existing.len(),
        added,
        removed
    );

    if !added_entries(&scanned, &existing).is_empty() {
        println!("\nnew placeholders:");
        for p in added_entries(&scanned, &existing) {
            println!("  + {}:{}  {}", p.path, p.line, p.reason);
        }
    }
    if !added_entries(&existing, &scanned).is_empty() {
        println!("\ncleared placeholders:");
        for p in added_entries(&existing, &scanned) {
            println!("  - {}:{}  {}", p.path, p.line, p.reason);
        }
    }

    if args.dry_run {
        return Ok(());
    }

    let path = placeholders::write_registry(repo, &scanned)?;
    println!("wrote {}", path.display());
    Ok(())
}

fn count_diff(a: &[placeholders::Placeholder], b: &[placeholders::Placeholder]) -> usize {
    added_entries(a, b).len()
}

fn added_entries<'a>(
    a: &'a [placeholders::Placeholder],
    b: &[placeholders::Placeholder],
) -> Vec<&'a placeholders::Placeholder> {
    a.iter().filter(|p| !b.contains(p)).collect()
}
