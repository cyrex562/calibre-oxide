use calibre_ebooks::conversion::options::ConversionOptions;
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::output::mobi_output::MOBIOutput;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_mobi_output_conversion() {
    let tmp_source = tempdir().unwrap();
    let source_path = tmp_source.path();

    // Content
    fs::write(
        source_path.join("page.html"),
        "<h1>MOBI Page</h1><p>Content</p>",
    )
    .unwrap();

    // Book
    let container = Box::new(DirContainer::new(source_path));
    let mut book = OEBBook::new(container);
    book.manifest
        .add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "MOBI Test Book");
    // The real writer2 EXTH builder (issue #34) requires a date or
    // timestamp, matching Python's `build_exth`.
    book.metadata.add("date", "2024-01-01T00:00:00+00:00");

    // Output
    let tmp_out = tempdir().unwrap();
    let output_path = tmp_out.path().join("book.mobi");

    // Convert
    let output = MOBIOutput::new();
    output
        .convert(&book, &output_path, &ConversionOptions::default())
        .expect("Conversion failed");

    // Verify file exists
    assert!(output_path.exists());

    // Basic Size Check
    let meta = fs::metadata(&output_path).unwrap();
    assert!(meta.len() > 100); // Should have headers

    // Future: Use Reader to verify (Circular dependency if we use lib in test? No, lib is under test)
    // We can try to use MobiReader if it's public.
    // use calibre_ebooks::mobi::reader::MobiReader;
    // But MobiReader might expect complex structure.
    // For now, just existence and size check is good initial verification.
}

fn book_for_compression_test(source_path: &std::path::Path) -> OEBBook {
    // Real, repetitive text -- long enough that PalmDOC compression
    // (LZ77-style back-references) actually shrinks it, so
    // `dont_compress` produces a real, observable size difference, not
    // just a different header byte.
    let paragraph = "The quick brown fox jumps over the lazy dog. ".repeat(200);
    fs::write(source_path.join("page.html"), format!("<p>{paragraph}</p>")).unwrap();

    let mut book = OEBBook::new(Box::new(DirContainer::new(source_path)));
    book.manifest.add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "Compression Test Book");
    book.metadata.add("date", "2024-01-01T00:00:00+00:00");
    book
}

/// The PalmDB header is a fixed 78 bytes, immediately followed by an
/// 8-byte record-info entry per record -- the first 4 bytes of the
/// very first entry (at byte offset 78) are record 0's own absolute
/// offset into the file. Record 0 opens with the PalmDOC header,
/// whose own first 2 bytes are the compression type
/// (`crate::mobi::writer2::mod::{UNCOMPRESSED, PALMDOC}`). See
/// `MobiWriter::write_header`/`generate_record0` for exactly where
/// each of these bytes gets written.
fn record0_compression_field(mobi_bytes: &[u8]) -> u16 {
    let record0_offset = u32::from_be_bytes(mobi_bytes[78..82].try_into().unwrap()) as usize;
    u16::from_be_bytes(mobi_bytes[record0_offset..record0_offset + 2].try_into().unwrap())
}

/// The real, end-to-end proof issue #690 asked for: a real
/// `ConversionOptions` field (`opts.mobi.dont_compress`, threaded from
/// `MOBIOutput::convert`'s new `opts` parameter into a real
/// `MobiWriterOpts` -- previously always `MobiWriterOpts::default()`,
/// silently discarding this exact option) really changes the bytes
/// `MOBIOutput` writes, not just a struct field nobody reads yet.
#[test]
fn dont_compress_option_really_disables_palmdoc_compression_and_changes_the_output() {
    const UNCOMPRESSED: u16 = 1;
    const PALMDOC: u16 = 2;

    let tmp_compressed = tempdir().unwrap();
    let compressed_path = tmp_compressed.path().join("compressed.mobi");
    MOBIOutput::new().convert(&book_for_compression_test(tmp_compressed.path()), &compressed_path, &ConversionOptions::default()).expect("compressed conversion failed");
    let compressed_bytes = fs::read(&compressed_path).unwrap();

    let tmp_uncompressed = tempdir().unwrap();
    let uncompressed_path = tmp_uncompressed.path().join("uncompressed.mobi");
    let mut opts = ConversionOptions::default();
    opts.mobi.dont_compress = true;
    MOBIOutput::new().convert(&book_for_compression_test(tmp_uncompressed.path()), &uncompressed_path, &opts).expect("uncompressed conversion failed");
    let uncompressed_bytes = fs::read(&uncompressed_path).unwrap();

    assert_eq!(record0_compression_field(&compressed_bytes), PALMDOC, "default options should still produce PalmDOC-compressed output");
    assert_eq!(record0_compression_field(&uncompressed_bytes), UNCOMPRESSED, "opts.mobi.dont_compress=true should really turn compression off");

    // Real, repetitive text compresses -- the uncompressed file should
    // be meaningfully larger, not just tagged differently.
    assert!(
        uncompressed_bytes.len() > compressed_bytes.len(),
        "uncompressed ({} bytes) should be larger than compressed ({} bytes) for real repetitive text",
        uncompressed_bytes.len(),
        compressed_bytes.len()
    );
}
