//! End-to-end test for the AZW3 output plugin (#812).
//!
//! Goes through the registry, for the same reason the KEPUB test does:
//! the gap #812 describes is engines that work while nothing can reach
//! them, so a test calling the engine directly would miss it.

use calibre_ebooks::conversion::options::ConversionOptions;
use calibre_ebooks::conversion::output_plugin::{builtin_output_registry, resolve_output_plugin};
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use std::fs;
use tempfile::tempdir;

fn a_minimal_book(source: &std::path::Path) -> OEBBook {
    fs::write(source.join("c1.html"), "<html><body><h1>Chapter One</h1><p>Some real text.</p></body></html>").unwrap();
    let container = Box::new(DirContainer::new(source));
    let mut book = OEBBook::new(container);
    book.manifest.add("c1", "c1.html", "application/xhtml+xml");
    book.spine.add("c1", true);
    book.metadata.add("title", "Kindle Test");
    book.metadata.add("language", "en");
    // KF8's EXTH block requires a publication date and `build_exth`
    // refuses without one -- faithfully, upstream raises the same
    // `missing date or timestamp`. A real conversion always has one by
    // the time it reaches an output plugin, so this is the fixture
    // matching reality rather than a workaround.
    book.metadata.add("date", "2026-09-27");
    book
}

#[test]
fn azw3_output_is_reachable_through_the_registry() {
    let plugin = resolve_output_plugin(builtin_output_registry(), "azw3").expect("AZW3 output should resolve");
    assert_eq!(plugin.name(), "AZW3 Output");
}

/// AZW3 must not collide with the MOBI plugin, which claims
/// `mobi`/`azw`/`prc`. `resolve_output_plugin` returns the *first* match,
/// so an overlapping extension set would silently route one format to the
/// wrong writer.
#[test]
fn azw3_and_mobi_do_not_claim_each_others_extensions() {
    let registry = builtin_output_registry();
    assert_eq!(resolve_output_plugin(registry, "azw3").unwrap().name(), "AZW3 Output");
    for ext in ["mobi", "azw", "prc"] {
        assert_eq!(resolve_output_plugin(registry, ext).unwrap().name(), "MOBI Output", "{ext} should still go to the MOBI writer");
    }
}

/// The real thing: a valid Palm database with MOBI's magic in record 0.
#[test]
fn azw3_output_writes_a_palm_database_with_mobi_magic() {
    let source = tempdir().unwrap();
    let mut book = a_minimal_book(source.path());

    let out = tempdir().unwrap();
    let output_path = out.path().join("Kindle Test.azw3");

    let plugin = resolve_output_plugin(builtin_output_registry(), "azw3").unwrap();
    plugin.convert(&mut book, &output_path, &ConversionOptions::default()).expect("AZW3 conversion failed");

    let bytes = fs::read(&output_path).expect("the file should exist where it was asked for");

    // PalmDB header: 32-byte name, then 8 words, then the type+creator
    // pair at offset 60.
    assert_eq!(&bytes[60..68], b"BOOKMOBI", "not a Palm database of MOBI type");

    // Record 0 carries the MOBI header, whose magic sits 16 bytes in.
    // Its offset is the first record-info entry, at 78.
    let record0_offset = u32::from_be_bytes([bytes[78], bytes[79], bytes[80], bytes[81]]) as usize;
    assert_eq!(&bytes[record0_offset + 16..record0_offset + 20], b"MOBI", "record 0 is not a MOBI header");

    let word_at = |offset: usize| u32::from_be_bytes([bytes[offset], bytes[offset + 1], bytes[offset + 2], bytes[offset + 3]]);

    // What actually makes this KF8 rather than MOBI6 is the MOBI header's
    // *file version*, stamped 8 by `KF8Book::record0` where the joint
    // output's MOBI6 view stamps 6. It sits at MOBI-header offset 36.
    assert_eq!(word_at(record0_offset + 36), 8, "file_version should be 8 for KF8; 6 would mean a MOBI6 header");

    // `book_type` stays 2 for a non-periodical book -- upstream sets
    // `0x101 if mobi_periodical else 2` for KF8 too, so 2 here is correct
    // and not a sign the KF8 path was skipped. Pinned because it is the
    // field one would reach for first and be misled by.
    assert_eq!(word_at(record0_offset + 24), 2, "book_type should be 2 for a non-periodical book");
}

/// `dont_compress` has to reach the KF8 writer. #690 had to go back and
/// fix exactly this for the MOBI path, so it is asserted here rather than
/// assumed.
#[test]
fn dont_compress_reaches_the_kf8_writer() {
    let source = tempdir().unwrap();
    let out = tempdir().unwrap();
    let plugin = resolve_output_plugin(builtin_output_registry(), "azw3").unwrap();

    let mut compressed_opts = ConversionOptions::default();
    compressed_opts.mobi.dont_compress = false;
    let mut book = a_minimal_book(source.path());
    let compressed_path = out.path().join("compressed.azw3");
    plugin.convert(&mut book, &compressed_path, &compressed_opts).unwrap();

    let mut plain_opts = ConversionOptions::default();
    plain_opts.mobi.dont_compress = true;
    let mut book = a_minimal_book(source.path());
    let plain_path = out.path().join("plain.azw3");
    plugin.convert(&mut book, &plain_path, &plain_opts).unwrap();

    // The compression field is a u16 at offset 0 of record 0.
    let compression_of = |path: &std::path::Path| -> u16 {
        let bytes = fs::read(path).unwrap();
        let record0 = u32::from_be_bytes([bytes[78], bytes[79], bytes[80], bytes[81]]) as usize;
        u16::from_be_bytes([bytes[record0], bytes[record0 + 1]])
    };

    assert_eq!(compression_of(&plain_path), 1, "dont_compress should mean compression 1 (none)");
    assert_ne!(compression_of(&compressed_path), compression_of(&plain_path), "the option should change the file");
}
