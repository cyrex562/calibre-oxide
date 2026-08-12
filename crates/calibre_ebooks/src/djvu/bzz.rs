//! BZZ decoder — the general-purpose compressor used by DjVu for text
//! (`TXTz`) and annotation (`ANTz`) chunks.
//!
//! Port of `old_src/src/calibre/ebooks/djvu/djvubzzdec.py` (the pure
//! Python decoder) and `old_src/src/calibre/ebooks/djvu/bzzdecoder.c`
//! (the C accelerator calibre actually ships, exposed to Python as
//! `calibre_extensions.bzzdec.decompress`). Both descend from Leon
//! Bottou's DjVuLibre `ZPCodec.{cpp,h}` and `BSByteStream.{cpp,h}`.
//!
//! BZZ is a block-sorting compressor: each block is a Burrows-Wheeler
//! transform of the input, move-to-front coded, then entropy coded with
//! the ZP-coder (an adaptive binary arithmetic coder). Decoding reverses
//! that: ZP-decode the MTF symbols, un-MTF them, then invert the BWT
//! using the encoded marker position.
//!
//! The two Python-side implementations differ in one respect and we
//! follow the C one, since that is the path calibre takes at runtime:
//! [`decompress`] strips the three-byte big-endian length prefix that
//! DjVu text records carry, and the pure-Python `BZZDecoder` does not.
//! Callers wanting the raw block stream can use [`BzzDecoder`] directly.

use thiserror::Error;

