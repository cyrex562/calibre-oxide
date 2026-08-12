//! `harness seed-issues` — populate GitHub issues from modules_to_port.md
//! and docs/placeholders.jsonl.
//!
//! Scope filter (per user decision):
//!   - Devices: keep only Windows/Linux/Android-relevant drivers. Skip
//!     obsolete devices (binatone, boeye, blackberry, cybook, eb600, edge,
//!     eslick, hanlin, hanvon, iliad, irexdr, iriver, jetbook, nokia, nuut2,
//!     paladin, prs505, prst1, sne, teclast, nook).
//!   - GUI: skip old_src/src/calibre/gui2/** entirely — replaced by
//!     app/ (Tauri+Vue).
//!   - LRF renderer / obscure formats: labeled `deferred`, not seeded by
//!     default. Use --include-deferred to include them.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::Path;

use crate::{gh, placeholders};

#[derive(clap::Args)]
pub struct Args {
    /// Only print what would be created; do not touch GitHub.
    #[arg(long)]
    pub dry_run: bool,
    /// Also seed issues that are marked as `deferred` by the scope filter.
    #[arg(long)]
    pub include_deferred: bool,
    /// Optional cluster tag to restrict seeding to (e.g. `db`, `mobi`).
    #[arg(long)]
    pub only: Option<String>,
}

pub fn run(repo: &Path, args: Args) -> Result<()> {
    gh::ensure_gh_available()?;
    let slug = gh::repo_slug()?;
    println!("repo: {}", slug);

    ensure_labels(&slug, args.dry_run)?;

    let port_items = collect_port_items(repo)?;
    let placeholder_items = collect_placeholder_items(repo)?;

    let existing: BTreeSet<String> = gh::list_issues(&slug, &["--state", "all"])?
        .into_iter()
        .map(|i| i.title)
        .collect();

    let mut planned: Vec<PlannedIssue> = Vec::new();
    for item in port_items.iter().chain(placeholder_items.iter()) {
        if let Some(only) = &args.only {
            if item.cluster != *only {
                continue;
            }
        }
        if !args.include_deferred && item.labels.iter().any(|l| l == "deferred") {
            continue;
        }
        if existing.contains(&item.title) {
            continue;
        }
        planned.push(item.clone());
    }

    println!("planned to create: {} issues (skipped {} already-existing)",
        planned.len(),
        (port_items.len() + placeholder_items.len()).saturating_sub(planned.len()));

    if args.dry_run {
        for p in &planned {
            println!("  + [{}] {}", p.labels.join(","), p.title);
        }
        return Ok(());
    }

    for p in &planned {
        let label_refs: Vec<&str> = p.labels.iter().map(String::as_str).collect();
        let n = gh::create_issue(gh::CreateIssue {
            repo: &slug,
            title: &p.title,
            body: &p.body,
            labels: &label_refs,
        })?;
        println!("created #{}: {}", n, p.title);
    }

    Ok(())
}

fn ensure_labels(slug: &str, dry_run: bool) -> Result<()> {
    let labels = &[
        ("port",             "0e8a16", "Port a Python/C source file to Rust"),
        ("placeholder",      "fbca04", "Clear a placeholder body — implement for real"),
        ("fault-tolerance",  "b60205", "Fault-tolerance / atomic-write / device-safety work"),
        ("gui",              "5319e7", "Tauri + Vue app work"),
        ("harness",          "1d76db", "The porting harness itself"),
        ("cross-validation", "c5def5", "Python↔Rust output diff test"),
        ("judge-review",     "d93f0b", "PR bounced by the harness judge, needs human eyes"),
        ("deferred",         "cccccc", "Out of current scope — skip during auto-seeding"),
        ("cluster:db",       "e99695", "Cluster: metadata database"),
        ("cluster:ebooks",   "e99695", "Cluster: ebook formats"),
        ("cluster:devices",  "e99695", "Cluster: device drivers"),
        ("cluster:mobi",     "e99695", "Cluster: MOBI / KF8"),
        ("cluster:utils",    "e99695", "Cluster: utility modules"),
        ("cluster:srv",      "e99695", "Cluster: content server"),
        ("cluster:conversion","e99695","Cluster: format conversion"),
    ];
    if dry_run {
        println!("(dry-run) would ensure {} labels", labels.len());
        return Ok(());
    }
    gh::ensure_labels(slug, labels)
}

#[derive(Debug, Clone)]
struct PlannedIssue {
    title: String,
    body: String,
    labels: Vec<String>,
    cluster: String,
}

