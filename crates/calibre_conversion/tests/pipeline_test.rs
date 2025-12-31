use anyhow::Result;
use calibre_conversion::plugins::epub_input::EpubInput;
use calibre_conversion::{
    ConversionOptions, ConversionPipeline, InputPlugin, OebBook, OutputPlugin,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// Mock Output Plugin
struct MockOutput {
    pub last_book: Arc<Mutex<Option<OebBook>>>,
}

impl MockOutput {
    fn new() -> Self {
        Self {
            last_book: Arc::new(Mutex::new(None)),
        }
    }
}

impl OutputPlugin for MockOutput {
    fn write(&self, book: &OebBook, _path: &Path, _options: &ConversionOptions) -> Result<()> {
        // Clone the book data we care about for verification
        // Since OebBook isn't fully Clone yet (maybe?), we just inspect it here or store it.
        // Actually OebBook derives Default/Debug, let's just store the title to verify.
        let mut loop_lock = self.last_book.lock().unwrap();
        // create a copy or just move fields if we could, but we can't consume.
        // We'll just manufacture a copy for the test check
        let copy = OebBook {
            metadata: calibre_ebooks::opf::OpfMetadata {
                title: book.metadata.title.clone(),
                authors: book.metadata.authors.clone(),
                ..Default::default()
            },
            ..Default::default()
        };
        *loop_lock = Some(copy);
        Ok(())
    }
}

#[test]
fn test_pipeline_epub_read() {
    // 1. Locate test file
    let mut d = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    d.pop(); // crates
    d.pop(); // project root
    let input_path = d.join("old_src/resources/quick_start/eng.epub");

    assert!(
        input_path.exists(),
        "Test file not found at {:?}",
        input_path
    );

    // 2. Setup Pipeline
    let input_plugin = Box::new(EpubInput);
    let mock_output = MockOutput::new();
    let result_storage = mock_output.last_book.clone();
    let output_plugin = Box::new(mock_output);

    let pipeline = ConversionPipeline::new(input_plugin, output_plugin);
    let options = ConversionOptions::default();

    // 3. Run
    let output_path = PathBuf::from("dummy_output.mobi"); // Won't be used by mock
    let res = pipeline.run(&input_path, &output_path, &options);

    assert!(res.is_ok(), "Pipeline failed: {:?}", res.err());

    // 4. Verify
    let db = result_storage.lock().unwrap();
    let book = db.as_ref().expect("Output plugin was not called");

    println!("Parsed Title: {}", book.metadata.title);
    println!("Parsed Authors: {:?}", book.metadata.authors);

    // Calibre Quick Start Guide usually has this title
    assert!(
        book.metadata.title.contains("Quick Start Guide"),
        "Title mismatch: {}",
        book.metadata.title
    );
}
