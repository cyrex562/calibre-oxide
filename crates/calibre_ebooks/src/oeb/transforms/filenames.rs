//! Port of `old_src/src/calibre/ebooks/oeb/transforms/filenames.py`.
//!
//! Rename manifest items and adjust the links that point at them.
//!
//! # OEB-model simplification
//!
//! Python's `Manifest.remove()` also drops the item from `oeb.spine` (it
//! is keyed by object identity), so `UniqueFilenames`/`FlatFilenames`
//! must capture `item.spine_position` before removing and re-insert
//! after re-adding under the new href. This port's [`crate::oeb::spine::Spine`]
//! is keyed by manifest **id** (a `String`), not object identity, and
//! [`crate::oeb::manifest::Manifest::remove`] never touches the spine.
//! Since every rename here keeps the item's id unchanged, removing and
//! re-adding the manifest entry under the same id leaves the spine
//! untouched automatically -- no explicit position bookkeeping is
//! needed.

use std::collections::{HashMap, HashSet};

use regex::Regex;

use crate::oeb::book::OEBBook;
use crate::oeb::constants::{CSS_MIME, OEB_DOCS};
use crate::oeb::manifest::Manifest;
use crate::oeb::toc::TOCNode;

// ---------------------------------------------------------------------
// URL helpers (book-internal hrefs only -- no external URL parsing)
// ---------------------------------------------------------------------

/// Split `href` into `(path, fragment)` on the first `#`. Port of
/// `urllib.parse.urldefrag` as used on internal book hrefs.
pub(crate) fn urldefrag(href: &str) -> (String, String) {
    match href.split_once('#') {
        Some((p, f)) => (p.to_string(), f.to_string()),
        None => (href.to_string(), String::new()),
    }
}

/// Normalize slashes in an internal book href. Narrower than Python's
/// `urlnormalize` (which also percent-normalizes each path segment) --
/// hrefs handled by this module are always already-written manifest
/// hrefs, which this crate keeps ASCII-clean at write time, so segment
/// requoting is not needed here.
pub(crate) fn urlnormalize(href: &str) -> String {
    href.replace('\\', "/")
}

/// Resolve `rel` against the directory of `base_href`, collapsing `.`/
/// `..` segments. Port of `Item.abshref`.
pub(crate) fn abshref(base_href: &str, rel: &str) -> String {
    if rel.contains("://") || rel.starts_with('#') || rel.is_empty() {
        return rel.to_string();
    }
    let base_dir = match base_href.rfind('/') {
        Some(idx) => &base_href[..=idx],
        None => "",
    };
    let combined = format!("{base_dir}{rel}");
    let mut out: Vec<&str> = Vec::new();
    for seg in combined.split('/') {
        match seg {
            "." => {}
            ".." => {
                out.pop();
            }
            _ => out.push(seg),
        }
    }
    out.join("/")
}

/// Resolve `target` (a book-internal href, possibly with a `#fragment`)
/// relative to `base_href`'s directory. Port of `Item.relhref`.
pub(crate) fn relhref(base_href: &str, target: &str) -> String {
    let (target_path, frag) = match target.split_once('#') {
        Some((p, f)) => (p, Some(f)),
        None => (target, None),
    };
    if target_path.is_empty() {
        return match frag {
            Some(f) => format!("#{f}"),
            None => String::new(),
        };
    }
    let base_dir: Vec<&str> = match base_href.rfind('/') {
        Some(i) => base_href[..i]
            .split('/')
            .filter(|s| !s.is_empty())
            .collect(),
        None => Vec::new(),
    };
    let target_segs: Vec<&str> = target_path.split('/').collect();
    let mut common = 0usize;
    while common < base_dir.len()
        && common + 1 < target_segs.len()
        && base_dir[common] == target_segs[common]
    {
        common += 1;
    }
    let ups = base_dir.len() - common;
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), ups).collect();
    parts.extend(target_segs[common..].iter().map(|s| (*s).to_string()));
    let rel = if parts.is_empty() {
        target_segs.last().copied().unwrap_or("").to_string()
    } else {
        parts.join("/")
    };
    match frag {
        Some(f) => format!("{rel}#{f}"),
        None => rel,
    }
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Port of `posixpath.splitext`: split into `(root, ext)` where `ext`
/// includes the leading dot, ignoring a leading dot in the basename
/// itself (dotfiles have no extension).
fn splitext(path: &str) -> (String, String) {
    let slash = path.rfind('/').map(|i| i + 1).unwrap_or(0);
    let base = &path[slash..];
    match base.rfind('.') {
        Some(i) if i > 0 => (path[..slash + i].to_string(), base[i..].to_string()),
        _ => (path.to_string(), String::new()),
    }
}

