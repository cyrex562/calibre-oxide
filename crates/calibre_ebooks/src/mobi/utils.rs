use anyhow::Result;

use std::collections::{BTreeMap, HashMap};
use std::io::Read;

use crate::oeb::toc::TOC;

/// Text record size (uncompressed). `RECORD_SIZE` in `mobi/utils.py`.
pub const RECORD_SIZE: usize = 0x1000;

/// Encode the integer `value` as a variable width integer and return the Vec<u8> corresponding to it.
/// If forward is true, the bytes returned are suitable for prepending to the output buffer,
/// otherwise they must be appended to the output buffer.
pub fn encint(mut value: u64, forward: bool) -> Vec<u8> {
    let mut byts = Vec::new();
    loop {
        let b = (value & 0x7F) as u8;
        value >>= 7;
        byts.push(b);
        if value == 0 {
            break;
        }
    }

    if forward {
        if let Some(first) = byts.first_mut() {
            *first |= 0x80;
        }
    } else {
        if let Some(last) = byts.last_mut() {
            *last |= 0x80;
        }
    }

    byts.reverse();
    byts
}

/// Read a variable width integer from the data and return the integer and the number of bytes read.
/// If forward is True bytes are read from the start of raw, otherwise from the end of raw.
pub fn decint(data: &[u8], forward: bool) -> Result<(u64, usize)> {
    let mut val: u64 = 0;
    let mut consumed = 0;

    let mut parts = Vec::new();

    if forward {
        for &b in data {
            consumed += 1;
            parts.push(b & 0x7F);
            if (b & 0x80) != 0 {
                break;
            }
        }
    } else {
        for &b in data.iter().rev() {
            consumed += 1;
            parts.push(b & 0x7F);
            if (b & 0x80) != 0 {
                break;
            }
        }
        parts.reverse();
    }

    for p in parts {
        val = (val << 7) | (p as u64);
    }

    Ok((val, consumed))
}

pub fn decode_string(raw: &[u8], codec: &str) -> Result<(String, usize)> {
    if raw.is_empty() {
        return Ok((String::new(), 0));
    }
    let length = raw[0] as usize;
    let content = &raw[1..1 + length];
    let consumed = length + 1;
    // codec handling is simplified here, assuming utf-8 or similar
    // for cp1252 etc we might need encoding_rs
    let s = if codec == "utf-8" {
        String::from_utf8_lossy(content).to_string()
    } else {
        // Fallback or use encoding_rs lookup
        match encoding_rs::Encoding::for_label(codec.as_bytes()) {
            Some(enc) => {
                let (cow, _, _) = enc.decode(content);
                cow.to_string()
            }
            None => String::from_utf8_lossy(content).to_string(),
        }
    };
    Ok((s, consumed))
}

pub fn encode_string(raw: &[u8]) -> Vec<u8> {
    let mut ans = Vec::with_capacity(raw.len() + 1);
    ans.push(raw.len() as u8);
    ans.extend_from_slice(raw);
    ans
}

/// Trailing-entry index -> raw bytes, plus the record with all trailing
/// entries stripped off.
pub type TrailingData = (BTreeMap<u32, Vec<u8>>, Vec<u8>);

pub fn get_trailing_data(record: &[u8], extra_data_flags: u32) -> Result<TrailingData> {
    let mut data = BTreeMap::new();
    let mut flags = extra_data_flags >> 1;
    let mut record_slice = record;

    let mut num = 0;
    while flags > 0 {
        num += 1;
        if (flags & 1) != 0 {
            let (sz, consumed) = decint(record_slice, false)?;
            let sz = sz as usize;
            let len = record_slice.len();
            if sz > consumed && sz <= len {
                let start = len - sz;
                let end = len - consumed;
                data.insert(num, record_slice[start..end].to_vec());
                record_slice = &record_slice[..start];
            } else {
                // Warning or error?
                // bail!("Invalid trailing data");
                // For robustness, maybe just stop?
                break;
            }
        }
        flags >>= 1;
    }

    if (extra_data_flags & 1) != 0 {
        let len = record_slice.len();
        if len > 0 {
            let last_byte = record_slice[len - 1];
            let sz = ((last_byte & 0x3) + 1) as usize;
            let consumed = 1;
            if sz > consumed && sz <= len {
                let start = len - sz;
                let end = len - consumed;
                data.insert(0, record_slice[start..end].to_vec());
                record_slice = &record_slice[..start];
            } else {
                record_slice = &record_slice[..len - sz]; // Strip it anyway?
            }
        }
    }

    Ok((data, record_slice.to_vec()))
}

