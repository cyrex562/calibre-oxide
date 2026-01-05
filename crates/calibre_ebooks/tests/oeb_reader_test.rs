use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::reader::OEBReader;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_oeb_reader_opf_parsing() {
    let dir = tempdir().unwrap();
    let opf_path = dir.path().join("content.opf");

    let opf_content = r#"
    <package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uuid_id">
        <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
            <dc:title>Test Book Title</dc:title>
            <dc:creator opf:role="aut">Test Author</dc:creator>
            <dc:language>en</dc:language>
            <meta name="cover" content="cover-image" />
        </metadata>
        <manifest>
            <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml" />
            <item id="cover-image" href="cover.jpg" media-type="image/jpeg" />
            <item id="content" href="index.html" media-type="application/xhtml+xml" />
        </manifest>
        <spine toc="ncx">
            <itemref idref="content" />
        </spine>
        <guide>
            <reference type="cover" title="Cover" href="cover.jpg" />
        </guide>
    </package>
    "#;

    fs::write(&opf_path, opf_content).unwrap();

    let container = Box::new(DirContainer::new(dir.path()));
    let mut book = OEBBook::new(container);
    let reader = OEBReader::new();

    if let Err(e) = reader.read_opf(&mut book, "content.opf") {
        panic!("Failed to read OPF: {:?}", e);
    }

    println!("Metadata Items: {:?}", book.metadata.items);
    println!("Manifest Items: {:?}", book.manifest.items);
    println!("Spine Items: {:?}", book.spine.items);
    println!("Guide Refs: {:?}", book.guide.references);

    // Verify Metadata
    let titles = book.metadata.get("title");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].value, "Test Book Title");

    let creators = book.metadata.get("creator");
    assert_eq!(creators.len(), 1);
    assert_eq!(creators[0].value, "Test Author");

    let metas = book.metadata.get("cover");
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].value, "cover-image");

    // Verify Manifest
    assert!(book.manifest.get_by_id("ncx").is_some());
    assert!(book.manifest.get_by_id("cover-image").is_some());
    let content_item = book.manifest.get_by_id("content").unwrap();
    assert_eq!(content_item.href, "index.html");
    assert_eq!(content_item.media_type, "application/xhtml+xml");

    // Verify Spine
    assert_eq!(book.spine.items.len(), 1);
    assert_eq!(book.spine.items[0].idref, "content");
    assert!(book.spine.items[0].linear);

    // Verify Guide
    assert!(book.guide.get("cover").is_some());
    assert_eq!(book.guide.get("cover").unwrap().href, "cover.jpg");
}
