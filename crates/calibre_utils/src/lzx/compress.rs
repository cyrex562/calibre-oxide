//! LZX compression.
//!
//! Port of `src/calibre/utils/lzx/lzxc.c` (lzxcomp, Copyright (C) 2002
//! Matthew T. Russotto) plus the buffering layer in `compressor.c` that
//! calibre exposes as `calibre.ebooks.lit.lzx.Compressor`.
//!
//! Two deliberate divergences from the C, both in paths that read out of
//! bounds there:
//!
//!  * `find_match_at` indexes `block_buf[block_loc - loc]` without
//!    checking that `loc` fits in the history accumulated so far. Here
//!    such a repeated-offset candidate is simply rejected, which costs a
//!    little ratio and never changes what the stream decodes to.
//!  * `compressor.c`'s `get_bytes` copies `nbytes` out of the residue
//!    buffer when only `resrem` are present. Only a non-flushing
//!    `compress()` can leave a residue, which `writer.py` never does;
//!    this port copies what is actually there.

use super::huffman::{build_huffman_tree, HuffEntry};
use super::lz::{LzInfo, LzSink, LzState};
use super::{LzxError, LZX_FRAME_SIZE};

const MIN_MATCH: i32 = 2;
const MAX_MATCH: usize = 257;
const NUM_CHARS: usize = 256;
const NUM_PRIMARY_LENGTHS: i32 = 7;
const NUM_SECONDARY_LENGTHS: usize = 249;

const LZX_MAX_CODE_LENGTH: i32 = 16;
const LZX_PRETREE_SIZE: usize = 20;
const LZX_ALIGNED_SIZE: usize = 8;

const LZX_VERBATIM_BLOCK: u32 = 1;
const LZX_ALIGNED_OFFSET_BLOCK: u32 = 2;

/// `num_position_slots` in `lzxc.c`, indexed by `window_bits - 15`.
const NUM_POSITION_SLOTS: [usize; 7] = [30, 32, 34, 36, 38, 42, 50];

/// `position_base` / `extra_bits` from `lzx_init_static`.
struct Slots {
    base: [u32; 51],
    extra: [u8; 52],
}

impl Slots {
    fn new() -> Self {
        let mut extra = [0u8; 52];
        let mut j = 0u8;
        let mut i = 0;
        while i <= 50 {
            extra[i] = j;
            extra[i + 1] = j;
            if i != 0 && j < 17 {
                j += 1;
            }
            i += 2;
        }
        let mut base = [0u32; 51];
        let mut acc = 0u32;
        for i in 0..51 {
            base[i] = acc;
            acc += 1 << extra[i];
        }
        Slots { base, extra }
    }
}

/// One frame boundary recorded by `mark_frame`: uncompressed bytes
/// consumed and compressed bytes emitted at that point.
pub type ResetEntry = (u32, u32);

/// The encoder proper: `lzxc_data` in the C, minus the callback
/// plumbing.
struct Encoder {
    slots: Slots,
    /// Input the caller handed to `compress`, and how much is consumed.
    input: Vec<u8>,
    input_offset: usize,
    /// Bytes left over from a previous non-flushing `compress`.
    residue: Vec<u8>,
    residue_offset: usize,
    flushing: bool,

    output: Vec<u8>,
    rtable: Vec<ResetEntry>,

    left_in_frame: i32,
    left_in_block: i32,
    r0: i32,
    r1: i32,
    r2: i32,
    num_position_slots: usize,

    main_freq_table: Vec<i32>,
    length_freq_table: [i32; NUM_SECONDARY_LENGTHS],
    aligned_freq_table: [i32; LZX_ALIGNED_SIZE],
    block_codes: Vec<u32>,

    main_tree: Vec<HuffEntry>,
    length_tree: Vec<HuffEntry>,
    aligned_tree: Vec<HuffEntry>,
    main_tree_size: usize,

    bit_buf: u16,
    bits_in_buf: i32,

    main_entropy: f64,
    last_ratio: f64,

    prev_main_treelengths: Vec<u8>,
    prev_length_treelengths: Vec<u8>,

    len_uncompressed_input: u32,
    len_compressed_output: u32,