pub fn encode_trailing_data(raw: &[u8]) -> Vec<u8> {
    let mut lsize = 1;
    loop {
        let encoded = encint((raw.len() + lsize) as u64, false);
        if encoded.len() == lsize {
            let mut res = raw.to_vec();
            res.extend_from_slice(&encoded);
            return res;
        }
        lsize += 1;
    }
}

pub fn encode_fvwi(val: u64, flags: u32, flag_size: u32) -> Vec<u8> {
    let mut ans = val << flag_size;
    for i in 0..flag_size {
        ans |= flags as u64 & (1 << i);
    }
    encint(ans, true)
}

pub fn decode_fvwi(byts: &[u8], flag_size: u32) -> Result<(u64, u32, usize)> {
    let (arg, consumed) = decint(byts, true)?;
    let val = arg >> flag_size;
    let mut flags = 0;
    for i in 0..flag_size {
        if (arg & (1 << i)) != 0 {
            flags |= 1 << i;
        }
    }
    Ok((val, flags as u32, consumed))
}

pub fn decode_tbs(byts: &[u8], flag_size: u32) -> Result<(u64, HashMap<u32, u64>, usize)> {
    let (val, flags, mut consumed) = decode_fvwi(byts, flag_size)?;
    let mut extra = HashMap::new();
    let mut current_slice = &byts[consumed..];

    if (flags & 0b1000) != 0 && flag_size > 3 {
        extra.insert(0b1000, 1); // True -> 1
    }
    if (flags & 0b0010) != 0 {
        let (x, c) = decint(current_slice, true)?;
        current_slice = &current_slice[c..];
        consumed += c;
        extra.insert(0b0010, x);
    }
    if (flags & 0b0100) != 0 && !current_slice.is_empty() {
        extra.insert(0b0100, current_slice[0] as u64);
        current_slice = &current_slice[1..];
        consumed += 1;
    }
    if (flags & 0b0001) != 0 {
        let (x, c) = decint(current_slice, true)?;
        // current_slice = &current_slice[c..]; // Not needed if last
        consumed += c;
        extra.insert(0b0001, x);
    }

    Ok((val, extra, consumed))
}

pub fn encode_tbs(val: u64, extra: &HashMap<u32, u64>, flag_size: u32) -> Vec<u8> {
    let mut flags = 0;
    for &flag in extra.keys() {
        flags |= flag;
    }
    let mut ans = encode_fvwi(val, flags, flag_size);

    if let Some(&v) = extra.get(&0b0010) {
        ans.extend_from_slice(&encint(v, true));
    }
    if let Some(&v) = extra.get(&0b0100) {
        ans.push(v as u8);
    }
    if let Some(&v) = extra.get(&0b0001) {
        ans.extend_from_slice(&encint(v, true));
    }
    ans
}

pub fn decode_hex_number(raw: &[u8]) -> Result<(u64, usize)> {
    let (s, consumed) = decode_string(raw, "utf-8")?;
    let val = u64::from_str_radix(&s, 16)?;
    Ok((val, consumed))
}

pub fn encode_number_as_hex(num: u64) -> Vec<u8> {
    let s = format!("{:X}", num);
    let mut s_bytes = s.into_bytes();
    if s_bytes.len() % 2 != 0 {
        s_bytes.insert(0, b'0');
    }
    encode_string(&s_bytes)
}

