//! Port of `old_src/src/calibre/ebooks/oeb/polish/replace.py`.
//!
//! `LinkReplacer`/`IdReplacer`/`LinkRebaser` are the callables Python
//! hands to `Container.replace_links` (already real, see
//! [`super::container::Container::replace_links`]'s docs) to rewrite
//! every link-bearing attribute in a document. This file's `LinkRebaser`
//! is also what closes `Container::rename`'s cross-directory-rename
//! `todo!()` from issue #161 -- see that method's updated docs.
//!
//! # Design note: no borrowed `&Container` in the replacer structs
//!
//! Python's `LinkReplacer.__init__(self, base, container, link_map,
//! frag_map)` stores a live `container` reference and calls
//! `self.container.href_to_name`/`.name_to_href` from inside `__call__`.
//! In Rust, these structs are used *as* the `replace_func` closure
//! passed to [`super::container::Container::replace_links`], which
//! already holds `&mut Container` for the whole call -- a second,
//! simultaneous `&Container` borrow inside the closure would not
//! typecheck. `href_to_name`/`name_to_href` only ever need
//! `Container::root` (see `container.rs`'s free
//! `href_to_name_at`/`name_to_href_at` functions, which already take
//! `root: &Path` instead of `&Container`), so every struct here just
//! clones `root` once at construction instead of borrowing the whole
//! container. This sidesteps the aliasing conflict entirely and needs
//! no `RefCell`/interior mutability.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use indexmap::IndexMap;

use crate::css::{Rule, StyleDeclarationBlock, Stylesheet};
use crate::dom::NodeId;
use crate::oeb::constants::{OEB_DOCS, OEB_STYLES};
use crate::oeb::polish::css::remove_property_value;
use crate::oeb::polish::fonts::set_dom_element_text_only;
use crate::oeb::polish::utils::{extract, guess_type, OEB_FONTS};

use super::container::{href_to_name_at, name_to_href_at, Container};

/// Splits `url` at its first `#`, matching `href.partition('#')`'s
/// `(before, frag)` shape (`frag` is `None` when there is no `#`, unlike
/// Python's `partition` which always returns a string -- callers that
/// need "always a string, possibly empty" use `.unwrap_or("")`).
fn split_frag(url: &str) -> (&str, Option<&str>) {
    match url.split_once('#') {
        Some((before, frag)) => (before, Some(frag)),
        None => (url, None),
    }
}

// ===================================================================
// LinkReplacer / IdReplacer / LinkRebaser
// ===================================================================

/// Port of `LinkReplacer`. Constructed fresh per document (`base`) by
/// [`replace_links`]; used as the `replace_func` of
/// [`Container::replace_links`].
pub struct LinkReplacer<'a> {
    base: String,
    root: PathBuf,
    link_map: &'a HashMap<String, String>,
    frag_map: &'a dyn Fn(&str, &str) -> String,
    pub replaced: bool,
}

impl<'a> LinkReplacer<'a> {
    pub fn new(
        container: &Container,
        base: &str,
        link_map: &'a HashMap<String, String>,
        frag_map: &'a dyn Fn(&str, &str) -> String,
    ) -> Self {
        LinkReplacer {
            base: base.to_string(),
            root: container.root.clone(),
            link_map,
            frag_map,
            replaced: false,
        }
    }

    /// Port of `LinkReplacer.__call__`. Returns `Some(new_url)` when the
    /// link actually changed, `None` to leave it untouched -- the shape
    /// [`Container::replace_links`] expects (unlike Python, which always
    /// returns a string and tracks `.replaced` separately; folding "no
    /// change" into `None` keeps both in sync for free).
    pub fn replace(&mut self, url: &str) -> Option<String> {
        if let Some(rest) = url.strip_prefix('#') {
            let repl = (self.frag_map)(&self.base, rest);
            if repl.is_empty() || repl == rest {
                return None;
            }
            self.replaced = true;
            return Some(format!("#{repl}"));
        }
        let name = href_to_name_at(url, &self.root, Some(&self.base))?;
        let nname = self.link_map.get(&name)?;
        let (_, frag) = split_frag(url);
        let mut href = name_to_href_at(nname, &self.root, Some(&self.base));
        if let Some(frag) = frag {
            if !frag.is_empty() {
                let nfrag = (self.frag_map)(&name, frag);
                if !nfrag.is_empty() {
                    href.push('#');
                    href.push_str(&nfrag);
                }
            }
        }
        if href != url {
            self.replaced = true;
            Some(href)
        } else {
            None
        }
    }
}

