//! Integration test for `oeb::iterator::book::EbookIterator` (issue
//! #38): build a tiny synthetic EPUB, run it through extraction, and
//! assert the resulting spine/pagination/bookmark round-trip is sane.
//! Mirrors the fixture pattern in `epub_input_test.rs`.

use std::fs;
use std::io::Write;

use calibre_ebooks::oeb::iterator::book::{EbookIterator, EbookIteratorOptions};
use calibre_ebooks::oeb::iterator::bookmarks::{
    Bookmark, BookmarkKind, BookmarkPos, BookmarksMixin,
};
use tempfile::tempdir;
use zip::write::FileOptions;

/// Build a minimal but structurally real two-chapter EPUB with a cross
/// reference, an internal anchor, and a cover image referenced from the
/// guide.
fn make_epub(path: &std::path::Path) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let stored = FileOptions::default().compression_method(zip::CompressionMethod::Stored);

    zip.start_file("mimetype", stored).unwrap();
    zip.write_all(b"application/epub+zip").unwrap();

    zip.add_directory("META-INF", stored).unwrap();
    zip.start_file("META-INF/container.xml", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
   <rootfiles>
      <rootfile full-path="content.opf" media-type="application/oebps-package+xml"/>
   </rootfiles>
</container>"#,
    )
    .unwrap();

    zip.start_file("content.opf", stored).unwrap();
    zip.write_all(
        br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" unique-identifier="dcid" version="2.0">
   <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
      <dc:title>Iterator Test Book</dc:title>
      <dc:language>en</dc:language>
   </metadata>
   <manifest>
      <item id="c1" href="chap1.html" media-type="application/xhtml+xml"/>
      <item id="c2" href="chap2.html" media-type="application/xhtml+xml"/>
      <item id="cover-image" href="cover.jpg" media-type="image/jpeg"/>
   </manifest>
   <spine>
      <itemref idref="c1"/>
      <itemref idref="c2"/>
   </spine>
   <guide>
      <reference type="cover" title="Cover" href="cover.jpg"/>
   </guide>
</package>"#,
    )
    .unwrap();

    zip.start_file("chap1.html", stored).unwrap();
    zip.write_all(
        br#"<html><body><h1 id="top">Chapter One</h1><p>Some opening text that is reasonably long so the character count is non trivial for pagination purposes.</p><p><a href="chap2.html#mid">Go to chapter two</a></p></body></html>"#,
    )
    .unwrap();

    zip.start_file("chap2.html", stored).unwrap();
    zip.write_all(
        br#"<html><body><h1>Chapter Two</h1><p id="mid">Middle anchor text.</p><p>More text to pad out the character count for this second chapter file.</p></body></html>"#,
    )
    .unwrap();

    // A tiny (invalid, but present) "image" -- EbookIterator never
    // decodes it, it only needs to exist as a manifest/guide target.
    zip.start_file("cover.jpg", stored).unwrap();
    zip.write_all(b"not-a-real-jpeg").unwrap();

    zip.finish().unwrap();
}

#[test]
fn opens_extracts_and_builds_paginated_spine() {
    let dir = tempdir().unwrap();
    let epub_path = dir.path().join("iter_test.epub");
    make_epub(&epub_path);

    let it = EbookIterator::open(&epub_path, EbookIteratorOptions::default())
        .expect("EbookIterator::open should succeed on a well-formed EPUB");

    assert_eq!(it.book_format(), "EPUB");
    assert_eq!(it.ebook_ext(), "epub");
    assert!(it.pathtoopf().exists(), "content.opf should be written out");
    assert_eq!(it.language(), Some("en"));

    // Two real chapters in spine order.
    assert_eq!(it.spine().len(), 2);
    assert!(it.spine()[0].path.ends_with("chap1.html"));
    assert!(it.spine()[1].path.ends_with("chap2.html"));

    // Every extracted file actually exists on disk under `base()`.
    for item in it.spine() {
        assert!(item.path.exists(), "{:?} should exist", item.path);
        assert!(item.path.starts_with(it.base()));
    }

    // Pagination: character counts are positive, pages non-negative,
    // and start/max page bookkeeping is monotonic and contiguous.
    assert_eq!(it.pages().len(), 2);
    let mut expected_start = 1i64;
    for item in it.spine() {
        assert!(item.character_count > 0);
        assert_eq!(item.start_page, expected_start);
        assert_eq!(item.max_page, item.start_page + item.pages - 1);
        expected_start += item.pages;
    }

    // The cross-chapter link with a fragment resolves to a verified
    // link pointing at chapter two, since "mid" is a real anchor there.
    let chap1 = &it.spine()[0];
    assert!(chap1.all_links.iter().any(|l| l.contains("chap2.html")));
    assert!(chap1
        .verified_links
        .iter()
        .any(|(p, frag)| p.ends_with("chap2.html") && frag.as_deref() == Some("mid")));

    // No bookmarks saved yet for a book opened for the first time.
    assert!(it.bookmarks().is_empty());
}

