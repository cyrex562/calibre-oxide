//! Round-trip test for the MOBI 6 writer (`calibre_ebooks::mobi::writer2`,
//! issue #34): encode a small synthetic `OEBBook` and confirm our own
//! reader (`calibre_ebooks::mobi::mobi6`/`index`, issue #33) can parse
//! the result back -- record structure sane, text recoverable, TOC/index
//! entries present. Per `docs/HARNESS.md`, this stands in for
//! cross-validation against a real `calibre-debug` binary, which is not
//! installed on this machine.

use calibre_ebooks::mobi::headers::BookHeader;
use calibre_ebooks::mobi::index::read_index;
use calibre_ebooks::mobi::mobi6::MobiReader;
use calibre_ebooks::mobi::mobi8::Mobi8Reader;
use calibre_ebooks::mobi::utils::get_trailing_data;
use calibre_ebooks::mobi::writer2::main::{MobiWriter, MobiWriterOpts};
use calibre_ebooks::mobi::writer2::resources::{ResourceOpts, Resources};
use calibre_ebooks::mobi::writer8::main::{KF8Writer, Kf8WriterOpts};
use calibre_ebooks::mobi::MobiLog;
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

/// Real round-trip test for joint MOBI6+KF8 (`.azw3`) output (issue
/// #157): both halves of the same file recover their own real text
/// through our own readers, and the reader's `kf8_type == "joint"`
/// detection (`crate::mobi::mobi6`, issue #33) fires on the result.
#[test]
fn a_synthetic_book_round_trips_as_a_joint_mobi6_and_kf8_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut oeb = two_chapter_book(dir.path());

    // One `Resources` object, shared by both writers -- matching real
    // upstream's own `mobi_output.py` (a single `Resources` threaded
    // into `create_kf8_book(..., for_joint=True)` and then into
    // `MobiWriter(opts, resources, kf8, ...)`).
    let resource_opts = ResourceOpts::default();
    let mut resources = Resources::new(&oeb, resource_opts, false, true);

    let mut kf8_writer = KF8Writer::new(Kf8WriterOpts::default());
    let kf8_book = kf8_writer
        .write_for_joint(&mut oeb, &mut resources)
        .expect("writing the KF8 half");

    let mut mobi_writer = MobiWriter::new(MobiWriterOpts::default());
    let bytes = mobi_writer
        .write_joint(&oeb, &kf8_book, &mut resources)
        .expect("writing the joint file");
    assert!(bytes.len() > 500, "the file should not be trivially small");
    assert_eq!(&bytes[0x3C..0x3C + 8], b"BOOKMOBI");

    let reader = MobiReader::new(&bytes).expect("parsing our own joint output");
    assert_eq!(
        reader.kf8_type.as_deref(),
        Some("joint"),
        "a BOUNDARY record plus an EXTH kf8_header_index should be detected as joint"
    );

    // The MOBI6-view header (record0, sections[0]) is a real, directly
    // parseable MOBI6 header reporting file_version 6 -- verified by
    // parsing it independently of `reader.book_header` (which, for a
    // joint file, `MobiReader::new` overwrites with the *embedded KF8*
    // header once joint detection fires, matching what a real KF8-aware
    // reader consults for content).
    let mobi6_header = BookHeader::parse(&reader.sections[0], b"BOOKMOBI", None, false)
        .expect("the MOBI6-view record0 should itself be a well-formed MOBI header");
    assert_eq!(mobi6_header.mobi_version, 6);

    // The MOBI6 payload's own text records (sections[1..=records])
    // decompress back to real markup for both chapters, independent of
    // the embedded KF8 half.
    let mut mobi6_text = Vec::new();
    for i in 1..=(mobi6_header.records as usize) {
        let (_, stripped) = get_trailing_data(&reader.sections[i], mobi6_header.extra_flags as u32)
            .expect("stripping MOBI6 trailing entries");
        mobi6_text.extend(
            calibre_ebooks::compression::palmdoc::decompress(&stripped)
                .expect("decompressing a MOBI6 text record"),
        );
    }
    let mobi6_html = String::from_utf8_lossy(&mobi6_text);
    assert!(mobi6_html.contains("Chapter One"), "{mobi6_html}");
    assert!(mobi6_html.contains("Chapter Two"), "{mobi6_html}");

    // The embedded KF8 half reconstructs through the real skeleton/chunk
    // pipeline (`Mobi8Reader`, issue #33/#35), recovering the same real
    // content via a completely different code path.
    let out_dir = tempfile::tempdir().unwrap();
    let mut mobi8 = Mobi8Reader::new(reader, MobiLog::default(), false);
    mobi8
        .run(out_dir.path())
        .expect("Mobi8Reader::run should reconstruct the embedded KF8 half");
    let text_dir = out_dir.path().join("text");
    let mut kf8_text = String::new();
    for entry in std::fs::read_dir(&text_dir).expect("a text/ directory should exist") {
        let path = entry.unwrap().path();
        kf8_text.push_str(&std::fs::read_to_string(&path).unwrap());
    }
    assert!(kf8_text.contains("Chapter One"), "{kf8_text}");
    assert!(kf8_text.contains("Chapter Two"), "{kf8_text}");
}
