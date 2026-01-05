use calibre_ebooks::conversion::preprocess::Preprocess;
use calibre_ebooks::conversion::search_replace::SearchReplace;
use calibre_ebooks::conversion::utils;

#[test]
fn test_search_replace() {
    let mut sr = SearchReplace::new();
    sr.add_rule(r"\d+", "#").unwrap(); // Replace numbers with #
    sr.add_rule("foo", "bar").unwrap();

    let input = "foo 123 foo 456";
    let output = sr.process(input);
    assert_eq!(output, "bar # bar #");
}

#[test]
fn test_preprocess_stub() {
    let pp = Preprocess::new();
    let input = "<html><script>alert('x')</script><body>Hi</body></html>";
    let output = pp.remove_scripts(input);
    assert!(output.contains("<!-- <script"));
    assert!(output.contains("</script> -->"));
}

#[test]
fn test_utils() {
    assert_eq!(utils::slugify("Hello World!"), "hello-world");
    assert_eq!(utils::clean_ascii_chars("Héllö"), "Hll");
    // Basic test assumptions: clean_ascii_chars logic in source was: c.is_ascii() || c.is_alphanumeric()
    // 'é' is typically not ascii.
}
