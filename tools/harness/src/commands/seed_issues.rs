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

    let module_issues = collect_module_port_issues(repo)?;
    let placeholder_items = collect_placeholder_items(repo)?;

    let existing: BTreeSet<String> = gh::list_issues(&slug, &["--state", "all"])?
        .into_iter()
        .map(|i| i.title)
        .collect();

    let all: Vec<PlannedIssue> = module_issues
        .iter()
        .chain(placeholder_items.iter())
        .cloned()
        .collect();

    let mut deferred = 0usize;
    let mut already_exists = 0usize;
    let mut filtered_by_only = 0usize;
    let mut planned: Vec<PlannedIssue> = Vec::new();
    for item in &all {
        if let Some(only) = &args.only {
            if item.cluster != *only {
                filtered_by_only += 1;
                continue;
            }
        }
        if !args.include_deferred && item.labels.iter().any(|l| l == "deferred") {
            deferred += 1;
            continue;
        }
        if existing.contains(&item.title) {
            already_exists += 1;
            continue;
        }
        planned.push(item.clone());
    }

    println!(
        "planned to create: {} issues  (of {} total; skipped {} deferred, {} already exist, {} filtered by --only)",
        planned.len(), all.len(), deferred, already_exists, filtered_by_only
    );

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

/// Group unchecked files by their innermost module (the deepest heading
/// they sit under in modules_to_port.md). One issue per module, with the
/// individual files as a checklist in the body. Aggressively filter out
/// noise files (`__init__.py`, `.ui`, `.sip`, standalone header pairs).
fn collect_module_port_issues(repo: &Path) -> Result<Vec<PlannedIssue>> {
    use std::collections::BTreeMap;

    let path = repo.join("docs/modules_to_port.md");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {:?}", path))?;

    // Group key = the full heading stack, e.g. "ebooks / mobi / reader".
    // Values = list of file names + whether the whole module is deferred.
    #[derive(Default)]
    struct Bucket {
        files: Vec<String>,
        cluster: String,
        deferred: bool,
    }
    let mut buckets: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut heading_stack: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim_end();
        // ## is the top-level Python package heading, e.g. "## src/pyj" or
        // "## src/calibre". Capture so we can defer whole sub-trees.
        if let Some(rest) = line.strip_prefix("## ") {
            heading_stack.clear();
            heading_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("### ") {
            heading_stack.truncate(1);
            heading_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("#### ") {
            heading_stack.truncate(2);
            heading_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("##### ") {
            heading_stack.truncate(3);
            heading_stack.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("###### ") {
            heading_stack.truncate(4);
            heading_stack.push(rest.trim().to_string());
        }

        let Some(rest) = line.trim_start().strip_prefix("- [ ] ") else {
            continue;
        };
        let file = rest.split_whitespace().next().unwrap_or("").trim_matches('`').to_string();
        if file.is_empty() || is_noise_file(&file) {
            continue;
        }
        let key = heading_stack.join(" / ");
        let bucket = buckets.entry(key.clone()).or_default();
        bucket.files.push(file.clone());
        if bucket.cluster.is_empty() {
            bucket.cluster = infer_cluster(&heading_stack);
        }
        if is_deferred(&heading_stack, &file) {
            // A single deferred file inside an otherwise-active module means
            // the module carries an active `port` label and the file
            // becomes a follow-up. But if EVERY file in the module is
            // deferred, mark the module itself deferred. We finalize after
            // grouping.
        }
    }

    // Second pass: decide deferred at the module level.
    for (key, bucket) in buckets.iter_mut() {
        let stack: Vec<String> = key.split(" / ").map(str::to_string).collect();
        bucket.deferred = bucket.files.iter().all(|f| is_deferred(&stack, f));
    }

    let mut items = Vec::new();
    for (key, bucket) in buckets {
        if bucket.files.is_empty() {
            continue;
        }
        let mut labels = vec!["port".to_string()];
        if let Some(l) = cluster_label(&bucket.cluster) {
            labels.push(l);
        }
        if bucket.deferred {
            labels.push("deferred".to_string());
        }
        let title = format!("port module: {} ({} file{})",
            key, bucket.files.len(), if bucket.files.len() == 1 { "" } else { "s" });
        let mut body = String::new();
        body.push_str(&format!("**Cluster**: {}\n**Module path (from modules_to_port.md)**: `{}`\n\n", bucket.cluster, key));
        body.push_str("Port every file below to Rust. Follow:\n\
                       - `docs/AGENT_PORTING_GUIDE.md`\n\
                       - `docs/FAULT_TOLERANCE.md`\n\n\
                       Definition of done:\n\
                       - Real signatures. `todo!(\"placeholder: ...\")` bodies only where genuinely blocked.\n\
                       - Unit tests exercising public API.\n\
                       - Cross-validation test if the format is round-trippable (see docs/HARNESS.md).\n\
                       - Mark each checkbox below AND in `docs/modules_to_port.md`.\n\n\
                       **Files:**\n");
        for f in &bucket.files {
            body.push_str(&format!("- [ ] `{}`\n", f));
        }
        items.push(PlannedIssue {
            title,
            body,
            labels,
            cluster: bucket.cluster,
        });
    }
    Ok(items)
}

/// Files that don't need dedicated issues. `__init__.py` is almost always
/// a re-export shim that gets folded into whichever real file needs it.
/// `.ui` files are Qt Designer XML we're not porting. `.sip` is
/// PyQt-specific.
fn is_noise_file(file: &str) -> bool {
    file == "__init__.py"
        || file.ends_with(".ui")
        || file.ends_with(".sip")
        || file.ends_with(".rst")
        || file.ends_with(".txt")
        || file == "TODO"
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
    // Stack is [top-package, subpackage, ...]. Top is "src/calibre" or
    // "src/pyj" or "src/perfect-hashing" etc. Cluster is the second entry
    // (e.g. "db", "ebooks", "devices"), or falls back to the top-package
    // basename.
    let cluster = stack.get(1).map(String::as_str).unwrap_or("").to_lowercase();
    let top = stack.first().map(String::as_str).unwrap_or("").to_lowercase();

    if cluster.starts_with("db") { "db".into() }
    else if cluster.starts_with("devices") { "devices".into() }
    else if cluster.starts_with("ebooks") { "ebooks".into() }
    else if cluster.starts_with("gui2") { "gui".into() }
    else if cluster.starts_with("srv") { "srv".into() }
    else if cluster.starts_with("utils") { "utils".into() }
    else if cluster.starts_with("conversion") { "conversion".into() }
    else if !cluster.is_empty() { cluster }
    else if !top.is_empty() { top.replace('/', "-") }
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
    if full.contains("pyj") {
        return true;
    }
    // qt bindings — not needed with Tauri.
    if full.contains("src/qt") || full.split('/').any(|s| s == "qt") {
        return true;
    }
    // Third-party header-only C++ libraries embedded in the tree.
    if full.contains("perfect-hashing") || full.contains("frozen") {
        return true;
    }
    // The Qt-based ebook viewer — the new viewer is a Tauri panel.
    if full.split('/').any(|s| s == "viewer") {
        return true;
    }
    // Headless Qt platform integration — Tauri owns windowing.
    if full.split('/').any(|s| s == "headless") {
        return true;
    }
    // Legacy library backend (database2.py etc.) — replaced by calibre_db.
    if full.split('/').any(|s| s == "library") && !full.contains("catalogs") {
        // catalogs stay in scope (they generate output)
        // library itself is old Python API — defer.
        // (keep this narrower than the raw "library" match)
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
        if full.split('/').any(|s| s == name) {
            return true;
        }
    }
    // LRF renderer / obscure Palm/Psion formats.
    let obscure_formats = ["lrf", "haodoo", "plucker", "ztxt", "azw4"];
    for name in obscure_formats {
        if full.split('/').any(|s| s == name) {
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
    fn noise_file_matches_expected() {
        assert!(is_noise_file("__init__.py"));
        assert!(is_noise_file("main.ui"));
        assert!(is_noise_file("QProgressIndicator.sip"));
        assert!(is_noise_file("README.txt"));
        assert!(is_noise_file("tbs_periodicals.rst"));
        assert!(is_noise_file("TODO"));
        assert!(!is_noise_file("cache.py"));
        assert!(!is_noise_file("libusb.c"));
    }

    #[test]
    fn deferred_scope_matches_expected() {
        assert!(is_deferred(&["src/calibre".into(), "gui2".into(), "actions".into()], "add.py"));
        assert!(is_deferred(&["src/calibre".into(), "devices".into(), "binatone".into()], "driver.py"));
        assert!(is_deferred(&["src/calibre".into(), "devices".into(), "nook".into()], "driver.py"));
        assert!(is_deferred(&["src/pyj".into(), "book_list".into()], "add.pyj"));
        assert!(is_deferred(&["src/pyj".into(), "read_book".into()], "cfi.pyj"));
        assert!(is_deferred(&["src/perfect-hashing".into(), "frozen".into()], "map.h"));
        assert!(is_deferred(&["src/calibre".into(), "gui2".into(), "viewer".into()], "main.py"));
        assert!(is_deferred(&["src/calibre".into(), "headless".into()], "main.cpp"));
        assert!(is_deferred(&["src/qt".into()], "core.py"));

        assert!(!is_deferred(&["src/calibre".into(), "db".into()], "cache.py"));
        assert!(!is_deferred(&["src/calibre".into(), "devices".into(), "kindle".into()], "driver.py"));
        assert!(!is_deferred(&["src/calibre".into(), "ebooks".into(), "mobi".into()], "utils.py"));
    }

    #[test]
    fn cluster_label_maps_known_clusters() {
        assert_eq!(cluster_label("db"), Some("cluster:db".into()));
        assert_eq!(cluster_label("calibre_ebooks"), Some("cluster:ebooks".into()));
        assert_eq!(cluster_label("unknown"), None);
    }
}
