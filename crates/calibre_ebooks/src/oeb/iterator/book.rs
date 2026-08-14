//! Converting any supported ebook file into an exploded, paginated,
//! indexed book suitable for a viewer.
//!
//! Port of `old_src/src/calibre/ebooks/oeb/iterator/book.py`'s
//! `EbookIterator` (plus its module-level `extract_book`/`write_oebbook`
//! helpers).
//!
//! # Architecture differences from Python
//!
//! - **No on-disk-OPF round-trip.** Python's `extract_book` runs in a
//!   forked worker process (`run_extract_book` -> `fork_job`), so the
//!   only thing that can cross the process boundary is a path; the
//!   result gets written to disk and `EbookIterator.__enter__`
//!   re-parses it as `OPF(self.pathtoopf, ...)`. This crate has no
//!   multiprocess-worker architecture (the `rar`/`worker.py` port,
//!   issue #31, already established running such logic in-process
//!   instead), so there's no process boundary to cross: [`open`] keeps
//!   the [`OEBBook`] the input plugin built and reads
//!   manifest/spine/guide/metadata/toc directly off it. The OEB
//!   directory is still written to disk via [`OEBWriter`] (matching
//!   `write_oebbook`'s contract, including "the returned opf path"),
//!   because [`crate::oeb::iterator::spine::SpineItem`] needs real files
//!   to read from -- calibre's viewer displays HTML files, not an
//!   in-memory tree.
//! - **`Plumber`'s input-plugin dispatch, not duplicated.** Extraction
//!   goes through [`crate::conversion::plumber::convert_to_oebbook`],
//!   the function [`crate::conversion::plumber::Plumber::run`] itself
//!   was refactored to call, rather than re-implementing the
//!   format-dispatch table here.
//! - **`__enter__`/`__exit__` collapse into `open`/`Drop`.** Rust has no
//!   context-manager protocol; [`EbookIterator::open`] does the work of
//!   both `__init__` and `__enter__` in one call, and cleanup
//!   (`__exit__`'s `remove_dir(self._tdir)`) happens automatically when
//!   the returned `EbookIterator` (and the [`tempfile::TempDir`] it
//!   owns) is dropped.
//! - **A plain [`tempfile::TempDir`], not Python's temp-dir hierarchy.**
//!   Python distinguishes `PersistentTemporaryDirectory` (survives past
//!   the current statement, cleaned up explicitly) from
//!   `tdir_in_cache`/`use_tdir_in_cache` (a cache-directory-backed
//!   variant). This port always uses an RAII-cleaned `TempDir`; the
//!   `use_tdir_in_cache` flag on Python's constructor has no equivalent
//!   here and isn't threaded through
//!   ([`EbookIteratorOptions`] doesn't need it, since there's only one
//!   temp-dir strategy).
//!
//! # Known scope gaps
//!
//! - `processed`/`only_input_plugin`/`view_kepub` are accepted by
//!   Python's `extract_book`/`Plumber` but this crate's `Plumber` has no
//!   `opts` mechanism to carry them yet (`Plumber::run` takes no
//!   options at all). They're kept on [`EbookIteratorOptions`] for
//!   call-site fidelity but are currently inert; wiring them through
//!   `Plumber` is out of this issue's scope (it isn't part of
//!   `book.py`/`bookmarks.py`/`spine.py`).
//! - `path_to_html_toc`/`opf_cover` are heuristic re-derivations against
//!   `OEBBook.guide`/`.manifest` rather than a port of `OPF.cover`/
//!   `OPF.path_to_html_toc` (which read a different data model --
//!   `calibre.ebooks.metadata.opf2.OPF`, not `oeb.base.OEBBook`/`TOC` --
//!   see the module docs on `spine.rs` for the same distinction as it
//!   affects `create_indexing_data`). They cover the common cases (a
//!   `<guide>` reference of the right type, or a manifest item whose
//!   href suggests it's the HTML TOC) but are not guaranteed to match
//!   Python's exact NCX-aware priority order.
//! - Path joining (spine item paths, TOC-entry-to-spine-file resolution
//!   in `verify_links`) is a plain [`Path::join`], not Python's
//!   `os.path.abspath(os.path.join(...))`. `..`-containing hrefs (rare
//!   in practice -- OEB hrefs are conventionally relative paths within a
//!   single flat extraction root) would not resolve identically. Full
//!   lexical normalization was judged not worth the added complexity for
//!   this issue; if it becomes a real problem, add a `lexically_normalize`
//!   helper and apply it at both call sites.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use tempfile::TempDir;

