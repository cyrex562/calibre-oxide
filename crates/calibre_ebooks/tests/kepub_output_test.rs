//! End-to-end test for the KEPUB output plugin (#812).
//!
//! Goes through the registry rather than calling the engine directly:
//! the whole point of the issue was that engines worked while nothing
//! could reach them, so a test that bypasses the registry would not have
//! caught the actual gap.

use calibre_ebooks::conversion::options::ConversionOptions;
use calibre_ebooks::conversion::output_plugin::{builtin_output_registry, resolve_output_plugin};
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use std::fs;
use tempfile::tempdir;

fn a_minimal_book(source: &std::path::Path) -> OEBBook {
    fs::write(source.join("page.html"), "<html><body><p>A sentence. And another one.</p></body></html>").unwrap();
    let container = Box::new(DirContainer::new(source));
    let mut book = OEBBook::new(container);
    book.manifest.add("page", "page.html", "application/xhtml+xml");
    book.spine.add("page", true);
    book.metadata.add("title", "Kobo Test");
    book.metadata.add("language", "en");
    book
}

#[test]
fn kepub_output_is_reachable_through_the_registry() {
    let plugin = resolve_output_plugin(builtin_output_registry(), "kepub").expect("KEPUB output should resolve");
    assert_eq!(plugin.name(), "KEPUB Output");
}

/// The real thing: a book goes in, a Kobo-flavoured EPUB comes out at the
/// path asked for.
#[test]
fn kepub_output_writes_a_zip_with_kobo_markup() {
    let source = tempdir().unwrap();
    let mut book = a_minimal_book(source.path());

    let out = tempdir().unwrap();
    let output_path = out.path().join("Kobo Test.kepub");

    let plugin = resolve_output_plugin(builtin_output_registry(), "kepub").unwrap();
    plugin.convert(&mut book, &output_path, &ConversionOptions::default()).expect("KEPUB conversion failed");

    assert!(output_path.exists(), "the book must land at the path the caller asked for, not a uniquified sibling");

    let file = fs::File::open(&output_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).expect("a kepub is a zip");

    // Kobo's reader requires its own script; its presence is the cheapest
    // real signal that this is a kepub rather than a plain EPUB.
    let names: Vec<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();
    assert!(names.iter().any(|n| n.contains("kobo")), "no Kobo asset in the archive: {names:?}");
    assert!(names.iter().any(|n| n.ends_with(".opf")), "no OPF in the archive: {names:?}");
}

/// Kobo's span markup is what makes a kepub a kepub -- it is how the
/// device tracks reading position per sentence.
#[test]
fn the_html_gains_kobo_spans() {
    let source = tempdir().unwrap();
    let mut book = a_minimal_book(source.path());

    let out = tempdir().unwrap();
    let output_path = out.path().join("spans.kepub");

    let plugin = resolve_output_plugin(builtin_output_registry(), "kepub").unwrap();
    plugin.convert(&mut book, &output_path, &ConversionOptions::default()).unwrap();

    let file = fs::File::open(&output_path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut found = false;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        if !entry.name().ends_with(".html") && !entry.name().ends_with(".xhtml") {
            continue;
        }
        let mut text = String::new();
        use std::io::Read;
        if entry.read_to_string(&mut text).is_ok() && text.contains("koboSpan") {
            found = true;
            break;
        }
    }
    assert!(found, "no koboSpan in any html entry -- the output is a plain EPUB with a .kepub name");
}
