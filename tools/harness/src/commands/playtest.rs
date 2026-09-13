//! `harness playtest-ready` — write a per-cluster playtest checklist.
//!
//! Per `docs/HARNESS.md`'s "Playtest flow" §3: walk commits since the
//! last playtest checkpoint, group them by cluster, and emit
//! `.harness/playtest/<timestamp>.md` with what changed and which files
//! were touched, so a human playtesting session has a concrete,
//! per-area starting point instead of re-reading the whole `git log`.
//!
//! # Commits, not merges
//!
//! `docs/HARNESS.md`'s original phrasing ("rebase and merge each
//! green-judged branch, then walk merges") describes the full
//! autonomous `run`/`sweep` pipeline, which was never actually used in
//! practice for this repo — every real port lands via `gh pr merge
//! --squash`, producing one ordinary (non-merge) commit per PR directly
//! on `master`, never a real two-parent merge commit. This command
//! walks `git log --no-merges` instead of looking for merge commits
//! that don't exist here, and reads each commit's `## Summary` section
//! (present verbatim in the squash commit body, since `gh pr merge
//! --squash` folds the PR description into it) rather than depending on
//! `state.json`'s `green_judged_prs` list, which the never-used judge
//! pipeline also never populates.
//!
//! # Cluster inference from changed paths, not GitHub labels
//!
//! Fetching each commit's originating PR's `cluster:*` label would need
//! one `gh` API call per commit. Since every real cluster in this repo
//! already corresponds 1:1 with a crate directory (`crates/calibre_db`
//! -> `db`, etc. — the same mapping `seed_issues.rs`'s own
//! `cluster_label` uses, duplicated here in [`cluster_for_path`] since
//! that one is private to its own module), inferring cluster from the
//! commit's changed file paths is equivalent for every commit this repo
//! actually produces and needs no network access.
//!
//! # No fabricated "what to click"
//!
//! The design doc's aspirational phrasing ("what to click, what to look
//! for") isn't literally invented here: this command has no way to know
//! what a change looks like in a running app, so the checklist lists
//! each commit's own `## Summary` bullets (real content the porting
//! session already wrote) and the files it touched, and leaves the
//! actual click-through judgment to the human reading it.

use anyhow::{anyhow, Context, Result};
use std::collections::BTreeMap;
use std::path::Path;

use crate::git;

const TAG_PREFIX: &str = "playtest-";

#[derive(clap::Args)]
pub struct Args {
    /// Git ref to diff against (default: last playtest tag or origin/master).
    #[arg(long)]
    pub since: Option<String>,
    /// Don't tag HEAD as `playtest-<timestamp>` afterwards. By default a
    /// tag is created so the *next* run's default `--since` picks up
    /// from here, per `docs/HARNESS.md`'s "last playtest tag" default.
    #[arg(long)]
    pub no_tag: bool,
}

struct CommitEntry {
    sha: String,
    subject: String,
    summary: Vec<String>,
    files: Vec<String>,
}

pub fn run(repo: &Path, args: Args) -> Result<()> {
    let since = resolve_since(repo, args.since.as_deref())?;
    let commits = git::commits_since(repo, &since)?;
    if commits.is_empty() {
        println!("no commits since {since} — nothing to playtest");
        return Ok(());
    }

    let mut by_cluster: BTreeMap<String, Vec<CommitEntry>> = BTreeMap::new();
    for c in &commits {
        let files = git::changed_files(repo, &c.sha)?;
        let cluster = infer_cluster(&files);
        by_cluster.entry(cluster).or_default().push(CommitEntry {
            sha: c.sha.clone(),
            subject: c.subject.clone(),
            summary: extract_summary(&c.body),
            files,
        });
    }

    let now = chrono::Utc::now();
    let markdown = render_report(&since, now, &by_cluster);

    let dir = repo.join(".harness/playtest");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {:?}", dir))?;
    let filename = format!("{}.md", now.format("%Y%m%dT%H%M%SZ"));
    let path = dir.join(&filename);
    std::fs::write(&path, &markdown).with_context(|| format!("write {:?}", path))?;

    println!("wrote {}", path.display());
    println!("{} commit(s) across {} cluster(s) since {since}", commits.len(), by_cluster.len());

    if !args.no_tag {
        let tag = format!("{TAG_PREFIX}{}", now.format("%Y%m%dT%H%M%SZ"));
        git::tag_head(repo, &tag)?;
        println!("tagged HEAD as {tag} (next run's default --since)");
    }

    Ok(())
}

