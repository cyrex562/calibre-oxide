//! LZX decompression.
//!
//! Port of `src/calibre/utils/lzx/lzxd.c` from libmspack (C) 2003-2004
//! Stuart Caie, together with the driver in `lzxmodule.c` that calibre
//! wraps as `calibre.ebooks.lit.lzx.Decompressor`.
//!
//! `lzxmodule.c` builds a fresh `lzxd_stream` for every `decompress()`
//! call — window bits and output length in, one buffer out — so this
//! port exposes the same shape as a single function over slices rather
//! than reproducing libmspack's callback-driven `mspack_system`.

use super::{LzxError, LZX_FRAME_SIZE};

const LZX_MIN_MATCH: u32 = 2;
const LZX_NUM_CHARS: u16 = 256;
const LZX_BLOCKTYPE_INVALID: u32 = 0;
const LZX_BLOCKTYPE_VERBATIM: u32 = 1;
const LZX_BLOCKTYPE_ALIGNED: u32 = 2;
const LZX_BLOCKTYPE_UNCOMPRESSED: u32 = 3;
const LZX_NUM_PRIMARY_LENGTHS: u16 = 7;
const LZX_NUM_SECONDARY_LENGTHS: usize = 249;

const PRETREE_MAXSYMBOLS: usize = 20;
const PRETREE_TABLEBITS: u32 = 6;
const MAINTREE_MAXSYMBOLS: usize = 256 + 50 * 8;
const MAINTREE_TABLEBITS: u32 = 12;
const LENGTH_MAXSYMBOLS: usize = LZX_NUM_SECONDARY_LENGTHS + 1;
const LENGTH_TABLEBITS: u32 = 12;
const ALIGNED_MAXSYMBOLS: usize = 8;
const ALIGNED_TABLEBITS: u32 = 7;

/// `LZX_LENTABLE_SAFETY` — table decoding overruns are allowed.
const LENTABLE_SAFETY: usize = 64;

/// Width of the bit buffer. The C uses `unsigned int`, so 32.
const BITBUF_WIDTH: u32 = 32;

/// `position_base` and `extra_bits` from `lzxd_static_init`.
struct PositionSlots {
    base: [u32; 51],
    extra: [u8; 51],
}

impl PositionSlots {
    fn new() -> Self {
        let mut extra = [0u8; 51];
        let mut j = 0u8;
        let mut i = 0;
        while i < 50 {
            extra[i] = j;
            extra[i + 1] = j;
            if i != 0 && j < 17 {
                j += 1;
            }
            i += 2;
        }
        extra[50] = 17;

        let mut base = [0u32; 51];
        let mut acc = 0u32;
        for i in 0..51 {
            base[i] = acc;
            acc += 1 << extra[i];
        }
        PositionSlots { base, extra }
    }
}

/// A canonical-Huffman decode table built by `make_decode_table`.
struct HuffTable {
    /// Code length per symbol, with the overrun safety margin the C
    /// relies on when `lzxd_read_lens` writes past `last`.
    len: Vec<u8>,
    table: Vec<u16>,
    maxsymbols: usize,
    tablebits: u32,
}

impl HuffTable {
    fn new(maxsymbols: usize, tablebits: u32) -> Self {
        HuffTable {
            len: vec![0u8; maxsymbols + LENTABLE_SAFETY],
            table: vec![0u16; (1 << tablebits) + maxsymbols * 2],
            maxsymbols,
            tablebits,
        }
    }

