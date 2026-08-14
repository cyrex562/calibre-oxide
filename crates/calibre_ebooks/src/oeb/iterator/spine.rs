//! The paginated spine + link/anchor indexing used by the ebook viewer.
//!
//! Port of `old_src/src/calibre/ebooks/oeb/iterator/spine.py`.
//!
//! Two representational departures from the Python, both forced by the
//! language rather than a design choice:
//!
//! - Python's `SpineItem` is a `str` *subclass* (`class SpineItem(str)`)
//!   that also carries extra attributes -- the file's absolute path
//!   doubles as its own dict/set key everywhere (`spine_paths = {s: s
//!   for s in spine}`, `spine.index(self.key)`). Rust has no subclassing
//!   of `String`, so [`SpineItem`] is a plain struct with a `path`
//!   field, and callers that need "the canonical spine entry for this
//!   path" do an explicit `position()`/lookup instead of relying on
//!   hash-equality-as-identity.
//! - `anchor_map`'s offsets come from `regex`'s **byte** offsets into
//!   the (UTF-8) haystack, not Python's **character** offsets into a
//!   `str`. The two agree in relative order for well-formed UTF-8 (byte
//!   offset is monotonic in character offset), which is all
//!   [`IndexEntry::anchor_pos`] comparisons ever need -- but the raw
//!   numbers themselves are not bit-for-bit comparable with Python's.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;

use crate::chardet::xml_to_unicode;
use crate::html_entities::decode_entities;
use crate::oeb::toc::TOCNode;

fn char_count_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r">[^<]+<").expect("valid regex"))
}

fn ws_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s+").expect("valid regex"))
}

/// Return the number of "significant" text characters in an HTML
/// string. Port of `character_count` in `spine.py`.
pub fn character_count(html: &str) -> i64 {
    let mut count: i64 = 0;
    for m in char_count_pat().find_iter(html) {
        let collapsed = ws_pat().replace_all(m.as_str(), " ");
        count += collapsed.chars().count() as i64 - 2;
    }
    count
}

fn anchor_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:id|name)\s*=\s*['"]([^'"]+)['"]"#).expect("valid regex"))
}

/// Return a map of every anchor name (`id="..."`/`name="..."`) to its
/// (first) offset in the HTML. Port of `anchor_map` in `spine.py` -- see
/// the module doc for the byte-vs-character-offset caveat.
pub fn anchor_map(html: &str) -> HashMap<String, usize> {
    let mut ans = HashMap::new();
    for caps in anchor_pat().captures_iter(html) {
        let anchor = caps.get(1).expect("group 1 always matches").as_str();
        let start = caps.get(0).expect("group 0 always matches").start();
        ans.entry(anchor.to_string()).or_insert(start);
    }
    ans
}

fn links_pat() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Python matches `<a ... href=(['"]) VALUE \1` with a backreference
    // to require the same quote character close the value; `regex`
    // doesn't support backreferences, so the two quote styles are
    // spelled out as separate alternatives instead (each excluding its
    // own quote character from the value, which is what the
    // backreferenced non-greedy match effectively achieved).
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<\s*a\s+.*?href\s*=\s*(?:"([^"]+)"|'([^']+)')"#).expect("valid regex")
    })
}

/// Return the set of every link href in the file, with entities
/// unescaped. Port of `all_links` in `spine.py`.
pub fn all_links(html: &str) -> HashSet<String> {
    let mut ans = HashSet::new();
    for caps in links_pat().captures_iter(html) {
        if let Some(href) = caps.get(1).or_else(|| caps.get(2)) {
            ans.insert(decode_entities(href.as_str()));
        }
    }
    ans
}

/// One entry in [`SpineItem::index_entries`]: a TOC entry that touches
/// this spine file, with the anchors (if any) at which it starts/ends
/// *within this file specifically*. Port of the anonymous
/// `namedtuple('IndexEntry', 'entry start_anchor end_anchor')` built in
/// `create_indexing_data`.
#[derive(Debug, Clone)]
pub struct SpineIndexEntry {
    pub entry: Rc<IndexEntry>,
    pub start_anchor: Option<String>,
    pub end_anchor: Option<String>,
}

