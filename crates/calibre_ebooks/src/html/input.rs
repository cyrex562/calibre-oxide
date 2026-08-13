//! Following the links out of an HTML file.
//!
//! Port of `old_src/src/calibre/ebooks/html/input.py`.
//!
//! Converting a loose HTML file means finding the rest of the book:
//! calibre reads the file, pulls every `<a href>` out of it with a
//! regular expression, resolves the local ones to paths, and repeats
//! breadth-first until it runs out of levels. The result is returned
//! twice over — in discovery order and in depth-first order — because
//! the conversion offers both as a user option.
//!
//! The link extraction really is a regex over the raw source rather
//! than a parse. That is deliberate upstream: the files being crawled
//! are frequently too broken to parse, and a link that only a tag-soup
//! parser could find is not worth the dependency at this stage. The
//! pattern is ported as written.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::Regex;

use crate::chardet;
use crate::html_entities::decode_entities;

/// How many bytes are read before deciding whether a file is HTML.
const HEADER_BYTES: usize = 4096;

fn html_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<\s*html").expect("valid regex"))
}

fn title_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<title>([^<>]+)</title>").expect("valid regex"))
}

fn link_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // `(?s)` for DOTALL, `(?i)` for IGNORECASE — the flags calibre
    // passes. The three alternatives are double-quoted, single-quoted
    // and bare hrefs.
    RE.get_or_init(|| {
        Regex::new(r#"(?si)<\s*a\s+.*?href\s*=\s*(?:(?:"([^"]+)")|(?:'([^']+)')|([^\s>]+))"#)
            .expect("valid regex")
    })
}

/// The pieces of a URL this module needs.
///
/// Stands in for Python's `urlparse`, which is more than is wanted
/// here: only the scheme, path, params, query and fragment are used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ParsedUrl {
    scheme: String,
    path: String,
    params: String,
    query: String,
    fragment: String,
}

fn parse_url(url: &str) -> ParsedUrl {
    let (rest, fragment) = match url.split_once('#') {
        Some((r, f)) => (r, f.to_string()),
        None => (url, String::new()),
    };
    let (rest, query) = match rest.split_once('?') {
        Some((r, q)) => (r, q.to_string()),
        None => (rest, String::new()),
    };
    // A scheme is a letter followed by letters, digits, `+`, `-` or
    // `.`, then a colon. Note this treats a Windows drive letter as a
    // scheme, exactly as Python's urlparse does.
    let (scheme, rest) = match rest.find(':') {
        Some(i)
            if i > 0
                && rest[..i].starts_with(|c: char| c.is_ascii_alphabetic())
                && rest[..i]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) =>
        {
            (rest[..i].to_lowercase(), &rest[i + 1..])
        }
        _ => (String::new(), rest),
    };
    // `params` is the `;`-suffix of the last path segment.
    let last_segment_start = rest.rfind('/').map(|i| i + 1).unwrap_or(0);
    let (path, params) = match rest[last_segment_start..].find(';') {
        Some(i) => {
            let split = last_segment_start + i;
            (rest[..split].to_string(), rest[split + 1..].to_string())
        }
        None => (rest.to_string(), String::new()),
    };
    ParsedUrl {
        scheme,
        path,
        params,
        query,
        fragment,
    }
}

/// Percent-decode a URL component.
fn unquote(s: &str) -> String {
    urlencoding::decode(s)
        .map(|c| c.into_owned())
        .unwrap_or_else(|_| s.to_string())
}

/// A link found in an HTML file.
///
/// Port of the Python `Link`.
///
/// Two links compare equal when their paths do — which means every
/// link *without* a path is equal to every other, since `None == None`.
/// That is calibre's own definition and it has a visible consequence:
/// [`HtmlFile`] deduplicates as it collects, so several fragment-only
/// links in one file collapse into the first of them. Reproduced.
#[derive(Debug, Clone)]
pub struct Link {
    /// The href as written, after entity replacement.
    pub url: String,
    /// Whether the link is to a file rather than to another server.
    pub is_local: bool,
    /// Whether the link is a bare fragment, pointing inside the same
    /// file.
    pub is_internal: bool,
    /// The resolved absolute path, for local non-internal links.
    pub path: Option<PathBuf>,
    /// The fragment, percent-decoded.
    pub fragment: String,
}

