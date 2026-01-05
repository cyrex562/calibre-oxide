use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::manifest::ManifestItem;
use calibre_ebooks::output::pdb_output::PDBOutput;
use calibre_ebooks::pdb::reader::PdbReader;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_pdb_output_conversion() {
    let tmp_dir = tempdir().unwrap();
    let book_dir = tmp_dir.path().join("book");
    fs::create_dir_all(&book_dir).unwrap();
    let output_path = tmp_dir.path().join("output.pdb");

    // Create a dummy OEB book
    let content_file = "content.html";
    let content_path = book_dir.join(content_file);
    fs::write(
        &content_path,
        "<html><body><p>PDB Record Content</p></body></html>",
    )
    .unwrap();

    let container = Box::new(DirContainer::new(&book_dir));
    let mut book = OEBBook::new(container);
    let id = "item1".to_string();
    book.manifest.items.insert(
        id.clone(),
        ManifestItem {
            id: id.clone(),
            href: content_file.to_string(),
            media_type: "application/xhtml+xml".to_string(),
            fallback: None,
            linear: true,
        },
    );
    book.spine.add(&id, true);

    let output = PDBOutput::new();
    output
        .convert(&book, &output_path)
        .expect("PDB output conversion failed");

    assert!(output_path.exists());

    // Read back to verify
    let mut reader = PdbReader::new(&output_path).unwrap();
    // Assuming 1 record for 1 content item
    assert!((reader.header.num_records as usize) >= 1);

    // Verify PDB structure
    // Record 0 should be PalmDoc header (16 bytes)
    assert!(reader.header.num_records >= 2);

    let rec0 = reader.read_record(0).unwrap();
    assert_eq!(rec0.len(), 16);

    // Record 1 should be the compressed contents
    let rec1 = reader.read_record(1).unwrap();
    assert!(!rec1.is_empty());
}
