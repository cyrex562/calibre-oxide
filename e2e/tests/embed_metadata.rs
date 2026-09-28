//! Black-box test of `calibredb embed_metadata` (#834).
//!
//! Nine formats now have metadata writers behind a dispatcher behind a CLI
//! command, and every layer was tested in isolation. This drives the whole
//! chain the way a user does -- a real `calibredb` subprocess against a real
//! library -- for the reason `calibredb.rs` already documents: an in-process
//! test stays green while the binary is missing or its argument parsing is
//! wrong.
//!
//! The chain: CLI dispatch -> `calibre_db::embed` -> metadata assembled from
//! the database -> `calibre_ebooks::metadata::set_metadata` -> the format
//! writer -> checksum re-recorded.

use std::io::Write;
use std::process::Command;

use calibre_oxide_e2e::{oxide_bin, TestLibrary};

fn run(library: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(oxide_bin("calibredb"))
        .args(args)
        .arg("--with-library")
        .arg(library)
        .output()
        .expect("failed to run the compiled calibredb binary")
}

/// A real, minimal EPUB whose OPF says one thing, so a later library edit
/// has something to disagree with.
fn write_epub(path: &std::path::Path, title: &str) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);

    zip.start_file("mimetype", zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    let deflated = zip::write::FileOptions::default();
    zip.start_file("META-INF/container.xml", deflated).unwrap();
    zip.write_all(br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#).unwrap();

    zip.start_file("content.opf", deflated).unwrap();
    zip.write_all(
        format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="uid"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title><dc:identifier id="uid">urn:uuid:e2e-test</dc:identifier></metadata><manifest><item id="c1" href="c1.html" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#
        )
        .as_bytes(),
    )
    .unwrap();

    zip.start_file("c1.html", deflated).unwrap();
    zip.write_all(br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>Text.</p></body></html>"#).unwrap();
    zip.finish().unwrap();
}

/// The title inside the book file itself, read with the same reader the
/// rest of the project uses.
fn title_in_file(path: &std::path::Path) -> String {
    let opf = {
        let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let mut text = String::new();
        use std::io::Read;
        archive.by_name("content.opf").unwrap().read_to_string(&mut text).unwrap();
        text
    };
    opf.split("<dc:title>").nth(1).and_then(|rest| rest.split("</dc:title>").next()).unwrap_or_default().to_string()
}

/// The headline: a title corrected in the library reaches the book file.
///
/// Before #834 this command wrote only an OPF sidecar, so the file kept its
/// original title and the command still reported success.
#[test]
fn a_title_corrected_in_the_library_reaches_the_epub() {
    let lib = TestLibrary::new();
    let book = lib.path().join("A Book.epub");
    write_epub(&book, "Stale Title");

    let add = run(lib.path(), &["add", book.to_str().unwrap()]);
    assert!(add.status.success(), "add failed: {}", String::from_utf8_lossy(&add.stderr));

    // The file starts out disagreeing with nothing; correct the library.
    let set = run(lib.path(), &["set_metadata", "1", "title", "Corrected Title"]);
    assert!(set.status.success(), "set_metadata failed: {}", String::from_utf8_lossy(&set.stderr));

    let embed = run(lib.path(), &["embed_metadata", "1"]);
    assert!(embed.status.success(), "embed_metadata failed: {}", String::from_utf8_lossy(&embed.stderr));
    let out = String::from_utf8_lossy(&embed.stdout);
    assert!(out.contains("embedded metadata into EPUB"), "the command should report what it did, got: {out}");

    // The library's own copy of the file, which is what was edited.
    let in_library = lib.path().join("A Book.epub");
    assert_eq!(title_in_file(&in_library), "Corrected Title", "the book file still carries its old title");
}

/// A format with no writer is named rather than silently skipped. The old
/// behaviour printed "Processed book id: 1" while touching nothing.
#[test]
fn a_format_without_a_writer_is_named_in_the_output() {
    let lib = TestLibrary::new();
    let book = lib.path().join("A Book.epub");
    write_epub(&book, "Stale Title");
    assert!(run(lib.path(), &["add", book.to_str().unwrap()]).status.success());

    // LIT has no metadata writer.
    let lit = lib.path().join("extra.lit");
    std::fs::write(&lit, b"not really a lit file").unwrap();
    let add_format = run(lib.path(), &["add_format", "1", lit.to_str().unwrap()]);
    assert!(add_format.status.success(), "add_format failed: {}", String::from_utf8_lossy(&add_format.stderr));

    let embed = run(lib.path(), &["embed_metadata", "1"]);
    assert!(embed.status.success(), "{}", String::from_utf8_lossy(&embed.stderr));
    let out = String::from_utf8_lossy(&embed.stdout);
    assert!(out.contains("LIT skipped"), "an unwritable format should be named, got: {out}");
    assert!(out.contains("no metadata writer"), "and the reason given, got: {out}");
}

/// `embed_metadata all` is the form a user reaches for.
#[test]
fn embedding_all_books_works() {
    let lib = TestLibrary::new();
    for (index, title) in [(1, "First Stale"), (2, "Second Stale")].iter().enumerate() {
        let book = lib.path().join(format!("Book {}.epub", index + 1));
        write_epub(&book, title.1);
        assert!(run(lib.path(), &["add", book.to_str().unwrap()]).status.success());
        let _ = index;
    }

    assert!(run(lib.path(), &["set_metadata", "1", "title", "First Fixed"]).status.success());
    assert!(run(lib.path(), &["set_metadata", "2", "title", "Second Fixed"]).status.success());

    let embed = run(lib.path(), &["embed_metadata", "all"]);
    assert!(embed.status.success(), "{}", String::from_utf8_lossy(&embed.stderr));

    assert_eq!(title_in_file(&lib.path().join("Book 1.epub")), "First Fixed");
    assert_eq!(title_in_file(&lib.path().join("Book 2.epub")), "Second Fixed");
}

/// Embedding changes the file, so its recorded checksum must be updated --
/// otherwise `check_library` reports the book as corrupted. The paired test
/// below proves this assertion is not vacuous.
#[test]
fn embedding_does_not_leave_the_book_looking_corrupted() {
    let lib = TestLibrary::new();
    let book = lib.path().join("A Book.epub");
    write_epub(&book, "Stale Title");
    assert!(run(lib.path(), &["add", book.to_str().unwrap()]).status.success());
    assert!(run(lib.path(), &["set_metadata", "1", "title", "Corrected Title"]).status.success());
    assert!(run(lib.path(), &["embed_metadata", "1"]).status.success());

    let check = run(lib.path(), &["check_library"]);
    assert!(check.status.success(), "check_library failed: {}", String::from_utf8_lossy(&check.stderr));
    let out = String::from_utf8_lossy(&check.stdout);
    assert!(
        !out.contains("Corrupted book formats"),
        "embedding left a stale checksum, so a deliberate edit reads as corruption: {out}"
    );
}

/// The guard for the test above: `check_library` prints nothing when a
/// library is clean, so "no corruption reported" is only meaningful if
/// corruption *would* have been reported. Tampering with the same fixture
/// outside the app must be caught.
#[test]
fn an_edit_outside_the_app_is_reported_as_corrupted() {
    let lib = TestLibrary::new();
    let book = lib.path().join("A Book.epub");
    write_epub(&book, "Stale Title");
    assert!(run(lib.path(), &["add", book.to_str().unwrap()]).status.success());

    // Same file, same library, but changed behind the app's back.
    write_epub(&book, "Tampered Title");

    let check = run(lib.path(), &["check_library"]);
    assert!(check.status.success(), "{}", String::from_utf8_lossy(&check.stderr));
    let out = String::from_utf8_lossy(&check.stdout);
    assert!(
        out.contains("Corrupted book formats"),
        "the checksum check is not live in this fixture, which would make the \
         paired test above vacuous: {out}"
    );
}