use calibre_utils::config::DynamicConfig;

use crate::conversion::plumber::convert_to_oebbook;
use crate::html_entities::decode_entities;
use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::mobi::mobi6::MobiReader;
use crate::oeb::book::OEBBook;
use crate::oeb::iterator::bookmarks::{Bookmark, BookmarksMixin};
use crate::oeb::iterator::spine::{create_indexing_data, SpineItem};
use crate::oeb::spine::SpineItem as OpfSpineEntry;
use crate::oeb::toc::TOCNode;
use crate::oeb::writer::OEBWriter;

/// `CoverManager.SVG_TEMPLATE` from
/// `old_src/src/calibre/ebooks/oeb/transforms/cover.py`, with the
/// `__ar__`/`__viewbox__`/`__width__`/`__height__` placeholders
/// pre-substituted the way `book.py`'s module-level `TITLEPAGE` constant
/// does (`preserveAspectRatio="none"`, `viewBox="0 0 600 800"`,
/// `width="600"`, `height="800"`). `CoverManager` itself -- the class
/// this one template constant is borrowed from -- is a separate, larger
/// module (`docs/modules_to_port.md`'s `transforms/cover.py`, currently
/// unported) and out of scope for this issue; only the constant is
/// needed here, to build the synthetic cover title-page calibre inserts
/// as spine item zero for formats whose cover isn't already a spine
/// page.
const TITLEPAGE: &str = concat!(
    "<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"en\">\n",
    "    <head>\n",
    "        <meta http-equiv=\"Content-Type\" content=\"text/html; charset=UTF-8\" />\n",
    "        <meta name=\"calibre:cover\" content=\"true\" />\n",
    "        <title>Cover</title>\n",
    "        <style type=\"text/css\" title=\"override_css\">\n",
    "            @page {padding: 0pt; margin:0pt}\n",
    "            body { text-align: center; padding:0pt; margin: 0pt; }\n",
    "        </style>\n",
    "    </head>\n",
    "    <body>\n",
    "        <div>\n",
    "            <svg version=\"1.1\" xmlns=\"http://www.w3.org/2000/svg\"\n",
    "                xmlns:xlink=\"http://www.w3.org/1999/xlink\"\n",
    "                width=\"100%\" height=\"100%\" viewBox=\"0 0 600 800\"\n",
    "                preserveAspectRatio=\"none\">\n",
    "                <image width=\"600\" height=\"800\" xlink:href=\"%%COVER_HREF%%\"/>\n",
    "            </svg>\n",
    "        </div>\n",
    "    </body>\n",
    "</html>\n",
);

const CHARACTERS_PER_PAGE: f64 = 1000.0;

/// `prepare_string_for_xml(raw, attribute=True)` in
/// `old_src/src/calibre/__init__.py`: decode any existing entities,
/// then re-escape `&`, `<`, `>`, `"`, `'` for safe use inside a
/// double-quoted XML attribute value.
fn prepare_string_for_xml_attr(raw: &str) -> String {
    let raw = decode_entities(raw);
    let raw = raw
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    raw.replace('"', "&quot;").replace('\'', "&apos;")
}

/// `EbookIterator.__enter__`'s keyword arguments, plus
/// `__init__`'s `copy_bookmarks_to_file`. See the module doc for why
/// `use_tdir_in_cache` has no field here.
#[derive(Debug, Clone, Copy)]
pub struct EbookIteratorOptions {
    /// Currently inert -- see the module doc's "known scope gaps".
    pub processed: bool,
    /// Currently inert -- see the module doc's "known scope gaps".
    pub only_input_plugin: bool,
    /// Currently inert -- see the module doc's "known scope gaps".
    pub view_kepub: bool,
    pub run_char_count: bool,
    pub read_anchor_map: bool,
    pub read_links: bool,
    pub copy_bookmarks_to_file: bool,
}

impl Default for EbookIteratorOptions {
    fn default() -> Self {
        EbookIteratorOptions {
            processed: false,
            only_input_plugin: false,
            view_kepub: false,
            run_char_count: true,
            read_anchor_map: true,
            read_links: true,
            copy_bookmarks_to_file: true,
        }
    }
}

