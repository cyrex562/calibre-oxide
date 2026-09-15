//! Persisted app settings -- currently just the last-opened library
//! path, stored as a plain JSON file under this app's own config
//! directory rather than pulling in a settings-store plugin for one
//! field.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    library_path: Option<PathBuf>,
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

pub fn save_library_path(app: &AppHandle, library_path: &Path) -> std::io::Result<()> {
    let mut settings = read(app);
    settings.library_path = Some(library_path.to_path_buf());
    let path = settings_path(app)?;
    std::fs::write(path, serde_json::to_vec_pretty(&settings)?)
}