pub struct CNCX {
    pub records: Vec<Vec<u8>>,
    pub strings: HashMap<String, u32>,
}

impl CNCX {
    const MAX_STRING_LENGTH: usize = 500;
    const RECORD_LIMIT: usize = 0x10000 - 1024;

    pub fn new(strings: &[String]) -> Self {
        let mut cncx = CNCX {
            records: Vec::new(),
            strings: HashMap::new(),
        };

        for s in strings {
            cncx.strings.insert(s.clone(), 0);
        }

        let mut offset = 0;
        let mut buf = Vec::new();

        for key in strings {
            let truncated_key = if key.len() > Self::MAX_STRING_LENGTH {
                &key[..Self::MAX_STRING_LENGTH]
            } else {
                key
            };

            let key_bytes = truncated_key.as_bytes();
            let l = key_bytes.len();
            let sz_bytes = encint(l as u64, true);

            let mut raw = sz_bytes;
            raw.extend_from_slice(key_bytes);

            if buf.len() + raw.len() > Self::RECORD_LIMIT {
                cncx.records.push(align_block(&buf, 4, 0));
                buf.clear();
                offset = cncx.records.len() * 0x10000;
            }

            buf.extend_from_slice(&raw);
            cncx.strings.insert(key.clone(), offset as u32);
            offset += raw.len();
        }

        if !buf.is_empty() {
            cncx.records.push(align_block(&buf, 4, 0));
        }

        cncx
    }
}

/// A non-empty, NFC-normalized UTF-8 encoding of `text.trim()`, or
/// `b"Unknown"` if `text` is empty/blank after trimming.
///
/// Port of `utf8_text` in `mobi/utils.py`. Used by
/// `writer8::exth::build_exth` for metadata values and by
/// `writer8::mobi::MOBIHeader`'s `full_title` field.
pub fn utf8_text(text: &str) -> Vec<u8> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        b"Unknown".to_vec()
    } else {
        let normalized: String =
            unicode_normalization::UnicodeNormalization::nfc(trimmed).collect();
        normalized.into_bytes()
    }
}

pub fn align_block(raw: &[u8], multiple: usize, pad: u8) -> Vec<u8> {
    let extra = raw.len() % multiple;
    if extra == 0 {
        return raw.to_vec();
    }
    let mut res = raw.to_vec();
    res.extend(std::iter::repeat_n(pad, multiple - extra));
    res
}

pub fn convert_color_for_font_tag(val: &str) -> String {
    if val.starts_with('#') {
        return val.to_string();
    }
    val.to_string()
}

pub fn is_guide_ref_start(title: Option<&str>, type_: Option<&str>) -> bool {
    if let Some(t) = title {
        if t.eq_ignore_ascii_case("start") {
            return true;
        }
    }
    if let Some(t) = type_ {
        let lower = t.to_lowercase();
        if lower == "start" || lower == "other.start" || lower == "text" {
            return true;
        }
    }
    false
}

// Stub implementations
pub fn mobify_image(data: &[u8]) -> Result<Vec<u8>> {
    Ok(data.to_vec())
}

pub fn rescale_image(
    _data: &[u8],
    _max_size: usize,
    _dimen: Option<(u32, u32)>,
) -> Result<Vec<u8>> {
    Ok(Vec::new())
}

/// A decoded `FONT` record. `read_font_record`'s return dict in
/// `mobi/utils.py`, typed.
#[derive(Debug, Clone)]
pub struct FontRecordInfo {
    /// The record's raw bytes, unmodified.
    pub raw_data: Vec<u8>,
    /// The de-obfuscated/decompressed font payload, if decoding
    /// succeeded.
    pub font_data: Option<Vec<u8>>,
    /// Why decoding failed, if it did.
    pub err: Option<String>,
    /// `ttf`, `otf`, `dat` (undetermined), or `failed`.
    pub ext: &'static str,
    /// Whether the payload was XOR-obfuscated.
    pub encrypted: bool,
}

