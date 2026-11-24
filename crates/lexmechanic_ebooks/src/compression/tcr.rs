//! TCR decompression.
//!
//! Port of `old_src/src/calibre/ebooks/compression/tcr.py`'s `decompress`.
//!
//! The format is a dictionary substitution: the header `!!8-Bit!!`, then 256
//! length-prefixed byte strings, then a body in which every byte is an index
//! into that dictionary. Decoding is a lookup per byte.
//!
//! Only decompression is ported. Upstream's `TCRCompressor` builds the
//! dictionary by searching for code pairs that always co-occur, which is
//! real work with no caller here -- nothing in this project writes TCR.

use thiserror::Error;

pub const MAGIC: &[u8; 9] = b"!!8-Bit!!";

#[derive(Debug, Error)]
pub enum TcrError {
    #[error("not a TCR file: expected the header {:?}", String::from_utf8_lossy(MAGIC))]
    BadHeader,
    #[error("the file ends inside its code dictionary")]
    TruncatedDictionary,
}

/// Decompresses a TCR file's bytes.
///
/// A body byte always indexes a dictionary of exactly 256 entries, so no
/// index can be out of range -- which is why this cannot fail past the
/// header and dictionary.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, TcrError> {
    if data.len() < MAGIC.len() || &data[..MAGIC.len()] != MAGIC {
        return Err(TcrError::BadHeader);
    }

    let mut at = MAGIC.len();
    let mut entries: Vec<&[u8]> = Vec::with_capacity(256);
    for _ in 0..256 {
        let Some(&length) = data.get(at) else {
            return Err(TcrError::TruncatedDictionary);
        };
        at += 1;
        let end = at + length as usize;
        let Some(entry) = data.get(at..end) else {
            return Err(TcrError::TruncatedDictionary);
        };
        entries.push(entry);
        at = end;
    }

    let mut out = Vec::new();
    for &index in &data[at..] {
        out.extend_from_slice(entries[index as usize]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a TCR file whose dictionary maps the given entries and whose
    /// body is the given indices.
    fn build(entries: &[&[u8]], body: &[u8]) -> Vec<u8> {
        let mut out = MAGIC.to_vec();
        for i in 0..256 {
            let entry: &[u8] = entries.get(i).copied().unwrap_or(b"");
            out.push(entry.len() as u8);
            out.extend_from_slice(entry);
        }
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn decompresses_a_dictionary_substitution() {
        // 0 -> "Hello ", 1 -> "world", 2 -> "!"
        let file = build(&[b"Hello ", b"world", b"!"], &[0, 1, 2]);
        assert_eq!(decompress(&file).unwrap(), b"Hello world!");
    }

    /// A repeated index expands every time -- that is the whole point of
    /// the format.
    #[test]
    fn a_repeated_code_expands_each_time() {
        let file = build(&[b"ab"], &[0, 0, 0]);
        assert_eq!(decompress(&file).unwrap(), b"ababab");
    }

    #[test]
    fn an_empty_body_decompresses_to_nothing() {
        let file = build(&[b"unused"], &[]);
        assert_eq!(decompress(&file).unwrap(), b"");
    }

    /// Every one of the 256 indices is addressable, including the last.
    #[test]
    fn the_highest_index_is_usable() {
        let mut entries: Vec<&[u8]> = vec![b""; 256];
        entries[255] = b"last";
        let file = build(&entries, &[255]);
        assert_eq!(decompress(&file).unwrap(), b"last");
    }

    #[test]
    fn a_file_without_the_header_is_refused() {
        assert!(matches!(decompress(b"not a TCR file at all").unwrap_err(), TcrError::BadHeader));
        assert!(matches!(decompress(b"").unwrap_err(), TcrError::BadHeader));
        // The right length, the wrong bytes.
        assert!(matches!(decompress(b"!!7-Bit!!").unwrap_err(), TcrError::BadHeader));
    }

    /// A file that ends inside the dictionary is refused rather than
    /// indexing past its end.
    #[test]
    fn a_truncated_dictionary_is_refused() {
        let mut truncated = MAGIC.to_vec();
        truncated.push(4); // claims a 4-byte entry
        truncated.extend_from_slice(b"ab"); // supplies 2
        assert!(matches!(decompress(&truncated).unwrap_err(), TcrError::TruncatedDictionary));

        // And one that stops partway through the 256 length bytes.
        let mut short = MAGIC.to_vec();
        short.push(0);
        assert!(matches!(decompress(&short).unwrap_err(), TcrError::TruncatedDictionary));
    }
}
