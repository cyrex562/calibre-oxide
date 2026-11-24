//! A flat view of a book's Dublin Core metadata.
//!
//! Port of `old_src/src/calibre/ebooks/html/meta.py`.
//!
//! `EasyMeta` exists so an HTML template can iterate a book's metadata
//! without knowing anything about namespaces: it yields the Dublin Core
//! terms with their prefixes stripped, and skips everything else.

use crate::oeb::metadata::Metadata;

/// The Dublin Core 1.1 namespace, whose terms are the ones worth
/// showing.
pub const DC11_NS: &str = "http://purl.org/dc/elements/1.1/";

/// One metadata item, reduced to a name and a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaItem {
    pub name: String,
    pub value: String,
}

/// A flattened view over a book's metadata.
///
/// Port of the Python `EasyMeta`.
#[derive(Debug, Clone, Copy)]
pub struct EasyMeta<'a> {
    meta: &'a Metadata,
}

impl<'a> EasyMeta<'a> {
    pub fn new(meta: &'a Metadata) -> Self {
        Self { meta }
    }

    /// Every Dublin Core item, with its namespace stripped.
    ///
    /// Port of the Python `__iter__`. Terms are matched by name rather
    /// than by resolved namespace: this crate's metadata store keeps
    /// bare terms (`title`, `creator`) where calibre keeps them fully
    /// qualified, so the DC vocabulary is the filter.
    pub fn items(&self) -> Vec<MetaItem> {
        let mut out = Vec::new();
        for term in DC_TERMS {
            for item in self.meta.get(term) {
                out.push(MetaItem {
                    name: (*term).to_string(),
                    value: item.value.clone(),
                });
            }
        }
        out
    }

    /// How many Dublin Core items there are.
    ///
    /// Port of the Python `__len__`.
    pub fn len(&self) -> usize {
        self.items().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Port of the Python `titles`.
    pub fn titles(&self) -> Vec<String> {
        self.meta
            .get("title")
            .iter()
            .map(|i| i.value.clone())
            .collect()
    }

    /// Port of the Python `creators`.
    pub fn creators(&self) -> Vec<String> {
        self.meta
            .get("creator")
            .iter()
            .map(|i| i.value.clone())
            .collect()
    }
}

/// The fifteen Dublin Core 1.1 elements.
pub const DC_TERMS: &[&str] = &[
    "title",
    "creator",
    "subject",
    "description",
    "publisher",
    "contributor",
    "date",
    "type",
    "format",
    "identifier",
    "source",
    "language",
    "relation",
    "coverage",
    "rights",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Metadata {
        let mut m = Metadata::new();
        m.add("title", "A Book");
        m.add("title", "An Alternative Title");
        m.add("creator", "Jane Austen");
        m.add("language", "en");
        // Not Dublin Core, so it must not appear.
        m.add("cover", "cover-id");
        m
    }

    #[test]
    fn yields_only_the_dublin_core_items() {
        let m = meta();
        let easy = EasyMeta::new(&m);
        let items = easy.items();
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["title", "title", "creator", "language"]);
        assert!(!names.contains(&"cover"));
        assert_eq!(easy.len(), 4);
        assert!(!easy.is_empty());
    }

    #[test]
    fn titles_and_creators_come_back_in_order() {
        let m = meta();
        let easy = EasyMeta::new(&m);
        assert_eq!(easy.titles(), vec!["A Book", "An Alternative Title"]);
        assert_eq!(easy.creators(), vec!["Jane Austen"]);
    }

    #[test]
    fn a_book_with_no_metadata_is_empty() {
        let m = Metadata::new();
        let easy = EasyMeta::new(&m);
        assert!(easy.is_empty());
        assert!(easy.items().is_empty());
        assert!(easy.titles().is_empty());
        assert!(easy.creators().is_empty());
    }
}