fn resolve_since(repo: &Path, explicit: Option<&str>) -> Result<String> {
    if let Some(s) = explicit {
        return Ok(s.to_string());
    }
    if let Some(tag) = git::latest_tag_with_prefix(repo, TAG_PREFIX)? {
        return Ok(tag);
    }
    if git::ref_exists(repo, "origin/master") {
        return Ok("origin/master".to_string());
    }
    Err(anyhow!("no --since given, no {TAG_PREFIX}* tag found, and origin/master doesn't resolve here — pass --since explicitly"))
}

/// Port of the same crate-name -> cluster mapping `seed_issues.rs`'s
/// own (private) `cluster_label` uses, applied to a changed-file path
/// instead of a placeholder's `crate_name` field.
fn cluster_for_path(path: &str) -> String {
    let mut parts = path.split('/');
    match parts.next() {
        Some("crates") => match parts.next().unwrap_or("unknown") {
            "calibre_db" => "db".to_string(),
            "calibre_devices" => "devices".to_string(),
            "calibre_ebooks" => "ebooks".to_string(),
            "calibre_utils" => "utils".to_string(),
            "calibre_srv" => "srv".to_string(),
            "calibre_conversion" => "conversion".to_string(),
            other => other.strip_prefix("calibre_").unwrap_or(other).to_string(),
        },
        Some("tools") => "harness".to_string(),
        Some("docs") => "docs".to_string(),
        Some("app") => "app".to_string(),
        Some("web") => "web-ui".to_string(),
        Some(other) => other.to_string(),
        None => "misc".to_string(),
    }
}

/// A commit's cluster is whichever cluster its changed files touch
/// most; ties break on the first-seen cluster (stable given
/// `changed_files`'s own sorted-by-path order).
fn infer_cluster(files: &[String]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for f in files {
        let c = cluster_for_path(f);
        match counts.iter_mut().find(|(name, _)| *name == c) {
            Some((_, n)) => *n += 1,
            None => counts.push((c, 1)),
        }
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c).unwrap_or_else(|| "misc".to_string())
}

/// Pulls the `## Summary` bullet list out of a commit body (real content
/// for any commit produced by `gh pr merge --squash`, since that folds
/// the PR description in verbatim) -- falls back to the first non-empty,
/// non-heading body line for a commit with no such section.
fn extract_summary(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_summary = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case("## summary") {
            in_summary = true;
            continue;
        }
        if in_summary {
            if trimmed.starts_with("##") {
                break;
            }
            if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
                out.push(item.to_string());
            }
        }
    }
    if out.is_empty() {
        if let Some(first) = body.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#')) {
            out.push(first.to_string());
        }
    }
    out
}

