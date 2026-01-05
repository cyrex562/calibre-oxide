use calibre_ebooks::conversion::plumber::Plumber;
use std::fs;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_plumber_epub_to_oeb() {
    let tmp_dir = tempdir().unwrap();
    let epub_path = tmp_dir.path().join("test_plumber.epub");
    let output_dir = tmp_dir.path().join("output_oeb");

    // Create a mock EPUB (Simplified)
    {
        let file = fs::File::create(&epub_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = FileOptions::default();

        zip.start_file("mimetype", options).unwrap();
        zip.write_all(b"application/epub+zip").unwrap();

        zip.add_directory("META-INF", options).unwrap();
        zip.start_file("META-INF/container.xml", options).unwrap();
        zip.write_all(r#"<container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0">
             <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
        </container>"#.as_bytes()).unwrap();

        zip.add_directory("OEBPS", options).unwrap();
        zip.start_file("OEBPS/content.opf", options).unwrap();
        zip.write_all(r#"<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
            <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Plumber Test</dc:title></metadata>
            <manifest><item id="p1" href="page.html" media-type="application/xhtml+xml"/></manifest>
            <spine><itemref idref="p1"/></spine>
        </package>"#.as_bytes()).unwrap();

        zip.start_file("OEBPS/page.html", options).unwrap();
        zip.write_all(b"<html>Content</html>").unwrap();
        zip.finish().unwrap();
    }

    // Run Plumber
    let plumber = Plumber::new(&epub_path, &output_dir);
    plumber.run().expect("Plumber failed");

    // Verify Output
    assert!(output_dir.join("content.opf").exists());
    assert!(output_dir.join("OEBPS/page.html").exists());

    let opf = fs::read_to_string(output_dir.join("content.opf")).unwrap();
    assert!(opf.contains("Plumber Test"));
}