#[test]
fn htmlz_input_is_not_accidentally_matched_for_epub() {
    // Sanity check on the input_fmt special-case: an EPUB must not take
    // the "single index.html" branch reserved for htmlz.
    let dir = tempdir().unwrap();
    let epub_path = dir.path().join("book.epub");
    make_epub(&epub_path);
    let it = EbookIterator::open(&epub_path, EbookIteratorOptions::default()).unwrap();
    assert_eq!(it.spine().len(), 2);
}

#[test]
fn bookmarks_round_trip_through_config_store() {
    let dir = tempdir().unwrap();
    let epub_path = dir.path().join("iter_test.epub");
    make_epub(&epub_path);

    let mut opts = EbookIteratorOptions::default();
    // Avoid mutating a shared real EPUB file across parallel test runs;
    // the config-store round trip is what this test is checking.
    opts.copy_bookmarks_to_file = false;
    // A dedicated config-store name (the `decouple`-equivalent) so this
    // test doesn't contend with other tests/processes over the one
    // real, shared `iterator.json` config file.
    let config_name = "iterator_test_bookmarks_round_trip";

    let mut it = EbookIterator::open_with_config_name(&epub_path, opts, config_name).unwrap();
    let bm = Bookmark {
        kind: BookmarkKind::Cfi,
        title: "My spot".to_string(),
        spine: 1,
        pos: BookmarkPos::Number(0.42),
    };
    it.add_bookmark(bm.clone(), false).unwrap();
    assert_eq!(it.bookmarks(), &[bm.clone()]);

    // Re-opening the same book (same canonical path -> same config key)
    // should pick the bookmark back up from the durable store.
    let it2 = EbookIterator::open_with_config_name(&epub_path, opts, config_name).unwrap();
    assert_eq!(it2.bookmarks(), &[bm]);
}

#[test]
fn embeds_bookmarks_into_the_epub_file_when_enabled() {
    let dir = tempdir().unwrap();
    let epub_path = dir.path().join("iter_test.epub");
    make_epub(&epub_path);

    let mut it = EbookIterator::open_with_config_name(
        &epub_path,
        EbookIteratorOptions::default(),
        "iterator_test_embed_epub",
    )
    .unwrap();
    it.add_bookmark(
        Bookmark {
            kind: BookmarkKind::Legacy,
            title: "Legacy spot".to_string(),
            spine: 0,
            pos: BookmarkPos::Text("chap1".to_string()),
        },
        false,
    )
    .unwrap();

    let file = fs::File::open(&epub_path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive
        .by_name("META-INF/calibre_bookmarks.txt")
        .expect("bookmarks should be embedded in the epub file");
    let mut s = String::new();
    std::io::Read::read_to_string(&mut entry, &mut s).unwrap();
    assert!(s.contains("Legacy spot"));
}

#[test]
fn search_finds_text_in_later_spine_item() {
    let dir = tempdir().unwrap();
    let epub_path = dir.path().join("iter_test.epub");
    make_epub(&epub_path);
    let it = EbookIterator::open(&epub_path, EbookIteratorOptions::default()).unwrap();

    let found = it.search("middle anchor", 0, false).unwrap();
    assert_eq!(found, Some(1));

    let not_found = it.search("nonexistent phrase xyz", 0, false).unwrap();
    assert_eq!(not_found, None);

    // Backwards search from the end should find chapter one's text.
    let found_back = it.search("chapter one", 1, true).unwrap();
    assert_eq!(found_back, Some(0));
}
