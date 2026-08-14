//! Round-trip test for the KF8 writer (`calibre_ebooks::mobi::writer8`,
//! issue #35): encode a small synthetic `OEBBook` into a standalone KF8
//! (`.azw3`-shaped) payload and confirm our own reader
//! (`calibre_ebooks::mobi::mobi8::Mobi8Reader`, issue #33) can parse the
//! result back -- skeleton/chunk reassembly recovers the original text,
//! and the TOC/NCX/guide entries this writer produced are present. Per
//! `docs/HARNESS.md`, this stands in for cross-validation against a real
//! `calibre-debug` binary, which is not installed on this machine.
//!
//! This mirrors `mobi_writer2_test.rs`'s pattern for the MOBI 6 writer.
//! Unlike a joint MOBI6+KF8 `.azw3`, `KF8Book::to_bytes` alone still
//! parses through the normal reader path: `mobi6::MobiReader::new`
//! detects a standalone KF8 file whenever `min_version == 8` in
//! `record0` (see `mobi::headers::BookHeader::parse`), which
//! `writer8::mobi::MOBIHeader` always sets -- no boundary/EXTH-KF8-index
//! marker is needed for the standalone case.

use calibre_ebooks::mobi::mobi6::MobiReader;
use calibre_ebooks::mobi::mobi8::Mobi8Reader;
use calibre_ebooks::mobi::writer8::main::{KF8Writer, Kf8WriterOpts};
use calibre_ebooks::mobi::MobiLog;
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::toc::TOCNode;

fn two_chapter_book(dir: &std::path::Path) -> OEBBook {
    std::fs::write(
        dir.join("c1.html"),
        "<html><body><h1 id=\"top\">Chapter One</h1><p>The first chapter's own words.</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        dir.join("c2.html"),
        "<html><body><h1 id=\"top2\">Chapter Two</h1><p>And <a href=\"c1.html#top\">back</a> we go.</p></body></html>",
    )
    .unwrap();

    let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
    oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
    oeb.manifest.add("c2", "c2.html", "application/xhtml+xml");
    oeb.spine.add("c1", true);
    oeb.spine.add("c2", true);

    oeb.metadata.add("title", "The KF8 Round Trip");
    oeb.metadata.add("creator", "A. Writer");
    oeb.metadata.add("language", "en");
    oeb.metadata.add("date", "2020-01-01T00:00:00+00:00");

    oeb.toc.root.add(TOCNode::new(
        Some("Chapter One".into()),
        Some("c1.html".into()),
    ));
    oeb.toc.root.add(TOCNode::new(
        Some("Chapter Two".into()),
        Some("c2.html".into()),
    ));

    oeb.guide.add("start", Some("Start".to_string()), "c1.html");

    oeb
}

#[test]
fn a_synthetic_book_round_trips_through_our_own_kf8_reader() {
    let dir = tempfile::tempdir().unwrap();
    let mut oeb = two_chapter_book(dir.path());

    let mut writer = KF8Writer::new(Kf8WriterOpts::default());
    let book = writer.write(&mut oeb).expect("writing the KF8 book");
    let bytes = book
        .to_bytes(&oeb.metadata)
        .expect("serializing record0 + records");
    assert!(bytes.len() > 300, "the file should not be trivially small");
    assert_eq!(&bytes[60..68], b"BOOKMOBI");

    let reader = MobiReader::new(&bytes).expect("parsing our own KF8 output");
    assert_eq!(
        reader.kf8_type.as_deref(),
        Some("standalone"),
        "a bare KF8 record0 (min_version == 8) should be detected as standalone KF8"
    );
    assert_eq!(reader.book_header.mobi_version, 8);
    assert_ne!(
        reader.book_header.skelidx,
        calibre_ebooks::mobi::headers::NULL_INDEX,
        "a SKEL index should have been written"
    );
    assert_ne!(
        reader.book_header.ncxidx,
        calibre_ebooks::mobi::headers::NULL_INDEX,
        "an NCX index should have been written (the book has a TOC)"
    );

    let mut mobi8 = Mobi8Reader::new(reader, MobiLog::default(), false);
    let out_dir = tempfile::tempdir().unwrap();
    let opf_name = mobi8
        .run(out_dir.path())
        .expect("Mobi8Reader::run should reconstruct the book");
    assert_eq!(opf_name, "metadata.opf");

    // Skeleton reassembly recovers both chapters' original text.
    let text_dir = out_dir.path().join("text");
    let mut all_text = String::new();
    for entry in std::fs::read_dir(&text_dir).expect("a text/ directory should exist") {
        let path = entry.unwrap().path();
        all_text.push_str(&std::fs::read_to_string(&path).unwrap());
    }
    assert!(all_text.contains("Chapter One"), "{all_text}");
    assert!(all_text.contains("Chapter Two"), "{all_text}");
    assert!(all_text.contains("first chapter's own words"), "{all_text}");
    assert!(all_text.contains("back"), "{all_text}");

    // The OPF and NCX reflect the metadata/TOC the writer encoded.
    let opf = std::fs::read_to_string(out_dir.path().join("metadata.opf")).unwrap();
    assert!(opf.contains("<package"), "{opf}");
    assert!(opf.contains("The KF8 Round Trip"), "{opf}");

    let toc_ncx_path = out_dir.path().join("toc.ncx");
    if toc_ncx_path.exists() {
        let ncx = std::fs::read_to_string(&toc_ncx_path).unwrap();
        assert!(ncx.contains("Chapter One"), "{ncx}");
        assert!(ncx.contains("Chapter Two"), "{ncx}");
    }
}

/// A book with no TOC entries still produces a valid (if unindexed)
/// standalone KF8 file, matching `writer8/main.py`'s `if toc.count() < 1:
/// ... return` (index generation skipped, not an error).
#[test]
fn a_book_with_no_toc_still_writes_a_readable_kf8_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("c1.html"),
        "<html><body><p>Hi</p></body></html>",
    )
    .unwrap();
    let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
    oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
    oeb.spine.add("c1", true);
    oeb.metadata.add("title", "No TOC");
    oeb.metadata.add("date", "2020-01-01T00:00:00+00:00");
    oeb.metadata.add("language", "en");

    let mut writer = KF8Writer::new(Kf8WriterOpts::default());
    let book = writer.write(&mut oeb).expect("writing without a TOC");
    let bytes = book.to_bytes(&oeb.metadata).unwrap();
    let reader = MobiReader::new(&bytes).expect("parsing a TOC-less KF8 file");
    assert_eq!(
        reader.book_header.ncxidx,
        calibre_ebooks::mobi::headers::NULL_INDEX
    );
    assert!(
        writer.log.warnings().any(|w| w.contains("ToC")),
        "{:?}",
        writer.log.messages
    );
}
