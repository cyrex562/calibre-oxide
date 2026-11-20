//! Embedded metadata that is not about the book (tranche 11).
//!
//! Office suites, print drivers and scanners write *their own* name or
//! the name of the file they were fed into a PDF's title and author fields.
//! Trusting that as metadata gave a real trial library false "same book"
//! groups: every file titled `ACDSee 32 print job` by `RDL_python` was one
//! book, and so was everything by `Admin`.
//!
//! This is the **one place** the junk lists live. Two consumers:
//!
//! * extraction (`scan_pipeline::apply_enrichment`, `import::overlay`) calls
//!   [`clean_title`] / [`clean_authors`] so a junk value is treated as
//!   *absent* and the filename fallback that already exists applies;
//! * duplicate evidence (`dedupe`) calls [`is_junk_title`] and
//!   [`is_junk_author`] so libraries imported *before* this existed benefit
//!   too: a junk author is never matching evidence, a junk title never keys a
//!   group.
//!
//! # What is junk
//!
//! Titles:
//! * `<anything> print job` (a print driver's job name);
//! * `Microsoft Word - foo.doc` and the other office/Acrobat prefixes: the
//!   prefix is stripped, and what remains is used only when it is itself
//!   meaningful;
//! * `Untitled`, `Document1`, `Book2`, `Presentation3` ... ([`JUNK_TITLES`] plus
//!   a stem-and-counter pattern);
//! * a bare filename or path ending in a document/image extension (`.doc`,
//!   `.tif`, `.indd` ...: [`FILE_EXTENSIONS`]).
//!
//! Authors: the OS/office placeholders in [`JUNK_AUTHORS`] (`Admin`, `Owner`,
//! `User` ...). Kept deliberately short: a name added here can never be used
//! to match books, so add only values that are never a person.

use lazy_static::lazy_static;
use regex::Regex;

/// Titles that are a tool's placeholder, compared after [`squash`].
const JUNK_TITLES: &[&str] = &["untitled", "untitled document", "document", "unknown", "unknown book", "scanned document", "scan", "scanned image", "powerpoint presentation", "presentation", "no title", "none", "null", "title", "new document"];

/// Prefixes an application puts in front of the document name.
const TITLE_PREFIXES: &[&str] = &["microsoft word - ", "microsoft powerpoint - ", "microsoft excel - ", "microsoft publisher - ", "adobe acrobat - ", "adobe indesign - ", "untitled - "];

/// Extensions that mark a "title" as being a filename.
const FILE_EXTENSIONS: &[&str] = &[
    "doc", "docx", "dot", "rtf", "txt", "odt", "wpd", "wps", "pdf", "ppt", "pptx", "xls", "xlsx", "pub", "indd", "qxd", "qxp", "psd", "ai", "cdr", "tif", "tiff", "jpg", "jpeg", "png", "bmp", "gif", "htm", "html", "mdb", "ps", "eps", "prn", "tmp",
];

/// Authors that are an account or application name, not a person
/// (compared after [`squash`]).
const JUNK_AUTHORS: &[&str] = &["admin", "administrator", "owner", "user", "ners", "root", "guest", "default", "author", "unknown", "anonymous", "your name", "your name here", "pc", "home", "customer", "rdl python", "acdsee", "pdfcreator", "pdffactory", "scansnap", "scanner", "adobe", "microsoft"];

lazy_static! {
    /// `Document1`, `Book 2`, `Untitled3`, `Presentation12`: an office suite's numbered default. (Not `Scan2`: a scanner's counter is also what some books call themselves in tests and in life.)
    static ref PLACEHOLDER_COUNTER: Regex = Regex::new(r"^(?:document|doc|book|untitled|presentation|slide|sheet)\s*\d+$").unwrap();
    static ref PRINT_JOB: Regex = Regex::new(r"(?i)\bprint\s*job\s*$").unwrap();
}

/// Lowercase alphanumerics, one space between words.
fn squash(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending && !out.is_empty() {
                out.push(' ');
            }
            pending = false;
            out.extend(c.to_lowercase());
        } else {
            pending = true;
        }
    }
    out
}

fn has_file_extension(s: &str) -> bool {
    match s.trim().rsplit_once('.') {
        Some((stem, ext)) => !stem.trim().is_empty() && !ext.contains(char::is_whitespace) && FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => false,
    }
}

