use calibre_ebooks::oeb::constants::XHTML_NS;
use calibre_ebooks::oeb::parse_utils::*;
use roxmltree;

#[test]
fn test_barename() {
    assert_eq!(barename("{http://example.com}tag"), "tag");
    assert_eq!(barename("tag"), "tag");
    assert_eq!(barename("{}tag"), "tag");
}

#[test]
fn test_namespace() {
    assert_eq!(namespace("{http://example.com}tag"), "http://example.com");
    assert_eq!(namespace("tag"), "");
    assert_eq!(namespace("{}tag"), "");
}

#[test]
fn test_xhtml_helper() {
    let tag = XHTML("div");
    assert_eq!(tag, format!("{{{}}}div", XHTML_NS));
}

#[test]
fn test_qualified_name() {
    assert_eq!(qualified_name("http://ns", "tag"), "{http://ns}tag");
}
