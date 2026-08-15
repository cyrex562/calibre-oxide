//! Port of `old_src/src/calibre/ebooks/oeb/polish/utils.py`.

use std::path::{Path, PathBuf};

use crate::mobi::dom::{Dom, NodeId};

/// Port of `BLOCK_TAG_NAMES`.
pub const BLOCK_TAG_NAMES: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "center",
    "dir",
    "fieldset",
    "isindex",
    "menu",
    "noframes",
    "hgroup",
    "noscript",
    "pre",
    "section",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "p",
    "div",
    "dd",
    "dl",
    "ul",
    "ol",
    "li",
    "body",
    "td",
    "th",
];

/// Port of `OEB_FONTS`: all font mimetypes seen in e-books.
pub const OEB_FONTS: &[&str] = &[
    "font/otf",
    "font/woff",
    "font/woff2",
    "font/ttf",
    "application/x-font-ttf",
    "application/x-font-otf",
    "application/font-sfnt",
    "application/vnd.ms-opentype",
    "application/x-font-truetype",
];

/// Port of `guess_type`: the expected mimetype for a filename, falling
/// back to `application/octet-stream`. Uses `mime_guess`, the
/// established boundary call for this crate (see e.g.
/// `docx::container`, `mobi::mobi6`) rather than porting calibre's own
/// `guess_type` table from `calibre/__init__.py`.
pub fn guess_type(name: &str) -> String {
    mime_guess::from_path(name)
        .first()
        .map(|m| m.essence_str().to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Port of `adjust_mime_for_epub`. `opf_version` is `(major, minor)`.
pub fn adjust_mime_for_epub(filename: &str, mime: Option<&str>, opf_version: (u32, u32)) -> String {
    let mut mime = mime
        .map(|s| s.to_string())
        .unwrap_or_else(|| guess_type(filename));
    if mime == "text/html" {
        // epubcheck complains if the mimetype for text documents is set
        // to text/html in EPUB 2 books.
        return "application/xhtml+xml".to_string();
    }
    if !OEB_FONTS.contains(&mime.as_str()) {
        return mime;
    }
    if mime.contains("ttf") || mime.contains("truetype") {
        mime = "font/ttf".to_string();
    } else if mime.contains("otf") || mime.contains("opentype") {
        mime = "font/otf".to_string();
    } else if mime == "application/font-sfnt" {
        mime = if filename.to_lowercase().ends_with(".otf") {
            "font/otf".to_string()
        } else {
            "font/ttf".to_string()
        };
    } else if mime.contains("woff2") {
        mime = "font/woff2".to_string();
    } else if mime.contains("woff") {
        mime = "font/woff".to_string();
    }

    match opf_version {
        (3, 0) => match mime.as_str() {
            "font/ttf" | "font/otf" => "application/vnd.ms-opentype".to_string(),
            "font/woff" => "application/font-woff".to_string(),
            _ => mime,
        },
        (3, 1) => match mime.as_str() {
            "font/ttf" | "font/otf" => "application/font-sfnt".to_string(),
            "font/woff" => "application/font-woff".to_string(),
            _ => mime,
        },
        (major, _) if major < 3 => match mime.as_str() {
            "font/ttf" => "application/x-font-truetype".to_string(),
            "font/otf" => "application/vnd.ms-opentype".to_string(),
            "font/woff" => "application/font-woff".to_string(),
            _ => mime,
        },
        _ => mime,
    }
}

/// Port of `setup_css_parser_serialization`. Python configures global
/// serialization preferences on the `css_parser` library. No CSS
/// serializer exists in this crate (issues #34/#35/#36 already
/// established there's no general CSS parser/serializer here, only the
/// narrow shorthand-property handling in `oeb::normalize_css`) -- see
/// [`parse_css`] for the matching gap on the parsing side.
pub fn setup_css_parser_serialization(_tab_width: usize) {
    todo!(
        "placeholder: no CSS serializer exists in this crate to configure \
         (see oeb::normalize_css and parse_css's docs -- this is the same, \
         already-documented gap as issues #34/#35/#36)"
    )
}

/// Minimal structural requirement [`actual_case_for_name`]/
/// [`corrected_case_for_name`] need from a container. Defined here
/// (rather than importing `container::Container`) so `utils` does not
/// need to depend on `container` for these two functions, matching
/// Python's duck-typing (they're called with a `Container` in practice,
/// but only ever use `.exists()`/`.name_to_abspath()`).
pub trait NameLookup {
    fn exists(&self, name: &str) -> bool;
    fn name_to_abspath(&self, name: &str) -> PathBuf;
}

/// Port of `actual_case_for_name`.
pub fn actual_case_for_name(container: &impl NameLookup, name: &str) -> anyhow::Result<String> {
    anyhow::ensure!(
        container.exists(name),
        "Cannot get actual case for {name} as it does not exist"
    );
    let parts: Vec<&str> = name.split('/').collect();
    let mut ans: Vec<String> = Vec::new();
    for x in &parts {
        let base = if ans.is_empty() {
            (*x).to_string()
        } else {
            format!("{}/{}", ans.join("/"), x)
        };
        let path = container.name_to_abspath(&base);
        let pdir = path.parent().unwrap_or(Path::new(""));
        let mut correct = None;
        if let Ok(entries) = std::fs::read_dir(pdir) {
            for entry in entries.flatten() {
                let candidate_name = entry.file_name();
                if candidate_name.to_string_lossy() == *x {
                    correct = Some((*x).to_string());
                    break;
                }
            }
        }
        if correct.is_none() {
            if let Ok(entries) = std::fs::read_dir(pdir) {
                for entry in entries.flatten() {
                    if calibre_utils::filenames::samefile(&entry.path(), &path) {
                        correct = Some(entry.file_name().to_string_lossy().into_owned());
                        break;
                    }
                }
            }
        }
        match correct {
            Some(c) => ans.push(c),
            None => anyhow::bail!("Something bad happened"),
        }
    }
    Ok(ans.join("/"))
}

/// Port of `corrected_case_for_name`. Returns `None` where Python
/// returns `None` (case-insensitive match not found, or a non-terminal
/// path component turned out to be a file).
pub fn corrected_case_for_name(container: &impl NameLookup, name: &str) -> Option<String> {
    let parts: Vec<&str> = name.split('/').collect();
    let mut ans: Vec<String> = Vec::new();
    for x in &parts {
        let base = if ans.is_empty() {
            (*x).to_string()
        } else {
            format!("{}/{}", ans.join("/"), x)
        };
        let correct = if container.exists(&base) {
            Some((*x).to_string())
        } else {
            let path = container.name_to_abspath(&base);
            let pdir = path.parent()?;
            let entries = std::fs::read_dir(pdir).ok()?;
            let mut found = None;
            for entry in entries.flatten() {
                let candidate = entry.file_name().to_string_lossy().into_owned();
                if candidate.to_lowercase() == x.to_lowercase() {
                    found = Some(candidate);
                    break;
                }
            }
            found
        };
        ans.push(correct?);
    }
    Some(ans.join("/"))
}

/// Port of `PositionFinder`: maps a byte offset into `raw` to a
/// `(line_number, column_offset)` pair.
pub struct PositionFinder {
    new_lines: Vec<usize>,
}

impl PositionFinder {
    pub fn new(raw: &str) -> Self {
        let new_lines = raw.match_indices('\n').map(|(i, _)| i + 1).collect();
        PositionFinder { new_lines }
    }

    /// Port of `PositionFinder.__call__`. Returns `(line_number,
    /// offset)`, both matching Python's `bisect.bisect` + fallback
    /// semantics (1-based line, `+1` again in Python's `(lnum + 1,
    /// offset)`).
    pub fn locate(&self, pos: usize) -> (usize, usize) {
        let lnum = self.new_lines.partition_point(|&nl| nl <= pos);
        let offset = if lnum == 0 {
            pos
        } else {
            pos.abs_diff(self.new_lines[lnum - 1])
        };
        (lnum + 1, offset)
    }
}

/// Port of `CommentFinder`: is a given offset inside a `/* ... */` CSS
/// comment?
pub struct CommentFinder {
    spans: Vec<(usize, usize)>,
}

impl CommentFinder {
    pub fn new(raw: &str) -> Self {
        let re = comment_re();
        let spans = re.find_iter(raw).map(|m| (m.start(), m.end())).collect();
        CommentFinder { spans }
    }

    pub fn contains(&self, offset: usize) -> bool {
        if self.spans.is_empty() {
            return false;
        }
        let idx = self.spans.partition_point(|&(start, _)| start <= offset);
        if idx == 0 {
            return false;
        }
        let (start, end) = self.spans[idx - 1];
        start <= offset && offset <= end
    }
}

fn comment_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"(?s)/\*.*?\*/").unwrap())
}