/// An ebook exploded into a directory, with a paginated,
/// anchor/link-indexed spine and a loaded bookmark list. Port of
/// `EbookIterator`.
pub struct EbookIterator {
    pathtoebook: PathBuf,
    ebook_ext: String,
    config: DynamicConfig,
    copy_bookmarks_to_file: bool,
    bookmarks: Vec<Bookmark>,
    /// Kept alive only so its `Drop` impl removes the extraction
    /// directory when this `EbookIterator` is dropped -- the Rust
    /// equivalent of `EbookIterator.__exit__`'s `remove_dir(self._tdir)`.
    #[allow(dead_code)]
    tdir: TempDir,
    base: PathBuf,
    book_format: String,
    pathtoopf: PathBuf,
    oeb: OEBBook,
    language: Option<String>,
    spine: Vec<SpineItem>,
    toc: TOCNode,
    pages: Vec<i64>,
}

impl EbookIterator {
    /// Extract `pathtoebook`, build its paginated/indexed spine, and
    /// load any saved bookmarks. Port of `EbookIterator.__init__`
    /// followed immediately by `__enter__` (see the module doc for why
    /// those collapse into one call here).
    ///
    /// Bookmarks are stored under the `"iterator"`-named
    /// [`DynamicConfig`] store, matching Python's
    /// `DynamicConfig(name='iterator')`. Use
    /// [`EbookIterator::open_with_config_name`] to override that name.
    pub fn open<P: AsRef<Path>>(pathtoebook: P, opts: EbookIteratorOptions) -> Result<Self> {
        Self::open_with_config_name(pathtoebook, opts, "iterator")
    }