/// Decode the font embedded in a MOBI `FONT` record.
///
/// Port of `read_font_record` in `mobi/utils.py`. Layout:
/// `FONT` (4) + uncompressed size (4) + flags (4) + offset to
/// compressed data (4) + XOR key length (4) + offset to XOR key (4),
/// all big-endian, followed by the payload. Flag bit 0 is zlib
/// compression, bit 1 is XOR obfuscation.
///
/// `extent` bounds how many bytes of the payload get XORed — every
/// font calibre has ever encountered obfuscates exactly 1040 bytes,
/// which is `DEFAULT_FONT_XOR_EXTENT`.
pub fn read_font_record(data: &[u8], extent: usize) -> FontRecordInfo {
    let raw_data = data.to_vec();
    let mut info = FontRecordInfo {
        raw_data: raw_data.clone(),
        font_data: None,
        err: None,
        ext: "failed",
        encrypted: false,
    };

    if data.len() < 24 {
        info.err = Some("Failed to read font record header fields".to_string());
        return info;
    }
    let usize_ = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let flags = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
    let dstart = u32::from_be_bytes([data[12], data[13], data[14], data[15]]) as usize;
    let xor_len = u32::from_be_bytes([data[16], data[17], data[18], data[19]]) as usize;
    let xor_start = u32::from_be_bytes([data[20], data[21], data[22], data[23]]) as usize;

    if dstart > data.len() {
        info.err = Some("Failed to read font record header fields".to_string());
        return info;
    }
    let mut font_data = data[dstart..].to_vec();

    if flags & 0b10 != 0 {
        if xor_len == 0 || xor_start + xor_len > data.len() {
            info.err = Some("Failed to read font record header fields".to_string());
            return info;
        }
        let key = &data[xor_start..xor_start + xor_len];
        let extent = extent.min(font_data.len());
        for (n, byte) in font_data.iter_mut().enumerate().take(extent) {
            *byte ^= key[n % xor_len];
        }
        info.encrypted = true;
    }

    if flags & 0b1 != 0 {
        let mut decoder = flate2::read::ZlibDecoder::new(&font_data[..]);
        let mut decompressed = Vec::new();
        match decoder.read_to_end(&mut decompressed) {
            Ok(_) if decompressed.len() == usize_ => font_data = decompressed,
            Ok(_) => {
                info.err = Some("Uncompressed font size mismatch".to_string());
                return info;
            }
            Err(e) => {
                info.err = Some(format!("Failed to zlib decompress font data ({e})"));
                return info;
            }
        }
    }

    let sig = &font_data[..font_data.len().min(4)];
    info.ext = if matches!(sig, b"\0\x01\0\0" | b"true" | b"ttcf") {
        "ttf"
    } else if sig == b"OTTO" {
        "otf"
    } else {
        "dat"
    };
    info.font_data = Some(font_data);
    info
}

/// The default XOR extent every font record calibre has encountered
/// uses. `read_font_record(data, extent=1040)` in `mobi/utils.py`.
pub const DEFAULT_FONT_XOR_EXTENT: usize = 1040;

