use calibre_ebooks::input::pml_input::PMLInput;
use calibre_ebooks::pdb::writer::PdbWriter;
use std::fs::{self, File};
use std::io::BufWriter;
use tempfile::tempdir;

#[test]
fn test_pml_input_conversion() {
    // 1. Create a PDB file with PML content
    let tmp_dir = tempdir().unwrap();
    let pdb_path = tmp_dir.path().join("book.pdb");

    let contents = b"\\pThis is a \\btest\\b of \\iPML\\i.";

    let file = File::create(&pdb_path).unwrap();
    let mut writer = BufWriter::new(file);
    let pdb_writer = PdbWriter::new();
    pdb_writer
        .write("PML Test", contents, &mut writer)
        .expect("Failed to write PDB");
    drop(writer);

    // 2. Convert using PMLInput
    println!("Created PDB at {:?}", pdb_path);
    let output_dir = tmp_dir.path().join("output");
    fs::create_dir(&output_dir).unwrap();

    let input = PMLInput::new();
    println!("Starting conversion...");
    let book = input
        .convert(&pdb_path, &output_dir)
        .map_err(|e| {
            println!("Conversion error: {:?}", e);
            e
        })
        .expect("Conversion failed");
    println!("Conversion successful");

    // 3. Verify
    assert_eq!(book.metadata.items[0].value, "PML Test");

    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());

    let html = fs::read_to_string(content_path).unwrap();
    // \p -> <p>, \b -> <strong>, \i -> <em>
    assert!(html.contains("<p>"));
    assert!(html.contains("<strong>test</strong>"));
    assert!(html.contains("<em>PML</em>")); // Note: My simple parser toggled off?
                                            // "of \\iPML\\i."
                                            // 1st \i: in_italic=true, push <em>
                                            // 2nd \i: in_italic=false, push </em>
                                            // So <em>PML</em>. Correct.
}
