//! LZX compression and decompression.
//!
//! Port of `src/calibre/utils/lzx/`, which calibre builds as the
//! `calibre_extensions.lzx` C extension:
//!
//!  * decompression from libmspack (`lzxd.c`), (C) 2003-2004 Stuart Caie
//!  * compression from lzxcomp (`lzxc.c`, `lzc.c`), (C) 2002 Matthew T.
//!    Russotto
//!
//! Both halves are used by the LIT reader and writer. LZX's own
//! window-size range, 2^15 to 2^21, is enforced here as it is in the C.

mod compress;
mod decompress;
mod huffman;
mod lz;

pub use compress::{Compressor, ResetEntry};
pub use decompress::{decompress, Decompressor};

/// The size of a frame in LZX. `LZX_FRAME_SIZE`.
pub const LZX_FRAME_SIZE: usize = 32768;

/// `LZXError` in `lzxmodule.c`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LzxError {
    /// `MSPACK_ERR_ARGS` — a bad window size or output length.
    #[error("invalid LZX parameter: {0}")]
    Args(&'static str),
    /// `MSPACK_ERR_READ` — the compressed stream ended early.
    #[error("LZX input ended unexpectedly")]
    Read,
    /// `MSPACK_ERR_DECRUNCH` — the stream is malformed.
    #[error("LZX decompression failed: {0}")]
    Decrunch(&'static str),
}
