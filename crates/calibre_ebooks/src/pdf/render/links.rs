//! Port of `old_src/src/calibre/ebooks/pdf/render/links.py` (144 lines).
//!
//! `Destination` and `Links`: builds PDF link annotations and the
//! document outline (bookmarks) tree from calibre's internal
//! href/anchor/TOC bookkeeping.
//!
//! Ported for real, no gaps.
//!
//! # Restructuring: the `Links.pdf` back-reference
//!
//! Python's `Links.__init__` stores `self.pdf` (a back-reference to the
//! owning `PDFStream`) and later calls `self.pdf.get_pageref(...)`,
//! `self.pdf.objects.add(...)`, `self.pdf.catalog`, `self.pdf.debug(...)`
//! from `add`/`add_links`/`add_outline`. A literal back-reference of that
//! shape is a self-referential-struct problem in Rust (`Links` would need
//! to borrow from the very `PdfStream` that owns it).
//!
//! Instead, [`Links`] holds no back-reference; the methods that need the
//! owner (`add`, `add_links`, `add_outline`) take the specific pieces
//! they need as explicit parameters: a page-ref lookup closure, a
//! `&mut IndirectObjects` (see `super::serialize`) for allocating/
//! mutating indirect objects, and a debug-log closure. This mirrors how
//! `serialize::PdfStream` ends up structured (it owns a plain `Links`
//! value and passes itself/its pieces explicitly at call sites) and is a
//! deliberate translation choice, not a workaround.
//!
//! Grepping `old_src/src/calibre/ebooks/pdf/` confirms `Links.add` is
//! never called from within these six files - it's called by whatever
//! HTML-to-PDF page-assembly code drives rendering (a `weprint.py`-style
//! file, out of scope for this issue). Only `Links.add_links()` is called
//! here, from `serialize::PdfStream::end`.
//!
//! # `Links.pdf.objects.add(annot).obj[...] = ...`-style post-add mutation
//!
//! Python's `IndirectObjects.add` returns a `Reference` whose `.obj`
//! field is a live handle to the object just added, mutated in place by
//! later code (e.g. `process_children` sets `childref.obj['Next'] =
//! ...`, `add_outline` sets `self.pdf.catalog.obj['Outlines'] =
//! parentref`). Here, [`super::serialize::IndirectObjects`] is an
//! arena keyed by [`super::common::Reference`]; the equivalent mutation
//! goes through `objects.get_dict_mut(&reference)` instead of a stored
//! handle.

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Result};

use super::common::{log_warn, Array, Dictionary, Name, PdfString, Reference, Utf16String};
use super::serialize::IndirectObjects;

/// Port of the `pos` dict shape used throughout `links.py`
/// (`{'top':..., 'column':..., 'left':...}`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pos {
    pub top: f64,
    pub column: i64,
    pub left: f64,
}