    /// `make_decode_table` in `lzxd.c`, coded by David Tritscher.
    ///
    /// Builds a fast decoding table from canonical code lengths.
    /// Returns an error for an over- or under-subscribed tree, except
    /// for the all-zero tree, which many CAB files rely on.
    // The two table-filling loops walk symbols against a running
    // position the way the C does; rewriting them as iterator chains
    // would obscure the correspondence with `make_decode_table`.
    #[allow(clippy::needless_range_loop, clippy::explicit_counter_loop)]
    fn build(&mut self) -> Result<(), LzxError> {
        let nsyms = self.maxsymbols;
        let nbits = self.tablebits;
        let length = &self.len;
        let table = &mut self.table;

        let mut pos: u32 = 0;
        let table_mask: u32 = 1 << nbits;
        let mut bit_mask: u32 = table_mask >> 1;
        let mut next_symbol: u32 = bit_mask;

        // Fill entries for codes short enough for a direct mapping.
        for bit_num in 1..=nbits {
            for sym in 0..nsyms {
                if u32::from(length[sym]) != bit_num {
                    continue;
                }
                let mut leaf = pos as usize;
                pos += bit_mask;
                if pos > table_mask {
                    return Err(LzxError::Decrunch("huffman table overrun"));
                }
                for _ in 0..bit_mask {
                    table[leaf] = sym as u16;
                    leaf += 1;
                }
            }
            bit_mask >>= 1;
        }

        if pos == table_mask {
            return Ok(());
        }

        // Clear the remainder of the table.
        for entry in table
            .iter_mut()
            .take(table_mask as usize)
            .skip(pos as usize)
        {
            *entry = 0xFFFF;
        }

        // Allow codes to be up to nbits+16 long instead of nbits.
        let mut pos = pos << 16;
        let table_mask = table_mask << 16;
        let mut bit_mask: u32 = 1 << 15;

        for bit_num in (nbits + 1)..=16 {
            for sym in 0..nsyms {
                if u32::from(length[sym]) != bit_num {
                    continue;
                }
                let mut leaf = (pos >> 16) as usize;
                for fill in 0..(bit_num - nbits) {
                    // If this path hasn't been taken yet, 'allocate'
                    // two entries.
                    if table[leaf] == 0xFFFF {
                        let ns = next_symbol as usize;
                        if (ns << 1) + 1 >= table.len() {
                            return Err(LzxError::Decrunch("huffman table overflow"));
                        }
                        table[ns << 1] = 0xFFFF;
                        table[(ns << 1) + 1] = 0xFFFF;
                        table[leaf] = next_symbol as u16;
                        next_symbol += 1;
                    }
                    leaf = (table[leaf] as usize) << 1;
                    if (pos >> (15 - fill)) & 1 != 0 {
                        leaf += 1;
                    }
                }
                if leaf >= table.len() {
                    return Err(LzxError::Decrunch("huffman table overflow"));
                }
                table[leaf] = sym as u16;

                pos += bit_mask;
                if pos > table_mask {
                    return Err(LzxError::Decrunch("huffman table overflow"));
                }
            }
            bit_mask >>= 1;
        }

        if pos == table_mask {
            return Ok(());
        }

        // Either an erroneous table, or all elements are zero.
        if length.iter().take(nsyms).any(|&l| l != 0) {
            return Err(LzxError::Decrunch("incomplete huffman table"));
        }
        Ok(())
    }
}

