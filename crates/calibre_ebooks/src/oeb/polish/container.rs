//! Port of `old_src/src/calibre/ebooks/oeb/polish/container.py`.
//!
//! This is the core of the "Polish Book" foundation: an editable,
//! in-memory model of an e-book (folder-of-files + an OPF) with
//! dirty-tracking and commit-to-disk semantics. See
//! `crates/calibre_ebooks/src/oeb/polish/mod.rs` for the overall scope
//! of this port (issue #39, foundation slice).
//!
//! # Design decisions
//!
//! **`ContainerBase` -> composition, not a separate base type.** Python's
//! `ContainerBase.guess_type` reads `self.opf_version_parsed`, a
//! property that only exists on the `Container` subclass -- i.e. even in
//! Python, `ContainerBase` is not really usable standalone for anything
//! that touches an OPF (its own docstring calls it a base "useful to
//! create virtual containers for testing", not something instantiated by
//! this module). Per `docs/AGENT_PORTING_GUIDE.md` ("prefer composition
//! ... over inheritance"), this port keeps [`ContainerBase`] as a small
//! struct holding just the parse-cache/mime-map state and the
//! encoding-agnostic `decode`/`parse_xml`/`parse_xhtml` helpers, and
//! [`Container`] embeds it as a field (`base`) rather than simulating
//! inheritance.
//!
//! **`Container` -> `EpubContainer` -> `KepubContainer`, and
//! `Container` -> `Azw3Container` -> composition + `Deref`.** Rust has
//! no struct inheritance. Each format-specific type *has-a* the type it
//! specializes (`EpubContainer` has a `Container`, `KepubContainer` has
//! an `EpubContainer`) and implements `Deref`/`DerefMut` to it, so the
//! ~90% of methods that aren't overridden (manifest/spine/guide
//! manipulation, `add_file`, `dirty`, ...) are simply inherited through
//! auto-deref, while overridden methods (`rename`, `commit`,
//! `remove_item`, the `names_that_*` sets, `book_type_for_display`) are
//! shadowed by an inherent method of the same name on the outer type.
//! This is the standard Rust idiom for "mostly reuse a type, override a
//! few methods" and keeps each format's extra state (`path_to_epub`,
//! `obfuscated_fonts`, ...) in a real, distinctly-named struct rather
//! than collapsing everything into one enum (future `polish` files will
//! want to name `EpubContainer`/`AZW3Container` directly, matching
//! Python).
//!
//! **No `href_to_name` memoization cache.** Python memoizes
//! `href_to_name` lookups in `self.href_to_name_cache`, purely as a
//! performance optimization (it doesn't change behavior). Dropping it
//! lets [`Container::href_to_name`]/[`Container::name_to_href`] be pure,
//! non-mutating (`&self`) functions, which removes a large amount of
//! borrow-checker friction throughout this file (many methods that
//! would otherwise need `&mut self` purely to refresh that cache can
//! stay `&self`, and can be called while another part of `self` is
//! already borrowed). If profiling ever shows this matters, the cache
//! can be reintroduced behind a `RefCell`.
//!
//! **No `log`/multiprocess-pickling (`clone_data`/`__getstate__`)
//! surface.** Python threads a `log` object through every constructor
//! purely for `.exception()`/`.debug()` calls on already-handled error
//! paths; this port routes every error through `Result` instead (per
//! `docs/AGENT_PORTING_GUIDE.md` #1), so there's no separate log sink to
//! model. Python's `clone_data`/`data_for_clone`/`__getstate__`/
//! `__setstate__` protocol exists so a `Container` can be pickled across
//! a multiprocessing checkpoint boundary in calibre's editor; this port
//! instead exposes [`Container::clone_to`], which delivers the same
//! real capability (an efficient hardlink-based on-disk clone, reopened
//! as a fresh `Container`) without modeling Python's pickle protocol,
//! which nothing in this crate needs.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use unicode_normalization::UnicodeNormalization;

use crate::mobi::dom::Dom;
use crate::oeb::constants::{DC11_NS, NCX_MIME, OEB_DOCS, OEB_STYLES, OPF2_NS};

use super::errors::PolishError;
use super::parsing;
use super::utils::{adjust_mime_for_epub, guess_type, NameLookup};
use super::xmltree::{Xml, XmlNodeId};

/// `OPF_NAMESPACES` -- the `opf:`/`dc:` prefixes `opf_xpath` resolves
/// against, matching every real call site in `container.py`/`opf.py`.
pub fn opf_namespaces() -> HashMap<&'static str, &'static str> {
    let mut m = HashMap::new();
    m.insert("opf", OPF2_NS);
    m.insert("dc", DC11_NS);
    m
}

/// A file's cached parsed representation. Port of what `Container.parse`
/// returns for each of Python's three branches (XHTML/HTML via `lxml`,
/// generic `+xml`/`/xml` via `lxml`, CSS via `css_parser`), plus a
/// fallback for anything else (Python implicitly leaves `data` as raw
/// bytes when no branch matches; nothing in the ported files' scope
/// exercises `parsed()` on a binary file, but the fallback keeps the
/// cache total).
pub enum ParsedItem {
    /// XHTML/HTML content documents, via [`crate::mobi::dom::Dom`].
    Xhtml(Dom),
    /// OPF/NCX/OCF-family strict XML, via [`Xml`].
    Xml(Xml),
    /// CSS text. **Not** a structured stylesheet object, even though
    /// [`crate::css::Stylesheet`] exists (see its module docs for issue
    /// #164, which added it): callers that need structure call
    /// [`Container::parsed_stylesheet`] to parse this text on demand and
    /// [`Container::set_css_text`] to write mutations back, matching how
    /// Python's `css_parser` objects are used -- parse, mutate,
    /// re-serialize -- without this crate's file cache having to hold a
    /// live, in-place-mutable object graph the way `get_xml`/`get_xhtml`
    /// do for OPF/NCX/XHTML. Storing the decoded raw text keeps generic
    /// file management -- read/write/rename/commit -- working uniformly
    /// for every file kind.
    Css(String),
    /// Anything else (images, fonts, ...): raw bytes, unparsed.
    Raw(Vec<u8>),
}

/// Port of `ContainerBase`. See the module docs for why this is a
/// plain field of [`Container`] rather than a simulated base class.
#[derive(Default)]
pub struct ContainerBase {
    pub tweak_mode: bool,
    pub parsed_cache: HashMap<String, ParsedItem>,
    pub mime_map: HashMap<String, String>,
    pub encoding_map: HashMap<String, String>,
    pub used_encoding: Option<String>,
}

impl ContainerBase {
    /// Port of `ContainerBase.decode`.
    pub fn decode(&mut self, data: &[u8], normalize_to_nfc: bool) -> String {
        let (text, used_encoding) = parsing::decode_xml(data, normalize_to_nfc);
        if !used_encoding.is_empty() {
            self.used_encoding = Some(used_encoding);
        }
        text
    }

    /// Port of `ContainerBase.parse_xml`.
    pub fn parse_xml(&mut self, data: &[u8]) -> Result<Xml> {
        let (text, encoding) = crate::chardet::xml_to_unicode(data, true, true);
        self.used_encoding = encoding;
        let text: String = text.nfc().collect();
        Xml::parse(&text)
    }

    /// Port of `ContainerBase.parse_xhtml`. Always takes the tag-soup
    /// path -- see `oeb::polish::parsing`'s module docs.
    pub fn parse_xhtml(&mut self, data: &[u8]) -> Dom {
        let (text, encoding) = crate::chardet::xml_to_unicode(data, false, true);
        self.used_encoding = encoding;
        parsing::parse(&text, true, false)
    }
}

/// Port of `Container` (the generic, folder-backed editable OEB model).
/// See the module docs for the `has-a` + `Deref` design used by
/// [`EpubContainer`]/[`Azw3Container`] to specialize this.
pub struct Container {
    pub base: ContainerBase,
    /// Absolute path to the root folder this container's names are
    /// relative to.
    pub root: PathBuf,
    pub name_path_map: HashMap<String, PathBuf>,
    pub opf_name: String,
    pub opf_dir: PathBuf,
    pub dirtied: HashSet<String>,
    pub pretty_print: HashSet<String>,
    pub cloned: bool,
}

/// Distinguishes which kind of document `replace_links`/`iterlinks` is
/// looking at -- passed to the replace closure instead of being an
/// attribute Python sets on the callable (`replace_func.file_type`),
/// which is a more natural shape in Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFileType {
    Opf,
    Text,
    Style,
    Ncx,
}

impl Container {
    /// Port of `Container.__init__`'s main (non-clone) path: walk
    /// `rootpath`, build `name_path_map`/`mime_map`, and locate the OPF
    /// file (either the one at `opfpath`, or -- matching Python's
    /// fallback -- the first `.opf` file found by walking the tree).
    pub fn open(rootpath: &Path, opfpath: &Path) -> Result<Container> {
        let root = rootpath
            .canonicalize()
            .with_context(|| format!("Failed to resolve root path {}", rootpath.display()))?;
        let opfpath = opfpath
            .canonicalize()
            .with_context(|| format!("Failed to resolve OPF path {}", opfpath.display()))?;

        let mut name_path_map = HashMap::new();
        let mut mime_map = HashMap::new();
        let mut opf_name: Option<String> = None;
        let mut opf_dir: Option<PathBuf> = None;
        let mut all_opf_files: Vec<(String, PathBuf)> = Vec::new();

        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path().to_path_buf();
            let name = abspath_to_name_at(&path, &root);
            mime_map.insert(name.clone(), guess_type(&name));
            if path == opfpath {
                opf_name = Some(name.clone());
                opf_dir = path.parent().map(|p| p.to_path_buf());
                mime_map.insert(name.clone(), guess_type("a.opf"));
            }
            if path
                .extension()
                .map(|e| e.eq_ignore_ascii_case("opf"))
                .unwrap_or(false)
            {
                all_opf_files.push((name.clone(), path.parent().unwrap_or(&root).to_path_buf()));
            }
            name_path_map.insert(name, path);
        }

        if opf_name.is_none() {
            if let Some((name, dir)) = all_opf_files.into_iter().next() {
                mime_map.insert(name.clone(), guess_type("a.opf"));
                opf_name = Some(name);
                opf_dir = Some(dir);
            }
        }

        let opf_name = opf_name.ok_or_else(|| {
            PolishError::InvalidBook(format!("Could not locate opf file: {:?}", opfpath))
        })?;
        let opf_dir = opf_dir.unwrap_or_else(|| root.clone());

