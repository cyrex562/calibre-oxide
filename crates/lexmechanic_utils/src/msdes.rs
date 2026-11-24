//! LIT-specific DES en/decryption.
//!
//! Port of `src/calibre/utils/msdes/` — D3DES V5.09 by Richard
//! Outerbridge (public domain), plus the `deskey`/`des` pair that
//! `msdesmodule.c` exposes to Python.
//!
//! The C keeps the expanded key schedule in a file-scoped global that
//! `deskey()` overwrites; here it lives in a [`DesKey`] value, so two
//! keys can be in use at once (which `reader.py` effectively needs when
//! it derives the title key and then decrypts content with it).
//!
//! Note on integer width: the C uses `unsigned long`, which is 64 bits
//! on LP64. Every place the extra width could leak high garbage into the
//! result is masked before it matters, so the 32-bit arithmetic used
//! here produces identical output. See the tests, which check the
//! published D3DES validation vector.

/// En/decryption direction. `EN0` in `d3des.h`.
pub const EN0: i16 = 0;
/// En/decryption direction. `DE1` in `d3des.h`.
pub const DE1: i16 = 1;

/// The DES block size, in bytes.
pub const BLOCK_SIZE: usize = 8;

/// `bytebit` in `des.c`.
const BYTEBIT: [u8; 8] = [0o200, 0o100, 0o40, 0o20, 0o10, 0o4, 0o2, 0o1];

/// `pc1` in `des.c` — permuted choice 1, from ANSI X3.92-1981.
const PC1: [u8; 56] = [
    56, 48, 40, 32, 24, 16, 8, 0, 57, 49, 41, 33, 25, 17, 9, 1, 58, 50, 42, 34, 26, 18, 10, 2, 59,
    51, 43, 35, 62, 54, 46, 38, 30, 22, 14, 6, 61, 53, 45, 37, 29, 21, 13, 5, 60, 52, 44, 36, 28,
    20, 12, 4, 27, 19, 11, 3,
];

/// `totrot` in `des.c` — cumulative left rotations per round.
const TOTROT: [u8; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 15, 17, 19, 21, 23, 25, 27, 28];

/// `pc2` in `des.c` — permuted choice 2.
const PC2: [u8; 48] = [
    13, 16, 10, 23, 0, 4, 2, 27, 14, 5, 20, 9, 22, 18, 11, 3, 25, 7, 15, 6, 26, 19, 12, 1, 40, 51,
    30, 36, 46, 54, 29, 39, 50, 44, 32, 47, 43, 48, 38, 55, 33, 52, 45, 41, 49, 35, 28, 31,
];

