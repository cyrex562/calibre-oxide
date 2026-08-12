//! `harness status` — print in-flight issues, open PRs, last sweep.

use anyhow::Result;
use std::path::Path;

use crate::{gh, state};

#[derive(clap::Args)]
pub struct Args {}

pub fn run(repo: &Path, _args: Args) -> Result<()> {
    let st = state::load(repo)?;
    println!("harness state ({}):", state::state_path(repo).display());
    if st.in_flight.is_empty() {
        println!("  in-flight: (none)");
    } else {
        println!("  in-flight ({}):", st.in_flight.len());
        for f in &st.in_flight {
            println!("    #{}  branch={}  started={}", f.issue, f.branch, f.started);
        }
    }
    println!("  green-judged PRs: {:?}", st.green_judged_prs);
    println!("  last sweep: {:?}", st.last_sweep);
    println!("  last seed:  {:?}", st.last_seed);

    if let Ok(()) = gh::ensure_gh_available() {
        let slug = gh::repo_slug()?;
        let issues = gh::list_issues(&slug, &["--state", "open", "--limit", "50"])?;
        println!("\nopen issues ({}):", issues.len());
        for i in issues.iter().take(20) {
            let labels: Vec<&str> = i.labels.iter().map(|l| l.name.as_str()).collect();
            println!("  #{:>4}  [{}]  {}", i.number, labels.join(","), i.title);
        }
        if issues.len() > 20 {
            println!("  ... {} more", issues.len() - 20);
        }
    }
    Ok(())
}
