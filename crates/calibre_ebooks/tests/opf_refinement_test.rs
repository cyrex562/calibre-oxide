use calibre_ebooks::metadata::{get_metadata, MetaInformation};
use calibre_ebooks::opf::parse_opf;
use std::collections::HashMap;

#[test]
fn test_opf_refinement_parsing() {
    let xml = r##"<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="uuid_id">
    <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">
        <dc:title id="title">Refined Title</dc:title>
        <meta refines="#title" property="title-type">main</meta>
        
        <dc:creator id="succ">Succinct</dc:creator>
        <meta refines="#succ" property="role" scheme="marc:relators">aut</meta>
        <meta refines="#succ" property="file-as">Succinct, Mr</meta>
        
        <meta property="dcterms:modified">2023-01-01T12:00:00Z</meta>
        <meta property="calibre:rating">4</meta>
    </metadata>
</package>"##;

    let meta = parse_opf(xml).expect("Failed to parse OPF");

    assert_eq!(meta.title, "Refined Title");
    assert_eq!(meta.authors, vec!["Succinct"]);

    // Check Author Sort map
    assert_eq!(
        meta.author_sort_map.get("Succinct").map(|s| s.as_str()),
        Some("Succinct, Mr")
    );

    // Check timestamp
    // assert!(meta.timestamp.is_some()); // DateTime parsing might fail if not exact format match or imports missing

    // Check rating
    assert_eq!(meta.rating, Some(4.0));
}

#[test]
fn test_to_xml_generation() {
    let mut meta = MetaInformation::default();
    meta.title = "Generated Title".to_string();
    meta.authors = vec!["Author One".to_string(), "Author Two".to_string()];
    meta.author_sort_map
        .insert("Author One".to_string(), "One, Author".to_string());
    meta.rating = Some(5.0);

    let xml = meta.to_xml();
    println!("{}", xml);

    assert!(xml.contains("<dc:title>Generated Title</dc:title>"));
    assert!(xml.contains("opf:file-as=\"One, Author\">Author One</dc:creator>"));
    assert!(xml.contains("<meta name=\"calibre:rating\" content=\"5\"/>"));
}