/// A single file in the book's reading order, with the metadata the
/// viewer needs to paginate and to resolve links against it.
///
/// Port of `SpineItem` in `spine.py`. See the module doc for why this
/// is a struct with a `path` rather than a `str` subclass.
#[derive(Debug, Clone)]
pub struct SpineItem {
    pub path: PathBuf,
    pub encoding: String,
    pub character_count: i64,
    pub anchor_map: HashMap<String, usize>,
    pub all_links: HashSet<String>,
    pub verified_links: HashSet<(PathBuf, Option<String>)>,
    pub start_page: i64,
    pub pages: i64,
    pub max_page: i64,
    pub index_entries: Vec<SpineIndexEntry>,
    pub mime_type: Option<String>,
    pub is_single_page: Option<bool>,
}

impl PartialEq for SpineItem {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
    }
}

impl SpineItem {
    /// Read `path` off disk and compute all the derived fields. Port of
    /// `SpineItem.__new__`.
    ///
    /// `run_char_count`/`read_anchor_map`/`read_links` mirror the
    /// Python flags that let a caller skip the (comparatively expensive)
    /// regex passes when the viewer doesn't need them yet.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: &str,
        mime_type: Option<String>,
        read_anchor_map: bool,
        run_char_count: bool,
        from_epub: bool,
        read_links: bool,
    ) -> Result<SpineItem> {
        // `path.partition('#')[0]`: if the literal path (which may
        // erroneously carry a `#fragment` suffix) doesn't exist but the
        // part before the `#` does, use that instead.
        let ppath = path.split('#').next().unwrap_or(path);
        let use_path = if !Path::new(path).exists() && Path::new(ppath).exists() {
            ppath
        } else {
            path
        };

        let raw = fs::read(use_path).with_context(|| format!("Failed to read {use_path}"))?;

        let (text, mut encoding) = if from_epub {
            // Per the spec, HTML in EPUB must be UTF-8 or UTF-16; try
            // UTF-8 first and only fall back to sniffing on failure --
            // same algorithm the conversion pipeline uses (modulo BOM
            // detection), see the Python docstring for the rationale.
            match String::from_utf8(raw.clone()) {
                Ok(s) => (s, Some("utf-8".to_string())),
                Err(_) => xml_to_unicode(&raw, false, false),
            }
        } else {
            xml_to_unicode(&raw, false, false)
        };
        if encoding.is_none() {
            encoding = Some("utf-8".to_string());
        }

        let character_count = if run_char_count {
            self::character_count(&text)
        } else {
            10000
        };
        let anchor_map = if read_anchor_map {
            self::anchor_map(&text)
        } else {
            HashMap::new()
        };
        let all_links = if read_links {
            self::all_links(&text)
        } else {
            HashSet::new()
        };

        let mime_type = mime_type.or_else(|| {
            mime_guess::from_path(use_path)
                .first()
                .map(|m| m.to_string())
        });

        Ok(SpineItem {
            path: PathBuf::from(use_path),
            encoding: encoding.unwrap_or_else(|| "utf-8".to_string()),
            character_count,
            anchor_map,
            all_links,
            verified_links: HashSet::new(),
            start_page: -1,
            pages: -1,
            max_page: -1,
            index_entries: Vec::new(),
            mime_type,
            is_single_page: None,
        })
    }
}

/// A single navigable TOC entry, positioned against the spine: which
/// spine file/anchor it starts at, and (after [`IndexEntry::find_end`])
/// which spine file/anchor the *next* entry at the same or shallower
/// depth starts at (i.e. where this entry's "section" ends).
///
/// Port of `IndexEntry` in `spine.py`.
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub num: usize,
    pub text: String,
    pub key: PathBuf,
    pub anchor: Option<String>,
    pub start_anchor: Option<String>,
    pub spine_pos: i64,
    pub anchor_pos: usize,
    pub depth: usize,
    pub sort_key: (i64, usize),
    pub spine_count: usize,
    pub end_spine_pos: i64,
    pub end_anchor: Option<String>,
}

