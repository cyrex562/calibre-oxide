use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Item {
    pub term: String,
    pub value: String,
    pub attrib: HashMap<String, String>,
}

impl Item {
    pub fn new(term: &str, value: &str, attrib: Option<HashMap<String, String>>) -> Self {
        let attrib = attrib.unwrap_or_default();

        // Basic Logic from python:
        // if namespace(term) == OPF2_NS: term = barename(term)
        // Check DC_TERMS / CALIBRE_TERMS coercion

        let term_str = term.to_string();

        Item {
            term: term_str,
            value: value.to_string(),
            attrib,
        }
    }

    pub fn get_attribute(&self, key: &str) -> Option<&String> {
        self.attrib.get(key)
    }

    pub fn set_attribute(&mut self, key: &str, value: &str) {
        self.attrib.insert(key.to_string(), value.to_string());
    }
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub items: Vec<Item>,
}

impl Metadata {
    pub fn new() -> Self {
        Metadata { items: Vec::new() }
    }

    pub fn add(&mut self, term: &str, value: &str) {
        self.items.push(Item::new(term, value, None));
    }

    pub fn add_with_attrib(&mut self, term: &str, value: &str, attrib: HashMap<String, String>) {
        self.items.push(Item::new(term, value, Some(attrib)));
    }

    pub fn get(&self, term: &str) -> Vec<&Item> {
        self.items.iter().filter(|i| i.term == term).collect()
    }

    /// Port of `oeb.metadata.clear(term)`: drop every item with this
    /// term.
    pub fn clear(&mut self, term: &str) {
        self.items.retain(|i| i.term != term);
    }

    /// Port of `oeb.metadata.filter(term, predicate)`: drop every item
    /// with this term for which `should_remove` returns `true` (Python's
    /// `predicate`).
    pub fn filter(&mut self, term: &str, mut should_remove: impl FnMut(&Item) -> bool) {
        self.items.retain(|i| i.term != term || !should_remove(i));
    }

    /// The first item with this term, if any (`str(m.title[0])` and
    /// similar single-valued accesses in Python).
    pub fn first(&self, term: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.term == term)
    }

    /// Mutable access to every item with this term, e.g. to update an
    /// existing `identifier` item's value in place by matching its
    /// `scheme` attribute (`meta_info_to_oeb_metadata`'s `x.content =
    /// val` loop).
    pub fn iter_mut<'a>(&'a mut self, term: &'a str) -> impl Iterator<Item = &'a mut Item> {
        self.items.iter_mut().filter(move |i| i.term == term)
    }
}

// Helper functions moved to parse_utils.rs
