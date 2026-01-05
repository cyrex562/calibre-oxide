use calibre_ebooks::oeb::normalize_css::normalize_edge;

#[test]
fn test_normalize_edge_margin() {
    let result = normalize_edge("margin", "10px");
    assert_eq!(result.get("margin-top").unwrap(), "10px");
    assert_eq!(result.get("margin-right").unwrap(), "10px");
    assert_eq!(result.get("margin-bottom").unwrap(), "10px");
    assert_eq!(result.get("margin-left").unwrap(), "10px");

    let result = normalize_edge("margin", "10px 20px");
    assert_eq!(result.get("margin-top").unwrap(), "10px");
    assert_eq!(result.get("margin-right").unwrap(), "20px");
    assert_eq!(result.get("margin-bottom").unwrap(), "10px");
    assert_eq!(result.get("margin-left").unwrap(), "20px");

    let result = normalize_edge("margin", "10px 20px 30px");
    assert_eq!(result.get("margin-top").unwrap(), "10px");
    assert_eq!(result.get("margin-right").unwrap(), "20px");
    assert_eq!(result.get("margin-bottom").unwrap(), "30px");
    assert_eq!(result.get("margin-left").unwrap(), "20px");

    let result = normalize_edge("margin", "10px 20px 30px 40px");
    assert_eq!(result.get("margin-top").unwrap(), "10px");
    assert_eq!(result.get("margin-right").unwrap(), "20px");
    assert_eq!(result.get("margin-bottom").unwrap(), "30px");
    assert_eq!(result.get("margin-left").unwrap(), "40px");
}

#[test]
fn test_normalize_edge_border_style() {
    // Check handling of hyphenated property prefixes roughly (normalize_edge supports it)
    let result = normalize_edge("border-style", "solid dashed");
    assert_eq!(result.get("border-top-style").unwrap(), "solid");
    assert_eq!(result.get("border-right-style").unwrap(), "dashed");
    assert_eq!(result.get("border-bottom-style").unwrap(), "solid");
    assert_eq!(result.get("border-left-style").unwrap(), "dashed");
}
