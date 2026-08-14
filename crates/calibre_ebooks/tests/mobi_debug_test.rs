//! Integration test for `mobi::debug` against a real, byte-correct
//! MOBI6 file.
//!
//! The fixture is built by hand rather than through
//! `output::mobi_output::MOBIOutput`: that writer (a different,
//! already-shipped module, not part of this port) declares a MOBI
//! header `length` of 232 while also appending its EXTH block
//! immediately after byte 232 — which makes a spec-correct reader
//! read the header's `has_extra_data_flags` region (bytes 192-247) as
//! EXTH bytes instead, corrupting fields like `primary_index_record`.
//! That's a pre-existing defect in `mobi::writer.rs`'s header layout,
//! separate from `mobi::writer.rs`'s record-table bug this same port
//! fixed (see the port's commit message) — fixing the header-layout
//! issue too would mean rewriting a module this issue doesn't own.
//!
//! Building the fixture directly instead means the byte layout is
//! known to be spec-correct by construction, so `inspect_mobi`'s
//! output can be checked against exact expected values rather than
//! against another module's possibly-wrong bytes.

use calibre_ebooks::mobi::debug::main::inspect_mobi;
use std::fs;
use tempfile::tempdir;

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}
fn be16(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}

/// Build a minimal, spec-correct, uncompressed MOBI6 file: a PalmDB
/// container with two records (the MOBI header record, and one text
/// record), no EXTH, no indices.
fn build_minimal_mobi6(title: &str, body: &[u8]) -> Vec<u8> {
    // --- Record 0: PalmDoc header (16 bytes) + MOBI header. ---
    let mut record0 = Vec::new();
    record0.extend_from_slice(&be16(1)); // compression: no compression
    record0.extend_from_slice(&be16(0)); // unused
    record0.extend_from_slice(&be32(body.len() as u32)); // text length
    record0.extend_from_slice(&be16(1)); // number of text records
    record0.extend_from_slice(&be16(4096)); // text record size
    record0.extend_from_slice(&be16(0)); // encryption
    record0.extend_from_slice(&be16(0)); // unknown

    let mobi_start = record0.len();
    record0.extend_from_slice(b"MOBI");
    record0.extend_from_slice(&be32(116)); // header length, relative to
                                           // the MOBI identifier (like
                                           // `exth_offset = 16 + length`
                                           // does): ends before
                                           // has_drm_data (needs >=174)
                                           // and has_extra_data_flags
                                           // (needs >=232) territory.
    record0.extend_from_slice(&be32(2)); // type: Mobipocket book
    record0.extend_from_slice(&be32(65001)); // encoding: utf-8
    record0.extend_from_slice(&be32(0)); // uid
    record0.extend_from_slice(&be32(6)); // file_version
    record0.extend_from_slice(&be32(0xFFFF_FFFF)); // meta_orth_indx: NULL
    record0.extend_from_slice(&be32(0xFFFF_FFFF)); // meta_infl_indx: NULL
    record0.extend_from_slice(&be32(0xFFFF_FFFF)); // secondary_index_record: NULL
    record0.extend_from_slice(&[0u8; 28]); // reserved
    record0.extend_from_slice(&be32(0xFFFF_FFFF)); // first_non_book_record: NULL
    let fullname_offset = (mobi_start + 116) as u32;
    record0.extend_from_slice(&be32(fullname_offset));
    record0.extend_from_slice(&be32(title.len() as u32)); // fullname_length
    record0.extend_from_slice(&be32(9)); // locale_raw: English
    record0.extend_from_slice(&be32(0)); // input_language
    record0.extend_from_slice(&be32(0)); // output_language
    record0.extend_from_slice(&be32(0)); // min_version
    record0.extend_from_slice(&be32(0xFFFF_FFFF)); // first_image_index: NULL
    record0.extend_from_slice(&be32(0)); // huffman_record_offset
    record0.extend_from_slice(&be32(0)); // huffman_record_count
    record0.extend_from_slice(&be32(0)); // datp_record_offset
    record0.extend_from_slice(&be32(0)); // datp_record_count
    record0.extend_from_slice(&be32(0)); // exth_flags: no EXTH
    assert_eq!(
        record0.len(),
        mobi_start + 116,
        "header field layout drifted"
    );
    record0.extend_from_slice(title.as_bytes());

    // --- PalmDB container: header + record table + gap + records. ---
    let num_records = 2u32;
    let base_offset = 78 + num_records * 8 + 2;
    let record0_offset = base_offset;
    let record1_offset = record0_offset + record0.len() as u32;

    let mut out = Vec::new();
    out.extend_from_slice(&[0u8; 32]); // name
    out.extend_from_slice(&be16(0)); // attributes
    out.extend_from_slice(&be16(0)); // version
    out.extend_from_slice(&be32(0)); // creation date
    out.extend_from_slice(&be32(0)); // modification date
    out.extend_from_slice(&be32(0)); // backup date
    out.extend_from_slice(&be32(0)); // modification number
    out.extend_from_slice(&be32(0)); // app info id
    out.extend_from_slice(&be32(0)); // sort info id
    out.extend_from_slice(b"BOOK");
    out.extend_from_slice(b"MOBI");
    out.extend_from_slice(&be32(0)); // last record uid + 1
    out.extend_from_slice(&be32(0)); // next rec list id
    out.extend_from_slice(&be16(2)); // number of records

    out.extend_from_slice(&be32(record0_offset));
    out.extend_from_slice(&[0u8; 4]); // attributes(1) + uid(3)
    out.extend_from_slice(&be32(record1_offset));
    out.extend_from_slice(&[0u8; 4]);

    out.extend_from_slice(&be16(0)); // gap
    assert_eq!(
        out.len() as u32,
        record0_offset,
        "record table size drifted"
    );

    out.extend_from_slice(&record0);
    out.extend_from_slice(body);
    out
}