    need_1bit_header: bool,
    /// 0 = don't subdivide, 1 = allowed, -1 = requested.
    subdivide: i32,
}

/// `rloge2` in `lzxc.c`: `1.0 / log(2)`.
const RLOGE2: f64 = std::f64::consts::LOG2_E;

impl Encoder {
    fn new(window_bits: u32) -> Self {
        let num_position_slots = NUM_POSITION_SLOTS[(window_bits - 15) as usize];
        let main_tree_size = NUM_CHARS + 8 * num_position_slots;
        Encoder {
            slots: Slots::new(),
            input: Vec::new(),
            input_offset: 0,
            residue: Vec::new(),
            residue_offset: 0,
            flushing: false,
            output: Vec::new(),
            rtable: Vec::new(),
            left_in_frame: 0,
            left_in_block: 0,
            r0: 1,
            r1: 1,
            r2: 1,
            num_position_slots,
            main_freq_table: vec![0i32; main_tree_size],
            length_freq_table: [0i32; NUM_SECONDARY_LENGTHS],
            aligned_freq_table: [0i32; LZX_ALIGNED_SIZE],
            block_codes: Vec::new(),
            main_tree: vec![HuffEntry::default(); main_tree_size],
            length_tree: vec![HuffEntry::default(); NUM_SECONDARY_LENGTHS],
            aligned_tree: vec![HuffEntry::default(); LZX_ALIGNED_SIZE],
            main_tree_size,
            bit_buf: 0,
            bits_in_buf: 0,
            main_entropy: 0.0,
            last_ratio: 0.0,
            prev_main_treelengths: vec![0u8; main_tree_size],
            prev_length_treelengths: vec![0u8; NUM_SECONDARY_LENGTHS],
            len_uncompressed_input: 0,
            len_compressed_output: 0,
            need_1bit_header: true,
            subdivide: 0,
        }
    }

    /// How much input is still unread. `COMPRESSOR_REMAINING`.
    fn remaining(&self) -> usize {
        (self.residue.len() - self.residue_offset) + (self.input.len() - self.input_offset)
    }

    /// `at_eof` in `compressor.c`.
    fn at_eof(&self) -> bool {
        self.flushing && self.remaining() == 0
    }

    /// `get_bytes` in `compressor.c` — residue first, then the caller's
    /// buffer.
    fn get_bytes(&mut self, buf: &mut [u8]) -> usize {
        let mut written = 0usize;
        let resrem = self.residue.len() - self.residue_offset;
        if resrem > 0 {
            let take = resrem.min(buf.len());
            buf[..take]
                .copy_from_slice(&self.residue[self.residue_offset..self.residue_offset + take]);
            self.residue_offset += take;
            written += take;
            if written == buf.len() {
                return written;
            }
        }
        let inrem = self.input.len() - self.input_offset;
        if inrem == 0 {
            return written;
        }
        let take = inrem.min(buf.len() - written);
        buf[written..written + take]
            .copy_from_slice(&self.input[self.input_offset..self.input_offset + take]);
        self.input_offset += take;
        written + take
    }

    /// `lzx_write_bits`.
    fn write_bits(&mut self, mut nbits: i32, bits: u32) {
        let mut cur_bits = self.bits_in_buf;
        while cur_bits + nbits >= 16 {
            let shift_bits = 16 - cur_bits;
            let rshift_bits = nbits - shift_bits;
            if shift_bits == 16 {
                self.bit_buf = ((bits >> rshift_bits) & 0xFFFF) as u16;
            } else {
                let mask_bits = (1u32 << shift_bits) - 1;
                self.bit_buf <<= shift_bits;
                self.bit_buf |= ((bits >> rshift_bits) & mask_bits) as u16;
            }
            self.output.extend_from_slice(&self.bit_buf.to_le_bytes());
            self.len_compressed_output += 2;
            self.bit_buf = 0;
            nbits -= shift_bits;
            cur_bits = 0;
        }
        let mask_bits = if nbits >= 16 {
            u32::MAX
        } else {
            (1u32 << nbits) - 1
        };
        self.bit_buf <<= nbits;
        self.bit_buf |= (bits & mask_bits) as u16;
        self.bits_in_buf = cur_bits + nbits;
    }