/// Largest block a stream may declare, in bytes (4 MiB). Matches
/// `MAXBLOCK * 1024` in both the C and the Python decoder.
const MAX_BLOCK: usize = 4096 * 1024;
/// Number of move-to-front frequencies tracked for the adaptive rotate.
const FREQMAX: usize = 4;
/// Number of ZP contexts reserved for the "distance from front" symbol.
const CTXIDS: usize = 3;
/// Size of the ZP context bank used by the block decoder.
const NCTX: usize = 300;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BzzError {
    /// The bitstream does not decode to a well-formed block. Carries the
    /// specific invariant that failed, mirroring the C decoder's
    /// "Corrupt bitstream at line: N" but in words rather than line
    /// numbers.
    #[error("corrupt BZZ bitstream: {0}")]
    Corrupt(&'static str),
    /// Ran off the end of the input with more than the ZP coder's
    /// tolerated 25 bytes of synthetic `0xff` padding consumed.
    #[error("unexpected end of BZZ input")]
    UnexpectedEof,
    /// A block declared a size larger than [`MAX_BLOCK`].
    #[error("BZZ block size {0} exceeds the {MAX_BLOCK} byte maximum")]
    BlockTooLarge(u32),
}

/// One entry of the ZP-coder probability table: interval width `p`,
/// adaptation threshold `m`, and the next state on a more/less probable
/// symbol (`up`/`dn`).
#[derive(Clone, Copy)]
struct ZEntry {
    p: u32,
    m: u32,
    up: u8,
    dn: u8,
}

const fn z(p: u32, m: u32, up: u8, dn: u8) -> ZEntry {
    ZEntry { p, m, up, dn }
}

/// The ZP-coder's default state table.
///
/// From DjVuLibre: "This table has been designed for the ZPCoder by
/// running the following command in file 'zptable.sn':
/// `(fast-crude (steady-mat 0.0035 0.0002) 260)`".
#[rustfmt::skip]
static DEFAULT_ZTABLE: [ZEntry; 256] = [
    z(0x8000, 0x0000,  84, 145), // 000
    z(0x8000, 0x0000,   3,   4), // 001
    z(0x8000, 0x0000,   4,   3), // 002
    z(0x6bbd, 0x10a5,   5,   1), // 003
    z(0x6bbd, 0x10a5,   6,   2), // 004
    z(0x5d45, 0x1f28,   7,   3), // 005
    z(0x5d45, 0x1f28,   8,   4), // 006
    z(0x51b9, 0x2bd3,   9,   5), // 007
    z(0x51b9, 0x2bd3,  10,   6), // 008
    z(0x4813, 0x36e3,  11,   7), // 009
    z(0x4813, 0x36e3,  12,   8), // 010
    z(0x3fd5, 0x408c,  13,   9), // 011
    z(0x3fd5, 0x408c,  14,  10), // 012
    z(0x38b1, 0x48fd,  15,  11), // 013
    z(0x38b1, 0x48fd,  16,  12), // 014
    z(0x3275, 0x505d,  17,  13), // 015
    z(0x3275, 0x505d,  18,  14), // 016
    z(0x2cfd, 0x56d0,  19,  15), // 017
    z(0x2cfd, 0x56d0,  20,  16), // 018
    z(0x2825, 0x5c71,  21,  17), // 019
    z(0x2825, 0x5c71,  22,  18), // 020
    z(0x23ab, 0x615b,  23,  19), // 021
    z(0x23ab, 0x615b,  24,  20), // 022
    z(0x1f87, 0x65a5,  25,  21), // 023
    z(0x1f87, 0x65a5,  26,  22), // 024
    z(0x1bbb, 0x6962,  27,  23), // 025
    z(0x1bbb, 0x6962,  28,  24), // 026
    z(0x1845, 0x6ca2,  29,  25), // 027
    z(0x1845, 0x6ca2,  30,  26), // 028
    z(0x1523, 0x6f74,  31,  27), // 029
    z(0x1523, 0x6f74,  32,  28), // 030
    z(0x1253, 0x71e6,  33,  29), // 031
    z(0x1253, 0x71e6,  34,  30), // 032
    z(0x0fcf, 0x7404,  35,  31), // 033
    z(0x0fcf, 0x7404,  36,  32), // 034
    z(0x0d95, 0x75d6,  37,  33), // 035
    z(0x0d95, 0x75d6,  38,  34), // 036
    z(0x0b9d, 0x7768,  39,  35), // 037
    z(0x0b9d, 0x7768,  40,  36), // 038
    z(0x09e3, 0x78c2,  41,  37), // 039
    z(0x09e3, 0x78c2,  42,  38), // 040
    z(0x0861, 0x79ea,  43,  39), // 041
    z(0x0861, 0x79ea,  44,  40), // 042
    z(0x0711, 0x7ae7,  45,  41), // 043
    z(0x0711, 0x7ae7,  46,  42), // 044
    z(0x05f1, 0x7bbe,  47,  43), // 045
    z(0x05f1, 0x7bbe,  48,  44), // 046
    z(0x04f9, 0x7c75,  49,  45), // 047
    z(0x04f9, 0x7c75,  50,  46), // 048
    z(0x0425, 0x7d0f,  51,  47), // 049
    z(0x0425, 0x7d0f,  52,  48), // 050
    z(0x0371, 0x7d91,  53,  49), // 051
    z(0x0371, 0x7d91,  54,  50), // 052
    z(0x02d9, 0x7dfe,  55,  51), // 053
    z(0x02d9, 0x7dfe,  56,  52), // 054
    z(0x0259, 0x7e5a,  57,  53), // 055
    z(0x0259, 0x7e5a,  58,  54), // 056
    z(0x01ed, 0x7ea6,  59,  55), // 057
    z(0x01ed, 0x7ea6,  60,  56), // 058
    z(0x0193, 0x7ee6,  61,  57), // 059
    z(0x0193, 0x7ee6,  62,  58), // 060
    z(0x0149, 0x7f1a,  63,  59), // 061
    z(0x0149, 0x7f1a,  64,  60), // 062
    z(0x010b, 0x7f45,  65,  61), // 063
    z(0x010b, 0x7f45,  66,  62), // 064
    z(0x00d5, 0x7f6b,  67,  63), // 065
    z(0x00d5, 0x7f6b,  68,  64), // 066
    z(0x00a5, 0x7f8d,  69,  65), // 067
    z(0x00a5, 0x7f8d,  70,  66), // 068
    z(0x007b, 0x7faa,  71,  67), // 069
    z(0x007b, 0x7faa,  72,  68), // 070
    z(0x0057, 0x7fc3,  73,  69), // 071
    z(0x0057, 0x7fc3,  74,  70), // 072
    z(0x003b, 0x7fd7,  75,  71), // 073
    z(0x003b, 0x7fd7,  76,  72), // 074
    z(0x0023, 0x7fe7,  77,  73), // 075
    z(0x0023, 0x7fe7,  78,  74), // 076
    z(0x0013, 0x7ff2,  79,  75), // 077
    z(0x0013, 0x7ff2,  80,  76), // 078
    z(0x0007, 0x7ffa,  81,  77), // 079
    z(0x0007, 0x7ffa,  82,  78), // 080
    z(0x0001, 0x7fff,  81,  79), // 081
    z(0x0001, 0x7fff,  82,  80), // 082
    z(0x5695, 0x0000,   9,  85), // 083
    z(0x24ee, 0x0000,  86, 226), // 084
    z(0x8000, 0x0000,   5,   6), // 085
    z(0x0d30, 0x0000,  88, 176), // 086
    z(0x481a, 0x0000,  89, 143), // 087
    z(0x0481, 0x0000,  90, 138), // 088
    z(0x3579, 0x0000,  91, 141), // 089
    z(0x017a, 0x0000,  92, 112), // 090
    z(0x24ef, 0x0000,  93, 135), // 091
    z(0x007b, 0x0000,  94, 104), // 092
    z(0x1978, 0x0000,  95, 133), // 093
    z(0x0028, 0x0000,  96, 100), // 094
    z(0x10ca, 0x0000,  97, 129), // 095
    z(0x000d, 0x0000,  82,  98), // 096
    z(0x0b5d, 0x0000,  99, 127), // 097
    z(0x0034, 0x0000,  76,  72), // 098
    z(0x078a, 0x0000, 101, 125), // 099
    z(0x00a0, 0x0000,  70, 102), // 100
    z(0x050f, 0x0000, 103, 123), // 101
    z(0x0117, 0x0000,  66,  60), // 102
    z(0x0358, 0x0000, 105, 121), // 103
    z(0x01ea, 0x0000, 106, 110), // 104
    z(0x0234, 0x0000, 107, 119), // 105
    z(0x0144, 0x0000,  66, 108), // 106
    z(0x0173, 0x0000, 109, 117), // 107
    z(0x0234, 0x0000,  60,  54), // 108
    z(0x00f5, 0x0000, 111, 115), // 109
    z(0x0353, 0x0000,  56,  48), // 110
    z(0x00a1, 0x0000,  69, 113), // 111
    z(0x05c5, 0x0000, 114, 134), // 112
    z(0x011a, 0x0000,  65,  59), // 113
    z(0x03cf, 0x0000, 116, 132), // 114
    z(0x01aa, 0x0000,  61,  55), // 115
    z(0x0285, 0x0000, 118, 130), // 116
    z(0x0286, 0x0000,  57,  51), // 117
    z(0x01ab, 0x0000, 120, 128), // 118
    z(0x03d3, 0x0000,  53,  47), // 119
    z(0x011a, 0x0000, 122, 126), // 120
    z(0x05c5, 0x0000,  49,  41), // 121
    z(0x00ba, 0x0000, 124,  62), // 122
    z(0x08ad, 0x0000,  43,  37), // 123
    z(0x007a, 0x0000,  72,  66), // 124
    z(0x0ccc, 0x0000,  39,  31), // 125
    z(0x01eb, 0x0000,  60,  54), // 126
    z(0x1302, 0x0000,  33,  25), // 127
    z(0x02e6, 0x0000,  56,  50), // 128
    z(0x1b81, 0x0000,  29, 131), // 129
    z(0x045e, 0x0000,  52,  46), // 130
    z(0x24ef, 0x0000,  23,  17), // 131
    z(0x0690, 0x0000,  48,  40), // 132
    z(0x2865, 0x0000,  23,  15), // 133
    z(0x09de, 0x0000,  42, 136), // 134
    z(0x3987, 0x0000, 137,   7), // 135
    z(0x0dc8, 0x0000,  38,  32), // 136
    z(0x2c99, 0x0000,  21, 139), // 137
    z(0x10ca, 0x0000, 140, 172), // 138
    z(0x3b5f, 0x0000,  15,   9), // 139
    z(0x0b5d, 0x0000, 142, 170), // 140
    z(0x5695, 0x0000,   9,  85), // 141
    z(0x078a, 0x0000, 144, 168), // 142
    z(0x8000, 0x0000, 141, 248), // 143
    z(0x050f, 0x0000, 146, 166), // 144
    z(0x24ee, 0x0000, 147, 247), // 145
    z(0x0358, 0x0000, 148, 164), // 146
    z(0x0d30, 0x0000, 149, 197), // 147
    z(0x0234, 0x0000, 150, 162), // 148
    z(0x0481, 0x0000, 151,  95), // 149
    z(0x0173, 0x0000, 152, 160), // 150
    z(0x017a, 0x0000, 153, 173), // 151
    z(0x00f5, 0x0000, 154, 158), // 152
    z(0x007b, 0x0000, 155, 165), // 153
    z(0x00a1, 0x0000,  70, 156), // 154
    z(0x0028, 0x0000, 157, 161), // 155
    z(0x011a, 0x0000,  66,  60), // 156
    z(0x000d, 0x0000,  81, 159), // 157
    z(0x01aa, 0x0000,  62,  56), // 158
    z(0x0034, 0x0000,  75,  71), // 159
    z(0x0286, 0x0000,  58,  52), // 160
    z(0x00a0, 0x0000,  69, 163), // 161
    z(0x03d3, 0x0000,  54,  48), // 162
    z(0x0117, 0x0000,  65,  59), // 163
    z(0x05c5, 0x0000,  50,  42), // 164
    z(0x01ea, 0x0000, 167, 171), // 165
    z(0x08ad, 0x0000,  44,  38), // 166
    z(0x0144, 0x0000,  65, 169), // 167
    z(0x0ccc, 0x0000,  40,  32), // 168
    z(0x0234, 0x0000,  59,  53), // 169
    z(0x1302, 0x0000,  34,  26), // 170
    z(0x0353, 0x0000,  55,  47), // 171
    z(0x1b81, 0x0000,  30, 174), // 172
    z(0x05c5, 0x0000, 175, 193), // 173
    z(0x24ef, 0x0000,  24,  18), // 174
    z(0x03cf, 0x0000, 177, 191), // 175
    z(0x2b74, 0x0000, 178, 222), // 176
    z(0x0285, 0x0000, 179, 189), // 177
    z(0x201d, 0x0000, 180, 218), // 178
    z(0x01ab, 0x0000, 181, 187), // 179
    z(0x1715, 0x0000, 182, 216), // 180
    z(0x011a, 0x0000, 183, 185), // 181
    z(0x0fb7, 0x0000, 184, 214), // 182
    z(0x00ba, 0x0000,  69,  61), // 183
    z(0x0a67, 0x0000, 186, 212), // 184
    z(0x01eb, 0x0000,  59,  53), // 185
    z(0x06e7, 0x0000, 188, 210), // 186
    z(0x02e6, 0x0000,  55,  49), // 187
    z(0x0496, 0x0000, 190, 208), // 188
    z(0x045e, 0x0000,  51,  45), // 189
    z(0x030d, 0x0000, 192, 206), // 190
    z(0x0690, 0x0000,  47,  39), // 191
    z(0x0206, 0x0000, 194, 204), // 192
    z(0x09de, 0x0000,  41, 195), // 193
    z(0x0155, 0x0000, 196, 202), // 194
    z(0x0dc8, 0x0000,  37,  31), // 195
    z(0x00e1, 0x0000, 198, 200), // 196
    z(0x2b74, 0x0000, 199, 243), // 197
    z(0x0094, 0x0000,  72,  64), // 198
    z(0x201d, 0x0000, 201, 239), // 199
    z(0x0188, 0x0000,  62,  56), // 200
    z(0x1715, 0x0000, 203, 237), // 201
    z(0x0252, 0x0000,  58,  52), // 202
    z(0x0fb7, 0x0000, 205, 235), // 203
    z(0x0383, 0x0000,  54,  48), // 204
    z(0x0a67, 0x0000, 207, 233), // 205
    z(0x0547, 0x0000,  50,  44), // 206
    z(0x06e7, 0x0000, 209, 231), // 207
    z(0x07e2, 0x0000,  46,  38), // 208
    z(0x0496, 0x0000, 211, 229), // 209
    z(0x0bc0, 0x0000,  40,  34), // 210
    z(0x030d, 0x0000, 213, 227), // 211
    z(0x1178, 0x0000,  36,  28), // 212
    z(0x0206, 0x0000, 215, 225), // 213
    z(0x19da, 0x0000,  30,  22), // 214
    z(0x0155, 0x0000, 217, 223), // 215
    z(0x24ef, 0x0000,  26,  16), // 216
    z(0x00e1, 0x0000, 219, 221), // 217
    z(0x320e, 0x0000,  20, 220), // 218
    z(0x0094, 0x0000,  71,  63), // 219
    z(0x432a, 0x0000,  14,   8), // 220
    z(0x0188, 0x0000,  61,  55), // 221
    z(0x447d, 0x0000,  14, 224), // 222
    z(0x0252, 0x0000,  57,  51), // 223
    z(0x5ece, 0x0000,   8,   2), // 224
    z(0x0383, 0x0000,  53,  47), // 225
    z(0x8000, 0x0000, 228,  87), // 226
    z(0x0547, 0x0000,  49,  43), // 227
    z(0x481a, 0x0000, 230, 246), // 228
    z(0x07e2, 0x0000,  45,  37), // 229
    z(0x3579, 0x0000, 232, 244), // 230
    z(0x0bc0, 0x0000,  39,  33), // 231
    z(0x24ef, 0x0000, 234, 238), // 232
    z(0x1178, 0x0000,  35,  27), // 233
    z(0x1978, 0x0000, 138, 236), // 234
    z(0x19da, 0x0000,  29,  21), // 235
    z(0x2865, 0x0000,  24,  16), // 236
    z(0x24ef, 0x0000,  25,  15), // 237
    z(0x3987, 0x0000, 240,   8), // 238
    z(0x320e, 0x0000,  19, 241), // 239
    z(0x2c99, 0x0000,  22, 242), // 240
    z(0x432a, 0x0000,  13,   7), // 241
    z(0x3b5f, 0x0000,  16,  10), // 242
    z(0x447d, 0x0000,  13, 245), // 243
    z(0x5695, 0x0000,  10,   2), // 244
    z(0x5ece, 0x0000,   7,   1), // 245
    z(0x8000, 0x0000, 244,  83), // 246
    z(0x8000, 0x0000, 249, 250), // 247
    z(0x5695, 0x0000,  10,   2), // 248
    z(0x481a, 0x0000,  89, 143), // 249
    z(0x481a, 0x0000, 230, 246), // 250
    z(0x0000, 0x0000,   0,   0), // 251
    z(0x0000, 0x0000,   0,   0), // 252
    z(0x0000, 0x0000,   0,   0), // 253
    z(0x0000, 0x0000,   0,   0), // 254
    z(0x0000, 0x0000,   0,   0), // 255
];

/// Streaming BZZ decoder over an in-memory bitstream.
///
/// Port of the Python `BZZDecoder` class. The Python version writes into
/// a caller-supplied `bytearray` and exposes `convert(size)`; the Rust
/// version hands back one decoded block at a time via [`next_block`],
/// which is where the natural boundary is anyway — the decoder cannot
/// produce output without decoding a whole block first.
///
/// [`next_block`]: BzzDecoder::next_block
pub struct BzzDecoder<'a> {
    input: &'a [u8],
    inptr: usize,
    byte: u8,
    /// Bits currently available in `buffer`.
    scount: u32,
    /// Countdown of synthetic `0xff` bytes tolerated past end of input.
    delay: i32,
    a: u32,
    code: u32,
    fence: u32,
    buffer: u32,
    ctx: [u8; NCTX],
    p: [u32; 256],
    m: [u32; 256],
    up: [u8; 256],
    dn: [u8; 256],
    /// Machine-independent "find first zero" lookup.
    ffzt: [u8; 256],
    outbuf: Vec<u8>,
    at_eof: bool,
}