/// Encode a ttf/otf font as a MOBI `FONT` record.
///
/// Port of `write_font_record` in `mobi/utils.py`. Inverse of
/// [`read_font_record`]: header layout is `FONT` + 5 big-endian u32s
/// (uncompressed size, flags, data start offset, xor key length, xor key
/// start offset), followed by the (optional) XOR key and then the
/// (optionally compressed, optionally obfuscated) font payload.
///
/// Obfuscation only applies (as in the Python) when the *compressed*
/// payload is at least [`DEFAULT_FONT_XOR_EXTENT`] bytes long -- short
/// fonts are left in the clear.
pub fn write_font_record(data: &[u8], obfuscate: bool, compress: bool) -> Vec<u8> {
    let key_len = 20usize;
    let usize_ = data.len() as u32;
    let mut flags: u32 = 0;
    let mut payload = data.to_vec();
    let mut xor_key: Vec<u8> = Vec::new();

    if compress {
        flags |= 0b1;
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::new(9));
        // A `Vec<u8>` writer never fails, so these are infallible in
        // practice; propagate defensively rather than unwrap.
        if std::io::Write::write_all(&mut enc, &payload).is_ok() {
            if let Ok(compressed) = enc.finish() {
                payload = compressed;
            }
        }
    }

    if obfuscate && payload.len() >= 1040 {
        flags |= 0b10;
        xor_key = (0..key_len).map(|_| rand::random::<u8>()).collect();
        let extent = payload.len().min(1040);
        for (i, b) in payload.iter_mut().enumerate().take(extent) {
            *b ^= xor_key[i % key_len];
        }
    }

    let key_start = 4u32 * 5 + 4; // struct.calcsize('>5L') + len('FONT')
    let data_start = key_start + xor_key.len() as u32;

    let mut header = Vec::with_capacity(24);
    header.extend_from_slice(b"FONT");
    header.extend_from_slice(&usize_.to_be_bytes());
    header.extend_from_slice(&flags.to_be_bytes());
    header.extend_from_slice(&data_start.to_be_bytes());
    header.extend_from_slice(&(xor_key.len() as u32).to_be_bytes());
    header.extend_from_slice(&key_start.to_be_bytes());

    let mut out = header;
    out.extend_from_slice(&xor_key);
    out.extend_from_slice(&payload);
    out
}

/// Render `num` in `base` (up to 36: digits then uppercase letters),
/// left-padded with zeros to `min_num_digits` if given.
///
/// Port of `to_base` in `mobi/utils.py`. Used for the `kf8_thumbnail_uri`
/// EXTH value (`kindle:embed:XXXX`, base 32, 4 digits).
pub fn to_base(mut num: i64, base: i64, min_num_digits: Option<usize>) -> String {
    const DIGITS: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let sign = if num >= 0 { 1 } else { -1 };
    if num == 0 {
        return match min_num_digits {
            Some(n) => "0".repeat(n.max(1)),
            None => "0".to_string(),
        };
    }
    num *= sign;
    let mut chars = Vec::new();
    while num != 0 {
        let digit = (num % base) as usize;
        chars.push(DIGITS[digit.min(DIGITS.len() - 1)]);
        num /= base;
    }
    if let Some(min) = min_num_digits {
        while chars.len() < min {
            chars.push(b'0');
        }
    }
    if sign < 0 {
        chars.push(b'-');
    }
    chars.reverse();
    String::from_utf8(chars).unwrap_or_default()
}