    /// `lzx_align_output` — pad to the next 16-bit boundary and record a
    /// reset-table entry.
    fn align_output(&mut self) {
        if self.bits_in_buf != 0 {
            self.write_bits(16 - self.bits_in_buf, 0);
        }
        self.rtable
            .push((self.len_uncompressed_input, self.len_compressed_output));
    }

    /// `check_entropy` — estimate the compression ratio so far and ask
    /// for a block subdivision once it starts getting worse.
    fn check_entropy(&mut self, main_index: usize, lz: &mut LzState) {
        if self.main_freq_table[main_index] != 1 {
            let freq = f64::from(self.main_freq_table[main_index] - 1);
            self.main_entropy += freq * freq.ln();
        }
        let freq = f64::from(self.main_freq_table[main_index]);
        self.main_entropy -= freq * freq.ln();
        let n = self.block_codes.len();

        if (n & 0xFFF) == 0 && self.left_in_block >= 0x1000 {
            let nf = n as f64;
            let n_ln_n = nf * nf.ln();
            let rn_ln2 = RLOGE2 / nf;
            let cur_ratio = (nf * rn_ln2 * (n_ln_n + self.main_entropy)
                + 24.0
                + 3.0 * 80.0
                + NUM_CHARS as f64
                + (self.main_tree_size - NUM_CHARS) as f64 * 3.0
                + NUM_SECONDARY_LENGTHS as f64)
                / nf;
            if cur_ratio > self.last_ratio {
                self.subdivide = -1;
                lz.stop_compressing();
            }
            self.last_ratio = cur_ratio;
        }
    }

    /// `find_match_at` — can this match be re-expressed as one of the
    /// three repeated offsets?
    fn find_match_at(&self, lz: &LzState, loc: i32, match_len: i32, match_loc: &mut i32) -> bool {
        if -*match_loc == loc {
            return false;
        }
        if loc < match_len {
            return false;
        }
        // Not in the C: reject offsets that reach back before the start
        // of the buffer rather than reading out of bounds.
        if loc > lz.block_loc as i32 {
            return false;
        }
        let a = (lz.block_loc as i32 + *match_loc) as usize;
        let b = (lz.block_loc as i32 - loc) as usize;
        let len = match_len as usize;
        if a + len > lz.block_buf.len() || b + len > lz.block_buf.len() {
            return false;
        }
        if lz.block_buf[a..a + len] == lz.block_buf[b..b + len] {
            *match_loc = -loc;
            return true;
        }
        false
    }

    /// `lzx_write_compressed_literals` — emit the block's codes using
    /// the trees just built.
    fn write_compressed_literals(&mut self, block_type: u32) {
        let mut frame_count = (self.len_uncompressed_input % LZX_FRAME_SIZE as u32) as i64;
        // Added back in as the frames are emitted.
        self.len_uncompressed_input -= frame_count as u32;

        let codes = std::mem::take(&mut self.block_codes);
        for &block_code in codes.iter() {
            if block_code & 0x8000_0000 != 0 {
                let match_len_m2 = (block_code & 0xFF) as i32;
                let position_footer = (block_code >> 8) & 0x1_FFFF;
                let position_slot = ((block_code >> 25) & 0x3F) as usize;

                let (length_header, length_footer) = if match_len_m2 < NUM_PRIMARY_LENGTHS {
                    (match_len_m2, None)
                } else {
                    (
                        NUM_PRIMARY_LENGTHS,
                        Some((match_len_m2 - NUM_PRIMARY_LENGTHS) as usize),
                    )
                };
                let len_pos_header = ((position_slot as i32) << 3) | length_header;
                let e = self.main_tree[len_pos_header as usize + NUM_CHARS];
                self.write_bits(i32::from(e.codelength), u32::from(e.code));
                if let Some(footer) = length_footer {
                    let e = self.length_tree[footer];
                    self.write_bits(i32::from(e.codelength), u32::from(e.code));
                }
                let extra = i32::from(self.slots.extra[position_slot]);
                if block_type == LZX_ALIGNED_OFFSET_BLOCK && extra >= 3 {
                    self.write_bits(extra - 3, position_footer >> 3);
                    let e = self.aligned_tree[(position_footer & 7) as usize];
                    self.write_bits(i32::from(e.codelength), u32::from(e.code));
                } else {
                    self.write_bits(extra, position_footer);
                }
                frame_count += i64::from(match_len_m2) + 2;
            } else {
                let e = self.main_tree[block_code as usize];
                self.write_bits(i32::from(e.codelength), u32::from(e.code));
                frame_count += 1;
            }
            if frame_count == LZX_FRAME_SIZE as i64 {
                self.len_uncompressed_input += frame_count as u32;
                self.align_output();
                frame_count = 0;
            }
        }
        self.len_uncompressed_input += frame_count as u32;
        self.block_codes = codes;
        self.block_codes.clear();
    }