impl<'a> BzzDecoder<'a> {
    /// Start decoding `input`, priming the ZP coder with the first 16
    /// bits of code and the lookahead buffer.
    pub fn new(input: &'a [u8]) -> Result<Self, BzzError> {
        let mut ffzt = [0u8; 256];
        for (i, slot) in ffzt.iter_mut().enumerate() {
            let mut j = i;
            while j & 0x80 != 0 {
                *slot += 1;
                j <<= 1;
            }
        }

        let mut p = [0u32; 256];
        let mut m = [0u32; 256];
        let mut up = [0u8; 256];
        let mut dn = [0u8; 256];
        for (i, e) in DEFAULT_ZTABLE.iter().enumerate() {
            p[i] = e.p;
            m[i] = e.m;
            up[i] = e.up;
            dn[i] = e.dn;
        }

        let mut dec = Self {
            input,
            inptr: 0,
            byte: 0,
            scount: 0,
            delay: 25,
            a: 0,
            code: 0,
            fence: 0,
            buffer: 0,
            ctx: [0; NCTX],
            p,
            m,
            up,
            dn,
            ffzt,
            outbuf: Vec::new(),
            at_eof: false,
        };

        // Read the first 16 bits of code. A truncated stream reads as
        // 0xff here rather than failing: the ZP coder tolerates a short
        // tail, and empty input then decodes to empty output.
        if !dec.read_byte() {
            dec.byte = 0xff;
        }
        dec.code = (dec.byte as u32) << 8;
        if !dec.read_byte() {
            dec.byte = 0xff;
        }
        dec.code |= dec.byte as u32;
        dec.preload()?;
        dec.update_fence();
        Ok(dec)
    }