        let mut container = Container {
            base: ContainerBase {
                mime_map,
                ..Default::default()
            },
            root,
            name_path_map,
            opf_name,
            opf_dir,
            dirtied: HashSet::new(),
            pretty_print: HashSet::new(),
            cloned: false,
        };
        container.refresh_mime_map()?;
        Ok(container)
    }

    /// Port of `Container.refresh_mime_map`.
    pub fn refresh_mime_map(&mut self) -> Result<()> {
        let items = self.opf_xpath("//opf:manifest/opf:item[@href and @media-type]")?;
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let xml = self.get_xml(&opf_name)?;
        let mut updates = Vec::new();
        for item in items {
            let href = xml.get_attr(item, "href").unwrap_or("").to_string();
            let mt = xml.get_attr(item, "media-type").unwrap_or("").to_string();
            updates.push((href, mt));
        }
        for (href, mt) in updates {
            if let Some(name) = self.href_to_name(&href, Some(&self.opf_name.clone())) {
                if name != self.opf_name && self.base.mime_map.contains_key(&name) && !mt.is_empty()
                {
                    self.base.mime_map.insert(name, mt);
                }
            }
        }
        Ok(())
    }

    /// Port of `clone_container`/`Container.clone_data`: commits
    /// pending changes, then hard-link-clones the whole tree into
    /// `dest_dir` and reopens it as an independent `Container`. See the
    /// module docs for why this replaces Python's pickle-oriented
    /// `clone_data`/`data_for_clone` protocol.
    pub fn clone_to(&mut self, dest_dir: &Path) -> Result<Container> {
        self.commit(false)?;
        clone_dir(&self.root, dest_dir)?;
        self.cloned = true;
        let opf_path = dest_dir.join(
            self.name_path_map[&self.opf_name]
                .strip_prefix(&self.root)
                .unwrap_or(Path::new(&self.opf_name)),
        );
        let mut cloned = Container::open(dest_dir, &opf_path)?;
        cloned.cloned = true;
        Ok(cloned)
    }

    // -- name/path/href conversions ----------------------------------

    pub fn abspath_to_name(&self, path: &Path) -> String {
        abspath_to_name_at(path, &self.root)
    }

    pub fn name_to_abspath(&self, name: &str) -> PathBuf {
        name_to_abspath_at(name, &self.root)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.name_to_abspath(name).exists()
    }

    /// Port of `Container.href_to_name`. See the module docs: no
    /// memoization, unlike Python.
    pub fn href_to_name(&self, href: &str, base: Option<&str>) -> Option<String> {
        href_to_name_at(href, &self.root, base)
    }

    /// Port of `Container.name_to_href`.
    pub fn name_to_href(&self, name: &str, base: Option<&str>) -> String {
        name_to_href_at(name, &self.root, base)
    }

    pub fn relpath(&self, path: &Path) -> PathBuf {
        pathdiff::diff_paths(path, &self.root).unwrap_or_else(|| path.to_path_buf())
    }

    pub fn has_name(&self, name: &str) -> bool {
        self.name_path_map.contains_key(name)
    }

    pub fn has_name_case_insensitive(&self, name: &str) -> bool {
        if name.is_empty() {
            return false;
        }
        let lower = name.to_lowercase();
        self.name_path_map.keys().any(|q| q.to_lowercase() == lower)
    }

    pub fn has_name_and_is_not_empty(&self, name: &str) -> bool {
        match self.name_path_map.get(name) {
            Some(path) => fs::metadata(path).map(|m| m.len() > 0).unwrap_or(false),
            None => false,
        }
    }

    // -- overridable format properties (shadowed by EpubContainer) ---

    pub fn book_type(&self) -> &'static str {
        "oeb"
    }

    pub fn book_type_for_display(&self) -> String {
        self.book_type().to_uppercase()
    }

    pub fn names_that_need_not_be_manifested(&self) -> HashSet<String> {
        [self.opf_name.clone()].into_iter().collect()
    }

    pub fn names_that_must_not_be_removed(&self) -> HashSet<String> {
        [self.opf_name.clone()].into_iter().collect()
    }

    pub fn names_that_must_not_be_changed(&self) -> HashSet<String> {
        HashSet::new()
    }

    pub fn ok_to_be_unmanifested(&self, name: &str) -> bool {
        self.names_that_need_not_be_manifested().contains(name)
    }

    // -- XML tree access -----------------------------------------------

    /// Ensures `name` is present in the parse cache, reading and
    /// parsing it from disk if not. Port of the lazy-population half of
    /// `Container.parsed`.
    pub fn ensure_parsed(&mut self, name: &str) -> Result<()> {
        if self.base.parsed_cache.contains_key(name) {
            return Ok(());
        }
        let path = self
            .name_path_map
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No such name in this container: {name}"))?;
        let data = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
        let mime = self
            .base
            .mime_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_type(name));
        let item = if OEB_DOCS.contains(&mime.as_str()) {
            ParsedItem::Xhtml(self.base.parse_xhtml(&data))
        } else if mime.ends_with("+xml") || mime.ends_with("/xml") {
            ParsedItem::Xml(self.base.parse_xml(&data)?)
        } else if OEB_STYLES.contains(&mime.as_str()) {
            let (text, encoding) = crate::chardet::xml_to_unicode(&data, false, true);
            self.base.used_encoding = encoding;
            ParsedItem::Css(text)
        } else {
            ParsedItem::Raw(data)
        };
        self.base.encoding_map.insert(
            name.to_string(),
            self.base.used_encoding.clone().unwrap_or_default(),
        );
        self.base.parsed_cache.insert(name.to_string(), item);
        Ok(())
    }

    fn ensure_opf_parsed(&mut self) -> Result<()> {
        let opf_name = self.opf_name.clone();
        self.ensure_parsed(&opf_name)
    }

    /// Returns the already-parsed XML tree for `name`. Callers must
    /// have called [`Container::ensure_parsed`] first (every public
    /// accessor below does so); kept as a separate `&self` step so
    /// reading the tree doesn't extend a `&mut self` borrow across the
    /// whole call (see the module docs' note on dropping the
    /// `href_to_name` cache for the same reason).
    pub fn get_xml(&self, name: &str) -> Result<&Xml> {
        match self.base.parsed_cache.get(name) {
            Some(ParsedItem::Xml(x)) => Ok(x),
            Some(_) => bail!("{name} is not an XML document"),
            None => bail!("{name} has not been parsed"),
        }
    }

    pub fn get_xml_mut(&mut self, name: &str) -> Result<&mut Xml> {
        match self.base.parsed_cache.get_mut(name) {
            Some(ParsedItem::Xml(x)) => Ok(x),
            Some(_) => bail!("{name} is not an XML document"),
            None => bail!("{name} has not been parsed"),
        }
    }

    pub fn get_xhtml(&self, name: &str) -> Result<&Dom> {
        match self.base.parsed_cache.get(name) {
            Some(ParsedItem::Xhtml(d)) => Ok(d),
            Some(_) => bail!("{name} is not an XHTML document"),
            None => bail!("{name} has not been parsed"),
        }
    }

    pub fn get_xhtml_mut(&mut self, name: &str) -> Result<&mut Dom> {
        match self.base.parsed_cache.get_mut(name) {
            Some(ParsedItem::Xhtml(d)) => Ok(d),
            Some(_) => bail!("{name} is not an XHTML document"),
            None => bail!("{name} has not been parsed"),
        }
    }

    /// Returns the raw (unparsed) CSS text cached for `name`. See
    /// [`ParsedItem::Css`]'s docs for why this crate caches CSS as text
    /// rather than a structured object: turning it into a
    /// [`crate::css::Stylesheet`] on demand is [`Container::parsed_stylesheet`]'s
    /// job, not the cache's.
    pub fn get_css_text(&self, name: &str) -> Result<&str> {
        match self.base.parsed_cache.get(name) {
            Some(ParsedItem::Css(s)) => Ok(s.as_str()),
            Some(_) => bail!("{name} is not a CSS document"),
            None => bail!("{name} has not been parsed"),
        }
    }

    /// Overwrites the cached CSS text for `name` (port of the effect of
    /// Python's `style.text = ...`/`sheet.cssText = ...` mutating the
    /// cached, live `CSSStyleSheet` object in place -- since this crate
    /// caches CSS as text rather than a mutable object graph, the
    /// equivalent is: parse a copy, mutate it, then write the
    /// serialized result back here). Does **not** call
    /// [`Container::dirty`]; callers that changed the content are
    /// expected to do that themselves, matching every real call site in
    /// `css.py` (which always calls `container.dirty(name)` right after
    /// assigning `sheet.cssText`/`style.text`).
    pub fn set_css_text(&mut self, name: &str, text: impl Into<String>) {
        self.base
            .parsed_cache
            .insert(name.to_string(), ParsedItem::Css(text.into()));
    }

    /// Port of `container.parsed(name)` for a CSS file: ensures `name`
    /// is read and cached as text, then parses that text into a fresh
    /// [`crate::css::Stylesheet`]. Unlike `get_xml`/`get_xhtml`, the
    /// parsed object is **not** cached -- see [`ParsedItem::Css`]'s
    /// docs; callers that mutate the result must write it back via
    /// [`Container::set_css_text`] (with
    /// `sheet.to_css_text()`) themselves.
    pub fn parsed_stylesheet(&mut self, name: &str) -> Result<crate::css::Stylesheet> {
        self.ensure_parsed(name)?;
        Ok(crate::css::Stylesheet::parse(self.get_css_text(name)?))
    }

    /// Port of the `opf` property.
    pub fn opf(&mut self) -> Result<&Xml> {
        self.ensure_opf_parsed()?;
        self.get_xml(&self.opf_name.clone())
    }

    pub fn opf_mut(&mut self) -> Result<&mut Xml> {
        self.ensure_opf_parsed()?;
        let opf_name = self.opf_name.clone();
        self.get_xml_mut(&opf_name)
    }

    /// Port of `Container.opf_xpath`.
    pub fn opf_xpath(&mut self, expr: &str) -> Result<Vec<XmlNodeId>> {
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        Ok(self.get_xml(&self.opf_name.clone())?.opf_xpath(expr, &ns))
    }

    pub fn opf_root(&mut self) -> Result<XmlNodeId> {
        self.ensure_opf_parsed()?;
        self.get_xml(&self.opf_name.clone())?
            .root_element()
            .ok_or_else(|| anyhow::anyhow!("OPF has no root element"))
    }

    pub fn dirty(&mut self, name: &str) {
        self.dirtied.insert(name.to_string());
    }

    pub fn insert_into_xml(
        &mut self,
        name: &str,
        parent: XmlNodeId,
        item: XmlNodeId,
        index: Option<usize>,
    ) -> Result<()> {
        self.get_xml_mut(name)?.insert_element(parent, item, index);
        Ok(())
    }

    pub fn remove_from_xml(&mut self, name: &str, item: XmlNodeId) -> Result<()> {
        self.get_xml_mut(name)?.detach(item);
        Ok(())
    }

    // -- version / manifest / spine / guide ---------------------------

    /// Port of the `opf_version` property.
    pub fn opf_version(&mut self) -> Result<String> {
        let root = self.opf_root()?;
        Ok(self
            .get_xml(&self.opf_name.clone())?
            .get_attr(root, "version")
            .unwrap_or("")
            .to_string())
    }

    /// Port of `opf_version_parsed`: `(major, minor)`.
    pub fn opf_version_parsed(&mut self) -> Result<(u32, u32)> {
        let v = self.opf_version()?;
        let mut parts = v.split('.');
        let major = parts.next().and_then(|s| s.parse().ok()).unwrap_or(2);
        let minor = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        Ok((major, minor))
    }

    /// Port of `ContainerBase.guess_type`. Placed on `Container`
    /// (rather than `ContainerBase`) since it genuinely needs
    /// `opf_version_parsed`, which only a `Container` has -- see the
    /// module docs.
    pub fn guess_type(&mut self, name: &str) -> String {
        let version = self.opf_version_parsed().unwrap_or((2, 0));
        adjust_mime_for_epub(name, None, version)
    }

    /// Port of `manifest_items` -- node ids of every manifest `<item
    /// href id>`.
    pub fn manifest_items(&mut self) -> Result<Vec<XmlNodeId>> {
        self.opf_xpath("//opf:manifest/opf:item[@href and @id]")
    }

    /// Port of `manifest_id_map`.
    pub fn manifest_id_map(&mut self) -> Result<HashMap<String, String>> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let items = xml.opf_xpath("//opf:manifest/opf:item[@href and @id]", &ns);
        let mut out = HashMap::new();
        for item in items {
            let id = xml.get_attr(item, "id").unwrap_or("").to_string();
            let href = xml.get_attr(item, "href").unwrap_or("");
            if let Some(name) = self.href_to_name(href, Some(&opf_name)) {
                out.insert(id, name);
            }
        }
        Ok(out)
    }

    /// Port of `manifest_type_map`.
    pub fn manifest_type_map(&mut self) -> Result<HashMap<String, Vec<String>>> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let items = xml.opf_xpath("//opf:manifest/opf:item[@href and @media-type]", &ns);
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        for item in items {
            let mt = xml
                .get_attr(item, "media-type")
                .unwrap_or("")
                .to_lowercase();
            let href = xml.get_attr(item, "href").unwrap_or("");
            if let Some(name) = self.href_to_name(href, Some(&opf_name)) {
                out.entry(mt).or_default().push(name);
            }
        }
        Ok(out)
    }

    /// Port of `manifest_items_of_type`.
    pub fn manifest_items_of_type(
        &mut self,
        predicate: impl Fn(&str) -> bool,
    ) -> Result<Vec<String>> {
        let map = self.manifest_type_map()?;
        let mut out = Vec::new();
        for (mt, names) in map {
            if predicate(&mt) {
                out.extend(names);
            }
        }
        Ok(out)
    }

    /// Port of `manifest_items_with_property`. Simplified: matches the
    /// `properties` attribute containing `property_name` literally.
    /// Python additionally resolves custom `prefix:` remappings
    /// declared on `<package prefix="...">` via
    /// `calibre.ebooks.metadata.opf3.read_prefixes`/
    /// `items_with_property` -- a not-yet-ported module. Real OPF
    /// content overwhelmingly uses the standard, unprefixed property
    /// names (`cover-image`, `nav`, `scripted`, ...), which this covers;
    /// custom-prefixed properties are the rare case this simplification
    /// misses.
    pub fn manifest_items_with_property(&mut self, property_name: &str) -> Result<Vec<String>> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let items = xml.opf_xpath("//opf:manifest/opf:item", &ns);
        let mut out = Vec::new();
        for item in items {
            let props = xml.get_attr(item, "properties").unwrap_or("");
            if props.split_whitespace().any(|p| p == property_name) {
                if let Some(href) = xml.get_attr(item, "href") {
                    if let Some(name) = self.href_to_name(href, Some(&opf_name)) {
                        out.push(name);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Port of `guide_type_map`.
    pub fn guide_type_map(&mut self) -> Result<HashMap<String, String>> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let items = xml.opf_xpath("//opf:guide/opf:reference[@href and @type]", &ns);
        let mut out = HashMap::new();
        for item in items {
            let t = xml.get_attr(item, "type").unwrap_or("").to_string();
            let href = xml.get_attr(item, "href").unwrap_or("");
            if let Some(name) = self.href_to_name(href, Some(&opf_name)) {
                out.insert(t, name);
            }
        }
        Ok(out)
    }

    /// Port of `spine_iter`: `(itemref node, name, is_linear)`, linear
    /// items first, then non-linear (matching Python's ordering).
    pub fn spine_iter(&mut self) -> Result<Vec<(XmlNodeId, String, bool)>> {
        let manifest_id_map = self.manifest_id_map()?;
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let itemrefs = xml.opf_xpath("//opf:spine/opf:itemref[@idref]", &ns);
        let mut linear = Vec::new();
        let mut non_linear = Vec::new();
        for item in itemrefs {
            let idref = xml.get_attr(item, "idref").unwrap_or("");
            if let Some(name) = manifest_id_map.get(idref) {
                if self.name_path_map.contains_key(name) {
                    let is_linear = xml.get_attr(item, "linear").unwrap_or("yes") == "yes";
                    if is_linear {
                        linear.push((item, name.clone(), true));
                    } else {
                        non_linear.push((item, name.clone(), false));
                    }
                }
            }
        }
        linear.extend(non_linear);
        Ok(linear)
    }

    pub fn index_in_spine(&mut self, name: &str) -> Result<Option<usize>> {
        Ok(self
            .spine_iter()?
            .into_iter()
            .position(|(_, n, _)| n == name))
    }

    pub fn spine_names(&mut self) -> Result<Vec<(String, bool)>> {
        Ok(self
            .spine_iter()?
            .into_iter()
            .map(|(_, n, l)| (n, l))
            .collect())
    }

    /// Port of `remove_from_spine`.
    pub fn remove_from_spine(
        &mut self,
        spine_items: &[(String, bool)],
        remove_if_no_longer_in_spine: bool,
    ) -> Result<()> {
        let current = self.spine_iter()?;
        let opf_name = self.opf_name.clone();
        let mut nixed = HashSet::new();
        for ((name, remove), (item, xname, _linear)) in spine_items.iter().zip(current.iter()) {
            if *remove && name == xname {
                self.get_xml_mut(&opf_name)?.detach(*item);
                nixed.insert(name.clone());
            }
        }
        if remove_if_no_longer_in_spine {
            let still_in_spine: HashSet<String> =
                self.spine_names()?.into_iter().map(|(n, _)| n).collect();
            for name in nixed {
                if !still_in_spine.contains(&name) {
                    self.remove_item(&name, true)?;
                }
            }
        }
        Ok(())
    }

    /// Port of `set_spine`.
    pub fn set_spine(&mut self, spine_items: &[(String, bool)]) -> Result<()> {
        let manifest_id_map = self.manifest_id_map()?;
        let mut id_of: HashMap<&str, &str> = HashMap::new();
        for (id, name) in &manifest_id_map {
            id_of.insert(name.as_str(), id.as_str());
        }
        let opf_name = self.opf_name.clone();
        let existing = self.spine_iter()?;
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let spine_node = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:spine", &ns)
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("OPF has no <spine>"))?
        };
        let xml = self.get_xml_mut(&opf_name)?;
        for (item, _, _) in &existing {
            xml.detach(*item);
        }
        for (name, linear) in spine_items {
            let idref = *id_of
                .get(name.as_str())
                .ok_or_else(|| anyhow::anyhow!("{name} is not in the manifest"))?;
            let node = xml.new_element("itemref", Some(OPF2_NS));
            xml.set_attr(node, "idref", idref);
            if !linear {
                xml.set_attr(node, "linear", "no");
            }
            xml.insert_element(spine_node, node, None);
        }
        self.dirty(&opf_name);
        Ok(())
    }

    // -- manifest mutation ---------------------------------------------

    /// Port of `add_name_to_manifest`.
    pub fn add_name_to_manifest(&mut self, name: &str, suggested_id: &str) -> Result<String> {
        let all_ids = self.all_opf_ids()?;
        let base_id = if suggested_id.is_empty() {
            "id"
        } else {
            suggested_id
        };
        let mut item_id = base_id.to_string();
        let mut c = 0u32;
        while all_ids.contains(&item_id) {
            c += 1;
            item_id = format!("{base_id}-{c}");
        }
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let manifest_node = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:manifest", &ns)
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("OPF has no <manifest>"))?
        };
        let href = self.name_to_href(name, Some(&opf_name));
        let media_type = self
            .base
            .mime_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_type(name));
        let xml = self.get_xml_mut(&opf_name)?;
        let item = xml.new_element("item", Some(OPF2_NS));
        xml.set_attr(item, "id", item_id.clone());
        xml.set_attr(item, "href", href);
        xml.set_attr(item, "media-type", media_type);
        xml.insert_element(manifest_node, item, None);
        self.dirty(&opf_name);
        Ok(item_id)
    }

    fn all_opf_ids(&mut self) -> Result<HashSet<String>> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let xml = self.get_xml(&opf_name)?;
        let mut ids = HashSet::new();
        collect_ids(xml, xml.root, &mut ids);
        Ok(ids)
    }

    pub fn manifest_has_name(&mut self, name: &str) -> Result<bool> {
        let opf_name = self.opf_name.clone();
        self.ensure_opf_parsed()?;
        let ns = opf_namespaces();
        let xml = self.get_xml(&opf_name)?;
        let items = xml.opf_xpath("//opf:manifest/opf:item[@href]", &ns);
        for item in items {
            let href = xml.get_attr(item, "href").unwrap_or("");
            if self.href_to_name(href, Some(&opf_name)).as_deref() == Some(name) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Port of `make_name_unique`.
    pub fn make_name_unique(&mut self, name: &str) -> Result<String> {
        let mut name = name.to_string();
        let mut c = 0u32;
        loop {
            if !self.has_name_case_insensitive(&name) && !self.manifest_has_name(&name)? {
                return Ok(name);
            }
            c += 1;
            let (base, ext) = match name.rsplit_once('.') {
                Some((b, e)) => (b.to_string(), e.to_string()),
                None => (name.clone(), String::new()),
            };
            let base = if c > 1 {
                base.rsplit_once('-')
                    .map(|(b, _)| b.to_string())
                    .unwrap_or(base)
            } else {
                base
            };
            name = format!("{base}-{c}.{ext}");
        }
    }

    /// Port of `add_file`.
    pub fn add_file(
        &mut self,
        name: &str,
        data: &[u8],
        media_type: Option<&str>,
        spine_index: Option<usize>,
        modify_name_if_needed: bool,
    ) -> Result<String> {
        if name.contains("..") {
            bail!("Names are not allowed to have .. in them");
        }
        let mut name = name.to_string();
        let mut href = self.name_to_href(&name, Some(&self.opf_name.clone()));
        if self.has_name_case_insensitive(&name) || self.manifest_has_name(&name)? {
            if !modify_name_if_needed {
                if self.has_name_case_insensitive(&name) {
                    bail!("A file with the name {name} already exists");
                }
                bail!("An item with the href {href} already exists in the manifest");
            }
            name = self.make_name_unique(&name)?;
            href = self.name_to_href(&name, Some(&self.opf_name.clone()));
        }
        let _ = &href;
        let path = self.name_to_abspath(&name);
        if let Some(base) = path.parent() {
            fs::create_dir_all(base)
                .with_context(|| format!("Failed to create {}", base.display()))?;
        }
        fs::write(&path, data).with_context(|| format!("Failed to write {}", path.display()))?;
        let mt = media_type
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.guess_type(&name));
        self.name_path_map.insert(name.clone(), path);
        self.base.mime_map.insert(name.clone(), mt.clone());
        if self.ok_to_be_unmanifested(&name) {
            return Ok(name);
        }
        let item_id = self.add_name_to_manifest(&name, "")?;
        if OEB_DOCS.contains(&mt.as_str()) {
            let opf_name = self.opf_name.clone();
            let ns = opf_namespaces();
            self.ensure_opf_parsed()?;
            let spine_node = {
                let xml = self.get_xml(&opf_name)?;
                xml.opf_xpath("//opf:spine", &ns)
                    .first()
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("OPF has no <spine>"))?
            };
            let xml = self.get_xml_mut(&opf_name)?;
            let itemref = xml.new_element("itemref", Some(OPF2_NS));
            xml.set_attr(itemref, "idref", item_id);
            xml.insert_element(spine_node, itemref, spine_index);
            self.dirty(&opf_name);
        }
        Ok(name)
    }

    /// Port of `generate_item`. `unique_href` controls whether `name`
    /// is passed through [`Container::make_name_unique`] first.
    pub fn generate_item(
        &mut self,
        name: &str,
        id_prefix: &str,
        media_type: Option<&str>,
        unique_href: bool,
    ) -> Result<XmlNodeId> {
        let id_prefix = if id_prefix.is_empty() {
            "id"
        } else {
            id_prefix
        };
        let media_type = media_type
            .map(|s| s.to_string())
            .unwrap_or_else(|| self.guess_type(name));
        let name = if unique_href {
            self.make_name_unique(name)?
        } else {
            name.to_string()
        };
        let href = self.name_to_href(&name, Some(&self.opf_name.clone()));

        let mut all_ids = self.all_opf_ids()?;
        if id_prefix.ends_with('-') {
            all_ids.insert(id_prefix.to_string());
        }
        let mut item_id = id_prefix.to_string();
        let mut c = 0u32;
        while all_ids.contains(&item_id) {
            c += 1;
            item_id = format!("{id_prefix}{c}");
        }

        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let manifest_node = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:manifest", &ns)
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("OPF has no <manifest>"))?
        };
        let xml = self.get_xml_mut(&opf_name)?;
        let item = xml.new_element("item", Some(OPF2_NS));
        xml.set_attr(item, "id", item_id);
        xml.set_attr(item, "href", href);
        xml.set_attr(item, "media-type", media_type.clone());
        xml.insert_element(manifest_node, item, None);
        self.dirty(&opf_name);

        let resolved_name = self
            .href_to_name(
                &self.name_to_href(&name, Some(&self.opf_name.clone())),
                Some(&self.opf_name.clone()),
            )
            .unwrap_or(name);
        let path = self.name_to_abspath(&resolved_name);
        self.name_path_map
            .insert(resolved_name.clone(), path.clone());
        self.base.mime_map.insert(resolved_name, media_type);
        if let Some(base) = path.parent() {
            fs::create_dir_all(base).ok();
        }
        // Ensure the file exists (empty), matching Python.
        File::create(&path).with_context(|| format!("Failed to create {}", path.display()))?;
        Ok(item)
    }

    /// Port of `apply_unique_properties`.
    pub fn apply_unique_properties(
        &mut self,
        name: Option<&str>,
        properties: &[&str],
    ) -> Result<(Vec<String>, Vec<String>)> {
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let items = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:manifest/opf:item", &ns)
        };
        let mut removed_names = Vec::new();
        let mut added_names = Vec::new();
        // Pre-resolve names for each item (needs `&self`, can't overlap
        // with the `&mut` xml borrow below).
        let mut resolved: Vec<(XmlNodeId, Option<String>)> = Vec::new();
        {
            let xml = self.get_xml(&opf_name)?;
            for item in &items {
                let href = xml.get_attr(*item, "href");
                let n = href.and_then(|h| self.href_to_name(h, Some(&opf_name)));
                resolved.push((*item, n));
            }
        }
        let xml = self.get_xml_mut(&opf_name)?;
        for (item, iname) in &resolved {
            let mut props: Vec<String> = xml
                .get_attr(*item, "properties")
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            for prop in properties {
                let lprops: HashSet<String> = props.iter().map(|p| p.to_lowercase()).collect();
                if lprops.contains(&prop.to_lowercase()) {
                    if name != iname.as_deref() {
                        if let Some(n) = iname {
                            removed_names.push(n.clone());
                        }
                        props.retain(|p| p.to_lowercase() != prop.to_lowercase());
                        if props.is_empty() {
                            xml.remove_attr(*item, "properties");
                        } else {
                            xml.set_attr(*item, "properties", props.join(" "));
                        }
                    }
                } else if name.is_some() && name == iname.as_deref() {
                    if let Some(n) = iname {
                        added_names.push(n.clone());
                    }
                    props.push(prop.to_string());
                    xml.set_attr(*item, "properties", props.join(" "));
                }
            }
        }
        self.dirty(&opf_name);
        Ok((removed_names, added_names))
    }

    /// Port of `add_properties`.
    pub fn add_properties(&mut self, name: &str, properties: &[&str]) -> Result<bool> {
        if properties.is_empty() {
            return Ok(true);
        }
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let items = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:manifest/opf:item", &ns)
        };
        let mut resolved: Vec<(XmlNodeId, Option<String>)> = Vec::new();
        {
            let xml = self.get_xml(&opf_name)?;
            for item in &items {
                let href = xml.get_attr(*item, "href");
                let n = href.and_then(|h| self.href_to_name(h, Some(&opf_name)));
                resolved.push((*item, n));
            }
        }
        let xml = self.get_xml_mut(&opf_name)?;
        for (item, iname) in &resolved {
            if iname.as_deref() == Some(name) {
                let mut props: HashSet<String> = xml
                    .get_attr(*item, "properties")
                    .unwrap_or("")
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                for p in properties {
                    props.insert(p.to_string());
                }
                let mut props: Vec<String> = props.into_iter().collect();
                props.sort();
                xml.set_attr(*item, "properties", props.join(" "));
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Port of `remove_item`.
    pub fn remove_item(&mut self, name: &str, remove_from_guide: bool) -> Result<()> {
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;

        let manifest_items = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:manifest/opf:item[@href]", &ns)
        };
        let mut removed_ids = HashSet::new();
        let mut opf_changed = false;
        {
            let mut to_remove = Vec::new();
            {
                let xml = self.get_xml(&opf_name)?;
                for item in &manifest_items {
                    let href = xml.get_attr(*item, "href").unwrap_or("");
                    if self.href_to_name(href, Some(&opf_name)).as_deref() == Some(name) {
                        if let Some(id) = xml.get_attr(*item, "id") {
                            removed_ids.insert(id.to_string());
                        }
                        to_remove.push(*item);
                    }
                }
            }
            let xml = self.get_xml_mut(&opf_name)?;
            for item in to_remove {
                xml.detach(item);
                opf_changed = true;
            }
        }

        if !removed_ids.is_empty() {
            let xml = self.get_xml_mut(&opf_name)?;
            for spine in xml.opf_xpath("//opf:spine", &ns) {
                if let Some(tocref) = xml.get_attr(spine, "toc").map(|s| s.to_string()) {
                    if removed_ids.contains(&tocref) {
                        xml.remove_attr(spine, "toc");
                        opf_changed = true;
                    }
                }
            }
            let itemrefs = xml.opf_xpath("//opf:spine/opf:itemref[@idref]", &ns);
            for item in itemrefs {
                if removed_ids.contains(xml.get_attr(item, "idref").unwrap_or("")) {
                    xml.detach(item);
                    opf_changed = true;
                }
            }
            let covers = xml.opf_xpath(r#"//opf:meta[@name="cover" and @content]"#, &ns);
            for meta in covers {
                if removed_ids.contains(xml.get_attr(meta, "content").unwrap_or("")) {
                    xml.detach(meta);
                    opf_changed = true;
                }
            }
            let refines = xml.opf_xpath("//opf:meta[@refines]", &ns);
            for meta in refines {
                let q = xml.get_attr(meta, "refines").unwrap_or("");
                if let Some(stripped) = q.strip_prefix('#') {
                    if removed_ids.contains(stripped) {
                        xml.detach(meta);
                        opf_changed = true;
                    }
                }
            }
        }

        if remove_from_guide {
            let guide_refs = {
                let xml = self.get_xml(&opf_name)?;
                xml.opf_xpath("//opf:guide/opf:reference[@href]", &ns)
            };
            let mut to_remove = Vec::new();
            {
                let xml = self.get_xml(&opf_name)?;
                for item in &guide_refs {
                    let href = xml.get_attr(*item, "href").unwrap_or("");
                    if self.href_to_name(href, Some(&opf_name)).as_deref() == Some(name) {
                        to_remove.push(*item);
                    }
                }
            }
            let xml = self.get_xml_mut(&opf_name)?;
            for item in to_remove {
                xml.detach(item);
                opf_changed = true;
            }
        }

        if opf_changed {
            self.dirty(&opf_name);
        }

        if let Some(path) = self.name_path_map.remove(name) {
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove {}", path.display()))?;
            }
        }
        self.base.mime_map.remove(name);
        self.base.parsed_cache.remove(name);
        self.dirtied.remove(name);
        Ok(())
    }

    /// Port of `set_media_overlay_durations`.
    pub fn set_media_overlay_durations(&mut self, duration_map: &[(String, f64)]) -> Result<()> {
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        {
            let xml = self.get_xml(&opf_name)?;
            let existing = xml.opf_xpath(r#"//opf:meta[@property="media:duration"]"#, &ns);
            let metadata_node = xml
                .opf_xpath("//opf:metadata", &ns)
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("OPF has no <metadata>"))?;
            let _ = existing;
            let _ = metadata_node;
        }
        let existing = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath(r#"//opf:meta[@property="media:duration"]"#, &ns)
        };
        let xml = self.get_xml_mut(&opf_name)?;
        for e in existing {
            xml.detach(e);
        }
        let metadata_node = xml
            .opf_xpath("//opf:metadata", &ns)
            .first()
            .copied()
            .ok_or_else(|| anyhow::anyhow!("OPF has no <metadata>"))?;
        let mut total = 0.0;
        for (item_id, duration) in duration_map {
            let meta = xml.new_element("meta", Some(OPF2_NS));
            xml.set_attr(meta, "property", "media:duration");
            xml.set_attr(meta, "refines", format!("#{item_id}"));
            xml.set_element_text(meta, seconds_to_timestamp(*duration));
            xml.insert_element(metadata_node, meta, None);
            total += duration;
        }
        if !duration_map.is_empty() {
            let meta = xml.new_element("meta", Some(OPF2_NS));
            xml.set_attr(meta, "property", "media:duration");
            xml.set_element_text(meta, seconds_to_timestamp(total));
            xml.insert_element(metadata_node, meta, None);
        }
        self.dirty(&opf_name);
        Ok(())
    }

    /// Port of `opf_get_or_create`.
    pub fn opf_get_or_create(&mut self, name: &str) -> Result<XmlNodeId> {
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let existing = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath(&format!("//opf:{name}"), &ns)
        };
        if let Some(first) = existing.first() {
            return Ok(*first);
        }
        let package = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath("//opf:package", &ns)
                .first()
                .copied()
                .ok_or_else(|| anyhow::anyhow!("OPF has no <package>"))?
        };
        let xml = self.get_xml_mut(&opf_name)?;
        let item = xml.new_element(name, Some(OPF2_NS));
        xml.insert_element(package, item, None);
        self.dirty(&opf_name);
        Ok(item)
    }

    /// Port of `format_opf`.
    pub fn format_opf(&mut self) -> Result<()> {
        let opf_name = self.opf_name.clone();
        let ns = opf_namespaces();
        self.ensure_opf_parsed()?;
        let covers = {
            let xml = self.get_xml(&opf_name)?;
            xml.opf_xpath(r#"//opf:meta[@name="cover"]"#, &ns)
        };
        let xml = self.get_xml_mut(&opf_name)?;
        for meta in covers {
            if let Some(content) = xml.get_attr(meta, "content").map(|s| s.to_string()) {
                xml.remove_attr(meta, "content");
                xml.set_attr(meta, "content", content);
            }
        }
        Ok(())
    }

    /// Port of `serialize_item`.
    pub fn serialize_item(&mut self, name: &str) -> Result<Vec<u8>> {
        if name == self.opf_name {
            self.format_opf()?;
        }
        self.ensure_parsed(name)?;
        match self.base.parsed_cache.get(name) {
            Some(ParsedItem::Xml(xml)) => Ok(xml.serialize()),
            Some(ParsedItem::Xhtml(dom)) => Ok(dom.serialize(dom.root).into_bytes()),
            Some(ParsedItem::Css(text)) => Ok(text.clone().into_bytes()),
            Some(ParsedItem::Raw(bytes)) => Ok(bytes.clone()),
            None => bail!("{name} has not been parsed"),
        }
    }

    /// Port of `commit_item`.
    pub fn commit_item(&mut self, name: &str, keep_parsed: bool) -> Result<()> {
        if !self.base.parsed_cache.contains_key(name) {
            return Ok(());
        }
        let data = self.serialize_item(name)?;
        self.dirtied.remove(name);
        if !keep_parsed {
            self.base.parsed_cache.remove(name);
        }
        let dest = self
            .name_path_map
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("No such name: {name}"))?;
        if self.cloned && nlinks(&dest) > 1 {
            fs::remove_file(&dest).ok();
        }
        fs::write(&dest, &data).with_context(|| format!("Failed to write {}", dest.display()))?;
        Ok(())
    }

    pub fn filesize(&mut self, name: &str) -> Result<u64> {
        if self.dirtied.contains(name) {
            self.commit_item(name, true)?;
        }
        let path = self.name_to_abspath(name);
        Ok(fs::metadata(&path)
            .with_context(|| format!("Failed to stat {}", path.display()))?
            .len())
    }

    /// Port of `get_file_path_for_processing`.
    pub fn get_file_path_for_processing(
        &mut self,
        name: &str,
        allow_modification: bool,
    ) -> Result<PathBuf> {
        if self.dirtied.contains(name) {
            self.commit_item(name, false)?;
        }
        self.base.parsed_cache.remove(name);
        let path = self.name_to_abspath(name);
        if let Some(base) = path.parent() {
            if !base.exists() {
                fs::create_dir_all(base)?;
            } else if self.cloned && allow_modification && path.exists() && nlinks(&path) > 1 {
                let temp = path.with_extension("xxx-decouple");
                fs::copy(&path, &temp)?;
                fs::remove_file(&path)?;
                fs::rename(&temp, &path)?;
            }
        }
        Ok(path)
    }

    /// Port of `raw_data`.
    pub fn raw_data(&mut self, name: &str, decode: bool) -> Result<Vec<u8>> {
        let path = self.get_file_path_for_processing(name, false)?;
        let data = fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))?;
        let mime = self
            .base
            .mime_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_type(name));
        if decode
            && (OEB_STYLES.contains(&mime.as_str())
                || OEB_DOCS.contains(&mime.as_str())
                || mime == "text/plain"
                || mime.ends_with("+xml")
                || mime.ends_with("/xml"))
        {
            return Ok(self.base.decode(&data, true).into_bytes());
        }
        Ok(data)
    }

    /// Port of `open(name, 'rb')`/`open(name, 'wb')`, simplified to
    /// read/write whole-file helpers rather than returning a live file
    /// handle -- see the module docs.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        let path = self.get_file_path_for_processing(name, false)?;
        fs::read(&path).with_context(|| format!("Failed to read {}", path.display()))
    }

    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<()> {
        let path = self.get_file_path_for_processing(name, true)?;
        fs::write(&path, data).with_context(|| format!("Failed to write {}", path.display()))
    }

    /// Port of `Container.commit`.
    pub fn commit(&mut self, keep_parsed: bool) -> Result<()> {
        for name in self.dirtied.clone() {
            self.commit_item(&name, keep_parsed)?;
        }
        Ok(())
    }

    /// Port of `compare_to`.
    pub fn compare_to(&self, other: &Container) -> Result<String> {
        let ours: HashSet<&String> = self.name_path_map.keys().collect();
        let theirs: HashSet<&String> = other.name_path_map.keys().collect();
        if ours != theirs {
            return Ok("Set of files is not the same".to_string());
        }
        let mut mismatches = Vec::new();
        for (name, path) in &self.name_path_map {
            let opath = &other.name_path_map[name];
            let a = fs::read(path)?;
            let b = fs::read(opath)?;
            if a != b {
                mismatches.push(format!("The file {name} is not the same"));
            }
        }
        Ok(mismatches.join("\n"))
    }

    /// Port of `rename`. Real end to end: the cross-directory case
    /// (rebasing every relative link *inside* the moved file itself, so
    /// they still resolve after the move) uses
    /// [`super::replace::LinkRebaser`], ported in issue #165 -- this
    /// closes the gap issue #161 left here.
    pub fn rename(&mut self, current_name: &str, new_name: &str) -> Result<()> {
        if self.names_that_must_not_be_changed().contains(current_name) {
            bail!("Renaming of {current_name} is not allowed");
        }
        if self.exists(new_name)
            && (new_name == current_name || new_name.to_lowercase() != current_name.to_lowercase())
        {
            bail!("Cannot rename {current_name} to {new_name} as {new_name} already exists");
        }
        let new_path = self.name_to_abspath(new_name);
        if let Some(base) = new_path.parent() {
            if base.is_file() {
                bail!(
                    "Cannot rename {current_name} to {new_name} as {} is a file",
                    base.display()
                );
            }
            if !base.exists() {
                fs::create_dir_all(base)?;
            }
        }
        let old_path = self.name_to_abspath(current_name);
        self.commit_item(current_name, false)?;
        fs::rename(&old_path, &new_path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                old_path.display(),
                new_path.display()
            )
        })?;
        // Remove now-empty parent directories, matching Python.
        let mut parent = old_path.parent().map(|p| p.to_path_buf());
        while let Some(p) = parent {
            if fs::remove_dir(&p).is_err() {
                break;
            }
            parent = p.parent().map(|q| q.to_path_buf());
        }

        for map in [&mut self.base.mime_map, &mut self.base.encoding_map] {
            if let Some(v) = map.remove(current_name) {
                map.insert(new_name.to_string(), v);
            }
        }
        self.name_path_map.insert(new_name.to_string(), new_path);
        self.name_path_map.remove(current_name);
        self.base.parsed_cache.remove(current_name);
        self.pretty_print.remove(current_name);
        self.dirtied.remove(current_name);
        if current_name == self.opf_name {
            self.opf_name = new_name.to_string();
        }
        if old_path.parent() != self.name_to_abspath(new_name).parent() {
            // Cross-directory rename: the file's own relative links now
            // resolve from a different directory, so they need
            // rebasing to still point at the same targets. Port of
            // `container.py`'s `rename`: only the *moved* file's own
            // links are touched here -- links *to* it from other files
            // are a separate, bulk operation (`replace.py`'s
            // `rename_files`, which calls `LinkReplacer` across the
            // whole container).
            let mut repl = super::replace::LinkRebaser::new(self, current_name, new_name);
            self.replace_links(new_name, |url, _ft| repl.rebase(url))?;
            self.dirty(new_name);
        }
        Ok(())
    }

    /// Port of `replace_links`. `replace_func` is handed each link's raw
    /// URL plus which kind of document it was found in (Python instead
    /// sets a `.file_type` attribute on the callable -- see the module
    /// docs for why a parameter is the more natural Rust shape); it
    /// returns `Some(new_url)` to replace it or `None` to leave it
    /// alone. Returns whether anything was actually replaced.
    ///
    /// Real for the OPF, XHTML/HTML (`href`/`src` attributes -- a
    /// bounded subset of Python's `oeb.base.iterlinks`' full attribute
    /// table, which also covers `srcset`, `longdesc`, `poster`, style
    /// attribute `url()`s, ...; this covers the overwhelming majority of
    /// real links) and NCX (`src` attributes) cases. The CSS case (`url(
    /// ...)` inside a stylesheet) needs a real CSS parser, which this
    /// crate doesn't have -- see [`super::utils::parse_css`]'s docs.
    pub fn replace_links(
        &mut self,
        name: &str,
        mut replace_func: impl FnMut(&str, LinkFileType) -> Option<String>,
    ) -> Result<bool> {
        let media_type = self
            .base
            .mime_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_type(name))
            .to_lowercase();
        let mut replaced = false;
        if name == self.opf_name {
            let opf_name = self.opf_name.clone();
            let ns = opf_namespaces();
            self.ensure_opf_parsed()?;
            let hrefs = {
                let xml = self.get_xml(&opf_name)?;
                xml.opf_xpath("//*[@href]", &ns)
            };
            let xml = self.get_xml_mut(&opf_name)?;
            for elem in hrefs {
                let cur = xml.get_attr(elem, "href").unwrap_or("").to_string();
                if let Some(new) = replace_func(&cur, LinkFileType::Opf) {
                    xml.set_attr(elem, "href", new);
                    replaced = true;
                }
            }
        } else if OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(&media_type)) {
            self.ensure_parsed(name)?;
            let dom = self.get_xhtml_mut(name)?;
            replaced = replace_links_in_dom(dom, LinkFileType::Text, &mut replace_func);
        } else if OEB_STYLES
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&media_type))
        {
            todo!(
                "placeholder: CSS url(...) link rewriting needs a real CSS \
                 parser, which this crate doesn't have -- see \
                 oeb::polish::utils::parse_css's docs (same gap as issues \
                 #34/#35/#36)"
            )
        } else if media_type == NCX_MIME.to_lowercase() {
            self.ensure_parsed(name)?;
            let empty_ns = HashMap::new();
            let srcs = {
                let xml = self.get_xml(name)?;
                xml.opf_xpath("//*[@src]", &empty_ns)
            };
            let xml = self.get_xml_mut(name)?;
            for elem in srcs {
                let cur = xml.get_attr(elem, "src").unwrap_or("").to_string();
                if let Some(new) = replace_func(&cur, LinkFileType::Ncx) {
                    xml.set_attr(elem, "src", new);
                    replaced = true;
                }
            }
        }
        if replaced {
            self.dirty(name);
        }
        Ok(replaced)
    }

    /// Port of `iterlinks`: every link found in `name`, as `(url,
    /// line_number, offset)`. `offset` is always `0` here (matching
    /// Python, which only computes a real sub-line column offset for
    /// the CSS case -- see [`Container::replace_links`]'s docs for why
    /// that case is `todo!()`).
    pub fn iterlinks(&mut self, name: &str) -> Result<Vec<(String, Option<u32>, usize)>> {
        let media_type = self
            .base
            .mime_map
            .get(name)
            .cloned()
            .unwrap_or_else(|| guess_type(name))
            .to_lowercase();
        let mut out = Vec::new();
        if name == self.opf_name {
            let opf_name = self.opf_name.clone();
            let ns = opf_namespaces();
            self.ensure_opf_parsed()?;
            let xml = self.get_xml(&opf_name)?;
            for elem in xml.opf_xpath("//*[@href]", &ns) {
                out.push((
                    xml.get_attr(elem, "href").unwrap_or("").to_string(),
                    xml.node(elem).sourceline,
                    0,
                ));
            }
        } else if OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(&media_type)) {
            self.ensure_parsed(name)?;
            let dom = self.get_xhtml(name)?;
            for el in dom.preorder_elements(dom.root) {
                for attr in ["href", "src"] {
                    if let Some(v) = dom.node(el).attrs.get(attr) {
                        out.push((v.clone(), None, 0));
                    }
                }
            }
        } else if OEB_STYLES
            .iter()
            .any(|m| m.eq_ignore_ascii_case(&media_type))
        {
            todo!(
                "placeholder: CSS url(...) link iteration needs a real CSS \
                 parser -- see Container::replace_links's docs"
            )
        } else if media_type == NCX_MIME.to_lowercase() {
            self.ensure_parsed(name)?;
            let empty_ns = HashMap::new();
            let xml = self.get_xml(name)?;
            for elem in xml.opf_xpath("//*[@src]", &empty_ns) {
                out.push((
                    xml.get_attr(elem, "src").unwrap_or("").to_string(),
                    xml.node(elem).sourceline,
                    0,
                ));
            }
        }
        Ok(out)
    }
}