    /// `lzx_write_compressed_tree` — RLE-compress a tree's code lengths
    /// against the previous block's, then emit pretree and payload.
    fn write_compressed_tree(&mut self, tree: &[HuffEntry], prevlengths: &[u8]) {
        let treesize = tree.len();
        let mut codes: Vec<u8> = Vec::with_capacity(treesize);
        let mut runs: Vec<u8> = Vec::with_capacity(treesize);
        let mut freqs = [0i32; LZX_PRETREE_SIZE];

        let mut cur_run: i32 = 1;
        let mut last_len = tree[0].codelength;
        for i in 1..=treesize {
            if i == treesize || tree[i].codelength != last_len {
                if last_len == 0 {
                    while cur_run >= 20 {
                        let excess = (cur_run - 20).min(31);
                        codes.push(18);
                        runs.push(excess as u8);
                        cur_run -= excess + 20;
                        freqs[18] += 1;
                    }
                    while cur_run >= 4 {
                        let excess = (cur_run - 4).min(15);
                        codes.push(17);
                        runs.push(excess as u8);
                        cur_run -= excess + 4;
                        freqs[17] += 1;
                    }
                    while cur_run > 0 {
                        let c = prevlengths[i - cur_run as usize];
                        codes.push(c);
                        freqs[c as usize] += 1;
                        runs.push(0);
                        cur_run -= 1;
                    }
                } else {
                    while cur_run >= 4 {
                        let excess = if cur_run == 4 { 0 } else { 1 };
                        codes.push(19);
                        runs.push(excess as u8);
                        freqs[19] += 1;
                        // MS lies again: the code is prev_len - len
                        // (mod 17), not prev_len + len.
                        let mut c = prevlengths[i - cur_run as usize].wrapping_sub(last_len as u8);
                        if c > 16 {
                            c = c.wrapping_add(17);
                        }
                        codes.push(c);
                        freqs[c as usize] += 1;
                        runs.push(0);
                        cur_run -= excess + 4;
                    }
                    while cur_run > 0 {
                        let mut c = prevlengths[i - cur_run as usize].wrapping_sub(last_len as u8);
                        if c > 16 {
                            c = c.wrapping_add(17);
                        }
                        runs.push(0);
                        cur_run -= 1;
                        codes.push(c);
                        freqs[c as usize] += 1;
                    }
                }
                if i != treesize {
                    last_len = tree[i].codelength;
                }
                cur_run = 0;
            }
            cur_run += 1;
        }

        let pretree = build_huffman_tree(LZX_PRETREE_SIZE, 16, &freqs);
        for entry in pretree.iter().take(LZX_PRETREE_SIZE) {
            self.write_bits(4, u32::from(entry.codelength));
        }

        let mut ci = 0usize;
        let mut ri = 0usize;
        while ci < codes.len() {
            let cur_code = codes[ci] as usize;
            ci += 1;
            let e = pretree[cur_code];
            self.write_bits(i32::from(e.codelength), u32::from(e.code));
            match cur_code {
                17 => self.write_bits(4, u32::from(runs[ri])),
                18 => self.write_bits(5, u32::from(runs[ri])),
                19 => {
                    self.write_bits(1, u32::from(runs[ri]));
                    let next = codes[ci] as usize;
                    ci += 1;
                    let e = pretree[next];
                    self.write_bits(i32::from(e.codelength), u32::from(e.code));
                    ri += 1;
                }
                _ => {}
            }
            ri += 1;
        }
    }

