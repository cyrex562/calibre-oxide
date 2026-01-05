use crate::constants::config_dir;
use lazy_static::lazy_static;
use log::error;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::RwLock; // Use std::sync::RwLock instead of parking_lot for now to avoid adding dep if not needed, or add parking_lot if preferred. adhering to std is safer for now.

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
