use calibre_ebooks::input::djvu_input::DJVUInput;
use std::fs;
use tempfile::tempdir;

// Same real chunk-building recipe as `djvu::file`'s own test module
// and `input::djvu_input`'s own inline tests.
const MAGIC: &[u8; 4] = b"AT&T";

fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(id);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    if payload.len() % 2 == 1 {
        out.push(0);
    }
    out
}

fn text_payload(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(text.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(text);
    out
}

fn djvu_with_text(pages: &[&[u8]]) -> Vec<u8> {
    let mut form_body = b"DJVU".to_vec();
    for page in pages {
        form_body.extend_from_slice(&chunk(b"TXTa", &text_payload(page)));
    }
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&chunk(b"FORM", &form_body));
    out
}

#[test]
fn test_djvu_input_conversion() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("test.djvu");
    fs::write(&input_path, djvu_with_text(&[b"Real OCR text"])).unwrap();

    let input = DJVUInput::new();
    let book = input.convert(&input_path, &output_dir).unwrap();

    // Real conversion output (issue #129): the DjVu's own OCR text
    // layer, HTML-ized and fed through the real HTML input plugin --
    // not the old "DJVU Content Not Supported Yet" placeholder page.
    assert!(!book.manifest.items.is_empty());
    let titles = book.metadata.get("title");
    assert_eq!(titles.len(), 1);
    assert_eq!(titles[0].value, "test");

    let mut found_text = false;
    for item in book.manifest.items.values() {
        if let Ok(content) = fs::read_to_string(output_dir.join(&item.href)) {
            if content.contains("Real OCR text") {
                found_text = true;
            }
        }
    }
    assert!(found_text, "the DjVu text layer should survive through to the HTML output");
}

#[test]
fn test_djvu_input_rejects_a_text_less_file() {
    let temp_dir = tempdir().unwrap();
    let output_dir = temp_dir.path().join("out");
    let input_path = temp_dir.path().join("scan.djvu");
    // A FORM:DJVU with no TXT* chunk at all -- pure page scan.
    let mut buf = MAGIC.to_vec();
    buf.extend_from_slice(&chunk(b"FORM", b"DJVU"));
    fs::write(&input_path, &buf).unwrap();

    let result = DJVUInput::new().convert(&input_path, &output_dir);
    let Err(err) = result else { panic!("expected a real error for a text-less DjVu file") };
    assert!(err.to_string().contains("no text"), "{err}");
}
