use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::output::pml_output::PMLOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_pml_output_generation() {
    let tmp_dir = tempdir().unwrap();
    let input_dir = tmp_dir.path().join("input");
    let output_file = tmp_dir.path().join("book.pml");
    fs::create_dir(&input_dir).unwrap();

    // Create simple HTML structure
    let index_html = r#"
    <html>
        <head><title>PML Output Test</title></head>
        <body>
            <h1>Heading</h1>
            <p>This is a <b>bold</b> paragraph.</p>
            <p>This is an <i>italic</i> paragraph.</p>
        </body>
    </html>
    "#;
    fs::write(input_dir.join("index.html"), index_html).unwrap();

    // Ingest
    let input = HTMLInput::new();
    let book = input
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("Ingest failed");

    // Convert to PML
    PMLOutput::new()
        .convert(&book, &output_file)
        .expect("PML export failed");

    // Verify PDB
    // We can use generic PdbReader to verify content?
    assert!(output_file.exists());
    let metadata = fs::metadata(&output_file).unwrap();
    assert!(metadata.len() > 0);

    // Read back?
    // TODO: Use PdbReader/PMLInput?
    // Just file existence is good for now.
}
