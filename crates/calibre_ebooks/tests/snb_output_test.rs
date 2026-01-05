use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::output::snb_output::SnbOutput;
use tempfile::tempdir;

#[test]
fn test_snb_output_stub() {
    let tmp_dir = tempdir().unwrap();
    let output_path = tmp_dir.path().join("book.snb");
    let container = Box::new(calibre_ebooks::oeb::container::DirContainer::new(
        tmp_dir.path(),
    ));
    let book = OEBBook::new(container);

    let output = SnbOutput::new();
    let result = output.convert(&book, &output_path);

    // Should return Error as not fully implemented
    assert!(result.is_err());
    assert!(result
        .err()
        .unwrap()
        .to_string()
        .contains("not fully implemented"));
}