/// Port of `IdReplacer`.
pub struct IdReplacer<'a> {
    base: String,
    root: PathBuf,
    id_map: &'a HashMap<String, HashMap<String, String>>,
    pub replaced: bool,
}

impl<'a> IdReplacer<'a> {
    pub fn new(
        container: &Container,
        base: &str,
        id_map: &'a HashMap<String, HashMap<String, String>>,
    ) -> Self {
        IdReplacer {
            base: base.to_string(),
            root: container.root.clone(),
            id_map,
            replaced: false,
        }
    }

    /// Port of `IdReplacer.__call__`.
    pub fn replace(&mut self, url: &str) -> Option<String> {
        if let Some(rest) = url.strip_prefix('#') {
            let repl = self.id_map.get(&self.base).and_then(|m| m.get(rest))?;
            if repl == rest {
                return None;
            }
            self.replaced = true;
            return Some(format!("#{repl}"));
        }
        let name = href_to_name_at(url, &self.root, Some(&self.base))?;
        let map = self.id_map.get(&name)?;
        let (before, frag) = split_frag(url);
        let frag = frag.unwrap_or("");
        let nfrag = map.get(frag)?;
        let new_url = format!("{before}#{nfrag}");
        if new_url != url {
            self.replaced = true;
            Some(new_url)
        } else {
            None
        }
    }
}

/// Port of `LinkRebaser`: rewrites the links *inside* a single document
/// (`new_name`) to keep pointing at the same targets after the document
/// itself moved from `old_name` to `new_name` (which may be in a
/// different directory, changing every relative link's resolution).
/// This is what [`Container::rename`] uses to close its
/// cross-directory-rename gap -- see that method's docs.
pub struct LinkRebaser {
    root: PathBuf,
    old_name: String,
    new_name: String,
    pub replaced: bool,
}

impl LinkRebaser {
    pub fn new(container: &Container, old_name: &str, new_name: &str) -> Self {
        LinkRebaser {
            root: container.root.clone(),
            old_name: old_name.to_string(),
            new_name: new_name.to_string(),
            replaced: false,
        }
    }

    /// Port of `LinkRebaser.__call__`.
    pub fn rebase(&mut self, url: &str) -> Option<String> {
        if url.is_empty() || url.starts_with('#') {
            return None;
        }
        let (_, frag) = split_frag(url);
        let name = href_to_name_at(url, &self.root, Some(&self.old_name))?;
        let name = if name == self.old_name {
            self.new_name.clone()
        } else {
            name
        };
        let mut href = name_to_href_at(&name, &self.root, Some(&self.new_name));
        if let Some(frag) = frag {
            href.push('#');
            href.push_str(frag);
        }
        if href != url {
            self.replaced = true;
            Some(href)
        } else {
            None
        }
    }
}

/// Port of `replace_links`. Rewrites every link in the container that
/// appears as a key in `link_map`, using `frag_map(name, frag) -> new_frag`
/// to remap anchors (an empty return means "no fragment"). Pass
/// `|_n, f| f.to_string()` for Python's default identity `frag_map`.
pub fn replace_links(
    container: &mut Container,
    link_map: &HashMap<String, String>,
    frag_map: &dyn Fn(&str, &str) -> String,
    replace_in_opf: bool,
) -> Result<()> {
    let names: Vec<String> = container.base.mime_map.keys().cloned().collect();
    let opf_name = container.opf_name.clone();
    for name in names {
        if name == opf_name && !replace_in_opf {
            continue;
        }
        let mut repl = LinkReplacer::new(container, &name, link_map, frag_map);
        container.replace_links(&name, |url, _ft| repl.replace(url))?;
    }
    Ok(())
}

