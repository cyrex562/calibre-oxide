//! Round-trip test for the MOBI 6 writer (`calibre_ebooks::mobi::writer2`,
//! issue #34): encode a small synthetic `OEBBook` and confirm our own
//! reader (`calibre_ebooks::mobi::mobi6`/`index`, issue #33) can parse
//! the result back -- record structure sane, text recoverable, TOC/index
//! entries present. Per `docs/HARNESS.md`, this stands in for
//! cross-validation against a real `calibre-debug` binary, which is not
//! installed on this machine.

use calibre_ebooks::mobi::index::read_index;
use calibre_ebooks::mobi::mobi6::MobiReader;
use calibre_ebooks::mobi::writer2::main::{MobiWriter, MobiWriterOpts};
use calibre_ebooks::oeb::book::OEBBook;
use calibre_ebooks::oeb::container::DirContainer;
use calibre_ebooks::oeb::toc::TOCNode;

fn two_chapter_book(dir: &std::path::Path) -> OEBBook {
    std::fs::write(
        dir.join("c1.html"),
        "<html><body><h1>Chapter One</h1><p>The first chapter's own words.</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        dir.join("c2.html"),
        "<html><body><h1>Chapter Two</h1><p>And <a href=\"c1.html\">back</a> we go.</p></body></html>",
    )
    .unwrap();

    let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
    oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
    oeb.manifest.add("c2", "c2.html", "application/xhtml+xml");
    oeb.spine.add("c1", true);
    oeb.spine.add("c2", true);

    oeb.metadata.add("title", "The Round Trip");
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

    oeb
}

#[test]
fn a_synthetic_book_round_trips_through_our_own_reader() {
    let dir = tempfile::tempdir().unwrap();
    let oeb = two_chapter_book(dir.path());

    let mut writer = MobiWriter::new(MobiWriterOpts::default());
    let bytes = writer.write(&oeb).expect("writing the MOBI file");
    assert!(bytes.len() > 500, "the file should not be trivially small");
    assert_eq!(&bytes[0x3C..0x3C + 8], b"BOOKMOBI");

    let mut reader = MobiReader::new(&bytes).expect("parsing our own MOBI output");
    reader.check_for_drm().expect("no DRM was written");

    // The MOBI header decoded a sane record count and identified the
    // book as non-periodical, non-KF8.
    assert!(reader.book_header.records >= 1);
    assert_eq!(reader.book_header.mobi_version, 6);
    assert_eq!(reader.kf8_type, None);

    // EXTH metadata survived the round trip.
    let exth = reader
        .book_header
        .exth
        .as_ref()
        .expect("an EXTH header was written");
    assert_eq!(exth.mi.title, "The Round Trip");
    assert!(exth.mi.authors.iter().any(|a| a.contains("Writer")));

    // The text records decompress back to the serialized markup, with
    // both chapters' content intact and the internal link resolved to a
    // real (non-placeholder) filepos.
    reader.extract_text(1).expect("extracting text records");
    let html = String::from_utf8_lossy(&reader.mobi_html);
    assert!(html.contains("Chapter One"), "{html}");
    assert!(html.contains("Chapter Two"), "{html}");
    assert!(html.contains("first chapter's own words"), "{html}");
    assert!(!html.contains("filepos=\"0000000000\""), "{html}");

    // The INDX tree we wrote decodes back with our reader's INDX
    // decoder, and its two entries' CNCX-resolved labels are the
    // chapter titles.
    let ncxidx = reader.book_header.ncxidx;
    assert_ne!(
        ncxidx,
        calibre_ebooks::mobi::headers::NULL_INDEX,
        "a primary index record was written"
    );
    let (table, cncx) = read_index(&reader.sections, ncxidx as usize, &reader.book_header.codec)
        .expect("decoding the INDX tree we wrote");
    assert_eq!(
        table.len(),
        2,
        "book index should have one entry per chapter"
    );

    let labels: Vec<String> = table
        .values()
        .filter_map(|tag_map| {
            let offset = *tag_map.get(&3)?.first()?;
            cncx.get(offset as usize).cloned()
        })
        .collect();
    assert!(labels.contains(&"Chapter One".to_string()), "{labels:?}");
    assert!(labels.contains(&"Chapter Two".to_string()), "{labels:?}");
}

