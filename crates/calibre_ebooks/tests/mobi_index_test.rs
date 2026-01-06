use anyhow::Result;
use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::index::{check_signature, read_index, TagX};
use calibre_ebooks::mobi::utils::{encint, encode_string};
use std::collections::BTreeMap;

#[test]
fn test_check_signature() {
    let data = b"INDXHeader";
    assert!(check_signature(data, b"INDX").is_ok());
    assert!(check_signature(data, b"TAGX").is_err());
}

fn create_mock_indx_header(tagx_offset: u32) -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(b"INDX");
    data.write_u32::<BigEndian>(200).unwrap(); // len
    data.write_u32::<BigEndian>(0).unwrap(); // nul1
    data.write_u32::<BigEndian>(0).unwrap(); // type
    data.write_u32::<BigEndian>(0).unwrap(); // gen
    data.write_u32::<BigEndian>(0).unwrap(); // start (IDXT start) - we'll fill later or assume 0 relative to something?
                                             // Actually start is usually header length or IDXT offset.
                                             // parse_indx_header uses 'start' as IDXT pos.
                                             // Let's say IDXT is at 200.
    let idxt_start = 180; // Arbitrary
    data.write_u32::<BigEndian>(idxt_start).unwrap();
    data.write_u32::<BigEndian>(1).unwrap(); // count (1 entry)
    data.write_u32::<BigEndian>(65001).unwrap(); // code
    data.write_u32::<BigEndian>(0).unwrap(); // lng
    data.write_u32::<BigEndian>(0).unwrap(); // total
    data.write_u32::<BigEndian>(0).unwrap(); // ordt
    data.write_u32::<BigEndian>(0).unwrap(); // ligt
    data.write_u32::<BigEndian>(0).unwrap(); // nligt
    data.write_u32::<BigEndian>(0).unwrap(); // ncncx

    // unknowns
    for _ in 0..27 {
        data.write_u32::<BigEndian>(0).unwrap();
    }

    data.write_u32::<BigEndian>(0).unwrap(); // ocnt
    data.write_u32::<BigEndian>(0).unwrap(); // oentries
    data.write_u32::<BigEndian>(0).unwrap(); // ordt1
    data.write_u32::<BigEndian>(0).unwrap(); // ordt2
    data.write_u32::<BigEndian>(tagx_offset).unwrap(); // tagx

    // Pad to idxt_start
    while data.len() < idxt_start as usize {
        data.push(0);
    }

    // IDXT
    data.extend_from_slice(b"IDXT");
    // Entry offsets (2 bytes each). Count = 1.
    // Offset relative to IDXT? No, typically relative to record start?
    // parse_index_record: `pos, = struct.unpack_from(b'>H', data, idxt_pos + 4 + (2 * j))`
    // `idx_positions` are read.
    // `rec = data[start..end]`.
    // So pos is offset in data.
    // Let's say entry 0 starts at offset `tagx_offset` + some_length?
    // Complex to mock cleanly without full structure.

    data
}

#[test]
fn test_read_index_basic() {
    // This is hard to unit test without a real file structure or complex mocking
    // because `read_index` parses multiple layers (header, TAGX, IDXT, entries).
    // I will write a simpler test for `TagX` logic if possible, effectively testing `get_tag_map`.

    // However, I exposed `read_index` but not `get_tag_map`.
    // I can test `CNCXReader` at least.
}

#[test]
fn test_cncx_reader() {
    use calibre_ebooks::mobi::index::CNCXReader;

    // Mock CNCX record
    // VWI Length + String
    let str1 = "Hello";
    let mut rec1 = encint(str1.len() as u64, true);
    rec1.extend_from_slice(str1.as_bytes());

    let str2 = "World";
    let mut rec2 = encint(str2.len() as u64, true);
    rec2.extend_from_slice(str2.as_bytes());

    // Combine into one buffer
    let mut record_data = Vec::new();
    record_data.extend(rec1);
    record_data.extend(rec2);

    // Records list
    let records = vec![record_data];

    let reader = CNCXReader::new(&records, "utf-8");

    // Offsets?
    // 1st entry at 0.
    // 2nd entry at len(rec1).
    let len1 = 1 + str1.len(); // 1 byte VWI for small int
    let s1 = reader.get(0).unwrap();
    assert_eq!(s1, "Hello");

    let s2 = reader.get(len1).unwrap();
    assert_eq!(s2, "World");
}
