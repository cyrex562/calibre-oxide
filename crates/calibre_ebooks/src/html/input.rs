use anyhow::Result;
use encoding_rs::Encoding;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub url: String,
    pub path: Option<PathBuf>,
    pub is_local: bool,
}

impl Link {
    pub fn new(url_str: &str, base: &Path) -> Self {
        // Simple heuristic parsing for now (mirroring Python's logic loosely)
        // If url is absolute (http, ftp), it's not local.
        // If it looks like a file path or relative URL, it might be local.

        // Use `url` crate for robust parsing
        let is_local = if let Ok(parsed) = Url::parse(url_str) {
            parsed.scheme() == "file" || parsed.scheme() == ""
        } else {
            // Failed to parse as absolute URL, assume relative
            true
        };

        let path = if is_local {
            // Remove fragment/query (heuristic)
            let path_part = url_str
                .split('#')
                .next()
                .unwrap_or(url_str)
                .split('?')
                .next()
                .unwrap_or(url_str);

            // On Windows, handle / starting paths if needed?
            // Join with base directly
            let joined = base.join(path_part);
            if joined.exists() {
                Some(std::fs::canonicalize(joined).unwrap_or_else(|_| base.join(path_part)))
            } else {
                // Try decoding %20 etc?
                if let Ok(decoded) = urlencoding::decode(path_part) {
                    let joined_decoded = base.join(decoded.into_owned());
                    if joined_decoded.exists() {
                        Some(std::fs::canonicalize(joined_decoded).unwrap_or(base.join(path_part)))
                    } else {
                        None // Not found locally
                    }
                } else {
                    None
                }
            }
        } else {
            None
        };

        Link {
            url: url_str.to_string(),
            path,
            is_local: is_local,
        }
    }
}

pub struct HTMLFile {
    pub path: PathBuf,
    pub level: usize,
    pub links: Vec<Link>,
    pub is_binary: bool,
    pub encoding: String,
}

lazy_static! {
    static ref LINK_REGEX: Regex =
        Regex::new(r#"(?i)<\s*a\s+.*?href\s*=\s*(?:(?:"([^"]+)")|(?:'([^']+)')|([^\s>]+))"#)
            .unwrap();
}

impl HTMLFile {
    pub fn new(path: &Path, level: usize) -> Result<Self> {
        let abs_path = fs::canonicalize(path)?;
        let content_bytes = fs::read(&abs_path)?;

        // Check for binary (null bytes in first 4096)
        let check_len = std::cmp::min(content_bytes.len(), 4096);
        let is_binary = content_bytes[..check_len].contains(&0);

        if is_binary {
            return Ok(HTMLFile {
                path: abs_path,
                level,
                links: Vec::new(),
                is_binary: true,
                encoding: "binary".to_string(),
            });
        }

        // Detect Encoding (Basic UTF-8 fallback for now, maybe use chardet later)
        let (cow, encoding_used, _) = encoding_rs::UTF_8.decode(&content_bytes);
        let content = cow.to_string();

        let mut links = Vec::new();
        let parent = abs_path.parent().unwrap_or(Path::new("."));

        for cap in LINK_REGEX.captures_iter(&content) {
            let url = cap.get(1).or(cap.get(2)).or(cap.get(3));
            if let Some(m) = url {
                let link = Link::new(m.as_str(), parent);
                if link.is_local {
                    links.push(link);
                }
            }
        }

        Ok(HTMLFile {
            path: abs_path,
            level,
            links,
            is_binary: false,
            encoding: encoding_used.name().to_string(),
        })
    }
}

pub fn traverse(root_path: &Path, max_levels: usize) -> Result<Vec<HTMLFile>> {
    let mut flat = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = VecDeque::new(); // For breadth-first like logic, but logic below mimics structure

    let root = HTMLFile::new(root_path, 0)?;
    seen.insert(root.path.clone());
    queue.push_back(root);

    // We process queue, adding new items to flat list
    // Python implementation tracks layers.
    // Let's implement BFS.

    while let Some(current) = queue.pop_front() {
        if current.level >= max_levels {
            flat.push(current);
            continue;
        }

        let mut children = Vec::new();
        for link in &current.links {
            if let Some(p) = &link.path {
                if !seen.contains(p) {
                    // Try to parse child
                    if let Ok(child) = HTMLFile::new(p, current.level + 1) {
                        if !child.is_binary {
                            // Only traverse text
                            seen.insert(child.path.clone());
                            children.push(child);
                        }
                    }
                }
            }
        }

        flat.push(current);
        for child in children {
            queue.push_back(child);
        }
    }

    Ok(flat)
}
