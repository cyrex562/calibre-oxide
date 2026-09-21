//! Persisted app settings -- the last-opened library path plus a
//! bounded "recent libraries" list, stored as a plain JSON file under
//! this app's own config directory rather than pulling in a
//! settings-store plugin for two small fields.
//!
//! # Real architecture decision (issue #725)
//!
//! This app is, and remains, single-library-per-running-instance: one
//! spawned `calibre_srv` process serves exactly one library at a time
//! (see `open_library` in `lib.rs`). There is no concurrent
//! multi-library serving here, and none is planned -- `calibre_srv`'s
//! own `LibraryBroker`/`AppState.libraries` broker mode exists in the
//! crate (used by a handful of `calibre_srv` routes, e.g.
//! `cdb::copy_to_library`) but has no CLI flag wiring it up in the
//! real shipped `calibre_srv` binary today, and this app doesn't add
//! one. "Switching libraries" here means: kill the current
//! `calibre_srv` child, spawn a fresh one against a different library
//! path, and re-navigate the window -- exactly what `open_library`
//! already does for the very first library choice, just re-triggered
//! for a previously-used path instead of a freshly-picked one. This
//! recent-libraries list is what makes that a one-click action instead
//! of re-running the folder-picker dialog every time.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Recent-libraries list is capped here, not left unbounded -- a
/// small, fixed number of one-click "switch to" targets is the real
/// value; growing this indefinitely as libraries get opened over
/// months would turn the picker into a stale-path graveyard instead.
const MAX_RECENT: usize = 8;

fn default_auto_reopen() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
struct Settings {
    library_path: Option<PathBuf>,
    #[serde(default)]
    recent_libraries: Vec<PathBuf>,
    /// Whether to auto-reopen `library_path` on launch (issue #721).
    /// Defaults to `true` -- this is the behavior every version of
    /// this app has always had before this setting existed, so an
    /// old `settings.json` with no such key must keep behaving the
    /// same way rather than silently stop reopening.
    #[serde(default = "default_auto_reopen")]
    auto_reopen: bool,
    /// A folder watched for new books (#816 item 4.4). `None` means
    /// the feature is off, which is the default -- a background
    /// process that moves files around is not something to enable
    /// without being asked.
    #[serde(default)]
    auto_add_folder: Option<PathBuf>,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings { library_path: None, recent_libraries: Vec::new(), auto_reopen: true, auto_add_folder: None }
    }
}

fn settings_path(app: &AppHandle) -> std::io::Result<PathBuf> {
    let dir = app.path().app_config_dir().map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("settings.json"))
}

fn read(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else { return Settings::default() };
    let Ok(raw) = std::fs::read_to_string(&path) else { return Settings::default() };
    serde_json::from_str(&raw).unwrap_or_default()
}

pub fn load_library_path(app: &AppHandle) -> Option<PathBuf> {
    read(app).library_path
}

/// Most-recently-opened first, deduplicated, capped at
/// [`MAX_RECENT`]. Includes the currently-open library (matching how
/// a real "recent files" menu always lists the current document too).
pub fn list_recent_libraries(app: &AppHandle) -> Vec<PathBuf> {
    read(app).recent_libraries
}

/// Moves `path` to the front of `list` (removing any earlier
/// occurrence first, so re-opening an already-recent library
/// reorders rather than duplicates it), then truncates to `cap`.
/// Pure and separately unit-tested since the rest of this module
/// needs a real `AppHandle` (a real app config directory) that isn't
/// available in this crate's test environment.
fn push_recent(list: &mut Vec<PathBuf>, path: PathBuf, cap: usize) {
    list.retain(|p| p != &path);
    list.insert(0, path);
    list.truncate(cap);
}

pub fn save_library_path(app: &AppHandle, library_path: &Path) -> std::io::Result<()> {
    let mut settings = read(app);
    settings.library_path = Some(library_path.to_path_buf());
    push_recent(&mut settings.recent_libraries, library_path.to_path_buf(), MAX_RECENT);
    let path = settings_path(app)?;
    std::fs::write(path, serde_json::to_vec_pretty(&settings)?)
}

pub fn get_auto_reopen(app: &AppHandle) -> bool {
    read(app).auto_reopen
}

pub fn set_auto_reopen(app: &AppHandle, enabled: bool) -> std::io::Result<()> {
    let mut settings = read(app);
    settings.auto_reopen = enabled;
    let path = settings_path(app)?;
    std::fs::write(path, serde_json::to_vec_pretty(&settings)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_recent_moves_an_existing_entry_to_the_front_instead_of_duplicating_it() {
        let mut list = vec![PathBuf::from("/a"), PathBuf::from("/b"), PathBuf::from("/c")];
        push_recent(&mut list, PathBuf::from("/b"), 8);
        assert_eq!(list, vec![PathBuf::from("/b"), PathBuf::from("/a"), PathBuf::from("/c")]);
    }

    #[test]
    fn an_old_settings_json_with_no_auto_reopen_key_defaults_to_true() {
        // Real regression guard: `auto_reopen` was added after
        // `library_path`/`recent_libraries` already shipped -- an
        // existing user's settings.json on disk has no such key at
        // all, and must keep reopening automatically (its only
        // behavior before this setting existed) rather than silently
        // stop.
        let settings: Settings = serde_json::from_str(r#"{"library_path": "/some/lib", "recent_libraries": []}"#).unwrap();
        assert!(settings.auto_reopen);
    }

    #[test]
    fn push_recent_truncates_to_the_cap() {
        // List order is already most-recent-first (as `save_library_path`
        // always maintains it) -- pushing a new entry keeps the front of
        // that existing order and drops the oldest tail beyond `cap`.
        let mut list: Vec<PathBuf> = (0..5).map(|i| PathBuf::from(format!("/lib{i}"))).collect();
        push_recent(&mut list, PathBuf::from("/new"), 3);
        assert_eq!(list, vec![PathBuf::from("/new"), PathBuf::from("/lib0"), PathBuf::from("/lib1")]);
    }
}

/// The watched auto-add folder, if one is configured.
pub fn get_auto_add_folder(app: &AppHandle) -> Option<PathBuf> {
    read(app).auto_add_folder
}

/// Sets or clears the watched folder.
pub fn set_auto_add_folder(app: &AppHandle, folder: Option<PathBuf>) -> std::io::Result<()> {
    let path = settings_path(app)?;
    let mut settings = read(app);
    settings.auto_add_folder = folder;
    std::fs::write(path, serde_json::to_vec_pretty(&settings)?)
}