fn replace_links_in_dom(
    dom: &mut Dom,
    file_type: LinkFileType,
    replace_func: &mut impl FnMut(&str, LinkFileType) -> Option<String>,
) -> bool {
    let mut replaced = false;
    for el in dom.preorder_elements(dom.root) {
        for attr in ["href", "src"] {
            let cur = dom.node(el).attrs.get(attr).cloned();
            if let Some(cur) = cur {
                if let Some(new) = replace_func(&cur, file_type) {
                    dom.node_mut(el).attrs.insert(attr.to_string(), new);
                    replaced = true;
                }
            }
        }
    }
    replaced
}

fn collect_ids(xml: &Xml, id: XmlNodeId, out: &mut HashSet<String>) {
    if let Some(v) = xml.get_attr(id, "id") {
        out.insert(v.to_string());
    }
    for &c in xml.children(id) {
        collect_ids(xml, c, out);
    }
}

/// Port of `seconds_to_timestamp`.
fn seconds_to_timestamp(duration: f64) -> String {
    let seconds_total = duration.floor();
    let float_part = duration - seconds_total;
    let seconds_total = seconds_total as u64;
    let hours = seconds_total / 3600;
    let minutes = (seconds_total % 3600) / 60;
    let seconds = seconds_total % 60;
    let mut ans = format!("{hours:02}:{minutes:02}:{seconds:02}");
    if float_part != 0.0 {
        let frac = format!("{float_part:.20}");
        let frac = frac.trim_end_matches('0');
        // Python: `f'{float_part:.20f}'.rstrip('0')[1:]` -- drops the
        // leading "0", keeping the "." and remaining digits.
        if let Some(stripped) = frac.strip_prefix('0') {
            ans.push_str(stripped);
        }
    }
    ans
}

