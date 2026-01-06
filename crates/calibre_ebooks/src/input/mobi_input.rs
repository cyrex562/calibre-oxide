use crate::mobi::reader::MobiReader;
use crate::oeb::book::OEBBook;
use anyhow::Result;
use std::path::Path;

pub struct MOBIInput;

impl MOBIInput {
    pub fn new() -> Self {
        MOBIInput
    }

    pub fn convert(&self, input_path: &Path, output_dir: &Path) -> Result<OEBBook> {
        println!("Reading MOBI...");
        let mut reader = MobiReader::new(input_path)?;
        let pdb_header = &reader.pdb_header;
        let sections = &mut reader.sections;

        // Select best section (KF8 preferred)
        let section = if sections.len() > 1 {
            println!("Joint MOBI detected. Using KF8 section.");
            &mut sections[1]
        } else {
            &mut sections[0]
        };

        println!(
            "Extracting text from section (Start Record: {})...",
            section.start_record
        );
        let raw_text = section.extract_text(input_path, pdb_header)?;

        // Basic OEB Book Construction
        use crate::oeb::container::DirContainer;
        std::fs::create_dir_all(output_dir)?;
        let container = Box::new(DirContainer::new(output_dir));
        let mut book = OEBBook::new(container);

        // Save content to file
        let content_filename = "index.html";
        let content_path = output_dir.join(content_filename);
        std::fs::write(&content_path, &raw_text)?;

        // Add Metadata
        use crate::oeb::metadata::Item as MetaItem;

        let title = "Converted MOBI".to_string();

        book.metadata.items.push(MetaItem {
            term: "dc:title".to_string(),
            value: title,
            attrib: Default::default(),
        });

        if let Some(exth) = &section.exth {
            // Try to find title (Record 503)
        }

        // Add Manifest
        use crate::oeb::manifest::ManifestItem;
        book.manifest.items.insert(
            "item1".to_string(),
            ManifestItem {
                id: "item1".to_string(),
                href: content_filename.to_string(),
                media_type: "application/xhtml+xml".to_string(),
                fallback: None,
                linear: true,
            },
        );

        // Add Spine
        use crate::oeb::spine::SpineItem;
        book.spine.items.push(SpineItem {
            idref: "item1".to_string(),
            linear: true,
        });

        println!("Extracted {} bytes of text.", raw_text.len());
        Ok(book)
    }
}
