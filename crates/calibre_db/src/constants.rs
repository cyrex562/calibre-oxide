pub const COVER_FILE_NAME: &str = "cover.jpg";
pub const METADATA_FILE_NAME: &str = "metadata.opf";
pub const DEFAULT_TRASH_EXPIRY_TIME_SECONDS: u64 = 14 * 86400;
pub const TRASH_DIR_NAME: &str = ".caltrash";
pub const NOTES_DIR_NAME: &str = ".calnotes";
pub const NOTES_DB_NAME: &str = "notes.db";
pub const DATA_DIR_NAME: &str = "data";
// DATA_FILE_PATTERN needs format! or similar if usage requires it, but constant strings are fine here.
// In Rust, we can't easily do f-string constants with runtime vars, but this looks static.
pub const DATA_FILE_PATTERN: &str = "data/**/*";
pub const BOOK_ID_PATH_TEMPLATE: &str = " ({})";
pub const RESOURCE_URL_SCHEME: &str = "calres";
pub const TEMPLATE_ICON_INDICATOR: &str = " template ";
pub const NO_SEARCH_LINK: &str = "__no_link__";

#[derive(Debug, Clone)]
pub struct TrashEntry {
    pub book_id: i32,
    pub title: String,
    pub author: String,
    pub cover_path: String,
    pub mtime: f64,
    pub formats: Vec<String>,
}