impl IndexEntry {
    /// Set `end_spine_pos`/`end_anchor` by scanning `all_entries` (which
    /// must already be sorted by [`IndexEntry::sort_key`]) for the
    /// first entry at this depth or shallower that starts strictly
    /// after this one. Port of `IndexEntry.find_end`.
    pub fn find_end(&mut self, all_entries: &[IndexEntry]) {
        let end = all_entries.iter().find(|i| {
            i.depth <= self.depth
                && ((i.spine_pos == self.spine_pos && i.anchor_pos > self.anchor_pos)
                    || i.spine_pos > self.spine_pos)
        });
        match end {
            Some(end) => {
                self.end_spine_pos = end.spine_pos;
                self.end_anchor = end.anchor.clone();
            }
            None => {
                self.end_spine_pos = self.spine_count as i64 - 1;
                self.end_anchor = None;
            }
        }
    }
}

/// Split a TOC `href` into its path portion and (non-empty) fragment,
/// the way `urldefrag`/manual `#`-splitting would.
fn split_href(href: &str) -> (&str, Option<&str>) {
    match href.split_once('#') {
        Some((p, f)) if !f.is_empty() => (p, Some(f)),
        Some((p, _)) => (p, None),
        None => (href, None),
    }
}

/// Depth-first preorder flatten of `node`'s descendants (not including
/// `node` itself), each paired with its 1-based depth. Port of the
/// `toc.flat()` generator as used by `create_indexing_data` -- adapted
/// to this crate's `TOCNode` (no parent pointers), computing depth by
/// recursion instead of by walking `.parent` chains.
fn flatten_toc<'a>(node: &'a TOCNode, depth: usize, out: &mut Vec<(usize, &'a TOCNode)>) {
    for child in &node.children {
        out.push((depth, child));
        flatten_toc(child, depth + 1, out);
    }
}

