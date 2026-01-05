use calibre_ebooks::conversion::plumber::Plumber;
use std::fs;
use std::io::Write;
use tempfile::tempdir;
use zip::write::FileOptions;

#[test]
fn test_plumber_epub_to_epub() {
    let tmp_dir = tempdir().unwrap();
    let epub_path = tmp_dir.path().join("input.epub");
    let output_path = tmp_dir.path().join("output.epub");

    // Create a valid Mock EPUB
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
            <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Round Trip</dc:title></metadata>
            <manifest><item id="p1" href="page.html" media-type="application/xhtml+xml"/></manifest>
            <spine><itemref idref="p1"/></spine>
        </package>"#.as_bytes()).unwrap();

        zip.start_file("OEBPS/page.html", options).unwrap();
        zip.write_all(b"<html>Content</html>").unwrap();
        zip.finish().unwrap();
    }

    // Run Plumber
    let plumber = Plumber::new(&epub_path, &output_path);
    plumber.run().expect("Plumber failed");

    // Verify Output is ZIP
    assert!(output_path.exists());
    let file = fs::File::open(&output_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();

    // Check META-INF/container.xml existence
    assert!(zip.by_name("META-INF/container.xml").is_ok());
    // Check content.opf existence (Note: OEBWriter writes flat to root usually, need to check if Plumber handles this structure change.
    // OEBWriter writes to root of temp dir. So content.opf will be at root of ZIP.
    // The Input EPUB had it in OEBPS/. The OEBReader reads it. The OEBWriter writes it to ROOT of output.
    // So output EPUB will have content.opf at root. This is valid EPUB if container points to it.
    // My EPUBOutput creates container.xml pointing to content.opf at root.
    assert!(zip.by_name("content.opf").is_ok());
}