fn collect_port_items(repo: &Path) -> Result<Vec<PlannedIssue>> {
    let path = repo.join("docs/modules_to_port.md");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {:?}", path))?;

    let mut items = Vec::new();
    let mut cluster_stack: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        if let Some(rest) = line.strip_prefix("### ") {
            cluster_stack.truncate(0);
            cluster_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("#### ") {
            cluster_stack.truncate(1);
            cluster_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("##### ") {
            cluster_stack.truncate(2);
            cluster_stack.push(rest.trim().to_string());
        }

        let unchecked = line.trim_start().strip_prefix("- [ ] ");
        if let Some(rest) = unchecked {
            let file = rest.split_whitespace().next().unwrap_or("").trim_matches('`').to_string();
            if file.is_empty() { continue; }
            let cluster = infer_cluster(&cluster_stack);
            let mut labels = vec!["port".to_string()];
            if let Some(l) = cluster_label(&cluster) {
                labels.push(l);
            }
            let deferred = is_deferred(&cluster_stack, &file);
            if deferred {
                labels.push("deferred".to_string());
            }
            let title = format!("port: {} ({})", file, cluster_stack.join(" / "));
            let body = format!(
                "**Cluster**: {}\n\
                 **File**: `{}`\n\
                 **Location in modules_to_port.md**: {}\n\n\
                 Port this Python/C source file to Rust following:\n\
                 - `docs/AGENT_PORTING_GUIDE.md`\n\
                 - `docs/FAULT_TOLERANCE.md`\n\n\
                 Definition of done:\n\
                 - Real signatures, `#[placeholder]`-style bodies only where genuinely blocked.\n\
                 - Unit tests exercising public API.\n\
                 - Cross-validation test if the format is round-trippable.\n\
                 - Mark the checkbox in `docs/modules_to_port.md`.\n",
                cluster, file, cluster_stack.join(" / ")
            );
            items.push(PlannedIssue {
                title,
                body,
                labels,
                cluster,
            });
        }
    }

    Ok(items)
}

fn collect_placeholder_items(repo: &Path) -> Result<Vec<PlannedIssue>> {
    let phs = placeholders::read_registry(repo)?;
    let mut items = Vec::new();
    for p in phs {
        let cluster = p.crate_name.clone();
        let mut labels = vec!["placeholder".to_string()];
        if let Some(l) = cluster_label(&cluster) {
            labels.push(l);
        }
        let title = format!("placeholder: {}:{} — {}", p.path, p.line, p.reason);
        let body = format!(
            "**Crate**: `{}`\n\
             **Path**: `{}` line {}\n\
             **Kind**: {:?}\n\
             **Reason left**: {}\n\n\
             Implement the real body. Preserve the existing signature.\n\
             When done, remove the placeholder marker and re-run\n\
             `harness scan-placeholders` to update the registry.\n",
            p.crate_name, p.path, p.line, p.kind, p.reason
        );
        items.push(PlannedIssue { title, body, labels, cluster });
    }
    Ok(items)
}

fn infer_cluster(stack: &[String]) -> String {
    let top = stack.first().map(String::as_str).unwrap_or("").to_lowercase();
    // top is like "src/calibre/ai" or "ai" (the leading "## src/calibre" is
    // stripped by the earlier heading level). Fall back to the second level.
    if top.starts_with("db") { "db".into() }
    else if top.starts_with("devices") { "devices".into() }
    else if top.starts_with("ebooks") { "ebooks".into() }
    else if top.starts_with("gui2") { "gui".into() }
    else if top.starts_with("srv") { "srv".into() }
    else if top.starts_with("utils") { "utils".into() }
    else if top.starts_with("conversion") { "conversion".into() }
    else if !top.is_empty() { top }
    else { "unknown".into() }
}

fn cluster_label(cluster: &str) -> Option<String> {
    match cluster {
        "db" | "devices" | "ebooks" | "utils" | "srv" | "conversion" | "mobi" => {
            Some(format!("cluster:{}", cluster))
        }
        "calibre_db" => Some("cluster:db".into()),
        "calibre_devices" => Some("cluster:devices".into()),
        "calibre_ebooks" => Some("cluster:ebooks".into()),
        "calibre_utils" => Some("cluster:utils".into()),
        _ => None,
    }
}

/// Per-user scope decision. Skip out-of-scope devices, all of gui2, and a
/// short blocklist of obsolete formats.
fn is_deferred(stack: &[String], file: &str) -> bool {
    let full = stack.join("/").to_lowercase();

    // All of gui2 — replaced by Tauri+Vue app/.
    if full.contains("gui2") {
        return true;
    }
    // pyj — old rapydscript viewer; new viewer is a Tauri panel.
    if full.starts_with("src/pyj") || full.starts_with("pyj") {
        return true;
    }
    // qt bindings — not needed with Tauri.
    if full.starts_with("src/qt") || full.starts_with("qt") {
        return true;
    }
    // Out-of-scope devices.
    let obsolete_devices = [
        "binatone", "boeye", "blackberry", "cybook", "eb600", "edge",
        "eslick", "hanlin", "hanvon", "iliad", "irexdr", "iriver",
        "jetbook", "nokia", "nuut2", "paladin", "prs505", "prst1",
        "sne", "teclast",
        // Nook / Sony readers — user only has Windows/Linux/Android.
        "nook",
        // MTP unix / windows native drivers — will wire Android via ADB, not raw MTP.
        "mtp",
    ];
    for name in obsolete_devices {
        if full.contains(name) {
            return true;
        }
    }
    // LRF renderer / obscure Palm/Psion formats.
    let obscure_formats = ["lrf", "haodoo", "plucker", "ztxt", "azw4"];
    for name in obscure_formats {
        if full.contains(&format!("/{}", name)) || full.starts_with(name) {
            return true;
        }
    }
    // Individual files that are obviously stale.
    if file.ends_with(".ui") || file.ends_with(".sip") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deferred_scope_matches_expected() {
        assert!(is_deferred(&["src/calibre/gui2/actions".into()], "add.py"));
        assert!(is_deferred(&["src/calibre/devices/binatone".into()], "driver.py"));
        assert!(is_deferred(&["src/calibre/devices/nook".into()], "driver.py"));
        assert!(is_deferred(&["src/pyj/book_list".into()], "add.pyj"));
        assert!(!is_deferred(&["src/calibre/db".into()], "cache.py"));
        assert!(!is_deferred(&["src/calibre/devices/kindle".into()], "driver.py"));
        assert!(!is_deferred(&["src/calibre/ebooks/mobi".into()], "utils.py"));
    }

    #[test]
    fn cluster_label_maps_known_clusters() {
        assert_eq!(cluster_label("db"), Some("cluster:db".into()));
        assert_eq!(cluster_label("calibre_ebooks"), Some("cluster:ebooks".into()));
        assert_eq!(cluster_label("unknown"), None);
    }
}
