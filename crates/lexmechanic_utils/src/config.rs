use crate::constants::config_dir;
use lazy_static::lazy_static;
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock; // Use std::sync::RwLock instead of parking_lot for now to avoid adding dep if not needed, or add parking_lot if preferred. adhering to std is safer for now.

fn default_series_index_auto_increment() -> String {
    "next".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GlobalPrefs {
    pub database_path: String,
    pub filename_pattern: String,
    pub isbndb_com_key: String,
    pub network_timeout: u64,
    pub library_path: Option<String>,
    pub language: Option<String>,
    pub output_format: String,
    pub input_format_order: Vec<String>,
    pub read_file_metadata: bool,
    pub worker_process_priority: String,
    pub swap_author_names: bool,
    pub add_formats_to_existing: bool,
    pub check_for_dupes_on_ctl: bool,
    pub installation_uuid: Option<String>,
    pub new_book_tags: Vec<String>,
    pub mark_new_books: bool,
    pub saved_searches: HashMap<String, String>,
    pub user_categories: HashMap<String, String>, // definition in python says dict, assuming string->string
    pub manage_device_metadata: String,
    pub limit_search_columns: bool,
    pub limit_search_columns_to: Vec<String>,
    pub use_primary_find_in_search: bool,
    pub case_sensitive: bool,
    pub numeric_collation: bool,
    pub migrated: bool,
    /// Port of the `series_index_auto_increment` *tweak* (a separate,
    /// lower-level config system from `prefs` upstream -- this crate
    /// doesn't distinguish the two, so it lives here). One of `"next"`
    /// (default)/`"first_free"`/`"next_free"`/`"last_free"`, or a
    /// fixed number as a string (e.g. `"1"`); see
    /// [`crate::series::get_next_series_num_for_list`].
    /// `#[serde(default)]` so an existing saved config file (from
    /// before this field existed) still loads instead of falling all
    /// the way back to `GlobalPrefs::default()` for every field.
    #[serde(default = "default_series_index_auto_increment")]
    pub series_index_auto_increment: String,
}

impl Default for GlobalPrefs {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let default_db_path = home.join("library1.db").to_string_lossy().to_string();

        GlobalPrefs {
            database_path: default_db_path,
            filename_pattern: "(?P<title>.+) - (?P<author>[^_]+)".to_string(),
            isbndb_com_key: "".to_string(),
            network_timeout: 5,
            library_path: None,
            language: None,
            output_format: "EPUB".to_string(),
            input_format_order: vec![
                "EPUB", "AZW3", "MOBI", "LIT", "PRC", "FB2", "HTML", "HTM", "XHTM", "SHTML",
                "XHTML", "ZIP", "DOCX", "ODT", "RTF", "PDF", "TXT",
            ]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
            read_file_metadata: true,
            worker_process_priority: "normal".to_string(),
            swap_author_names: false,
            add_formats_to_existing: false,
            check_for_dupes_on_ctl: false,
            installation_uuid: None, // Will be generated if None
            new_book_tags: vec![],
            mark_new_books: false,
            saved_searches: HashMap::new(),
            user_categories: HashMap::new(),
            manage_device_metadata: "manual".to_string(),
            limit_search_columns: false,
            limit_search_columns_to: vec!["title", "authors", "tags", "series", "publisher"]
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
            use_primary_find_in_search: true,
            case_sensitive: false,
            numeric_collation: false,
            migrated: false,
            series_index_auto_increment: "next".to_string(),
        }
    }
}

pub struct Config {
    prefs: RwLock<GlobalPrefs>,
    file_path: PathBuf,
}

impl Config {
    pub fn new() -> Self {
        let config_dir = config_dir();
        // Ensure config dir exists
        if let Err(e) = fs::create_dir_all(&config_dir) {
            error!("Failed to create config directory: {}", e);
        }
        let file_path = config_dir.join("global.py.json");
        let mut prefs = GlobalPrefs::default();

        if file_path.exists() {
            match Self::load_from_file(&file_path) {
                Ok(loaded_prefs) => prefs = loaded_prefs,
                Err(e) => error!("Failed to load config from {:?}: {}", file_path, e),
            }
        }

        // Initialize UUID if missing
        if prefs.installation_uuid.is_none() {
            prefs.installation_uuid = Some(uuid::Uuid::new_v4().to_string());
            // We should save here, but we can't easily do it in 'new' without potential side effects or error handling.
            // Ideally we save immediately or on first modification.
            // For now, let's try to save it back.
            if let Err(e) = Self::save_to_file(&file_path, &prefs) {
                error!("Failed to save generated UUID to config: {}", e);
            }
        }

        Config {
            prefs: RwLock::new(prefs),
            file_path,
        }
    }

    fn load_from_file(path: &Path) -> anyhow::Result<GlobalPrefs> {
        let content = fs::read_to_string(path)?;
        let mut prefs: GlobalPrefs = serde_json::from_str(&content)?;

        // Merge with defaults for missing fields (partial update logic)
        // serde_json doesn't do this automatically for missing fields in struct unless we use default attribute on every field or custom deserializer.
        // For simplicity, we just trust serde defaults or if we want robust merging we need a more complex strategy.
        // However, serde_json::from_str will fail if required fields are missing in the JSON but present in the struct,
        // unless we used `#[serde(default)]` on the struct fields.
        // Let's rely on serde for now, but improving this to handle schema evolution is a future task.
        Ok(prefs)
    }

