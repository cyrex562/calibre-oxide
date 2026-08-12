//! Thin wrapper around `git` CLI. Deliberately not `git2` — the harness
//! shells out to keep the binary small and to match how the user
//! interacts with the repo.
//!
//! Bootstrap PR uses these only in tests; the porting iteration loop
//! (follow-up PR) is the first real caller.
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Command;

pub fn current_branch(repo: &Path) -> Result<String> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("git rev-parse HEAD failed")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

pub fn is_clean(repo: &Path) -> Result<bool> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["status", "--porcelain"])
        .output()?;
    Ok(out.stdout.is_empty())
}
