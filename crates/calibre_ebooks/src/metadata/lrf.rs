use crate::metadata::MetaInformation;
use anyhow::Result;
use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(_stream: R) -> Result<MetaInformation> {
    // LRF metadata parsing is quite complex (binary objects).
    // For this initial port, we will return default/unknown metadata
    // rather than implementing a full binary parser for proprietary LRF.
    // TODO: Implement full LRF metadata extraction.

    let mut mi = MetaInformation::default();
    mi.title = "Unknown LRF".to_string();
    mi.authors = vec!["Unknown".to_string()];

    Ok(mi)
}