fn is_placeholder(s: &str) -> bool {
    let q = squash(s);
    q.is_empty() || JUNK_TITLES.contains(&q.as_str()) || PLACEHOLDER_COUNTER.is_match(&q) || q.chars().all(|c| c.is_ascii_digit() || c == ' ')
}

/// The usable form of an embedded title, or `None` when it is junk and the
/// filename should be used instead.
///
/// `Microsoft Word - Dragon Lore.doc` becomes `Dragon Lore`;
/// `Microsoft Word - Document1` and `ACDSee 32 print job` become `None`.
pub fn clean_title(raw: &str) -> Option<String> {
    let mut title = raw.trim();
    let lower = title.to_lowercase();
    for prefix in TITLE_PREFIXES {
        if lower.starts_with(prefix) {
            title = title[prefix.len()..].trim();
            // The rest is a document name: drop a trailing extension.
            if has_file_extension(title) {
                title = title.rsplit_once('.').map(|(s, _)| s.trim()).unwrap_or(title);
            }
            break;
        }
    }
    if title.is_empty() || is_placeholder(title) || PRINT_JOB.is_match(title) || has_file_extension(title) {
        return None;
    }
    Some(title.to_string())
}

/// Whether `title` is junk as it stands (used as evidence, so the
/// `Microsoft Word - ` family counts even when something follows it).
pub fn is_junk_title(title: &str) -> bool {
    let t = title.trim();
    let lower = t.to_lowercase();
    TITLE_PREFIXES.iter().any(|p| lower.starts_with(p)) || clean_title(t).is_none()
}

/// Whether `name` is an account or application name rather than a person.
pub fn is_junk_author(name: &str) -> bool {
    JUNK_AUTHORS.contains(&squash(name).as_str())
}

/// `authors` without the junk and the blank ones.
pub fn clean_authors<'a>(authors: impl IntoIterator<Item = &'a String>) -> Vec<String> {
    authors.into_iter().map(|a| a.trim()).filter(|a| !a.is_empty() && !is_junk_author(a)).map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_job_titles_are_junk() {
        assert_eq!(clean_title("ACDSee 32 print job"), None);
        assert_eq!(clean_title("Some Thing Print Job"), None);
        assert!(is_junk_title("ACDSee 32 print job"));
    }

    #[test]
    fn office_prefixes_are_stripped_when_the_rest_means_something() {
        assert_eq!(clean_title("Microsoft Word - Dragon Lore.doc").as_deref(), Some("Dragon Lore"));
        assert_eq!(clean_title("Microsoft Word - Document1"), None);
        assert_eq!(clean_title("Microsoft Word - foo.doc").as_deref(), Some("foo"));
        assert_eq!(clean_title("Microsoft Word - 12.doc"), None);
        assert!(is_junk_title("Microsoft Word - Dragon Lore.doc"));
    }

    #[test]
    fn placeholders_and_filenames_are_junk() {
        for t in ["untitled", "Untitled", "Document1", "document 12", "Book2", "Presentation3", "  ", "1234", "F:\\On Hold\\For Pat\\1-45.tif", "layout.indd", "scan0001.TIF", "report.doc"] {
            assert_eq!(clean_title(t), None, "{t:?}");
        }
    }

    #[test]
    fn real_titles_survive() {
        for t in ["The Hobbit", "Learning Node.js", "ASP.NET Unleashed", "Print Job Scheduling for Beginners", "Document Engineering", "Dune"] {
            assert_eq!(clean_title(t).as_deref(), Some(t), "{t:?}");
        }
    }

    #[test]
    fn junk_authors() {
        for a in ["Admin", "administrator", " Owner ", "USER", "Ners", "RDL_python", "Unknown"] {
            assert!(is_junk_author(a), "{a:?}");
        }
        for a in ["Frank Herbert", "J.R.R. Tolkien", "Ada User"] {
            assert!(!is_junk_author(a), "{a:?}");
        }
        let authors = vec!["Admin".to_string(), "Ann Lee".to_string(), "  ".to_string()];
        assert_eq!(clean_authors(&authors), vec!["Ann Lee"]);
    }
}
