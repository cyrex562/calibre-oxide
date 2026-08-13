//! The LZ match finder behind the LZX compressor.
//!
//! Port of `src/calibre/utils/lzx/lzc.c` (`lz_nonslide.c` in lzxcomp),
//! Copyright (C) 2002 Matthew T. Russotto. The C is compiled with
//! `NONSLIDE`, `LZ_ONEBUFFER` and `LAZY` defined, so only those paths
//! are ported.
//!
//! The engine holds a single flat block buffer rather than a sliding
//! window, and analyses the whole buffer up front: `lentab[i]` is the
//! length of the best match ending at `i` and `prevtab[i]` is where that
//! match starts. Matches are then emitted left to right, with one step
//! of lazy evaluation.

/// The caller-supplied side of the engine. `lz_info`'s three callbacks
/// in the C, which all reach back into `lzxc_data`.
pub trait LzSink {
    /// `get_chars` — fill `buf` with up to `buf.len()` more input bytes,
    /// returning how many were written.
    fn get_chars(&mut self, buf: &mut [u8]) -> usize;

    /// `output_match` — offer a match at `match_pos` (negative, relative
    /// to the current position) of `match_len` bytes. Returning `false`
    /// rejects it, and the byte is emitted as a literal instead.
    fn output_match(&mut self, match_pos: i32, match_len: i32, lz: &mut LzState) -> bool;

    /// `output_literal`.
    fn output_literal(&mut self, ch: u8, lz: &mut LzState);
}

/// The parts of `lz_info` a sink may touch or mutate mid-compression.
///
/// `lzx_output_match` needs `block_buf`/`block_loc` to verify repeated
/// offsets, and `check_entropy` needs to be able to stop the run.
pub struct LzState {
    /// `block_buf`.
    pub block_buf: Vec<u8>,
    /// `block_loc` — the current position within `block_buf`.
    pub block_loc: usize,
    /// `cur_loc` — the position within the overall input.
    pub cur_loc: u32,
    /// `stop` — set by `lz_stop_compressing`.
    pub stop: bool,
}

impl LzState {
    /// `lz_stop_compressing`.
    pub fn stop_compressing(&mut self) {
        self.stop = true;
    }
}

/// Port of `lz_info`.
pub struct LzInfo {
    /// Fields the sink is allowed to see.
    pub state: LzState,
    max_match: usize,
    min_match: usize,
    max_dist: usize,
    block_buf_size: usize,
    chars_in_buf: usize,
    eofcount: u32,
    frame_size: usize,
    lentab: Vec<i32>,
    /// `prevtab`, as indices into `block_buf` rather than pointers. A
    /// value of `NO_PREV` stands for the C's NULL.
    prevtab: Vec<usize>,
    analysis_valid: bool,
}

/// Stand-in for a NULL entry in `prevtab`.
const NO_PREV: usize = usize::MAX;

impl LzInfo {
    /// `lz_init`.
    ///
    /// The separate `max_dist` exists because LZX cannot reach the first
    /// three characters of its nominal window, but using a smaller
    /// window would be inefficient around reset intervals.
    pub fn new(
        wsize: usize,
        max_dist: usize,
        max_match: usize,
        min_match: usize,
        frame_size: usize,
    ) -> Self {
        let max_match = max_match.min(wsize);
        let min_match = min_match.max(3);
        let block_buf_size = wsize + max_dist;
        LzInfo {
            state: LzState {
                block_buf: vec![0u8; block_buf_size],
                block_loc: 0,
                cur_loc: 0,
                stop: false,
            },
            max_match,
            min_match,
            max_dist,
            block_buf_size,
            chars_in_buf: 0,
            eofcount: 0,
            frame_size,
            lentab: vec![0i32; block_buf_size + 1],
            prevtab: vec![NO_PREV; block_buf_size + 1],
            analysis_valid: false,
        }
    }

    /// `lz_reset`.
    pub fn reset(&mut self) {
        let residual = self.chars_in_buf - self.state.block_loc;
        self.state
            .block_buf
            .copy_within(self.state.block_loc..self.chars_in_buf, 0);
        self.chars_in_buf = residual;
        self.state.block_loc = 0;
        self.analysis_valid = false;
    }

    /// `lz_left_to_process`.
    pub fn left_to_process(&self) -> usize {
        self.chars_in_buf - self.state.block_loc
    }

    /// `fill_blockbuf`.
    fn fill_blockbuf<S: LzSink>(&mut self, sink: &mut S, maxchars: usize) {
        if self.eofcount != 0 {
            return;
        }
        let maxchars = maxchars.saturating_sub(self.left_to_process());
        let mut toread = self.block_buf_size - self.chars_in_buf;
        if toread > maxchars {
            toread = maxchars;
        }
        let start = self.chars_in_buf;
        let nread = sink.get_chars(&mut self.state.block_buf[start..start + toread]);
        self.chars_in_buf += nread;
        if nread != toread {
            self.eofcount += 1;
        }
    }

