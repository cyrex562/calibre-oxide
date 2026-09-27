//! Black-box tests of the console tools wired up by #813.
//!
//! Run as real subprocesses, for the reason `calibredb.rs` gives: an
//! in-process test of the engine stays green while the binary is missing
//! or its argument parsing is wrong, which is exactly the gap these tools
//! existed in. Every engine here was already merged and tested; what was
//! untested was whether anyone could *run* it.

use std::io::Write;
use std::process::Command;

use calibre_oxide_e2e::oxide_bin;

/// A real, minimal, valid EPUB — enough for `EpubContainer::open_zip` to
/// accept and for polishing to have something to act on.
fn write_minimal_epub(path: &std::path::Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    // `mimetype` must be first and stored uncompressed — that is what
    // makes a zip an EPUB rather than just a zip.
    zip.start_file("mimetype", zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    let deflated = zip::write::FileOptions::default();
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();

    zip.start_file("content.opf", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>A Polished Book</dc:title><dc:creator>Someone</dc:creator><dc:language>en</dc:language><dc:identifier id="uid">urn:uuid:test</dc:identifier></metadata><manifest><item id="c1" href="c1.html" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();

    zip.start_file("c1.html", deflated).unwrap();
    // Straight quotes and a double hyphen, so --smarten-punctuation has
    // real work to do rather than reporting a no-op.
    zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><head><title>One</title></head><body><p>She said "hello" -- and left...</p></body></html>"#).unwrap();

    zip.finish().unwrap();
}

#[test]
fn every_console_tool_has_a_usable_help() {
    for tool in ["ebook-polish", "fetch-ebook-metadata", "web2disk", "calibre-smtp"] {
        let out = Command::new(oxide_bin(tool)).arg("--help").output().unwrap_or_else(|e| panic!("could not run {tool}: {e}"));
        assert!(out.status.success(), "{tool} --help failed: {}", String::from_utf8_lossy(&out.stderr));
        assert!(!out.stdout.is_empty(), "{tool} --help printed nothing");
    }
}

/// The real thing: an EPUB goes in, a changed EPUB comes out.
#[test]
fn ebook_polish_smartens_punctuation_in_a_real_epub() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    write_minimal_epub(&book);
    let before = std::fs::read(&book).unwrap();

    let out = Command::new(oxide_bin("ebook-polish")).arg(&book).arg("--smarten-punctuation").output().unwrap();
    assert!(out.status.success(), "ebook-polish failed: {}", String::from_utf8_lossy(&out.stderr));

    let after = std::fs::read(&book).unwrap();
    assert_ne!(before, after, "the book should have been rewritten in place");
}

/// `--dry-run` must report without writing. A polish tool that always
/// writes is one nobody can safely explore with.
#[test]
fn ebook_polish_dry_run_leaves_the_file_alone() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    write_minimal_epub(&book);
    let before = std::fs::read(&book).unwrap();

    let out = Command::new(oxide_bin("ebook-polish")).arg(&book).arg("--smarten-punctuation").arg("--dry-run").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(std::fs::read(&book).unwrap(), before, "--dry-run must not rewrite the book");
}

/// `--output` writes elsewhere and leaves the original untouched.
#[test]
fn ebook_polish_can_write_to_a_separate_file() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    let destination = dir.path().join("polished.epub");
    write_minimal_epub(&book);
    let before = std::fs::read(&book).unwrap();

    let out = Command::new(oxide_bin("ebook-polish")).arg(&book).arg("--smarten-punctuation").arg("--output").arg(&destination).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    assert!(destination.is_file(), "the output file should exist");
    assert_eq!(std::fs::read(&book).unwrap(), before, "the input must be left alone when --output is given");
}

/// Asking for nothing is refused rather than silently succeeding: with no
/// options the engine changes nothing and returns Ok, so the tool would
/// look like it worked while doing no work.
#[test]
fn ebook_polish_refuses_when_no_work_was_requested() {
    let dir = tempfile::tempdir().unwrap();
    let book = dir.path().join("book.epub");
    write_minimal_epub(&book);

    let out = Command::new(oxide_bin("ebook-polish")).arg(&book).output().unwrap();
    assert!(!out.status.success(), "polishing with no options should be an error");
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to do"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn fetch_ebook_metadata_refuses_an_empty_query() {
    let out = Command::new(oxide_bin("fetch-ebook-metadata")).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("nothing to search for"), "{}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn web2disk_refuses_a_non_http_url() {
    let out = Command::new(oxide_bin("web2disk")).arg("ftp://example.invalid/x").output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an http(s) URL"), "{}", String::from_utf8_lossy(&out.stderr));
}

/// A bad regex has to be reported as a bad regex, not as a failed fetch —
/// otherwise the user retries against the network for no reason.
#[test]
fn web2disk_reports_a_bad_regex_before_touching_the_network() {
    let out = Command::new(oxide_bin("web2disk")).arg("http://example.invalid/").arg("--match-regexp").arg("[unclosed").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("match-regexp"), "the error should name the offending option: {stderr}");
}

#[test]
fn calibre_smtp_requires_a_relay() {
    let out = Command::new(oxide_bin("calibre-smtp")).arg("--from").arg("a@example.invalid").arg("--to").arg("b@example.invalid").arg("--text").arg("hi").output().unwrap();
    assert!(!out.status.success(), "direct-to-MX delivery is not implemented, so --relay is required");
}

#[test]
fn calibre_smtp_rejects_an_unknown_encryption_mode() {
    let out = Command::new(oxide_bin("calibre-smtp"))
        .args(["--from", "a@example.invalid", "--to", "b@example.invalid", "--text", "hi", "--relay", "mail.example.invalid", "--encryption", "rot13"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown --encryption"), "{}", String::from_utf8_lossy(&out.stderr));
}