/// Port of `lead_text`: the leading text of `top_elem` (including
/// descendants), up to `num_words` words. Operates on
/// [`crate::mobi::dom::Dom`] since it walks XHTML content documents.
pub fn lead_text(dom: &Dom, top_elem: NodeId, num_words: usize) -> String {
    let ws_re = whitespace_re();
    let mut words: Vec<String> = Vec::new();

    // (node, is_text_not_tail): matches Python's stack of (elem, attr)
    // pairs walking `.text` then children then `.tail`. Our arena
    // stores text/tail as literal sibling Text nodes, so "get_text"
    // reduces to: if a node is itself a Text node, split it into words.
    let mut stack: Vec<NodeId> = vec![top_elem];
    // Pre-order-ish walk matching Python's stack-based traversal: for
    // an element, push text content then children (front-to-back, so a
    // stack popped from the back needs them reversed); for a text
    // node, just take its words.
    fn walk(dom: &Dom, id: NodeId, words: &mut Vec<String>, ws_re: &regex::Regex, limit: usize) {
        if words.len() >= limit {
            return;
        }
        match &dom.node(id).kind {
            crate::mobi::dom::NodeKind::Text(t) => {
                for w in ws_re.split(t) {
                    if !w.is_empty() {
                        words.push(w.to_string());
                        if words.len() >= limit {
                            return;
                        }
                    }
                }
            }
            crate::mobi::dom::NodeKind::Element(_) => {
                for &c in &dom.node(id).children {
                    walk(dom, c, words, ws_re, limit);
                    if words.len() >= limit {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
    let _ = &mut stack;
    walk(dom, top_elem, &mut words, ws_re, num_words);
    words.truncate(num_words);
    words.join(" ")
}

fn whitespace_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\s+").unwrap())
}

/// Port of `parse_css`. No general CSS parser exists in this crate
/// (issues #34/#35/#36 established this boundary; `oeb::normalize_css`
/// only handles narrow shorthand-property rewriting, not full
/// declaration/rule parsing). Real, general CSS parsing is out of
/// scope for this port.
pub fn parse_css(_data: &str, _is_declaration: bool) {
    todo!(
        "placeholder: no general CSS parser exists in this crate (see \
         oeb::normalize_css's docs and issues #34/#35/#36) -- container.py's \
         callers of Container::parse_css need real declaration/rule parsing, \
         which is out of scope for this port"
    )
}

/// Port of `handle_entities`.
pub fn handle_entities(text: &str, func: impl Fn(&str) -> String) -> String {
    func(&crate::html_entities::xml_replace_entities(text))
}

/// Port of `extract`: removes `elem` from the tree, keeping its tail
/// (i.e. the trailing sibling text that logically follows it). Because
/// this arena represents "tail" text as an ordinary sibling `Text` node
/// rather than an out-of-band attribute, removing `elem` from its
/// parent's child list *is* "extract" -- the sibling text node is
/// untouched and simply becomes adjacent to whatever now precedes it.
/// See `xmltree`'s module docs for the same property on the XML side.
pub fn extract(dom: &mut Dom, id: NodeId) {
    dom.detach(id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_mime_for_epub_text_html_becomes_xhtml() {
        assert_eq!(
            adjust_mime_for_epub("a.html", Some("text/html"), (2, 0)),
            "application/xhtml+xml"
        );
    }

    #[test]
    fn adjust_mime_for_epub_fonts_epub3() {
        assert_eq!(
            adjust_mime_for_epub("f.ttf", Some("application/x-font-ttf"), (3, 0)),
            "application/vnd.ms-opentype"
        );
        assert_eq!(
            adjust_mime_for_epub("f.otf", Some("application/x-font-otf"), (2, 0)),
            "application/vnd.ms-opentype"
        );
    }

    #[test]
    fn adjust_mime_for_epub_passthrough_for_non_font() {
        assert_eq!(
            adjust_mime_for_epub("a.jpg", Some("image/jpeg"), (2, 0)),
            "image/jpeg"
        );
    }

    #[test]
    fn guess_type_falls_back_to_octet_stream() {
        assert_eq!(guess_type("noextension"), "application/octet-stream");
        assert_eq!(guess_type("a.html"), "text/html");
    }

    #[test]
    fn position_finder_locates_lines() {
        let raw = "line1\nline2\nline3";
        let pf = PositionFinder::new(raw);
        let (line, _off) = pf.locate(0);
        assert_eq!(line, 1);
        let pos_of_line3 = raw.find("line3").unwrap();
        let (line, _off) = pf.locate(pos_of_line3);
        assert_eq!(line, 3);
    }

    #[test]
    fn comment_finder_detects_offsets_inside_comments() {
        let raw = "a { color: red; } /* comment */ b { color: blue; }";
        let cf = CommentFinder::new(raw);
        let comment_start = raw.find("/*").unwrap();
        assert!(cf.contains(comment_start + 3));
        assert!(!cf.contains(0));
    }

    #[test]
    fn lead_text_stops_at_word_limit() {
        let dom = Dom::parse("<html><body><p>one two three four five</p></body></html>");
        let body = dom.find_first_tag_global("body").unwrap();
        assert_eq!(lead_text(&dom, body, 3), "one two three");
    }

    #[test]
    fn extract_removes_node_keeping_sibling_text() {
        let mut dom = Dom::parse("<html><body><b>bold</b> after</body></html>");
        let b = dom.find_first_tag_global("b").unwrap();
        extract(&mut dom, b);
        let body = dom.find_first_tag_global("body").unwrap();
        let out = dom.serialize(body);
        assert!(!out.contains("<b>"));
        assert!(out.contains("after"));
    }

    struct FakeContainer {
        root: PathBuf,
    }
    impl NameLookup for FakeContainer {
        fn exists(&self, name: &str) -> bool {
            self.name_to_abspath(name).exists()
        }
        fn name_to_abspath(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }

    #[test]
    fn corrected_case_for_name_fixes_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Chapter1.html"), b"x").unwrap();
        let c = FakeContainer {
            root: dir.path().to_path_buf(),
        };
        assert_eq!(
            corrected_case_for_name(&c, "chapter1.html"),
            Some("Chapter1.html".to_string())
        );
        assert_eq!(corrected_case_for_name(&c, "missing.html"), None);
    }

    #[test]
    fn actual_case_for_name_returns_existing_case() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Chapter1.html"), b"x").unwrap();
        let c = FakeContainer {
            root: dir.path().to_path_buf(),
        };
        assert_eq!(
            actual_case_for_name(&c, "Chapter1.html").unwrap(),
            "Chapter1.html"
        );
    }
}