#[cfg(unix)]
fn nlinks(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    fs::metadata(path).map(|m| m.nlink()).unwrap_or(1)
}

#[cfg(not(unix))]
fn nlinks(_path: &Path) -> u64 {
    1
}

/// Clone a folder using hard links where possible, falling back to a
/// real copy (matches `clone_dir`'s `hardlink_file`/`shutil.copy2`
/// fallback). `dest` is created if missing.
fn clone_dir(src: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let spath = entry.path();
        let dpath = dest.join(entry.file_name());
        if spath.is_dir() {
            clone_dir(&spath, &dpath)?;
        } else if fs::hard_link(&spath, &dpath).is_err() {
            fs::copy(&spath, &dpath)?;
        }
    }
    Ok(())
}

// -- free functions mirroring container.py's module-level helpers -----

pub fn abspath_to_name_at(path: &Path, root: &Path) -> String {
    let rel = pathdiff::diff_paths(path, root).unwrap_or_else(|| path.to_path_buf());
    let s = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    s.nfc().collect()
}

pub fn name_to_abspath_at(name: &str, root: &Path) -> PathBuf {
    let mut p = root.to_path_buf();
    for part in name.split('/') {
        p.push(part);
    }
    p
}

pub fn name_to_href_at(name: &str, root: &Path, base: Option<&str>) -> String {
    let fullpath = name_to_abspath_at(name, root);
    let basepath = match base {
        Some(b) => name_to_abspath_at(b, root)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf()),
        None => root.to_path_buf(),
    };
    let rel = pathdiff::diff_paths(&fullpath, &basepath).unwrap_or(fullpath);
    let s = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    crate::lit::urlquote(&s)
}

