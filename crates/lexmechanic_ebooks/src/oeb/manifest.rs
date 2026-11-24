use indexmap::IndexMap;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    pub fallback: Option<String>,
    pub linear: bool, // Note: spine positions are managed by Spine, but linear is property of item in Python? No, SpineItem has linear. But ManifestItem has 'linear' in Python docstring? Python codebase says "linear: True for textual content items...".
                      // We will keep linear here for now or just in Spine.
                      // In Python base.py: Item has `linear = True`.
}

impl ManifestItem {
    pub fn new(id: &str, href: &str, media_type: &str) -> Self {
        ManifestItem {
            id: id.to_string(),
            href: href.to_string(),
            media_type: media_type.to_string(),
            fallback: None,
            linear: true,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    // `IndexMap` (not `HashMap`): item order matters. Several ported
    // modules (e.g. `mobi::writer2::resources::Resources`) assign
    // sequential record/image indices by manifest iteration order, and
    // that order must be stable and match insertion order (as Python's
    // `OrderedDict`-backed manifest does) for the assigned indices to be
    // reproducible.
    pub items: IndexMap<String, ManifestItem>, // Map id -> Item
    pub hrefs: HashMap<String, String>,        // Map href -> id
}

impl Manifest {
    pub fn new() -> Self {
        Manifest {
            items: IndexMap::new(),
            hrefs: HashMap::new(),
        }
    }

    pub fn add(&mut self, id: &str, href: &str, media_type: &str) -> ManifestItem {
        let item = ManifestItem::new(id, href, media_type);
        self.items.insert(id.to_string(), item.clone());
        self.hrefs.insert(href.to_string(), id.to_string());
        item
    }

    pub fn get_by_id(&self, id: &str) -> Option<&ManifestItem> {
        self.items.get(id)
    }

    pub fn get_by_href(&self, href: &str) -> Option<&ManifestItem> {
        self.hrefs.get(href).and_then(|id| self.items.get(id))
    }

    /// A fresh, unused `(id, href)` pair built from `id_prefix`/
    /// `href_prefix`. Port of `oeb.manifest.generate(id_prefix,
    /// href_prefix)` (used by `mobi::writer8::toc::TocAdder` to name a
    /// generated inline-TOC document, matching
    /// `oeb.manifest.generate('contents', 'contents.xhtml')`).
    pub fn generate(&self, id_prefix: &str, href_prefix: &str) -> (String, String) {
        let mut n = 1u32;
        loop {
            let id = if n == 1 {
                id_prefix.to_string()
            } else {
                format!("{id_prefix}{n}")
            };
            let href = if n == 1 {
                href_prefix.to_string()
            } else {
                match href_prefix.rsplit_once('.') {
                    Some((base, ext)) => format!("{base}{n}.{ext}"),
                    None => format!("{href_prefix}{n}"),
                }
            };
            if !self.items.contains_key(&id) && !self.hrefs.contains_key(&href) {
                return (id, href);
            }
            n += 1;
        }
    }

    pub fn remove(&mut self, id: &str) {
        // `shift_remove`, not `remove`/`swap_remove`: this preserves
        // the relative order of every other item, matching Python's
        // `OrderedDict`-backed manifest (see the `items` field doc).
        if let Some(item) = self.items.shift_remove(id) {
            self.hrefs.remove(&item.href);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ManifestItem> {
        self.items.values()
    }

    /// Port of `Manifest.main_stylesheet`: the first manifest item whose
    /// media type is a CSS stylesheet, in manifest order. Used by
    /// `transforms::page_margin::RemoveFakeMargins` to find the "flattened
    /// CSS" sheet it edits.
    pub fn main_stylesheet(&self) -> Option<&ManifestItem> {
        self.items
            .values()
            .find(|i| crate::oeb::constants::OEB_STYLES.contains(&i.media_type.as_str()))
    }
}