impl PartialEq for Link {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for Link {}

impl Link {
    /// Resolve `url` against the folder `base`.
    ///
    /// Port of the Python `Link.__init__`.
    pub fn new(url: &str, base: &Path) -> Self {
        let parsed = parse_url(url);
        let is_local = parsed.scheme.is_empty() || parsed.scheme == "file";
        let is_internal = is_local && parsed.path.is_empty();
        let path = if is_local && !is_internal {
            Some(url_to_local_path(&parsed, base))
        } else {
            None
        };
        Self {
            url: url.to_string(),
            is_local,
            is_internal,
            path,
            fragment: unquote(&parsed.fragment),
        }
    }
}

/// Turn a parsed local URL into a filesystem path.
///
/// Port of the Python `url_to_local_path`. Note that the query and
/// params are re-attached to the path before unquoting, so
/// `chapter.html?x=1` resolves to a file whose name contains the
/// query — that is what the Python's `urlunparse` call does.
fn url_to_local_path(url: &ParsedUrl, base: &Path) -> PathBuf {
    let mut path = url.path.clone();
    // On Windows a local URL path is `/C:/...`; the leading slash is
    // not part of the filename.
    let is_abs = if cfg!(windows) && path.starts_with('/') {
        path.remove(0);
        true
    } else {
        false
    };
    if !url.params.is_empty() {
        path.push(';');
        path.push_str(&url.params);
    }
    if !url.query.is_empty() {
        path.push('?');
        path.push_str(&url.query);
    }
    let path = unquote(&path);
    let candidate = PathBuf::from(&path);
    if is_abs || candidate.is_absolute() {
        return candidate;
    }
    normalize(&base.join(candidate))
}

/// Resolve `.` and `..` without touching the filesystem.
///
/// Stands in for `os.path.abspath`, which also normalizes without
/// requiring the path to exist — unlike `std::fs::canonicalize`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Why a file was left out of the traversal.
///
/// Port of the Python `IgnoreFile`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreFile {
    pub message: String,
    /// Whether the file was simply absent, which is not worth
    /// reporting — a dead link in an HTML file is ordinary.
    pub doesnt_exist: bool,
}

/// One HTML file in the crawl, and the links out of it.
///
/// Port of the Python `HTMLFile`.
#[derive(Debug, Clone)]
pub struct HtmlFile {
    /// Absolute path to the file.
    pub path: PathBuf,
    /// The folder relative links are resolved against.
    pub base: PathBuf,
    /// 0 for the file the crawl started from.
    pub level: usize,
    /// The `<title>` if there is one, else the file's stem.
    pub title: String,
    /// The encoding the content was decoded with.
    pub encoding: Option<String>,
    /// Whether the file turned out not to be HTML.
    pub is_binary: bool,
    pub links: Vec<Link>,
    /// The file that first linked here.
    ///
    /// The Python holds the referring `HTMLFile` itself; this holds its
    /// path, which says the same thing without a reference cycle.
    pub referrer: Option<PathBuf>,
}