/// Lexically absolutize+normalize a path the way Python's
/// `os.path.normcase(os.path.abspath(p))` does on POSIX (`normcase` is a
/// no-op there): join onto `cwd` if relative, then collapse `.`/`..`
/// components - purely lexically, no filesystem access (matching
/// `abspath`'s own behavior of not resolving symlinks).
pub fn abspath_normcase(p: &Path) -> PathBuf {
    let joined = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Port of `Destination` (links.py lines 15-31): a PDF `/XYZ` link
/// destination array, `[pageref /XYZ left top null]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Destination(pub Array);

impl Destination {
    /// Port of `Destination.__init__`. `get_pageref` mirrors the Python
    /// `IndexError`-on-miss behavior via `Option::None`. Mutates `pos`
    /// in place exactly like the Python original (on a fallback, `pos.left`
    /// and `pos.top` are zeroed - and since callers often pass in a
    /// long-lived `Pos`, e.g. `Links::start`, that zeroing is visible to
    /// later calls too, matching Python's shared-dict aliasing).
    ///
    /// Returns `Err` if no page reference at all can be found down to
    /// page 0 - the Rust analogue of the Python original's unhandled
    /// `UnboundLocalError` in that (pathological, empty-document) case.
    pub fn new(
        start_page: i64,
        pos: &mut Pos,
        get_pageref: impl Fn(i64) -> Option<Reference>,
    ) -> Result<Destination> {
        let pnum = start_page + pos.column.max(0);
        let mut q = pnum;
        let mut pref = None;
        while q > -1 {
            if let Some(r) = get_pageref(q) {
                pref = Some(r);
                break;
            }
            pos.left = 0.0;
            pos.top = 0.0;
            q -= 1;
        }
        let pref = pref.ok_or_else(|| anyhow!("Could not find any page for link destination"))?;
        if q != pnum {
            log_warn(&format!(
                "Could not find page {pnum} for link destination, using page {q} instead"
            ));
        }
        let mut arr = Array::new();
        arr.push(pref);
        arr.push(Name::new("XYZ"));
        arr.push(pos.left);
        arr.push(pos.top);
        arr.push(super::common::PdfObj::Null);
        Ok(Destination(arr))
    }
}

/// A `(href, page_num, rect)` link, port of the `(href, page, rect)`
/// tuples in `links.py`'s `links` list (link.py's `add` parameter).
#[derive(Debug, Clone)]
pub struct LinkSpec {
    pub href: String,
    pub page: i64,
    pub rect: Vec<f64>,
}

#[derive(Debug, Clone)]
struct PendingLink {
    path: PathBuf,
    href: String,
    frag: Option<String>,
    pageref: Reference,
    rect: Array,
}

/// Port of `Links` (links.py lines 34-145). See the module doc comment
/// for the back-reference restructuring.
pub struct Links {
    /// `path -> (anchor (None = whole-document) -> Destination)`.
    pub anchors: indexmap::IndexMap<PathBuf, indexmap::IndexMap<Option<String>, Destination>>,
    pending: Vec<PendingLink>,
    pub start: Pos,
    pub mark_links: bool,
}

impl Links {
    /// Port of `Links.__init__` (links.py lines 36-41).
    pub fn new(mark_links: bool, page_size: (f64, f64)) -> Self {
        Links {
            anchors: indexmap::IndexMap::new(),
            pending: Vec::new(),
            start: Pos {
                top: page_size.1,
                column: 0,
                left: 0.0,
            },
            mark_links,
        }
    }

    /// Port of `Links.add` (links.py lines 43-61).
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        base_path: &Path,
        start_page: i64,
        links: &[LinkSpec],
        anchors: &indexmap::IndexMap<String, Pos>,
        get_pageref: impl Fn(i64) -> Option<Reference>,
        mut debug: impl FnMut(&str),
    ) -> Result<()> {
        let path = abspath_normcase(base_path);
        let mut a: indexmap::IndexMap<Option<String>, Destination> = indexmap::IndexMap::new();
        a.insert(
            None,
            Destination::new(start_page, &mut self.start, &get_pageref)?,
        );
        for (anchor, pos) in anchors {
            let mut pos = *pos;
            a.insert(
                Some(anchor.clone()),
                Destination::new(start_page, &mut pos, &get_pageref)?,
            );
        }
        self.anchors.insert(path.clone(), a);

        for link in links {
            let (p, frag) = match link.href.split_once('#') {
                Some((p, frag)) => (
                    p.to_string(),
                    if frag.is_empty() {
                        None
                    } else {
                        Some(frag.to_string())
                    },
                ),
                None => (link.href.clone(), None),
            };
            let pageref = match get_pageref(link.page) {
                Some(r) => r,
                None => match get_pageref(link.page - 1) {
                    Some(r) => {
                        debug(&format!(
                            "The link {} points to non-existent page, moving it one page back",
                            link.href
                        ));
                        r
                    }
                    None => {
                        debug(&format!(
                            "Unable to find page for link: {link:?}, ignoring it"
                        ));
                        continue;
                    }
                },
            };
            self.pending.push(PendingLink {
                path: path.clone(),
                href: p,
                frag,
                pageref,
                rect: Array(
                    link.rect
                        .iter()
                        .map(|&v| super::common::PdfObj::Real(v))
                        .collect(),
                ),
            });
        }
        Ok(())
    }

    /// Port of `Links.add_links` (links.py lines 63-105).
    pub fn add_links(&mut self, objects: &mut IndirectObjects, mut debug: impl FnMut(&str)) {
        for link in self.pending.drain(..) {
            let combined_path = if link.href.is_empty() {
                link.path.clone()
            } else {
                let mut joined = link
                    .path
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                for part in urlencoding::decode(&link.href)
                    .unwrap_or_default()
                    .split('/')
                {
                    joined.push(part);
                }
                abspath_normcase(&joined)
            };
            let is_local = link.href.is_empty() || self.anchors.contains_key(&combined_path);

            let mut annot = Dictionary::new();
            annot.insert("Type", Name::new("Annot"));
            annot.insert("Subtype", Name::new("Link"));
            annot.insert("Rect", link.rect.clone());
            annot.insert("Border", {
                let mut a = Array::new();
                a.push(0i64);
                a.push(0i64);
                a.push(0i64);
                a
            });
            if self.mark_links {
                annot.insert("Border", {
                    let mut a = Array::new();
                    a.push(16i64);
                    a.push(16i64);
                    a.push(1i64);
                    a
                });
                annot.insert("C", {
                    let mut a = Array::new();
                    a.push(1.0f64);
                    a.push(0.0f64);
                    a.push(0.0f64);
                    a
                });
            }

            if is_local {
                let dest_path = if link.href.is_empty() {
                    &link.path
                } else {
                    &combined_path
                };
                let dest = self
                    .anchors
                    .get(dest_path)
                    .and_then(|a| a.get(&link.frag).or_else(|| a.get(&None)));
                if let Some(dest) = dest {
                    annot.insert("Dest", dest.0.clone());
                }
                // else: falls through with neither 'A' nor 'Dest' set,
                // matching Python's nested `except KeyError: pass`.
            } else {
                let url = match &link.frag {
                    Some(frag) => format!("{}#{}", link.href, frag),
                    None => link.href.clone(),
                };
                match url::Url::parse(&url) {
                    Ok(purl) => {
                        if !purl.scheme().is_empty() && purl.scheme() != "file" {
                            let mut action = Dictionary::new();
                            action.insert("Type", Name::new("Action"));
                            action.insert("S", Name::new("URI"));
                            action.insert("URI", PdfString::new(url.clone()));
                            annot.insert("A", action);
                        }
                    }
                    Err(_) => {
                        debug(&format!("Ignoring unparsable URL: {url:?}"));
                        continue;
                    }
                }
            }

            if annot.contains_key("A") || annot.contains_key("Dest") {
                let annot_ref = objects.add_dict(annot);
                let page = objects.get_dict_mut(&link.pageref);
                match page.get("Annots").cloned() {
                    Some(super::common::PdfObj::Array(mut arr)) => {
                        arr.push(annot_ref);
                        page.insert("Annots", arr);
                    }
                    _ => {
                        let mut arr = Array::new();
                        arr.push(annot_ref);
                        page.insert("Annots", arr);
                    }
                }
            } else {
                debug(&format!(
                    "Could not find destination for link: {} in file {}",
                    link.href,
                    link.path.display()
                ));
            }
        }
    }

    /// Port of `Links.add_outline` (links.py lines 107-111).
    pub fn add_outline(
        &mut self,
        toc: &[TocItem],
        objects: &mut IndirectObjects,
        catalog_ref: &Reference,
    ) {
        let mut parent = Dictionary::new();
        parent.insert("Type", Name::new("Outlines"));
        let parentref = objects.add_dict(parent);
        self.process_children(toc, &parentref, true, objects);
        objects
            .get_dict_mut(catalog_ref)
            .insert("Outlines", parentref);
    }

    /// Port of `Links.process_children` (links.py lines 113-130).
    fn process_children(
        &self,
        toc: &[TocItem],
        parentref: &Reference,
        parent_is_root: bool,
        objects: &mut IndirectObjects,
    ) {
        let mut childrefs: Vec<Reference> = Vec::new();
        for child in toc {
            let childref = match self.process_toc_item(child, parentref, objects) {
                Some(r) => r,
                None => continue,
            };
            if let Some(prev) = childrefs.last() {
                let prev = *prev;
                objects.get_dict_mut(&prev).insert("Next", childref);
                objects.get_dict_mut(&childref).insert("Prev", prev);
            }
            childrefs.push(childref);

            if !child.children.is_empty() {
                self.process_children(&child.children, &childref, false, objects);
            }
        }
        if let (Some(&first), Some(&last)) = (childrefs.first(), childrefs.last()) {
            let count = childrefs.len();
            let parent = objects.get_dict_mut(parentref);
            parent.insert("First", first);
            parent.insert("Last", last);
            if !parent_is_root {
                parent.insert("Count", -(count as i64));
            }
        }
    }

    /// Port of `Links.process_toc_item` (links.py lines 132-144). The
    /// Python `_('Unknown')` gettext call is ported as the literal
    /// string `"Unknown"` - no i18n system is in scope for this port.
    fn process_toc_item(
        &self,
        toc: &TocItem,
        parentref: &Reference,
        objects: &mut IndirectObjects,
    ) -> Option<Reference> {
        let path = toc.abspath.as_ref()?;
        let frag = toc.fragment.clone();
        let path = abspath_normcase(path);
        let a = self.anchors.get(&path)?;
        let dest = a.get(&frag).or_else(|| a.get(&None))?;
        let mut item = Dictionary::new();
        item.insert("Parent", *parentref);
        item.insert("Dest", dest.0.clone());
        item.insert(
            "Title",
            Utf16String::new(toc.text.clone().unwrap_or_else(|| "Unknown".to_string())),
        );
        Some(objects.add_dict(item))
    }
}

