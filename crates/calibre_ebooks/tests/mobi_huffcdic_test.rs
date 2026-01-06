use byteorder::{BigEndian, WriteBytesExt};
use calibre_ebooks::mobi::huffcdic::HuffReader;
use std::io::Write;

#[test]
fn test_huffreader_init() {
    // Construct a valid HUFF record
    let mut huff_data = Vec::new();

    // Header
    huff_data.write_all(b"HUFF").unwrap();
    huff_data.write_all(&[0, 0, 0, 0]).unwrap(); // Version/Unused

    // Offsets
    let off1 = 24; // Dict1 is immediately after header (8 + 8 + 8 = 24) ??
                   // Header is 8 bytes.
                   // offsets are at 8, 12.
                   // off1, off2 read from offset 8.
                   // "off1, off2 = struct.unpack_from(b'>LL', huff, 8)"
                   // So 8 bytes magic+padding. Then off1, off2. (total 16 bytes).
                   // Then extra 8 bytes? No.
                   // "if huff[0:8] != b'HUFF\x00\x00\x00\x18':" -> Magic + 0x18 length?
                   // 0x18 = 24.
                   // So header is 24 bytes long?
                   // 0-4: HUFF
                   // 4-8: 00 00 00 18
                   // 8-12: off1
                   // 12-16: off2
                   // 16-24: Padding?

    huff_data.clear();
    huff_data.write_all(b"HUFF").unwrap();
    huff_data.write_u32::<BigEndian>(24).unwrap(); // 0x18

    let offset1 = 24;
    huff_data.write_u32::<BigEndian>(offset1).unwrap();

    // Dict1 is 256 * 4 = 1024 bytes.
    let offset2 = offset1 + 1024;
    huff_data.write_u32::<BigEndian>(offset2).unwrap();

    huff_data.write_u32::<BigEndian>(0).unwrap(); // Padding to 24
    huff_data.write_u32::<BigEndian>(0).unwrap(); // Padding to 24

    assert_eq!(huff_data.len(), 24);

    // Write Dict1 (256 u32s)
    // We want valid dictionary entries.
    // codelen (5 bits), term (1 bit), maxcode (2 bytes approx)
    // Fill with 0s for now (codelen=0, term=0)
    for _ in 0..256 {
        huff_data.write_u32::<BigEndian>(0).unwrap();
    }

    // Write Dict2 (64 u32s)
    for _ in 0..64 {
        huff_data.write_u32::<BigEndian>(0).unwrap();
    }

    // CDIC Record
    let mut cdic_data = Vec::new();
    cdic_data.write_all(b"CDIC").unwrap();
    cdic_data.write_u32::<BigEndian>(16).unwrap(); // Header len 0x10?
                                                   // "if cdic[0:8] != b'CDIC\x00\x00\x00\x10':"
                                                   // 4-8: 0x10

    // 8-12: phrases
    // 12-16: bits
    cdic_data.write_u32::<BigEndian>(1).unwrap(); // 1 phrase
    cdic_data.write_u32::<BigEndian>(1).unwrap(); // 1 bit

    // Data starts at 16.
    // Entry offsets (n * 2 bytes). n = min(1<<bits, phrases - dict_len) = 1.
    // 1 entry offset. 2 bytes.
    cdic_data.write_u16::<BigEndian>(0).unwrap(); // Offset 0

    // Entry 0 data.
    // 2 bytes header (blen).
    // blen = len & 0x7fff. flag = 0x8000.
    // Let's make "a" (len 1). flag 1 (true, meaning terminal).
    // blen = 1 | 0x8000 = 0x8001.
    cdic_data.write_u16::<BigEndian>(0x8001).unwrap();
    cdic_data.write_all(b"a").unwrap();

    let huffs = vec![huff_data, cdic_data];

    let reader = HuffReader::new(&huffs);
    assert!(reader.is_ok(), "HuffReader init failed: {:?}", reader.err());
}