impl PartialEq for HtmlFile {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl Eq for HtmlFile {}

impl HtmlFile {
    /// Read a file and find its links.
    ///
    /// Port of the Python `HTMLFile.__init__`. A file that cannot be
    /// read is an error at level 0 and an [`IgnoreFile`] below it,
    /// matching the Python's two failure modes.
    pub fn new(
        path: &Path,
        level: usize,
        encoding: Option<&str>,
        referrer: Option<PathBuf>,
    ) -> Result<Self, IgnoreFile> {
        let path = normalize(&absolute(path));
        let base = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();

        let src = fs::read(&path).map_err(|err| IgnoreFile {
            message: format!(
                "Could not read from file: {} with error: {err}",
                path.display()
            ),
            doesnt_exist: err.kind() == std::io::ErrorKind::NotFound,
        })?;

        let mut file = Self {
            path,
            base,
            level,
            title: stem,
            encoding: None,
            is_binary: false,
            links: Vec::new(),
            referrer,
        };

        if src.is_empty() {
            // An empty root file is an error upstream; below the root it
            // is simply not HTML.
            if level == 0 {
                return Err(IgnoreFile {
                    message: format!("The file {} is empty", file.path.display()),
                    doesnt_exist: false,
                });
            }
            file.is_binary = true;
            return Ok(file);
        }

        let header = &src[..src.len().min(HEADER_BYTES)];
        let declared = detect_xml_encoding(header);
        // Below the root, anything without an <html> tag in its first
        // few kilobytes is treated as a binary attachment.
        if level > 0 {
            let decoded_header = decode_with(header, declared.as_deref().or(encoding));
            file.is_binary = !html_pat().is_match(&decoded_header);
        }
        if file.is_binary {
            return Ok(file);
        }

        let chosen = encoding
            .map(str::to_string)
            .or(declared)
            .unwrap_or_else(|| chardet::detect(header).encoding);
        let text = decode_with(&src, Some(&chosen));
        file.encoding = Some(chosen);
        if let Some(m) = title_pat().captures(&text) {
            file.title = m[1].to_string();
        }
        file.find_links(&text);
        Ok(file)
    }

    /// Collect the links out of the source.
    ///
    /// Port of the Python `find_links`.
    fn find_links(&mut self, src: &str) {
        for caps in link_pat().captures_iter(src) {
            // The three alternatives are quoted, quoted and bare.
            let Some(url) = (1..=3)
                .filter_map(|i| caps.get(i))
                .map(|m| m.as_str())
                .find(|u| !u.is_empty())
            else {
                continue;
            };
            let url = decode_entities(url);
            let link = Link::new(&url, &self.base);
            if !self.links.contains(&link) {
                self.links.push(link);
            }
        }
    }
}

/// The hrefs `LINK_PAT` finds in a source, in order and before entity
/// replacement.
///
/// Exposed so the extraction can be compared against calibre's directly;
/// [`HtmlFile`] resolves and deduplicates what this returns.
pub fn find_links_in(src: &str) -> Vec<String> {
    link_pat()
        .captures_iter(src)
        .filter_map(|caps| {
            (1..=3)
                .filter_map(|i| caps.get(i))
                .map(|m| m.as_str())
                .find(|u| !u.is_empty())
                .map(str::to_string)
        })
        .collect()
}

/// Make a path absolute without requiring it to exist.
fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    }
}

/// Detect an encoding from a BOM or an encoding declaration.
///
/// Stands in for `calibre.ebooks.chardet.detect_xml_encoding`, built
/// from the pieces [`crate::chardet`] already provides.
fn detect_xml_encoding(raw: &[u8]) -> Option<String> {
    if let Some((_, encoding)) = chardet::detect_bom(raw) {
        return Some(encoding.to_string());
    }
    chardet::find_declared_encoding(raw, HEADER_BYTES)
}

fn decode_with(raw: &[u8], encoding: Option<&str>) -> String {
    let label = encoding.unwrap_or("utf-8");
    let enc = encoding_rs::Encoding::for_label(label.as_bytes()).unwrap_or(encoding_rs::UTF_8);
    enc.decode(raw).0.into_owned()
}

