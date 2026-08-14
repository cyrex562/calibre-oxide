//! Reading and writing viewer bookmarks, including a copy embedded
//! inside the book file itself.
//!
//! Port of `old_src/src/calibre/ebooks/oeb/iterator/bookmarks.py`.
//!
//! Python's `BookmarksMixin` is mixed into `EbookIterator` via multiple
//! inheritance. Rust has no mixins, so this is a trait
//! ([`BookmarksMixin`]) with a handful of required accessors (the state
//! `EbookIterator` owns) and default-implemented methods carrying the
//! actual logic -- `impl BookmarksMixin for EbookIterator {}` then only
//! needs to supply the accessors, exactly mirroring how little the
//! Python mixin required of its host class.
//!
//! # A deliberate deviation from Python: no silent failures
//!
//! Python's `save_bookmarks` writes the bookmark blob to the durable
//! `DynamicConfig` store unconditionally, then makes a *best-effort*
//! attempt to also embed it in the EPUB file, silently swallowing any
//! `OSError` from that second step (`except OSError: return`) since
//! it's a secondary convenience copy, not the source of truth.
//!
//! This port keeps the two-tier design (config store is authoritative;
//! the in-file copy is a convenience mirror) but does **not** swallow
//! the in-file write's errors: both writes return `Result` and both
//! propagate failures to the caller. `docs/FAULT_TOLERANCE.md` calls
//! out "no silent fallbacks" for any write path that touches a file the
//! user owns, and rewriting bytes inside the user's actual book file
//! qualifies even though this isn't a Calibre *library* path. A caller
//! that wants Python's original best-effort shrug can simply ignore the
//! `Result`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use calibre_utils::config::DynamicConfig;

use crate::zipfile_safe_replace::safe_replace;

/// `BM_FIELD_SEP` in `bookmarks.py`: the field separator used by the
/// modern ("cfi") bookmark serialization.
pub const BM_FIELD_SEP: &str = "*|!|?|*";
/// `BM_LEGACY_ESC` in `bookmarks.py`: stand-in text used to escape a
/// literal `^` inside a serialized `pos` string (since `^` is otherwise
/// meaningful only in the legacy format, this is defensive rather than
/// load-bearing for the `cfi` format's own delimiter, which is
/// [`BM_FIELD_SEP`]).
pub const BM_LEGACY_ESC: &str = "esc-text-%&*#%(){}ads19-end-esc";

/// The two bookmark record shapes `parse_bookmarks` recognizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BookmarkKind {
    /// `title^spine#pos`, from older calibre versions.
    Legacy,
    /// `title*|!|?|*spine*|!|?|*pos`, an EPUB CFI or scroll fraction.
    Cfi,
}

/// A bookmark's `pos` field: either free text (a CFI, or a legacy
/// position string) or a numeric scroll fraction. Mirrors Python's
/// dynamically-typed `pos`, which is a `str` unless it happens to parse
/// as `float`.
#[derive(Debug, Clone, PartialEq)]
pub enum BookmarkPos {
    Text(String),
    Number(f64),
}

impl BookmarkPos {
    /// Try to reinterpret a text position as a number, the way
    /// `parse_bookmarks`'s `try: pos = float(pos) except Exception: pass`
    /// does. Leaves `Number` values untouched.
    fn renumber(self) -> Self {
        match self {
            BookmarkPos::Text(s) => match s.trim().parse::<f64>() {
                Ok(n) => BookmarkPos::Number(n),
                Err(_) => BookmarkPos::Text(s),
            },
            n => n,
        }
    }
}

/// One saved reading position. Port of the `dict` records
/// `parse_bookmarks`/`serialize_bookmarks` produce and consume.
#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub kind: BookmarkKind,
    pub title: String,
    pub spine: i64,
    pub pos: BookmarkPos,
}

