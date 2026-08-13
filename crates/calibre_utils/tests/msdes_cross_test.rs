//! Cross-validation of the LIT DES engine against calibre's C.
//!
//! The vectors in `data/msdes_vectors.rs` come from compiling
//! `old_src/src/calibre/utils/msdes/des.c` (with its `spr.h` S-boxes)
//! and running `deskey()` + `des()` over each input.
//!
//! Worth knowing: the validation sets in the comment at the bottom of
//! `des.c` do *not* hold for the code as shipped. `spr.h` carries
//! Microsoft's modified S-boxes rather than the stock D3DES ones the
//! comment was written against, so the published `c957 4425 6a5e d31d`
//! ciphertext is stale. The C actually produces `06c0 e899 8c38 6087`
//! for that key/plaintext pair, and so does this port.

#[path = "data/msdes_vectors.rs"]
mod vectors;

use calibre_utils::msdes::{DesKey, DE1, EN0};

#[test]
fn output_matches_calibre_on_every_vector() {
    let mut mismatches = Vec::new();
    for (key, edf, plain, expected) in vectors::MSDES_VECTORS {
        let got = DesKey::new(key, *edf)
            .process(plain)
            .expect("vectors are whole blocks");
        if got != *expected {
            mismatches.push(format!(
                "key={key:02x?} edf={edf} len={}: rust={:02x?} calibre={:02x?}",
                plain.len(),
                &got[..8.min(got.len())],
                &expected[..8.min(expected.len())]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::MSDES_VECTORS.len(),
        mismatches.join("\n")
    );
}

#[test]
fn the_stale_comment_vector_in_des_c_does_not_hold() {
    // Guards the note above: if someone swaps `spr.h` for stock D3DES
    // tables, this test fails and points at why.
    let key = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
    let plain = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xe7];
    let got = DesKey::new(&key, EN0).process_block(&plain);
    assert_eq!(got, [0x06, 0xc0, 0xe8, 0x99, 0x8c, 0x38, 0x60, 0x87]);
}

#[test]
fn en0_and_de1_invert_each_other() {
    // What `reader.py` relies on: seal with EN0 in the writer, open
    // with DE1 in the reader.
    let key = [0x0a, 0x1b, 0x2c, 0x3d, 0x4e, 0x5f, 0x60, 0x71];
    let data: Vec<u8> = (0u8..=255).collect();
    let sealed = DesKey::new(&key, EN0).process(&data).expect("whole blocks");
    let opened = DesKey::new(&key, DE1)
        .process(&sealed)
        .expect("whole blocks");
    assert_eq!(opened, data);
}