/// The bitstream reader: `ENSURE_BITS` / `PEEK_BITS` / `REMOVE_BITS`.
struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit_buffer: u32,
    bits_left: i32,
    /// `lzx->input_end` — libmspack fakes two zero bytes once past the
    /// end of the input, because `ENSURE_BITS(16)` may overrun even when
    /// those bits are never used.
    input_end: bool,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader {
            data,
            pos: 0,
            bit_buffer: 0,
            bits_left: 0,
            input_end: false,
        }
    }

    /// Fetch the next little-endian 16-bit group, faking zeroes once at
    /// the end of input as `lzxd_read_input` does.
    fn next_pair(&mut self) -> Result<u32, LzxError> {
        if self.pos >= self.data.len() {
            if self.input_end {
                return Err(LzxError::Read);
            }
            self.input_end = true;
            self.pos += 2;
            return Ok(0);
        }
        let lo = u32::from(self.data[self.pos]);
        // A trailing odd byte reads as if the buffer were zero-padded;
        // the C reads one byte past `i_end` into its own input buffer
        // here, and those bits are never used.
        let hi = self.data.get(self.pos + 1).map_or(0u32, |b| u32::from(*b));
        self.pos += 2;
        Ok((hi << 8) | lo)
    }

    fn ensure(&mut self, nbits: i32) -> Result<(), LzxError> {
        while self.bits_left < nbits {
            let pair = self.next_pair()?;
            self.bit_buffer |= pair << (BITBUF_WIDTH as i32 - 16 - self.bits_left);
            self.bits_left += 16;
        }
        Ok(())
    }

    fn peek(&self, nbits: u32) -> u32 {
        if nbits == 0 {
            0
        } else {
            self.bit_buffer >> (BITBUF_WIDTH - nbits)
        }
    }

    fn remove(&mut self, nbits: i32) {
        self.bit_buffer = if nbits >= 32 {
            0
        } else {
            self.bit_buffer << nbits
        };
        self.bits_left -= nbits;
    }

    fn read_bits(&mut self, nbits: i32) -> Result<u32, LzxError> {
        if nbits == 0 {
            return Ok(0);
        }
        self.ensure(nbits)?;
        let val = self.peek(nbits as u32);
        self.remove(nbits);
        Ok(val)
    }

    /// `READ_HUFFSYM` — decode one symbol, table lookup first and tree
    /// traversal for anything longer than `tablebits`.
    fn read_huffsym(&mut self, tbl: &HuffTable) -> Result<u16, LzxError> {
        self.ensure(16)?;
        let mut sym = tbl.table[self.peek(tbl.tablebits) as usize];
        if sym as usize >= tbl.maxsymbols {
            let mut i: u32 = 1 << (BITBUF_WIDTH - tbl.tablebits);
            loop {
                i >>= 1;
                if i == 0 {
                    return Err(LzxError::Decrunch("out of bits in huffman decode"));
                }
                let mut idx = (sym as usize) << 1;
                if self.bit_buffer & i != 0 {
                    idx |= 1;
                }
                if idx >= tbl.table.len() {
                    return Err(LzxError::Decrunch("huffman tree index out of range"));
                }
                sym = tbl.table[idx];
                if (sym as usize) < tbl.maxsymbols {
                    break;
                }
            }
        }
        self.remove(i32::from(tbl.len[sym as usize]));
        Ok(sym)
    }

    /// Read one byte directly from the stream, for uncompressed blocks.
    fn read_byte(&mut self) -> Result<u8, LzxError> {
        if self.pos >= self.data.len() {
            return Err(LzxError::Read);
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn at_input_end(&self) -> bool {
        self.pos >= self.data.len()
    }
}

/// An LZX decompressor over one buffer, mirroring `lzxd_stream`.
struct Lzxd<'a> {
    bits: BitReader<'a>,
    slots: PositionSlots,
    window: Vec<u8>,
    window_size: u32,
    window_posn: u32,
    frame_posn: u32,
    frame: u32,
    posn_slots: u32,
    r0: u32,
    r1: u32,
    r2: u32,
    header_read: bool,
    block_type: u32,
    block_length: u32,
    block_remaining: u32,
    intel_filesize: i32,
    intel_curpos: i32,
    intel_started: bool,
    pretree: HuffTable,
    maintree: HuffTable,
    length: HuffTable,
    aligned: HuffTable,
    e8_buf: Vec<u8>,
    /// Total bytes emitted so far. `lzx->offset` in the C.
    offset: u64,
    /// Expected total output length. `lzx->length` in the C.
    out_length: u64,
    out: Vec<u8>,
}

impl<'a> Lzxd<'a> {
    fn new(data: &'a [u8], window_bits: u32, out_length: usize) -> Result<Self, LzxError> {
        if !(15..=21).contains(&window_bits) {
            return Err(LzxError::Args("window size must be 15..=21 bits"));
        }
        let window_size = 1u32 << window_bits;
        // window bits:    15  16  17  18  19  20  21
        // position slots: 30  32  34  36  38  42  50
        let posn_slots = match window_bits {
            21 => 50,
            20 => 42,
            n => n << 1,
        };
        let mut lzx = Lzxd {
            bits: BitReader::new(data),
            slots: PositionSlots::new(),
            window: vec![0u8; window_size as usize],
            window_size,
            window_posn: 0,
            frame_posn: 0,
            frame: 0,
            posn_slots,
            r0: 1,
            r1: 1,
            r2: 1,
            header_read: false,
            block_type: LZX_BLOCKTYPE_INVALID,
            block_length: 0,
            block_remaining: 0,
            intel_filesize: 0,
            intel_curpos: 0,
            intel_started: false,
            pretree: HuffTable::new(PRETREE_MAXSYMBOLS, PRETREE_TABLEBITS),
            maintree: HuffTable::new(MAINTREE_MAXSYMBOLS, MAINTREE_TABLEBITS),
            length: HuffTable::new(LENGTH_MAXSYMBOLS, LENGTH_TABLEBITS),
            aligned: HuffTable::new(ALIGNED_MAXSYMBOLS, ALIGNED_TABLEBITS),
            e8_buf: vec![0u8; LZX_FRAME_SIZE],
            offset: 0,
            out_length: out_length as u64,
            out: Vec::with_capacity(out_length),
        };
        lzx.reset_state();
        Ok(lzx)
    }