    /// Decode the next block, or `None` once the end-of-stream marker
    /// has been read.
    ///
    /// The returned slice borrows the decoder's internal block buffer
    /// and is valid until the next call.
    pub fn next_block(&mut self) -> Result<Option<&[u8]>, BzzError> {
        if self.at_eof {
            return Ok(None);
        }
        let size = self.decode_block()?;
        if size == 0 {
            self.at_eof = true;
            return Ok(None);
        }
        // The last byte of a block is the BWT sentinel, never output.
        Ok(Some(&self.outbuf[..size - 1]))
    }

    /// Decode every remaining block, concatenated.
    pub fn decode_all(&mut self) -> Result<Vec<u8>, BzzError> {
        let mut out = Vec::new();
        while let Some(block) = self.next_block()? {
            out.extend_from_slice(block);
        }
        Ok(out)
    }

    // -- bitstream plumbing ------------------------------------------

    fn read_byte(&mut self) -> bool {
        match self.input.get(self.inptr) {
            Some(&b) => {
                self.byte = b;
                self.inptr += 1;
                true
            }
            None => false,
        }
    }

    fn preload(&mut self) -> Result<(), BzzError> {
        while self.scount <= 24 {
            if !self.read_byte() {
                self.byte = 0xff;
                self.delay -= 1;
                if self.delay < 1 {
                    return Err(BzzError::UnexpectedEof);
                }
            }
            self.buffer = (self.buffer << 8) | self.byte as u32;
            self.scount += 8;
        }
        Ok(())
    }

