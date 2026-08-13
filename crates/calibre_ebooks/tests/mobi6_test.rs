mod common;

use calibre_ebooks::compression::palmdoc::compress as palmdoc_compress;
use calibre_ebooks::mobi::mobi6::MobiReader;
use common::{build_mobi6_pdb, Mobi6Options};

#[test]
fn test_mobi_reader_init() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("Test Book", b"<html><body><p>Hi</p></body></html>", &opts);

    let reader = MobiReader::new(&data);
    assert!(
        reader.is_ok(),
        "MobiReader initialization failed: {:?}",
        reader.err()
    );

    let mobi = reader.unwrap();
    assert_eq!(mobi.name, "Test Book");
    assert_eq!(mobi.num_sections, 2);
    assert_eq!(mobi.book_header.codec, "utf-8");
    assert_eq!(mobi.book_header.mobi_version, 6);
    assert!(mobi.kf8_type.is_none());
}

#[test]
fn test_extract_text_uncompressed() {
    let opts = Mobi6Options::new();
    let html = b"<html><body><p>Hello, world!</p></body></html>";
    let data = build_mobi6_pdb("Plain", html, &opts);

    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.extract_text(1).unwrap();
    assert_eq!(mobi.mobi_html, html);
}

#[test]
fn test_extract_text_palmdoc_compressed() {
    let html = b"<html><body>The quick brown fox jumps over the lazy dog. \
        The quick brown fox jumps over the lazy dog again and again.</body></html>";
    let compressed = palmdoc_compress(html).unwrap();

    let mut opts = Mobi6Options::new();
    opts.compression = 2;
    let data = build_mobi6_pdb("Compressed", &compressed, &opts);

    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.extract_text(1).unwrap();
    assert_eq!(mobi.mobi_html, html.to_vec());
}

#[test]
fn test_check_for_drm() {
    let mut opts = Mobi6Options::new();
    opts.encryption_type = 2;
    let data = build_mobi6_pdb("DRM Book", b"<html></html>", &opts);
    let mobi = MobiReader::new(&data).unwrap();
    assert!(mobi.check_for_drm().is_err());

    let opts2 = Mobi6Options::new();
    let data2 = build_mobi6_pdb("Free Book", b"<html></html>", &opts2);
    let mobi2 = MobiReader::new(&data2).unwrap();
    assert!(mobi2.check_for_drm().is_ok());
}

#[test]
fn test_replace_page_breaks() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("PB", b"x", &opts);
    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.processed_html = "before<mbp:pagebreak/>after".to_string();
    mobi.replace_page_breaks();
    assert_eq!(
        mobi.processed_html,
        "before<div  class=\"mbp_pagebreak\" />after"
    );
}

#[test]
fn test_add_anchors_inserts_filepos_targets() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("Anchors", b"x", &opts);
    let mut mobi = MobiReader::new(&data).unwrap();
    // A link pointing at filepos 10, and the target content starting there.
    mobi.mobi_html = b"<a filepos=0000000010>link</a><xxxxxxxxx><p>target</p>".to_vec();
    let out = mobi.add_anchors();
    let out_str = String::from_utf8_lossy(&out);
    assert!(out_str.contains("filepos10"), "{out_str}");
}

#[test]
fn test_cleanup_html_removes_zero_height_divs() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("Cleanup", b"x", &opts);
    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.processed_html = "<p>a</p><div height=\"0pt\"></div><p>b</p>".to_string();
    mobi.cleanup_html();
    assert!(!mobi.processed_html.contains("height=\"0pt\""));
    assert!(mobi.processed_html.contains("<p>a</p>"));
    assert!(mobi.processed_html.contains("<p>b</p>"));
}

#[test]
fn test_extract_content_produces_html_opf_css() {
    let opts = Mobi6Options::new();
    let html = b"<html><head><title>My Book</title></head><body><h1>Chapter 1</h1><p>Once upon a time.</p></body></html>";
    let data = build_mobi6_pdb("My Book", html, &opts);

    let tmp = tempfile::tempdir().unwrap();
    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.extract_content(tmp.path())
        .expect("extract_content should succeed");

    let index_html = std::fs::read_to_string(tmp.path().join("index.html")).unwrap();
    assert!(index_html.contains("<html"), "{index_html}");
    assert!(index_html.contains("Chapter 1"), "{index_html}");
    assert!(index_html.contains("Once upon a time"), "{index_html}");

    let styles = std::fs::read_to_string(tmp.path().join("styles.css")).unwrap();
    assert!(styles.contains("body { text-align: justify }"));

    let opf_path = mobi.created_opf_path.expect("opf path recorded");
    let opf = std::fs::read_to_string(&opf_path).unwrap();
    assert!(opf.contains("<package"), "{opf}");
    assert!(opf.contains("index.html"), "{opf}");
}

#[test]
fn test_extract_content_with_exth_metadata() {
    let mut opts = Mobi6Options::new();
    opts.exth = vec![
        (100, b"Doe, Jane".to_vec()), // author, "Last, First" form
        (101, b"Test Publisher".to_vec()),
        (103, b"A test book.".to_vec()),
    ];
    let html = b"<html><body><p>Content</p></body></html>";
    let data = build_mobi6_pdb("EXTH Book", html, &opts);

    let tmp = tempfile::tempdir().unwrap();
    let mut mobi = MobiReader::new(&data).unwrap();
    mobi.extract_content(tmp.path())
        .expect("extract_content should succeed");

    let exth = mobi.book_header.exth.as_ref().expect("exth parsed");
    assert_eq!(exth.mi.authors, vec!["Jane Doe".to_string()]);
    assert_eq!(exth.mi.publisher.as_deref(), Some("Test Publisher"));
    assert_eq!(exth.mi.comments.as_deref(), Some("A test book."));

    let opf_path = mobi.created_opf_path.unwrap();
    let opf = std::fs::read_to_string(&opf_path).unwrap();
    assert!(opf.contains("Jane Doe"), "{opf}");
}

#[test]
fn test_upshift_markup_converts_presentational_tags() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("Upshift", b"x", &opts);
    let mut mobi = MobiReader::new(&data).unwrap();
    let mut dom =
        calibre_ebooks::mobi::dom::Dom::parse("<html><body><b>bold</b><i>italic</i></body></html>");
    mobi.upshift_markup(&mut dom, &std::collections::HashMap::new());
    let body = dom.find_first_tag_global("body").unwrap();
    let out = dom.serialize(body);
    assert!(out.contains("class=\"bold\""), "{out}");
    assert!(out.contains("class=\"italic\""), "{out}");
    assert!(!out.contains("<b>"), "{out}");
    assert!(!out.contains("<i>"), "{out}");
}

#[test]
fn test_remove_random_bytes() {
    let opts = Mobi6Options::new();
    let data = build_mobi6_pdb("RB", b"x", &opts);
    let mobi = MobiReader::new(&data).unwrap();
    let cleaned = mobi.remove_random_bytes("a\u{14}b\u{01}c");
    assert_eq!(cleaned, "abc");
}
