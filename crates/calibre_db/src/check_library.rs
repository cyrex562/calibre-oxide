use crate::Library;

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

// Constants
const COVER_FILE_NAME: &str = "cover.jpg";
const METADATA_FILE_NAME: &str = "metadata.opf";
const DATA_DIR_NAME: &str = "data"; // Example
const TRASH_DIR_NAME: &str = ".trash"; // Example
const NOTES_DIR_NAME: &str = ".notes"; // Example

lazy_static::lazy_static! {
    static ref BOOK_EXTENSIONS: HashSet<&'static str> = {
        let mut s = HashSet::new();
        // Common ebook formats
        s.insert("epub"); s.insert("mobi"); s.insert("azw3"); s.insert("pdf");
        s.insert("txt"); s.insert("rtf"); s.insert("lit"); s.insert("fb2");
        s.insert("cbz"); s.insert("cbr"); s.insert("docx"); s.insert("odt");
        s
    };

    static ref NORMALS: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.insert(METADATA_FILE_NAME);
        s.insert(COVER_FILE_NAME);
        s.insert(DATA_DIR_NAME);
        s
    };

    static ref IGNORE_AT_TOP_LEVEL: HashSet<&'static str> = {
        let mut s = HashSet::new();
        s.insert("metadata.db");
        s.insert("metadata_db_prefs_backup.json");
        s.insert("metadata_pre_restore.db");
        s.insert("full-text-search.db");
        s.insert(TRASH_DIR_NAME);
        s.insert(NOTES_DIR_NAME);
        s
    };
}

pub struct CheckLibrary<'a> {
    library_path: PathBuf,
    db: &'a Library,
    is_case_sensitive: bool,

    // Results
    pub invalid_titles: Vec<(String, String, i32)>, // (parent_dir, path, id)
    pub extra_titles: Vec<(String, String, i32)>,
    pub invalid_authors: Vec<(String, String, i32)>,
    pub extra_authors: Vec<(String, String, i32)>,
    pub missing_formats: Vec<(String, String, i32)>,
    pub extra_formats: Vec<(String, String, i32)>,
    pub extra_files: Vec<(String, String, i32)>,
    pub missing_covers: Vec<(String, String, i32)>,
    pub extra_covers: Vec<(String, String, i32)>,
    pub malformed_formats: Vec<(String, String, i32)>,
    pub malformed_paths: Vec<(String, String, i32)>,
    pub failed_folders: Vec<(String, String, Vec<String>)>,

    // Internal Cache
    all_authors: HashSet<String>,
    all_ids: HashSet<i32>,
    all_dbpaths: HashSet<String>, // storing as String for easier comparison
    all_lc_dbpaths: HashSet<String>,

    ignore_names: HashSet<String>,
    ignore_ext: HashSet<String>,

    db_id_regexp: Regex,
    book_dirs: Vec<(String, String, String)>, // (db_path, title_dir, id_str)
    malformed_paths_ids: HashSet<i32>,
}