    fn update_fence(&mut self) {
        self.fence = self.code.min(0x7fff);
    }

    /// Number of leading one bits in the interval width, used to size
    /// the renormalization shift.
    fn ffz(&self) -> u32 {
        let x = self.a;
        if x >= 0xff00 {
            self.ffzt[(x & 0xff) as usize] as u32 + 8
        } else {
            self.ffzt[((x >> 8) & 0xff) as usize] as u32
        }
    }

    /// Pull `shift` fresh bits into `code` after an LPS decision.
    fn renormalize_lps(&mut self, shift: u32) -> Result<(), BzzError> {
        self.scount -= shift;
        self.a = (self.a << shift) & 0xffff;
        let fresh = (self.buffer >> self.scount) & ((1u32 << shift) - 1);
        self.code = ((self.code << shift) | fresh) & 0xffff;
        if self.scount < 16 {
            self.preload()?;
        }
        self.update_fence();
        Ok(())
    }

    /// Pull one fresh bit into `code` after an MPS decision.
    fn renormalize_mps(&mut self, z: u32) -> Result<(), BzzError> {
        self.scount -= 1;
        self.a = (z << 1) & 0xffff;
        self.code = ((self.code << 1) | ((self.buffer >> self.scount) & 1)) & 0xffff;
        if self.scount < 16 {
            self.preload()?;
        }
        self.update_fence();
        Ok(())
    }

    // -- ZP coder -----------------------------------------------------

    /// Context-free decode, used for the block header and the
    /// estimation-speed bits.
    fn decode_passthrough(&mut self) -> Result<u32, BzzError> {
        let z = 0x8000 + (self.a >> 1);
        if z > self.code {
            // LPS branch.
            let z = 0x10000 - z;
            self.a += z;
            self.code += z;
            let shift = self.ffz();
            self.renormalize_lps(shift)?;
            Ok(1)
        } else {
            self.renormalize_mps(z)?;
            Ok(0)
        }
    }

    /// Adaptive decode against context `index`.
    fn decode_ctx(&mut self, index: usize) -> Result<u32, BzzError> {
        let state = self.ctx[index] as usize;
        let z = self.a + self.p[state];
        if z <= self.fence {
            self.a = z;
            return Ok((self.ctx[index] & 1) as u32);
        }
        let bit = (self.ctx[index] & 1) as u32;
        // Avoid interval reversion.
        let z = z.min(0x6000 + ((z + self.a) >> 2));
        if z > self.code {
            // LPS branch.
            let z = 0x10000 - z;
            self.a += z;
            self.code += z;
            self.ctx[index] = self.dn[state];
            let shift = self.ffz();
            self.renormalize_lps(shift)?;
            Ok(bit ^ 1)
        } else {
            if self.a >= self.m[state] {
                self.ctx[index] = self.up[state];
            }
            self.renormalize_mps(z)?;
            Ok(bit)
        }
    }

    /// Decode `bits` bits without context modelling.
    fn decode_raw(&mut self, bits: u32) -> Result<u32, BzzError> {
        let m = 1u32 << bits;
        let mut n = 1u32;
        while n < m {
            let b = self.decode_passthrough()?;
            n = (n << 1) | b;
        }
        Ok(n - m)
    }