#[test]
fn inspect_mobi_dumps_a_correctly_built_mobi6_file() {
    let title = "MOBI Debug Test Book";
    let body = b"<html><body><h1>MOBI Debug Test</h1><p>Some content here.</p></body></html>";
    let raw = build_minimal_mobi6(title, body);

    let tmp = tempdir().expect("tempdir");
    let ddir = tmp.path().join("decompiled");
    fs::create_dir_all(&ddir).expect("mkdir");
    inspect_mobi(raw, &ddir).expect("inspect_mobi failed on a spec-correct MOBI6 file");

    let header = fs::read_to_string(ddir.join("header.txt")).expect("header.txt");
    assert!(header.contains("PalmDB Header"), "{header}");
    assert!(header.contains("Identifier: [77, 79, 66, 73]"), "{header}"); // "MOBI"
    assert!(
        header.contains(&format!("Text length: {}", body.len())),
        "{header}"
    );
    assert!(header.contains("Number of text records: 1"), "{header}");
    assert!(header.contains("MOBI 6 Header"), "{header}");

    let text = fs::read_to_string(ddir.join("text.html")).expect("text.html");
    assert_eq!(text.as_bytes(), body);

    for dir in ["text", "images", "binary", "font"] {
        assert!(ddir.join(dir).is_dir(), "missing {dir} dir");
    }
    // Exactly one dumped text record, since the fixture has one.
    let text_dumps: Vec<_> = fs::read_dir(ddir.join("text")).unwrap().collect();
    assert_eq!(
        text_dumps.len(),
        2,
        "expected a .txt and a .trailing_data file"
    );
}

#[test]
fn inspect_mobi_rejects_a_file_that_is_not_mobi() {
    let tmp = tempdir().expect("tempdir");
    let ddir = tmp.path().join("decompiled");
    fs::create_dir_all(&ddir).expect("mkdir");
    assert!(inspect_mobi(b"definitely not a mobi file".to_vec(), &ddir).is_err());
}
