//! Thin wrapper around the `gh` CLI. We shell out rather than pull in a
//! GitHub SDK because (a) auth is already in the OS keyring via gh, and
//! (b) the harness itself is a user-facing tool and `gh` is a hard
//! dependency anyway.

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::process::Command;

pub fn ensure_gh_available() -> Result<()> {
    let out = Command::new("gh").arg("--version").output()
        .context("failed to invoke `gh` — install from https://cli.github.com")?;
    if !out.status.success() {
        return Err(anyhow!("`gh --version` failed: {:?}", out));
    }
    Ok(())
}

/// Return the origin remote's owner/repo, e.g. `cyrex562/calibre-oxide`.
///
/// Deliberately does NOT use `gh repo view`: when the repo is a fork with
/// an `upstream` remote pointing at the parent, `gh` returns the parent's
/// slug (kovidgoyal/calibre here), which is not what the harness wants to
/// touch. Instead we parse the origin URL directly.
pub fn repo_slug() -> Result<String> {
    let out = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .context("git config remote.origin.url failed")?;
    if !out.status.success() {
        return Err(anyhow!(
            "git config remote.origin.url failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let url = String::from_utf8(out.stdout)?.trim().to_string();
    parse_repo_slug(&url).ok_or_else(|| anyhow!("could not parse repo slug from `{}`", url))
}

fn parse_repo_slug(url: &str) -> Option<String> {
    // Handle: https://github.com/OWNER/REPO(.git)  or  git@github.com:OWNER/REPO(.git)
    let after_host = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let stripped = after_host.trim_end_matches(".git").trim_end_matches('/');
    if stripped.matches('/').count() == 1 {
        Some(stripped.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_slug_https() {
        assert_eq!(
            parse_repo_slug("https://github.com/cyrex562/calibre-oxide.git"),
            Some("cyrex562/calibre-oxide".to_string())
        );
        assert_eq!(
            parse_repo_slug("https://github.com/cyrex562/calibre-oxide"),
            Some("cyrex562/calibre-oxide".to_string())
        );
    }

    #[test]
    fn parse_slug_ssh() {
        assert_eq!(
            parse_repo_slug("git@github.com:cyrex562/calibre-oxide.git"),
            Some("cyrex562/calibre-oxide".to_string())
        );
    }

    #[test]
    fn parse_slug_rejects_nonsense() {
        assert_eq!(parse_repo_slug("not-a-url"), None);
        assert_eq!(parse_repo_slug("https://gitlab.com/foo/bar"), None);
    }
}

#[derive(Debug, Deserialize)]
pub struct IssueSummary {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    #[allow(dead_code)] // consumed by `sweep` / `run` in follow-up PRs
    pub state: String,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
}

pub fn list_issues(repo: &str, extra_args: &[&str]) -> Result<Vec<IssueSummary>> {
    let mut cmd = Command::new("gh");
    cmd.args([
        "issue", "list", "--repo", repo, "--limit", "500",
        "--json", "number,title,labels,state",
    ]);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd.output().context("gh issue list failed")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh issue list failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

pub struct CreateIssue<'a> {
    pub repo: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub labels: &'a [&'a str],
}

pub fn create_issue(req: CreateIssue<'_>) -> Result<u64> {
    let mut cmd = Command::new("gh");
    cmd.args(["issue", "create", "--repo", req.repo, "--title", req.title, "--body", req.body]);
    for l in req.labels {
        cmd.arg("--label").arg(l);
    }
    let out = cmd.output().context("gh issue create failed")?;
    if !out.status.success() {
        return Err(anyhow!(
            "gh issue create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    // gh emits the URL on stdout: https://github.com/<owner>/<repo>/issues/<n>
    let url = String::from_utf8(out.stdout)?.trim().to_string();
    let n = url.rsplit('/').next().unwrap_or("");
    n.parse::<u64>()
        .with_context(|| format!("could not parse issue number from `{}`", url))
}

pub fn ensure_labels(repo: &str, labels: &[(&str, &str, &str)]) -> Result<()> {
    // (name, color hex without #, description)
    let existing: Vec<serde_json::Value> = {
        let out = Command::new("gh")
            .args(["label", "list", "--repo", repo, "--json", "name", "--limit", "200"])
            .output()?;
        if !out.status.success() {
            return Err(anyhow!(
                "gh label list failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        serde_json::from_slice(&out.stdout)?
    };
    let existing_names: std::collections::HashSet<String> = existing
        .iter()
        .filter_map(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
        .collect();

    for (name, color, desc) in labels {
        if existing_names.contains(*name) {
            continue;
        }
        let out = Command::new("gh")
            .args([
                "label", "create", name,
                "--repo", repo,
                "--color", color,
                "--description", desc,
            ])
            .output()?;
        if !out.status.success() {
            return Err(anyhow!(
                "gh label create {} failed: {}",
                name,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
    }
    Ok(())
}