/// `SP1` from `src/calibre/utils/msdes/spr.h`.
const SP1: [u32; 64] = [
    0x02080800, 0x00080000, 0x02000002, 0x02080802, 0x02000000, 0x00080802, 0x00080002, 0x02000002,
    0x00080802, 0x02080800, 0x02080000, 0x00000802, 0x02000802, 0x02000000, 0x00000000, 0x00080002,
    0x00080000, 0x00000002, 0x02000800, 0x00080800, 0x02080802, 0x02080000, 0x00000802, 0x02000800,
    0x00000002, 0x00000800, 0x00080800, 0x02080002, 0x00000800, 0x02000802, 0x02080002, 0x00000000,
    0x00000000, 0x02080802, 0x02000800, 0x00080002, 0x02080800, 0x00080000, 0x00000802, 0x02000800,
    0x02080002, 0x00000800, 0x00080800, 0x02000002, 0x00080802, 0x00000002, 0x02000002, 0x02080000,
    0x02080802, 0x00080800, 0x02080000, 0x02000802, 0x02000000, 0x00000802, 0x00080002, 0x00000000,
    0x00080000, 0x02000000, 0x02000802, 0x02080800, 0x00000002, 0x02080002, 0x00000800, 0x00080802,
];
/// `SP2` from `src/calibre/utils/msdes/spr.h`.
const SP2: [u32; 64] = [
    0x40108010, 0x00000000, 0x00108000, 0x40100000, 0x40000010, 0x00008010, 0x40008000, 0x00108000,
    0x00008000, 0x40100010, 0x00000010, 0x40008000, 0x00100010, 0x40108000, 0x40100000, 0x00000010,
    0x00100000, 0x40008010, 0x40100010, 0x00008000, 0x00108010, 0x40000000, 0x00000000, 0x00100010,
    0x40008010, 0x00108010, 0x40108000, 0x40000010, 0x40000000, 0x00100000, 0x00008010, 0x40108010,
    0x00100010, 0x40108000, 0x40008000, 0x00108010, 0x40108010, 0x00100010, 0x40000010, 0x00000000,
    0x40000000, 0x00008010, 0x00100000, 0x40100010, 0x00008000, 0x40000000, 0x00108010, 0x40008010,
    0x40108000, 0x00008000, 0x00000000, 0x40000010, 0x00000010, 0x40108010, 0x00108000, 0x40100000,
    0x40100010, 0x00100000, 0x00008010, 0x40008000, 0x40008010, 0x00000010, 0x40100000, 0x00108000,
];
/// `SP3` from `src/calibre/utils/msdes/spr.h`.
const SP3: [u32; 64] = [
    0x04000001, 0x04040100, 0x00000100, 0x04000101, 0x00040001, 0x04000000, 0x04000101, 0x00040100,
    0x04000100, 0x00040000, 0x04040000, 0x00000001, 0x04040101, 0x00000101, 0x00000001, 0x04040001,
    0x00000000, 0x00040001, 0x04040100, 0x00000100, 0x00000101, 0x04040101, 0x00040000, 0x04000001,
    0x04040001, 0x04000100, 0x00040101, 0x04040000, 0x00040100, 0x00000000, 0x04000000, 0x00040101,
    0x04040100, 0x00000100, 0x00000001, 0x00040000, 0x00000101, 0x00040001, 0x04040000, 0x04000101,
    0x00000000, 0x04040100, 0x00040100, 0x04040001, 0x00040001, 0x04000000, 0x04040101, 0x00000001,
    0x00040101, 0x04000001, 0x04000000, 0x04040101, 0x00040000, 0x04000100, 0x04000101, 0x00040100,
    0x04000100, 0x00000000, 0x04040001, 0x00000101, 0x04000001, 0x00040101, 0x00000100, 0x04040000,
];
/// `SP4` from `src/calibre/utils/msdes/spr.h`.
const SP4: [u32; 64] = [
    0x00401008, 0x10001000, 0x00000008, 0x10401008, 0x00000000, 0x10400000, 0x10001008, 0x00400008,
    0x10401000, 0x10000008, 0x10000000, 0x00001008, 0x10000008, 0x00401008, 0x00400000, 0x10000000,
    0x10400008, 0x00401000, 0x00001000, 0x00000008, 0x00401000, 0x10001008, 0x10400000, 0x00001000,
    0x00001008, 0x00000000, 0x00400008, 0x10401000, 0x10001000, 0x10400008, 0x10401008, 0x00400000,
    0x10400008, 0x00001008, 0x00400000, 0x10000008, 0x00401000, 0x10001000, 0x00000008, 0x10400000,
    0x10001008, 0x00000000, 0x00001000, 0x00400008, 0x00000000, 0x10400008, 0x10401000, 0x00001000,
    0x10000000, 0x10401008, 0x00401008, 0x00400000, 0x10401008, 0x00000008, 0x10001000, 0x00401008,
    0x00400008, 0x00401000, 0x10400000, 0x10001008, 0x00001008, 0x10000000, 0x10000008, 0x10401000,
];
/// `SP5` from `src/calibre/utils/msdes/spr.h`.
const SP5: [u32; 64] = [
    0x08000000, 0x00010000, 0x00000400, 0x08010420, 0x08010020, 0x08000400, 0x00010420, 0x08010000,
    0x00010000, 0x00000020, 0x08000020, 0x00010400, 0x08000420, 0x08010020, 0x08010400, 0x00000000,
    0x00010400, 0x08000000, 0x00010020, 0x00000420, 0x08000400, 0x00010420, 0x00000000, 0x08000020,
    0x00000020, 0x08000420, 0x08010420, 0x00010020, 0x08010000, 0x00000400, 0x00000420, 0x08010400,
    0x08010400, 0x08000420, 0x00010020, 0x08010000, 0x00010000, 0x00000020, 0x08000020, 0x08000400,
    0x08000000, 0x00010400, 0x08010420, 0x00000000, 0x00010420, 0x08000000, 0x00000400, 0x00010020,
    0x08000420, 0x00000400, 0x00000000, 0x08010420, 0x08010020, 0x08010400, 0x00000420, 0x00010000,
    0x00010400, 0x08010020, 0x08000400, 0x00000420, 0x00000020, 0x00010420, 0x08010000, 0x08000020,
];
/// `SP6` from `src/calibre/utils/msdes/spr.h`.
const SP6: [u32; 64] = [
    0x80000040, 0x00200040, 0x00000000, 0x80202000, 0x00200040, 0x00002000, 0x80002040, 0x00200000,
    0x00002040, 0x80202040, 0x00202000, 0x80000000, 0x80002000, 0x80000040, 0x80200000, 0x00202040,
    0x00200000, 0x80002040, 0x80200040, 0x00000000, 0x00002000, 0x00000040, 0x80202000, 0x80200040,
    0x80202040, 0x80200000, 0x80000000, 0x00002040, 0x00000040, 0x00202000, 0x00202040, 0x80002000,
    0x00002040, 0x80000000, 0x80002000, 0x00202040, 0x80202000, 0x00200040, 0x00000000, 0x80002000,
    0x80000000, 0x00002000, 0x80200040, 0x00200000, 0x00200040, 0x80202040, 0x00202000, 0x00000040,
    0x80202040, 0x00202000, 0x00200000, 0x80002040, 0x80000040, 0x80200000, 0x00202040, 0x00000000,
    0x00002000, 0x80000040, 0x80002040, 0x80202000, 0x80200000, 0x00002040, 0x00000040, 0x80200040,
];
/// `SP7` from `src/calibre/utils/msdes/spr.h`.
const SP7: [u32; 64] = [
    0x00004000, 0x00000200, 0x01000200, 0x01000004, 0x01004204, 0x00004004, 0x00004200, 0x00000000,
    0x01000000, 0x01000204, 0x00000204, 0x01004000, 0x00000004, 0x01004200, 0x01004000, 0x00000204,
    0x01000204, 0x00004000, 0x00004004, 0x01004204, 0x00000000, 0x01000200, 0x01000004, 0x00004200,
    0x01004004, 0x00004204, 0x01004200, 0x00000004, 0x00004204, 0x01004004, 0x00000200, 0x01000000,
    0x00004204, 0x01004000, 0x01004004, 0x00000204, 0x00004000, 0x00000200, 0x01000000, 0x01004004,
    0x01000204, 0x00004204, 0x00004200, 0x00000000, 0x00000200, 0x01000004, 0x00000004, 0x01000200,
    0x00000000, 0x01000204, 0x01000200, 0x00004200, 0x00000204, 0x00004000, 0x01004204, 0x01000000,
    0x01004200, 0x00000004, 0x00004004, 0x01004204, 0x01000004, 0x01004200, 0x01004000, 0x00004004,
];
/// `SP8` from `src/calibre/utils/msdes/spr.h`.
const SP8: [u32; 64] = [
    0x20800080, 0x20820000, 0x00020080, 0x00000000, 0x20020000, 0x00800080, 0x20800000, 0x20820080,
    0x00000080, 0x20000000, 0x00820000, 0x00020080, 0x00820080, 0x20020080, 0x20000080, 0x20800000,
    0x00020000, 0x00820080, 0x00800080, 0x20020000, 0x20820080, 0x20000080, 0x00000000, 0x00820000,
    0x20000000, 0x00800000, 0x20020080, 0x20800080, 0x00800000, 0x00020000, 0x20820000, 0x00000080,
    0x00800000, 0x00020000, 0x20000080, 0x20820080, 0x00020080, 0x20000000, 0x00000000, 0x00820000,
    0x20800080, 0x20020080, 0x20020000, 0x00800080, 0x20820000, 0x00000080, 0x00800080, 0x20020000,
    0x20820080, 0x00800000, 0x20800000, 0x20000080, 0x00820000, 0x00020080, 0x20020080, 0x20800000,
    0x00000080, 0x20820000, 0x00820080, 0x00000000, 0x20000000, 0x20800080, 0x00020000, 0x00820080,
];

