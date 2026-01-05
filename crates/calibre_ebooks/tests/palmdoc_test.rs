use calibre_ebooks::compression::palmdoc::decompress;

#[test]
fn test_palmdoc_decompression_vectors() {
    // "a b c " ->
    // 'a' = 0x61. ' ' = 0x20. 'b' = 0x62. ' ' = 0x20. 'c' = 0x63. ' ' = 0x20.
    // Encoded: 'a'(literal) ' '(space_char_b) ' '(space_char_c) ' ' ?
    // space + char logic: >= 0xC0.
    // ' b': ' ' + 'b'(0x62). 0x62 ^ 0x80 = 0xE2. 0xC0 threshold.
    // ' c': 0xE3?
    let input = b"a\xe2\xe3 ";
    // a(0x61). E2 -> ' ' + (E2^80=62='b'). E3 -> ' ' + (E3^80=63='c'). ' '(0x20 literal).
    // Result: "a b c "

    let result = decompress(input).expect("Decompression failed");
    assert_eq!(result, b"a b c ");
}

#[test]
fn test_lz77_backref() {
    // "abcabc": "abc" + repeat(start=0, len=3)
    // "abc": 0x61, 0x62, 0x63
    // Repeat: len 3, dist 3.
    // Pair: distance 3, length 3.
    // code = 0x8000 + (dist << 3) + (len - 3).
    // dist=3 (0...011). len-3 = 0.
    // code = 0x8000 + (3 << 3) + 0 = 0x8000 + 24 = 0x8018.
    // Bytes: 0x80, 0x18.
    let input = b"abc\x80\x18";
    let result = decompress(input).expect("Decompression failed");
    assert_eq!(result, b"abcabc");
}