    /// Like [`EbookIterator::open`], but storing bookmarks under a
    /// `DynamicConfig` store named `config_name` instead of the default
    /// `"iterator"`. The Rust equivalent of Python's
    /// `DynamicConfig.decouple(prefix)`; mainly useful for tests, which
    /// would otherwise all contend for the one real, shared
    /// `iterator.json` config file.
    pub fn open_with_config_name<P: AsRef<Path>>(
        pathtoebook: P,
        opts: EbookIteratorOptions,
        config_name: &str,
    ) -> Result<Self> {
        let raw_path = pathtoebook.as_ref();
        // `os.path.abspath(pathtoebook.strip())`: canonicalize when
        // possible, but don't hard-fail just because the path doesn't
        // exist yet -- extraction below will fail with a clearer error
        // in that case.
        let pathtoebook = fs::canonicalize(raw_path).unwrap_or_else(|_| raw_path.to_path_buf());
        let ebook_ext = compute_ebook_ext(&pathtoebook);
        let config = DynamicConfig::new(config_name);

        let tdir =
            tempfile::tempdir().context("Failed to create temporary extraction directory")?;
        let base = tdir.path().to_path_buf();

        let mut oeb = convert_to_oebbook(&pathtoebook, &base)
            .with_context(|| format!("Failed to extract {}", pathtoebook.display()))?;
        OEBWriter::new()
            .write_book(&mut oeb, &base)
            .context("Failed to write exploded OEB directory")?;
        let pathtoopf = base.join("content.opf");

        let input_fmt = pathtoebook
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        let book_format = compute_book_format(&pathtoebook);
        let from_epub = book_format == "EPUB";

        let language = oeb
            .metadata
            .get("language")
            .first()
            .map(|i| i.value.to_lowercase());

        let mut spine: Vec<SpineItem> = Vec::new();
        if input_fmt == "htmlz" {
            let p = base.join("index.html");
            if let Some(p_str) = p.to_str() {
                let item = SpineItem::new(
                    p_str,
                    Some("text/html".to_string()),
                    opts.read_anchor_map,
                    opts.run_char_count,
                    false,
                    opts.read_links,
                )
                .with_context(|| format!("Failed to read htmlz spine item {}", p.display()))?;
                spine.push(item);
            }
        } else {
            let mut ordered: Vec<&OpfSpineEntry> =
                oeb.spine.items.iter().filter(|i| i.linear).collect();
            ordered.extend(oeb.spine.items.iter().filter(|i| !i.linear));
            let is_comic = matches!(input_fmt.as_str(), "cbc" | "cbz" | "cbr" | "cb7");

            for entry in ordered {
                let manifest_item = oeb.manifest.get_by_id(&entry.idref);
                let href = match manifest_item {
                    Some(m) => m.href.clone(),
                    None => {
                        eprintln!("Warning: missing spine item: {:?}", entry.idref);
                        continue;
                    }
                };
                let spath = base.join(&href);
                let spath_str = match spath.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                let mt = manifest_item
                    .map(|m| m.media_type.clone())
                    .filter(|s| !s.is_empty())
                    .or_else(|| mime_guess::from_path(&spath).first().map(|g| g.to_string()));

                match SpineItem::new(
                    spath_str,
                    mt,
                    opts.read_anchor_map,
                    opts.run_char_count,
                    from_epub,
                    opts.read_links,
                ) {
                    Ok(mut item) => {
                        if is_comic {
                            item.is_single_page = Some(true);
                        }
                        spine.push(item);
                    }
                    Err(e) => eprintln!("Warning: missing spine item: {spath_str:?}: {e}"),
                }
            }
        }

        if let Some(cover_path) = opf_cover(&oeb, &base) {
            if matches!(
                ebook_ext.as_str(),
                "lit" | "mobi" | "prc" | "opf" | "fb2" | "azw" | "azw3" | "docx" | "htmlz"
            ) {
                let cfile = base.join("calibre_iterator_cover.html");
                let rcpath = pathdiff::diff_paths(&cover_path, &base).unwrap_or(cover_path);
                let rcpath_str = rcpath.to_string_lossy().replace('\\', "/");
                let chtml =
                    TITLEPAGE.replace("%%COVER_HREF%%", &prepare_string_for_xml_attr(&rcpath_str));
                fs::write(&cfile, chtml.as_bytes())
                    .context("Failed to write synthetic cover page")?;
                if let Some(p) = cfile.to_str() {
                    match SpineItem::new(
                        p,
                        Some("application/xhtml+xml".to_string()),
                        opts.read_anchor_map,
                        opts.run_char_count,
                        false,
                        opts.read_links,
                    ) {
                        Ok(item) => spine.insert(0, item),
                        Err(e) => {
                            eprintln!("Warning: failed to add synthetic cover page: {e}")
                        }
                    }
                }
            }
        }

        if let Some(toc_path) = path_to_html_toc(&oeb, &base) {
            if !spine.iter().any(|s| s.path == toc_path) {
                if let Some(p) = toc_path.to_str() {
                    match SpineItem::new(
                        p,
                        None,
                        opts.read_anchor_map,
                        opts.run_char_count,
                        false,
                        opts.read_links,
                    ) {
                        Ok(item) => spine.push(item),
                        Err(e) => eprintln!("Warning: failed to add html toc to spine: {e}"),
                    }
                }
            }
        }

        let pages: Vec<i64> = spine
            .iter()
            .map(|s| (s.character_count as f64 / CHARACTERS_PER_PAGE).ceil() as i64)
            .collect();
        for (item, &p) in spine.iter_mut().zip(pages.iter()) {
            item.pages = p;
        }
        let mut start = 1i64;
        for item in spine.iter_mut() {
            item.start_page = start;
            start += item.pages;
            item.max_page = item.start_page + item.pages - 1;
        }

        let toc = oeb.toc.root.clone();
        if opts.read_anchor_map {
            create_indexing_data(&mut spine, &toc, &base);
        }

        verify_links(&mut spine);

        let mut it = EbookIterator {
            pathtoebook,
            ebook_ext,
            config,
            copy_bookmarks_to_file: opts.copy_bookmarks_to_file,
            bookmarks: Vec::new(),
            tdir,
            base,
            book_format,
            pathtoopf,
            oeb,
            language,
            spine,
            toc,
            pages,
        };
        it.read_bookmarks()?;
        Ok(it)
    }

    pub fn pathtoebook(&self) -> &Path {
        &self.pathtoebook
    }
    pub fn ebook_ext(&self) -> &str {
        &self.ebook_ext
    }
    pub fn base(&self) -> &Path {
        &self.base
    }
    pub fn book_format(&self) -> &str {
        &self.book_format
    }
    pub fn pathtoopf(&self) -> &Path {
        &self.pathtoopf
    }
    pub fn oeb(&self) -> &OEBBook {
        &self.oeb
    }
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
    pub fn spine(&self) -> &[SpineItem] {
        &self.spine
    }
    pub fn toc(&self) -> &TOCNode {
        &self.toc
    }
    pub fn pages(&self) -> &[i64] {
        &self.pages
    }