    /// `lzxc_reset`.
    fn reset(&mut self) {
        self.need_1bit_header = true;
        self.r0 = 1;
        self.r1 = 1;
        self.r2 = 1;
        self.prev_main_treelengths.iter_mut().for_each(|l| *l = 0);
        self.prev_length_treelengths.iter_mut().for_each(|l| *l = 0);
    }

    fn clear_freq_tables(&mut self) {
        self.length_freq_table.iter_mut().for_each(|f| *f = 0);
        self.main_freq_table.iter_mut().for_each(|f| *f = 0);
        self.aligned_freq_table.iter_mut().for_each(|f| *f = 0);
    }
}

impl LzSink for Encoder {
    /// `lzx_get_chars` — read input, zero-padding a short read up to the
    /// end of the current frame.
    fn get_chars(&mut self, buf: &mut [u8]) -> usize {
        let n = buf.len();
        let mut chars_read = self.get_bytes(buf);
        self.left_in_frame -= (chars_read % LZX_FRAME_SIZE) as i32;
        if self.left_in_frame < 0 {
            self.left_in_frame += LZX_FRAME_SIZE as i32;
        }
        if chars_read < n && self.left_in_frame != 0 {
            let mut chars_pad = (n - chars_read) as i32;
            if chars_pad > self.left_in_frame {
                chars_pad = self.left_in_frame;
            }
            // Never emit a full frame of padding: that would be silly
            // when compress() is called at EOF but EOF isn't detected.
            if chars_pad == LZX_FRAME_SIZE as i32 {
                chars_pad = 0;
            }
            let pad = chars_pad as usize;
            buf[chars_read..chars_read + pad].fill(0);
            self.left_in_frame -= chars_pad;
            chars_read += pad;
        }
        chars_read
    }

    /// `lzx_output_match`.
    fn output_match(&mut self, match_pos: i32, match_len: i32, lz: &mut LzState) -> bool {
        let mut match_pos = match_pos;
        let mut position_footer: u32 = 0;
        let mut btdt = false;
        let position_slot: usize;

        loop {
            if match_pos == -self.r0 {
                position_slot = 0;
                break;
            } else if match_pos == -self.r1 {
                self.r1 = self.r0;
                self.r0 = -match_pos;
                position_slot = 1;
                break;
            } else if match_pos == -self.r2 {
                self.r2 = self.r0;
                self.r0 = -match_pos;
                position_slot = 2;
                break;
            }

            if !btdt {
                btdt = true;
                if self.find_match_at(lz, self.r0, match_len, &mut match_pos)
                    || self.find_match_at(lz, self.r1, match_len, &mut match_pos)
                    || self.find_match_at(lz, self.r2, match_len, &mut match_pos)
                {
                    continue;
                }
            }

            let offset = (-match_pos + 2) as u32;

            // Reject matches whose extra bits would likely cost more
            // than emitting literals. The thresholds come from the C.
            if match_len < 3
                || (offset >= 64 && match_len < 4)
                || (offset >= 2048 && match_len < 5)
                || (offset >= 65536 && match_len < 6)
            {
                return false;
            }

            self.r2 = self.r1;
            self.r1 = self.r0;
            self.r0 = -match_pos;

            let slot = if offset >= 262_144 {
                (offset >> 17) as usize + 34
            } else {
                // Binary search of the position-base table.
                let mut left = 3usize;
                let mut right = self.num_position_slots - 1;
                let mut found = None;
                while left <= right {
                    let mid = (left + right) / 2;
                    if self.slots.base[mid] <= offset && self.slots.base[mid + 1] > offset {
                        found = Some(mid);
                        break;
                    }
                    if offset > self.slots.base[mid] {
                        left = mid + 1;
                    } else {
                        right = mid;
                    }
                }
                match found {
                    Some(m) => m,
                    // Unreachable for well-formed offsets; refuse the
                    // match rather than emitting a bogus slot.
                    None => return false,
                }
            };
            position_footer = ((1u32 << self.slots.extra[slot]) - 1) & offset;
            position_slot = slot;
            break;
        }

        // match length = 8 bits, position_slot = 6 bits,
        // position_footer = 17 bits, plus the literal/match flag.
        self.block_codes.push(
            0x8000_0000
                | ((position_slot as u32) << 25)
                | (position_footer << 8)
                | ((match_len - MIN_MATCH) as u32),
        );

        let length_header = if match_len < NUM_PRIMARY_LENGTHS + MIN_MATCH {
            match_len - MIN_MATCH
        } else {
            let length_footer = match_len - (NUM_PRIMARY_LENGTHS + MIN_MATCH);
            self.length_freq_table[length_footer as usize] += 1;
            NUM_PRIMARY_LENGTHS
        };
        let len_pos_header = ((position_slot as i32) << 3) | length_header;
        self.main_freq_table[len_pos_header as usize + NUM_CHARS] += 1;
        if self.slots.extra[position_slot] >= 3 {
            self.aligned_freq_table[(position_footer & 7) as usize] += 1;
        }
        self.left_in_block -= match_len;
        if self.subdivide != 0 {
            self.check_entropy(len_pos_header as usize + NUM_CHARS, lz);
        }
        true
    }