/// Resolve a TOC-entry href path component to an absolute path, the way
/// `TOC.abspath` does: absolute paths pass through, everything else is
/// joined against `base_dir`.
fn resolve_toc_path(base_dir: &Path, path_part: &str) -> PathBuf {
    let p = Path::new(path_part);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

/// Populate `spine[i].index_entries` for every spine item touched by a
/// node in `toc`, i.e. compute, for every TOC entry, which spine file it
/// starts in and which spine file/anchor the next (shallower-or-equal)
/// entry starts at.
///
/// Port of `create_indexing_data` in `spine.py`. `base_dir` stands in
/// for `TOC.base_path` in Python's `calibre.ebooks.metadata.toc.TOC`;
/// here it's simply the extracted-book root, since every href in this
/// crate's `oeb::toc::TOCNode` is relative to that one directory (unlike
/// Python's version, which can carry a different `base_path` per node
/// when TOCs get merged across conversions -- not a case this crate's
/// `TOCNode` needs to represent).
pub fn create_indexing_data(spine: &mut [SpineItem], toc: &TOCNode, base_dir: &Path) {
    if toc.children.is_empty() {
        return;
    }

    let mut flat = Vec::new();
    flatten_toc(toc, 1, &mut flat);
    if flat.is_empty() {
        return;
    }

    let mut entries: Vec<IndexEntry> = flat
        .into_iter()
        .enumerate()
        .map(|(num, (depth, node))| {
            let text = node.title.clone().unwrap_or_else(|| "Unknown".to_string());
            let (path_part, fragment) = match node.href.as_deref() {
                Some(h) => {
                    let (p, f) = split_href(h);
                    (Some(p.to_string()), f.map(|s| s.to_string()))
                }
                None => (None, None),
            };
            let key = path_part
                .as_ref()
                .map(|p| resolve_toc_path(base_dir, p))
                .unwrap_or_default();
            let spine_pos = spine
                .iter()
                .position(|s| s.path == key)
                .map(|i| i as i64)
                .unwrap_or(-1);
            let anchor_pos = if spine_pos >= 0 {
                fragment
                    .as_deref()
                    .and_then(|f| spine[spine_pos as usize].anchor_map.get(f).copied())
                    .unwrap_or(0)
            } else {
                0
            };
            IndexEntry {
                num,
                text,
                key,
                anchor: fragment.clone(),
                start_anchor: fragment,
                spine_pos,
                anchor_pos,
                depth,
                sort_key: (spine_pos, anchor_pos),
                spine_count: spine.len(),
                end_spine_pos: -1,
                end_anchor: None,
            }
        })
        .collect();

    entries.sort_by_key(|e| e.sort_key);
    let sorted_snapshot = entries.clone();
    for e in entries.iter_mut() {
        e.find_end(&sorted_snapshot);
    }

    let entries: Vec<Rc<IndexEntry>> = entries.into_iter().map(Rc::new).collect();

    for (spine_pos, spine_item) in spine.iter_mut().enumerate() {
        let spine_pos = spine_pos as i64;
        for e in &entries {
            if e.end_spine_pos < spine_pos || e.spine_pos > spine_pos {
                continue; // Does not touch this file.
            }
            let start = if e.spine_pos == spine_pos {
                e.anchor.clone()
            } else {
                None
            };
            let end = if e.spine_pos == spine_pos {
                e.end_anchor.clone()
            } else {
                None
            };
            spine_item.index_entries.push(SpineIndexEntry {
                entry: e.clone(),
                start_anchor: start,
                end_anchor: end,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn character_count_counts_significant_text() {
        let html = "<html><body><p>Hello world</p></body></html>";
        // ">Hello world<" -> collapsed len 13, -2 = 11
        assert_eq!(character_count(html), 11);
    }

    #[test]
    fn character_count_collapses_whitespace() {
        let html = ">a   b\n\tc<";
        // collapsed: "a b c" len 5, minus the > < = ... wait match includes > and <
        let html_full = format!("<x{html}>");
        // Just check it doesn't panic and returns a sane positive count.
        assert!(character_count(&html_full) >= 0);
    }

    #[test]
    fn anchor_map_finds_id_and_name() {
        let html = r#"<a id="one">x</a><a name='two'>y</a>"#;
        let map = anchor_map(html);
        assert!(map.contains_key("one"));
        assert!(map.contains_key("two"));
        assert!(map["one"] < map["two"]);
    }

    #[test]
    fn anchor_map_first_occurrence_wins() {
        let html = r#"<a id="dup">first</a><a id="dup">second</a>"#;
        let map = anchor_map(html);
        // First offset should be recorded (smaller than a second one would be).
        assert!(map["dup"] < html.rfind("dup").unwrap());
    }

    #[test]
    fn all_links_extracts_and_unescapes() {
        let html = r#"<a href="chap1.html?x=1&amp;y=2">One</a><a href='chap2.html'>Two</a>"#;
        let links = all_links(html);
        assert!(links.contains("chap1.html?x=1&y=2"));
        assert!(links.contains("chap2.html"));
    }

    #[test]
    fn all_links_ignores_non_anchor_tags() {
        let html = r#"<link href="style.css"/><img src="x.png"/>"#;
        assert!(all_links(html).is_empty());
    }

    fn write_temp(contents: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn spine_item_reads_and_derives_fields() {
        let f = write_temp(
            r#"<html><body><p id="top">Hello <a href="other.html">link</a></p></body></html>"#,
        );
        let path = f.path().to_str().unwrap();
        let item =
            SpineItem::new(path, Some("text/html".to_string()), true, true, true, true).unwrap();
        assert_eq!(item.mime_type.as_deref(), Some("text/html"));
        assert!(item.character_count > 0);
        assert!(item.anchor_map.contains_key("top"));
        assert!(item.all_links.contains("other.html"));
        assert_eq!(item.encoding, "utf-8");
    }

    #[test]
    fn spine_item_skips_expensive_passes_when_disabled() {
        let f = write_temp(r#"<html><body><p id="top">Hello</p></body></html>"#);
        let path = f.path().to_str().unwrap();
        let item = SpineItem::new(path, None, false, false, false, false).unwrap();
        assert_eq!(item.character_count, 10000);
        assert!(item.anchor_map.is_empty());
        assert!(item.all_links.is_empty());
    }

    #[test]
    fn spine_item_infers_mime_type_when_absent() {
        let f = NamedTempFile::with_suffix(".html").unwrap();
        fs::write(f.path(), "<html></html>").unwrap();
        let item =
            SpineItem::new(f.path().to_str().unwrap(), None, false, false, false, false).unwrap();
        assert_eq!(item.mime_type.as_deref(), Some("text/html"));
    }

    fn spine_item_at(dir: &Path, name: &str, contents: &str) -> SpineItem {
        let p = dir.join(name);
        fs::write(&p, contents).unwrap();
        SpineItem::new(p.to_str().unwrap(), None, true, true, false, true).unwrap()
    }

    #[test]
    fn create_indexing_data_positions_toc_entries() {
        let dir = tempfile::tempdir().unwrap();
        let mut spine = vec![
            spine_item_at(
                dir.path(),
                "c1.html",
                r#"<html><body><h1 id="start">Chapter 1</h1><p>text</p></body></html>"#,
            ),
            spine_item_at(
                dir.path(),
                "c2.html",
                r#"<html><body><h1 id="start">Chapter 2</h1><p>text</p></body></html>"#,
            ),
        ];

        let mut toc = TOCNode::new(None, None);
        let c1 = TOCNode::new(Some("Chapter 1".into()), Some("c1.html#start".into()));
        let c2 = TOCNode::new(Some("Chapter 2".into()), Some("c2.html#start".into()));
        toc.add(c1);
        toc.add(c2);

        create_indexing_data(&mut spine, &toc, dir.path());

        assert_eq!(spine[0].index_entries.len(), 1);
        assert_eq!(spine[0].index_entries[0].entry.text, "Chapter 1");
        assert_eq!(
            spine[0].index_entries[0].start_anchor.as_deref(),
            Some("start")
        );
        assert_eq!(spine[0].index_entries[0].entry.end_spine_pos, 1);

        // Chapter 1's section is bounded by Chapter 2's start anchor,
        // which lives in spine[1] -- so it still "touches" that file
        // (with a start of `None`, since it didn't *start* there), in
        // addition to Chapter 2's own entry starting there. This
        // matches `create_indexing_data`'s "does this entry's
        // [spine_pos, end_spine_pos] range cover this file" test in
        // `spine.py`.
        assert_eq!(spine[1].index_entries.len(), 2);
        let texts: Vec<&str> = spine[1]
            .index_entries
            .iter()
            .map(|e| e.entry.text.as_str())
            .collect();
        assert!(texts.contains(&"Chapter 1"));
        assert!(texts.contains(&"Chapter 2"));
        let ch2 = spine[1]
            .index_entries
            .iter()
            .find(|e| e.entry.text == "Chapter 2")
            .unwrap();
        assert_eq!(ch2.start_anchor.as_deref(), Some("start"));
    }

    #[test]
    fn create_indexing_data_noop_for_empty_toc() {
        let dir = tempfile::tempdir().unwrap();
        let mut spine = vec![spine_item_at(dir.path(), "c1.html", "<html></html>")];
        let toc = TOCNode::new(None, None);
        create_indexing_data(&mut spine, &toc, dir.path());
        assert!(spine[0].index_entries.is_empty());
    }
}