fn render_report(since: &str, now: chrono::DateTime<chrono::Utc>, by_cluster: &BTreeMap<String, Vec<CommitEntry>>) -> String {
    let total: usize = by_cluster.values().map(Vec::len).sum();
    let mut out = String::new();
    out.push_str(&format!("# Playtest checklist — {}\n\n", now.to_rfc3339()));
    out.push_str(&format!("Changes since `{since}`: {total} commit(s) across {} cluster(s).\n\n", by_cluster.len()));

    for (cluster, entries) in by_cluster {
        out.push_str(&format!("## {cluster} ({} commit{})\n\n", entries.len(), if entries.len() == 1 { "" } else { "s" }));
        for e in entries {
            let short_sha = &e.sha[..e.sha.len().min(12)];
            out.push_str(&format!("- [ ] `{short_sha}` {}\n", e.subject));
            for s in &e.summary {
                out.push_str(&format!("  - {s}\n"));
            }
            if !e.files.is_empty() {
                let shown: Vec<&str> = e.files.iter().take(8).map(String::as_str).collect();
                out.push_str(&format!("  - Files: {}", shown.join(", ")));
                if e.files.len() > shown.len() {
                    out.push_str(&format!(", … +{} more", e.files.len() - shown.len()));
                }
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn cluster_for_path_maps_real_crate_names() {
        assert_eq!(cluster_for_path("crates/calibre_db/src/lib.rs"), "db");
        assert_eq!(cluster_for_path("crates/calibre_ebooks/src/css_selectors/select.rs"), "ebooks");
        assert_eq!(cluster_for_path("crates/calibre_utils/src/podofo.rs"), "utils");
        assert_eq!(cluster_for_path("crates/calibre_srv/src/lib.rs"), "srv");
        assert_eq!(cluster_for_path("crates/some_other_crate/src/lib.rs"), "some_other_crate");
        assert_eq!(cluster_for_path("tools/harness/src/main.rs"), "harness");
        assert_eq!(cluster_for_path("docs/modules_to_port.md"), "docs");
        assert_eq!(cluster_for_path("README.md"), "README.md");
    }

    #[test]
    fn infer_cluster_picks_the_majority_touched_cluster() {
        let files = vec![
            "crates/calibre_ebooks/src/a.rs".to_string(),
            "crates/calibre_ebooks/src/b.rs".to_string(),
            "docs/modules_to_port.md".to_string(),
        ];
        assert_eq!(infer_cluster(&files), "ebooks");
    }

    #[test]
    fn infer_cluster_of_no_files_is_misc() {
        assert_eq!(infer_cluster(&[]), "misc");
    }

    #[test]
    fn extract_summary_reads_the_real_summary_section() {
        let body = "## Summary\n- did the first thing\n- did the second thing\n\n## Test plan\n- [x] ran tests\n";
        assert_eq!(extract_summary(body), vec!["did the first thing", "did the second thing"]);
    }

    #[test]
    fn extract_summary_falls_back_to_the_first_body_line_without_a_summary_section() {
        let body = "\nJust a plain commit message, no headings.\n\nmore detail here\n";
        assert_eq!(extract_summary(body), vec!["Just a plain commit message, no headings."]);
    }

    #[test]
    fn extract_summary_is_empty_for_an_empty_body() {
        assert!(extract_summary("").is_empty());
    }

    #[test]
    fn render_report_lists_commits_grouped_by_cluster_with_a_checkbox() {
        let mut by_cluster = BTreeMap::new();
        by_cluster.insert(
            "ebooks".to_string(),
            vec![CommitEntry {
                sha: "abcdef1234567890".to_string(),
                subject: "port: thing (#1)".to_string(),
                summary: vec!["did the thing".to_string()],
                files: vec!["crates/calibre_ebooks/src/a.rs".to_string()],
            }],
        );
        let now = chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap().with_timezone(&chrono::Utc);
        let md = render_report("origin/master", now, &by_cluster);
        assert!(md.contains("## ebooks (1 commit)"));
        assert!(md.contains("- [ ] `abcdef123456` port: thing (#1)"));
        assert!(md.contains("did the thing"));
        assert!(md.contains("Files: crates/calibre_ebooks/src/a.rs"));
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = Command::new("git").current_dir(repo).args(args).status().expect("git invocation failed");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo_with_commits() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        run_git(repo, &["init", "-q"]);
        run_git(repo, &["config", "user.email", "test@example.com"]);
        run_git(repo, &["config", "user.name", "Test"]);

        std::fs::create_dir_all(repo.join("crates/calibre_ebooks/src")).unwrap();
        std::fs::write(repo.join("crates/calibre_ebooks/src/a.rs"), "// a\n").unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-q", "-m", "initial commit"]);
        run_git(repo, &["tag", "playtest-0"]);

        std::fs::write(repo.join("crates/calibre_ebooks/src/a.rs"), "// a v2\n").unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-q", "-m", "port: ebooks thing (#42)\n\n## Summary\n- did the ebooks thing\n"]);

        std::fs::create_dir_all(repo.join("crates/calibre_utils/src")).unwrap();
        std::fs::write(repo.join("crates/calibre_utils/src/b.rs"), "// b\n").unwrap();
        run_git(repo, &["add", "."]);
        run_git(repo, &["commit", "-q", "-m", "port: utils thing (#43)\n\n## Summary\n- did the utils thing\n"]);

        tmp
    }

    #[test]
    fn run_writes_a_real_report_grouped_by_cluster_and_tags_head() {
        let tmp = init_repo_with_commits();
        let repo = tmp.path();

        let args = Args { since: Some("playtest-0".to_string()), no_tag: false };
        run(repo, args).unwrap();

        let dir = repo.join(".harness/playtest");
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(entries.len(), 1, "expected exactly one report file");
        let content = std::fs::read_to_string(entries[0].as_ref().unwrap().path()).unwrap();
        assert!(content.contains("## ebooks (1 commit)"));
        assert!(content.contains("## utils (1 commit)"));
        assert!(content.contains("did the ebooks thing"));
        assert!(content.contains("did the utils thing"));

        // A new playtest-* tag was created at HEAD, ahead of playtest-0.
        let tag = git::latest_tag_with_prefix(repo, "playtest-").unwrap().unwrap();
        assert_ne!(tag, "playtest-0");
    }

    #[test]
    fn resolve_since_prefers_explicit_then_latest_tag() {
        let tmp = init_repo_with_commits();
        let repo = tmp.path();
        assert_eq!(resolve_since(repo, Some("explicit-ref")).unwrap(), "explicit-ref");
        assert_eq!(resolve_since(repo, None).unwrap(), "playtest-0");
    }

    #[test]
    fn run_reports_nothing_to_do_when_since_is_head() {
        let tmp = init_repo_with_commits();
        let repo = tmp.path();
        run_git(repo, &["tag", "playtest-latest"]);
        let args = Args { since: Some("playtest-latest".to_string()), no_tag: true };
        run(repo, args).unwrap();
        // No report directory should have been created.
        assert!(!repo.join(".harness/playtest").exists());
    }
}
