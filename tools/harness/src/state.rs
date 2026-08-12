//! Harness state file (.harness/state.json).
//!
//! Atomically written via temp+rename so a crash mid-write can't corrupt it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HarnessState {
    #[serde(default)]
    pub in_flight: Vec<InFlight>,
    #[serde(default)]
    pub green_judged_prs: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sweep: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seed: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InFlight {
    pub issue: u64,
    pub branch: String,
    pub worktree: PathBuf,
    pub started: chrono::DateTime<chrono::Utc>,
}

pub fn state_path(repo: &Path) -> PathBuf {
    repo.join(".harness/state.json")
}

pub fn load(repo: &Path) -> Result<HarnessState> {
    let path = state_path(repo);
    if !path.exists() {
        return Ok(HarnessState::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("read {:?}", path))?;
    Ok(serde_json::from_str(&text)?)
}

#[allow(dead_code)] // consumed by `run` / `sweep` in follow-up PRs
pub fn save(repo: &Path, state: &HarnessState) -> Result<()> {
    let path = state_path(repo);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        let text = serde_json::to_string_pretty(state)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_default_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let s = load(tmp.path()).unwrap();
        assert!(s.in_flight.is_empty());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = HarnessState::default();
        s.green_judged_prs.push(42);
        save(tmp.path(), &s).unwrap();
        let round = load(tmp.path()).unwrap();
        assert_eq!(round.green_judged_prs, vec![42]);
    }
}