/// Parse a raw bookmarks blob (the contents of
/// `META-INF/calibre_bookmarks.txt`, or the `DynamicConfig`-stored
/// copy) into [`Bookmark`]s. Port of the module-level `parse_bookmarks`
/// generator function in `bookmarks.py`.
///
/// Malformed lines (in either format) are silently skipped, matching
/// Python's bare `except Exception: continue` -- a resilience choice
/// inherent to the format itself (old bookmark files may have partial
/// writes or unrecognized rows), not a case this port should turn into
/// a hard error.
pub fn parse_bookmarks(raw: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    for line in raw.lines() {
        if let Some(pos_caret) = line.rfind('^') {
            let title = line[..pos_caret].to_string();
            let ref_part = &line[pos_caret + 1..];
            let (spine_str, pos) = match ref_part.split_once('#') {
                Some((s, p)) => (s, p.to_string()),
                None => (ref_part, String::new()),
            };
            match spine_str.trim().parse::<i64>() {
                Ok(spine) => out.push(Bookmark {
                    kind: BookmarkKind::Legacy,
                    title,
                    spine,
                    pos: BookmarkPos::Text(pos),
                }),
                Err(_) => continue,
            }
        } else if line.contains(BM_FIELD_SEP) {
            let parts: Vec<&str> = line.trim().split(BM_FIELD_SEP).collect();
            if parts.len() != 3 {
                continue;
            }
            let (title, spine_str, pos_str) = (parts[0], parts[1], parts[2]);
            let spine: i64 = match spine_str.parse() {
                Ok(s) => s,
                Err(_) => continue,
            };
            let pos = pos_str.replace(BM_LEGACY_ESC, "^");
            out.push(Bookmark {
                kind: BookmarkKind::Cfi,
                title: title.to_string(),
                spine,
                pos: BookmarkPos::Text(pos).renumber(),
            });
        }
    }
    out
}

/// Serialize a list of [`Bookmark`]s back to the on-disk text format.
/// Port of `BookmarksMixin.serialize_bookmarks`.
pub fn serialize_bookmarks(bookmarks: &[Bookmark]) -> String {
    let mut dat: Vec<String> = Vec::with_capacity(bookmarks.len());
    for bm in bookmarks {
        let rec = match bm.kind {
            BookmarkKind::Legacy => {
                let pos = match &bm.pos {
                    BookmarkPos::Text(s) => s.clone(),
                    BookmarkPos::Number(n) => n.to_string(),
                };
                format!("{}^{}#{}", bm.title, bm.spine, pos)
            }
            BookmarkKind::Cfi => {
                let pos = match &bm.pos {
                    BookmarkPos::Number(n) => n.to_string(),
                    BookmarkPos::Text(s) => s.replace('^', BM_LEGACY_ESC),
                };
                [bm.title.clone(), bm.spine.to_string(), pos].join(BM_FIELD_SEP)
            }
        };
        dat.push(rec);
    }
    let mut s = dat.join("\n");
    s.push('\n');
    s
}

/// The state a [`BookmarksMixin`] host must expose. `EbookIterator`
/// (`oeb::iterator::book`) implements this with plain field accessors.
pub trait BookmarksMixin {
    /// The original book file (`EbookIterator.pathtoebook`).
    fn pathtoebook(&self) -> &Path;
    /// The exploded-book root directory (`EbookIterator.base`), used to
    /// look for `META-INF/calibre_bookmarks.txt`.
    fn base(&self) -> &Path;
    /// The `"iterator"`-scoped [`DynamicConfig`] store.
    fn config(&self) -> &DynamicConfig;
    /// `EbookIterator.copy_bookmarks_to_file`.
    fn copy_bookmarks_to_file(&self) -> bool;
    fn bookmarks(&self) -> &[Bookmark];
    fn bookmarks_mut(&mut self) -> &mut Vec<Bookmark>;

    /// The `DynamicConfig` key this book's bookmarks are stored under:
    /// `'bookmarks_' + pathtoebook` in Python.
    fn bookmarks_config_key(&self) -> String {
        format!("bookmarks_{}", self.pathtoebook().display())
    }