/// Split the byte-length of a UTF-8 lead byte (ASCII or a 2/3/4-byte
/// sequence start). Continuation bytes and invalid lead bytes are
/// treated as length-1 so callers can't loop forever on malformed input.
fn utf8_lead_len(b: u8) -> usize {
    if b < 0x80 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

/// Slice one PalmDOC-record-sized (<= [`RECORD_SIZE`] byte) chunk out of
/// `text` starting at `*pos`, plus any extra "overlap" bytes needed to
/// complete a UTF-8 character that would otherwise be split across the
/// record boundary. Advances `*pos` to the next record boundary (i.e.
/// *not* including the overlap, matching `text.seek(npos)` in Python).
///
/// Port of `create_text_record` in `mobi/utils.py`. The Python
/// implementation finds the boundary by repeatedly attempting a UTF-8
/// decode of a growing trailing slice; since `text` here is always known
/// to be valid UTF-8 as a whole (it's our own serializer's output), this
/// is reimplemented as a direct UTF-8 lead-byte scan for the same
/// result, rather than transliterating the decode-and-retry loop.
pub fn create_text_record(text: &[u8], pos: &mut usize) -> (Vec<u8>, Vec<u8>) {
    let opos = *pos;
    let total = text.len();
    let npos = (opos + RECORD_SIZE).min(total);

    let mut extra = 0usize;
    if npos < total && npos > opos {
        let mut j = npos;
        let mut steps = 0;
        while j > opos && steps < 4 {
            j -= 1;
            steps += 1;
            let b = text[j];
            if b < 0x80 || (b & 0xC0) != 0x80 {
                let clen = utf8_lead_len(b);
                if j + clen > npos {
                    extra = (j + clen).min(total) - npos;
                }
                break;
            }
        }
    }

    let data = text[opos..npos].to_vec();
    let overlap = text[npos..npos + extra].to_vec();
    *pos = npos;
    (data, overlap)
}

/// Detect whether `toc` has the exact three-level
/// periodical/section/article shape kindlegen requires to generate a
/// periodical (as opposed to an ordinary book).
///
/// Port of `detect_periodical` in `mobi/utils.py`.
pub fn detect_periodical(toc: &TOC, mut log: Option<&mut crate::mobi::MobiLog>) -> bool {
    if toc.count() < 1 {
        return false;
    }
    let Some(first) = toc.first() else {
        return false;
    };
    if first.klass.as_deref() != Some("periodical") {
        return false;
    }
    for node in toc.iter_descendants(false) {
        let depth = node.depth();
        if depth == 1 && node.klass.as_deref() != Some("article") {
            if let Some(l) = log.as_deref_mut() {
                l.debug("Not a periodical: Deepest node does not have class=\"article\"");
            }
            return false;
        }
        if depth == 2 && node.klass.as_deref() != Some("section") {
            if let Some(l) = log.as_deref_mut() {
                l.debug("Not a periodical: Second deepest node does not have class=\"section\"");
            }
            return false;
        }
        if depth == 3 && node.klass.as_deref() != Some("periodical") {
            if let Some(l) = log.as_deref_mut() {
                l.debug("Not a periodical: Third deepest node does not have class=\"periodical\"");
            }
            return false;
        }
        if depth > 3 {
            if let Some(l) = log.as_deref_mut() {
                l.debug("Not a periodical: Has nodes of depth > 3");
            }
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_encint_decint() {
        let num = 0x11111; // 69905

        // Forward
        let encoded_fwd = encint(num, true);
        assert_eq!(encoded_fwd, vec![0x04, 0x22, 0x91]);
        let (decoded_fwd, len_fwd) = decint(&encoded_fwd, true).unwrap();
        assert_eq!(decoded_fwd, num);
        assert_eq!(len_fwd, 3);

        // Backward
        let encoded_bwd = encint(num, false);
        assert_eq!(encoded_bwd, vec![0x84, 0x22, 0x11]);
        let (decoded_bwd, len_bwd) = decint(&encoded_bwd, false).unwrap();
        assert_eq!(decoded_bwd, num);
        assert_eq!(len_bwd, 3);
    }

    #[test]
    fn test_trailing_data() {
        let raw_data = b"SomeContentHere";
        let trailer1 = b"Trailer1";
        let encoded_trailer1 = encode_trailing_data(trailer1);

        let mut full_record = raw_data.to_vec();
        full_record.extend_from_slice(&encoded_trailer1);

        // flag = 2 (bit 1 set) -> num=1
        let flags = 0b10;

        let (data, content) = get_trailing_data(&full_record, flags).unwrap();

        assert_eq!(content, raw_data);
        assert_eq!(data.get(&1).unwrap(), trailer1);
    }

    #[test]
    fn test_tbs() {
        let val = 123;
        let mut extra = HashMap::new();
        extra.insert(0b0010, 456);
        extra.insert(0b0100, 77); // 'M'

        let encoded = encode_tbs(val, &extra, 4);
        let (decoded_val, decoded_extra, consumed) = decode_tbs(&encoded, 4).unwrap();

        assert_eq!(decoded_val, val);
        assert_eq!(decoded_extra.get(&0b0010), Some(&456));
        assert_eq!(decoded_extra.get(&0b0100), Some(&77));
        assert_eq!(consumed, encoded.len());
    }

    #[test]
    fn test_enc_dec_hex() {
        let num = 255;
        let encoded = encode_number_as_hex(num);
        assert_eq!(encoded, vec![2, b'F', b'F']);

        let (val, consumed) = decode_hex_number(&encoded).unwrap();
        assert_eq!(val, 255);
        assert_eq!(consumed, 3);
    }

    #[test]
    fn test_align_block() {
        let data = vec![1, 2, 3];
        let aligned = align_block(&data, 4, 0);
        assert_eq!(aligned, vec![1, 2, 3, 0]);

        let data2 = vec![1, 2, 3, 4];
        let aligned2 = align_block(&data2, 4, 0);
        assert_eq!(aligned2, vec![1, 2, 3, 4]);
    }

    fn build_font_record(payload: &[u8], compress: bool, obfuscate: bool) -> Vec<u8> {
        let mut body = payload.to_vec();
        let usize_ = body.len() as u32;
        let mut flags: u32 = 0;
        if compress {
            flags |= 0b1;
            let mut enc =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(&body).unwrap();
            body = enc.finish().unwrap();
        }
        let key: Vec<u8> = vec![0x5A, 0x3C, 0x91, 0x7E];
        if obfuscate {
            flags |= 0b10;
            let extent = body.len().min(DEFAULT_FONT_XOR_EXTENT);
            for (i, b) in body.iter_mut().enumerate().take(extent) {
                *b ^= key[i % key.len()];
            }
        }
        let dstart = 24u32;
        let xor_start = dstart + body.len() as u32; // placed after payload
        let mut rec = Vec::new();
        rec.extend_from_slice(b"FONT");
        rec.extend_from_slice(&usize_.to_be_bytes());
        rec.extend_from_slice(&flags.to_be_bytes());
        rec.extend_from_slice(&dstart.to_be_bytes());
        rec.extend_from_slice(&(key.len() as u32).to_be_bytes());
        rec.extend_from_slice(&xor_start.to_be_bytes());
        rec.extend_from_slice(&body);
        if obfuscate {
            // Only present (and only read) when the obfuscation flag
            // is set; otherwise it would be read as trailing payload.
            rec.extend_from_slice(&key);
        }
        rec
    }

    #[test]
    fn read_font_record_round_trips_a_plain_ttf() {
        let mut payload = b"\x00\x01\x00\x00".to_vec();
        payload.extend_from_slice(b"rest of a fake sfnt font table directory here");
        let rec = build_font_record(&payload, false, false);
        let info = read_font_record(&rec, DEFAULT_FONT_XOR_EXTENT);
        assert!(info.err.is_none(), "{:?}", info.err);
        assert_eq!(info.ext, "ttf");
        assert!(!info.encrypted);
        assert_eq!(info.font_data.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn read_font_record_deobfuscates_and_decompresses_an_otf() {
        let mut payload = b"OTTO".to_vec();
        payload.extend_from_slice(&[7u8; 200]);
        let rec = build_font_record(&payload, true, true);
        let info = read_font_record(&rec, DEFAULT_FONT_XOR_EXTENT);
        assert!(info.err.is_none(), "{:?}", info.err);
        assert_eq!(info.ext, "otf");
        assert!(info.encrypted);
        assert_eq!(info.font_data.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn read_font_record_reports_a_truncated_header() {
        let info = read_font_record(b"FONT", DEFAULT_FONT_XOR_EXTENT);
        assert!(info.err.is_some());
        assert_eq!(info.ext, "failed");
    }

    #[test]
    fn read_font_record_falls_back_to_dat_for_unknown_signatures() {
        let payload = b"not a real font signature at all, just bytes".to_vec();
        let rec = build_font_record(&payload, false, false);
        let info = read_font_record(&rec, DEFAULT_FONT_XOR_EXTENT);
        assert_eq!(info.ext, "dat");
    }
}