/// Yield the files in depth-first order, following each file's links
/// before moving on to its siblings.
///
/// Port of the Python `depth_first`. Files reachable only through a
/// link that the crawl never resolved (because `max_levels` cut it
/// off) are skipped.
pub fn depth_first(flat: &[HtmlFile]) -> Vec<usize> {
    let mut order = Vec::new();
    if flat.is_empty() {
        return order;
    }
    order.push(0);
    let mut visited: HashSet<PathBuf> = HashSet::new();
    visited.insert(flat[0].path.clone());

    // A deque used as a stack whose pushes go to the front, which is
    // what makes the traversal depth-first while keeping link order.
    let mut stack: Vec<PathBuf> = Vec::new();
    let add_links_from = |file: &HtmlFile, stack: &mut Vec<PathBuf>, visited: &HashSet<PathBuf>| {
        for link in file.links.iter().rev() {
            if let Some(path) = &link.path {
                if !visited.contains(path) {
                    stack.insert(0, path.clone());
                }
            }
        }
    };
    add_links_from(&flat[0], &mut stack, &visited);

    while !stack.is_empty() {
        let path = stack.remove(0);
        let Some(index) = flat.iter().position(|f| f.path == path) else {
            continue;
        };
        if visited.contains(&flat[index].path) {
            continue;
        }
        order.push(index);
        visited.insert(flat[index].path.clone());
        add_links_from(&flat[index], &mut stack, &visited);
    }
    order
}

/// What a crawl produced.
#[derive(Debug, Clone, Default)]
pub struct Traversal {
    /// Every file, in the order it was discovered — breadth first.
    pub flat: Vec<HtmlFile>,
    /// The same files in depth-first order.
    pub depth_first: Vec<HtmlFile>,
    /// Files that were linked but left out, and why.
    pub ignored: Vec<IgnoreFile>,
}

/// Follow every link out of an HTML file, recursively.
///
/// `max_levels` bounds the recursion; 0 means the root file's links are
/// not followed at all. `encoding` overrides detection.
///
/// Port of the Python `traverse`.
pub fn traverse(
    path: &Path,
    max_levels: usize,
    encoding: Option<&str>,
) -> Result<Traversal, IgnoreFile> {
    let root = HtmlFile::new(path, 0, encoding, None)?;
    let mut flat = vec![root];
    let mut seen: HashSet<PathBuf> = HashSet::new();
    seen.insert(flat[0].path.clone());
    let mut ignored = Vec::new();

    let mut level = 0usize;
    let mut next_level: Vec<usize> = vec![0];
    while level < max_levels && !next_level.is_empty() {
        level += 1;
        let mut nl = Vec::new();
        for index in next_level {
            let (candidates, referrer) = {
                let file = &flat[index];
                (
                    file.links
                        .iter()
                        .filter_map(|l| l.path.clone())
                        .collect::<Vec<_>>(),
                    file.path.clone(),
                )
            };
            let mut rejects: Vec<PathBuf> = Vec::new();
            for link_path in candidates {
                if seen.contains(&link_path) {
                    continue;
                }
                match HtmlFile::new(&link_path, level, encoding, Some(referrer.clone())) {
                    Ok(nf) => {
                        if seen.contains(&nf.path) {
                            continue;
                        }
                        seen.insert(nf.path.clone());
                        if nf.is_binary {
                            ignored.push(IgnoreFile {
                                message: format!("{} is a binary file", nf.path.display()),
                                doesnt_exist: false,
                            });
                            rejects.push(link_path);
                            continue;
                        }
                        nl.push(flat.len());
                        flat.push(nf);
                    }
                    Err(err) => {
                        ignored.push(err);
                        rejects.push(link_path);
                    }
                }
            }
            // A link that led nowhere is dropped, so the conversion
            // does not later try to rewrite it.
            flat[index]
                .links
                .retain(|l| !l.path.as_ref().is_some_and(|p| rejects.contains(p)));
        }
        next_level = nl;
    }

    let order = depth_first(&flat);
    let depth_first_files = order.iter().map(|i| flat[*i].clone()).collect();
    Ok(Traversal {
        flat,
        depth_first: depth_first_files,
        ignored,
    })
}

