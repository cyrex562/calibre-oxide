use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Annotation {
    #[serde(rename = "bookmark")]
    Bookmark(Bookmark),
    #[serde(rename = "highlight")]
    Highlight(Highlight),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub timestamp: String, // ISO8601 string
    pub pos: String,       // CFI or other position format
    pub pos_type: String,  // "epubcfi", etc.
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Highlight {
    pub uuid: String,
    pub timestamp: String,
    pub start_cfi: String,
    pub end_cfi: String,
    pub highlighted_text: Option<String>,
    pub notes: Option<String>,
}

impl Annotation {
    pub fn timestamp(&self) -> &str {
        match self {
            Annotation::Bookmark(b) => &b.timestamp,
            Annotation::Highlight(h) => &h.timestamp,
        }
    }
}

/// Sorts annotations by timestamp (descending).
pub fn sort_annotations_by_timestamp(annotations: &mut [Annotation]) {
    annotations.sort_by(|a, b| {
        // Reverse order for descending
        b.timestamp().cmp(a.timestamp())
    });
}