    fn save_to_file(path: &Path, prefs: &GlobalPrefs) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(prefs)?;
        // Atomic write
        let tmp_path = path.with_extension("tmp");
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(tmp_path, path)?;
        Ok(())
    }

    pub fn get_prefs(&self) -> GlobalPrefs {
        self.prefs.read().unwrap().clone()
    }

    pub fn update_prefs<F>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce(&mut GlobalPrefs),
    {
        let mut prefs = self.prefs.write().unwrap();
        f(&mut *prefs);
        Self::save_to_file(&self.file_path, &prefs)
    }
}

lazy_static! {
    pub static ref CONFIG: Config = Config::new();
}

/// A named, string-keyed, JSON-backed value store.
///
/// Port of `calibre.utils.config.DynamicConfig` -- an open-ended
/// key/value config used for things that don't have a fixed schema
/// (e.g. `oeb::iterator::book::EbookIterator`'s per-book bookmark
/// blobs, keyed as `"bookmarks_" + pathtoebook`). Each `name` gets its
/// own file, `<config_dir>/<name>.json`, holding a flat `{key: value}`
/// object.
///
/// Every value in this store, at least for the `"iterator"` instance
/// this crate uses it for, is itself already a serialized string (the
/// bookmark blob produced by `serialize_bookmarks`), so unlike
/// `GlobalPrefs` there's no typed-field schema to maintain: this is
/// `HashMap<String, String>`, matching Python's untyped `dict` usage of
/// `DynamicConfig`.
///
/// Writes follow the same write-temp/fsync/rename discipline as
/// [`Config::save_to_file`] (see `docs/FAULT_TOLERANCE.md` §2): never a
/// direct in-place `fs::write`, so a crash mid-write leaves either the
/// old file or the new one, never a truncated one.
pub struct DynamicConfig {
    file_path: PathBuf,
    data: RwLock<HashMap<String, String>>,
}

impl DynamicConfig {
    /// Open (or create) the named store under the standard config
    /// directory. A missing or unreadable/corrupt file is treated as an
    /// empty store rather than an error -- matching Python's
    /// `DynamicConfig`, which silently starts fresh if the JSON file is
    /// absent or fails to parse.
    pub fn new(name: &str) -> Self {
        let dir = config_dir();
        if let Err(e) = fs::create_dir_all(&dir) {
            error!("Failed to create config directory: {}", e);
        }
        let file_path = dir.join(format!("{name}.json"));
        let data = match fs::read_to_string(&file_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
        DynamicConfig {
            file_path,
            data: RwLock::new(data),
        }
    }

    /// Open a store at an explicit file path, bypassing the standard
    /// config directory. Used by tests so they don't touch the user's
    /// real config directory.
    pub fn at_path(file_path: PathBuf) -> Self {
        let data = match fs::read_to_string(&file_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        };
        DynamicConfig {
            file_path,
            data: RwLock::new(data),
        }
    }

    /// `self.config[key]` in Python -- `None` if absent (Python's
    /// `DynamicConfig.get` default is `None` too).
    pub fn get(&self, key: &str) -> Option<String> {
        self.data
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
    }

    /// `self.config[key] = value` in Python. Persists immediately
    /// (write-temp/fsync/rename), matching `DynamicConfig.__setitem__`,
    /// which writes the whole file back out on every assignment.
    pub fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
        let mut guard = self.data.write().unwrap_or_else(|e| e.into_inner());
        guard.insert(key.to_string(), value.to_string());
        Self::save_to_file(&self.file_path, &guard)
    }

    fn save_to_file(path: &Path, data: &HashMap<String, String>) -> anyhow::Result<()> {
        let content = serde_json::to_string_pretty(data)?;
        let tmp_path = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        {
            let mut file = fs::File::create(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, path)?;
        if let Some(parent) = path.parent() {
            if let Ok(dir) = fs::File::open(parent) {
                // Best-effort: not every platform/filesystem supports
                // fsync on a directory handle (notably Windows), and
                // failure here doesn't leave anything corrupt -- the
                // rename above already landed.
                let _ = dir.sync_all();
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod dynamic_config_tests {
    use super::*;

    #[test]
    fn get_missing_key_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DynamicConfig::at_path(dir.path().join("iterator.json"));
        assert_eq!(cfg.get("nope"), None);
    }

    #[test]
    fn set_then_get_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DynamicConfig::at_path(dir.path().join("iterator.json"));
        cfg.set("bookmarks_/tmp/book.epub", "hello\nworld\n")
            .unwrap();
        assert_eq!(
            cfg.get("bookmarks_/tmp/book.epub").as_deref(),
            Some("hello\nworld\n")
        );
    }

    #[test]
    fn reopening_loads_persisted_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iterator.json");
        {
            let cfg = DynamicConfig::at_path(path.clone());
            cfg.set("k", "v").unwrap();
        }
        let cfg2 = DynamicConfig::at_path(path);
        assert_eq!(cfg2.get("k").as_deref(), Some("v"));
    }

    #[test]
    fn missing_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = DynamicConfig::at_path(dir.path().join("does_not_exist.json"));
        assert_eq!(cfg.get("anything"), None);
    }
}