pub fn href_to_name_at(href: &str, root: &Path, base: Option<&str>) -> Option<String> {
    let (scheme, path_part) = split_scheme(href);
    if scheme.is_some() {
        return None;
    }
    let path_part = path_part.split('#').next().unwrap_or("");
    let path_part = path_part.split('?').next().unwrap_or("");
    if path_part.is_empty() {
        return None;
    }
    let unquoted = crate::lit::urlunquote(path_part);
    let basepath = match base {
        Some(b) => name_to_abspath_at(b, root)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| root.to_path_buf()),
        None => root.to_path_buf(),
    };
    let mut full = basepath;
    for part in unquoted.split('/') {
        if part == ".." {
            full.pop();
        } else if part.is_empty() || part == "." {
            continue;
        } else {
            full.push(part);
        }
    }
    Some(abspath_to_name_at(&full, root))
}

fn split_scheme(href: &str) -> (Option<&str>, &str) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.\-]*:").unwrap());
    if let Some(m) = re.find(href) {
        (Some(&href[..m.end() - 1]), &href[m.end()..])
    } else {
        (None, href)
    }
}

impl NameLookup for Container {
    fn exists(&self, name: &str) -> bool {
        Container::exists(self, name)
    }
    fn name_to_abspath(&self, name: &str) -> PathBuf {
        Container::name_to_abspath(self, name)
    }
}

// ===================================================================
// EPUB
// ===================================================================

pub const OCF_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:container";

/// Port of `ADOBE_OBFUSCATION`/`IDPF_OBFUSCATION`/`decrypt_font_data`
/// from `calibre.ebooks.conversion.plugins.epub_input` -- that module
/// isn't ported (out of scope), but this one function (a five-line XOR
/// cipher) is small and self-contained enough to duplicate narrowly
/// here rather than pull in the whole input-plugin module.
pub const ADOBE_OBFUSCATION: &str = "http://ns.adobe.com/pdf/enc#RC";
pub const IDPF_OBFUSCATION: &str = "http://www.idpf.org/2008/embedding";

pub fn decrypt_font_data(key: &[u8], data: &[u8], algorithm: &str) -> Vec<u8> {
    let crypt_len = if algorithm == ADOBE_OBFUSCATION {
        1024
    } else {
        1040
    }
    .min(data.len());
    let mut out = Vec::with_capacity(data.len());
    for (i, &b) in data[..crypt_len].iter().enumerate() {
        out.push(b ^ key[i % key.len().max(1)]);
    }
    out.extend_from_slice(&data[crypt_len..]);
    out
}

