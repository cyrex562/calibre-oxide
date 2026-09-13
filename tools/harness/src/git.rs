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

/// The most recently created tag matching `prefix*`, if any.
///
/// Sorted by refname (`--sort=-refname`), not `--creatordate`: a
/// *lightweight* tag (what [`tag_head`] creates) has no timestamp of
/// its own -- `--creatordate` falls back to the tagged commit's own
/// date, which two tags created in the same wall-clock second (trivial
/// in a fast test, or even a fast real session) can tie on, making
/// "most recent" ambiguous. Every real caller here names its tags with
/// a lexicographically-sortable timestamp suffix (`playtest-
/// YYYYMMDDTHHMMSSZ`), so plain descending refname order is both
/// correct and immune to that tie. Found by a real test failure, not
/// assumed: an initial `--sort=-creatordate` implementation returned
/// the OLDER of two tags created moments apart in the same test run.
pub fn latest_tag_with_prefix(repo: &Path, prefix: &str) -> Result<Option<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["tag", "--list", &format!("{prefix}*"), "--sort=-refname"])
        .output()
        .context("git tag --list failed")?;
    if !out.status.success() {
        return Err(anyhow!("git tag --list failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    let text = String::from_utf8(out.stdout)?;
    Ok(text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
}

/// Does `refname` resolve to a real commit in this repo?
pub fn ref_exists(repo: &Path, refname: &str) -> bool {
    Command::new("git")
        .current_dir(repo)
        .args(["rev-parse", "--verify", "--quiet", &format!("{refname}^{{commit}}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Creates a lightweight tag `name` at `HEAD`.
pub fn tag_head(repo: &Path, name: &str) -> Result<()> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["tag", name])
        .output()
        .context("git tag failed")?;
    if !out.status.success() {
        return Err(anyhow!("git tag {} failed: {}", name, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

pub struct CommitInfo {
    pub sha: String,
    pub subject: String,
    /// Everything after the subject line (the commit's own free-form
    /// body) -- for a squash merge via `gh pr merge --squash`, this is
    /// the full PR description, `## Summary`/`## Test plan` and all.
    pub body: String,
}

/// Every non-merge commit in `since..HEAD`, oldest first.
pub fn commits_since(repo: &Path, since: &str) -> Result<Vec<CommitInfo>> {
    const UNIT_SEP: char = '\u{1f}';
    const REC_SEP: char = '\u{1e}';
    let range = format!("{since}..HEAD");
    let format_arg = format!("--format=%H{UNIT_SEP}%s{UNIT_SEP}%b{REC_SEP}");
    let out = Command::new("git")
        .current_dir(repo)
        .args(["log", "--no-merges", "--reverse", &format_arg, &range])
        .output()
        .context("git log failed")?;
    if !out.status.success() {
        return Err(anyhow!("git log {} failed: {}", range, String::from_utf8_lossy(&out.stderr)));
    }
    let text = String::from_utf8(out.stdout)?;
    let mut commits = Vec::new();
    for record in text.split(REC_SEP) {
        let record = record.trim_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, UNIT_SEP);
        let sha = parts.next().unwrap_or("").to_string();
        if sha.is_empty() {
            continue;
        }
        let subject = parts.next().unwrap_or("").to_string();
        let body = parts.next().unwrap_or("").trim().to_string();
        commits.push(CommitInfo { sha, subject, body });
    }
    Ok(commits)
}

/// Files changed by a single (non-merge) commit, relative to its first
/// parent.
pub fn changed_files(repo: &Path, sha: &str) -> Result<Vec<String>> {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["diff-tree", "--no-commit-id", "--name-only", "-r", sha])
        .output()
        .context("git diff-tree failed")?;
    if !out.status.success() {
        return Err(anyhow!("git diff-tree {} failed: {}", sha, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8(out.stdout)?.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
}
