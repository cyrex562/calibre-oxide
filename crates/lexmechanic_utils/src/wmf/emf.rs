//! Port of `old_src/src/calibre/utils/wmf/emf.py`: an EMF (Enhanced
//! Metafile) record-stream parser, scoped the same way [`super::parse`]
//! is -- extracting the largest embedded raster image and converting
//! it to PNG. See
//! <https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-emf/>
//! for the record format.
//!
//! Real: every record is walked (`EMR_STRETCHDIBITS` handled, every
//! other opcode skipped, `EMR_EOF` stops parsing), matching upstream's
//! own `func_map` dispatch, which only defines handlers for
//! `stretchdibits`/`header`/`eof` and routes everything else to
//! `handle_unknown` (a no-op besides an optional debug print).
//!
//! One disclosed deviation: a record with a `size` field of zero would
//! spin upstream's `while self.pos < len(raw)` loop forever (`self.pos`
//! never advances). This port treats that as
//! [`EmfError::ZeroLengthRecord`] instead of hanging.

use super::dib::{create_bmp_from_dib, bmp_to_png, DibError};

const EMR_STRETCHDIBITS: u32 = 0x51;
const EMR_EOF: u32 = 0xe;

#[derive(Debug, thiserror::Error)]
pub enum EmfError {
    #[error("EMF data is truncated")]
    Truncated,
    #[error("EMF record has a zero-length size field")]
    ZeroLengthRecord,
    #[error("No raster image found in the EMF")]
    NoRasterImage,
    #[error(transparent)]
    Dib(#[from] DibError),
}

/// Port of the `EMF` class.
#[derive(Debug, Default)]
pub struct Emf {
    pub bitmaps: Vec<Vec<u8>>,
    pub found_eof: bool,
}

impl Emf {
    pub fn parse(raw: &[u8]) -> Result<Self, EmfError> {
        let mut e = Emf::default();
        let mut pos = 0usize;
        while pos < raw.len() && !e.found_eof {
            if pos + 8 > raw.len() {
                return Err(EmfError::Truncated);
            }
            let rtype = u32::from_le_bytes(raw[pos..pos + 4].try_into().unwrap());
            let size = u32::from_le_bytes(raw[pos + 4..pos + 8].try_into().unwrap()) as usize;
            if size == 0 {
                return Err(EmfError::ZeroLengthRecord);
            }
            let end = (pos + size).min(raw.len());
            let record = &raw[pos..end];
            pos += size;

            match rtype {
                EMR_STRETCHDIBITS => e.handle_stretchdibits(record)?,
                EMR_EOF => e.found_eof = true,
                _ => {}
            }
        }
        Ok(e)
    }

    fn handle_stretchdibits(&mut self, raw: &[u8]) -> Result<(), EmfError> {
        // StretchDiBits: 18 little-endian u32 fields starting 8 bytes
        // into the record (after its own rtype+size header).
        const FIELDS: usize = 18;
        if raw.len() < 8 + FIELDS * 4 {
            return Err(EmfError::Truncated);
        }
        let field = |i: usize| -> usize {
            let o = 8 + i * 4;
            u32::from_le_bytes(raw[o..o + 4].try_into().unwrap()) as usize
        };
        // Field order: left top right bottom x_dest y_dest x_src
        // y_src cx_src cy_src bmp_hdr_offset bmp_header_size
        // bmp_bits_offset bmp_bits_size usage op dest_width dest_height.
        let bmp_hdr_offset = field(10);
        let bmp_header_size = field(11);
        let bmp_bits_offset = field(12);
        let bmp_bits_size = field(13);

        let hdr = slice_within(raw, bmp_hdr_offset, bmp_header_size);
        let bits = slice_within(raw, bmp_bits_offset, bmp_bits_size);
        let mut combined = Vec::with_capacity(hdr.len() + bits.len());
        combined.extend_from_slice(hdr);
        combined.extend_from_slice(bits);

        self.bitmaps.push(create_bmp_from_dib(&combined)?);
        Ok(())
    }

    pub fn has_raster_image(&self) -> bool {
        !self.bitmaps.is_empty()
    }

    /// Port of `EMF.to_png`: converts the *largest* embedded bitmap.
    pub fn to_png(&self) -> Result<Vec<u8>, EmfError> {
        let bmp = self.bitmaps.iter().max_by_key(|b| b.len()).ok_or(EmfError::NoRasterImage)?;
        Ok(bmp_to_png(bmp)?)
    }
}

/// Matches Python slicing semantics: an out-of-range start/end never
/// panics, it just clips to what's available (or returns empty).
fn slice_within(data: &[u8], start: usize, len: usize) -> &[u8] {
    if start >= data.len() {
        return &[];
    }
    let end = (start + len).min(data.len());
    &data[start..end]
}

/// Port of `emf_unwrap`: returns the largest embedded raster image in
/// the EMF, as PNG data.
pub fn emf_unwrap(raw: &[u8]) -> Result<Vec<u8>, EmfError> {
    let e = Emf::parse(raw)?;
    if !e.has_raster_image() {
        return Err(EmfError::NoRasterImage);
    }
    e.to_png()
}

#[cfg(test)]
mod tests {
    use super::super::dib::test_fixtures::make_24bpp_dib;
    use super::*;

    fn stretchdibits_record(dib_header: &[u8], dib_bits: &[u8]) -> Vec<u8> {
        let fixed_len = 8 + 18 * 4;
        let hdr_offset = fixed_len as u32;
        let bits_offset = hdr_offset + dib_header.len() as u32;
        let total_len = bits_offset as usize + dib_bits.len();

        let mut record = Vec::with_capacity(total_len);
        record.extend_from_slice(&EMR_STRETCHDIBITS.to_le_bytes());
        record.extend_from_slice(&(total_len as u32).to_le_bytes());

        let mut fields = [0u32; 18];
        fields[10] = hdr_offset; // bmp_hdr_offset
        fields[11] = dib_header.len() as u32; // bmp_header_size
        fields[12] = bits_offset; // bmp_bits_offset
        fields[13] = dib_bits.len() as u32; // bmp_bits_size
        for f in fields {
            record.extend_from_slice(&f.to_le_bytes());
        }
        record.extend_from_slice(dib_header);
        record.extend_from_slice(dib_bits);
        record
    }

    fn eof_record() -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&EMR_EOF.to_le_bytes());
        r.extend_from_slice(&8u32.to_le_bytes());
        r
    }

    #[test]
    fn extracts_the_embedded_bitmap_as_png() {
        let dib = make_24bpp_dib(3, 3);
        let (header, bits) = dib.split_at(40);
        let mut file = stretchdibits_record(header, bits);
        file.extend_from_slice(&eof_record());

        let png = emf_unwrap(&file).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(decoded.width(), 3);
        assert_eq!(decoded.height(), 3);
    }

    #[test]
    fn errors_when_there_is_no_raster_image() {
        let file = eof_record();
        assert!(matches!(emf_unwrap(&file), Err(EmfError::NoRasterImage)));
    }

    #[test]
    fn stops_at_a_zero_length_record_instead_of_hanging() {
        let mut file = Vec::new();
        file.extend_from_slice(&0x99u32.to_le_bytes());
        file.extend_from_slice(&0u32.to_le_bytes());
        assert!(matches!(Emf::parse(&file), Err(EmfError::ZeroLengthRecord)));
    }
}