/// Standard RFC 3174 SHA-1 (needed for `read_raw_unique_identifier`'s
/// `hashlib.sha1(...)` call). No `sha1`/`sha-1` crate is a dependency of
/// this workspace and the standard algorithm is small, fixed, and
/// well-known enough to implement directly rather than adding a new
/// crate dependency for one call site. Not to be confused with
/// [`crate::lit::mssha1`], which is Microsoft's *non-standard* SHA-1
/// variant used only by the (unrelated) LIT format.
fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// `(package_id, raw_unique_identifier_text, idpf_obfuscation_key)` --
/// the return shape of [`EpubContainer::read_raw_unique_identifier`],
/// named to satisfy `clippy::type_complexity` and because each position
/// otherwise reads as opaque.
pub type RawUniqueIdentifier = (Option<String>, Option<String>, Option<Vec<u8>>);

/// Port of `EpubContainer`'s extra state (obfuscated-font tracking +
/// the on-disk path this book was opened from/will commit to).
pub struct EpubContainer {
    pub container: Container,
    pub path_to_epub: PathBuf,
    pub is_dir: bool,
    /// name -> (algorithm, key) for fonts deobfuscated on open (and
    /// re-obfuscated on commit).
    pub obfuscated_fonts: HashMap<String, (String, Vec<u8>)>,
}

impl std::ops::Deref for EpubContainer {
    type Target = Container;
    fn deref(&self) -> &Container {
        &self.container
    }
}

impl std::ops::DerefMut for EpubContainer {
    fn deref_mut(&mut self) -> &mut Container {
        &mut self.container
    }
}

const META_INF_NAMES: &[&str] = &[
    "container.xml",
    "manifest.xml",
    "encryption.xml",
    "metadata.xml",
    "signatures.xml",
    "rights.xml",
];

impl EpubContainer {
    /// Port of `EpubContainer.__init__`'s zip-file path.
    pub fn open_zip(path_to_epub: &Path, tdir: &Path) -> Result<EpubContainer> {
        fs::create_dir_all(tdir)?;
        let file = File::open(path_to_epub)
            .with_context(|| format!("Failed to open {}", path_to_epub.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("{} is not a valid zip/EPUB file", path_to_epub.display()))?;
        archive
            .extract(tdir)
            .with_context(|| format!("Failed to extract {}", path_to_epub.display()))?;
        Self::finish_open(path_to_epub, tdir, false)
    }

    /// Port of `EpubContainer.__init__`'s already-unzipped-directory
    /// path.
    pub fn open_dir(path_to_epub_dir: &Path, tdir: &Path) -> Result<EpubContainer> {
        clone_dir(path_to_epub_dir, tdir)?;
        Self::finish_open(path_to_epub_dir, tdir, true)
    }

    fn finish_open(path_to_epub: &Path, tdir: &Path, is_dir: bool) -> Result<EpubContainer> {
        let _ = fs::remove_file(tdir.join("mimetype"));

        // NFC-normalize every filename (Linux target: a direct rename
        // suffices; Python's double-rename trick exists purely to dodge
        // case-insensitive-filesystem collisions, not relevant here).
        normalize_tree_to_nfc(tdir)?;

        let container_path = tdir.join("META-INF").join("container.xml");
        if !container_path.exists() {
            return Err(
                PolishError::InvalidBook("No META-INF/container.xml in epub".to_string()).into(),
            );
        }
        let container_xml = fs::read_to_string(&container_path)?;
        let ocf = Xml::parse(&container_xml)?;
        let mut ns = HashMap::new();
        ns.insert("ocf", OCF_NS);
        let opf_mime = guess_type("a.opf");
        let rootfiles = ocf.opf_xpath("//ocf:rootfiles/ocf:rootfile[@media-type]", &ns);
        let opf_full_path = rootfiles
            .into_iter()
            .find(|&n| ocf.get_attr(n, "media-type") == Some(opf_mime.as_str()))
            .and_then(|n| ocf.get_attr(n, "full-path"))
            .ok_or_else(|| {
                PolishError::InvalidBook(
                    "META-INF/container.xml contains no link to OPF file".to_string(),
                )
            })?;
        let opf_path = tdir.join(crate::lit::urlunquote(opf_full_path));
        if !opf_path.exists() {
            return Err(PolishError::InvalidBook(
                "OPF file does not exist at location pointed to by META-INF/container.xml"
                    .to_string(),
            )
            .into());
        }

        let mut container = Container::open(tdir, &opf_path)?;
        container
            .base
            .parsed_cache
            .insert("META-INF/container.xml".to_string(), ParsedItem::Xml(ocf));

        let mut epub = EpubContainer {
            container,
            path_to_epub: path_to_epub.to_path_buf(),
            is_dir,
            obfuscated_fonts: HashMap::new(),
        };
        if epub.name_path_map.contains_key("META-INF/encryption.xml") {
            epub.process_encryption()?;
        }
        Ok(epub)
    }

    // -- overrides ------------------------------------------------------

    pub fn book_type(&self) -> &'static str {
        "epub"
    }

    pub fn book_type_for_display(&mut self) -> String {
        let mut ans = self.book_type().to_uppercase();
        if let Ok((major, minor)) = self.container.opf_version_parsed() {
            if major == 2 {
                ans.push_str(" 2");
            } else if minor == 0 {
                ans.push_str(&format!(" {major}"));
            } else {
                ans.push_str(&format!(" {major}.{minor}"));
            }
        }
        ans
    }

    pub fn names_that_need_not_be_manifested(&self) -> HashSet<String> {
        let mut s = self.container.names_that_need_not_be_manifested();
        s.extend(META_INF_NAMES.iter().map(|n| format!("META-INF/{n}")));
        s
    }

    pub fn ok_to_be_unmanifested(&self, name: &str) -> bool {
        self.names_that_need_not_be_manifested().contains(name) || name.starts_with("META-INF/")
    }

    pub fn names_that_must_not_be_removed(&self) -> HashSet<String> {
        let mut s = self.container.names_that_must_not_be_removed();
        s.insert("META-INF/container.xml".to_string());
        s
    }

    pub fn names_that_must_not_be_changed(&self) -> HashSet<String> {
        let mut s = self.container.names_that_must_not_be_changed();
        s.extend(META_INF_NAMES.iter().map(|n| format!("META-INF/{n}")));
        s
    }

    /// Port of `EpubContainer.rename`.
    pub fn rename(&mut self, old_name: &str, new_name: &str) -> Result<()> {
        let is_opf = old_name == self.container.opf_name;
        self.container.rename(old_name, new_name)?;
        if is_opf {
            let ns_map = {
                let mut m = HashMap::new();
                m.insert("ocf", OCF_NS);
                m
            };
            let opf_mime = guess_type("a.opf");
            let xml = self.container.get_xml_mut("META-INF/container.xml")?;
            let rootfiles = xml.opf_xpath("//ocf:rootfiles/ocf:rootfile[@media-type]", &ns_map);
            for r in rootfiles {
                if xml.get_attr(r, "media-type") == Some(opf_mime.as_str()) {
                    xml.set_attr(r, "full-path", new_name.to_string());
                }
            }
            self.container.dirty("META-INF/container.xml");
        }
        if let Some(v) = self.obfuscated_fonts.remove(old_name) {
            self.obfuscated_fonts
                .insert(new_name.to_string(), v.clone());
            let old_href = self.container.name_to_href(old_name, None);
            let new_href = self.container.name_to_href(new_name, None);
            if self
                .container
                .name_path_map
                .contains_key("META-INF/encryption.xml")
            {
                let mut changed = false;
                let xml = self.container.get_xml_mut("META-INF/encryption.xml")?;
                let crs = xml.opf_xpath(r#"//*[@URI]"#, &HashMap::new());
                for cr in crs {
                    if xml.get_attr(cr, "URI") == Some(old_href.as_str()) {
                        xml.set_attr(cr, "URI", new_href.clone());
                        changed = true;
                    }
                }
                if changed {
                    self.container.dirty("META-INF/encryption.xml");
                }
            }
        }
        Ok(())
    }

    /// Port of `EpubContainer.remove_item`.
    pub fn remove_item(&mut self, name: &str, remove_from_guide: bool) -> Result<()> {
        if name == "META-INF/encryption.xml" {
            self.obfuscated_fonts.clear();
        }
        if self.obfuscated_fonts.contains_key(name) {
            self.obfuscated_fonts.remove(name);
            if self
                .container
                .name_path_map
                .contains_key("META-INF/encryption.xml")
            {
                let href = self.container.name_to_href(name, None);
                let mut changed = false;
                let xml = self.container.get_xml_mut("META-INF/encryption.xml")?;
                let ems = xml.opf_xpath(r#"//*[@Algorithm]"#, &HashMap::new());
                let mut to_remove = Vec::new();
                for em in ems {
                    let alg = xml.get_attr(em, "Algorithm").unwrap_or("").to_string();
                    if alg != ADOBE_OBFUSCATION && alg != IDPF_OBFUSCATION {
                        continue;
                    }
                    if let Some(parent) = xml.parent(em) {
                        let crs = xml.opf_xpath(r#"//*[@URI]"#, &HashMap::new());
                        for cr in crs {
                            if descends_from(xml, cr, parent)
                                && xml.get_attr(cr, "URI") == Some(href.as_str())
                            {
                                to_remove.push(parent);
                            }
                        }
                    }
                }
                for parent in to_remove {
                    xml.detach(parent);
                    changed = true;
                }
                if changed {
                    self.container.dirty("META-INF/encryption.xml");
                }
            }
        }
        self.container.remove_item(name, remove_from_guide)
    }

    /// Port of `read_raw_unique_identifier`.
    pub fn read_raw_unique_identifier(&mut self) -> Result<RawUniqueIdentifier> {
        let root = self.container.opf_root()?;
        let opf_name = self.container.opf_name.clone();
        let package_id = self
            .container
            .get_xml(&opf_name)?
            .node(root)
            .attrs
            .get("unique-identifier")
            .cloned();
        let mut raw_unique_identifier = None;
        if let Some(pid) = &package_id {
            let xml = self.container.get_xml(&opf_name)?;
            if let Some(elem) = find_by_id_attr(xml, pid) {
                raw_unique_identifier = xml.element_text(elem).map(|s| s.to_string());
            }
        }
        let idpf_key = raw_unique_identifier.as_ref().map(|raw| {
            let stripped: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
            sha1_digest(stripped.as_bytes()).to_vec()
        });
        Ok((package_id, raw_unique_identifier, idpf_key))
    }

    /// Port of `iter_encryption_entries`: `(EncryptionMethod node, URI
    /// attribute value)` pairs.
    pub fn iter_encryption_entries(&mut self) -> Result<Vec<(XmlNodeId, Option<String>)>> {
        if !self
            .container
            .name_path_map
            .contains_key("META-INF/encryption.xml")
        {
            return Ok(Vec::new());
        }
        self.container.ensure_parsed("META-INF/encryption.xml")?;
        let xml = self.container.get_xml("META-INF/encryption.xml")?;
        let ems = xml.opf_xpath("//*[@Algorithm]", &HashMap::new());
        let mut out = Vec::new();
        for em in ems {
            let uri = xml
                .parent(em)
                .and_then(|parent| {
                    xml.children(parent)
                        .iter()
                        .find(|&&c| xml.has_attr(c, "URI"))
                        .copied()
                })
                .and_then(|cr| xml.get_attr(cr, "URI"))
                .map(|s| s.to_string());
            out.push((em, uri));
        }
        Ok(out)
    }

    /// Port of `process_encryption`. Real: parses
    /// `META-INF/encryption.xml`, detects the two font-obfuscation
    /// algorithms calibre understands, raises [`PolishError::drm`] for
    /// anything else (any other `EncryptionMethod` means genuine DRM,
    /// which this container cannot edit), and deobfuscates matched font
    /// files in place.
    pub fn process_encryption(&mut self) -> Result<()> {
        let entries = self.iter_encryption_entries()?;
        let opf_name = self.container.opf_name.clone();
        let mut fonts: HashMap<String, String> = HashMap::new();
        for (em, uri) in &entries {
            let alg = {
                let xml = self.container.get_xml("META-INF/encryption.xml")?;
                xml.get_attr(*em, "Algorithm").unwrap_or("").to_string()
            };
            if alg != ADOBE_OBFUSCATION && alg != IDPF_OBFUSCATION {
                return Err(PolishError::drm().into());
            }
            let Some(uri) = uri else { continue };
            if let Some(name) = self.container.href_to_name(uri, None) {
                if self.container.name_path_map.contains_key(&name) {
                    fonts.insert(name, alg);
                }
            }
        }

        let (_pid, _raw, idpf_key) = self.read_raw_unique_identifier()?;
        let mut key: Option<Vec<u8>> = None;
        let identifiers = {
            let ns = opf_namespaces();
            self.container.ensure_opf_parsed()?;
            self.container
                .get_xml(&opf_name)?
                .opf_xpath("//opf:metadata/dc:identifier", &ns)
        };
        for item in identifiers {
            let xml = self.container.get_xml(&opf_name)?;
            let scheme = xml
                .node(item)
                .attrs
                .iter()
                .find(|(k, _)| k.to_lowercase().ends_with("scheme"))
                .map(|(_, v)| v.clone());
            let text = xml.element_text(item).map(|s| s.to_string());
            let is_uuid_scheme = scheme
                .map(|s| s.eq_ignore_ascii_case("uuid"))
                .unwrap_or(false);
            let is_urn_uuid = text
                .as_deref()
                .map(|t| t.starts_with("urn:uuid:"))
                .unwrap_or(false);
            if is_uuid_scheme || is_urn_uuid {
                if let Some(text) = text {
                    if let Some((_, uuid_str)) = text.rsplit_once(':') {
                        if let Ok(u) = uuid::Uuid::parse_str(uuid_str.trim()) {
                            key = Some(u.as_bytes().to_vec());
                        }
                    }
                }
            }
        }

        for (name, alg) in fonts {
            let tkey = if alg == ADOBE_OBFUSCATION {
                key.clone()
            } else {
                idpf_key.clone()
            };
            let Some(tkey) = tkey else {
                return Err(anyhow::anyhow!("Failed to find obfuscation key for {name}"));
            };
            let raw = self.container.raw_data(&name, false)?;
            let decrypted = decrypt_font_data(&tkey, &raw, &alg);
            self.container.write_file(&name, &decrypted)?;
            self.obfuscated_fonts.insert(name, (alg, tkey));
        }
        Ok(())
    }

    /// Port of `update_modified_timestamp`. Narrow, real
    /// implementation of what `calibre.ebooks.metadata.opf3.
    /// set_last_modified_in_opf` does for EPUB3 (that module is not
    /// otherwise ported): set/create a
    /// `<meta property="dcterms:modified">` element to the current UTC
    /// time in the required `CCYY-MM-DDThh:mm:ssZ` form.
    pub fn update_modified_timestamp(&mut self) -> Result<()> {
        let opf_name = self.container.opf_name.clone();
        let ns = opf_namespaces();
        self.container.ensure_opf_parsed()?;
        let existing = {
            let xml = self.container.get_xml(&opf_name)?;
            xml.opf_xpath(r#"//opf:meta[@property="dcterms:modified"]"#, &ns)
        };
        let now = format_utc_now();
        if let Some(node) = existing.first() {
            self.container
                .get_xml_mut(&opf_name)?
                .set_element_text(*node, now);
        } else {
            let metadata_node = {
                let xml = self.container.get_xml(&opf_name)?;
                xml.opf_xpath("//opf:metadata", &ns)
                    .first()
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("OPF has no <metadata>"))?
            };
            let xml = self.container.get_xml_mut(&opf_name)?;
            let meta = xml.new_element("meta", Some(OPF2_NS));
            xml.set_attr(meta, "property", "dcterms:modified");
            xml.set_element_text(meta, now);
            xml.insert_element(metadata_node, meta, None);
        }
        self.container.dirty(&opf_name);
        Ok(())
    }

    /// Port of `EpubContainer.commit`.
    pub fn commit(&mut self, outpath: Option<&Path>) -> Result<()> {
        if self.container.opf_version_parsed()?.0 == 3 {
            self.update_modified_timestamp()?;
        }
        self.container.commit(false)?;
        let container_path = self.container.root.join("META-INF").join("container.xml");
        if !container_path.exists() {
            return Err(PolishError::InvalidBook(
                "No META-INF/container.xml in EPUB, this typically happens if the temporary \
                 files calibre is using are deleted by some other program while calibre is \
                 running"
                    .to_string(),
            )
            .into());
        }
        let mut restore_fonts = Vec::new();
        for (name, (alg, key)) in self.obfuscated_fonts.clone() {
            if !self.container.name_path_map.contains_key(&name) {
                continue;
            }
            let data = self.container.raw_data(&name, false)?;
            let obfuscated = decrypt_font_data(&key, &data, &alg);
            self.container.write_file(&name, &obfuscated)?;
            restore_fonts.push((name, data));
        }
        let outpath = outpath
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.path_to_epub.clone());
        self.commit_epub(&outpath)?;
        for (name, data) in restore_fonts {
            self.container.write_file(&name, &data)?;
        }
        Ok(())
    }

    /// Port of `commit_epub`.
    pub fn commit_epub(&mut self, outpath: &Path) -> Result<()> {
        if self.is_dir {
            sync_dir_tree(&self.container.root, &self.path_to_epub)
        } else {
            let root = self.container.root.clone();
            crate::zipfile_safe_replace::build_zip_atomic(outpath, |writer| {
                let mimetype_path = root.join("mimetype");
                fs::write(&mimetype_path, guess_type("a.epub").as_bytes())?;
                let opts = zip::write::FileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored);
                writer.start_file("mimetype", opts)?;
                writer.write_all(guess_type("a.epub").as_bytes())?;
                for entry in walkdir::WalkDir::new(&root)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel = entry.path().strip_prefix(&root).unwrap();
                    let zname: String = rel
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                        .nfc()
                        .collect();
                    if zname == "mimetype" {
                        continue;
                    }
                    let data = fs::read(entry.path())?;
                    let opts = zip::write::FileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated);
                    writer.start_file(&zname, opts)?;
                    writer.write_all(&data)?;
                }
                Ok(())
            })
        }
    }
}