    /// Load bookmarks from the config store, falling back to the
    /// in-file copy if the store has none yet. Port of
    /// `BookmarksMixin.read_bookmarks`.
    fn read_bookmarks(&mut self) -> Result<()> {
        self.bookmarks_mut().clear();
        let mut raw = self.config().get(&self.bookmarks_config_key());
        if raw.as_deref().unwrap_or("").is_empty() {
            let bmfile = self.base().join("META-INF").join("calibre_bookmarks.txt");
            if bmfile.exists() {
                let bytes = fs::read(&bmfile)
                    .with_context(|| format!("Failed to read {}", bmfile.display()))?;
                // Defensively lossy-decode rather than hard-failing on
                // malformed UTF-8 (Python's `.decode('utf-8')` would
                // raise here) -- a corrupt bookmarks sidecar shouldn't
                // block opening the book.
                raw = Some(String::from_utf8_lossy(&bytes).into_owned());
            }
        }
        let bms = parse_bookmarks(&raw.unwrap_or_default());
        *self.bookmarks_mut() = bms;
        Ok(())
    }

    /// Persist `bookmarks` (or, if `None`, the current in-memory list)
    /// to the config store and, for a writable EPUB, into the book file
    /// itself. Port of `BookmarksMixin.save_bookmarks`; see the module
    /// doc for how error handling differs from Python here.
    fn save_bookmarks(&self, bookmarks: Option<&[Bookmark]>, no_copy_to_file: bool) -> Result<()> {
        let dat = serialize_bookmarks(bookmarks.unwrap_or_else(|| self.bookmarks()));
        self.config()
            .set(&self.bookmarks_config_key(), &dat)
            .context("Failed to persist bookmarks to config store")?;

        let is_epub = self
            .pathtoebook()
            .extension()
            .map(|e| e.eq_ignore_ascii_case("epub"))
            .unwrap_or(false);
        if !no_copy_to_file && self.copy_bookmarks_to_file() && is_epub {
            safe_replace(
                self.pathtoebook(),
                "META-INF/calibre_bookmarks.txt",
                dat.as_bytes(),
                true,
            )
            .with_context(|| {
                format!(
                    "Failed to embed bookmarks in {}",
                    self.pathtoebook().display()
                )
            })?;
        }
        Ok(())
    }

    /// Replace any existing bookmark with the same title, then persist.
    /// Port of `BookmarksMixin.add_bookmark`.
    fn add_bookmark(&mut self, bm: Bookmark, no_copy_to_file: bool) -> Result<()> {
        self.bookmarks_mut().retain(|x| x.title != bm.title);
        self.bookmarks_mut().push(bm);
        self.save_bookmarks(None, no_copy_to_file)
    }