    /// `lzx_output_literal`.
    fn output_literal(&mut self, ch: u8, lz: &mut LzState) {
        self.left_in_block -= 1;
        self.block_codes.push(u32::from(ch));
        self.main_freq_table[ch as usize] += 1;
        if self.subdivide != 0 {
            self.check_entropy(ch as usize, lz);
        }
    }
}

/// An LZX compressor. Port of `calibre.ebooks.lit.lzx.Compressor` /
/// `lzx.Compressor` in `compressor.c`.
pub struct Compressor {
    lzi: LzInfo,
    enc: Encoder,
    /// Whether the compressor resets its state after every block.
    /// `Compressor(wbits)` in Python defaults this to true.
    pub reset_each_block: bool,
    /// Window size in bits.
    pub wbits: u32,
    /// `1 << wbits`.
    pub blocksize: usize,
}

impl Compressor {
    /// `Compressor.__init__(wbits, reset=True)`.
    pub fn new(wbits: u32) -> Result<Self, LzxError> {
        Self::with_reset(wbits, true)
    }

    /// `Compressor.__init__` with an explicit `reset` flag.
    pub fn with_reset(wbits: u32, reset_each_block: bool) -> Result<Self, LzxError> {
        if !(15..=21).contains(&wbits) {
            return Err(LzxError::Args("window size must be 15..=21 bits"));
        }
        let wsize = 1usize << wbits;
        let mut c = Compressor {
            // The -3 prevents matches at wsize, wsize-1 and wsize-2,
            // all of which are illegal.
            lzi: LzInfo::new(
                wsize,
                wsize - 3,
                MAX_MATCH,
                MIN_MATCH as usize,
                LZX_FRAME_SIZE,
            ),
            enc: Encoder::new(wbits),
            reset_each_block,
            wbits,
            blocksize: wsize,
        };
        c.enc.reset();
        Ok(c)
    }

