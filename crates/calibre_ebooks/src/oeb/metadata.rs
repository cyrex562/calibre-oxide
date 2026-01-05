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
}

// Helper functions moved to parse_utils.rs
