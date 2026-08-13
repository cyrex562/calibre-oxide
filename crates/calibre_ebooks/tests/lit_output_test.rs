//! End-to-end checks on the LIT writer.
//!
//! The strongest available check is the loop: write a book with
//! [`LitOutput`], then open the result with [`LitInput`] and see the
//! same content come back. That exercises the ITOLITLS header, the
//! directory chunks, LZX compression, DES sealing and the binary
//! tokenisation in both directions.

use calibre_ebooks::input::html_input::HTMLInput;
use calibre_ebooks::input::lit_input::LitInput;
use calibre_ebooks::lit::reader::LitFile;
use calibre_ebooks::output::lit_output::LitOutput;
use std::fs;
use tempfile::tempdir;

/// Build a small book on disk and convert it to LIT.
fn write_sample_lit(dir: &std::path::Path) -> std::path::PathBuf {
    let input_dir = dir.join("input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    let index_html = "<html><head><title>LIT Output Test</title></head>\
         <body><p>LIT Content</p></body></html>";
    fs::write(input_dir.join("index.html"), index_html).expect("write index");

    let mut book = HTMLInput::new()
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("ingest HTML");

    let output_file = dir.join("book.lit");
    LitOutput::new()
        .convert(&mut book, &output_file)
        .expect("write LIT");
    output_file
}

#[test]
fn writes_a_file_the_reader_accepts() {
    let tmp = tempdir().expect("tempdir");
    let output_file = write_sample_lit(tmp.path());
    assert!(output_file.exists());

    let file = fs::File::open(&output_file).expect("open");
    let lit = LitFile::new(file, Some("book.lit")).expect("parse LIT");
    assert_eq!(lit.opf_path, "book.opf");
    // The container bookkeeping every LIT file carries.
    assert!(lit.entries.contains_key("/manifest"));
    assert!(lit.entries.contains_key("/meta"));
    assert!(lit.entries.contains_key("::DataSpace/NameList"));
    assert!(lit
        .entries
        .contains_key("::DataSpace/Storage/MSCompressed/Content"));
    // Nominally sealed, with the all-zero key the Python writes.
    assert_eq!(lit.drmlevel, 1);
}

#[test]
fn the_written_book_reads_back_with_its_content_intact() {
    let tmp = tempdir().expect("tempdir");
    let output_file = write_sample_lit(tmp.path());

    let extracted = tmp.path().join("extracted");
    let book = LitInput::new()
        .convert(&output_file, &extracted)
        .expect("read the LIT back");

    assert!(!book.manifest.items.is_empty(), "manifest came back empty");
    let mut found = false;
    for item in book.manifest.items.values() {
        let data = fs::read(extracted.join(&item.href)).expect("extracted file");
        if String::from_utf8_lossy(&data).contains("LIT Content") {
            found = true;
        }
    }
    assert!(found, "the paragraph text did not survive the round trip");
}

#[test]
fn images_survive_the_round_trip_byte_for_byte() {
    let tmp = tempdir().expect("tempdir");
    let input_dir = tmp.path().join("input");
    fs::create_dir_all(&input_dir).expect("create input dir");

    // A one-pixel PNG; the bytes only need to be stable, not valid.
    let png: Vec<u8> = (0u8..=255).chain(0u8..=255).collect();
    fs::write(input_dir.join("cover.png"), &png).expect("write png");
    fs::write(
        input_dir.join("index.html"),
        "<html><body><img src=\"cover.png\" /></body></html>",
    )
    .expect("write index");

    let mut book = HTMLInput::new()
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("ingest HTML");
    book.manifest.add("cover-img", "cover.png", "image/png");

    let output_file = tmp.path().join("book.lit");
    LitOutput::new()
        .convert(&mut book, &output_file)
        .expect("write LIT");

    let extracted = tmp.path().join("extracted");
    LitInput::new()
        .convert(&output_file, &extracted)
        .expect("read the LIT back");

    let out = fs::read(extracted.join("cover.png")).expect("extracted png");
    assert_eq!(out, png, "image bytes changed in the round trip");
}

#[test]
fn a_book_without_a_cover_is_reported_but_still_written() {
    let tmp = tempdir().expect("tempdir");
    let input_dir = tmp.path().join("input");
    fs::create_dir_all(&input_dir).expect("create input dir");
    fs::write(
        input_dir.join("index.html"),
        "<html><body><p>No cover here.</p></body></html>",
    )
    .expect("write index");

    let mut book = HTMLInput::new()
        .convert(&input_dir.join("index.html"), &input_dir)
        .expect("ingest HTML");

    let output_file = tmp.path().join("book.lit");
    let warnings = LitOutput::new()
        .convert(&mut book, &output_file)
        .expect("write LIT");
    assert!(warnings.iter().any(|w| w.contains("cover")), "{warnings:?}");
    assert!(output_file.exists());
}