/// Port of `replace_ids`. Returns whether at least one link was changed.
pub fn replace_ids(
    container: &mut Container,
    id_map: &HashMap<String, HashMap<String, String>>,
) -> Result<bool> {
    let mut changed = false;
    let names: Vec<String> = container.base.mime_map.keys().cloned().collect();
    let opf_name = container.opf_name.clone();
    for name in names {
        let mut repl = IdReplacer::new(container, &name, id_map);
        container.replace_links(&name, |url, _ft| repl.replace(url))?;
        if name == opf_name {
            if let Some(imap) = id_map.get(&name) {
                let items = container.opf_xpath("//*[@idref]")?;
                for item in items {
                    let old_id = {
                        let xml = container.get_xml(&opf_name)?;
                        xml.get_attr(item, "idref").map(|s| s.to_string())
                    };
                    if let Some(old_id) = old_id {
                        if let Some(new_id) = imap.get(&old_id) {
                            let new_id = new_id.clone();
                            let xml = container.get_xml_mut(&opf_name)?;
                            xml.set_attr(item, "idref", new_id);
                        }
                    }
                }
            }
        }
        if repl.replaced {
            changed = true;
        }
    }
    Ok(changed)
}

// ===================================================================
// Punctuation smartening
// ===================================================================

/// Port of `smarten_punctuation`. See
/// [`super::smartypants::smarten_punctuation_html`]'s docs for the
/// quote-educating fidelity tradeoff versus Python's
/// `calibre.ebooks.conversion.preprocess.smarten_punctuation` (itself a
/// thin wrapper -- `<!--`/`-->` guard + `smartyPants` + entity
/// re-encoding -- around `calibre.utils.smartypants.smartyPants`).
pub fn smarten_punctuation(
    container: &mut Container,
    mut report: impl FnMut(&str),
) -> Result<bool> {
    let mut smartened = false;
    let spine_names: Vec<String> = container
        .spine_names()?
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    for name in spine_names {
        let data = container.read_file(&name)?;
        let html = container.base.decode(&data, true);
        let newhtml = smarten_html(&html);
        let mut changed = false;
        if newhtml != html {
            changed = true;
            report(&format!("Smartened punctuation in: {name}"));
            let stripped =
                crate::chardet::strip_encoding_declarations(newhtml.as_bytes(), 50 * 1024, false);
            let mut out = vec![0xEFu8, 0xBB, 0xBF];
            out.extend_from_slice(&stripped);
            container.write_file(&name, &out)?;
        }
        if changed {
            // Add an encoding declaration (added automatically on
            // serialize); strip any stale `<meta http-equiv>` first.
            container.ensure_parsed(&name)?;
            let metas: Vec<NodeId> = {
                let dom = container.get_xhtml(&name)?;
                dom.preorder_elements(dom.root)
                    .into_iter()
                    .filter(|&id| {
                        dom.tag(id) == Some("meta") && dom.node(id).attrs.contains_key("http-equiv")
                    })
                    .collect()
            };
            if !metas.is_empty() {
                let dom = container.get_xhtml_mut(&name)?;
                for m in metas {
                    dom.detach(m);
                }
            }
            container.dirty(&name);
            smartened = true;
        }
    }
    if !smartened {
        report("No punctuation that could be smartened found");
    }
    Ok(smartened)
}

/// Port of `calibre.ebooks.conversion.preprocess.smarten_punctuation`'s
/// body: protect HTML comments from the quote-educator by swapping their
/// delimiters for unlikely-to-collide placeholders, run the educator,
/// then swap them back and decode any entities introduced by prior
/// passes over the (already-decoded) text.
fn smarten_html(html: &str) -> String {
    let start = "calibre-smartypants-start-guard";
    let stop = "calibre-smartypants-stop-guard";
    let guarded = html.replace("<!--", start).replace("-->", stop);
    let educated = super::smartypants::smarten_punctuation_html(&guarded);
    let restored = educated.replace(start, "<!--").replace(stop, "-->");
    crate::html_entities::xml_replace_entities(&restored)
}

// ===================================================================
// File organization
// ===================================================================

