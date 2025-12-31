use anyhow::Result;
use calibre_conversion::plugins::epub_input::EpubInput;
use calibre_conversion::plugins::epub_output::EpubOutput;
use calibre_conversion::{ConversionOptions, ConversionPipeline};
use std::path::PathBuf;
use tempfile::Builder;

#[test]
fn test_identity_conversion() -> Result<()> {
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
    let output_plugin = Box::new(EpubOutput);

    let pipeline = ConversionPipeline::new(input_plugin, output_plugin);
    let options = ConversionOptions::default();

    // 3. Prepare Output
    let temp_dir = Builder::new().prefix("calibre_test_output_").tempdir()?;
    let output_path = temp_dir.path().join("output.epub");

    // 4. Run
    pipeline.run(&input_path, &output_path, &options)?;

    // 5. Verify Output
    assert!(output_path.exists(), "Output EPUB was not created");

    // Check basic ZIP structure
    let file = std::fs::File::open(&output_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    // Check key files
    assert!(archive.by_name("mimetype").is_ok(), "mimetype missing");
    assert!(
        archive.by_name("META-INF/container.xml").is_ok(),
        "container.xml missing"
    );
    assert!(
        archive.by_name("content.opf").is_ok(),
        "content.opf missing"
    );

    // Read OPF and check title
    let mut opf_file = archive.by_name("content.opf")?;
    let mut opf_content = String::new();
    std::io::Read::read_to_string(&mut opf_file, &mut opf_content)?;

    println!("Generated OPF:\n{}", opf_content);
    assert!(
        opf_content.contains("Quick Start Guide"),
        "Title mismatch in generated OPF"
    );

    Ok(())
}
