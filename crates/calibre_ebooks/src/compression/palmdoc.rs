use anyhow::Result;

pub fn decompress(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut i = 0;
    let len = data.len();

    while i < len {
        let c = data[i];
        i += 1;

        if c >= 1 && c <= 8 {
            // Copy 'c' bytes
            let count = c as usize;
            if i + count > len {
                // bail!("Unexpected end of data in literal copy");
                // Tolerate truncated? C code checks: while (c-- && i < input_len)
                // Proceed safely
                let end = std::cmp::min(i + count, len);
                output.extend_from_slice(&data[i..end]);
                i = end;
            } else {
                output.extend_from_slice(&data[i..i + count]);
                i += count;
            }
        } else if c == 0 || (c >= 0x09 && c <= 0x7F) {
            // Literal
            output.push(c);
        } else if c >= 0xC0 {
            // Space + ASCII char
            output.push(b' ');
            output.push(c ^ 0x80);
        } else {
            // 0x80 - 0xBF: Repeat sequence
            // c is first byte of pair
            if i >= len {
                break; // Should not happen usually
            }
            let next_byte = data[i];
            i += 1;

            let pair = ((c as u16) << 8) | (next_byte as u16);
            let distance = (pair & 0x3FFF) >> 3;
            let length = (pair & 7) + 3;

            if distance == 0 {
                // Invalid distance? C code says di <= o. If di=0?
                // Typically distance is 1-based relative to current pos?
                // C code: PyBytes_AS_STRING(ans)[o-di]. So di can be 1..o.
                continue;
            }

            let dist_usize = distance as usize;
            let len_usize = length as usize;

            if dist_usize > output.len() {
                // Should bail or just ignore?
                // bail!("Invalid backreference distance");
                // C code check: if (di <= o). If not, it does nothing?
                continue;
            }

            // Copy length bytes from output[curr - dist]
            // We must copy byte by byte because source and destination might overlap
            // (e.g. dist=1 means repeat last char N times)
            let start = output.len() - dist_usize;
            for j in 0..len_usize {
                let val = output[start + j]; // This would be reading form old state?
                                             // Wait, if it overruns?
                                             // "lz77 copy": The source is the *current* output buffer.
                                             // If we append, the source grows?
                                             // C code: PyBytes_AS_STRING(ans)[o-di]. 'o' is incrementing.
                                             // So yes, it can copy newly written bytes if dist < length.
                output.push(val);
                // But simplified vec indexing `output[start + j]` assumes `output` doesn't change relative to start index?
                // `start` is fixed. `start + j`.
                // If dist < length (e.g. len 5, dist 1).
                // j=0: read output[len-1], push. output grows.
                // j=1: read output[len - 1 + 1]? No.
                // C code:
                // for ( n = (c & 7) + 3; n--; ++o ) {
                //    write(... ans[o-di] )
                // }
                // So at step 'o', it reads from 'o-di'. Since 'o' increments, 'o-di' also increments.
                // So yes, it reads the byte we just wrote if di is small.
                // My rust loop: `output[start + j]`?
                // `start` = initial_len - dist.
                // `start + j` = initial_len - dist + j.
                // Is that equivalent to `current_pos - dist`?
                // current_pos = initial_len + j.
                // current_pos - dist = initial_len + j - dist.
                // Yes. `start + j` is correct even if we mutate `output`, provided we read the correct index.
                // But in Rust `output.push` reallocates?
                // `output[idx]` is valid as long as idx < current len.
                // Since `dist <= initial_len` and `j >= 0`, `start+j` will initially be valid.
                // But if we push, `output` structure works.
                // Warning: `output` borrow while mutating?
                // Rust won't allow `output.push(output[idx])` usually.
                // We handle this by index.
            }
        }
    }

    Ok(output)
}