    /// `lzxc_compress_block` — LZ-analyse and emit up to `block_size`
    /// bytes, subdividing into multiple LZX blocks when the entropy
    /// estimate says the trees have gone stale.
    fn compress_block(&mut self, block_size: usize, subdivide: bool) {
        let Compressor { lzi, enc, .. } = self;
        enc.block_codes.clear();
        enc.block_codes.reserve(block_size);
        enc.subdivide = i32::from(subdivide);
        enc.left_in_block = block_size as i32;
        enc.left_in_frame = LZX_FRAME_SIZE as i32;
        enc.main_entropy = 0.0;
        enc.last_ratio = 9_999_999.0;
        enc.clear_freq_tables();

        let mut written_sofar: i32 = 0;
        loop {
            let left = enc.left_in_block.max(0) as usize;
            lzi.compress(enc, left);
            if enc.left_in_frame == 0 {
                enc.left_in_frame = LZX_FRAME_SIZE as i32;
            }

            if enc.subdivide < 0
                || enc.left_in_block == 0
                || (lzi.left_to_process() == 0 && enc.at_eof())
            {
                let uncomp_length = block_size as i32 - enc.left_in_block - written_sofar;
                // Zero happens when the input length is an exact
                // multiple of the frame size.
                if uncomp_length != 0 {
                    if enc.subdivide < 0 {
                        enc.subdivide = 1;
                    }
                    if enc.need_1bit_header {
                        // One-bit Intel preprocessing header, always 0
                        // because this implementation doesn't do it.
                        enc.write_bits(1, 0);
                        enc.need_1bit_header = false;
                    }

                    // Decide between a verbatim and an aligned-offset
                    // block by costing the extra bits both ways.
                    let mut uncomp_bits: i64 = 0;
                    let mut comp_bits: i64 = 0;
                    enc.aligned_tree =
                        build_huffman_tree(LZX_ALIGNED_SIZE, 7, &enc.aligned_freq_table);
                    for i in 0..LZX_ALIGNED_SIZE {
                        uncomp_bits += i64::from(enc.aligned_freq_table[i]) * 3;
                        comp_bits += i64::from(enc.aligned_freq_table[i])
                            * i64::from(enc.aligned_tree[i].codelength);
                    }
                    let comp_bits_ovh = comp_bits + LZX_ALIGNED_SIZE as i64 * 3;
                    let block_type = if comp_bits_ovh < uncomp_bits {
                        LZX_ALIGNED_OFFSET_BLOCK
                    } else {
                        LZX_VERBATIM_BLOCK
                    };

                    enc.write_bits(3, block_type);
                    enc.write_bits(24, uncomp_length as u32);

                    written_sofar = block_size as i32 - enc.left_in_block;

                    if block_type == LZX_ALIGNED_OFFSET_BLOCK {
                        for i in 0..LZX_ALIGNED_SIZE {
                            let len = u32::from(enc.aligned_tree[i].codelength);
                            enc.write_bits(3, len);
                        }
                    }

                    enc.main_tree = build_huffman_tree(
                        enc.main_tree_size,
                        LZX_MAX_CODE_LENGTH,
                        &enc.main_freq_table,
                    );
                    enc.length_tree =
                        build_huffman_tree(NUM_SECONDARY_LENGTHS, 16, &enc.length_freq_table);

                    let main_tree = std::mem::take(&mut enc.main_tree);
                    let length_tree = std::mem::take(&mut enc.length_tree);
                    let prev_main = std::mem::take(&mut enc.prev_main_treelengths);
                    let prev_length = std::mem::take(&mut enc.prev_length_treelengths);

                    enc.write_compressed_tree(&main_tree[..NUM_CHARS], &prev_main[..NUM_CHARS]);
                    enc.write_compressed_tree(&main_tree[NUM_CHARS..], &prev_main[NUM_CHARS..]);
                    enc.write_compressed_tree(&length_tree, &prev_length);

                    enc.main_tree = main_tree;
                    enc.length_tree = length_tree;
                    enc.prev_main_treelengths = prev_main;
                    enc.prev_length_treelengths = prev_length;

                    enc.write_compressed_literals(block_type);

                    // Copy the tree lengths somewhere safe for the next
                    // block's delta compression.
                    for i in 0..enc.main_tree_size {
                        enc.prev_main_treelengths[i] = enc.main_tree[i].codelength as u8;
                    }
                    for i in 0..NUM_SECONDARY_LENGTHS {
                        enc.prev_length_treelengths[i] = enc.length_tree[i].codelength as u8;
                    }
                    enc.main_entropy = 0.0;
                    enc.last_ratio = 9_999_999.0;
                    enc.block_codes.clear();
                    enc.clear_freq_tables();
                }
            }

            if enc.left_in_block == 0 || (lzi.left_to_process() == 0 && enc.at_eof()) {
                break;
            }
        }
    }

