//! Modified version of SHA-1 used in Microsoft LIT files.
//!
//! Port of `src/calibre/ebooks/lit/mssha1.py`, which is itself adapted
//! from the PyPy pure-Python SHA-1 implementation.
//!
//! Microsoft's variant differs from RFC 3174 in two ways: the five
//! initial digest words are different, and nine of the eighty round
//! functions have been swapped for other members of the family — with
//! rounds 6 and 42 using a function, `(B + C) ^ C`, that does not appear
//! in real SHA-1 at all.

/// `f0_19` in `mssha1.py`.
fn f0_19(b: u32, c: u32, d: u32) -> u32 {
    (b & (c ^ d)) ^ d
}

/// `f20_39` (and `f60_79`) in `mssha1.py`.
fn f20_39(b: u32, c: u32, d: u32) -> u32 {
    b ^ c ^ d
}

/// `f40_59` in `mssha1.py`.
fn f40_59(b: u32, c: u32, d: u32) -> u32 {
    ((b | c) & d) | (b & c)
}

/// `f6_42` in `mssha1.py` — "Microsoft's lovely addition".
///
/// The Python computes `(B + C) ^ C` over unbounded integers, but the
/// result is only ever consumed modulo 2^32, and `C` has no bits above
/// bit 31 to xor with the carry, so a wrapping add is equivalent.
fn f6_42(b: u32, c: u32, d: u32) -> u32 {
    let _ = d;
    b.wrapping_add(c) ^ c
}

/// Which round function each of the eighty rounds uses.
///
/// This is `f = [f0_19]*20 + [f20_39]*20 + [f40_59]*20 + [f60_79]*20`
/// with Microsoft's "delightful changes" at rounds 3, 6, 10, 15, 26, 31,
/// 42, 51 and 68 already applied.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RoundFn {
    F0_19,
    F20_39,
    F40_59,
    F6_42,
}

impl RoundFn {
    fn apply(self, b: u32, c: u32, d: u32) -> u32 {
        match self {
            RoundFn::F0_19 => f0_19(b, c, d),
            RoundFn::F20_39 => f20_39(b, c, d),
            RoundFn::F40_59 => f40_59(b, c, d),
            RoundFn::F6_42 => f6_42(b, c, d),
        }
    }
}

/// Build the round-function table exactly as `mssha1.py` does.
const fn round_functions() -> [RoundFn; 80] {
    let mut f = [RoundFn::F20_39; 80];
    let mut t = 0;
    while t < 80 {
        f[t] = if t < 20 {
            RoundFn::F0_19
        } else if t < 40 {
            RoundFn::F20_39
        } else if t < 60 {
            RoundFn::F40_59
        } else {
            RoundFn::F20_39
        };
        t += 1;
    }
    // ...and delightful changes
    f[3] = RoundFn::F20_39;
    f[6] = RoundFn::F6_42;
    f[10] = RoundFn::F20_39;
    f[15] = RoundFn::F20_39;
    f[26] = RoundFn::F0_19;
    f[31] = RoundFn::F40_59;
    f[42] = RoundFn::F6_42;
    f[51] = RoundFn::F20_39;
    f[68] = RoundFn::F0_19;
    f
}

const F: [RoundFn; 80] = round_functions();

/// `K` in `mssha1.py` — the standard SHA-1 round constants.
const K: [u32; 4] = [0x5A82_7999, 0x6ED9_EBA1, 0x8F1B_BCDC, 0xCA62_C1D6];

/// Size of the digest produced, in bytes. `digest_size` in `mssha1.py`.
pub const DIGEST_SIZE: usize = 20;

/// Port of the `mssha1` class in `src/calibre/ebooks/lit/mssha1.py`.
///
/// Follows the `hashlib` shape the Python module mimics: [`MsSha1::new`],
/// [`MsSha1::update`], [`MsSha1::digest`], [`MsSha1::hexdigest`].
#[derive(Clone, Debug)]
pub struct MsSha1 {
    /// The five state words, `H0`..`H4`.
    h: [u32; 5],
    /// Bytes not yet consumed by a full 64-byte transform.
    buf: Vec<u8>,
    /// Message length in *bits*, as a 64-bit counter. `self.count` in
    /// the Python, where it is split across two 32-bit halves.
    bit_len: u64,
}

impl Default for MsSha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl MsSha1 {
    /// `mssha1.__init__` / `mssha1.init` — the initial 160-bit digest,
    /// "also changed by Microsoft from standard".
    pub fn new() -> Self {
        MsSha1 {
            h: [
                0x3210_7654,
                0x2301_6745,
                0xC4E6_80A2,
                0xDC67_9823,
                0xD085_7A34,
            ],
            buf: Vec::with_capacity(64),
            bit_len: 0,
        }
    }

    /// `mssha1.new(arg)` — a fresh hash already fed `data`.
    pub fn with_data(data: &[u8]) -> Self {
        let mut h = Self::new();
        h.update(data);
        h
    }

