use calibre_ebooks::oeb::stylizer::Stylizer;
use roxmltree::Document;

#[test]
fn test_stylizer_inline_style() {
    let xml = r#"<div style="color: red; font-size: 14pt">Text</div>"#;
    let doc = Document::parse(xml).unwrap();
    let root = doc.root_element();

    let stylizer = Stylizer::new(96.0, 12.0);
    let style = stylizer.style(&root);

    assert_eq!(style.color(), "red");
    assert_eq!(style.font_size(), 14.0);
}

#[test]
fn test_stylizer_inheritance() {
    let xml = r#"<div style="color: blue; font-size: 20pt"><p><span>Text</span></p></div>"#;
    let doc = Document::parse(xml).unwrap();
    let root = doc.root_element(); // div
    let p = root.first_element_child().unwrap(); // p
    let span = p.first_element_child().unwrap(); // span

    let stylizer = Stylizer::new(96.0, 12.0);

    // Div
    let div_style = stylizer.style(&root);
    assert_eq!(div_style.color(), "blue");
    assert_eq!(div_style.font_size(), 20.0);

    // Span (inherited)
    let span_style = stylizer.style(&span);
    assert_eq!(span_style.color(), "blue");
    assert_eq!(span_style.font_size(), 20.0);
}

#[test]
fn test_stylizer_em_units() {
    // 10pt base -> 1.5em = 15pt -> 2em = 30pt
    let xml = r#"<div style="font-size: 10pt"><p style="font-size: 1.5em"><span style="font-size: 2em">Text</span></p></div>"#;
    let doc = Document::parse(xml).unwrap();
    let root = doc.root_element(); // div
    let p = root.first_element_child().unwrap(); // p
    let span = p.first_element_child().unwrap(); // span

    let stylizer = Stylizer::new(96.0, 12.0);

    let div_style = stylizer.style(&root);
    assert_eq!(div_style.font_size(), 10.0);

    let p_style = stylizer.style(&p);
    assert_eq!(p_style.font_size(), 15.0);

    let span_style = stylizer.style(&span);
    assert_eq!(span_style.font_size(), 30.0);
}