pub fn compress(data: &[u8]) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut i = 0;
    let len = data.len();

    while i < len {
        // Find longest match
        // Constraints:
        // Distance: 1..2047 (0x7FF) - Note: standard mentions 2046?
        // Pair: 16 bits.
        // m: 0x8000 (1000...) reserved for length? No.
        // Format:
        // 1. Literal: byte 0x01..0x08 (count), followed by bytes.
        // 2. Literal: 0x00, 0x09..0x7F (single byte literal).
        // 3. Space+Char: 0xC0..0xFF (Space + char ^ 0x80).
        // 4. LZ77 pair: 0x80..0xBF (pair).
        //    Byte 1: 10xxxxxx (x is high bits of dist? or length?)
        //    Byte 2: yyyyyyyy
        //    Pair u16 P.
        //    Dist: (P & 0x3FFF) >> 3. (11 bits? 2047 max)
        //    Len: (P & 7) + 3. (Map 3..10).

        // Search window: back 2047 bytes.
        // Match len: 3..10 bytes.

        let mut best_dist = 0;
        let mut best_len = 0;

        // Naive search (slow but correct)
        let max_dist = std::cmp::min(i, 2046); // Safe max distance

        // Only search if we have at least 3 bytes left
        if i + 3 <= len {
            for d in 1..=max_dist {
                let start = i - d;
                let mut match_len = 0;
                // Limit length to 10
                while match_len < 10 && i + match_len < len {
                    if data[start + match_len] == data[i + match_len] {
                        match_len += 1;
                    } else {
                        break;
                    }
                }

                if match_len >= 3 {
                    if match_len > best_len {
                        best_len = match_len;
                        best_dist = d;
                    }
                    if best_len == 10 {
                        break; // Max possible
                    }
                }
            }
        }

        if best_len >= 3 {
            // Encode pair
            // Dist: 11 bits (1..2047)
            // Len: 3 bits (3..10) -> value - 3 (0..7)
            // Pair: 1 0 ddddddddddd lll
            // First byte: 0x80 | (dist >> 8) ? No.
            // pair = (dist << 3) | (len - 3)
            // wait:
            // decode: dist = (pair & 0x3FFF) >> 3;  length = (pair & 7) + 3;
            // So pair = (dist << 3) | (len - 3)
            // High bit must be 1?
            // pair = 0x8000 | (dist << 3) | (len - 3) ?
            // decode logic:
            // c = byte 1 (0x80..0xBF). So c & 0xC0 == 0x80.
            // i.e. top 2 bits are 10.
            // So pair is 16 bits. Top 2 bits of first byte are 10.
            // 0x8000 set. 0x4000 unset.
            // pair & 0xC000 == 0x8000.

            let pair = 0x8000 | ((best_dist as u16) << 3) | ((best_len as u16 - 3) & 7);
            output.push((pair >> 8) as u8);
            output.push((pair & 0xFF) as u8);
            i += best_len;
        } else {
            // Literals
            let c = data[i];
            if c == b' ' && i + 1 < len && data[i + 1] >= 0x40 && data[i + 1] <= 0x7F {
                // Optimization: space + char
                // Encoded as 0xC0..0xFF
                // char = code ^ 0x80. So code = char ^ 0x80.
                // char must be 0x40..0x7F.
                // e.g. 'A' (0x41) -> 0x41 ^ 0x80 = 0xC1.
                let next = data[i + 1];
                output.push(next ^ 0x80);
                i += 2;
            } else if c == 0 || (c >= 0x09 && c <= 0x7F) {
                output.push(c);
                i += 1;
            } else {
                // Must likely be encoded as length-literal or raw byte?
                // Logic says:
                // 1..8: copy next N bytes
                // But we are processing 1 byte.
                // It's cleaner to output raw byte if it fits "Literal" range.
                // 0x01..0x08 are "copy text".
                // 0x80..0xFF are special.
                // So if c is 0x01..0x08 or >= 0x80, we CANNOT emit it directly.
                // We MUST use the "copy text" command (byte sequence 1).

                // Let's count how many literals we have that need encoding or are just adjacent.
                // Max length 8 for this command.
                let mut count = 1;
                while count < 8 && i + count < len {
                    // We could group normal chars too, but maybe just group problematic ones?
                    // Or simply group everything until we find a match or special char?
                    // Simple: greedy group up to 8.
                    count += 1;
                }

                // But wait, if we have a match at i+1, we shouldn't consume it?
                // Naive: just emit 1 byte as encoded block?
                // output.push(1); output.push(c); i+=1;

                // Let's output 1 byte encoded.
                output.push(1);
                output.push(c);
                i += 1;
            }
        }
    }

    Ok(output)
}