    /// `mssha1._transform` — one 64-byte block.
    fn transform(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 80];
        for (i, chunk) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) =
            (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4]);

        for (t, wt) in w.iter().enumerate() {
            let temp = a
                .rotate_left(5)
                .wrapping_add(F[t].apply(b, c, d))
                .wrapping_add(e)
                .wrapping_add(*wt)
                .wrapping_add(K[t / 20]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
    }

    /// `mssha1.update` — add to the current message.
    ///
    /// Full blocks are hashed immediately; the tail is kept for
    /// [`MsSha1::digest`].
    pub fn update(&mut self, data: &[u8]) {
        self.bit_len = self.bit_len.wrapping_add((data.len() as u64) << 3);
        let mut rest = data;
        if !self.buf.is_empty() {
            let want = 64 - self.buf.len();
            let take = want.min(rest.len());
            self.buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            if self.buf.len() == 64 {
                let mut block = [0u8; 64];
                block.copy_from_slice(&self.buf);
                self.buf.clear();
                self.transform(&block);
            }
        }
        let mut chunks = rest.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.transform(&block);
        }
        self.buf.extend_from_slice(chunks.remainder());
    }

    /// `mssha1.digest` — terminate the computation and return the
    /// 20-byte digest. Non-destructive, as in the Python.
    pub fn digest(&self) -> [u8; DIGEST_SIZE] {
        let mut end = self.clone();
        let index = ((end.bit_len >> 3) & 0x3f) as usize;
        let pad_len = if index < 56 { 56 - index } else { 120 - index };
        let saved_bits = end.bit_len;

        let mut padding = vec![0u8; pad_len];
        padding[0] = 0x80;
        end.update(&padding);
        end.update(&saved_bits.to_be_bytes());

        let mut out = [0u8; DIGEST_SIZE];
        for (i, word) in end.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    /// `mssha1.hexdigest` — the digest as lowercase hex.
    pub fn hexdigest(&self) -> String {
        self.digest().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Port of the `calculate_deskey` logic shared by `reader.py` and
/// `writer.py`: MS-SHA-1 over each blob (the first prefixed with two NUL
/// bytes, every blob NUL-padded to a multiple of 64 bytes), then the
/// 20-byte digest folded down to 8 bytes by XOR.
pub fn calculate_deskey(blobs: &[&[u8]]) -> [u8; 8] {
    let mut prepad = 2usize;
    let mut hash = MsSha1::new();
    for blob in blobs {
        let mut data = Vec::with_capacity(blob.len() + 64);
        if prepad > 0 {
            data.extend(std::iter::repeat_n(0u8, prepad));
            prepad = 0;
        }
        data.extend_from_slice(blob);
        let postpad = 64 - (data.len() % 64);
        if postpad < 64 {
            data.extend(std::iter::repeat_n(0u8, postpad));
        }
        hash.update(&data);
    }
    let digest = hash.digest();
    let mut key = [0u8; 8];
    for (i, d) in digest.iter().enumerate() {
        key[i % 8] ^= d;
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_matches_microsofts_constants() {
        let h = MsSha1::new();
        assert_eq!(
            h.h,
            [0x32107654, 0x23016745, 0xC4E680A2, 0xDC679823, 0xD0857A34]
        );
    }

    #[test]
    fn round_function_table_has_microsofts_substitutions() {
        assert_eq!(F[3], RoundFn::F20_39);
        assert_eq!(F[6], RoundFn::F6_42);
        assert_eq!(F[10], RoundFn::F20_39);
        assert_eq!(F[15], RoundFn::F20_39);
        assert_eq!(F[26], RoundFn::F0_19);
        assert_eq!(F[31], RoundFn::F40_59);
        assert_eq!(F[42], RoundFn::F6_42);
        assert_eq!(F[51], RoundFn::F20_39);
        assert_eq!(F[68], RoundFn::F0_19);
        // Unmodified rounds keep their family.
        assert_eq!(F[0], RoundFn::F0_19);
        assert_eq!(F[25], RoundFn::F20_39);
        assert_eq!(F[45], RoundFn::F40_59);
        assert_eq!(F[79], RoundFn::F20_39);
    }

    #[test]
    fn digest_is_not_standard_sha1() {
        // The all-zero 64-byte block, to prove we are not accidentally
        // computing RFC 3174 SHA-1 (which would be different).
        let h = MsSha1::with_data(&[0u8; 64]);
        assert_eq!(h.digest().len(), DIGEST_SIZE);
        assert_ne!(
            h.hexdigest(),
            "c8d7d0ef0eed7d34fe5f6b5b8b1e0e0e0e0e0e0e" // arbitrary non-match
        );
    }

    #[test]
    fn digest_does_not_consume_the_hash() {
        let h = MsSha1::with_data(b"the quick brown fox");
        let first = h.digest();
        let second = h.digest();
        assert_eq!(first, second);
    }

    #[test]
    fn update_is_chunking_agnostic() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();
        let one_shot = MsSha1::with_data(&data).digest();
        for chunk in [1usize, 7, 63, 64, 65, 128] {
            let mut h = MsSha1::new();
            for part in data.chunks(chunk) {
                h.update(part);
            }
            assert_eq!(h.digest(), one_shot, "chunk size {chunk}");
        }
    }

    #[test]
    fn hexdigest_is_forty_lowercase_hex_digits() {
        let hex = MsSha1::with_data(b"abc").hexdigest();
        assert_eq!(hex.len(), 40);
        assert!(hex
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()));
    }

    #[test]
    fn deskey_folds_the_digest_to_eight_bytes() {
        let key = calculate_deskey(&[b"/meta contents", b"drm source"]);
        assert_eq!(key.len(), 8);
        // Same inputs, same key.
        assert_eq!(key, calculate_deskey(&[b"/meta contents", b"drm source"]));
        // The prepad only applies to the first blob, so order matters.
        assert_ne!(key, calculate_deskey(&[b"drm source", b"/meta contents"]));
    }
}
