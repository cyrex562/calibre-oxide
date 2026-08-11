//! Resolve the repo root regardless of where the harness is invoked from.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

pub fn resolve_repo_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return canonicalize_and_check(p);
    }

    // Try the current working directory upward.
    let cwd = std::env::current_dir().context("failed to get CWD")?;
    for candidate in cwd.ancestors() {
        if is_repo_root(candidate) {
            return Ok(candidate.to_path_buf());
        }
    }

    // Fall back to CARGO_MANIFEST_DIR at build time (this harness lives at
    // tools/harness relative to the workspace root).
    let manifest = env!("CARGO_MANIFEST_DIR");
    let derived = PathBuf::from(manifest).join("../..").canonicalize()?;
    if is_repo_root(&derived) {
        return Ok(derived);
    }

    Err(anyhow!(
        "could not locate repo root from CWD {:?} or CARGO_MANIFEST_DIR {}",
        cwd,
        manifest
    ))
}

fn canonicalize_and_check(p: &Path) -> Result<PathBuf> {
    let c = p.canonicalize()
        .with_context(|| format!("failed to canonicalize {:?}", p))?;
    if !is_repo_root(&c) {
        return Err(anyhow!("{:?} does not look like the calibre-oxide repo root", c));
    }
    Ok(c)
}

fn is_repo_root(p: &Path) -> bool {
    p.join("Cargo.toml").is_file() && p.join("crates").is_dir() && p.join("docs").is_dir()
}
