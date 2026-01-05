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
    pub items: HashMap<String, ManifestItem>, // Map id -> Item
    pub hrefs: HashMap<String, String>,       // Map href -> id
}

impl Manifest {
    pub fn new() -> Self {
        Manifest {
            items: HashMap::new(),
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

    pub fn remove(&mut self, id: &str) {
        if let Some(item) = self.items.remove(id) {
            self.hrefs.remove(&item.href);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &ManifestItem> {
        self.items.values()
    }
}
