use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::writer::OEBWriter;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_oeb_writer_opf_generation() {
    let dir = tempdir().unwrap();
    let container = Box::new(DirContainer::new(dir.path()));
    let mut book = OEBBook::new(container);

    book.metadata.add("dc:title", "My Book");
    book.metadata.add("dc:language", "en");
    book.manifest
        .add("item1", "chapter1.html", "application/xhtml+xml");
    book.spine.add("item1", true);

    let writer = OEBWriter::new();
    let opf = writer.write_opf(&book).unwrap();

    println!("{}", opf);

    assert!(opf.contains("<dc:title>My Book</dc:title>"));
    assert!(opf.contains("<dc:language>en</dc:language>"));
    assert!(opf.contains(
        "<item id=\"item1\" href=\"chapter1.html\" media-type=\"application/xhtml+xml\" />"
    ));
    assert!(opf.contains("<itemref idref=\"item1\" linear=\"yes\" />"));
}

#[test]
fn test_oeb_writer_round_trip_files() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();

    // Setup source
    fs::write(src_dir.path().join("chapter1.html"), "<html>Content</html>").unwrap();

    let container = Box::new(DirContainer::new(src_dir.path()));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("item1", "chapter1.html", "application/xhtml+xml");
    book.spine.add("item1", true);

    // Write
    let writer = OEBWriter::new();
    writer.write_book(&mut book, dst_dir.path()).unwrap();

    // Verify
    assert!(dst_dir.path().join("content.opf").exists());
    assert!(dst_dir.path().join("chapter1.html").exists());

    let content = fs::read_to_string(dst_dir.path().join("chapter1.html")).unwrap();
    assert_eq!(content, "<html>Content</html>");
}
