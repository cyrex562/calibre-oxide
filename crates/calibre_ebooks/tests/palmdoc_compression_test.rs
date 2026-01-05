use calibre_ebooks::compression::palmdoc::{compress, decompress};

#[test]
fn test_palmdoc_roundtrip() {
    let data = b"This is a test string. This is a test string. This is a test string.";
    let compressed = compress(data).unwrap();
    let decompressed = decompress(&compressed).unwrap();

    assert_eq!(data.to_vec(), decompressed);
}

#[test]
fn test_palmdoc_compression_efficiency() {
    let data = b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"; // 50 'A's
    let compressed = compress(data).unwrap();
    // Should be compressed significantly (literal A, pair(dist=1, len=10) x 4?, pair(dist=1, len=9)?)
    // 50 bytes -> ~10 bytes?
    assert!(compressed.len() < data.len());
}
