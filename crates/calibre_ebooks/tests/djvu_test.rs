//! End-to-end DjVu tests: a multi-page container built from real
//! BZZ-compressed text chunks, parsed and read back through the public
//! API.
//!
//! The two `TXTz` payloads below were produced by a reference BZZ
//! encoder transliterated from DjVuLibre's `BSEncodeByteStream.cpp` and
//! verified to round-trip through calibre's own
//! `calibre.ebooks.djvu.djvubzzdec.BZZDecoder` — so decoding them here
//! is a check against the Python implementation this crate ports, not
//! against ourselves.

use calibre_ebooks::djvu::{DjvuError, DjvuFile, TEXT_SEPARATOR};

const PAGE1_TEXT: &str = "Call me Ishmael. Some years ago-never mind how long precisely-";
const PAGE2_TEXT: &str = "having little or no money in my purse, and nothing particular";

/// BZZ-compressed text record for [`PAGE1_TEXT`].
const PAGE1_TXTZ: &str = concat!(
    "ffffbdfe96415472b75c8183c283999ff9d86c923543bffea4ea80dd15394c821f84aadd",
    "cc36a632e73845d253fb5a92c098be7b9953c29c17630afb60913f",
);

/// BZZ-compressed text record for [`PAGE2_TEXT`].
const PAGE2_TXTZ: &str = concat!(
    "ffffbeff1ba2cc21de4ba5d243edf6f19bef4ead2fa6d83211b213d84445822eb843b0ad",
    "f2779990dacdf1af22d303a270121701baadc33f",
);

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "hex fixture must have an even length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
        .collect()
}

/// Wrap `payload` in an IFF chunk, padded to an even offset.
fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::from(*id);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn form(subtype: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut payload = Vec::from(*subtype);
    payload.extend_from_slice(body);
    chunk(b"FORM", &payload)
}

/// A two-page `FORM:DJVM` document, each page a `FORM:DJVU` holding one
/// BZZ-compressed text chunk — the shape calibre's DJVU input plugin
/// meets in the wild.
fn two_page_document() -> Vec<u8> {
    let mut pages = Vec::new();
    for txtz in [PAGE1_TXTZ, PAGE2_TXTZ] {
        let mut body = chunk(
            b"INFO",
            &[0x08, 0x2e, 0x0a, 0xa0, 0x18, 0x00, 0x2c, 0x01, 0x16, 0x00],
        );
        body.extend_from_slice(&chunk(b"TXTz", &hex(txtz)));
        pages.extend_from_slice(&form(b"DJVU", &body));
    }
    let mut body = chunk(b"DIRM", &[0x01, 0x00, 0x00, 0x02]);
    body.extend_from_slice(&pages);

    let mut out = Vec::from(*b"AT&T");
    out.extend_from_slice(&form(b"DJVM", &body));
    out
}

#[test]
fn extracts_text_from_a_multi_page_document() {
    let file = DjvuFile::from_bytes(two_page_document()).expect("parses");
    let text = file.text().expect("decodes the text layer");

    let expected = format!(
        "{PAGE1_TEXT}{sep}{PAGE2_TEXT}{sep}",
        sep = TEXT_SEPARATOR as char
    );
    assert_eq!(String::from_utf8(text).expect("UTF-8 text"), expected);
}

#[test]
fn walks_nested_forms() {
    let file = DjvuFile::from_bytes(two_page_document()).expect("parses");
    let root = file.root();
    assert_eq!(root.subtype_str(), Some("DJVM"));

    let ids: Vec<&str> = root.iter().map(|c| c.id_str()).collect();
    assert_eq!(
        ids,
        vec!["FORM", "DIRM", "FORM", "INFO", "TXTz", "FORM", "INFO", "TXTz"]
    );

    let dump = file.dump(10);
    assert!(dump.starts_with("  FORM:DJVM ["), "unexpected dump: {dump}");
    assert_eq!(dump.matches("FORM:DJVU").count(), 2, "dump was: {dump}");
}

#[test]
fn dump_honours_the_depth_limit() {
    let file = DjvuFile::from_bytes(two_page_document()).expect("parses");
    // Depth 1 is the root only; the Python `dump(maxlevel=...)` argument
    // behaves the same way.
    assert_eq!(file.dump(1).lines().count(), 1);
    assert!(file.dump(10).lines().count() > 1);
}

#[test]
fn a_corrupt_text_chunk_is_reported_not_swallowed() {
    // Flip the TXTz payload to bytes that are not a valid BZZ stream.
    let mut body = chunk(b"TXTz", b"this is not BZZ data");
    body.splice(0..0, *b"DJVU");
    let mut raw = Vec::from(*b"AT&T");
    raw.extend_from_slice(&chunk(b"FORM", &body));

    let file = DjvuFile::from_bytes(raw).expect("parses");
    let err = file
        .text()
        .expect_err("must not silently return empty text");
    assert!(matches!(err, DjvuError::Bzz { .. }), "got {err:?}");
}

#[test]
fn reads_a_document_from_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("moby.djvu");
    std::fs::write(&path, two_page_document()).expect("write");

    let file = DjvuFile::open(&path).expect("opens");
    assert!(file
        .text()
        .expect("text")
        .starts_with(PAGE1_TEXT.as_bytes()));
}

#[test]
fn a_missing_file_surfaces_the_io_error() {
    let err = DjvuFile::open("/nonexistent/nope.djvu").expect_err("must fail");
    assert!(matches!(err, DjvuError::Io(_)), "got {err:?}");
}