    /// `Compressor.compress(data, flush)`.
    ///
    /// Returns the compressed bytes and the reset table: one
    /// `(uncompressed, compressed)` pair per 32K frame boundary.
    pub fn compress(&mut self, data: &[u8], flush: bool) -> (Vec<u8>, Vec<ResetEntry>) {
        self.enc.flushing = flush;
        self.enc.input.clear();
        self.enc.input.extend_from_slice(data);
        self.enc.input_offset = 0;
        self.enc.output.clear();

        let blocksize = self.blocksize;
        while self.enc.remaining() >= blocksize {
            self.compress_block(blocksize, true);
            if self.reset_each_block {
                self.enc.reset();
                self.lzi.reset();
            }
        }
        if flush && self.enc.remaining() > 0 {
            self.compress_block(blocksize, true);
            if self.reset_each_block {
                self.enc.reset();
                self.lzi.reset();
            }
            self.enc.residue.clear();
            self.enc.residue_offset = 0;
        } else {
            let rest = self.enc.input[self.enc.input_offset..].to_vec();
            self.enc.residue = rest;
            self.enc.residue_offset = 0;
        }

        let rtable = std::mem::take(&mut self.enc.rtable);
        let out = std::mem::take(&mut self.enc.output);
        (out, rtable)
    }

    /// `Compressor.flush()`.
    pub fn flush(&mut self) -> (Vec<u8>, Vec<ResetEntry>) {
        self.compress(&[], true)
    }

    /// Total uncompressed bytes consumed so far.
    /// `lzxc_results.len_uncompressed_input`.
    pub fn len_uncompressed_input(&self) -> u32 {
        self.enc.len_uncompressed_input
    }

    /// Total compressed bytes emitted so far.
    /// `lzxc_results.len_compressed_output`.
    pub fn len_compressed_output(&self) -> u32 {
        self.enc.len_compressed_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lzx::decompress;

    /// Decode a whole compressed stream the way `LitFile.decompress`
    /// does: the compressor resets after every `blocksize` bytes, so
    /// each block is an independent LZX stream and the reset table says
    /// where each one starts.
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
            let chunk =
                decompress(&compressed[base..], wbits, remaining).expect("tail decompresses");
            out.extend_from_slice(&chunk);
        }
        out
    }

    fn roundtrip(data: &[u8], wbits: u32) {
        let mut c = Compressor::new(wbits).expect("valid window size");
        let (compressed, rtable) = c.compress(data, true);
        let out = decompress_blocks(&compressed, &rtable, wbits, data.len());
        assert_eq!(out.len(), data.len());
        assert_eq!(out, data, "{} bytes round-tripped wrong", data.len());
        // Short reads are zero-padded to the end of the frame, so
        // there is one reset-table entry per *started* frame.
        assert_eq!(rtable.len(), data.len().div_ceil(LZX_FRAME_SIZE));
    }

    #[test]
    fn rejects_window_sizes_outside_the_supported_range() {
        for bits in [0u32, 14, 22] {
            assert!(matches!(Compressor::new(bits), Err(LzxError::Args(_))));
        }
        for bits in 15u32..=21 {
            assert!(Compressor::new(bits).is_ok());
        }
    }

    #[test]
    fn round_trips_highly_compressible_data() {
        roundtrip(&vec![b'a'; 40_000], 17);
    }

    #[test]
    fn round_trips_incompressible_data() {
        let data: Vec<u8> = (0..50_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        roundtrip(&data, 17);
    }

    #[test]
    fn round_trips_realistic_markup() {
        let mut data = Vec::new();
        for i in 0..900 {
            data.extend_from_slice(
                format!("<p class=\"body\" id=\"p{i}\">Paragraph number {i} of the test.</p>\n")
                    .as_bytes(),
            );
        }
        roundtrip(&data, 17);
    }

    #[test]
    fn round_trips_across_every_window_size() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        for bits in 15u32..=21 {
            roundtrip(&data, bits);
        }
    }

    #[test]
    fn round_trips_data_spanning_several_frames() {
        let mut data = Vec::new();
        for i in 0..8u32 {
            data.extend(std::iter::repeat_n((i * 31) as u8, 32_768));
        }
        roundtrip(&data, 17);
    }

    #[test]
    fn empty_input_produces_no_output() {
        let mut c = Compressor::new(17).expect("valid window size");
        let (out, rtable) = c.compress(&[], true);
        assert!(out.is_empty());
        assert!(rtable.is_empty());
    }

    #[test]
    fn round_trips_short_input() {
        for n in [1usize, 2, 3, 7, 64, 255, 256, 1000] {
            let data: Vec<u8> = (0..n).map(|i| (i * 7 % 253) as u8).collect();
            roundtrip(&data, 17);
        }
    }
}
