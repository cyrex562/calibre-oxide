use calibre_ebooks::mobi::langcodes::{iana2mobi, mobi2iana};

#[test]
fn test_iana2mobi() {
    // 'en' -> (9, 0) -> 00 00 00 09
    assert_eq!(iana2mobi("en"), vec![0, 0, 0, 9]);
    // 'en-US' -> (9, 4) -> 00 00 04 09
    assert_eq!(iana2mobi("en-US"), vec![0, 0, 4, 9]);
    // 'en-us' -> same
    assert_eq!(iana2mobi("en-us"), vec![0, 0, 4, 9]);

    // 'ar-SA' -> (1, 4) -> 00 00 04 01
    assert_eq!(iana2mobi("ar-SA"), vec![0, 0, 4, 1]);

    // Unknown -> 'und' -> (0,0)? No, "und" maps to {None: (0,0)} in our logic if we added it?
    // In python: IANA_MOBI[None] is {None: (0,0)}.
    // if not found, it keeps defaults (0,0).
    // iana2mobi('xyz') -> (0,0) -> 00 00 00 00
    assert_eq!(iana2mobi("xyz"), vec![0, 0, 0, 0]);
}

#[test]
fn test_mobi2iana() {
    // (9, 0) -> "en"
    assert_eq!(mobi2iana(9, 0), "en");
    // (9, 4) -> "en-us"
    assert_eq!(mobi2iana(9, 4), "en-us");

    // (1, 4) -> "ar-sa"
    assert_eq!(mobi2iana(1, 4), "ar-sa");

    // Unknown subcode -> prefix only
    // (9, 99) -> "en" (assuming 99 doesn't exist)
    assert_eq!(mobi2iana(9, 99), "en");

    // Unknown langcode -> "und"
    assert_eq!(mobi2iana(999, 0), "und");
}
