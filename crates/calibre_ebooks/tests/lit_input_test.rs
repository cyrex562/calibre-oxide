//! Checks on the LIT reader as a conversion input.
//!
//! Real LIT files cannot be committed, so the fixtures here are written
//! by the ported writer. Cross-validation of the pieces that *can* be
//! compared against calibre — the MS-SHA-1 variant, the DES engine and
//! the LZX codec — lives in `mssha1_cross_test.rs` and, in
//! `calibre_utils`, `msdes_cross_test.rs` and `lzx_cross_test.rs`.

use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::input::lit_input::LitInput;
use calibre_ebooks::lit::reader::{LitContainer, LitFile};
use calibre_ebooks::output::lit_output::LitOutput;
use std::fs;
use std::io::Cursor;
use tempfile::tempdir;

/// Build a LIT file with the given documents, and return its bytes.
fn build_lit(tmp: &std::path::Path, pages: &[(&str, &str)]) -> Vec<u8> {
    let input_dir = tmp.join("input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    for (name, html) in pages {
        fs::write(input_dir.join(name), html).expect("write page");
    }
    let mut book = HTMLInput::new()
        .convert(&input_dir.join(pages[0].0), &input_dir)
        .expect("ingest HTML");
    let out = tmp.join("book.lit");
    LitOutput::new()
        .convert(&mut book, &out)
        .expect("write LIT");
    fs::read(&out).expect("read LIT")
}

#[test]
fn rejects_files_that_are_not_lit() {
    let err = LitFile::new(Cursor::new(b"PK\x03\x04 not a lit".to_vec()), None)
        .err()
        .expect("a zip is not a LIT file");
    assert!(err.to_string().contains("Not a valid LIT file"), "{err}");
}

#[test]
fn rejects_a_truncated_lit_file() {
    let tmp = tempdir().expect("tempdir");
    let mut data = build_lit(
        tmp.path(),
        &[("index.html", "<html><body><p>Hello</p></body></html>")],
    );
    data.truncate(data.len() / 2);
    // Truncation must surface as an error, not a panic.
    let result = LitFile::new(Cursor::new(data), None);
    assert!(result.is_err());
}

#[test]
fn reconstructs_the_opf_from_the_tokenised_meta_entry() {
    let tmp = tempdir().expect("tempdir");
    let data = build_lit(
        tmp.path(),
        &[(
            "index.html",
            "<html><head><title>A Title</title></head><body><p>Body</p></body></html>",
        )],
    );
    let mut container = LitContainer::new(Cursor::new(data), Some("book.lit")).expect("open");
    let opf = container.get_metadata().expect("metadata");
    assert!(opf.contains("<package"), "{opf}");
    assert!(opf.contains("manifest"), "{opf}");
    assert!(opf.contains("spine"), "{opf}");
}

#[test]
fn the_container_lists_and_reads_its_entries() {
    let tmp = tempdir().expect("tempdir");
    let data = build_lit(
        tmp.path(),
        &[("index.html", "<html><body><p>Readable</p></body></html>")],
    );
    let mut container = LitContainer::new(Cursor::new(data), Some("book.lit")).expect("open");

    let names = container.namelist();
    assert!(names.contains(&"book.opf".to_string()), "{names:?}");
    assert!(container.exists("book.opf"));
    assert!(!container.exists("nonexistent.html"));

    let index = names
        .iter()
        .find(|n| n.ends_with(".html") || n.ends_with(".htm"))
        .expect("a document in the manifest")
        .clone();
    let content = container.read(&index).expect("read document");
    let text = String::from_utf8_lossy(&content);
    assert!(text.contains("Readable"), "{text}");
    // Reconstructed documents carry the OEB 1.0.1 doctype.
    assert!(text.contains("oebdoc101.dtd"), "{text}");
}

#[test]
fn conversion_extracts_every_manifest_item() {
    let tmp = tempdir().expect("tempdir");
    let input_dir = tmp.path().join("input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    fs::write(
        input_dir.join("index.html"),
        "<html><head><link rel=\"stylesheet\" href=\"s.css\" /></head>\
         <body><p>One</p></body></html>",
    )
    .expect("write index");
    fs::write(input_dir.join("s.css"), "p { color: red }").expect("write css");

    let mut book = HTMLInput::new()
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("ingest HTML");
    book.manifest.add("css", "s.css", "text/css");

    let lit_path = tmp.path().join("book.lit");
    LitOutput::new()
        .convert(&mut book, &lit_path)
        .expect("write LIT");

    let extracted = tmp.path().join("extracted");
    let read_back = LitInput::new()
        .convert(&lit_path, &extracted)
        .expect("convert LIT");

    assert!(extracted.join("book.opf").exists(), "no OPF was written");
    for item in read_back.manifest.items.values() {
        assert!(
            extracted.join(&item.href).exists(),
            "{} was not extracted",
            item.href
        );
    }
    let css = fs::read_to_string(extracted.join("s.css")).expect("css");
    assert_eq!(css, "p { color: red }");
}