/// A minimal stand-in for calibre's TOC tree node type (the Python
/// original's `toc` items are `TOC` objects that are simultaneously
/// list-of-children and per-node metadata; `len(child) > 0` in
/// `process_children` becomes `!child.children.is_empty()` here).
#[derive(Debug, Clone, Default)]
pub struct TocItem {
    pub abspath: Option<PathBuf>,
    pub fragment: Option<String>,
    pub text: Option<String>,
    pub children: Vec<TocItem>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::render::common::PdfObj;

    fn simple_pageref(n: i64) -> Option<Reference> {
        if (0..3).contains(&n) {
            Some(Reference::new((n + 1) as u32))
        } else {
            None
        }
    }

    #[test]
    fn destination_finds_exact_page() {
        let mut pos = Pos {
            top: 100.0,
            column: 0,
            left: 5.0,
        };
        let dest = Destination::new(1, &mut pos, simple_pageref).unwrap();
        assert_eq!(dest.0 .0[0], PdfObj::Reference(Reference::new(2)));
        assert_eq!(dest.0 .0[2], PdfObj::Real(5.0));
        assert_eq!(dest.0 .0[3], PdfObj::Real(100.0));
        assert_eq!(dest.0 .0[4], PdfObj::Null);
    }

    #[test]
    fn destination_falls_back_to_earlier_page_and_zeroes_pos() {
        let mut pos = Pos {
            top: 100.0,
            column: 0,
            left: 5.0,
        };
        // page 5 doesn't exist, should fall back down to page 2 (index 2 valid)
        let dest = Destination::new(5, &mut pos, simple_pageref).unwrap();
        assert_eq!(dest.0 .0[0], PdfObj::Reference(Reference::new(3)));
        assert_eq!(pos.left, 0.0);
        assert_eq!(pos.top, 0.0);
    }

