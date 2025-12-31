use anyhow::Result;
use std::collections::HashSet;

// Stubbed implementation for now as we lack full HTML parsing capabilities
// in this iteration.

pub fn export_note(doc: &str, _get_resource: impl Fn(&str) -> Option<Vec<u8>>) -> String {
    // Just return the doc as is for now, or strip tags if we had a lightweight way.
    // For MVP, returning raw doc is acceptable or wrapped.
    doc.to_string()
}

pub fn import_note(
    html: &str,
    _basedir: &str,
    _add_resource: impl Fn(&[u8], &str) -> String,
) -> Result<(String, String, HashSet<String>)> {
    // Return (doc, searchable_text, resources)
    // Stub: searchable text is just same as html for now.
    Ok((html.to_string(), html.to_string(), HashSet::new()))
}