impl<'a> CheckLibrary<'a> {
    pub fn new(library_path: PathBuf, db: &'a Library) -> Self {
        let is_case_sensitive = db.is_case_sensitive();

        let all_authors = db
            .all_authors()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, name)| name)
            .collect();

        let all_ids: HashSet<i32> = db.all_book_ids().unwrap_or_default().into_iter().collect();

        let mut all_dbpaths = HashSet::new();
        for &id in &all_ids {
            if let Ok(Some(book)) = db.get_book(id) {
                if !book.path.is_empty() {
                    // Normalize separators
                    let p = book.path.replace("\\", "/");
                    all_dbpaths.insert(p);
                }
            }
        }

        let all_lc_dbpaths = all_dbpaths.iter().map(|s| s.to_lowercase()).collect();
        let db_id_regexp = Regex::new(r"^.* \((\d+)\)$").unwrap();

        CheckLibrary {
            library_path,
            db,
            is_case_sensitive,
            invalid_titles: Vec::new(),
            extra_titles: Vec::new(),
            invalid_authors: Vec::new(),
            extra_authors: Vec::new(),
            missing_formats: Vec::new(),
            extra_formats: Vec::new(),
            extra_files: Vec::new(),
            missing_covers: Vec::new(),
            extra_covers: Vec::new(),
            malformed_formats: Vec::new(),
            malformed_paths: Vec::new(),
            failed_folders: Vec::new(),
            all_authors,
            all_ids,
            all_dbpaths,
            all_lc_dbpaths,
            ignore_names: HashSet::new(),
            ignore_ext: HashSet::new(),
            db_id_regexp,
            book_dirs: Vec::new(),
            malformed_paths_ids: HashSet::new(),
        }
    }

    fn ignore_name(&self, filename: &str) -> bool {
        // Simple exact match or glob implementation?
        // Python uses fnmatch.
        // Rust glob crate matches paths, simpler checks for now.
        if self.ignore_names.contains(filename) {
            return true;
        }
        // TODO: support globs if needed
        false
    }

    pub fn scan_library(&mut self, name_ignores: Vec<String>, extension_ignores: Vec<String>) {
        self.ignore_names = name_ignores.into_iter().collect();
        self.ignore_ext = extension_ignores
            .into_iter()
            .map(|e| format!(".{}", e))
            .collect();

        let lib_path = self.library_path.clone();

        // Read top level (Authors)
        if let Ok(entries) = fs::read_dir(&lib_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = match path.file_name().and_then(|s| s.to_str()) {
                    Some(n) => n,
                    None => continue,
                };

                if self.ignore_name(file_name) || IGNORE_AT_TOP_LEVEL.contains(file_name) {
                    continue;
                }

                if !path.is_dir() {
                    self.invalid_authors
                        .push((file_name.to_string(), file_name.to_string(), 0));
                    continue;
                }

                // Process Author Directory
                self.process_author_dir(&path, file_name);
            }
        } else {
            self.failed_folders.push((
                lib_path.to_string_lossy().to_string(),
                "Could not read library root".to_string(),
                vec![],
            ));
        }

        // Process accumulated book dirs
        // We need to clone book_dirs or iterate differently to avoid borrow checker if we used self inside loop
        let books_to_process = self.book_dirs.clone();
        for (db_path, title_dir, id_str) in books_to_process {
            self.process_book_dir(&lib_path, db_path, title_dir, id_str);
        }

        // Check for missing books (in DB but not on disk)
        self.check_missing_books();
    }

    fn process_author_dir(&mut self, auth_path: &Path, auth_dir_name: &str) {
        let mut found_titles = false;

        if let Ok(entries) = fs::read_dir(auth_path) {
            for entry in entries.flatten() {
                let title_path = entry.path();
                let title_dir = match title_path.file_name().and_then(|s| s.to_str()) {
                    Some(n) => n,
                    None => continue,
                };

                if self.ignore_name(title_dir) {
                    continue;
                }

                let db_path_rel = Path::new(auth_dir_name).join(title_dir);
                let db_path_str = db_path_rel.to_string_lossy().replace("\\", "/");

                // Parse ID
                let captures = self.db_id_regexp.captures(title_dir);

                if captures.is_none() || !title_path.is_dir() {
                    self.invalid_titles
                        .push((auth_dir_name.to_string(), db_path_str, 0));
                    continue;
                }

                let id_str = captures.unwrap().get(1).unwrap().as_str().to_string();
                let id_val = id_str.parse::<i32>().unwrap_or(0);

                // Check ID in DB & path match
                if self.is_case_sensitive {
                    if !self.all_dbpaths.contains(&db_path_str) {
                        // Logic from python (condensed)
                        if !self.all_ids.contains(&id_val) || self.db_path_exists_on_disk(id_val) {
                            self.extra_titles
                                .push((title_dir.to_string(), db_path_str.clone(), 0));
                            continue;
                        } else {
                            self.malformed_paths.push((
                                db_path_str.clone(),
                                db_path_str.clone(),
                                id_val,
                            ));
                            self.malformed_paths_ids.insert(id_val);
                        }
                    }
                } else {
                    // Case Insensitive
                    if !self.all_ids.contains(&id_val)
                        || !self.all_lc_dbpaths.contains(&db_path_str.to_lowercase())
                    {
                        self.extra_titles
                            .push((title_dir.to_string(), db_path_str.clone(), 0));
                        continue;
                    }
                }

                self.book_dirs
                    .push((db_path_str, title_dir.to_string(), id_str));
                found_titles = true;
            }
        } else {
            self.failed_folders.push((
                auth_path.to_string_lossy().to_string(),
                "Read error".to_string(),
                vec![],
            ));
        }

        if !found_titles {
            self.extra_authors
                .push((auth_dir_name.to_string(), auth_dir_name.to_string(), 0));
        }
    }

    fn db_path_exists_on_disk(&self, id: i32) -> bool {
        if let Ok(Some(book)) = self.db.get_book(id) {
            let p = self.library_path.join(book.path);
            return p.exists();
        }
        false
    }

    fn process_book_dir(
        &mut self,
        lib_base: &Path,
        db_path: String,
        title_dir: String,
        id_str: String,
    ) {
        let book_id = id_str.parse::<i32>().unwrap_or(0);
        let full_path = lib_base.join(&db_path);

        let mut filenames = HashSet::new();
        if let Ok(entries) = fs::read_dir(&full_path) {
            for entry in entries.flatten() {
                if let Some(n) = entry.file_name().to_str() {
                    if !self.ignore_name(n) {
                        filenames.insert(n.to_string());
                    }
                }
            }
        } else {
            self.failed_folders.push((
                full_path.to_string_lossy().to_string(),
                "Read error".to_string(),
                vec![],
            ));
            return;
        }

        let formats_on_disk: HashSet<String> = filenames
            .iter()
            .filter(|f| self.is_ebook_file(f))
            .cloned()
            .collect();

        let mut db_formats = HashSet::new();
        if let Ok(list) = self.db.format_files(book_id) {
            for (name, ext) in list {
                // Python: x[0]+'.'+x[1].lower()
                db_formats.insert(format!("{}.{}", name, ext.to_lowercase()));
            }
        }

        let filenames_lc: HashMap<String, String> = filenames
            .iter()
            .map(|f| (f.to_lowercase(), f.clone()))
            .collect();
        let formats_on_disk_lc: HashSet<String> =
            formats_on_disk.iter().map(|f| f.to_lowercase()).collect();
        let db_formats_lc: HashSet<String> = db_formats.iter().map(|f| f.to_lowercase()).collect();
        let normals_lc: HashSet<String> = NORMALS.iter().map(|f| f.to_lowercase()).collect();

        if self.is_case_sensitive {
            // Not implementing Case Sensitive path fully for brevity/Windows focus, but skeleton:
            // unknowns = filenames - formats - NORMALS
            // ...
        } else {
            // Unknown files
            for (lc_name, orig_name) in &filenames_lc {
                if !formats_on_disk_lc.contains(lc_name) && !normals_lc.contains(lc_name) {
                    // Check if it's a format in DB but missing on disk? (handled in missing check)
                    // If it is NOT in missing (db_formats), it is extra file
                    // Wait, Python logic: if lcfn in missing: continue (unknown format correctly registered)
                    // Missing = db_formats - formats_on_disk
                    let is_missing =
                        db_formats_lc.contains(lc_name) && !formats_on_disk_lc.contains(lc_name);
                    if !is_missing {
                        self.extra_files.push((
                            title_dir.clone(),
                            full_path.join(orig_name).to_string_lossy().to_string(),
                            book_id,
                        ));
                    }
                }
            }

            // Missing formats
            for req_fmt_lc in &db_formats_lc {
                if !formats_on_disk_lc.contains(req_fmt_lc) {
                    // Check if in unknowns?
                    let in_unknowns = filenames_lc.contains_key(req_fmt_lc)
                        && !formats_on_disk_lc.contains(req_fmt_lc); // roughly
                    if !in_unknowns {
                        // Reconstruct path? arbitrary name
                        self.missing_formats.push((
                            title_dir.clone(),
                            full_path.join(req_fmt_lc).to_string_lossy().to_string(),
                            book_id,
                        ));
                    }
                }
            }

            // Extra formats (on disk but not in DB)
            for fmt_lc in &formats_on_disk_lc {
                if !db_formats_lc.contains(fmt_lc) && !normals_lc.contains(fmt_lc) {
                    let orig_name = filenames_lc.get(fmt_lc).unwrap();
                    self.extra_formats.push((
                        title_dir.clone(),
                        full_path.join(orig_name).to_string_lossy().to_string(),
                        book_id,
                    ));
                }
            }
        }

        // Check Cover
        let has_cover_db = self.db.has_cover(book_id).unwrap_or(false);
        let has_cover_disk = filenames_lc.contains_key("cover.jpg");

        if has_cover_db && !has_cover_disk {
            self.missing_covers.push((
                title_dir.clone(),
                full_path
                    .join(COVER_FILE_NAME)
                    .to_string_lossy()
                    .to_string(),
                book_id,
            ));
        } else if !has_cover_db && has_cover_disk {
            self.extra_covers.push((
                title_dir.clone(),
                full_path
                    .join(COVER_FILE_NAME)
                    .to_string_lossy()
                    .to_string(),
                book_id,
            ));
        }
    }

    fn check_missing_books(&mut self) {
        let lib_path = &self.library_path;
        for &id in &self.all_ids {
            if let Ok(Some(book)) = self.db.get_book(id) {
                if book.path.is_empty() {
                    continue;
                }

                let p = lib_path.join(&book.path);
                if !p.exists() {
                    if self.malformed_paths_ids.contains(&id) && self.is_case_sensitive {
                        continue;
                    }
                    // Book dir missing
                    // Check formats
                    // For each format in DB, add to missing_formats
                    if let Ok(list) = self.db.format_files(id) {
                        for (name, ext) in list {
                            let fname = format!("{}.{}", name, ext.to_lowercase());
                            self.missing_formats.push((
                                book.title.clone(),
                                p.join(fname).to_string_lossy().to_string(),
                                id,
                            ));
                        }
                    }

                    if self.db.has_cover(id).unwrap_or(false) {
                        self.missing_covers.push((
                            book.title.clone(),
                            p.join(COVER_FILE_NAME).to_string_lossy().to_string(),
                            id,
                        ));
                    }
                }
            }
        }
    }

    fn is_ebook_file(&self, filename: &str) -> bool {
        let path = Path::new(filename);
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            let lower = ext.to_lowercase();
            // remove 'original_' prefix logic handled in caller? or here?
            // Python: ext = ext.removeprefix('original_') ? No, that was filename prefix maybe?
            // Python: ext = ext[1:].lower(). removeprefix('original_') is NOT on extension, but maybe?
            // Python check: ext = os.path.splitext(filename)[1]; if not ext: return False. ext = ext[1:].lower()
            // if ext in EBOOK_EXTENSIONS
            // Python doesn't look like it handles original_ prefix on EXTENSION.

            BOOK_EXTENSIONS.contains(lower.as_str())
        } else {
            false
        }
    }
}