/// Port of `rename_files`.
pub fn rename_files(container: &mut Container, file_map: &HashMap<String, String>) -> Result<()> {
    let dests: HashSet<&String> = file_map.values().collect();
    let overlap: Vec<&String> = file_map.keys().filter(|k| dests.contains(k)).collect();
    if !overlap.is_empty() {
        bail!(
            "Circular rename detected. The files {} are both rename targets and destinations",
            overlap
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for (name, dest) in file_map {
        if container.exists(dest) {
            if name != dest && name.to_lowercase() == dest.to_lowercase() {
                continue;
            }
            bail!("Cannot rename {name} to {dest} as {dest} already exists");
        }
    }
    let unique_dests: HashSet<&String> = file_map.values().collect();
    if unique_dests.len() != file_map.len() {
        bail!("Cannot rename, the set of destination files contains duplicates");
    }
    let mut link_map = HashMap::new();
    for (current_name, new_name) in file_map {
        container.rename(current_name, new_name)?;
        if new_name != &container.opf_name {
            link_map.insert(current_name.clone(), new_name.clone());
        }
    }
    replace_links(container, &link_map, &|_n, f| f.to_string(), true)
}

/// Port of `replace_file`: overwrites the container item `name` with the
/// bytes at the external filesystem path `path`, renamed to `basename`
/// (sanitized) if it differs from `name`'s own basename.
pub fn replace_file(
    container: &mut Container,
    name: &str,
    path: &Path,
    basename: &str,
    force_mt: Option<&str>,
) -> Result<()> {
    let (dirname, _base) = match name.rsplit_once('/') {
        Some((d, b)) => (d.to_string(), b.to_string()),
        None => (String::new(), name.to_string()),
    };
    let mut nname = calibre_utils::filenames::sanitize_file_name(basename);
    if !dirname.is_empty() {
        nname = format!("{dirname}/{nname}");
    }
    let data = std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    if name != nname {
        let mut count = 0u32;
        let (b, e) = rpartition_dot(&nname);
        while container.exists(&nname) {
            count += 1;
            nname = format!("{b}_{count}.{e}");
        }
        let mut file_map = HashMap::new();
        file_map.insert(name.to_string(), nname.clone());
        rename_files(container, &file_map)?;
        let mt = match force_mt {
            Some(mt) => mt.to_string(),
            None => container.guess_type(&nname),
        };
        container.base.mime_map.insert(nname.clone(), mt.clone());
        let id_map = container.manifest_id_map()?;
        let matching_ids: Vec<String> = id_map
            .iter()
            .filter(|(_, n)| **n == nname)
            .map(|(id, _)| id.clone())
            .collect();
        if !matching_ids.is_empty() {
            let items = container.opf_xpath("//opf:manifest/opf:item[@href and @id]")?;
            let opf_name = container.opf_name.clone();
            let matches: Vec<_> = {
                let xml = container.get_xml(&opf_name)?;
                items
                    .into_iter()
                    .filter(|&item| {
                        xml.get_attr(item, "id")
                            .map(|id| matching_ids.iter().any(|m| m == id))
                            .unwrap_or(false)
                    })
                    .collect()
            };
            let xml = container.get_xml_mut(&opf_name)?;
            for item in matches {
                xml.set_attr(item, "media-type", mt.clone());
            }
        }
    }
    let opf_name = container.opf_name.clone();
    container.dirty(&opf_name);
    container.write_file(&nname, &data)?;
    Ok(())
}

/// Port of `mt_to_category`.
pub fn mt_to_category(_container: &Container, mt: &str) -> String {
    if OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(mt)) {
        "text".to_string()
    } else if OEB_STYLES.iter().any(|m| m.eq_ignore_ascii_case(mt)) {
        "style".to_string()
    } else if OEB_FONTS.iter().any(|m| m.eq_ignore_ascii_case(mt)) {
        "font".to_string()
    } else if mt.eq_ignore_ascii_case(&guess_type("a.opf")) {
        "opf".to_string()
    } else if mt.eq_ignore_ascii_case(&guess_type("a.ncx")) {
        "toc".to_string()
    } else {
        mt.split('/').next().unwrap_or(mt).to_string()
    }
}

/// Picks the most-common key in `counter`, first-inserted wins on ties
/// (matches Python `Counter.most_common(1)`'s stable ordering).
fn most_common(counter: &IndexMap<String, u32>) -> Option<String> {
    let mut best: Option<(&String, u32)> = None;
    for (k, &v) in counter {
        if best.map(|(_, bv)| v > bv).unwrap_or(true) {
            best = Some((k, v));
        }
    }
    best.map(|(k, _)| k.clone())
}

fn basename(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

/// Port of `get_recommended_folders`.
pub fn get_recommended_folders(container: &Container, names: &[String]) -> HashMap<String, String> {
    let mut counts: IndexMap<String, IndexMap<String, u32>> = IndexMap::new();
    for (name, mt) in &container.base.mime_map {
        let folder = match name.rsplit_once('/') {
            Some((d, _)) => d.to_string(),
            None => String::new(),
        };
        let category = mt_to_category(container, mt);
        *counts
            .entry(category)
            .or_default()
            .entry(folder)
            .or_insert(0) += 1;
    }
    let opf_folder = counts.get("opf").and_then(most_common).unwrap_or_default();
    let recommendations: HashMap<String, String> = counts
        .iter()
        .filter_map(|(cat, counter)| most_common(counter).map(|f| (cat.clone(), f)))
        .collect();
    names
        .iter()
        .map(|n| {
            let mt = guess_type(basename(n));
            let category = mt_to_category(container, &mt);
            let folder = recommendations
                .get(&category)
                .cloned()
                .unwrap_or_else(|| opf_folder.clone());
            (n.clone(), folder)
        })
        .collect()
}

/// Port of `normalize_case`.
pub fn normalize_case(container: &Container, val: &str) -> String {
    let parts: Vec<&str> = val.split('/').collect();
    let mut ans: Vec<String> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let q = parts[..=i].join("/");
        let x = container.name_to_abspath(&q);
        let xl = part.to_lowercase();
        let candidate = x
            .parent()
            .and_then(|dir| std::fs::read_dir(dir).ok())
            .and_then(|entries| {
                entries.flatten().find_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    if n != *part && n.to_lowercase() == xl {
                        Some(n)
                    } else {
                        None
                    }
                })
            });
        ans.push(candidate.unwrap_or_else(|| (*part).to_string()));
    }
    ans.join("/")
}