    /// Decode `bits` bits against the binary context tree rooted at
    /// `index` (which owns `2^bits - 1` contexts).
    fn decode_binary(&mut self, index: usize, bits: u32) -> Result<u32, BzzError> {
        let m = 1u32 << bits;
        let mut n = 1u32;
        while n < m {
            let b = self.decode_ctx(index + n as usize - 1)?;
            n = (n << 1) | b;
        }
        Ok(n - m)
    }

    // -- block decoding -----------------------------------------------

    /// Decode one block into `self.outbuf`, returning its size (0 at the
    /// end-of-stream marker).
    fn decode_block(&mut self) -> Result<usize, BzzError> {
        let xsize = self.decode_raw(24)?;
        if xsize == 0 {
            return Ok(0);
        }
        if xsize as usize > MAX_BLOCK {
            return Err(BzzError::BlockTooLarge(xsize));
        }
        let xsize = xsize as usize;
        self.outbuf.clear();
        self.outbuf.resize(xsize, 0);

        // Estimation speed, unary coded in up to two bits.
        let mut fshift = 0u32;
        if self.decode_passthrough()? != 0 {
            fshift += 1;
            if self.decode_passthrough()? != 0 {
                fshift += 1;
            }
        }

        let mut mtf: [u8; 256] = std::array::from_fn(|i| i as u8);
        let mut freq = [0u32; FREQMAX];
        let mut fadd: u32 = 4;
        let mut mtfno = 3usize;
        let mut markerpos: Option<usize> = None;

        for i in 0..xsize {
            // Decode the move-to-front distance: two direct contexts for
            // distances 0 and 1, then a binary tree per power-of-two
            // bucket, then the end-of-block marker.
            let ctxid = (CTXIDS - 1).min(mtfno);
            let mut decoded = None;
            if self.decode_ctx(ctxid)? != 0 {
                decoded = Some(0);
            } else if self.decode_ctx(ctxid + CTXIDS)? != 0 {
                decoded = Some(1);
            } else {
                let mut base = 2 * CTXIDS;
                for j in 1..8u32 {
                    if self.decode_ctx(base)? != 0 {
                        let bucket = 1usize << j;
                        decoded = Some(bucket + self.decode_binary(base + 1, j)? as usize);
                        break;
                    }
                    base += 1 << j;
                }
            }

            let Some(n) = decoded else {
                // End of block marker: records where the BWT rotation
                // that starts the original string landed.
                mtfno = 256;
                self.outbuf[i] = 0;
                markerpos = Some(i);
                continue;
            };
            mtfno = n;
            self.outbuf[i] = mtf[mtfno];

            // Rotate the MTF table according to empirical frequencies.
            fadd += fadd >> fshift;
            if fadd > 0x1000_0000 {
                fadd >>= 24;
                for f in freq.iter_mut() {
                    *f >>= 24;
                }
            }
            let mut fc = fadd;
            if mtfno < FREQMAX {
                fc += freq[mtfno];
            }
            let mut k = mtfno;
            while k >= FREQMAX {
                mtf[k] = mtf[k - 1];
                k -= 1;
            }
            while k > 0 && fc >= freq[k - 1] {
                mtf[k] = mtf[k - 1];
                freq[k] = freq[k - 1];
                k -= 1;
            }
            mtf[k] = self.outbuf[i];
            freq[k] = fc;
        }

        self.invert_bwt(xsize, markerpos)?;
        Ok(xsize)
    }

    /// Undo the Burrows-Wheeler sort transform in place over
    /// `self.outbuf[..xsize]`.
    fn invert_bwt(&mut self, xsize: usize, markerpos: Option<usize>) -> Result<(), BzzError> {
        let markerpos = markerpos.ok_or(BzzError::Corrupt("block has no end-of-block marker"))?;
        if markerpos < 1 || markerpos >= xsize {
            return Err(BzzError::Corrupt("marker position outside block"));
        }

        // posn[i] packs the byte value in the top 8 bits and its
        // occurrence number in the low 24.
        let mut posn = vec![0u32; xsize];
        let mut count = [0u32; 256];
        // The marker byte itself is skipped: it stands for the rotation
        // that starts the original string, not for a real byte.
        for i in (0..markerpos).chain(markerpos + 1..xsize) {
            let c = self.outbuf[i] as usize;
            posn[i] = ((c as u32) << 24) | (count[c] & 0xff_ffff);
            count[c] += 1;
        }

        // Turn occurrence counts into first-occurrence offsets.
        let mut last = 1u32;
        for c in count.iter_mut() {
            let tmp = *c;
            *c = last;
            last += tmp;
        }

        // Walk the permutation backwards, writing the original string.
        let mut i = 0usize;
        let mut last = xsize - 1;
        while last > 0 {
            let n = *posn
                .get(i)
                .ok_or(BzzError::Corrupt("BWT permutation left the block"))?;
            let c = (n >> 24) as u8;
            last -= 1;
            self.outbuf[last] = c;
            i = (count[c as usize] + (n & 0xff_ffff)) as usize;
        }
        if i != markerpos {
            return Err(BzzError::Corrupt("BWT permutation did not close the cycle"));
        }
        Ok(())
    }
}

