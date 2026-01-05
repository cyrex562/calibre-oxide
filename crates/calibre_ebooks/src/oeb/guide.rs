use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Reference {
    pub type_: String,
    pub title: Option<String>,
    pub href: String,
}

impl Reference {
    pub fn new(type_: &str, title: Option<String>, href: &str) -> Self {
        Reference {
            type_: type_.to_string(),
            title,
            href: href.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Guide {
    pub references: HashMap<String, Reference>,
}

impl Guide {
    pub fn new() -> Self {
        Guide {
            references: HashMap::new(),
        }
    }

    pub fn add(&mut self, type_: &str, title: Option<String>, href: &str) {
        let reference = Reference::new(type_, title, href);
        self.references.insert(type_.to_string(), reference);
    }

    pub fn get(&self, type_: &str) -> Option<&Reference> {
        self.references.get(type_)
    }
}