    /// Search the spine for `text` (case-insensitive), scanning forward
    /// from `index` (or backward, if `backwards`), and return the index
    /// of the first spine item whose body text contains it. Port of
    /// `EbookIterator.search`, using [`crate::mobi::dom`] (the HTML5
    /// parser+arena from issue #33) in place of Python's
    /// `calibre.ebooks.oeb.polish.parsing.parse` (a separate, unported
    /// lxml-based parser).
    pub fn search(&self, text: &str, index: usize, backwards: bool) -> Result<Option<usize>> {
        let mut order: Vec<usize> = (0..self.spine.len()).collect();
        if backwards {
            order.reverse();
        }
        let q = text.to_lowercase();
        for i in order {
            let in_range = if backwards { i < index } else { i > index };
            if !in_range {
                continue;
            }
            let item = &self.spine[i];
            let raw = fs::read(&item.path)
                .with_context(|| format!("Failed to read {}", item.path.display()))?;
            let enc = encoding_rs::Encoding::for_label(item.encoding.as_bytes())
                .unwrap_or(encoding_rs::UTF_8);
            let (decoded, _, _) = enc.decode(&raw);
            let dom = Dom::parse(&decoded);
            let mut fragments = String::new();
            for body in dom.find_all_tag_global("body") {
                collect_search_text(&dom, body, &mut fragments);
            }
            if fragments.to_lowercase().contains(&q) {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }
}

impl BookmarksMixin for EbookIterator {
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
        self.copy_bookmarks_to_file
    }
    fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }
    fn bookmarks_mut(&mut self) -> &mut Vec<Bookmark> {
        &mut self.bookmarks
    }
}

/// Tags whose own text is excluded from search (but whose *tail* text --
/// i.e. the text of their next sibling -- still counts), matching the
/// `{'script', 'style', 'del'}` skip-set in `EbookIterator.search`.
const SEARCH_SKIP_TAGS: [&str; 3] = ["script", "style", "del"];

/// Depth-first collect of the lowercasable text content under `id`,
/// skipping the subtrees of [`SEARCH_SKIP_TAGS`] elements. Since this
/// crate's [`Dom`] represents text nodes as ordinary children
/// interleaved with element children (rather than lxml's
/// `.text`/`.tail`-on-element model), a plain "walk children in order,
/// recurse into non-skipped elements" naturally reproduces Python's
/// `serialize` helper: a skipped element's *tail* text is just the next
/// sibling in the same child list, visited on the next loop iteration
/// regardless of whether the previous sibling was recursed into.
fn collect_search_text(dom: &Dom, id: NodeId, out: &mut String) {
    for child in dom.children(id) {
        match &dom.node(child).kind {
            NodeKind::Text(t) => out.push_str(t),
            NodeKind::Element(tag) => {
                if !SEARCH_SKIP_TAGS.contains(&tag.as_str()) {
                    collect_search_text(dom, child, out);
                }
            }
            NodeKind::Comment(_) | NodeKind::Document => {}
        }
    }
}

/// `os.path.splitext(pathtoebook)[1].replace('.', '').lower()` then
/// `re.sub(r'(x{0,1})htm(l{0,1})', 'html', ext)` then
/// `.replace('original_', '')`, from `EbookIterator.__init__`.
fn compute_ebook_ext(pathtoebook: &Path) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    // Port of Python's `r'(x{0,1})htm(l{0,1})'`: an optional leading
    // `x`, literal `htm`, then an optional trailing `l` -- matches
    // `htm`, `html`, `xhtm`, `xhtml`.
    let re = RE.get_or_init(|| Regex::new(r"x?htm[l]?").expect("valid regex"));
    let ext = pathtoebook
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let ext = re.replace_all(&ext, "html").into_owned();
    ext.replace("original_", "")
}

/// `book_format = os.path.splitext(pathtoebook)[1][1:].upper()`,
/// overridden to `"KF8"`/`"KF8:joint"` when the file carries a KF8
/// payload. From `extract_book` in `book.py`.
///
/// Python gets `is_kf8`/`mobi_is_joint` for free off the already-run
/// input plugin instance (`plumber.input_plugin.is_kf8`); this crate's
/// `MOBIInput::convert` doesn't expose that flag (it's a local variable
/// inside the function), so this does a second, header-only parse of
/// the file purely for the label -- cheap relative to the extraction
/// that already happened, and best-effort (a parse failure here just
/// falls back to the plain extension, it doesn't fail the whole open).
fn compute_book_format(pathtoebook: &Path) -> String {
    let ext = pathtoebook
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_uppercase();
    if matches!(ext.as_str(), "MOBI" | "AZW" | "AZW3" | "PRC") {
        if let Ok(raw) = fs::read(pathtoebook) {
            if let Ok(reader) = MobiReader::new(&raw) {
                if let Some(kf8_type) = &reader.kf8_type {
                    let suffix = if kf8_type == "joint" { ":joint" } else { "" };
                    return format!("KF8{suffix}");
                }
            }
        }
    }
    ext
}

