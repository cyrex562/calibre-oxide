//! Carries the app's own data across the rename.
//!
//! Tauri keeps the app's settings, window state, logs and web-view storage in
//! a folder named after the bundle identifier. The identifier changed with
//! the project's name (`com.cyrex.calibre-oxide` to `com.cyrex.lexmechanic`),
//! so a user who upgrades would otherwise start with no recent libraries, no
//! remembered window size and an empty web-view store.
//!
//! On start, before Tauri resolves any of those folders, [`migrate_app_data`]
//! copies each old folder to the new name when the old one exists and the new
//! one does not. The old folder is left where it is: the old app may still be
//! installed (the installer's product name changed too, so the two sit side
//! by side), and nothing here is worth losing to a failed copy.

use std::io;
use std::path::{Path, PathBuf};

/// The folder name the app's data had before the rename.
pub const LEGACY_IDENTIFIER: &str = "com.cyrex.calibre-oxide";
/// The current bundle identifier (`identifier` in `tauri.conf.json`).
pub const IDENTIFIER: &str = "com.cyrex.lexmechanic";

/// Folders inside a web-view profile that are caches: rebuilt on demand, and
/// not worth copying.
const SKIPPED_DIRS: &[&str] = &["Cache", "Code Cache", "GPUCache", "GrShaderCache", "ShaderCache", "DawnCache", "Crashpad"];

/// What one [`migrate_dir`] call did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// There was no old folder.
    NothingToMigrate,
    /// The new folder already exists: it is used as it is.
    AlreadyMigrated,
    /// The old folder was copied; `skipped` files could not be copied (for
    /// example because the old app had them open) and were left out.
    Copied { files: usize, skipped: usize },
}

/// The folders Tauri places per-app data in on this platform: the roaming and
/// the local app data folders on Windows, the config and data folders
/// elsewhere. Each holds a `<identifier>` folder.
fn data_roots() -> Vec<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
    let mut roots = Vec::new();
    if cfg!(windows) {
        roots.extend(env("APPDATA"));
        roots.extend(env("LOCALAPPDATA"));
    } else if cfg!(target_os = "macos") {
        if let Some(home) = env("HOME") {
            roots.push(home.join("Library").join("Application Support"));
        }
    } else {
        let home = env("HOME");
        roots.extend(env("XDG_CONFIG_HOME").or_else(|| home.as_ref().map(|h| h.join(".config"))));
        roots.extend(env("XDG_DATA_HOME").or_else(|| home.as_ref().map(|h| h.join(".local").join("share"))));
    }
    roots
}

/// Runs the migration over every data root of this platform. Failures are
/// logged and never stop the app from starting.
pub fn migrate_app_data() {
    for root in data_roots() {
        match migrate_dir(&root.join(LEGACY_IDENTIFIER), &root.join(IDENTIFIER)) {
            Ok(Outcome::Copied { files, skipped }) => {
                eprintln!("copied {files} file(s) of the app's data from {LEGACY_IDENTIFIER} to {IDENTIFIER} ({skipped} skipped)")
            }
            Ok(_) => {}
            Err(e) => eprintln!("could not copy the app's data from {LEGACY_IDENTIFIER} to {IDENTIFIER} under {}: {e}", root.display()),
        }
    }
}

/// Copies `old` to `new` when `old` is a folder and `new` does not exist.
///
/// The copy goes to `<new>.migrating` and is renamed into place at the end, so
/// an interrupted copy never leaves a half-filled `new` that the next start
/// would take for the real thing.
pub fn migrate_dir(old: &Path, new: &Path) -> io::Result<Outcome> {
    if new.exists() {
        return Ok(Outcome::AlreadyMigrated);
    }
    if !old.is_dir() {
        return Ok(Outcome::NothingToMigrate);
    }
    let staging = {
        let mut name = new.file_name().unwrap_or_default().to_os_string();
        name.push(".migrating");
        new.with_file_name(name)
    };
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    let (mut files, mut skipped) = (0, 0);
    copy_tree(old, &staging, &mut files, &mut skipped)?;
    std::fs::rename(&staging, new)?;
    Ok(Outcome::Copied { files, skipped })
}

fn copy_tree(from: &Path, to: &Path, files: &mut usize, skipped: &mut usize) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if SKIPPED_DIRS.iter().any(|d| name.to_string_lossy().eq_ignore_ascii_case(d)) {
                continue;
            }
            copy_tree(&entry.path(), &target, files, skipped)?;
        } else if kind.is_file() {
            match std::fs::copy(entry.path(), &target) {
                Ok(_) => *files += 1,
                Err(_) => *skipped += 1,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn identifiers_match_the_bundle_config() {
        let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], IDENTIFIER);
    }

    #[test]
    fn copies_settings_and_nested_files_to_the_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        write(&old.join("settings.json"), r#"{"library_path":"/lib"}"#);
        write(&old.join("logs").join("server.log"), "hello");

        let outcome = migrate_dir(&old, &new).unwrap();

        assert_eq!(outcome, Outcome::Copied { files: 2, skipped: 0 });
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), r#"{"library_path":"/lib"}"#);
        assert_eq!(fs::read_to_string(new.join("logs").join("server.log")).unwrap(), "hello");
        assert!(old.join("settings.json").is_file(), "the old folder is left alone");
        assert!(!root.path().join(format!("{IDENTIFIER}.migrating")).exists(), "no staging folder is left behind");
    }

    #[test]
    fn an_existing_new_folder_is_never_overwritten() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        write(&old.join("settings.json"), "old");
        write(&new.join("settings.json"), "new");

        assert_eq!(migrate_dir(&old, &new).unwrap(), Outcome::AlreadyMigrated);
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), "new");
    }

    #[test]
    fn nothing_happens_without_an_old_folder() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        assert_eq!(migrate_dir(&old, &new).unwrap(), Outcome::NothingToMigrate);
        assert!(!new.exists());
    }

    #[test]
    fn it_runs_once() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        write(&old.join("settings.json"), "old");
        assert!(matches!(migrate_dir(&old, &new).unwrap(), Outcome::Copied { .. }));
        write(&new.join("settings.json"), "edited since");
        assert_eq!(migrate_dir(&old, &new).unwrap(), Outcome::AlreadyMigrated);
        assert_eq!(fs::read_to_string(new.join("settings.json")).unwrap(), "edited since");
    }

    #[test]
    fn web_view_caches_are_not_copied_but_its_storage_is() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        write(&old.join("EBWebView").join("Default").join("Local Storage").join("leveldb").join("000003.log"), "ls");
        write(&old.join("EBWebView").join("Default").join("Cache").join("data_0"), "cache");
        write(&old.join("EBWebView").join("Default").join("GPUCache").join("data_1"), "gpu");

        migrate_dir(&old, &new).unwrap();

        assert!(new.join("EBWebView/Default/Local Storage/leveldb/000003.log").is_file());
        assert!(!new.join("EBWebView/Default/Cache").exists());
        assert!(!new.join("EBWebView/Default/GPUCache").exists());
    }

    #[test]
    fn a_stale_staging_folder_from_an_interrupted_copy_is_replaced() {
        let root = tempfile::tempdir().unwrap();
        let (old, new) = (root.path().join(LEGACY_IDENTIFIER), root.path().join(IDENTIFIER));
        write(&old.join("settings.json"), "old");
        write(&root.path().join(format!("{IDENTIFIER}.migrating")).join("half.tmp"), "partial");

        assert!(matches!(migrate_dir(&old, &new).unwrap(), Outcome::Copied { files: 1, .. }));
        assert!(!new.join("half.tmp").exists());
    }
}
