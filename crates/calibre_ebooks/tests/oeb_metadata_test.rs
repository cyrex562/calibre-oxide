use calibre_ebooks::oeb::metadata::*;
use calibre_ebooks::oeb::parse_utils::*;
use std::collections::HashMap;

#[test]
fn test_metadata_basic() {
    let mut m = Metadata::new();
    m.add("dc:title", "My Book");

    let items = m.get("dc:title");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, "My Book");
}

#[test]
fn test_metadata_attributes() {
    let mut m = Metadata::new();
    let mut attr = HashMap::new();
    attr.insert("role".to_string(), "aut".to_string());
    m.add_with_attrib("dc:creator", "John Doe", attr);

    let items = m.get("dc:creator");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].value, "John Doe");
    assert_eq!(items[0].get_attribute("role").unwrap(), "aut");
}

#[test]
fn test_namespace_helpers() {
    assert_eq!(barename("test"), "test");
    assert_eq!(barename("{http://ns}test"), "test");
    assert_eq!(namespace("{http://ns}test"), "http://ns");
}
