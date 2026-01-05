use indexmap::IndexMap;
use lazy_static::lazy_static;
use log::warn;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::PathBuf;

// --- force_to_bool ---

pub fn force_to_bool(val: &str) -> Option<bool> {
    let val_lower = val.to_lowercase();
    match val_lower.as_str() {
        "yes" | "checked" | "true" | "1" => Some(true),
        "no" | "unchecked" | "false" | "0" => Some(false),
        _ => None,
    }
}

// --- fuzzy_title ---

lazy_static! {
    static ref TITLE_PATTERNS: Vec<(Regex, &'static str)> = vec![
        (Regex::new(r"[\[\](){}<>\x27\x22;,:#]").unwrap(), ""),
        (Regex::new(r"[-._]").unwrap(), " "),
        (Regex::new(r"\s+").unwrap(), " "),
    ];
}

pub fn fuzzy_title(title: &str) -> String {
    let mut title = title.trim().to_lowercase();
    for (pat, repl) in TITLE_PATTERNS.iter() {
        title = pat.replace_all(&title, *repl).to_string();
    }
    title
}

// --- ThumbnailCache ---

#[derive(Debug, Clone)]
struct CacheEntry {
    path: PathBuf,
    size: u64,
    timestamp: f64,
    thumbnail_size: (u32, u32),
}

pub struct ThumbnailCache {
    max_size: u64,
    location: PathBuf,
    thumbnail_size: (u32, u32),
    items: IndexMap<(String, i32), CacheEntry>, // (group_id, book_id) -> Entry
    total_size: u64,
    group_id: String,
}

impl ThumbnailCache {
    pub fn new(location: PathBuf, max_size_mb: u64, thumbnail_size: (u32, u32)) -> Self {
        let max_size = max_size_mb * 1024 * 1024;
        let mut cache = ThumbnailCache {
            max_size,
            location: location.join("thumbnail-cache"), // Default name
            thumbnail_size,
            items: IndexMap::new(),
            total_size: 0,
            group_id: "group".to_string(),
        };
        // Ensure dir exists
        if let Err(e) = fs::create_dir_all(&cache.location) {
            warn!("Failed to create thumbnail cache dir: {}", e);
        }

        let _ = cache.load_index();
        cache
    }

    fn load_index(&mut self) -> io::Result<()> {
        // Limited implementation.
        Ok(())
    }

    pub fn insert(&mut self, book_id: i32, timestamp: f64, data: &[u8]) {
        if self.max_size < data.len() as u64 {
            return;
        }

        self.enforce_size();

        let key = (self.group_id.clone(), book_id);

        // Construct path
        let ts_str = format!("{:.2}", timestamp).replace(".00", "");
        let filename = format!(
            "{}-{}-{}-{}x{}",
            book_id,
            ts_str,
            data.len(),
            self.thumbnail_size.0,
            self.thumbnail_size.1
        );
        let subdir = book_id % 100;
        let dir_path = self.location.join(&self.group_id).join(subdir.to_string());
        let file_path = dir_path.join(&filename);

        // Remove existing
        if let Some(old_entry) = self.items.shift_remove(&key) {
            self.total_size -= old_entry.size;
            let _ = fs::remove_file(old_entry.path);
        }

        // Write file
        match fs::create_dir_all(&dir_path) {
            Ok(_) => {
                if let Err(e) = fs::write(&file_path, data) {
                    warn!("Failed to write thumbnail: {}", e);
                    return;
                }
            }
            Err(e) => {
                warn!("Failed to create thumb dir: {}", e);
                return;
            }
        }

        let entry = CacheEntry {
            path: file_path,
            size: data.len() as u64,
            timestamp,
            thumbnail_size: self.thumbnail_size,
        };

        self.total_size += entry.size;
        self.items.insert(key, entry);

        self.enforce_size();
    }

    pub fn get(&mut self, book_id: i32) -> Option<(Vec<u8>, f64)> {
        let key = (self.group_id.clone(), book_id);

        if let Some(entry) = self.items.shift_remove(&key) {
            // Move to end (LRU)
            match fs::read(&entry.path) {
                Ok(data) => {
                    let ts = entry.timestamp;
                    // Re-insert to mark as recently used
                    self.items.insert(key, entry);
                    Some((data, ts))
                }
                Err(_) => {
                    self.total_size -= entry.size;
                    None
                }
            }
        } else {
            None
        }
    }

    fn enforce_size(&mut self) {
        while self.total_size > self.max_size && !self.items.is_empty() {
            if let Some((_, entry)) = self.items.shift_remove_index(0) {
                let _ = fs::remove_file(&entry.path);
                self.total_size -= entry.size;
            }
        }
    }

    pub fn invalidate(&mut self, book_ids: &[i32]) {
        let gid = self.group_id.clone();
        for &bid in book_ids {
            let key = (gid.clone(), bid);
            if let Some(entry) = self.items.shift_remove(&key) {
                let _ = fs::remove_file(entry.path);
                self.total_size -= entry.size;
            }
        }
    }
}

// --- Duplicate Detection ---

/// Finds books in the destination that match the given metadata (Title/Authors).
///
/// `author_map`: Lowercase Author Name -> Vec<AuthorID>
/// `aid_to_bids`: AuthorID -> Vec<BookID>
/// `title_map`: BookID -> Title
pub fn find_identical_books(
    title: &str,
    authors: &[String],
    author_map: &IndexMap<String, Vec<i32>>,
    aid_to_bids: &IndexMap<i32, Vec<i32>>,
    title_map: &IndexMap<i32, String>,
) -> HashSet<i32> {
    let mut found_books: Option<HashSet<i32>> = None;

    // 1. Intersect books for all authors
    for author in authors {
        let author_lower = author.trim().to_lowercase();
        if let Some(author_ids) = author_map.get(&author_lower) {
            let mut books_for_this_author = HashSet::new();
            for aid in author_ids {
                if let Some(bids) = aid_to_bids.get(aid) {
                    for &bid in bids {
                        books_for_this_author.insert(bid);
                    }
                }
            }

            match found_books {
                None => found_books = Some(books_for_this_author),
                Some(ref mut current_set) => {
                    current_set.retain(|bid| books_for_this_author.contains(bid));
                }
            }

            if let Some(ref s) = found_books {
                if s.is_empty() {
                    return HashSet::new();
                }
            }
        } else {
            // Author not found at all, so no match possible
            return HashSet::new();
        }
    }

    let candidates = found_books.unwrap_or_default();
    let mut ans = HashSet::new();
    let title_fuzzy = fuzzy_title(title);

    // 2. Filter by Fuzzy Title
    for book_id in candidates {
        if let Some(candidate_title) = title_map.get(&book_id) {
            if fuzzy_title(candidate_title) == title_fuzzy {
                ans.insert(book_id);
            }
        }
    }

    ans
}

// --- Range Parsing ---

pub fn integers_from_string(s: &str) -> Vec<i32> {
    let mut ids = HashSet::new();
    for part in s.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.contains('-') {
            let ranges: Vec<&str> = part.split('-').collect();
            if ranges.len() == 2 {
                if let (Ok(start), Ok(end)) = (
                    ranges[0].trim().parse::<i32>(),
                    ranges[1].trim().parse::<i32>(),
                ) {
                    for i in start..=end {
                        ids.insert(i);
                    }
                }
            }
        } else if let Ok(id) = part.parse::<i32>() {
            ids.insert(id);
        }
    }
    let mut result: Vec<i32> = ids.into_iter().collect();
    result.sort();
    result
}
