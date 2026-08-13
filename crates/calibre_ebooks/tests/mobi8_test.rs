//! Integration test for `Mobi8Reader` exercising the real KF8
//! skeleton/div (SKEL INDX) reassembly path end to end: a hand-built
//! standalone KF8 PDB with a minimal but structurally real two-level
//! INDX (index-of-indices -> one data row -> one entry) SKEL table, no
//! DIV/GUIDE/FDST/NCX tables (all left `NULL_INDEX`, which `build_parts`
//! and `read_indices` treat as "empty" rather than needing to be
//! present -- see the inline comments below for why a single-skeleton,
//! zero-div-entries fixture is the minimal *real* KF8 structure).
//!
//! Building the general div-table/multi-part case would require encoding
//! the TAGX/INDX wire format's more elaborate control-byte-group
//! packing, which isn't practical to hand-verify without a reference
//! implementation (no `calibre-debug`/python calibre available in this
//! environment -- see `docs/HARNESS.md`). The pure helper functions
//! (`get_first_resource_index`, `reverse_tag_iter`,
//! `locate_beg_end_of_tag`, `extract_resources`'s FONT/CRES handling)
//! are unit-tested directly in `src/mobi/mobi8.rs`.

use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::headers::NULL_INDEX;
use calibre_ebooks::mobi::mobi6::MobiReader;
use calibre_ebooks::mobi::mobi8::Mobi8Reader;
use calibre_ebooks::mobi::MobiLog;

/// Encodes `value` the way `mobi::utils::encint` does for small values
/// (< 128): a single byte with the continuation bit set.
fn small_vwi(value: u8) -> u8 {
    assert!(value < 0x80);
    value | 0x80
}

/// Builds the two-record SKEL INDX table (index-of-indices + one data
/// row) described in the module docs, holding a single skeleton entry
/// named "0" spanning `[0, text_len)` with zero div-table entries.
fn build_skel_index_records(text_len: u8) -> (Vec<u8>, Vec<u8>) {
    // --- Top-level "index of indices" record ---
    let mut top = vec![0u8; 184];
    top[0..4].copy_from_slice(b"INDX");
    (&mut top[4..8]).write_u32::<BigEndian>(204).unwrap(); // len (informational)
    (&mut top[24..28]).write_u32::<BigEndian>(1).unwrap(); // count: 1 data row follows
    (&mut top[28..32]).write_u32::<BigEndian>(65001).unwrap(); // code
    (&mut top[180..184]).write_u32::<BigEndian>(184).unwrap(); // tagx offset

    // TAGX section right after the header: one real tag (6 == combined
    // start/length, 2 values) packed into bit 0 of a single control
    // byte, plus the end-of-control-bytes marker tag.
    top.extend_from_slice(b"TAGX");
    top.write_u32::<BigEndian>(20).unwrap(); // first_entry_offset (relative to TAGX start)
    top.write_u32::<BigEndian>(1).unwrap(); // control_byte_count
    top.extend_from_slice(&[6, 2, 0b0000_0001, 0]); // tag=6, num_of_values=2, bitmask=bit0, eof=0
    top.extend_from_slice(&[0, 0, 0, 1]); // eof marker: consumes the control byte
    assert_eq!(top.len(), 204);

    // --- Data row: one entry, identifier "0", tag 6 = (start=0, length=text_len) ---
    let mut data = vec![0u8; 184];
    data[0..4].copy_from_slice(b"INDX");
    (&mut data[24..28]).write_u32::<BigEndian>(1).unwrap(); // count: 1 entry
    (&mut data[28..32]).write_u32::<BigEndian>(65001).unwrap();
    let entry_start = data.len(); // == 184
    data.push(1); // identifier length-prefix
    data.push(b'0'); // identifier "0"
    data.push(0b0000_0001); // control byte: bit0 set -> tag 6 present
    data.push(small_vwi(0)); // start_position = 0
    data.push(small_vwi(text_len)); // length = text_len
    let idxt_pos = data.len();
    data.extend_from_slice(b"IDXT");
    data.write_u16::<BigEndian>(entry_start as u16).unwrap();
    (&mut data[20..24])
        .write_u32::<BigEndian>(idxt_pos as u32)
        .unwrap(); // start (IDXT position)

    (top, data)
}