/// Port of `Manifest.generate(href=...)` (the `id=None` case): a fresh
/// href built from `href_prefix`, unique (case-insensitively) among the
/// manifest's current hrefs.
pub(crate) fn generate_href(manifest: &Manifest, href_prefix: &str) -> String {
    let href = urlnormalize(href_prefix);
    let (base, ext) = splitext(&href);
    let lhrefs: HashSet<String> = manifest.hrefs.keys().map(|h| h.to_lowercase()).collect();
    if !lhrefs.contains(&href.to_lowercase()) {
        return href;
    }
    let mut index = 1u32;
    loop {
        let candidate = format!("{base}{index}{ext}");
        if !lhrefs.contains(&candidate.to_lowercase()) {
            return candidate;
        }
        index += 1;
    }
}

lazy_static::lazy_static! {
    /// Port of `_css_url_re`.
    static ref CSS_URL_RE: Regex = Regex::new(r#"(?is)url\s*\(\s*['"]?(.*?)['"]?\s*\)"#).unwrap();
}

/// Every `url(...)` reference in a CSS stylesheet's text. Port of the
/// `url()`-matching half of `itercsslinks`/`css_parser.getUrls`.
pub(crate) fn extract_css_urls(css_text: &str) -> Vec<String> {
    CSS_URL_RE
        .captures_iter(css_text)
        .map(|c| c[1].to_string())
        .collect()
}

/// Rewrite every `url(...)` reference in a CSS stylesheet's text via
/// `replace_fn`. `replace_fn` returning `None` leaves that reference
/// unchanged (mirrors [`RenameFiles::url_replacer`] returning the
/// original link when there's nothing to rewrite).
pub(crate) fn rewrite_css_urls(
    css_text: &str,
    mut replace_fn: impl FnMut(&str) -> Option<String>,
) -> String {
    CSS_URL_RE
        .replace_all(css_text, |caps: &regex::Captures| {
            let url = &caps[1];
            match replace_fn(url) {
                Some(new) => format!("url({new})"),
                None => caps[0].to_string(),
            }
        })
        .into_owned()
}

// ---------------------------------------------------------------------
// RenameFiles
// ---------------------------------------------------------------------

/// Port of `RenameFiles`: rewrite every link pointing at a renamed file.
/// Note that the spine and manifest are not touched by this transform --
/// the caller (`UniqueFilenames`/`FlatFilenames`) has already updated
/// them.
pub struct RenameFiles<'a> {
    rename_map: &'a HashMap<String, String>,
    /// `new_href -> pre-rename href`, used only by `FlatFilenames` (where
    /// the item's directory itself moved, so a relative link inside it
    /// must first be resolved against its *old* location before being
    /// looked up in `rename_map`).
    renamed_items_map: Option<&'a HashMap<String, String>>,
}

impl<'a> RenameFiles<'a> {
    pub fn new(
        rename_map: &'a HashMap<String, String>,
        renamed_items_map: Option<&'a HashMap<String, String>>,
    ) -> Self {
        RenameFiles {
            rename_map,
            renamed_items_map,
        }
    }