    /// `lzxd_reset_state`.
    fn reset_state(&mut self) {
        self.r0 = 1;
        self.r1 = 1;
        self.r2 = 1;
        self.header_read = false;
        self.block_remaining = 0;
        self.block_type = LZX_BLOCKTYPE_INVALID;
        // Initialise tables to 0, because deltas will be applied to them.
        self.maintree.len.iter_mut().for_each(|l| *l = 0);
        self.length.len.iter_mut().for_each(|l| *l = 0);
    }

    /// `lzxd_read_lens` — code lengths are stored in LZX's own
    /// pretree-plus-run-length encoding.
    fn read_lens(&mut self, which: Tree, first: usize, last: usize) -> Result<(), LzxError> {
        // Lengths for the pretree itself: 20 symbols, fixed 4 bits each.
        for x in 0..20 {
            let y = self.bits.read_bits(4)?;
            self.pretree.len[x] = y as u8;
        }
        self.pretree.build()?;

        let mut x = first;
        while x < last {
            let z = self.bits.read_huffsym(&self.pretree)? as i32;
            match z {
                17 => {
                    // Run of ([read 4 bits] + 4) zeros.
                    let mut y = self.bits.read_bits(4)? + 4;
                    while y > 0 {
                        self.tree_mut(which)[x] = 0;
                        x += 1;
                        y -= 1;
                    }
                }
                18 => {
                    // Run of ([read 5 bits] + 20) zeros.
                    let mut y = self.bits.read_bits(5)? + 20;
                    while y > 0 {
                        self.tree_mut(which)[x] = 0;
                        x += 1;
                        y -= 1;
                    }
                }
                19 => {
                    // Run of ([read 1 bit] + 4) [read huffman symbol].
                    let mut y = self.bits.read_bits(1)? + 4;
                    let z2 = self.bits.read_huffsym(&self.pretree)? as i32;
                    let mut v = i32::from(self.tree_mut(which)[x]) - z2;
                    if v < 0 {
                        v += 17;
                    }
                    while y > 0 {
                        self.tree_mut(which)[x] = v as u8;
                        x += 1;
                        y -= 1;
                    }
                }
                _ => {
                    // Code 0 to 16: delta the current length entry.
                    let lens = self.tree_mut(which);
                    let mut v = i32::from(lens[x]) - z;
                    if v < 0 {
                        v += 17;
                    }
                    lens[x] = v as u8;
                    x += 1;
                }
            }
        }
        Ok(())
    }

    fn tree_mut(&mut self, which: Tree) -> &mut Vec<u8> {
        match which {
            Tree::Maintree => &mut self.maintree.len,
            Tree::Length => &mut self.length.len,
        }
    }