/// `OPF.cover`: the guide reference of type `cover` (or one of the two
/// MS-cover-image aliases), resolved to an absolute path. See the
/// module doc for why this reads `OEBBook.guide` directly instead of
/// re-parsing an on-disk OPF.
fn opf_cover(book: &OEBBook, base_dir: &Path) -> Option<PathBuf> {
    for t in [
        "cover",
        "other.ms-coverimage-standard",
        "other.ms-coverimage",
    ] {
        if let Some(r) = book.guide.get(t) {
            return Some(base_dir.join(&r.href));
        }
    }
    None
}

/// A best-effort re-derivation of `OPF.path_to_html_toc`: prefer a
/// `<guide>` reference of type `toc` that isn't itself an NCX file,
/// else fall back to a manifest item whose href suggests it's an HTML
/// TOC page. See the module doc's "known scope gaps" for how this
/// differs from Python's NCX-aware version.
fn path_to_html_toc(book: &OEBBook, base_dir: &Path) -> Option<PathBuf> {
    if let Some(r) = book.guide.get("toc") {
        let p = base_dir.join(&r.href);
        let is_ncx = p
            .extension()
            .map(|e| e.eq_ignore_ascii_case("ncx"))
            .unwrap_or(false);
        if !is_ncx && p.is_file() {
            return Some(p);
        }
    }
    for item in book.manifest.iter() {
        if item.href.to_lowercase().contains("toc")
            && (item.media_type.contains("html") || item.media_type.contains("xhtml"))
        {
            let p = base_dir.join(&item.href);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// The pieces of a URL `verify_links` needs: whether it's external
/// (non-empty `scheme`/`netloc`), its `path`, and its `fragment`.
/// Stands in for Python's `urlparse`, scoped to just what
/// `EbookIterator.verify_links` reads off the result.
#[derive(Debug, Default)]
struct ParsedLink {
    scheme: String,
    netloc: String,
    path: String,
    fragment: String,
}

fn parse_link(url: &str) -> ParsedLink {
    let (rest, fragment) = match url.split_once('#') {
        Some((r, f)) => (r, f.to_string()),
        None => (url, String::new()),
    };
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
    let (netloc, path) = match rest.strip_prefix("//") {
        Some(stripped) => match stripped.find('/') {
            Some(i) => (stripped[..i].to_string(), stripped[i..].to_string()),
            None => (stripped.to_string(), String::new()),
        },
        None => (String::new(), rest.to_string()),
    };
    ParsedLink {
        scheme,
        netloc,
        path,
        fragment,
    }
}

/// Populate `item.verified_links` for every link that resolves to a
/// real spine file (and, if it carries a fragment, a real anchor in
/// that file). Port of `EbookIterator.verify_links`.
fn verify_links(spine: &mut [SpineItem]) {
    let path_index: std::collections::HashMap<PathBuf, usize> = spine
        .iter()
        .enumerate()
        .map(|(i, s)| (s.path.clone(), i))
        .collect();

    let mut additions: Vec<(usize, PathBuf, Option<String>)> = Vec::new();
    for (idx, item) in spine.iter().enumerate() {
        let base = item.path.parent().unwrap_or_else(|| Path::new(""));
        for link in &item.all_links {
            let decoded = crate::lit::urlunquote(link);
            let parsed = parse_link(&decoded);
            if !parsed.scheme.is_empty() || !parsed.netloc.is_empty() {
                continue;
            }
            let target_path = if parsed.path.is_empty() {
                item.path.clone()
            } else {
                base.join(&parsed.path)
            };
            if let Some(&target_idx) = path_index.get(&target_path) {
                let frag = if parsed.fragment.is_empty() {
                    None
                } else {
                    Some(parsed.fragment.clone())
                };
                let ok = match &frag {
                    None => true,
                    Some(f) => spine[target_idx].anchor_map.contains_key(f),
                };
                if ok {
                    additions.push((idx, spine[target_idx].path.clone(), frag));
                }
            }
        }
    }
    for (idx, path, frag) in additions {
        spine[idx].verified_links.insert((path, frag));
    }
}
