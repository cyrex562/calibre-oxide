//! Placeholder registry: schema, scanner, JSONL I/O.
//!
//! A "placeholder" is a partially-implemented symbol whose real signature is
//! committed but whose body defers work. The convention is:
//!
//!     fn foo(&self) -> Result<Bar> {
//!         todo!("placeholder: <one-line reason>")
//!     }
//!
//! or, for macro-invisible sites:
//!
//!     // PLACEHOLDER: <one-line reason>
//!
//! The registry is `docs/placeholders.jsonl`, one entry per line, stable-sorted
//! by (crate, path, line) so diffs are readable.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Placeholder {
    pub crate_name: String,
    /// Repo-relative path.
    pub path: String,
    pub line: u32,
    pub kind: PlaceholderKind,
    pub reason: String,
    /// Optional pointer to the Python source being ported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<Priority>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlaceholderKind {
    /// `todo!("placeholder: ...")` body marker.
    TodoMacro,
    /// `unimplemented!("placeholder: ...")` body marker.
    UnimplementedMacro,
    /// `// PLACEHOLDER: ...` comment marker.
    Comment,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Low,
    Medium,
    High,
}

/// Scan the repo for placeholders. Only descends into `crates/` and
/// `tools/harness/` — Rust source only, no vendored trees.
pub fn scan(repo: &Path) -> Result<Vec<Placeholder>> {
    let mut out = Vec::new();
    for root_rel in &["crates", "tools/harness/src", "app/src"] {
        let root = repo.join(root_rel);
        if !root.exists() {
            continue;
        }
        scan_dir(&root, repo, &mut out)?;
    }
    out.sort_by(|a, b| {
        a.crate_name
            .cmp(&b.crate_name)
            .then(a.path.cmp(&b.path))
            .then(a.line.cmp(&b.line))
    });
    Ok(out)
}

fn scan_dir(dir: &Path, repo: &Path, out: &mut Vec<Placeholder>) -> Result<()> {
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        if is_self_referential(path) {
            continue;
        }
        scan_file(path, repo, out)?;
    }
    Ok(())
}

/// The scanner's own source file contains raw-string fixtures that syntactically
/// look like placeholder markers. Skip them — the line-based text scan can't
/// tell string-literal content from real code without a Rust AST, and the
/// harness's placeholders.rs never needs to be its own port target.
fn is_self_referential(path: &Path) -> bool {
    let s = path.to_string_lossy().replace('\\', "/");
    s.ends_with("tools/harness/src/placeholders.rs")
}

fn scan_file(file: &Path, repo: &Path, out: &mut Vec<Placeholder>) -> Result<()> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("read {:?}", file))?;
    let rel = file
        .strip_prefix(repo)
        .unwrap_or(file)
        .to_string_lossy()
        .replace('\\', "/");
    let crate_name = infer_crate_name(&rel);

    let mut skipper = ScopeSkipper::default();

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx as u32 + 1;
        let should_skip = skipper.observe(line);
        if should_skip {
            continue;
        }
        // Doc comments and inner doc comments never carry real placeholders.
        let trimmed = line.trim_start();
        if trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }

        if let Some(reason) = extract_marker(line, "todo!(\"placeholder:", "\")") {
            out.push(Placeholder {
                crate_name: crate_name.clone(),
                path: rel.clone(),
                line: lineno,
                kind: PlaceholderKind::TodoMacro,
                reason,
                python_ref: None,
                priority: None,
            });
        } else if let Some(reason) = extract_marker(line, "unimplemented!(\"placeholder:", "\")") {
            out.push(Placeholder {
                crate_name: crate_name.clone(),
                path: rel.clone(),
                line: lineno,
                kind: PlaceholderKind::UnimplementedMacro,
                reason,
                python_ref: None,
                priority: None,
            });
        } else if let Some(reason) = trimmed.strip_prefix("// PLACEHOLDER:") {
            out.push(Placeholder {
                crate_name: crate_name.clone(),
                path: rel.clone(),
                line: lineno,
                kind: PlaceholderKind::Comment,
                reason: reason.trim().to_string(),
                python_ref: None,
                priority: None,
            });
        }
    }
    Ok(())
}

/// Tracks `#[cfg(test)]` module scopes so we can skip everything inside
/// them. Naive brace-counting is fine because we only care about whether
/// we're inside a `#[cfg(test)] mod X { ... }` — not about nested lexers.
#[derive(Default)]
struct ScopeSkipper {
    cfg_test_pending: bool,
    depth: u32,
    skipping_depth: Option<u32>,
}

