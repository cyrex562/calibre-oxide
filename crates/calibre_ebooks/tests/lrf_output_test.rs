
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::lrf_output::LRFOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_lrf_output_conversion_stub() {
    let tmp_dir = tempdir().unwrap();
    let book_dir = tmp_dir.path().join("book");
    fs::create_dir_all(&book_dir).unwrap();
    let output_path = tmp_dir.path().join("output.lrf");

    let container = Box::new(DirContainer::new(&book_dir));
    let book = OEBBook::new(container);

    let output = LRFOutput::new();
    output.convert(&book, &output_path).expect("LRF output conversion failed");

    assert!(output_path.exists());
    let content = fs::read_to_string(output_path).unwrap();
    assert_eq!(content, "LRF STUB FILE");
}