    pub fn call(&self, oeb: &mut OEBBook) {
        let items: Vec<(String, String)> = oeb
            .manifest
            .iter()
            .map(|i| (i.href.clone(), i.media_type.clone()))
            .collect();
        for (href, media_type) in items {
            let Ok(raw) = oeb.container.read(&href) else {
                continue;
            };
            if OEB_DOCS.contains(&media_type.as_str())
                || media_type.ends_with("/xml")
                || media_type.ends_with("+xml")
            {
                let html = String::from_utf8_lossy(&raw);
                let mut dom = crate::mobi::dom::Dom::parse(&html);
                let mut changed = false;
                for el in dom.preorder_elements(dom.root) {
                    for attr in ["href", "src", "xlink:href"] {
                        let cur = dom.node(el).attrs.get(attr).cloned();
                        if let Some(cur) = cur {
                            if let Some(new) = self.url_replacer(&href, &cur) {
                                if new != cur {
                                    dom.node_mut(el).attrs.insert(attr.to_string(), new);
                                    changed = true;
                                }
                            }
                        }
                    }
                }
                if changed {
                    let rendered = dom.serialize(dom.root).into_bytes();
                    let _ = oeb.container.write(&href, &rendered);
                }
            } else if media_type == CSS_MIME {
                let text = String::from_utf8_lossy(&raw);
                let mut changed = false;
                let new_text = rewrite_css_urls(&text, |url| {
                    let repl = self.url_replacer(&href, url);
                    if repl.as_deref() != Some(url) {
                        changed = true;
                    }
                    repl
                });
                if changed {
                    let _ = oeb.container.write(&href, new_text.as_bytes());
                }
            }
        }

        let refs: Vec<(String, String)> = oeb
            .guide
            .values()
            .map(|r| (r.type_.clone(), r.href.clone()))
            .collect();
        for (type_, href) in refs {
            let href = urlnormalize(&href);
            let (path, frag) = urldefrag(&href);
            if let Some(replacement) = self.rename_map.get(&path) {
                let mut nhref = replacement.clone();
                if !frag.is_empty() {
                    nhref.push('#');
                    nhref.push_str(&frag);
                }
                if let Some(r) = oeb.guide.references.get_mut(&type_) {
                    r.href = nhref;
                }
            }
        }

        self.fix_toc_entry(&mut oeb.toc.root);
    }

    fn fix_toc_entry(&self, node: &mut TOCNode) {
        if let Some(href) = &node.href {
            let href_n = urlnormalize(href);
            let (path, frag) = urldefrag(&href_n);
            if let Some(replacement) = self.rename_map.get(&path) {
                let nhref = match &frag[..] {
                    "" => replacement.clone(),
                    f => format!("{replacement}#{f}"),
                };
                node.href = Some(nhref);
            }
        }
        for child in &mut node.children {
            self.fix_toc_entry(child);
        }
    }

    /// Returns `None` when `orig_url` should be left completely alone
    /// (an absolute/external URL); otherwise the (possibly unchanged)
    /// replacement link.
    fn url_replacer(&self, current_href: &str, orig_url: &str) -> Option<String> {
        let url = urlnormalize(orig_url);
        if url.contains("://") {
            // Only rewrite local URLs.
            return Some(orig_url.to_string());
        }
        let (path, frag) = urldefrag(&url);
        let orig_item_href = match self.renamed_items_map {
            Some(map) => map
                .get(current_href)
                .cloned()
                .unwrap_or_else(|| current_href.to_string()),
            None => current_href.to_string(),
        };
        let href = abshref(&orig_item_href, &path);
        let target = self.rename_map.get(&href).cloned().unwrap_or(href);
        let mut replacement = relhref(current_href, &target);
        if !frag.is_empty() {
            replacement.push('#');
            replacement.push_str(&frag);
        }
        Some(replacement)
    }
}

// ---------------------------------------------------------------------
// UniqueFilenames
// ---------------------------------------------------------------------

/// Port of `UniqueFilenames`: ensure every manifest item has a unique
/// filename (ignoring directory), for broken readers that only look at
/// the basename.
pub struct UniqueFilenames;

impl UniqueFilenames {
    pub fn call(&self, oeb: &mut OEBBook) {
        let mut seen_filenames: HashSet<String> = HashSet::new();
        let mut rename_map: HashMap<String, String> = HashMap::new();
        let ids: Vec<String> = oeb.manifest.items.keys().cloned().collect();
        for id in ids {
            let Some(item) = oeb.manifest.get_by_id(&id) else {
                continue;
            };
            let href = item.href.clone();
            let media_type = item.media_type.clone();
            let fname = basename(&href).to_string();
            if seen_filenames.contains(&fname) {
                let suffix = Self::unique_suffix(&fname, &seen_filenames);
                let (base, ext) = splitext(&href);
                let nhref_candidate = format!("{base}{suffix}{ext}");
                let nhref = generate_href(&oeb.manifest, &nhref_candidate);
                let data = oeb.container.read(&href).unwrap_or_default();
                oeb.manifest.remove(&id);
                oeb.manifest.add(&id, &nhref, &media_type);
                let _ = oeb.container.write(&nhref, &data);
                seen_filenames.insert(basename(&nhref).to_string());
                rename_map.insert(href, nhref);
            } else {
                seen_filenames.insert(fname);
            }
        }
        if !rename_map.is_empty() {
            RenameFiles::new(&rename_map, None).call(oeb);
        }
    }