    /// Copy a match out of the window, wrapping if the offset reaches
    /// back past the start.
    // The copies are deliberately byte-at-a-time and may overlap, so
    // the source and destination cursors advance together.
    #[allow(clippy::explicit_counter_loop)]
    fn copy_match(&mut self, match_offset: u32, match_length: u32) -> Result<(), LzxError> {
        if self.window_posn + match_length > self.window_size {
            return Err(LzxError::Decrunch("match ran over window wrap"));
        }
        let mut dest = self.window_posn as usize;
        let mut i = match_length as usize;
        if match_offset > self.window_posn {
            // The match reaches back around the start of the window.
            let j = (match_offset - self.window_posn) as usize;
            if j > self.window_size as usize {
                return Err(LzxError::Decrunch("match offset beyond window boundaries"));
            }
            let mut src = self.window_size as usize - j;
            if j < i {
                i -= j;
                for _ in 0..j {
                    self.window[dest] = self.window[src];
                    dest += 1;
                    src += 1;
                }
                src = 0;
            }
            for _ in 0..i {
                self.window[dest] = self.window[src];
                dest += 1;
                src += 1;
            }
        } else {
            let mut src = dest - match_offset as usize;
            for _ in 0..i {
                self.window[dest] = self.window[src];
                dest += 1;
                src += 1;
            }
        }
        self.window_posn += match_length;
        Ok(())
    }

    /// `lzxd_decompress` for the whole output at once.
    fn decompress(&mut self) -> Result<(), LzxError> {
        let mut out_bytes = self.out_length as i64;
        if out_bytes == 0 {
            return Ok(());
        }
        let end_frame = ((self.offset + out_bytes as u64) / LZX_FRAME_SIZE as u64) as u32 + 1;

        while self.frame < end_frame {
            // The LIT glue passes a reset interval of 0x7fff and resets
            // by hand, so in practice this fires only on frame 0.
            if self.frame.is_multiple_of(0x7fff) {
                if self.block_remaining != 0 {
                    return Err(LzxError::Decrunch("bytes remaining at reset interval"));
                }
                self.reset_state();
            }

            if !self.header_read {
                // 1 bit: whether an Intel filesize follows.
                let mut j = 0u32;
                let mut i = self.bits.read_bits(1)?;
                if i != 0 {
                    i = self.bits.read_bits(16)?;
                    j = self.bits.read_bits(16)?;
                }
                self.intel_filesize = ((i << 16) | j) as i32;
                self.header_read = true;
            }

            // All frames are 32k except the final one.
            let mut frame_size = LZX_FRAME_SIZE as u32;
            if self.out_length != 0 && (self.out_length - self.offset) < u64::from(frame_size) {
                frame_size = (self.out_length - self.offset) as u32;
            }

            let mut bytes_todo =
                self.frame_posn as i64 + i64::from(frame_size) - self.window_posn as i64;
            while bytes_todo > 0 {
                if self.block_remaining == 0 {
                    self.start_block()?;
                }

                let mut this_run = self.block_remaining as i64;
                if this_run > bytes_todo {
                    this_run = bytes_todo;
                }
                bytes_todo -= this_run;
                self.block_remaining -= this_run as u32;

                this_run = match self.block_type {
                    LZX_BLOCKTYPE_VERBATIM => self.run_verbatim(this_run)?,
                    LZX_BLOCKTYPE_ALIGNED => self.run_aligned(this_run)?,
                    LZX_BLOCKTYPE_UNCOMPRESSED => self.run_uncompressed(this_run)?,
                    _ => return Err(LzxError::Decrunch("bad block type")),
                };

                // Did the final match overrun the desired run length?
                if this_run < 0 {
                    if (-this_run) as u32 > self.block_remaining {
                        return Err(LzxError::Decrunch("overrun went past end of block"));
                    }
                    self.block_remaining -= (-this_run) as u32;
                }
            }

            // Re-align the input bitstream.
            if self.bits.bits_left > 0 {
                self.bits.ensure(16)?;
            }
            if self.bits.bits_left & 15 != 0 {
                let n = self.bits.bits_left & 15;
                self.bits.remove(n);
            }

            self.emit_frame(frame_size, &mut out_bytes);

            self.frame_posn += frame_size;
            self.frame += 1;

            if self.window_posn == self.window_size {
                self.window_posn = 0;
            }
            if self.frame_posn == self.window_size {
                self.frame_posn = 0;
            }
        }

        if out_bytes != 0 {
            return Err(LzxError::Decrunch("bytes left to output"));
        }
        Ok(())
    }

