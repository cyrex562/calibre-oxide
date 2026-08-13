//! Cross-validation of the LZX codec against calibre's C extension.
//!
//! The vectors in `data/lzx_vectors.rs` are produced by compiling
//! `old_src/src/calibre/utils/lzx/{lzxc.c,lzc.c}` and driving them
//! exactly as `compressor.c` does for
//! `Compressor(wbits).compress(data, flush=True)` — same block loop,
//! same per-block reset, same `mark_frame` bookkeeping.
//!
//! Three properties are checked, which together pin both halves of the
//! codec to the C:
//!
//!  1. the Rust compressor emits byte-identical output and an identical
//!     reset table;
//!  2. the Rust decompressor reproduces the original from the *C's*
//!     compressed bytes;
//!  3. the two halves agree with each other.

#[path = "data/lzx_vectors.rs"]
mod vectors;

use calibre_utils::lzx::{decompress, Compressor, ResetEntry, LZX_FRAME_SIZE};

/// Decode a whole stream the way `LitFile.decompress` does: the
/// compressor resets after every `blocksize` bytes, so each block is an
/// independent LZX stream and the reset table says where each starts.
fn decompress_blocks(
    compressed: &[u8],
    rtable: &[ResetEntry],
    wbits: u32,
    total_len: usize,
) -> Vec<u8> {
    let blocksize = 1usize << wbits;
    let mut out = Vec::with_capacity(total_len);
    let mut base = 0usize;
    let mut remaining = total_len;
    for &(uncomp, comp) in rtable {
        if remaining < blocksize {
            break;
        }
        if !(uncomp as usize).is_multiple_of(blocksize) {
            continue;
        }
        let chunk = decompress(&compressed[base..comp as usize], wbits, blocksize)
            .expect("block decompresses");
        out.extend_from_slice(&chunk);
        base = comp as usize;
        remaining -= blocksize;
    }
    if remaining > 0 {
        let chunk = decompress(&compressed[base..], wbits, remaining).expect("tail decompresses");
        out.extend_from_slice(&chunk);
    }
    out
}

#[test]
fn compressor_output_is_byte_identical_to_calibres() {
    let mut mismatches = Vec::new();
    for v in vectors::LZX_VECTORS {
        let mut c = Compressor::new(v.wbits).expect("supported window size");
        let (compressed, rtable) = c.compress(v.plain, true);
        if compressed != v.compressed {
            let at = compressed
                .iter()
                .zip(v.compressed.iter())
                .position(|(a, b)| a != b);
            mismatches.push(format!(
                "{}: {} bytes vs calibre's {}, first difference at {at:?}",
                v.name,
                compressed.len(),
                v.compressed.len()
            ));
        }
        let expected: Vec<(u32, u32)> = v.rtable.to_vec();
        if rtable != expected {
            mismatches.push(format!(
                "{}: reset table {rtable:?} vs calibre's {expected:?}",
                v.name
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors::LZX_VECTORS.len(),
        mismatches.join("\n")
    );
}

#[test]
fn decompressor_reads_streams_produced_by_calibres_compressor() {
    for v in vectors::LZX_VECTORS {
        if v.plain.is_empty() {
            continue;
        }
        let out = decompress_blocks(v.compressed, v.rtable, v.wbits, v.plain.len());
        assert_eq!(out, v.plain, "{}", v.name);
    }
}

#[test]
fn the_two_halves_round_trip_each_other() {
    for v in vectors::LZX_VECTORS {
        if v.plain.is_empty() {
            continue;
        }
        let mut c = Compressor::new(v.wbits).expect("supported window size");
        let (compressed, rtable) = c.compress(v.plain, true);
        let out = decompress_blocks(&compressed, &rtable, v.wbits, v.plain.len());
        assert_eq!(out, v.plain, "{}", v.name);
    }
}

#[test]
fn the_reset_table_has_one_entry_per_started_frame() {
    // `writer.py` drops the last entry when building the LIT reset
    // table, so the count matters: input is zero-padded up to a frame
    // boundary, giving `ceil(len / 32768)` marks.
    for v in vectors::LZX_VECTORS {
        assert_eq!(
            v.rtable.len(),
            v.plain.len().div_ceil(LZX_FRAME_SIZE),
            "{}",
            v.name
        );
    }
}