/// The file list a conversion should use.
///
/// Port of the Python `get_filelist`: the same crawl, returned in
/// whichever order the caller asked for.
pub fn get_filelist(
    path: &Path,
    max_levels: usize,
    encoding: Option<&str>,
    breadth_first: bool,
) -> Result<Vec<HtmlFile>, IgnoreFile> {
    let traversal = traverse(path, max_levels, encoding)?;
    Ok(if breadth_first {
        traversal.flat
    } else {
        traversal.depth_first
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, content).unwrap();
        path
    }

    fn page(title: &str, links: &[&str]) -> String {
        let body: String = links
            .iter()
            .map(|l| format!("<a href=\"{l}\">x</a>"))
            .collect();
        format!("<html><head><title>{title}</title></head><body>{body}</body></html>")
    }

    #[test]
    fn parses_the_parts_of_a_url_that_matter() {
        let u = parse_url("chapter.html?x=1#frag");
        assert_eq!(u.scheme, "");
        assert_eq!(u.path, "chapter.html");
        assert_eq!(u.query, "x=1");
        assert_eq!(u.fragment, "frag");

        let u = parse_url("https://example.com/a");
        assert_eq!(u.scheme, "https");

        let u = parse_url("#only-a-fragment");
        assert_eq!(u.path, "");
        assert_eq!(u.fragment, "only-a-fragment");

        // Params are the `;` suffix of the last segment only.
        let u = parse_url("a;b/c;d");
        assert_eq!(u.path, "a;b/c");
        assert_eq!(u.params, "d");
    }

    #[test]
    fn local_and_remote_links_are_told_apart() {
        let base = Path::new("/books/one");
        let local = Link::new("chapter2.html", base);
        assert!(local.is_local && !local.is_internal);
        assert_eq!(
            local.path.as_deref(),
            Some(Path::new("/books/one/chapter2.html"))
        );

        let remote = Link::new("https://example.com/a.html", base);
        assert!(!remote.is_local);
        assert_eq!(remote.path, None);

        // A `file:` URL is local.
        let file_url = Link::new("file:next.html", base);
        assert!(file_url.is_local);

        let internal = Link::new("#section", base);
        assert!(internal.is_local && internal.is_internal);
        assert_eq!(internal.path, None);
        assert_eq!(internal.fragment, "section");
    }

    #[test]
    fn relative_links_resolve_and_normalize() {
        let base = Path::new("/books/one/text");
        assert_eq!(
            Link::new("../images/a.html", base).path.as_deref(),
            Some(Path::new("/books/one/images/a.html"))
        );
        assert_eq!(
            Link::new("./b.html", base).path.as_deref(),
            Some(Path::new("/books/one/text/b.html"))
        );
        assert_eq!(
            Link::new("/abs/c.html", base).path.as_deref(),
            Some(Path::new("/abs/c.html"))
        );
    }

    #[test]
    fn percent_escapes_are_decoded_in_paths_and_fragments() {
        let base = Path::new("/books");
        let link = Link::new("a%20file.html#a%20frag", base);
        assert_eq!(link.path.as_deref(), Some(Path::new("/books/a file.html")));
        assert_eq!(link.fragment, "a frag");
    }

    #[test]
    fn a_query_string_becomes_part_of_the_filename() {
        // calibre re-attaches the query to the path before resolving,
        // so a link with one addresses a file whose name contains it.
        let link = Link::new("chapter.html?x=1", Path::new("/books"));
        assert_eq!(
            link.path.as_deref(),
            Some(Path::new("/books/chapter.html?x=1"))
        );
    }

    #[test]
    fn links_are_extracted_from_all_three_quoting_styles() {
        let dir = TempDir::new().unwrap();
        let src = r#"<html><body>
            <a href="one.html">1</a>
            <a href='two.html'>2</a>
            <a href=three.html>3</a>
            <a  class="x"  href = "four.html" >4</a>
        </body></html>"#;
        let path = write(&dir, "root.html", src);
        let file = HtmlFile::new(&path, 0, None, None).unwrap();
        let names: Vec<String> = file
            .links
            .iter()
            .map(|l| {
                l.path
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(
            names,
            vec!["one.html", "two.html", "three.html", "four.html"]
        );
    }

    #[test]
    fn entities_in_hrefs_are_resolved() {
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "root.html",
            r#"<html><body><a href="a.html?x=1&amp;y=2">x</a></body></html>"#,
        );
        let file = HtmlFile::new(&path, 0, None, None).unwrap();
        assert_eq!(file.links.len(), 1);
        assert!(file.links[0].url.contains("&y=2"), "{}", file.links[0].url);
    }

    #[test]
    fn fragment_only_links_collapse_into_one() {
        // Every pathless link compares equal to every other, which is
        // calibre's own definition of Link equality, so the collector
        // keeps only the first.
        let dir = TempDir::new().unwrap();
        let path = write(
            &dir,
            "root.html",
            r##"<html><body><a href="#a">a</a><a href="#b">b</a></body></html>"##,
        );
        let file = HtmlFile::new(&path, 0, None, None).unwrap();
        assert_eq!(file.links.len(), 1);
        assert_eq!(file.links[0].fragment, "a");
    }

    #[test]
    fn the_title_comes_from_the_markup_and_falls_back_to_the_stem() {
        let dir = TempDir::new().unwrap();
        let titled = write(&dir, "a.html", &page("Chapter One", &[]));
        assert_eq!(
            HtmlFile::new(&titled, 0, None, None).unwrap().title,
            "Chapter One"
        );

        let untitled = write(&dir, "b.html", "<html><body>hi</body></html>");
        assert_eq!(HtmlFile::new(&untitled, 0, None, None).unwrap().title, "b");
    }

    #[test]
    fn a_missing_root_file_is_an_error() {
        let err = HtmlFile::new(Path::new("/nonexistent/x.html"), 0, None, None).unwrap_err();
        assert!(err.doesnt_exist);
        assert!(err.message.contains("Could not read"));
    }

    #[test]
    fn an_empty_root_file_is_rejected() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "empty.html", "");
        let err = HtmlFile::new(&path, 0, None, None).unwrap_err();
        assert!(err.message.contains("is empty"));
        // Below the root it is simply not HTML.
        let file = HtmlFile::new(&path, 1, None, None).unwrap();
        assert!(file.is_binary);
    }

    #[test]
    fn a_non_html_file_below_the_root_is_binary() {
        let dir = TempDir::new().unwrap();
        let path = write(&dir, "notes.txt", "just some text, no markup at all");
        // At the root, the check is not applied at all.
        assert!(!HtmlFile::new(&path, 0, None, None).unwrap().is_binary);
        assert!(HtmlFile::new(&path, 1, None, None).unwrap().is_binary);
    }

    /// calibre's own `test_depth_first`, with files on disk in place of
    /// its stub objects.
    #[test]
    fn depth_first_matches_calibres_own_test() {
        let dir = TempDir::new().unwrap();
        write(&dir, "root.html", &page("root", &["a.html", "b.html"]));
        write(&dir, "a.html", &page("a", &["a1.html", "a2.html"]));
        write(&dir, "a1.html", &page("a1", &["x.html"]));
        write(&dir, "a2.html", &page("a2", &[]));
        write(&dir, "b.html", &page("b", &["b1.html"]));
        write(&dir, "b1.html", &page("b1", &[]));
        write(&dir, "x.html", &page("x", &[]));

        let root = dir.path().join("root.html");
        let traversal = traverse(&root, 10, None).unwrap();

        let names = |files: &[HtmlFile]| -> Vec<String> {
            files
                .iter()
                .map(|f| f.path.file_stem().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        // Discovery order is breadth first.
        assert_eq!(
            names(&traversal.flat),
            vec!["root", "a", "b", "a1", "a2", "b1", "x"]
        );
        // And the depth-first order is the one calibre's test asserts.
        assert_eq!(
            names(&traversal.depth_first),
            vec!["root", "a", "a1", "x", "a2", "b", "b1"]
        );
    }

    #[test]
    fn max_levels_bounds_the_crawl() {
        let dir = TempDir::new().unwrap();
        write(&dir, "root.html", &page("root", &["a.html"]));
        write(&dir, "a.html", &page("a", &["b.html"]));
        write(&dir, "b.html", &page("b", &[]));
        let root = dir.path().join("root.html");

        assert_eq!(traverse(&root, 0, None).unwrap().flat.len(), 1);
        assert_eq!(traverse(&root, 1, None).unwrap().flat.len(), 2);
        assert_eq!(traverse(&root, 2, None).unwrap().flat.len(), 3);
    }

    #[test]
    fn a_cycle_is_visited_once() {
        let dir = TempDir::new().unwrap();
        write(&dir, "a.html", &page("a", &["b.html"]));
        write(&dir, "b.html", &page("b", &["a.html"]));
        let traversal = traverse(&dir.path().join("a.html"), 10, None).unwrap();
        assert_eq!(traversal.flat.len(), 2);
        assert_eq!(traversal.depth_first.len(), 2);
    }

    #[test]
    fn dead_links_are_dropped_and_reported() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "root.html",
            &page("root", &["gone.html", "here.html"]),
        );
        write(&dir, "here.html", &page("here", &[]));
        let traversal = traverse(&dir.path().join("root.html"), 10, None).unwrap();

        assert_eq!(traversal.flat.len(), 2);
        assert_eq!(traversal.ignored.len(), 1);
        assert!(traversal.ignored[0].doesnt_exist);
        // The link is removed from the file that carried it.
        assert_eq!(traversal.flat[0].links.len(), 1);
    }

    #[test]
    fn a_binary_attachment_is_reported_and_not_followed() {
        let dir = TempDir::new().unwrap();
        write(&dir, "root.html", &page("root", &["image.bin"]));
        write(&dir, "image.bin", "\u{0}\u{1}binary junk with no markup");
        let traversal = traverse(&dir.path().join("root.html"), 10, None).unwrap();
        assert_eq!(traversal.flat.len(), 1);
        assert_eq!(traversal.ignored.len(), 1);
        assert!(traversal.ignored[0].message.contains("binary"));
    }

    #[test]
    fn get_filelist_offers_both_orders() {
        let dir = TempDir::new().unwrap();
        write(&dir, "root.html", &page("root", &["a.html", "b.html"]));
        write(&dir, "a.html", &page("a", &["a1.html"]));
        write(&dir, "a1.html", &page("a1", &[]));
        write(&dir, "b.html", &page("b", &[]));
        let root = dir.path().join("root.html");

        let stems = |files: Vec<HtmlFile>| -> Vec<String> {
            files
                .iter()
                .map(|f| f.path.file_stem().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            stems(get_filelist(&root, 10, None, true).unwrap()),
            vec!["root", "a", "b", "a1"]
        );
        assert_eq!(
            stems(get_filelist(&root, 10, None, false).unwrap()),
            vec!["root", "a", "a1", "b"]
        );
    }

    #[test]
    fn the_referrer_records_who_linked_first() {
        let dir = TempDir::new().unwrap();
        write(&dir, "root.html", &page("root", &["a.html"]));
        write(&dir, "a.html", &page("a", &[]));
        let traversal = traverse(&dir.path().join("root.html"), 10, None).unwrap();
        assert_eq!(traversal.flat[0].referrer, None);
        assert_eq!(
            traversal.flat[1].referrer.as_deref(),
            Some(traversal.flat[0].path.as_path())
        );
    }

    #[test]
    fn a_declared_encoding_is_honoured() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("latin.html");
        let mut bytes = b"<html><head><meta charset=\"iso-8859-1\"><title>caf".to_vec();
        bytes.push(0xe9); // an e-acute in latin-1
        bytes.extend_from_slice(b"</title></head><body></body></html>");
        fs::write(&path, bytes).unwrap();

        let file = HtmlFile::new(&path, 0, None, None).unwrap();
        assert_eq!(file.title, "café");
    }
}
