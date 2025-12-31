use crate::oeb::OebBook;
use crate::traits::{ConversionOptions, Transform};
use anyhow::{Context, Result};
use html5ever::parse_document;
use html5ever::serialize::{serialize, SerializeOpts};
use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{RcDom, SerializableHandle};
use std::fs::File;
use std::io::{Read, Write};

pub struct HtmlRoundTrip;

impl Transform for HtmlRoundTrip {
    fn process(&self, book: &mut OebBook, _options: &ConversionOptions) -> Result<()> {
        for item in book.manifest.values() {
            // Only process HTML/XHTML files
            if item.media_type == "application/xhtml+xml" || item.media_type == "text/html" {
                let path = &item.path;

                // 1. Read content
                let mut content = String::new();
                File::open(path)
                    .context(format!("Failed to open file {:?}", path))?
                    .read_to_string(&mut content)?;

                // 2. Parse to DOM
                let dom = parse_document(RcDom::default(), Default::default())
                    .from_utf8()
                    .read_from(&mut content.as_bytes())?;

                // 3. Serialize back to bytes
                let mut serialized = Vec::new();
                let document: SerializableHandle = dom.document.clone().into();
                serialize(&mut serialized, &document, SerializeOpts::default())
                    .context("Failed to serialize DOM")?;

                // 4. Overwrite file
                let mut file = File::create(path).context("Failed to create file for writing")?;
                file.write_all(&serialized)?;

                log::info!("Round-tripped HTML file: {:?}", item.href);
            }
        }
        Ok(())
    }
}