/// An expanded DES key schedule.
///
/// Corresponds to `deskey(key, edf)` followed by the implicit `KnL`
/// global in `des.c`.
#[derive(Clone)]
pub struct DesKey {
    /// `KnL` — 32 cooked subkey words, two per round.
    kn: [u32; 32],
}

impl DesKey {
    /// `deskey` in `des.c`: build the schedule for an 8-byte key.
    ///
    /// `edf` selects the direction: [`EN0`] to encrypt, [`DE1`] to
    /// decrypt (which simply reverses the round order).
    // The permutation loops index several tables at once against the
    // same counter, exactly as `deskey` does.
    #[allow(clippy::needless_range_loop)]
    pub fn new(key: &[u8; 8], edf: i16) -> Self {
        let mut pc1m = [0u8; 56];
        let mut pcr = [0u8; 56];
        let mut kn = [0u32; 32];

        for (j, pc1j) in PC1.iter().enumerate() {
            let l = *pc1j as usize;
            let m = l & 7;
            pc1m[j] = u8::from(key[l >> 3] & BYTEBIT[m] != 0);
        }
        for i in 0..16 {
            let m = if edf == DE1 { (15 - i) << 1 } else { i << 1 };
            let n = m + 1;
            kn[m] = 0;
            kn[n] = 0;
            for j in 0..28 {
                let l = j + TOTROT[i] as usize;
                pcr[j] = if l < 28 { pc1m[l] } else { pc1m[l - 28] };
            }
            for j in 28..56 {
                let l = j + TOTROT[i] as usize;
                pcr[j] = if l < 56 { pc1m[l] } else { pc1m[l - 28] };
            }
            for j in 0..24 {
                // `bigbyte[j]` is 0x800000 >> j.
                let bigbyte = 0x0080_0000u32 >> j;
                if pcr[PC2[j] as usize] != 0 {
                    kn[m] |= bigbyte;
                }
                if pcr[PC2[j + 24] as usize] != 0 {
                    kn[n] |= bigbyte;
                }
            }
        }
        DesKey { kn: cookey(&kn) }
    }

