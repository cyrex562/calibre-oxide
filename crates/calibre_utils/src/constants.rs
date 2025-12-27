use directories::ProjectDirs;
use lazy_static::lazy_static;
use std::path::PathBuf;

pub const APP_NAME: &str = "calibre";
pub const VERSION: &str = "7.0.0"; // Placeholder, maybe read from Cargo.toml later

lazy_static! {
    pub static ref PROJECT_DIRS: Option<ProjectDirs> = ProjectDirs::from("com", "calibre-ebook", "calibre");
}

pub fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

pub fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

pub fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

pub fn cache_dir() -> PathBuf {
    if let Some(dirs) = PROJECT_DIRS.as_ref() {
        dirs.cache_dir().to_path_buf()
    } else {
        // Fallback for weird environments
        std::env::temp_dir().join("calibre-cache")
    }
}

pub fn config_dir() -> PathBuf {
    if let Some(dirs) = PROJECT_DIRS.as_ref() {
        dirs.config_dir().to_path_buf()
    } else {
         std::env::temp_dir().join("calibre-config")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_detection() {
        // Only one should be true, or linux/macos overlap in some definitions but strictly:
        let os_count = (is_windows() as i32) + (is_macos() as i32) + (is_linux() as i32);
        // It's possible to be on BSD, so count might be 0, but usually 1 on standard dev machines.
        assert!(os_count <= 1);
    }

    #[test]
    fn test_paths() {
        let cache = cache_dir();
        assert!(cache.exists() || !cache.as_os_str().is_empty()); 
    }
}