    fn unique_suffix(fname: &str, seen: &HashSet<String>) -> String {
        let (base, ext) = splitext(fname);
        let mut c = 0u32;
        loop {
            c += 1;
            let suffix = format!("_u{c}");
            let candidate = format!("{base}{suffix}{ext}");
            if !seen.contains(&candidate) {
                return suffix;
            }
        }
    }
}

// ---------------------------------------------------------------------
// FlatFilenames
// ---------------------------------------------------------------------

/// Port of `FlatFilenames`: ensure every manifest item has a unique
/// filename without subfolders (`a/b/c/index.html` ->
/// `a_b_c_index.html`), for readers that don't support nested paths.
pub struct FlatFilenames;

impl FlatFilenames {
    pub fn call(&self, oeb: &mut OEBBook) {
        let mut rename_map: HashMap<String, String> = HashMap::new();
        let mut renamed_items_map: HashMap<String, String> = HashMap::new();
        let ids: Vec<String> = oeb.manifest.items.keys().cloned().collect();
        for id in ids {
            let Some(item) = oeb.manifest.get_by_id(&id) else {
                continue;
            };
            let href = item.href.clone();
            let media_type = item.media_type.clone();
            let nhref_flat = href.replace('/', "_");
            if href == nhref_flat {
                continue;
            }
            let data = oeb.container.read(&href).unwrap_or_default();
            let nhref = generate_href(&oeb.manifest, &nhref_flat);
            oeb.manifest.remove(&id);
            oeb.manifest.add(&id, &nhref, &media_type);
            let _ = oeb.container.write(&nhref, &data);
            rename_map.insert(href.clone(), nhref.clone());
            renamed_items_map.insert(nhref, href);
        }
        if !rename_map.is_empty() {
            RenameFiles::new(&rename_map, Some(&renamed_items_map)).call(oeb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::transforms::test_support::Builder;

    #[test]
    fn abshref_and_relhref_round_trip() {
        assert_eq!(abshref("text/a.html", "../img/x.png"), "img/x.png");
        assert_eq!(relhref("text/a.html", "img/x.png"), "../img/x.png");
        assert_eq!(relhref("a.html", "a.html"), "a.html");
    }

    #[test]
    fn unique_filenames_renames_duplicate_basenames_and_rewrites_links() {
        let mut oeb = Builder::new()
            .page("a/index.html", r#"<a href="../b/index.html">link</a>"#)
            .page("b/index.html", "<p>b</p>")
            .build();
        UniqueFilenames.call(&mut oeb);
        // One of the two "index.html" items should have been renamed.
        let hrefs: HashSet<String> = oeb.manifest.iter().map(|i| i.href.clone()).collect();
        assert!(hrefs.contains("a/index.html"));
        assert!(hrefs.contains("b/index_u1.html"));
        // Spine order/ids are unaffected (ids didn't change).
        assert_eq!(oeb.spine.items.len(), 2);
        // The link in a/index.html should now point at the renamed file.
        let raw = oeb.container.read("a/index.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains("b/index_u1.html"), "{html}");
    }

    #[test]
    fn flat_filenames_flattens_and_rewrites_sibling_links() {
        let mut oeb = Builder::new()
            .page("text/a.html", r#"<img src="../img/z.png"/>"#)
            .part("img/z.png", "image/png", b"png", false)
            .build();
        FlatFilenames.call(&mut oeb);
        let hrefs: HashSet<String> = oeb.manifest.iter().map(|i| i.href.clone()).collect();
        assert!(hrefs.contains("text_a.html"));
        assert!(hrefs.contains("img_z.png"));
        let raw = oeb.container.read("text_a.html").unwrap();
        let html = String::from_utf8_lossy(&raw);
        assert!(html.contains(r#"src="img_z.png""#), "{html}");
    }

    #[test]
    fn extract_and_rewrite_css_urls() {
        let css = "body { background: url('images/bg.png') }";
        assert_eq!(extract_css_urls(css), vec!["images/bg.png".to_string()]);
        let out = rewrite_css_urls(css, |u| {
            if u == "images/bg.png" {
                Some("assets/bg.png".to_string())
            } else {
                None
            }
        });
        assert!(out.contains("url(assets/bg.png)"), "{out}");
    }
}