    /// `des` in `des.c`: transform a single 8-byte block.
    pub fn process_block(&self, block: &[u8; BLOCK_SIZE]) -> [u8; BLOCK_SIZE] {
        let mut work = scrunch(block);
        desfunc(&mut work, &self.kn);
        unscrun(&work)
    }

    /// `msdes_des` in `msdesmodule.c`: transform a whole buffer, one
    /// block at a time (ECB).
    ///
    /// Returns `None` when `data` is empty or not a whole number of
    /// blocks, matching the `MsDesError` the Python raises.
    pub fn process(&self, data: &[u8]) -> Option<Vec<u8>> {
        if data.is_empty() || !data.len().is_multiple_of(BLOCK_SIZE) {
            return None;
        }
        let mut out = Vec::with_capacity(data.len());
        for chunk in data.chunks_exact(BLOCK_SIZE) {
            let mut block = [0u8; BLOCK_SIZE];
            block.copy_from_slice(chunk);
            out.extend_from_slice(&self.process_block(&block));
        }
        Some(out)
    }
}

/// `cookey` in `des.c` — fold the raw subkeys into the form `desfunc`
/// indexes directly.
fn cookey(raw: &[u32; 32]) -> [u32; 32] {
    let mut dough = [0u32; 32];
    for i in 0..16 {
        let (r0, r1) = (raw[2 * i], raw[2 * i + 1]);
        dough[2 * i] = ((r0 & 0x00fc_0000) << 6)
            | ((r0 & 0x0000_0fc0) << 10)
            | ((r1 & 0x00fc_0000) >> 10)
            | ((r1 & 0x0000_0fc0) >> 6);
        dough[2 * i + 1] = ((r0 & 0x0003_f000) << 12)
            | ((r0 & 0x0000_003f) << 16)
            | ((r1 & 0x0003_f000) >> 4)
            | (r1 & 0x0000_003f);
    }
    dough
}

/// `scrunch` in `des.c` — 8 bytes to two big-endian words.
fn scrunch(block: &[u8; BLOCK_SIZE]) -> [u32; 2] {
    [
        u32::from_be_bytes([block[0], block[1], block[2], block[3]]),
        u32::from_be_bytes([block[4], block[5], block[6], block[7]]),
    ]
}

/// `unscrun` in `des.c` — two big-endian words back to 8 bytes.
fn unscrun(work: &[u32; 2]) -> [u8; BLOCK_SIZE] {
    let mut out = [0u8; BLOCK_SIZE];
    out[..4].copy_from_slice(&work[0].to_be_bytes());
    out[4..].copy_from_slice(&work[1].to_be_bytes());
    out
}