/// Builds a standalone KF8 PDB: record 0 (PalmDOC+MOBI header, KF8
/// index fields pointing at the SKEL table), record 1 (the raw flow
/// text), record 2 (SKEL top-level INDX), record 3 (SKEL data row).
fn build_standalone_kf8_pdb(html: &[u8]) -> Vec<u8> {
    let (skel_top, skel_data) = build_skel_index_records(html.len() as u8);
    let skelidx: u32 = 2; // kf8_sections index (== overall sections index, since offset=1 for standalone)

    let mut rec0 = vec![0u8; 264];
    // PalmDOC header (0..16)
    (&mut rec0[0..2]).write_u16::<BigEndian>(1).unwrap(); // compression: none
    (&mut rec0[8..10]).write_u16::<BigEndian>(1).unwrap(); // record_count: 1 text record
    (&mut rec0[10..12]).write_u16::<BigEndian>(4096).unwrap();

    rec0[16..20].copy_from_slice(b"MOBI");
    (&mut rec0[20..24]).write_u32::<BigEndian>(232).unwrap(); // header_length
    (&mut rec0[28..32]).write_u32::<BigEndian>(65001).unwrap(); // text_encoding: utf-8

    // full_name_offset/length (raw[0x54:0x5c]) -> title placed right after
    // the fixed 264-byte header region.
    let title = b"KF8 Test Book";
    (&mut rec0[0x54..0x58]).write_u32::<BigEndian>(264).unwrap();
    (&mut rec0[0x58..0x5c])
        .write_u32::<BigEndian>(title.len() as u32)
        .unwrap();

    (&mut rec0[0x68..0x6c]).write_u32::<BigEndian>(8).unwrap(); // min_version -> mobi_version 8
    (&mut rec0[0x6c..0x70]).write_u32::<BigEndian>(4).unwrap(); // first_image_index: past all 4 sections (no resources)

    // ncxidx (0xF4) / dividx,skelidx,datpidx,othidx (0xF8..0x108) / fdstidx,fdstcnt (0xC0..0xC8)
    (&mut rec0[0xF4..0xF8])
        .write_u32::<BigEndian>(NULL_INDEX)
        .unwrap();
    (&mut rec0[0xF8..0xFC])
        .write_u32::<BigEndian>(NULL_INDEX)
        .unwrap(); // dividx
    (&mut rec0[0xFC..0x100])
        .write_u32::<BigEndian>(skelidx)
        .unwrap(); // skelidx
    (&mut rec0[0x100..0x104])
        .write_u32::<BigEndian>(NULL_INDEX)
        .unwrap(); // datpidx
    (&mut rec0[0x104..0x108])
        .write_u32::<BigEndian>(NULL_INDEX)
        .unwrap(); // othidx
    (&mut rec0[0xC0..0xC4])
        .write_u32::<BigEndian>(NULL_INDEX)
        .unwrap(); // fdstidx
    (&mut rec0[0xC4..0xC8]).write_u32::<BigEndian>(0).unwrap(); // fdstcnt <= 1 -> fdstidx forced NULL_INDEX anyway

    rec0.extend_from_slice(title);

    let records = vec![rec0, html.to_vec(), skel_top, skel_data];
    build_pdb("KF8 Test Book", &records)
}

fn build_pdb(name: &str, records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut name_bytes = [0u8; 32];
    let n = name.as_bytes();
    let len = n.len().min(31);
    name_bytes[..len].copy_from_slice(&n[..len]);
    out.extend_from_slice(&name_bytes);
    out.write_u16::<BigEndian>(0).unwrap();
    out.write_u16::<BigEndian>(0).unwrap();
    for _ in 0..6 {
        out.write_u32::<BigEndian>(0).unwrap();
    }
    out.extend_from_slice(b"BOOK");
    out.extend_from_slice(b"MOBI");
    out.write_u32::<BigEndian>(0).unwrap();
    out.write_u32::<BigEndian>(0).unwrap();
    out.write_u16::<BigEndian>(records.len() as u16).unwrap();

    let header_and_list_len = 78 + records.len() * 8;
    let mut offset = header_and_list_len as u32;
    let mut offsets = Vec::new();
    for r in records {
        offsets.push(offset);
        offset += r.len() as u32;
    }
    for off in &offsets {
        out.write_u32::<BigEndian>(*off).unwrap();
        out.extend_from_slice(&[0u8; 4]);
    }
    for r in records {
        out.extend_from_slice(r);
    }
    out
}

#[test]
fn test_mobi6_reader_detects_standalone_kf8() {
    let html = b"<html><body><p>KF8 content</p></body></html>";
    let pdb = build_standalone_kf8_pdb(html);
    let reader = MobiReader::new(&pdb).expect("standalone KF8 PDB should parse");
    assert_eq!(reader.kf8_type.as_deref(), Some("standalone"));
    assert_eq!(reader.book_header.mobi_version, 8);
    assert_eq!(reader.book_header.skelidx, 2);
    assert_eq!(reader.book_header.dividx, NULL_INDEX);
}

#[test]
fn test_mobi8_reader_rebuilds_single_skeleton_part() {
    let html = b"<html><body><p>KF8 content</p></body></html>";
    let pdb = build_standalone_kf8_pdb(html);
    let reader = MobiReader::new(&pdb).expect("standalone KF8 PDB should parse");
    assert_eq!(reader.kf8_type.as_deref(), Some("standalone"));

    let mut mobi8 = Mobi8Reader::new(reader, MobiLog::default(), false);
    let tmp = tempfile::tempdir().unwrap();
    let opf_name = mobi8
        .run(tmp.path())
        .expect("Mobi8Reader::run should succeed");
    assert_eq!(opf_name, "metadata.opf");

    // The skeleton (divtbl_count == 0) is written back out verbatim as a
    // single part file under text/.
    let text_dir = tmp.path().join("text");
    let entries: Vec<_> = std::fs::read_dir(&text_dir).unwrap().collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one reconstructed part file"
    );
    let part_path = entries[0].as_ref().unwrap().path();
    let part_contents = std::fs::read_to_string(&part_path).unwrap();
    assert!(part_contents.contains("KF8 content"), "{part_contents}");

    let opf = std::fs::read_to_string(tmp.path().join("metadata.opf")).unwrap();
    assert!(opf.contains("<package"), "{opf}");
    assert!(opf.contains("text/"), "{opf}");
}
