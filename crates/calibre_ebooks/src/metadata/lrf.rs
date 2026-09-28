use crate::metadata::MetaInformation;
use anyhow::Result;
use std::io::{Read, Seek};

/// LRF metadata: not parsed.
///
/// Real LRF metadata is a zlib-compressed XML block (`<BookInfo><Title>`)
/// behind a fixed binary header -- `compressed_info_size` at 0x4c,
/// `uncompressed_info_size` at 0x54. Portable, but LRF is a discontinued
/// Sony format and a full parser is disproportionate to its remaining
/// users. See `old_src/.../lrf/meta.py`'s `LRFMetaFile` for the layout if
/// that changes.
///
/// What this used to do was worse than not parsing: it returned
/// `title = "Unknown LRF"` and `authors = ["Unknown"]` while ignoring the
/// file entirely, and returned `Ok`. So adding a `.lrf` to a library gave
/// it a **fabricated title that looked real**, and nothing indicated the
/// metadata had not been read.
///
/// Returning the plain default instead means `title` is the `"Unknown"`
/// placeholder every reader uses for "not known" -- which
/// `calibre_db::scan`'s indexing already recognises, falling back to the
/// filename. A user sees their file's name rather than an invention.
pub fn get_metadata<R: Read + Seek>(_stream: R) -> Result<MetaInformation> {
    Ok(MetaInformation::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The point of the change: no invented title.
    #[test]
    fn no_fabricated_title_is_returned() {
        let mi = get_metadata(Cursor::new(b"LRF bytes we do not parse".to_vec())).unwrap();
        assert_eq!(mi.title, "Unknown", "the title must be the standard placeholder, not an invention");
        assert_ne!(mi.title, "Unknown LRF", "this was the fabricated value");
    }

    /// And the placeholder is one the indexer recognises, so the filename
    /// is used instead -- the behaviour that makes this an improvement
    /// rather than just a different string.
    #[test]
    fn the_title_is_the_placeholder_the_indexer_treats_as_absent() {
        let mi = get_metadata(Cursor::new(Vec::new())).unwrap();
        // `calibre_db::scan::index_scan` checks exactly this.
        assert!(mi.title.trim().is_empty() || mi.title == "Unknown");
    }
}
