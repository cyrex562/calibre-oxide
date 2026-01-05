use calibre_ebooks::metadata::authors::{author_to_author_sort, remove_bracketed_text};
use std::collections::HashSet;

#[test]
fn test_remove_bracketed_text() {
    assert_eq!(remove_bracketed_text("a[b]c(d)e{f}g<h>i"), "aceg<h>i");
    assert_eq!(
        remove_bracketed_text("a[[b]c(d)e{f}]g(h(i)j[k]l{m})n{{{o}}}p"),
        "agnp"
    );

    // Mismatched
    assert_eq!(remove_bracketed_text("a[b(c]d)e"), "ae");
    assert_eq!(remove_bracketed_text("a{b(c}d)e"), "ae");

    // Extra closed
    assert_eq!(remove_bracketed_text("a]b}c)d"), "abcd");
    assert_eq!(remove_bracketed_text("a[b]c]d(e)f{g)h}i}j)k]l"), "acdfijkl");

    // Unclosed
    assert_eq!(remove_bracketed_text("a]b[c"), "ab");
    assert_eq!(remove_bracketed_text("a(b[c]d{e}f"), "a");
    assert_eq!(remove_bracketed_text("a{b}c{d[e]f(g)h"), "ac");
}

fn check_all_methods(
    name: &str,
    invert: Option<&str>,
    comma: Option<&str>,
    nocomma: Option<&str>,
    copy: Option<&str>,
    copywords: Option<&HashSet<String>>,
    use_surname_prefixes: Option<bool>,
    surname_prefixes: Option<&HashSet<String>>,
) {
    let invert_expected = invert.unwrap_or(name);
    let comma_expected = comma.unwrap_or(invert_expected);
    let nocomma_expected = nocomma.unwrap_or(comma_expected);
    let copy_expected = copy.unwrap_or(name);

    assert_eq!(
        author_to_author_sort(
            name,
            Some("invert"),
            copywords,
            use_surname_prefixes,
            surname_prefixes,
            None,
            None
        ),
        invert_expected,
        "Method: invert"
    );
    assert_eq!(
        author_to_author_sort(
            name,
            Some("copy"),
            copywords,
            use_surname_prefixes,
            surname_prefixes,
            None,
            None
        ),
        copy_expected,
        "Method: copy"
    );
    assert_eq!(
        author_to_author_sort(
            name,
            Some("comma"),
            copywords,
            use_surname_prefixes,
            surname_prefixes,
            None,
            None
        ),
        comma_expected,
        "Method: comma"
    );
    assert_eq!(
        author_to_author_sort(
            name,
            Some("nocomma"),
            copywords,
            use_surname_prefixes,
            surname_prefixes,
            None,
            None
        ),
        nocomma_expected,
        "Method: nocomma"
    );
}

#[test]
fn test_single() {
    check_all_methods("Aristotle", None, None, None, None, None, None, None);
}

#[test]
fn test_all_prefix() {
    check_all_methods("Mr. Dr Prof.", None, None, None, None, None, None, None);
}

#[test]
fn test_all_suffix() {
    check_all_methods("Senior Inc", None, None, None, None, None, None, None);
}

#[test]
fn test_copywords() {
    check_all_methods(
        "Don \"Team\" Smith",
        Some("Smith, Don \"Team\""), // invert
        Some("Smith, Don \"Team\""), // comma defaults to invert
        Some("Smith Don \"Team\""),  // nocomma
        None,                        // copy defaults to name
        None,
        None,
        None,
    );
    check_all_methods("Don Team Smith", None, None, None, None, None, None, None);
}

#[test]
fn test_national() {
    // Check with "National" in copywords (default)
    check_all_methods("National Lampoon", None, None, None, None, None, None, None);

    // Remove "National" from copywords
    let mut cw: HashSet<String> = [
        "Agency",
        "Corporation",
        "Company",
        "Co.",
        "Council",
        "Committee",
        "Inc.",
        "Institute",
        "Society",
        "Club",
        "Team",
        "Software",
        "Games",
        "Entertainment",
        "Media",
        "Studios",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    check_all_methods(
        "National Lampoon",
        Some("Lampoon, National"),
        Some("Lampoon, National"),
        Some("Lampoon National"),
        None,
        Some(&cw),
        None,
        None,
    );
}

#[test]
fn test_method() {
    check_all_methods(
        "Jane Doe",
        Some("Doe, Jane"),
        Some("Doe, Jane"),
        Some("Doe Jane"),
        None,
        None,
        None,
        None,
    );
}

#[test]
fn test_invalid_method() {
    // defaults to invert? Python code says "if method is None: default. if method==copy: return... if method==comma... "
    // If unknown method, it falls through to logic which behaves like 'invert' (or 'comma' without actual comma).
    let name = "Jane, Q. van Doe[ed] Jr.";
    let res = author_to_author_sort(name, Some("__unknown__"), None, None, None, None, None);
    let invert = author_to_author_sort(name, Some("invert"), None, None, None, None, None);
    assert_eq!(res, invert);
}

#[test]
fn test_prefix_suffix() {
    check_all_methods(
        "Mrs. Jane Q. Doe III",
        Some("Doe, Jane Q. III"),
        Some("Doe, Jane Q. III"),
        Some("Doe Jane Q. III"),
        None,
        None,
        None,
        None,
    );
}

#[test]
fn test_surname_prefix() {
    // With prefixes enabled
    check_all_methods(
        "Leonardo Da Vinci",
        Some("Da Vinci, Leonardo"),
        Some("Da Vinci, Leonardo"),
        Some("Da Vinci Leonardo"),
        None,
        None,
        Some(true),
        None,
    );
    check_all_methods(
        "Liam Da Mathúna",
        Some("Da Mathúna, Liam"),
        Some("Da Mathúna, Liam"),
        Some("Da Mathúna Liam"),
        None,
        None,
        Some(true),
        None,
    );
    check_all_methods("Van Gogh", None, None, None, None, None, Some(true), None);
    check_all_methods("Van", None, None, None, None, None, Some(true), None);

    // With prefixes disabled (default false in my impl?) verify standard logic
    check_all_methods(
        "Leonardo Da Vinci",
        Some("Vinci, Leonardo Da"),
        Some("Vinci, Leonardo Da"),
        Some("Vinci Leonardo Da"),
        None,
        None,
        Some(false),
        None,
    );
    check_all_methods(
        "Van Gogh",
        Some("Gogh, Van"),
        Some("Gogh, Van"),
        Some("Gogh Van"),
        None,
        None,
        Some(false),
        None,
    );
}

#[test]
fn test_comma() {
    check_all_methods(
        "James Wesley, Rawles",
        Some("Rawles, James Wesley,"), // invert behaves like this if no comma protection
        Some("James Wesley, Rawles"),  // comma method preserves existing comma
        Some("Rawles James Wesley,"),  // nocomma
        None,
        None,
        None,
        None,
    );
}

#[test]
fn test_brackets() {
    check_all_methods(
        "Seventh Author [7]",
        Some("Author, Seventh"),
        Some("Author, Seventh"),
        Some("Author Seventh"),
        None,
        None,
        None,
        None,
    );
    check_all_methods(
        "John [x]von Neumann (III)",
        Some("Neumann, John von"),
        Some("Neumann, John von"),
        Some("Neumann John von"),
        None,
        None,
        None,
        None,
    );
}

#[test]
fn test_falsy() {
    check_all_methods("", Some(""), Some(""), Some(""), Some(""), None, None, None);
}