/// Decompress a DjVu text record: BZZ-decode `raw`, then strip the
/// three-byte big-endian length prefix the record carries and truncate
/// to that length.
///
/// Port of `bzz_decompress` in
/// `old_src/src/calibre/ebooks/djvu/bzzdecoder.c`, which is what
/// `calibre.ebooks.djvu.djvu` calls as
/// `calibre_extensions.bzzdec.decompress`.
pub fn decompress(raw: &[u8]) -> Result<Vec<u8>, BzzError> {
    let decoded = BzzDecoder::new(raw)?.decode_all()?;
    if decoded.len() < 3 {
        return Ok(Vec::new());
    }
    let declared = decoded[..3]
        .iter()
        .fold(0usize, |acc, &b| (acc << 8) | b as usize);
    let end = (3 + declared).min(decoded.len());
    Ok(decoded[3..end].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_decodes_to_nothing() {
        // The ZP coder tolerates a short tail, so an empty stream is not
        // an error — it simply yields no blocks. Matches the Python
        // decoder, which returns an empty bytearray for b''.
        let mut dec = BzzDecoder::new(b"").expect("primes on empty input");
        assert_eq!(dec.decode_all().unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn ztable_matches_reference() {
        // Spot-check the transcribed ZP table against DjVuLibre's
        // zptable: first, last real, and a couple of interior rows.
        assert_eq!(
            (
                DEFAULT_ZTABLE[0].p,
                DEFAULT_ZTABLE[0].up,
                DEFAULT_ZTABLE[0].dn
            ),
            (0x8000, 84, 145)
        );
        assert_eq!((DEFAULT_ZTABLE[3].p, DEFAULT_ZTABLE[3].m), (0x6bbd, 0x10a5));
        assert_eq!(
            (
                DEFAULT_ZTABLE[250].p,
                DEFAULT_ZTABLE[250].up,
                DEFAULT_ZTABLE[250].dn
            ),
            (0x481a, 230, 246)
        );
        assert_eq!(DEFAULT_ZTABLE[255].p, 0);
    }

    #[test]
    fn all_ones_stream_decodes_to_nothing() {
        // An all-0xff stream reads as an immediate end-of-stream marker.
        // Verified against calibre's Python decoder, which likewise
        // returns b'' for this input rather than raising.
        assert_eq!(
            BzzDecoder::new(&[0xffu8; 64])
                .unwrap()
                .decode_all()
                .unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(
            BzzDecoder::new(&[0xffu8; 8]).unwrap().decode_all().unwrap(),
            Vec::<u8>::new()
        );
    }

    /// Inputs calibre's Python decoder rejects with
    /// `BZZDecoderError('BiteStream.corrupt')`. The Rust port
    /// distinguishes *why* the block is unusable, so the assertion is
    /// on rejection rather than on a single variant.
    #[test]
    fn garbage_streams_are_rejected() {
        for raw in [
            &[0x00u8; 8][..],
            &[0x00, 0x00, 0x30, 0x12][..],
            &[0x00][..],
            b"not a bzz stream at all",
        ] {
            let result = BzzDecoder::new(raw).unwrap().decode_all();
            assert!(
                result.is_err(),
                "expected {raw:?} to be rejected, got {result:?}"
            );
        }
    }

    #[test]
    fn short_decoded_record_yields_empty() {
        // decompress() needs three bytes of length prefix before there
        // is anything to return.
        assert_eq!(decompress(b"").unwrap(), Vec::<u8>::new());
    }

    // -- Cross-validation vectors -------------------------------------
    //
    // Each stream below was produced by a reference BZZ encoder
    // transliterated from DjVuLibre's `BSEncodeByteStream.cpp` +
    // `ZPCodec.cpp`, and every one was confirmed to round-trip through
    // calibre's own `djvubzzdec.BZZDecoder` before being recorded here.
    // So these assert agreement with the Python implementation this
    // module is a port of, not merely self-consistency.

    /// Decode a compact hex fixture.
    fn hex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0, "hex fixture must have an even length");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex digit"))
            .collect()
    }

    fn decode_hex(stream: &str) -> Vec<u8> {
        BzzDecoder::new(&hex(stream))
            .expect("stream primes")
            .decode_all()
            .expect("stream decodes")
    }

    /// 11 bytes of plaintext, 16 bytes of BZZ.
    const HELLO_BZZ: &str = concat!("fffff3ff37cea38325b12e1e96016b6f",);

    /// 35 bytes of plaintext, 17 bytes of BZZ.
    const ABRACADABRA_BZZ: &str = concat!("ffffdbff3c087cb7e8394f504bd31ac787",);

    /// 256 bytes of plaintext, 275 bytes of BZZ.
    const ALL_BYTE_VALUES_BZZ: &str = concat!(
        "fffefeff806bc9bdfec0798ef834a6227ad2689564f986630d950d5f67fd2f84bf5ec06b",
        "f234f27bc98a61be772c44d6283dddaf7d99720ae8974480459026660ddeb8d8f53c7d68",
        "16fe9586d96baaf1e67f4afd2387442c2d5911bdbfd42c753229598af4972f26d4c996f0",
        "3e4777d35a32cafeb6ec1e3e9e12ea2bfbe548d3e9b981713bdb69c5c0aebb5f395e98de",
        "5dc0ce6b5b062b8c6e2393b4d840c33a32730df4e4c118a21067eee618fe2b4457e07d8e",
        "0f2f8ab475bf9625d4e902958227aac45f4950e7f024e19d10d9dac59fdeec5d4e59ff01",
        "b7a1af07cacb0e47b6c8b20206daa04d92d4215259e3ad8073affb5a0925d840d81df4c0",
        "15634eeb6191e104e8dd695e94bea24c5cee0798e1d20e",
    );

    /// 900 bytes of plaintext, 74 bytes of BZZ.
    const PARAGRAPH_BZZ: &str = concat!(
        "fffc7afacfb534d96401267c73f073cd94a2f55a130924d2a911ddc4e02ce4dbde5f05e1",
        "a46179ca8e84129cd7a686c8537a78addcfdee00c4b891098fb85a5bbb6e9e40ec8d27c7",
        "697f",
    );

    /// 34000 bytes of plaintext, 152 bytes of BZZ.
    const MULTI_BLOCK_BZZ: &str = concat!(
        "ffd7fffefddc267060bc3e05f7d6f138c297319624a0ce1107280425a8dbcf68533c570f",
        "a7aa4c1de834d4ccf07334958dfae6a87d3b6c9f2bf55792d16d5985dba73081df0ebfce",
        "0445099724180747af7ddc9c4e6e9acd42547b1c5411d5114b483f570a4ffbf0f69ea08f",
        "cec7bd3fe48b3855b62bcbf477977e5dfc2a5bbd672a1547c153fc8e603956470e855a1a",
        "8a72d27911b9cb83",
    );

    /// TXTz payload for 'Call me Ishmael. Some ye'...
    const TXTZ_PAGE1_BZZ: &str = concat!(
        "ffffbdfe96415472b75c8183c283999ff9d86c923543bffea4ea80dd15394c821f84aadd",
        "cc36a632e73845d253fb5a92c098be7b9953c29c17630afb60913f",
    );

    #[test]
    fn decodes_a_short_ascii_block() {
        assert_eq!(decode_hex(HELLO_BZZ), b"hello world");
    }

    #[test]
    fn decodes_a_block_with_repeated_runs() {
        // Repetition is what the BWT is for: this one compresses 35
        // bytes to 17 and leans on the move-to-front frequency rotate.
        assert_eq!(
            decode_hex(ABRACADABRA_BZZ),
            b"abracadabra abracadabra abracadabra"
        );
    }

    #[test]
    fn decodes_every_byte_value() {
        // 0x00..=0xff in order drives the MTF distance up to 255, which
        // exercises the deepest branch of the symbol decoder.
        let expected: Vec<u8> = (0..=255u8).collect();
        assert_eq!(decode_hex(ALL_BYTE_VALUES_BZZ), expected);
    }

    #[test]
    fn decodes_a_paragraph() {
        let expected = "The quick brown fox jumps over the lazy dog.\n".repeat(20);
        assert_eq!(decode_hex(PARAGRAPH_BZZ), expected.as_bytes());
    }

    #[test]
    fn decodes_a_stream_split_across_several_blocks() {
        // Encoded with a 10 KiB block size, so the decoder has to run
        // its block loop rather than stopping after the first one.
        let expected = "Call me Ishmael. ".repeat(2000);
        let raw = hex(MULTI_BLOCK_BZZ);
        let mut dec = BzzDecoder::new(&raw).unwrap();
        let mut blocks = 0;
        let mut out = Vec::new();
        while let Some(block) = dec.next_block().unwrap() {
            blocks += 1;
            out.extend_from_slice(block);
        }
        assert!(blocks > 1, "expected a multi-block stream, got {blocks}");
        assert_eq!(out, expected.as_bytes());
    }

    #[test]
    fn damaged_streams_error_rather_than_panic() {
        // Fault tolerance: a DjVu file off a flaky external disk can
        // hand us a half-written TXTz chunk. Every path out of the
        // decoder must be an error, never an index or arithmetic panic.
        // (Run with overflow checks on, i.e. a debug build.)
        let seeds: Vec<Vec<u8>> = [HELLO_BZZ, ABRACADABRA_BZZ, ALL_BYTE_VALUES_BZZ]
            .iter()
            .map(|s| hex(s))
            .collect();
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let (mut decoded, mut rejected) = (0, 0);
        for _ in 0..1000 {
            let mut raw = seeds[next() as usize % seeds.len()].clone();
            for _ in 0..1 + next() % 4 {
                let pos = next() as usize % raw.len();
                match next() % 3 {
                    0 => raw[pos] ^= 1 << (next() % 8),
                    1 => raw[pos] = (next() & 0xff) as u8,
                    _ => raw.truncate(pos.max(1)),
                }
            }
            match BzzDecoder::new(&raw).and_then(|mut d| d.decode_all()) {
                Ok(_) => decoded += 1,
                Err(_) => rejected += 1,
            }
        }
        // Both outcomes are legitimate; the point is that neither panics.
        assert_eq!(decoded + rejected, 1000);
        assert!(rejected > 0, "expected corruption to be caught");
    }

    #[test]
    fn decompress_strips_the_text_record_prefix() {
        // TXTz payloads carry a three-byte big-endian length ahead of
        // the text; decompress() is the C extension's behaviour, which
        // removes it.
        let text = b"Call me Ishmael. Some years ago-never mind how long precisely-";
        assert_eq!(decompress(&hex(TXTZ_PAGE1_BZZ)).unwrap(), text);
    }
}