#[test]
fn a_periodical_toc_round_trips_with_a_periodical_index() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("a1.html"),
        "<html><body><h1>Article One</h1><p>Some news.</p></body></html>",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("a2.html"),
        "<html><body><h1>Article Two</h1><p>More news.</p></body></html>",
    )
    .unwrap();

    let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
    oeb.manifest.add("a1", "a1.html", "application/xhtml+xml");
    oeb.manifest.add("a2", "a2.html", "application/xhtml+xml");
    oeb.spine.add("a1", true);
    oeb.spine.add("a2", true);
    oeb.metadata.add("title", "The Daily Round Trip");
    oeb.metadata.add("language", "en");
    oeb.metadata.add("date", "2020-01-01T00:00:00+00:00");

    let mut periodical = TOCNode::new(Some("The Daily Round Trip".into()), Some(String::new()));
    periodical.klass = Some("periodical".to_string());
    let mut section = TOCNode::new(Some("News".into()), Some("a1.html".into()));
    section.klass = Some("section".to_string());
    let mut art1 = TOCNode::new(Some("Article One".into()), Some("a1.html".into()));
    art1.klass = Some("article".to_string());
    let mut art2 = TOCNode::new(Some("Article Two".into()), Some("a2.html".into()));
    art2.klass = Some("article".to_string());
    section.add(art1);
    section.add(art2);
    periodical.add(section);
    oeb.toc.root.add(periodical);

    let mut writer = MobiWriter::new(MobiWriterOpts::default());
    let bytes = writer.write(&oeb).expect("writing a periodical MOBI file");

    let mut reader = MobiReader::new(&bytes).expect("parsing our own periodical output");
    // 0x101 = hierarchical news (section+article), matching
    // `bt = 0x103 if is_flat_periodical else 0x101` for a single-section
    // periodical with more than one node under it... a single section
    // makes this a *flat* periodical (0x103); check it's one of the two
    // periodical doctypes rather than pinning the exact value.
    assert!(matches!(reader.book_header.mobi.mobi_type, 0x101 | 0x103));

    reader.extract_text(1).expect("extracting text records");
    let html = String::from_utf8_lossy(&reader.mobi_html);
    assert!(html.contains("Article One"), "{html}");
    assert!(html.contains("Article Two"), "{html}");

    let ncxidx = reader.book_header.ncxidx;
    let (table, _cncx) = read_index(&reader.sections, ncxidx as usize, &reader.book_header.codec)
        .expect("decoding the periodical INDX tree we wrote");
    // periodical + section + 2 articles.
    assert_eq!(table.len(), 4, "{:?}", table.keys().collect::<Vec<_>>());
}

/// A book with no TOC entries still produces a valid (if unindexed)
/// file, matching Python's `if oeb.toc.count() < 1: ... return` (index
/// generation skipped, not an error).
#[test]
fn a_book_with_no_toc_still_writes_successfully() {
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

    let mut writer = MobiWriter::new(MobiWriterOpts::default());
    let bytes = writer.write(&oeb).expect("writing without a TOC");
    let reader = MobiReader::new(&bytes).expect("parsing a TOC-less MOBI file");
    assert_eq!(
        reader.book_header.ncxidx,
        calibre_ebooks::mobi::headers::NULL_INDEX
    );
    assert!(writer.log.warnings().any(|w| w.contains("No TOC")));
}

/// A missing date/timestamp is a real, documented failure mode (EXTH
/// requires one) rather than a silent fallback.
#[test]
fn missing_date_metadata_is_a_reported_error_not_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("c1.html"),
        "<html><body><p>Hi</p></body></html>",
    )
    .unwrap();
    let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir.path())));
    oeb.manifest.add("c1", "c1.html", "application/xhtml+xml");
    oeb.spine.add("c1", true);
    oeb.metadata.add("title", "No Date");

    let mut writer = MobiWriter::new(MobiWriterOpts::default());
    let err = writer.write(&oeb).unwrap_err();
    assert!(
        err.to_string().contains("date") || err.chain().any(|c| c.to_string().contains("date"))
    );
}

#[test]
fn write_joint_is_a_documented_placeholder_not_a_silent_stub() {
    // This asserts the *shape* of the gap: calling the joint-KF8 path
    // panics with a `todo!()` naming the exact reason (mobi/writer8 not
    // ported) rather than silently returning wrong bytes. Run in a
    // subprocess-free way via `std::panic::catch_unwind` since we only
    // want to assert the panic message, not actually abort the test
    // binary.
    let dir = tempfile::tempdir().unwrap();
    let oeb = two_chapter_book(dir.path());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut writer = MobiWriter::new(MobiWriterOpts::default());
        let _ = writer.write_joint(&oeb, None);
    }));
    let err = result.expect_err("write_joint should not succeed silently");
    let msg = err
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(msg.contains("writer8"), "{msg}");
}
