use calibre_ebooks::opf::OpfMetadata;
use std::collections::HashMap;
use std::path::PathBuf;

/// Represents an item in the book's manifest (a file).
#[derive(Debug, Clone)]
pub struct ManifestItem {
    pub id: String,
    pub href: String,
    pub media_type: String,
    /// Absolute path to the file on disk (temp location during conversion)
    pub path: PathBuf,
}

/// Represents an item in the book's spine (reading order).
#[derive(Debug, Clone)]
pub struct SpineItem {
    pub idref: String,
    pub linear: bool,
}

/// Represents a reference in the book's guide (e.g., cover, table of contents).
#[derive(Debug, Clone)]
pub struct GuideReference {
    pub reference_type: String,
    pub title: String,
    pub href: String,
}

/// The Intermediate Representation (IR) of the e-book during conversion.
/// Based on the Open eBook (OEB) format used by Calibre.
#[derive(Debug, Default)]
pub struct OebBook {
    pub metadata: OpfMetadata,
    pub manifest: HashMap<String, ManifestItem>,
    pub spine: Vec<SpineItem>,
    pub guide: Vec<GuideReference>,
    /// Version of the OPF/Package file (e.g., "2.0", "3.0")
    pub version: String,
}

impl OebBook {
    pub fn new() -> Self {
        Self {
            version: "2.0".to_string(), // Default to 2.0
            ..Default::default()
        }
    }
}
