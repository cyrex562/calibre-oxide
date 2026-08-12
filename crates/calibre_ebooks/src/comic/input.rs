//! Comic archive extraction + page enumeration.
//!
//! Port of the archive/page-listing half of
//! `old_src/src/calibre/ebooks/comic/input.py`. The Qt image
//! rendering pipeline (`PageProcessor` and friends) is deferred to a
//! follow-up issue since it needs an `image` / `imageproc`-based
//! reimplementation, not a Qt-to-Rust translation.

use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use zip::ZipArchive;

/// The image extensions the Python `find_pages` considered valid
/// comic pages. Order doesn't matter; comparison is case-insensitive.
pub fn comic_exts() -> &'static [&'static str] {
    &[
        "jpg", "jpeg", "png", "gif", "webp", "bmp", "tiff", "tif", "avif",
    ]
}

pub fn is_comic_page(path: &Path) -> bool {
    if path.components().any(|c| {
        // Skip __MACOSX resource-fork directories. Matches
        // Python's `if '__MACOSX' in path`.
        matches!(c, std::path::Component::Normal(seg) if seg == "__MACOSX")
    }) {
        return false;
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let lower = ext.to_ascii_lowercase();
    comic_exts().iter().any(|e| *e == lower.as_str())
}

/// Sanitize a filename per Python `extract_comic`'s post-extract
/// walk: replace `#` with `_`, strip control characters, drop
/// leading/trailing whitespace. Return `None` if the sanitized name
/// would be empty.
pub fn sanitize_page_filename(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .map(|c| if c == '#' { '_' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Extract a comic archive (CBZ / plain ZIP) to a destination
/// directory, renaming any files whose sanitized name differs from
/// the original (matches Python `os.rename` in `extract_comic`).
///
/// Returns the list of extracted file paths. Errors on
/// non-ZIP archives — CBR (RAR) and CB7 (7z) support are separate
/// (calibre_utils::unrar + sevenz-rust bindings) and can be layered
/// via a caller-provided archive backend if needed.
pub fn extract_comic(archive_path: &Path, dest_dir: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("create dest {:?}", dest_dir))?;
    let file = fs::File::open(archive_path)
        .with_context(|| format!("open comic archive {:?}", archive_path))?;
    let mut archive = ZipArchive::new(file)
        .context("open zip archive")?;
    extract_zip_with_sanitization(&mut archive, dest_dir)
}

/// Internal: same as `extract_comic` but takes any Read+Seek so tests
/// can drive it with an in-memory buffer.
pub fn extract_zip_with_sanitization<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    dest_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let mut extracted: Vec<PathBuf> = Vec::new();
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("read zip entry {}", i))?;
        let raw_name = match entry.enclosed_name() {
            Some(p) => p.to_path_buf(),
            None => continue, // zip-slip guard: reject paths with `..` etc.
        };
        // Skip directory entries.
        if entry.is_dir() {
            fs::create_dir_all(dest_dir.join(&raw_name)).ok();
            continue;
        }

        // Sanitize each path component. If any component would
        // sanitize to empty, we drop the whole file.
        let mut sanitized = PathBuf::new();
        let mut ok = true;
        for comp in raw_name.components() {
            match comp {
                std::path::Component::Normal(seg) => {
                    let s = seg.to_string_lossy();
                    match sanitize_page_filename(&s) {
                        Some(clean) => sanitized.push(clean),
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                std::path::Component::CurDir => {}
                _ => {
                    // ParentDir / RootDir / Prefix — path-traversal
                    // vectors. enclosed_name() already filters `..`
                    // but be defensive.
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let final_path = dest_dir.join(&sanitized);
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {:?}", parent))?;
        }
        let mut out = fs::File::create(&final_path)
            .with_context(|| format!("write {:?}", final_path))?;
        std::io::copy(&mut entry, &mut out)
            .with_context(|| format!("extract {:?}", final_path))?;
        extracted.push(final_path);
    }
    Ok(extracted)
}

/// Whether to sort pages by name (default) or by last-modified time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageSort {
    Name,
    Mtime,
}

/// Walk `dir` recursively and return the sorted list of comic pages.
/// Sort strategy matches Python:
/// - `PageSort::Name`: natural-numeric key on filename (so
///   `page2.jpg` sorts before `page10.jpg`). When files are in
///   folders of differing depth, sort by full path instead of
///   basename (Python `len(sep_counts) > 1`).
/// - `PageSort::Mtime`: last-modified time ascending.
pub fn find_pages(dir: &Path, sort: PageSort) -> Result<Vec<PathBuf>> {
    let mut pages: Vec<PathBuf> = Vec::new();
    walk_files(dir, &mut pages)?;
    pages.retain(|p| is_comic_page(p));

    // Depth-heterogeneity check: if every page has the same "/"
    // depth relative to `dir`, sort by basename; otherwise by full
    // path. Matches the Python `len(sep_counts) > 1` heuristic.
    let depths: std::collections::HashSet<usize> = pages
        .iter()
        .filter_map(|p| p.strip_prefix(dir).ok())
        .map(|rel| rel.components().count())
        .collect();
    let use_full_path = depths.len() > 1;

    match sort {
        PageSort::Name => {
            pages.sort_by(|a, b| {
                let ka = sort_key(a, use_full_path);
                let kb = sort_key(b, use_full_path);
                numeric_sort_key(&ka).cmp(&numeric_sort_key(&kb))
            });
        }
        PageSort::Mtime => {
            pages.sort_by_key(|p| p.metadata().and_then(|m| m.modified()).ok());
        }
    }
    Ok(pages)
}

fn sort_key(p: &Path, use_full_path: bool) -> String {
    if use_full_path {
        p.to_string_lossy().into_owned()
    } else {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !dir.is_dir() {
        return Err(anyhow!("{:?} is not a directory", dir));
    }
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {:?}", dir))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Split a string into `(non-digit run, digit run)` pairs, with
/// the digit runs converted to `u64` so numeric comparison works.
/// This mirrors Python's `calibre.utils.icu.numeric_sort_key` well
/// enough for filename sorting; full ICU-quality sorting is
/// overkill for a comic page list.
///
/// The return type is a `Vec` of enum variants so the standard
/// `cmp` derives lexicographic order over the sequence — exactly
/// what we want (compare piece-by-piece; a Text piece is always
/// ordered before a Num piece at the same position, which is
/// deliberate for stable numeric sort).
pub fn numeric_sort_key(s: &str) -> Vec<SortToken> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_digits = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_digits && !buf.is_empty() {
                out.push(SortToken::Text(std::mem::take(&mut buf).to_lowercase()));
            }
            in_digits = true;
            buf.push(c);
        } else {
            if in_digits {
                if let Ok(n) = buf.parse::<u64>() {
                    out.push(SortToken::Num(n));
                } else {
                    out.push(SortToken::Text(std::mem::take(&mut buf).to_lowercase()));
                }
                buf.clear();
            }
            in_digits = false;
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        if in_digits {
            if let Ok(n) = buf.parse::<u64>() {
                out.push(SortToken::Num(n));
            } else {
                out.push(SortToken::Text(buf.to_lowercase()));
            }
        } else {
            out.push(SortToken::Text(buf.to_lowercase()));
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortToken {
    // Order matters: Text sorts before Num for a given position.
    Text(String),
    Num(u64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn comic_exts_are_lowercase_and_expected_set() {
        for e in comic_exts() {
            assert_eq!(*e, e.to_ascii_lowercase());
        }
        assert!(comic_exts().contains(&"jpg"));
        assert!(comic_exts().contains(&"png"));
        assert!(comic_exts().contains(&"webp"));
    }

    #[test]
    fn is_comic_page_accepts_common_extensions() {
        assert!(is_comic_page(Path::new("cover.jpg")));
        assert!(is_comic_page(Path::new("PAGE01.JPEG")));
        assert!(is_comic_page(Path::new("sub/dir/03.PNG")));
    }

    #[test]
    fn is_comic_page_rejects_non_images_and_macosx() {
        assert!(!is_comic_page(Path::new("readme.txt")));
        assert!(!is_comic_page(Path::new("book.epub")));
        assert!(!is_comic_page(Path::new("__MACOSX/cover.jpg")));
    }

    #[test]
    fn sanitize_replaces_hash_and_strips_controls() {
        assert_eq!(sanitize_page_filename("page#1.jpg"), Some("page_1.jpg".to_string()));
        assert_eq!(sanitize_page_filename("a\x07b.jpg"), Some("ab.jpg".to_string()));
        assert_eq!(sanitize_page_filename("  cover.jpg  "), Some("cover.jpg".to_string()));
    }

    #[test]
    fn sanitize_rejects_empty_and_whitespace_only() {
        assert_eq!(sanitize_page_filename(""), None);
        assert_eq!(sanitize_page_filename("   "), None);
        assert_eq!(sanitize_page_filename("\x00\x01"), None);
    }

    #[test]
    fn numeric_sort_key_orders_naturally() {
        // "page2" should sort before "page10", which the naive
        // string sort would reverse.
        let mut names = vec!["page10.jpg", "page1.jpg", "page2.jpg"];
        names.sort_by_key(|s| numeric_sort_key(s));
        assert_eq!(names, vec!["page1.jpg", "page2.jpg", "page10.jpg"]);
    }

    #[test]
    fn numeric_sort_key_is_case_insensitive_for_text_parts() {
        assert_eq!(numeric_sort_key("Page1"), numeric_sort_key("PAGE1"));
    }

    #[test]
    fn extract_comic_extracts_and_sanitizes_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("comic.cbz");
        {
            let f = std::fs::File::create(&archive_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::FileOptions::default();
            w.start_file("page#1.jpg", opts).unwrap();
            w.write_all(b"fake-jpeg").unwrap();
            w.start_file("subdir/page#2.png", opts).unwrap();
            w.write_all(b"fake-png").unwrap();
            w.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        let extracted = extract_comic(&archive_path, &dest).unwrap();
        assert_eq!(extracted.len(), 2);
        // # → _ sanitization happened.
        assert!(extracted.iter().any(|p| p.ends_with("page_1.jpg")));
        assert!(extracted.iter().any(|p| p.ends_with("page_2.png")));
        // Original names with # must not appear on disk.
        assert!(!dest.join("page#1.jpg").exists());
    }

    #[test]
    fn extract_comic_rejects_zip_slip() {
        // A malicious archive containing `../evil.jpg` must NOT
        // escape the dest dir. Python relies on `os.path.join`
        // behavior; Rust `enclosed_name()` in the zip crate
        // performs the same check.
        let tmp = tempfile::tempdir().unwrap();
        let archive_path = tmp.path().join("evil.cbz");
        {
            let f = std::fs::File::create(&archive_path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::FileOptions::default();
            w.start_file("../evil.jpg", opts).unwrap();
            w.write_all(b"bad").unwrap();
            w.finish().unwrap();
        }
        let dest = tmp.path().join("out");
        let extracted = extract_comic(&archive_path, &dest).unwrap();
        assert!(extracted.is_empty(), "zip-slip must have been blocked");
        // Parent of dest must NOT have the evil file.
        assert!(!tmp.path().join("evil.jpg").exists());
    }

    #[test]
    fn find_pages_by_name_uses_natural_sort() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["page10.jpg", "page1.jpg", "page2.jpg", "notes.txt"] {
            std::fs::write(tmp.path().join(name), b"").unwrap();
        }
        let pages = find_pages(tmp.path(), PageSort::Name).unwrap();
        let names: Vec<String> = pages
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["page1.jpg", "page2.jpg", "page10.jpg"]);
    }

    #[test]
    fn find_pages_skips_macosx_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("cover.jpg"), b"").unwrap();
        std::fs::create_dir_all(tmp.path().join("__MACOSX")).unwrap();
        std::fs::write(tmp.path().join("__MACOSX/cover.jpg"), b"").unwrap();
        let pages = find_pages(tmp.path(), PageSort::Name).unwrap();
        assert_eq!(pages.len(), 1);
        assert!(pages[0].ends_with("cover.jpg"));
    }

    #[test]
    fn find_pages_sorts_by_full_path_when_depth_varies() {
        // Two folders at different depths. Python switches to full-
        // path sort in that case to keep folder ordering stable.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a")).unwrap();
        std::fs::create_dir_all(tmp.path().join("b/c")).unwrap();
        std::fs::write(tmp.path().join("a/1.jpg"), b"").unwrap();
        std::fs::write(tmp.path().join("b/c/2.jpg"), b"").unwrap();
        let pages = find_pages(tmp.path(), PageSort::Name).unwrap();
        // a/1.jpg should come before b/c/2.jpg regardless of natural
        // key on the basenames alone.
        assert_eq!(pages.len(), 2);
        assert!(pages[0].to_string_lossy().contains("a"));
        assert!(pages[1].to_string_lossy().contains("b"));
    }

    #[test]
    fn find_pages_by_mtime_orders_ascending() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["z.jpg", "a.jpg", "m.jpg"] {
            std::fs::write(tmp.path().join(name), b"").unwrap();
            // Sleep briefly so mtimes differ. 10 ms is enough on
            // any FS that stores mtime with millisecond precision.
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        let pages = find_pages(tmp.path(), PageSort::Mtime).unwrap();
        // Order should be creation order (z, a, m), not
        // alphabetical.
        let names: Vec<String> = pages
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["z.jpg", "a.jpg", "m.jpg"]);
    }
}