    #[test]
    fn destination_errors_when_nothing_found() {
        let mut pos = Pos {
            top: 0.0,
            column: 0,
            left: 0.0,
        };
        let res = Destination::new(0, &mut pos, |_| None);
        assert!(res.is_err());
    }

    #[test]
    fn abspath_normcase_collapses_dotdot() {
        let p = abspath_normcase(Path::new("/a/b/../c"));
        assert_eq!(p, PathBuf::from("/a/c"));
    }

    #[test]
    fn links_add_and_add_links_creates_annotation() {
        let mut objects = IndirectObjects::new();
        let page1 = objects.add_dict(Dictionary::new());
        let get_pageref = |n: i64| if n == 0 { Some(page1) } else { None };

        let mut links = Links::new(false, (600.0, 800.0));
        let anchors = indexmap::IndexMap::new();
        let link_specs = vec![LinkSpec {
            href: "https://example.com/".to_string(),
            page: 0,
            rect: vec![0.0, 0.0, 10.0, 10.0],
        }];
        links
            .add(
                Path::new("/docs/a.html"),
                0,
                &link_specs,
                &anchors,
                get_pageref,
                |_| {},
            )
            .unwrap();
        links.add_links(&mut objects, |_| {});

        let page_dict = objects.get_dict(&page1);
        assert!(page_dict.contains_key("Annots"));
    }

    #[test]
    fn links_add_outline_builds_sibling_chain() {
        let mut objects = IndirectObjects::new();
        let catalog_ref = objects.add_dict(Dictionary::new());
        let page1 = objects.add_dict(Dictionary::new());
        let get_pageref = |n: i64| if n == 0 { Some(page1) } else { None };

        let mut links = Links::new(false, (600.0, 800.0));
        let anchors = indexmap::IndexMap::new();
        links
            .add(
                Path::new("/docs/a.html"),
                0,
                &[],
                &anchors,
                get_pageref,
                |_| {},
            )
            .unwrap();

        let toc = vec![
            TocItem {
                abspath: Some(PathBuf::from("/docs/a.html")),
                fragment: None,
                text: Some("Chapter 1".to_string()),
                children: vec![],
            },
            TocItem {
                abspath: Some(PathBuf::from("/docs/a.html")),
                fragment: None,
                text: Some("Chapter 2".to_string()),
                children: vec![],
            },
        ];
        links.add_outline(&toc, &mut objects, &catalog_ref);

        let catalog = objects.get_dict(&catalog_ref);
        let outlines_ref = match catalog.get("Outlines") {
            Some(PdfObj::Reference(r)) => *r,
            _ => panic!("expected Outlines reference"),
        };
        let outlines = objects.get_dict(&outlines_ref);
        let first = match outlines.get("First") {
            Some(PdfObj::Reference(r)) => *r,
            _ => panic!("expected First reference"),
        };
        let first_item = objects.get_dict(&first);
        assert!(first_item.contains_key("Next"));
    }
}