    /// Read a block header and, for compressed blocks, its trees.
    fn start_block(&mut self) -> Result<(), LzxError> {
        // Realign if the previous block was an odd-sized uncompressed one.
        if self.block_type == LZX_BLOCKTYPE_UNCOMPRESSED && (self.block_length & 1) != 0 {
            if self.bits.at_input_end() {
                return Err(LzxError::Read);
            }
            self.bits.pos += 1;
        }

        self.block_type = self.bits.read_bits(3)?;
        let i = self.bits.read_bits(16)?;
        let j = self.bits.read_bits(8)?;
        self.block_length = (i << 8) | j;
        self.block_remaining = self.block_length;

        match self.block_type {
            LZX_BLOCKTYPE_ALIGNED => {
                for i in 0..8 {
                    let j = self.bits.read_bits(3)?;
                    self.aligned.len[i] = j as u8;
                }
                self.aligned.build()?;
                // Falls through: the rest of the header is as verbatim.
                self.read_verbatim_trees()?;
            }
            LZX_BLOCKTYPE_VERBATIM => self.read_verbatim_trees()?,
            LZX_BLOCKTYPE_UNCOMPRESSED => {
                // We can't assume otherwise.
                self.intel_started = true;

                // Read 1-16 (not 0-15) bits to align to a byte boundary.
                self.bits.ensure(16)?;
                if self.bits.bits_left > 16 {
                    self.bits.pos -= 2;
                }
                self.bits.bits_left = 0;
                self.bits.bit_buffer = 0;

                // Read 12 bytes of stored R0 / R1 / R2 values.
                let mut buf = [0u8; 12];
                for slot in buf.iter_mut() {
                    *slot = self.bits.read_byte()?;
                }
                self.r0 = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                self.r1 = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
                self.r2 = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
            }
            _ => return Err(LzxError::Decrunch("bad block type")),
        }
        Ok(())
    }

    fn read_verbatim_trees(&mut self) -> Result<(), LzxError> {
        self.read_lens(Tree::Maintree, 0, 256)?;
        let last = 256 + (self.posn_slots as usize) * 8;
        self.read_lens(Tree::Maintree, 256, last)?;
        self.maintree.build()?;
        // If the literal 0xE8 is anywhere in the block...
        if self.maintree.len[0xE8] != 0 {
            self.intel_started = true;
        }
        self.read_lens(Tree::Length, 0, LZX_NUM_SECONDARY_LENGTHS)?;
        self.length.build()?;
        Ok(())
    }

    /// Decode the match length that follows a main-tree match element.
    fn match_length(&mut self, main_element: u16) -> Result<u32, LzxError> {
        let mut match_length = u32::from(main_element & LZX_NUM_PRIMARY_LENGTHS);
        if match_length == u32::from(LZX_NUM_PRIMARY_LENGTHS) {
            let length_footer = self.bits.read_huffsym(&self.length)?;
            match_length += u32::from(length_footer);
        }
        Ok(match_length + LZX_MIN_MATCH)
    }

    fn run_verbatim(&mut self, mut this_run: i64) -> Result<i64, LzxError> {
        while this_run > 0 {
            let sym = self.bits.read_huffsym(&self.maintree)?;
            if sym < LZX_NUM_CHARS {
                self.window[self.window_posn as usize] = sym as u8;
                self.window_posn += 1;
                this_run -= 1;
                continue;
            }
            let main_element = sym - LZX_NUM_CHARS;
            let match_length = self.match_length(main_element)?;

            let mut match_offset = u32::from(main_element >> 3);
            match match_offset {
                0 => match_offset = self.r0,
                1 => {
                    match_offset = self.r1;
                    self.r1 = self.r0;
                    self.r0 = match_offset;
                }
                2 => {
                    match_offset = self.r2;
                    self.r2 = self.r0;
                    self.r0 = match_offset;
                }
                3 => {
                    match_offset = 1;
                    self.r2 = self.r1;
                    self.r1 = self.r0;
                    self.r0 = match_offset;
                }
                slot => {
                    let extra = self.slots.extra[slot as usize];
                    let verbatim_bits = self.bits.read_bits(i32::from(extra))?;
                    match_offset = self.slots.base[slot as usize] - 2 + verbatim_bits;
                    self.r2 = self.r1;
                    self.r1 = self.r0;
                    self.r0 = match_offset;
                }
            }

            self.copy_match(match_offset, match_length)?;
            this_run -= i64::from(match_length);
        }
        Ok(this_run)
    }