    /// `lz_analyze_block` — build `lentab`/`prevtab` for the whole
    /// buffer.
    ///
    /// The first pass chains equal bytes; each subsequent pass extends
    /// every chain that currently has length `maxlen` by one byte, so
    /// after the loop `lentab[i]` holds the longest match ending at `i`
    /// within `max_dist`.
    fn analyze_block(&mut self) {
        let n = self.chars_in_buf;
        let buf = &self.state.block_buf;
        let lentab = &mut self.lentab;
        let prevtab = &mut self.prevtab;
        lentab[..n].iter_mut().for_each(|l| *l = 0);
        prevtab[..n].iter_mut().for_each(|p| *p = NO_PREV);

        let mut chartab = [NO_PREV; 256];
        for i in 0..n {
            let ch = buf[i] as usize;
            if chartab[ch] != NO_PREV {
                prevtab[i] = chartab[ch];
                lentab[i] = 1;
            }
            chartab[ch] = i;
        }

        let max_dist = self.max_dist;
        let mut wasinc = true;
        let mut maxlen = 1usize;
        while wasinc && maxlen < self.max_match {
            wasinc = false;
            // The C walks bbp from `bbe - maxlen - 1` down to, but not
            // including, `block_buf`.
            if n <= maxlen + 1 {
                maxlen += 1;
                continue;
            }
            let mut i = n - maxlen - 1;
            while i > 0 {
                if lentab[i] == maxlen as i32 {
                    let ch = buf[i + maxlen];
                    let mut cursor = prevtab[i];
                    while cursor != NO_PREV && (i - cursor) <= max_dist {
                        let prevlen = lentab[cursor];
                        if buf[cursor + maxlen] == ch {
                            prevtab[i] = cursor;
                            lentab[i] += 1;
                            wasinc = true;
                            break;
                        }
                        if prevlen != maxlen as i32 {
                            break;
                        }
                        cursor = prevtab[cursor];
                    }
                }
                i -= 1;
            }
            maxlen += 1;
        }
        self.analysis_valid = true;
    }

    /// `lz_compress` — emit up to `nchars` bytes' worth of matches and
    /// literals through `sink`.
    pub fn compress<S: LzSink>(&mut self, sink: &mut S, mut nchars: usize) {
        self.state.stop = false;
        while (self.left_to_process() > 0 || self.eofcount == 0) && !self.state.stop && nchars > 0 {
            if !self.analysis_valid
                || (self.eofcount == 0 && (self.chars_in_buf - self.state.block_loc) < nchars)
            {
                let residual = self.chars_in_buf - self.state.block_loc;
                let mut bytes_to_move = self.max_dist + residual;
                if bytes_to_move > self.chars_in_buf {
                    bytes_to_move = self.chars_in_buf;
                }
                self.state
                    .block_buf
                    .copy_within(self.chars_in_buf - bytes_to_move..self.chars_in_buf, 0);
                self.state.block_loc = bytes_to_move - residual;
                self.chars_in_buf = bytes_to_move;
                self.fill_blockbuf(sink, nchars);
                self.analyze_block();
            }

            let holdback = if self.eofcount != 0 {
                0
            } else {
                self.max_match
            };
            let end = if self.chars_in_buf < nchars + self.state.block_loc {
                self.chars_in_buf.saturating_sub(holdback)
            } else {
                self.state.block_loc + nchars
            };

            while self.state.block_loc < end && !self.state.stop {
                let loc = self.state.block_loc;
                let mut trimmed = false;
                let mut len = self.lentab[loc];

                if self.frame_size != 0 {
                    let to_frame_end =
                        self.frame_size - (self.state.cur_loc as usize % self.frame_size);
                    if len > to_frame_end as i32 {
                        trimmed = true;
                        len = to_frame_end as i32;
                    }
                }
                if len > nchars as i32 {
                    trimmed = true;
                    len = nchars as i32;
                }

                if len >= self.min_match as i32 {
                    // Lazy evaluation: if the next position starts a
                    // strictly longer match, emit a literal here.
                    if loc + 1 < end && !trimmed && self.lentab[loc + 1] > len + 1 {
                        len = 1;
                    } else {
                        let match_pos = self.prevtab[loc] as i32 - loc as i32;
                        if !sink.output_match(match_pos, len, &mut self.state) {
                            len = 1; // match rejected
                        }
                    }
                } else {
                    len = 1;
                }

                if len < self.min_match as i32 {
                    let ch = self.state.block_buf[loc];
                    sink.output_literal(ch, &mut self.state);
                }

                let len = len as usize;
                self.state.block_loc += len;
                self.state.cur_loc += len as u32;
                nchars -= len;
            }
        }
    }
}
