use calibre_ebooks::input::snb_input::SnbInput;
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::snb_output::SnbOutput;
use tempfile::tempdir;

/// Writing an empty book is a real, valid (if minimal) SNB container --
/// `SnbOutput` is no longer the unimplemented stub `write_dummy` was.
#[test]
fn test_snb_output_empty_book_produces_a_valid_container() {
    let tmp_dir = tempdir().unwrap();
    let output_path = tmp_dir.path().join("book.snb");
    let container = Box::new(DirContainer::new(tmp_dir.path()));
    let mut book = OEBBook::new(container);
    book.metadata.add("title", "Empty Book");

    let output = SnbOutput::new();
    let result = output.convert(&book, &output_path);

    assert!(result.is_ok(), "{result:?}");
    assert!(output_path.exists());
}

/// Definition-of-done round trip: write a book with `SnbOutput`, read
/// it back with `SnbInput` (the same dispatcher a real `.snb` file goes
/// through), and check the original text and metadata survive.
#[test]
fn test_snb_output_then_input_round_trips_text_and_metadata() {
    let src_tmp = tempdir().unwrap();
    let mut book = OEBBook::new(Box::new(DirContainer::new(src_tmp.path())));
    book.manifest
        .add("item1", "index.html", "application/xhtml+xml");
    book.spine.add("item1", true);
    book.container
        .write(
            "index.html",
            b"<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>Round trip SNB content</p></body></html>",
        )
        .unwrap();
    book.metadata.add("title", "Round Trip Book");
    book.metadata.add("creator", "Round Trip Author");

    let out_path = src_tmp.path().join("book.snb");
    SnbOutput::new().convert(&book, &out_path).unwrap();

    let extract_dir = tempdir().unwrap();
    let read_back = SnbInput::new()
        .convert(&out_path, extract_dir.path())
        .unwrap();

    assert_eq!(
        read_back.metadata.first("title").map(|i| i.value.clone()),
        Some("Round Trip Book".to_string())
    );
    assert_eq!(
        read_back.metadata.get("creator")[0].value,
        "Round Trip Author".to_string()
    );

    assert_eq!(read_back.spine.items.len(), 1);
    let page = read_back
        .manifest
        .get_by_id(&read_back.spine.items[0].idref)
        .unwrap();
    let html = read_back.container.read(&page.href).unwrap();
    let html = String::from_utf8_lossy(&html);
    assert!(html.contains("Round trip SNB content"), "{html}");
}
