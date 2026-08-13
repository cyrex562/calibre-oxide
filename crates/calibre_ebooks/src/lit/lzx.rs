//! LZX compression/decompression wrapper.
//!
//! Port of `src/calibre/ebooks/lit/lzx.py`, which is a thin shim over
//! the `calibre_extensions.lzx` C extension. The codec itself is ported
//! in [`calibre_utils::lzx`]; this module exists so the LIT code reads
//! the way the Python does.

pub use calibre_utils::lzx::{Compressor, Decompressor, LzxError, ResetEntry};

/// `lzx.decompress(data, outlen)` after `lzx.init(wbits)`.
pub use calibre_utils::lzx::decompress;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_exposes_both_directions() {
        let data = b"lit content, lit content, lit content".repeat(40);
        let mut c = Compressor::new(17).expect("supported window size");
        let (compressed, _) = c.compress(&data, true);
        let out = Decompressor::new(17)
            .decompress(&compressed, data.len())
            .expect("round trip");
        assert_eq!(out, data);
    }

    #[test]
    fn reset_is_a_no_op_as_in_the_c() {
        let d = Decompressor::new(17);
        d.reset();
        assert_eq!(d.wbits, 17);
        assert_eq!(d.blocksize, 1 << 17);
    }
}