    fn run_aligned(&mut self, mut this_run: i64) -> Result<i64, LzxError> {
        while this_run > 0 {
            let sym = self.bits.read_huffsym(&self.maintree)?;
            if sym < LZX_NUM_CHARS {
                self.window[self.window_posn as usize] = sym as u8;
                self.window_posn += 1;
                this_run -= 1;
                continue;
            }
            let main_element = sym - LZX_NUM_CHARS;
            let match_length = self.match_length(main_element)?;

            let mut match_offset = u32::from(main_element >> 3);
            match match_offset {
                0 => match_offset = self.r0,
                1 => {
                    match_offset = self.r1;
                    self.r1 = self.r0;
                    self.r0 = match_offset;
                }
                2 => {
                    match_offset = self.r2;
                    self.r2 = self.r0;
                    self.r0 = match_offset;
                }
                slot => {
                    let extra = i32::from(self.slots.extra[slot as usize]);
                    match_offset = self.slots.base[slot as usize] - 2;
                    if extra > 3 {
                        // Verbatim and aligned bits.
                        let verbatim_bits = self.bits.read_bits(extra - 3)?;
                        match_offset += verbatim_bits << 3;
                        let aligned_bits = self.bits.read_huffsym(&self.aligned)?;
                        match_offset += u32::from(aligned_bits);
                    } else if extra == 3 {
                        // Aligned bits only.
                        let aligned_bits = self.bits.read_huffsym(&self.aligned)?;
                        match_offset += u32::from(aligned_bits);
                    } else if extra > 0 {
                        // Verbatim bits only (extra == 1 or 2).
                        match_offset += self.bits.read_bits(extra)?;
                    } else {
                        // extra == 0: not defined in the LZX spec.
                        match_offset = 1;
                    }
                    self.r2 = self.r1;
                    self.r1 = self.r0;
                    self.r0 = match_offset;
                }
            }

            self.copy_match(match_offset, match_length)?;
            this_run -= i64::from(match_length);
        }
        Ok(this_run)
    }

    fn run_uncompressed(&mut self, this_run: i64) -> Result<i64, LzxError> {
        // `this_run` is limited so as not to wrap a frame, which also
        // means it can't wrap the window (a multiple of 32k).
        let mut dest = self.window_posn as usize;
        self.window_posn += this_run as u32;
        let mut left = this_run;
        while left > 0 {
            let avail = self.bits.data.len().saturating_sub(self.bits.pos);
            if avail == 0 {
                return Err(LzxError::Read);
            }
            let take = avail.min(left as usize);
            self.window[dest..dest + take]
                .copy_from_slice(&self.bits.data[self.bits.pos..self.bits.pos + take]);
            dest += take;
            self.bits.pos += take;
            left -= take as i64;
        }
        Ok(0)
    }

    /// Undo the E8 (call) preprocessing for one frame, then append it to
    /// the output.
    fn emit_frame(&mut self, frame_size: u32, out_bytes: &mut i64) {
        let start = self.frame_posn as usize;
        let end = start + frame_size as usize;
        let take = (*out_bytes).min(i64::from(frame_size)) as usize;

        if self.intel_started && self.intel_filesize != 0 && self.frame <= 32768 && frame_size > 10
        {
            let buf = &mut self.e8_buf[..frame_size as usize];
            buf.copy_from_slice(&self.window[start..end]);
            let mut curpos = self.intel_curpos;
            let filesize = self.intel_filesize;
            let dataend = frame_size as usize - 10;
            let mut i = 0usize;
            while i < dataend {
                if buf[i] != 0xE8 {
                    i += 1;
                    curpos += 1;
                    continue;
                }
                i += 1;
                let abs_off = i32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]);
                if abs_off >= -curpos && abs_off < filesize {
                    let rel_off = if abs_off >= 0 {
                        abs_off.wrapping_sub(curpos)
                    } else {
                        abs_off.wrapping_add(filesize)
                    };
                    buf[i..i + 4].copy_from_slice(&rel_off.to_le_bytes());
                }
                i += 4;
                curpos += 5;
            }
            self.intel_curpos += frame_size as i32;
            self.out.extend_from_slice(&self.e8_buf[..take]);
        } else {
            if self.intel_filesize != 0 {
                self.intel_curpos += frame_size as i32;
            }
            self.out
                .extend_from_slice(&self.window[start..start + take]);
        }

        self.offset += take as u64;
        *out_bytes -= take as i64;
    }
}