/// `desfunc` in `des.c` — the sixteen Feistel rounds, bracketed by the
/// initial and inverse permutations.
fn desfunc(block: &mut [u32; 2], keys: &[u32; 32]) {
    let mut leftt = block[0];
    let mut right = block[1];

    let mut work = ((leftt >> 4) ^ right) & 0x0f0f_0f0f;
    right ^= work;
    leftt ^= work << 4;
    work = ((leftt >> 16) ^ right) & 0x0000_ffff;
    right ^= work;
    leftt ^= work << 16;
    work = ((right >> 2) ^ leftt) & 0x3333_3333;
    leftt ^= work;
    right ^= work << 2;
    work = ((right >> 8) ^ leftt) & 0x00ff_00ff;
    leftt ^= work;
    right ^= work << 8;
    right = right.rotate_left(1);
    work = (leftt ^ right) & 0xaaaa_aaaa;
    leftt ^= work;
    right ^= work;
    leftt = leftt.rotate_left(1);

    let mut k = keys.iter();
    for _ in 0..8 {
        let mut fval;
        work = right.rotate_right(4) ^ k.next().copied().unwrap_or(0);
        fval = SP7[(work & 0x3f) as usize]
            | SP5[((work >> 8) & 0x3f) as usize]
            | SP3[((work >> 16) & 0x3f) as usize]
            | SP1[((work >> 24) & 0x3f) as usize];
        work = right ^ k.next().copied().unwrap_or(0);
        fval |= SP8[(work & 0x3f) as usize]
            | SP6[((work >> 8) & 0x3f) as usize]
            | SP4[((work >> 16) & 0x3f) as usize]
            | SP2[((work >> 24) & 0x3f) as usize];
        leftt ^= fval;

        work = leftt.rotate_right(4) ^ k.next().copied().unwrap_or(0);
        fval = SP7[(work & 0x3f) as usize]
            | SP5[((work >> 8) & 0x3f) as usize]
            | SP3[((work >> 16) & 0x3f) as usize]
            | SP1[((work >> 24) & 0x3f) as usize];
        work = leftt ^ k.next().copied().unwrap_or(0);
        fval |= SP8[(work & 0x3f) as usize]
            | SP6[((work >> 8) & 0x3f) as usize]
            | SP4[((work >> 16) & 0x3f) as usize]
            | SP2[((work >> 24) & 0x3f) as usize];
        right ^= fval;
    }

    right = right.rotate_right(1);
    work = (leftt ^ right) & 0xaaaa_aaaa;
    leftt ^= work;
    right ^= work;
    leftt = leftt.rotate_right(1);
    work = ((leftt >> 8) ^ right) & 0x00ff_00ff;
    right ^= work;
    leftt ^= work << 8;
    work = ((leftt >> 2) ^ right) & 0x3333_3333;
    right ^= work;
    leftt ^= work << 2;
    work = ((right >> 16) ^ leftt) & 0x0000_ffff;
    leftt ^= work;
    right ^= work << 16;
    work = ((right >> 4) ^ leftt) & 0x0f0f_0f0f;
    leftt ^= work;
    right ^= work << 4;

    block[0] = right;
    block[1] = leftt;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The S-boxes in `spr.h` are Microsoft's modified set, not the
    /// stock D3DES ones, so the validation sets in the comment at the
    /// bottom of `des.c` no longer apply. This is what calibre's C
    /// actually produces for that key/plaintext pair; the full
    /// comparison lives in `tests/msdes_cross_test.rs`.
    #[test]
    fn matches_calibres_c_for_the_classic_key() {
        let key = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef];
        let plain = [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xe7];
        let cipher = [0x06, 0xc0, 0xe8, 0x99, 0x8c, 0x38, 0x60, 0x87];

        assert_eq!(DesKey::new(&key, EN0).process_block(&plain), cipher);
        assert_eq!(DesKey::new(&key, DE1).process_block(&cipher), plain);
    }

    #[test]
    fn round_trips_multi_block_buffers() {
        let key = [0u8; 8];
        let enc = DesKey::new(&key, EN0);
        let dec = DesKey::new(&key, DE1);
        let data: Vec<u8> = (0u8..64).collect();
        let ct = enc.process(&data).expect("whole blocks");
        assert_eq!(ct.len(), data.len());
        assert_ne!(ct, data);
        assert_eq!(dec.process(&ct).expect("whole blocks"), data);
    }

    #[test]
    fn rejects_inputs_that_are_not_whole_blocks() {
        let enc = DesKey::new(&[0u8; 8], EN0);
        assert!(enc.process(b"").is_none());
        assert!(enc.process(b"1234567").is_none());
        assert!(enc.process(b"123456789").is_none());
        assert!(enc.process(b"12345678").is_some());
    }

    #[test]
    fn ecb_blocks_are_independent() {
        let enc = DesKey::new(&[1, 2, 3, 4, 5, 6, 7, 8], EN0);
        let one = enc.process(b"abcdefgh").expect("whole blocks");
        let two = enc.process(b"abcdefghabcdefgh").expect("whole blocks");
        assert_eq!(two[..8], one[..]);
        assert_eq!(two[8..], one[..]);
    }

    #[test]
    fn direction_changes_the_schedule() {
        let key = [0x0f, 0x1e, 0x2d, 0x3c, 0x4b, 0x5a, 0x69, 0x78];
        let a = DesKey::new(&key, EN0);
        let b = DesKey::new(&key, DE1);
        assert_ne!(a.process_block(b"12345678"), b.process_block(b"12345678"));
    }
}