    /// Replace the in-memory bookmark list without persisting. Port of
    /// `BookmarksMixin.set_bookmarks`.
    fn set_bookmarks(&mut self, bookmarks: Vec<Bookmark>) {
        *self.bookmarks_mut() = bookmarks;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_bookmark() {
        let raw = "My Title^3#some-pos\n";
        let bms = parse_bookmarks(raw);
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].kind, BookmarkKind::Legacy);
        assert_eq!(bms[0].title, "My Title");
        assert_eq!(bms[0].spine, 3);
        assert_eq!(bms[0].pos, BookmarkPos::Text("some-pos".to_string()));
    }

    #[test]
    fn parse_legacy_bookmark_no_fragment() {
        let raw = "Title^5\n";
        let bms = parse_bookmarks(raw);
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].spine, 5);
        assert_eq!(bms[0].pos, BookmarkPos::Text(String::new()));
    }

    #[test]
    fn parse_legacy_bookmark_bad_spine_skipped() {
        let raw = "Title^notanumber#pos\n";
        assert!(parse_bookmarks(raw).is_empty());
    }

    #[test]
    fn parse_cfi_bookmark_text_pos() {
        let raw = format!("Chapter One{BM_FIELD_SEP}2{BM_FIELD_SEP}/2/4/6:10\n");
        let bms = parse_bookmarks(&raw);
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].kind, BookmarkKind::Cfi);
        assert_eq!(bms[0].title, "Chapter One");
        assert_eq!(bms[0].spine, 2);
        assert_eq!(bms[0].pos, BookmarkPos::Text("/2/4/6:10".to_string()));
    }

    #[test]
    fn parse_cfi_bookmark_numeric_pos() {
        let raw = format!("Progress{BM_FIELD_SEP}0{BM_FIELD_SEP}0.75\n");
        let bms = parse_bookmarks(&raw);
        assert_eq!(bms[0].pos, BookmarkPos::Number(0.75));
    }

    #[test]
    fn parse_cfi_bookmark_unescapes_caret() {
        let raw = format!("T{BM_FIELD_SEP}0{BM_FIELD_SEP}a{BM_LEGACY_ESC}b\n");
        let bms = parse_bookmarks(&raw);
        assert_eq!(bms[0].pos, BookmarkPos::Text("a^b".to_string()));
    }

    #[test]
    fn parse_cfi_bookmark_bad_spine_skipped() {
        let raw = format!("T{BM_FIELD_SEP}notanumber{BM_FIELD_SEP}pos\n");
        assert!(parse_bookmarks(&raw).is_empty());
    }

    #[test]
    fn serialize_legacy_roundtrip() {
        let bms = vec![Bookmark {
            kind: BookmarkKind::Legacy,
            title: "Foo".to_string(),
            spine: 2,
            pos: BookmarkPos::Text("bar".to_string()),
        }];
        let dat = serialize_bookmarks(&bms);
        assert_eq!(dat, "Foo^2#bar\n");
        let back = parse_bookmarks(&dat);
        assert_eq!(back, bms);
    }

    #[test]
    fn serialize_cfi_text_roundtrip() {
        let bms = vec![Bookmark {
            kind: BookmarkKind::Cfi,
            title: "Foo".to_string(),
            spine: 1,
            pos: BookmarkPos::Text("/2/4:10".to_string()),
        }];
        let dat = serialize_bookmarks(&bms);
        let back = parse_bookmarks(&dat);
        assert_eq!(back, bms);
    }

    #[test]
    fn serialize_cfi_numeric_roundtrip() {
        let bms = vec![Bookmark {
            kind: BookmarkKind::Cfi,
            title: "Foo".to_string(),
            spine: 1,
            pos: BookmarkPos::Number(0.5),
        }];
        let dat = serialize_bookmarks(&bms);
        let back = parse_bookmarks(&dat);
        assert_eq!(back, bms);
    }

    #[test]
    fn serialize_cfi_escapes_caret_in_text_pos() {
        let bms = vec![Bookmark {
            kind: BookmarkKind::Cfi,
            title: "Foo".to_string(),
            spine: 0,
            pos: BookmarkPos::Text("a^b".to_string()),
        }];
        let dat = serialize_bookmarks(&bms);
        assert!(dat.contains(BM_LEGACY_ESC));
        assert!(!dat.contains("a^b"));
        let back = parse_bookmarks(&dat);
        assert_eq!(back, bms);
    }

    #[test]
    fn serialize_multiple_bookmarks_one_per_line() {
        let bms = vec![
            Bookmark {
                kind: BookmarkKind::Legacy,
                title: "A".to_string(),
                spine: 0,
                pos: BookmarkPos::Text(String::new()),
            },
            Bookmark {
                kind: BookmarkKind::Cfi,
                title: "B".to_string(),
                spine: 1,
                pos: BookmarkPos::Number(1.0),
            },
        ];
        let dat = serialize_bookmarks(&bms);
        assert_eq!(dat.lines().count(), 2);
        let back = parse_bookmarks(&dat);
        assert_eq!(back, bms);
    }

    // --- BookmarksMixin, exercised through a minimal test host ---

    struct TestHost {
        pathtoebook: std::path::PathBuf,
        base: std::path::PathBuf,
        config: DynamicConfig,
        copy_to_file: bool,
        bookmarks: Vec<Bookmark>,
    }

    impl BookmarksMixin for TestHost {
        fn pathtoebook(&self) -> &Path {
            &self.pathtoebook
        }
        fn base(&self) -> &Path {
            &self.base
        }
        fn config(&self) -> &DynamicConfig {
            &self.config
        }
        fn copy_bookmarks_to_file(&self) -> bool {
            self.copy_to_file
        }
        fn bookmarks(&self) -> &[Bookmark] {
            &self.bookmarks
        }
        fn bookmarks_mut(&mut self) -> &mut Vec<Bookmark> {
            &mut self.bookmarks
        }
    }

    fn test_host(dir: &Path) -> TestHost {
        TestHost {
            pathtoebook: dir.join("book.txt"),
            base: dir.join("extracted"),
            config: DynamicConfig::at_path(dir.join("iterator.json")),
            copy_to_file: true,
            bookmarks: Vec::new(),
        }
    }

    #[test]
    fn read_bookmarks_empty_when_nothing_saved() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = test_host(dir.path());
        host.read_bookmarks().unwrap();
        assert!(host.bookmarks().is_empty());
    }

    #[test]
    fn save_then_read_bookmarks_via_config_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = test_host(dir.path());
        host.copy_to_file = false; // pathtoebook isn't a real epub here
        let bm = Bookmark {
            kind: BookmarkKind::Cfi,
            title: "Start".to_string(),
            spine: 0,
            pos: BookmarkPos::Number(0.1),
        };
        host.add_bookmark(bm.clone(), false).unwrap();

        let mut host2 = test_host(dir.path());
        host2.read_bookmarks().unwrap();
        assert_eq!(host2.bookmarks(), &[bm]);
    }

    #[test]
    fn add_bookmark_replaces_same_title() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = test_host(dir.path());
        host.copy_to_file = false;
        host.add_bookmark(
            Bookmark {
                kind: BookmarkKind::Cfi,
                title: "Start".to_string(),
                spine: 0,
                pos: BookmarkPos::Number(0.1),
            },
            false,
        )
        .unwrap();
        host.add_bookmark(
            Bookmark {
                kind: BookmarkKind::Cfi,
                title: "Start".to_string(),
                spine: 2,
                pos: BookmarkPos::Number(0.9),
            },
            false,
        )
        .unwrap();
        assert_eq!(host.bookmarks().len(), 1);
        assert_eq!(host.bookmarks()[0].spine, 2);
    }

    #[test]
    fn falls_back_to_in_file_bookmarks_when_config_empty() {
        let dir = tempfile::tempdir().unwrap();
        let mut host = test_host(dir.path());
        let meta_inf = host.base.join("META-INF");
        fs::create_dir_all(&meta_inf).unwrap();
        fs::write(meta_inf.join("calibre_bookmarks.txt"), "InFile^1#pos\n").unwrap();

        host.read_bookmarks().unwrap();
        assert_eq!(host.bookmarks().len(), 1);
        assert_eq!(host.bookmarks()[0].title, "InFile");
    }

    #[test]
    fn save_bookmarks_embeds_into_real_epub() {
        let dir = tempfile::tempdir().unwrap();
        let epub_path = dir.path().join("book.epub");
        {
            use std::io::Write as _;
            let file = fs::File::create(&epub_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            zip.start_file("mimetype", zip::write::FileOptions::default())
                .unwrap();
            zip.write_all(b"application/epub+zip").unwrap();
            zip.finish().unwrap();
        }
        let mut host = test_host(dir.path());
        host.pathtoebook = epub_path.clone();
        host.add_bookmark(
            Bookmark {
                kind: BookmarkKind::Cfi,
                title: "X".to_string(),
                spine: 0,
                pos: BookmarkPos::Number(0.0),
            },
            false,
        )
        .unwrap();

        let file = fs::File::open(&epub_path).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name("META-INF/calibre_bookmarks.txt").unwrap();
        let mut s = String::new();
        std::io::Read::read_to_string(&mut entry, &mut s).unwrap();
        assert!(s.contains("X"));
    }
}