impl ScopeSkipper {
    /// Returns true if this line falls inside a `#[cfg(test)]` scope.
    fn observe(&mut self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(any(test") {
            self.cfg_test_pending = true;
        }

        let opens = line.chars().filter(|&c| c == '{').count() as i64;
        let closes = line.chars().filter(|&c| c == '}').count() as i64;

        let entering = self.cfg_test_pending && opens > 0;
        if entering {
            self.skipping_depth = Some(self.depth);
            self.cfg_test_pending = false;
        }

        // Update depth AFTER deciding "entering" so the opening line counts
        // as inside the scope.
        let net = opens - closes;
        if net >= 0 {
            self.depth = self.depth.saturating_add(net as u32);
        } else {
            self.depth = self.depth.saturating_sub((-net) as u32);
        }

        let inside = match self.skipping_depth {
            Some(d) if self.depth > d => true,
            Some(d) if self.depth <= d => {
                self.skipping_depth = None;
                // The closing line itself is still inside.
                closes > 0
            }
            _ => false,
        };

        inside
    }
}

fn extract_marker(line: &str, open: &str, close: &str) -> Option<String> {
    let start = line.find(open)? + open.len();
    let rest = &line[start..];
    let end = rest.rfind(close)?;
    Some(rest[..end].trim().to_string())
}

fn infer_crate_name(rel: &str) -> String {
    // rel is "crates/<name>/src/..." or "tools/harness/src/..." or "app/src/..."
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.first().copied() {
        Some("crates") | Some("tools") if parts.len() >= 2 => parts[1].to_string(),
        Some("app") => "app".to_string(),
        _ => "unknown".to_string(),
    }
}

pub fn write_registry(repo: &Path, placeholders: &[Placeholder]) -> Result<PathBuf> {
    let path = repo.join("docs/placeholders.jsonl");
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut f = std::fs::File::create(&tmp)
            .with_context(|| format!("create {:?}", tmp))?;
        for p in placeholders {
            serde_json::to_writer(&mut f, p)?;
            f.write_all(b"\n")?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn read_registry(repo: &Path) -> Result<Vec<Placeholder>> {
    let path = repo.join("docs/placeholders.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(&path)?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let entry: Placeholder = serde_json::from_str(line)
            .with_context(|| format!("parse line {}: {}", i + 1, line))?;
        out.push(entry);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_marker_isolates_reason() {
        let line = r#"        todo!("placeholder: needs Huffman table")"#;
        let got = extract_marker(line, "todo!(\"placeholder:", "\")").unwrap();
        assert_eq!(got, "needs Huffman table");
    }

    #[test]
    fn extract_marker_ignores_non_placeholder_todo() {
        let line = r#"        todo!("figure this out later")"#;
        assert!(extract_marker(line, "todo!(\"placeholder:", "\")").is_none());
    }

    #[test]
    fn scan_skips_doc_comments_and_cfg_test_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let crate_dir = repo.join("crates/example/src");
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(
            crate_dir.join("lib.rs"),
            "/// Example: todo!(\"placeholder: doc comment noise\")\n\
             //! Also: todo!(\"placeholder: inner doc noise\")\n\
             pub fn real() { todo!(\"placeholder: real one\") }\n\
             #[cfg(test)]\n\
             mod tests {\n\
                 #[test]\n\
                 fn t() { todo!(\"placeholder: test noise\"); }\n\
             }\n",
        )
        .unwrap();
        let found = scan(repo).unwrap();
        assert_eq!(found.len(), 1, "found = {:#?}", found);
        assert_eq!(found[0].reason, "real one");
    }

    #[test]
    fn scan_finds_all_three_marker_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        let crate_dir = repo.join("crates/example/src");
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        std::fs::write(
            crate_dir.join("lib.rs"),
            r#"pub fn a() { todo!("placeholder: reason A") }
pub fn b() { unimplemented!("placeholder: reason B") }
// PLACEHOLDER: reason C
pub fn c() {}
"#,
        )
        .unwrap();

        let found = scan(repo).unwrap();
        assert_eq!(found.len(), 3, "found = {:?}", found);
        assert_eq!(found[0].kind, PlaceholderKind::TodoMacro);
        assert_eq!(found[0].reason, "reason A");
        assert_eq!(found[1].kind, PlaceholderKind::UnimplementedMacro);
        assert_eq!(found[1].reason, "reason B");
        assert_eq!(found[2].kind, PlaceholderKind::Comment);
        assert_eq!(found[2].reason, "reason C");
        assert_eq!(found[0].crate_name, "example");
    }

    #[test]
    fn roundtrip_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();
        std::fs::create_dir_all(repo.join("docs")).unwrap();
        let entries = vec![Placeholder {
            crate_name: "calibre_db".to_string(),
            path: "crates/calibre_db/src/foo.rs".to_string(),
            line: 42,
            kind: PlaceholderKind::TodoMacro,
            reason: "needs schema info".to_string(),
            python_ref: Some("old_src/src/calibre/db/foo.py".to_string()),
            priority: Some(Priority::Medium),
        }];
        write_registry(repo, &entries).unwrap();
        let round = read_registry(repo).unwrap();
        assert_eq!(round, entries);
    }
}