#[derive(Clone, Copy)]
enum Tree {
    Maintree,
    Length,
}

/// Decompress `data` into exactly `out_length` bytes.
///
/// This is `lzx.decompress(data, outlen)` as exposed by `lzxmodule.c`,
/// with `window_bits` supplied by the preceding `lzx.init(window)` call.
pub fn decompress(data: &[u8], window_bits: u32, out_length: usize) -> Result<Vec<u8>, LzxError> {
    let mut lzx = Lzxd::new(data, window_bits, out_length)?;
    lzx.decompress()?;
    Ok(std::mem::take(&mut lzx.out))
}

/// Port of `calibre.ebooks.lit.lzx.Decompressor`.
///
/// The Python class only remembers the window size — `lzx.init()`
/// stashes it and every `decompress()` builds a fresh stream — so this
/// carries no state between calls either.
#[derive(Clone, Copy, Debug)]
pub struct Decompressor {
    /// Window size in bits.
    pub wbits: u32,
    /// `1 << wbits`.
    pub blocksize: usize,
}

impl Decompressor {
    /// `Decompressor.__init__`.
    pub fn new(wbits: u32) -> Self {
        Decompressor {
            wbits,
            blocksize: 1usize << wbits,
        }
    }

    /// `Decompressor.decompress`.
    pub fn decompress(&self, data: &[u8], out_length: usize) -> Result<Vec<u8>, LzxError> {
        decompress(data, self.wbits, out_length)
    }

    /// `Decompressor.reset` — a no-op, as in the C ("Doesn't exist. Oh
    /// well, reinitialize state every time anyway").
    pub fn reset(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_window_sizes_outside_the_supported_range() {
        for bits in [0u32, 14, 22, 32] {
            assert!(matches!(
                decompress(&[0u8; 16], bits, 16),
                Err(LzxError::Args(_))
            ));
        }
        // 15..=21 at least get as far as parsing the bitstream.
        for bits in 15u32..=21 {
            assert!(!matches!(
                decompress(&[0u8; 16], bits, 0),
                Err(LzxError::Args(_))
            ));
        }
    }

    #[test]
    fn zero_length_output_is_empty_and_reads_nothing() {
        assert!(decompress(&[], 17, 0).expect("no work to do").is_empty());
    }

    #[test]
    fn truncated_input_is_a_read_error_not_a_panic() {
        // Not a valid stream; the point is that it fails cleanly.
        let err = decompress(&[0xff; 4], 17, 32768).unwrap_err();
        assert!(matches!(err, LzxError::Read | LzxError::Decrunch(_)));
    }

    #[test]
    fn position_slot_tables_match_the_c_construction() {
        let slots = PositionSlots::new();
        // extra_bits = 0,0,0,0,1,1,2,2,3,3,...
        assert_eq!(&slots.extra[..10], &[0, 0, 0, 0, 1, 1, 2, 2, 3, 3]);
        assert_eq!(slots.extra[50], 17);
        // position_base = 0,1,2,3,4,6,8,12,16,24,32,...
        assert_eq!(&slots.base[..11], &[0, 1, 2, 3, 4, 6, 8, 12, 16, 24, 32]);
    }

    #[test]
    fn an_all_zero_length_table_builds_without_error() {
        // Many CAB files have a completely empty length tree.
        let mut tbl = HuffTable::new(LENGTH_MAXSYMBOLS, LENGTH_TABLEBITS);
        assert!(tbl.build().is_ok());
    }

    #[test]
    fn an_incomplete_table_is_rejected() {
        let mut tbl = HuffTable::new(ALIGNED_MAXSYMBOLS, ALIGNED_TABLEBITS);
        // One symbol with a 2-bit code leaves the table under-filled.
        tbl.len[0] = 2;
        assert!(tbl.build().is_err());
    }
}
