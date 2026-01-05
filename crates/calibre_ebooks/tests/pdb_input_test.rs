use calibre_ebooks::input::pdb_input::PDBInput;
use calibre_ebooks::pdb::writer::PdbWriter;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_pdb_input_conversion() {
    let tmp_dir = tempdir().unwrap();
    let input_path = tmp_dir.path().join("test_gen.pdb");
    let output_dir = tmp_dir.path().join("output");

    // Create a real PDB using PdbWriter
    {
        let mut file = fs::File::create(&input_path).unwrap();
        let writer = PdbWriter::new();
        let content = b"Record 0 Content\nRecord 1 Content";
        writer.write("TestPDB", content, &mut file).unwrap();
    }

    let input = PDBInput::new();
    let book = input
        .convert(&input_path, &output_dir)
        .expect("PDB conversion failed");

    // Check Manifest
    assert!(!book.manifest.items.is_empty());
    let titles = book.metadata.get("title");
    assert!(!titles.is_empty());
    assert_eq!(titles[0].value, "TestPDB");

    // Check Content File
    let content_path = output_dir.join("index.html");
    assert!(content_path.exists());
    let content = fs::read_to_string(content_path).unwrap();
    assert!(content.contains("Record 0:"));
    assert!(content.contains("Record 1:"));

    // Check that binary records were created
    assert!(output_dir.join("record_0.bin").exists());
    assert!(output_dir.join("record_1.bin").exists());
}
