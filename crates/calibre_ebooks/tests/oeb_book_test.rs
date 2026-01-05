use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::NullContainer;

#[test]
fn test_oeb_book_structure() {
    let container = Box::new(NullContainer::new());
    let mut book = OEBBook::new(container);

    // Metadata
    book.metadata.add("title", "Test Book");
    assert_eq!(book.metadata.get("title")[0].value, "Test Book");

    // Manifest
    book.manifest
        .add("item1", "text/chap1.html", "application/xhtml+xml");
    assert!(book.manifest.get_by_id("item1").is_some());
    assert_eq!(
        book.manifest.get_by_href("text/chap1.html").unwrap().id,
        "item1"
    );

    // Spine
    book.spine.add("item1", true);
    assert_eq!(book.spine.items.len(), 1);
    assert_eq!(book.spine.items[0].idref, "item1");

    // Guide
    book.guide
        .add("cover", Some("Cover".to_string()), "text/cover.html");
    assert!(book.guide.get("cover").is_some());

    // TOC
    let mut node = calibre_ebooks::oeb::toc::TOCNode::new(
        Some("Chapter 1".to_string()),
        Some("text/chap1.html".to_string()),
    );
    book.toc.root.add(node);
    assert_eq!(book.toc.root.children.len(), 1);
}