/// Port of `walk_dir`+the copy loop inside `EpubContainer.commit_epub`'s
/// `is_dir` branch: mirrors the working directory `root` onto
/// `dest_dir` (removing files that no longer exist, copying everything
/// else).
fn sync_dir_tree(root: &Path, dest_dir: &Path) -> Result<()> {
    fs::create_dir_all(dest_dir)?;
    // Remove files from dest that no longer exist in root.
    for entry in walkdir::WalkDir::new(dest_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_name() == "mimetype" && entry.path().parent() == Some(dest_dir) {
            continue;
        }
        let rel = entry.path().strip_prefix(dest_dir).unwrap();
        if !root.join(rel).exists() {
            let _ = fs::remove_file(entry.path());
        }
    }
    // Copy everything from root to dest.
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let rel = entry.path().strip_prefix(root).unwrap();
        let dest_path = dest_dir.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest_path)?;
        } else {
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}

/// Is `node` a descendant of `ancestor` (matching Python's
/// `em.getparent().xpath('descendant::*[...]')`, which searches the
/// whole subtree, not just direct children)?
fn descends_from(xml: &Xml, node: XmlNodeId, ancestor: XmlNodeId) -> bool {
    let mut cur = xml.parent(node);
    while let Some(p) = cur {
        if p == ancestor {
            return true;
        }
        cur = xml.parent(p);
    }
    false
}

fn find_by_id_attr(xml: &Xml, id_value: &str) -> Option<XmlNodeId> {
    fn walk(xml: &Xml, node: XmlNodeId, id_value: &str) -> Option<XmlNodeId> {
        if xml.get_attr(node, "id") == Some(id_value) {
            return Some(node);
        }
        for &c in xml.children(node) {
            if let Some(found) = walk(xml, c, id_value) {
                return Some(found);
            }
        }
        None
    }
    walk(xml, xml.root, id_value)
}

fn normalize_tree_to_nfc(root: &Path) -> Result<()> {
    let entries: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .contents_first(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_path_buf())
        .collect();
    for path in entries {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let normalized: String = name.nfc().collect();
            if normalized != name {
                let new_path = path.with_file_name(normalized);
                fs::rename(&path, &new_path)?;
            }
        }
    }
    Ok(())
}

fn format_utc_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Minimal, dependency-free civil-from-days calculation (Howard
    // Hinnant's algorithm) -- avoids pulling `chrono`'s timezone
    // machinery in just for a UTC "now" stamp.
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days + 719468);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ===================================================================
// KEPUB
// ===================================================================

/// Port of `KEPUBContainer`. Kobo's `.kepub` format is an EPUB with
/// Kobo-specific markup injected on read and stripped on write
/// (`unkepubify_container`/`kepubify_container` in `kepubify.py`, one of
/// the ~23 not-yet-ported `polish` feature files -- out of scope for
/// this slice). The struct shape and delegation to `EpubContainer` are
/// real; the kepubify-dependent steps are `todo!()`.
pub struct KepubContainer {
    pub epub: EpubContainer,
}

impl std::ops::Deref for KepubContainer {
    type Target = EpubContainer;
    fn deref(&self) -> &EpubContainer {
        &self.epub
    }
}

impl std::ops::DerefMut for KepubContainer {
    fn deref_mut(&mut self) -> &mut EpubContainer {
        &mut self.epub
    }
}

impl KepubContainer {
    pub fn open_zip(path_to_kepub: &Path, tdir: &Path) -> Result<KepubContainer> {
        let _epub = EpubContainer::open_zip(path_to_kepub, tdir)?;
        todo!(
            "placeholder: KEPUBContainer::open_zip needs \
             oeb::polish::kepubify::unkepubify_container (kepubify.py is one \
             of the ~23 not-yet-ported polish feature files); EpubContainer \
             opening above this call is real"
        )
    }

    pub fn book_type(&self) -> &'static str {
        "kepub"
    }

    /// Port of `KEPUBContainer.commit_epub`.
    pub fn commit_epub(&mut self, outpath: &Path) -> Result<()> {
        if self.epub.is_dir {
            return self.epub.commit_epub(outpath);
        }
        todo!(
            "placeholder: non-directory KEPUBContainer::commit_epub needs \
             oeb::polish::kepubify::kepubify_container (not yet ported, see \
             open_zip's docs); the is_dir branch above delegates to the real \
             EpubContainer::commit_epub"
        )
    }
}

// ===================================================================
// AZW3
// ===================================================================

/// Port of `AZW3Container`. Converting between AZW3/MOBI and an exploded
/// OEB directory needs `Plumber`/`create_oebbook`/`opf_to_azw3`-style
/// wiring across `mobi::reader`/`mobi::writer2`/`mobi::writer8` (all
/// already ported, issues #32-#35) -- real, nontrivial *integration*
/// work connecting modules that exist but have never been wired this
/// way together (the same shape of gap as the open joint-MOBI6+KF8 issue
/// #157; this is a distinct gap, not a duplicate of it -- #157 is about
/// reading joint MOBI6+KF8 files, this is about the OPF<->AZW3 round
/// trip a polish `Container` needs). That wiring is out of scope for
/// this foundation slice; the struct shape is real so later work has
/// something to build on.
pub struct Azw3Container {
    pub container: Container,
    pub path_to_azw3: PathBuf,
    pub obfuscated_fonts: HashSet<String>,
}

impl std::ops::Deref for Azw3Container {
    type Target = Container;
    fn deref(&self) -> &Container {
        &self.container
    }
}

impl std::ops::DerefMut for Azw3Container {
    fn deref_mut(&mut self) -> &mut Container {
        &mut self.container
    }
}

impl Azw3Container {
    /// Port of `AZW3Container.__init__`'s cheap, dependency-free magic
    /// check (this part is real: Topaz files start with the literal
    /// bytes `TPZ`, no header parsing needed). Everything past that --
    /// `MetadataHeader` parsing, `kf8_type`/DRM detection, and exploding
    /// to an OEB directory via a forked `do_explode` worker
    /// (`mobi.reader.mobi6.MobiReader`/`mobi.reader.mobi8.Mobi8Reader`)
    /// -- needs the `Plumber`-style wiring described in this struct's
    /// docs and is `todo!()`.
    pub fn open(pathtoazw3: &Path, _tdir: &Path) -> Result<Azw3Container> {
        let mut header = [0u8; 3];
        {
            use std::io::Read;
            let mut f = File::open(pathtoazw3)
                .with_context(|| format!("Failed to open {}", pathtoazw3.display()))?;
            let n = f.read(&mut header).unwrap_or(0);
            if n == 3 && &header == b"TPZ" {
                return Err(PolishError::InvalidBook(
                    "This is not a MOBI file. It is a Topaz file.".to_string(),
                )
                .into());
            }
        }
        todo!(
            "placeholder: AZW3Container::open needs BookHeader parsing (kf8_type/ \
             encryption_type detection) and exploding the MOBI/AZW3 to an OEB \
             directory via mobi::reader::{{mobi6, mobi8}} -- see this struct's \
             docs for why this is real integration work, not a small gap. The \
             Topaz magic-byte check above is real."
        )
    }