fn posix_join(a: &str, b: &str) -> String {
    if a.is_empty() {
        b.to_string()
    } else if a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// Port of `str.rpartition('.')`'s `[0::2]` (head, tail) projection:
/// `("a.b.c", '.')` -> `("a.b", "c")`; when the separator is absent,
/// Python's `rpartition` returns `('', '', s)`, i.e. an empty head and
/// the whole string as tail.
fn rpartition_dot(s: &str) -> (String, String) {
    match s.rfind('.') {
        Some(idx) => (s[..idx].to_string(), s[idx + 1..].to_string()),
        None => (String::new(), s.to_string()),
    }
}

/// Port of `rationalize_folders`. `folder_type_map` is normalized
/// in-place (its values are passed through [`normalize_case`]), matching
/// Python's mutation of the caller's dict.
pub fn rationalize_folders(
    container: &Container,
    folder_type_map: &mut HashMap<String, String>,
) -> HashMap<String, String> {
    for val in folder_type_map.values_mut() {
        *val = normalize_case(container, val);
    }
    let all_names: HashSet<&String> = container.base.mime_map.keys().collect();
    let mut new_names: HashSet<String> = HashSet::new();
    let mut name_map = HashMap::new();
    for name in &all_names {
        if name.starts_with("META-INF/") {
            continue;
        }
        let mt = &container.base.mime_map[*name];
        let category = mt_to_category(container, mt);
        let Some(folder) = folder_type_map.get(&category) else {
            continue;
        };
        let bn = basename(name);
        let mut new_name = posix_join(folder, bn);
        if &new_name != *name {
            let mut c = 0u32;
            while all_names.contains(&new_name) || new_names.contains(&new_name) {
                c += 1;
                let (n, ext) = rpartition_dot(bn);
                new_name = posix_join(folder, &format!("{n}_{c}.{ext}"));
            }
            name_map.insert((*name).clone(), new_name.clone());
            new_names.insert(new_name);
        }
    }
    name_map
}

// ===================================================================
// CSS link removal
// ===================================================================

fn extract_url(token: &str) -> Option<&str> {
    let t = token.trim();
    if t.len() < 5 || !t[..4].eq_ignore_ascii_case("url(") || !t.ends_with(')') {
        return None;
    }
    let inner = t[4..t.len() - 1].trim();
    Some(inner.trim_matches(|c| c == '"' || c == '\''))
}

/// Mutable counterpart to [`crate::oeb::polish::css::iter_declarations`]
/// (which is read-only by design -- see that function's docs): every
/// declaration block reachable from `rules` (style + `@font-face` rules,
/// recursing into `@media`), needed here because
/// [`remove_links_in_sheet`] must actually edit each one.
fn iter_declarations_mut(rules: &mut [Rule]) -> Vec<&mut StyleDeclarationBlock> {
    let mut out = Vec::new();
    for rule in rules {
        match rule {
            Rule::Style(sr) => out.push(&mut sr.style),
            Rule::FontFace(d) => out.push(d),
            Rule::Media(m) => out.extend(iter_declarations_mut(&mut m.rules)),
            _ => {}
        }
    }
    out
}

/// Port of `remove_links_in_declaration`: removes every `url(...)` value
/// token (across every property in `style`) for which `predicate`
/// returns true.
pub fn remove_links_in_declaration(
    href_to_name: &impl Fn(&str) -> Option<String>,
    style: &mut StyleDeclarationBlock,
    predicate: &impl Fn(Option<&str>, &str, Option<&str>) -> bool,
) -> bool {
    let names: Vec<String> = {
        let mut seen = HashSet::new();
        style
            .properties
            .iter()
            .filter(|d| seen.insert(d.name.to_ascii_lowercase()))
            .map(|d| d.name.clone())
            .collect()
    };
    let mut changed = false;
    for name in names {
        changed |= remove_property_value(style, &name, |token| match extract_url(token) {
            Some(uri) => {
                let hname = href_to_name(uri);
                predicate(hname.as_deref(), uri, None)
            }
            None => false,
        });
    }
    changed
}

/// Port of `remove_links_in_sheet`.
pub fn remove_links_in_sheet(
    href_to_name: &impl Fn(&str) -> Option<String>,
    sheet: &mut Stylesheet,
    predicate: &impl Fn(Option<&str>, &str, Option<&str>) -> bool,
) -> bool {
    let mut changed = false;
    let mut remove_idxs = Vec::new();
    for (i, rule) in sheet.rules.iter().enumerate() {
        if let Rule::Import(imp) = rule {
            let hname = href_to_name(&imp.href);
            if predicate(hname.as_deref(), &imp.href, None) {
                remove_idxs.push(i);
            }
        }
    }
    for i in remove_idxs.into_iter().rev() {
        sheet.rules.remove(i);
        changed = true;
    }
    for dec in iter_declarations_mut(&mut sheet.rules) {
        changed = remove_links_in_declaration(href_to_name, dec, predicate) || changed;
    }
    changed
}

/// Port of `remove_links_to`. `predicate` must return true iff the link
/// (`name` -- `None` if unresolvable, `href`, `fragment`) should be
/// removed. See the module docs for why link-bearing attributes are
/// limited to `href`/`src` (the same bounded subset
/// [`Container::replace_links`]/`iterlinks` already use, rather than
/// `oeb.base.iterlinks`'s full attribute table).
pub fn remove_links_to(
    container: &mut Container,
    predicate: &impl Fn(Option<&str>, &str, Option<&str>) -> bool,
) -> Result<HashSet<String>> {
    let root_path = container.root.clone();
    let entries: Vec<(String, String)> = container
        .base
        .mime_map
        .iter()
        .map(|(n, mt)| (n.clone(), mt.clone()))
        .collect();
    let mut changed = HashSet::new();
    for (name, mt) in entries {
        let mut removed = false;
        let href_to_name = |href: &str| href_to_name_at(href, &root_path, Some(&name));
        if OEB_DOCS.iter().any(|m| m.eq_ignore_ascii_case(&mt)) {
            container.ensure_parsed(&name)?;

            let candidates: Vec<(NodeId, &'static str, String)> = {
                let dom = container.get_xhtml(&name)?;
                let mut v = Vec::new();
                for id in dom.preorder_elements(dom.root) {
                    for attr in ["href", "src"] {
                        if let Some(val) = dom.node(id).attrs.get(attr) {
                            v.push((id, attr, val.clone()));
                        }
                    }
                }
                v
            };
            let mut to_extract = Vec::new();
            let mut to_unset: Vec<(NodeId, &'static str)> = Vec::new();
            for (id, attr, href) in &candidates {
                let hname = href_to_name(href);
                let frag = href.split_once('#').map(|(_, f)| f).unwrap_or("");
                if predicate(hname.as_deref(), href, Some(frag)) {
                    let tag = {
                        let dom = container.get_xhtml(&name)?;
                        dom.tag(*id).map(|s| s.to_string())
                    };
                    if tag.as_deref() == Some("link") || tag.as_deref() == Some("img") {
                        to_extract.push(*id);
                    } else {
                        to_unset.push((*id, attr));
                    }
                    removed = true;
                }
            }
            if removed {
                let dom = container.get_xhtml_mut(&name)?;
                for id in to_extract {
                    extract(dom, id);
                }
                for (id, attr) in to_unset {
                    dom.node_mut(id).attrs.shift_remove(attr);
                }
            }

            let style_tags: Vec<(NodeId, String)> = {
                let dom = container.get_xhtml(&name)?;
                dom.find_all_tag_global("style")
                    .into_iter()
                    .filter(|&id| {
                        dom.node(id)
                            .attrs
                            .get("type")
                            .map(|t| t.to_lowercase())
                            .unwrap_or_else(|| "text/css".to_string())
                            == "text/css"
                    })
                    .map(|id| (id, dom.text_content(id)))
                    .filter(|(_, t)| !t.is_empty())
                    .collect()
            };
            let mut style_updates = Vec::new();
            for (id, text) in style_tags {
                let mut sheet = Stylesheet::parse(&text);
                if remove_links_in_sheet(&href_to_name, &mut sheet, predicate) {
                    style_updates.push((id, sheet.to_css_text()));
                    removed = true;
                }
            }
            if !style_updates.is_empty() {
                let dom = container.get_xhtml_mut(&name)?;
                for (id, text) in style_updates {
                    set_dom_element_text_only(dom, id, &text);
                }
            }

            let style_attrs: Vec<(NodeId, String)> = {
                let dom = container.get_xhtml(&name)?;
                dom.preorder_elements(dom.root)
                    .into_iter()
                    .filter_map(|id| dom.node(id).attrs.get("style").map(|s| (id, s.clone())))
                    .filter(|(_, s)| !s.is_empty())
                    .collect()
            };
            let mut attr_updates = Vec::new();
            for (id, text) in style_attrs {
                let mut decl = crate::css::parser::parse_declaration_list(&text);
                if remove_links_in_declaration(&href_to_name, &mut decl, predicate) {
                    attr_updates.push((id, decl.to_css_text(" ")));
                    removed = true;
                }
            }
            if !attr_updates.is_empty() {
                let dom = container.get_xhtml_mut(&name)?;
                for (id, text) in attr_updates {
                    dom.node_mut(id).attrs.insert("style".to_string(), text);
                }
            }
        } else if OEB_STYLES.iter().any(|m| m.eq_ignore_ascii_case(&mt)) {
            let mut sheet = container.parsed_stylesheet(&name)?;
            if remove_links_in_sheet(&href_to_name, &mut sheet, predicate) {
                container.set_css_text(&name, sheet.to_css_text());
                removed = true;
            }
        }
        if removed {
            changed.insert(name);
        }
    }
    for n in &changed {
        container.dirty(n);
    }
    Ok(changed)
}

/// Port of `get_spine_order_for_all_files`. Unlike Python, an
/// unresolvable link target (`container.href_to_name` returning `None`,
/// e.g. an external URL) is skipped rather than inserted under a `None`
/// key -- Python's `dict` can hold a `None` key, which
/// `HashMap<String, _>` structurally cannot, and no real caller could
/// productively look one up anyway.
pub fn get_spine_order_for_all_files(
    container: &mut Container,
) -> Result<HashMap<String, (usize, i64)>> {
    let spine_names = container.spine_names()?;
    let mut linear = Vec::new();
    let mut non_linear = Vec::new();
    for (name, is_linear) in spine_names {
        if is_linear {
            linear.push(name);
        } else {
            non_linear.push(name);
        }
    }
    linear.extend(non_linear);
    let spine_set: HashSet<&String> = linear.iter().collect();
    let mut ans: HashMap<String, (usize, i64)> = HashMap::new();
    for (spine_pos, name) in linear.iter().enumerate() {
        ans.entry(name.clone()).or_insert((spine_pos, -1));
        let links = container.iterlinks(name)?;
        for (i, (href, _line, _off)) in links.into_iter().enumerate() {
            let Some(lname) = container.href_to_name(&href, Some(name)) else {
                continue;
            };
            if !spine_set.contains(&lname) {
                ans.entry(lname).or_insert((spine_pos, i as i64));
            }
        }
    }
    Ok(ans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(
        files: &[(&str, &str, &[u8])],
        spine: &[&str],
    ) -> (tempfile::TempDir, Container) {
        let dir = tempfile::tempdir().unwrap();
        let opf_path = dir.path().join("content.opf");
        let mut manifest_items = String::new();
        for (name, mt, content) in files {
            if let Some(parent) = std::path::Path::new(name).parent() {
                if !parent.as_os_str().is_empty() {
                    fs::create_dir_all(dir.path().join(parent)).unwrap();
                }
            }
            fs::write(dir.path().join(name), content).unwrap();
            manifest_items.push_str(&format!(
                r#"<item id="{name}" href="{name}" media-type="{mt}"/>"#
            ));
        }
        let spine_items: String = spine
            .iter()
            .map(|n| format!(r#"<itemref idref="{n}"/>"#))
            .collect();
        let opf = format!(
            r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>T</dc:title><dc:identifier id="bookid">x</dc:identifier></metadata>
  <manifest>{manifest_items}</manifest>
  <spine>{spine_items}</spine>
</package>"#
        );
        fs::write(&opf_path, opf).unwrap();
        let container = Container::open(dir.path(), &opf_path).unwrap();
        (dir, container)
    }

    #[test]
    fn link_replacer_rewrites_mapped_link() {
        let (_dir, container) = make_container(
            &[
                ("a.html", "application/xhtml+xml", b"x"),
                ("b.html", "application/xhtml+xml", b"x"),
            ],
            &[],
        );
        let mut link_map = HashMap::new();
        link_map.insert("b.html".to_string(), "c.html".to_string());
        let frag_map = |_n: &str, f: &str| f.to_string();
        let mut repl = LinkReplacer::new(&container, "a.html", &link_map, &frag_map);
        assert_eq!(repl.replace("b.html"), Some("c.html".to_string()));
        assert!(repl.replaced);
        assert_eq!(repl.replace("unrelated.html"), None);
    }

    #[test]
    fn rename_files_updates_links_across_directories() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "text/index.html",
                    "application/xhtml+xml",
                    b"<html><body><a href=\"../images/cover.jpg\">x</a></body></html>",
                ),
                ("images/cover.jpg", "image/jpeg", b"fake"),
            ],
            &["text/index.html"],
        );
        let mut file_map = HashMap::new();
        file_map.insert(
            "images/cover.jpg".to_string(),
            "assets/cover.jpg".to_string(),
        );
        rename_files(&mut container, &file_map).unwrap();
        assert!(container.exists("assets/cover.jpg"));
        assert!(!container.exists("images/cover.jpg"));
        container.ensure_parsed("text/index.html").unwrap();
        let dom = container.get_xhtml("text/index.html").unwrap();
        let a = dom.find_first_tag_global("a").unwrap();
        assert_eq!(
            dom.node(a).attrs.get("href").map(|s| s.as_str()),
            Some("../assets/cover.jpg")
        );
    }

    #[test]
    fn mt_to_category_classifies_known_types() {
        let (_dir, container) = make_container(&[], &[]);
        assert_eq!(mt_to_category(&container, "application/xhtml+xml"), "text");
        assert_eq!(mt_to_category(&container, "text/css"), "style");
        assert_eq!(mt_to_category(&container, "font/ttf"), "font");
        assert_eq!(mt_to_category(&container, "image/jpeg"), "image");
    }

    #[test]
    fn rationalize_folders_moves_mismatched_categories() {
        let (_dir, container) =
            make_container(&[("weird/place/a.css", "text/css", b".a{color:red}")], &[]);
        let mut map = HashMap::new();
        map.insert("style".to_string(), "styles".to_string());
        let name_map = rationalize_folders(&container, &mut map);
        assert_eq!(
            name_map.get("weird/place/a.css").map(|s| s.as_str()),
            Some("styles/a.css")
        );
    }

    #[test]
    fn remove_links_to_strips_matching_image_and_css_url() {
        let (_dir, mut container) = make_container(
            &[
                (
                    "index.html",
                    "application/xhtml+xml",
                    b"<html><head><style>.a{background:url(bad.png)}</style></head>\
                      <body><img src=\"bad.png\"/><a href=\"good.html\">x</a></body></html>",
                ),
                ("bad.png", "image/png", b"x"),
                (
                    "good.html",
                    "application/xhtml+xml",
                    b"<html><body/></html>",
                ),
            ],
            &[],
        );
        let changed = remove_links_to(&mut container, &|name: Option<&str>, _href, _frag| {
            name == Some("bad.png")
        })
        .unwrap();
        assert!(changed.contains("index.html"));
        let dom = container.get_xhtml("index.html").unwrap();
        assert!(dom.find_first_tag_global("img").is_none());
        let style = dom.find_first_tag_global("style").unwrap();
        assert!(!dom.text_content(style).contains("bad.png"));
        assert!(dom.find_first_tag_global("a").is_some());
    }
}
