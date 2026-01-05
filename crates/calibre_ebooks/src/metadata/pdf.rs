use crate::metadata::MetaInformation;
use anyhow::{Context, Result};
use lopdf::{Document, Object};
use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    // lopdf requires reading the whole stream or from a path.
    // Since we have a stream, let's load it into memory.
    // Note: This might be heavy for large PDFs, but old_code did subprocess.
    // For now, load into memory.
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer)?;

    let doc = Document::load_mem(&buffer).context("Failed to load PDF document")?;

    let mut mi = MetaInformation::default();

    if let Some(info_id) = doc
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|o| o.as_reference().ok())
    {
        if let Ok(info_dict) = doc.get_object(info_id).and_then(|o| o.as_dict()) {
            if let Some(title) = info_dict
                .get(b"Title")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.title = title;
            }
            if let Some(author) = info_dict
                .get(b"Author")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.authors = author
                    .split(&[',', ';'][..])
                    .map(|s| s.trim().to_string())
                    .collect();
            }
            if let Some(subject) = info_dict
                .get(b"Subject")
                .ok()
                .and_then(|o| from_pdf_object(o).ok())
            {
                mi.tags = subject
                    .split(&[',', ';'][..])
                    .map(|s| s.trim().to_string())
                    .collect();
            }
        }
    }

    Ok(mi)
}

fn from_pdf_object(obj: &Object) -> Result<String> {
    match obj {
        Object::String(bytes, _) => Ok(String::from_utf8_lossy(bytes).to_string()),
        Object::Name(bytes) => Ok(String::from_utf8_lossy(bytes).to_string()),
        _ => anyhow::bail!("Not a string object"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Dictionary;
    use lopdf::Object;

    #[test]
    fn test_pdf_metadata() -> Result<()> {
        // Construct a minimal PDF with info dict in memory?
        // lopdf allows constructing documents.
        let mut doc = Document::with_version("1.4");
        let pages_id = doc.new_object_id();
        let info_id = doc.new_object_id();

        let mut info = Dictionary::new();
        info.set("Title", Object::string_literal("Test PDF Title"));
        info.set("Author", Object::string_literal("Test Author"));
        info.set("Subject", Object::string_literal("Tag1, Tag2"));

        doc.objects.insert(info_id, Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));
        doc.trailer.set("Root", Object::Reference(pages_id)); // Minimal root

        // Serialize to buffer
        let mut buffer = Vec::new();
        doc.save_to(&mut buffer)?;

        let mut stream = std::io::Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "Test PDF Title");
        assert_eq!(mi.authors, vec!["Test Author"]);
        assert!(mi.tags.contains(&"Tag1".to_string()));

        Ok(())
    }
}