    pub fn book_type(&self) -> &'static str {
        "azw3"
    }

    pub fn names_that_must_not_be_changed(&self) -> HashSet<String> {
        self.container.name_path_map.keys().cloned().collect()
    }

    /// Port of `AZW3Container.commit`: needs `opf_to_azw3` (Plumber +
    /// `mobi::writer2`/`writer8` wiring -- see this struct's docs).
    pub fn commit(&mut self, _outpath: Option<&Path>) -> Result<()> {
        self.container.commit(false)?;
        todo!(
            "placeholder: AZW3Container::commit needs opf_to_azw3 (Plumber + \
             mobi::writer2/writer8 wiring, see this struct's docs); the \
             generic Container::commit above (writing dirtied OPF/content \
             files back into the exploded working directory) is real"
        )
    }
}

/// Port of `get_container`: dispatches to the right container type by
/// file extension (or directory-ness). Returns a small enum since Rust
/// needs a single concrete return type; callers `match` on it (or use
/// `AnyContainer::as_container`/`as_container_mut` for the common,
/// format-independent operations, via `Deref`).
pub enum AnyContainer {
    Epub(EpubContainer),
    Kepub(KepubContainer),
    Azw3(Azw3Container),
}

impl AnyContainer {
    pub fn as_container(&self) -> &Container {
        match self {
            AnyContainer::Epub(c) => c,
            AnyContainer::Kepub(c) => c,
            AnyContainer::Azw3(c) => &c.container,
        }
    }

    pub fn as_container_mut(&mut self) -> &mut Container {
        match self {
            AnyContainer::Epub(c) => c,
            AnyContainer::Kepub(c) => c,
            AnyContainer::Azw3(c) => &mut c.container,
        }
    }
}

/// Port of `get_container`.
pub fn get_container(path: &Path, tdir: &Path, tweak_mode: bool) -> Result<AnyContainer> {
    let is_dir = path.is_dir();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut any = if !is_dir
        && matches!(
            ext.as_str(),
            "azw3" | "mobi" | "original_azw3" | "original_mobi"
        ) {
        AnyContainer::Azw3(Azw3Container::open(path, tdir)?)
    } else if !is_dir && matches!(ext.as_str(), "kepub" | "original_kepub") {
        AnyContainer::Kepub(KepubContainer::open_zip(path, tdir)?)
    } else if is_dir {
        AnyContainer::Epub(EpubContainer::open_dir(path, tdir)?)
    } else {
        AnyContainer::Epub(EpubContainer::open_zip(path, tdir)?)
    };
    any.as_container_mut().base.tweak_mode = tweak_mode;
    Ok(any)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_dir_book(dir: &Path) {
        fs::create_dir_all(dir.join("META-INF")).unwrap();
        fs::write(
            dir.join("META-INF/container.xml"),
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        fs::write(
            dir.join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Test Book</dc:title>
    <dc:language>en</dc:language>
    <dc:identifier id="bookid">urn:uuid:12345678-1234-1234-1234-123456789012</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
    <item id="c2" href="chap2.html" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.join("chap1.html"),
            b"<html><body><p>Chapter <a href=\"chap2.html\">1</a></p></body></html>",
        )
        .unwrap();
        fs::write(
            dir.join("chap2.html"),
            b"<html><body><p>Chapter 2</p></body></html>",
        )
        .unwrap();
        fs::write(dir.join("style.css"), b"body { color: black; }").unwrap();
    }

    fn write_epub_zip(path: &Path) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::FileOptions::default();
        zip.start_file("mimetype", opts).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();
        zip.start_file("META-INF/container.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#,
        )
        .unwrap();
        zip.start_file("content.opf", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="3.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Zip Book</dc:title>
    <dc:identifier id="bookid">urn:uuid:aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        zip.start_file("chap1.html", opts).unwrap();
        zip.write_all(b"<html><body><p>Hello</p></body></html>")
            .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn opens_a_plain_directory_book_and_reads_manifest_spine() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert_eq!(c.opf_name, "content.opf");
        let mmap = c.manifest_id_map().unwrap();
        assert_eq!(mmap.len(), 3);
        assert_eq!(mmap.get("c1").unwrap(), "chap1.html");
        let spine = c.spine_names().unwrap();
        assert_eq!(
            spine,
            vec![
                ("chap1.html".to_string(), true),
                ("chap2.html".to_string(), true)
            ]
        );
    }

    #[test]
    fn add_file_manifests_and_spines_a_new_xhtml_document() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let name = c
            .add_file(
                "chap3.html",
                b"<html><body>Three</body></html>",
                None,
                None,
                false,
            )
            .unwrap();
        assert_eq!(name, "chap3.html");
        assert!(c.manifest_has_name("chap3.html").unwrap());
        let spine = c.spine_names().unwrap();
        assert!(spine.iter().any(|(n, _)| n == "chap3.html"));
    }

    #[test]
    fn add_file_with_existing_name_modifies_when_allowed() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let name = c.add_file("chap1.html", b"data", None, None, true).unwrap();
        assert_ne!(name, "chap1.html");
        assert!(c.has_name(&name));
    }

    #[test]
    fn rename_moves_the_file_and_updates_container_bookkeeping() {
        // `rename` moves the file and updates the container's own
        // bookkeeping (`name_path_map`, `opf_name` if applicable), but
        // -- matching Python's documented behavior -- does *not* rewrite
        // the *other* files (including the OPF manifest) that link to
        // the renamed file; that's the caller's job, done in bulk via
        // `replace_links` for performance. See `Container::rename`'s
        // docs.
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.rename("chap2.html", "renamed.html").unwrap();
        assert!(!dir.path().join("chap2.html").exists());
        assert!(dir.path().join("renamed.html").exists());
        assert!(c.has_name("renamed.html"));
        assert!(!c.has_name("chap2.html"));
    }

    #[test]
    fn rename_across_directories_rebases_the_moved_files_own_links() {
        // Issue #161 left this case as a `todo!()` waiting on
        // `replace.py`'s `LinkRebaser` (issue #165). A file with a
        // relative link to a sibling moves into a subdirectory; its own
        // link must be rewritten to keep resolving to the same target
        // now that its base directory changed, even though `rename`
        // never touches *other* files' links (see the sibling test
        // above).
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("text")).unwrap();
        fs::write(
            dir.path().join("content.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" xmlns:dc="http://purl.org/dc/elements/1.1/" version="2.0" unique-identifier="bookid">
  <metadata>
    <dc:title>Cross Dir Book</dc:title>
    <dc:identifier id="bookid">x</dc:identifier>
  </metadata>
  <manifest>
    <item id="c1" href="text/index.html" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#,
        )
        .unwrap();
        fs::write(
            dir.path().join("text/index.html"),
            b"<html><head><link rel=\"stylesheet\" href=\"../style.css\"/></head>\
              <body><p>hi</p></body></html>",
        )
        .unwrap();
        fs::write(dir.path().join("style.css"), b"body{color:red}").unwrap();

        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.rename("text/index.html", "index.html").unwrap();

        assert!(c.has_name("index.html"));
        assert!(dir.path().join("index.html").exists());
        c.ensure_parsed("index.html").unwrap();
        let dom = c.get_xhtml("index.html").unwrap();
        let link = dom.find_first_tag_global("link").unwrap();
        // The file moved from `text/` to the root, so its relerative
        // link to the (unmoved) `style.css` must become `style.css`,
        // not the stale `../style.css`.
        assert_eq!(
            dom.node(link).attrs.get("href").map(|s| s.as_str()),
            Some("style.css")
        );
    }

    #[test]
    fn rename_of_opf_updates_opf_name() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.rename("content.opf", "package.opf").unwrap();
        assert_eq!(c.opf_name, "package.opf");
        assert!(dir.path().join("package.opf").exists());
    }

    #[test]
    fn remove_item_drops_manifest_spine_and_file() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.remove_item("chap2.html", true).unwrap();
        assert!(!dir.path().join("chap2.html").exists());
        assert!(!c.manifest_has_name("chap2.html").unwrap());
        let spine = c.spine_names().unwrap();
        assert!(!spine.iter().any(|(n, _)| n == "chap2.html"));
    }

    #[test]
    fn commit_writes_dirty_opf_back_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.add_name_to_manifest("style.css", "mystyle").unwrap();
        assert!(c.dirtied.contains("content.opf"));
        c.commit(false).unwrap();
        assert!(c.dirtied.is_empty());
        let mut reopened = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(reopened.manifest_has_name("style.css").unwrap());
    }

    #[test]
    fn round_trip_through_our_own_reader() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        c.remove_item("chap2.html", true).unwrap();
        c.commit(false).unwrap();
        let mut reopened = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert!(!reopened.manifest_has_name("chap2.html").unwrap());
        assert!(reopened.manifest_has_name("chap1.html").unwrap());
    }

    #[test]
    fn clone_to_produces_independent_container() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let dest = tempfile::tempdir().unwrap();
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let mut cloned = c.clone_to(dest.path()).unwrap();
        cloned.remove_item("chap2.html", true).unwrap();
        cloned.commit(false).unwrap();
        // Original is untouched.
        assert!(c.manifest_has_name("chap2.html").unwrap());
        assert!(dir.path().join("chap2.html").exists());
    }

    #[test]
    fn opens_a_zipped_epub_and_round_trips_commit() {
        let dir = tempfile::tempdir().unwrap();
        let epub_path = dir.path().join("book.epub");
        write_epub_zip(&epub_path);
        let tdir = tempfile::tempdir().unwrap();
        let mut epub = EpubContainer::open_zip(&epub_path, tdir.path()).unwrap();
        assert_eq!(epub.opf_name, "content.opf");
        assert!(epub.manifest_has_name("chap1.html").unwrap());

        let outpath = dir.path().join("out.epub");
        epub.commit(Some(&outpath)).unwrap();
        assert!(outpath.exists());

        let tdir2 = tempfile::tempdir().unwrap();
        let mut reopened = EpubContainer::open_zip(&outpath, tdir2.path()).unwrap();
        assert!(reopened.manifest_has_name("chap1.html").unwrap());
        assert_eq!(reopened.container.opf_version().unwrap(), "3.0");
    }

    #[test]
    fn epub_names_must_not_be_changed_includes_meta_inf() {
        let dir = tempfile::tempdir().unwrap();
        let epub_path = dir.path().join("book.epub");
        write_epub_zip(&epub_path);
        let tdir = tempfile::tempdir().unwrap();
        let epub = EpubContainer::open_zip(&epub_path, tdir.path()).unwrap();
        assert!(epub
            .names_that_must_not_be_changed()
            .contains("META-INF/container.xml"));
    }

    #[test]
    fn href_to_name_and_name_to_href_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let href = c.name_to_href("chap2.html", Some("content.opf"));
        let name = c.href_to_name(&href, Some("content.opf"));
        assert_eq!(name.as_deref(), Some("chap2.html"));
    }

    #[test]
    fn href_to_name_rejects_absolute_urls() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        assert_eq!(c.href_to_name("http://example.com/x.html", None), None);
        assert_eq!(c.href_to_name("mailto:a@b.com", None), None);
    }

    #[test]
    fn seconds_to_timestamp_formats_hms() {
        assert_eq!(seconds_to_timestamp(3661.0), "01:01:01");
    }

    #[test]
    fn replace_links_rewrites_xhtml_href_and_dirties() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let replaced = c
            .replace_links("chap1.html", |url, ft| {
                assert_eq!(ft, LinkFileType::Text);
                if url == "chap2.html" {
                    Some("renamed.html".to_string())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(replaced);
        assert!(c.dirtied.contains("chap1.html"));
        let dom = c.get_xhtml("chap1.html").unwrap();
        let a = dom.find_all_tag_global("a");
        assert_eq!(dom.node(a[0]).attrs.get("href").unwrap(), "renamed.html");
    }

    #[test]
    fn replace_links_rewrites_opf_hrefs() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let replaced = c
            .replace_links("content.opf", |url, ft| {
                assert_eq!(ft, LinkFileType::Opf);
                if url == "chap2.html" {
                    Some("renamed.html".to_string())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(replaced);
        let mmap = c.manifest_id_map().unwrap();
        assert_eq!(mmap.get("c2").unwrap(), "renamed.html");
    }

    #[test]
    fn iterlinks_finds_opf_and_xhtml_links() {
        let dir = tempfile::tempdir().unwrap();
        write_dir_book(dir.path());
        let mut c = Container::open(dir.path(), &dir.path().join("content.opf")).unwrap();
        let opf_links = c.iterlinks("content.opf").unwrap();
        assert!(opf_links.iter().any(|(u, _, _)| u == "chap1.html"));
        let html_links = c.iterlinks("chap1.html").unwrap();
        assert!(html_links.iter().any(|(u, _, _)| u == "chap2.html"));
    }
}
