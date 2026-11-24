//! Port of `old_src/src/calibre/utils/wmf/parse.py`: a WMF (Windows
//! Metafile) record-stream parser, scoped to what [`wmf_unwrap`]
//! needs -- extracting the largest embedded raster image and
//! converting it to PNG.
//!
//! # Scope
//!
//! Real: [`Wmf::parse`] walks every record in the stream (so a
//! malformed/truncated record stops parsing at the right point,
//! exactly like upstream), but only `SetMapMode`/`SetWindowOrg`/
//! `SetWindowExt`/`DibStretchBlt` have real handlers -- matching
//! upstream's own `WMF` class, which defines handlers for only those
//! four record types and silently skips every other opcode in the
//! `function_map` table (all ~90 of them).
//!
//! One narrow, disclosed deviation from a literal port:
//! `SetWindowOrg`/`SetWindowExt`'s dead `len(params) == 16` branch
//! (which would actually raise a `struct.error` in Python, since
//! `struct.unpack('<LL', ...)` only accepts an 8-byte buffer -- an
//! upstream bug in code path that's never hit by any real WMF file,
//! since those records are always 4 or 8 bytes long) is not
//! reproduced; unsupported lengths just log a warning, matching the
//! *other* two length-mismatch cases in the same upstream function.

use log::warn;

use super::dib::{create_bmp_from_dib, bmp_to_png, DibError};

const OP_SET_MAP_MODE: u16 = 259;
const OP_SET_WINDOW_ORG: u16 = 523;
const OP_SET_WINDOW_EXT: u16 = 524;
const OP_DIB_STRETCH_BLT: u16 = 2881;

#[derive(Debug, thiserror::Error)]
pub enum WmfError {
    #[error("Not a WMF file")]
    NotAWmfFile,
    #[error("WMF file header specifies incorrect file size")]
    BadFileSize,
    #[error("WMF data is truncated")]
    Truncated,
    #[error("WMF record has a zero-length size field")]
    ZeroLengthRecord,
    #[error("No raster image found in the WMF")]
    NoRasterImage,
    #[error(transparent)]
    Dib(#[from] DibError),
}

struct WmfHeader {
    records_start_at: usize,
}

impl WmfHeader {
    fn parse(data: &[u8]) -> Result<Self, WmfError> {
        if data.len() < 12 {
            return Err(WmfError::Truncated);
        }
        let header_size = u16::from_le_bytes(data[2..4].try_into().unwrap());
        if header_size != 9 {
            return Err(WmfError::NotAWmfFile);
        }
        let file_size = u32::from_le_bytes(data[6..10].try_into().unwrap());
        if (file_size as usize) * 2 != data.len() {
            return Err(WmfError::BadFileSize);
        }
        Ok(WmfHeader { records_start_at: (header_size as usize) * 2 })
    }
}

/// Port of the `WMF` class.
#[derive(Debug, Default)]
pub struct Wmf {
    pub bitmaps: Vec<Vec<u8>>,
    pub map_mode: Option<u16>,
    pub window_origin: Option<(i64, i64)>,
    pub window_extent: Option<(i64, i64)>,
}

impl Wmf {
    pub fn parse(data: &[u8]) -> Result<Self, WmfError> {
        let header = WmfHeader::parse(data)?;
        let mut w = Wmf::default();

        let mut offset = header.records_start_at;
        while data.len() >= 6 && offset < data.len() - 6 {
            let size = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize * 2;
            let func = u16::from_le_bytes(data[offset + 4..offset + 6].try_into().unwrap());
            if size < 6 {
                // A well-formed record is always at least as large as
                // its own 6-byte header; upstream would spin forever
                // here (`offset` never advances) rather than error, so
                // this is a deliberate, disclosed safety deviation.
                return Err(WmfError::ZeroLengthRecord);
            }
            offset += 6;
            let delta = size - 6;
            let end = (offset + delta).min(data.len());
            let params = &data[offset..end];
            offset += delta;

            match func {
                OP_SET_MAP_MODE => w.handle_set_map_mode(params),
                OP_SET_WINDOW_ORG => w.window_origin = parse_point(params, "SetWindowOrg"),
                OP_SET_WINDOW_EXT => w.window_extent = parse_point(params, "SetWindowExt"),
                OP_DIB_STRETCH_BLT => w.handle_dib_stretch_blt(params)?,
                _ => {}
            }
        }

        Ok(w)
    }

    fn handle_set_map_mode(&mut self, params: &[u8]) {
        if params.len() == 2 {
            self.map_mode = Some(u16::from_le_bytes(params.try_into().unwrap()));
        } else {
            warn!("Invalid SetMapMode param");
        }
    }

    fn handle_dib_stretch_blt(&mut self, raw: &[u8]) -> Result<(), WmfError> {
        // fmt '<IHHHHHHHH': raster_op, src_height, src_width, y_src,
        // x_src, dest_height, dest_width, y_dest, x_dest.
        if raw.len() < 20 {
            return Err(WmfError::Truncated);
        }
        let bmp_data = &raw[20..];
        self.bitmaps.push(create_bmp_from_dib(bmp_data)?);
        Ok(())
    }

    pub fn has_raster_image(&self) -> bool {
        !self.bitmaps.is_empty()
    }

    /// Port of `WMF.to_png`: converts the *largest* embedded bitmap.
    pub fn to_png(&self) -> Result<Vec<u8>, WmfError> {
        let bmp = self.bitmaps.iter().max_by_key(|b| b.len()).ok_or(WmfError::NoRasterImage)?;
        Ok(bmp_to_png(bmp)?)
    }
}

fn parse_point(params: &[u8], record_name: &str) -> Option<(i64, i64)> {
    match params.len() {
        4 => {
            let a = i16::from_le_bytes(params[0..2].try_into().unwrap());
            let b = i16::from_le_bytes(params[2..4].try_into().unwrap());
            Some((a as i64, b as i64))
        }
        8 => {
            let a = i32::from_le_bytes(params[0..4].try_into().unwrap());
            let b = i32::from_le_bytes(params[4..8].try_into().unwrap());
            Some((a as i64, b as i64))
        }
        _ => {
            warn!("Invalid {record_name} param");
            None
        }
    }
}

/// Port of `wmf_unwrap`: returns the largest embedded raster image in
/// the WMF, as PNG data.
pub fn wmf_unwrap(wmf_data: &[u8]) -> Result<Vec<u8>, WmfError> {
    let w = Wmf::parse(wmf_data)?;
    if !w.has_raster_image() {
        return Err(WmfError::NoRasterImage);
    }
    w.to_png()
}

#[cfg(test)]
mod tests {
    use super::super::dib::test_fixtures::make_24bpp_dib;
    use super::*;

    fn wmf_header(records_len: usize) -> Vec<u8> {
        let total_len = 18 + records_len;
        let mut h = Vec::with_capacity(18);
        h.extend_from_slice(&1u16.to_le_bytes()); // file_type
        h.extend_from_slice(&9u16.to_le_bytes()); // header_size (WORDs)
        h.extend_from_slice(&0x300u16.to_le_bytes()); // windows_version
        h.extend_from_slice(&((total_len / 2) as u32).to_le_bytes()); // file_size (WORDs)
        h.extend_from_slice(&1u16.to_le_bytes()); // num_of_objects
        h.extend_from_slice(&[0u8; 6]); // unused tail of the 18-byte standard header
        h
    }

    fn dib_stretch_blt_record(dib: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&0u32.to_le_bytes()); // raster_op
        for _ in 0..8 {
            body.extend_from_slice(&0u16.to_le_bytes()); // src/dest dims, all zero (unused by the port)
        }
        body.extend_from_slice(dib);

        let record_len = 6 + body.len();
        assert_eq!(record_len % 2, 0);
        let mut record = Vec::with_capacity(record_len);
        record.extend_from_slice(&((record_len / 2) as u32).to_le_bytes());
        record.extend_from_slice(&OP_DIB_STRETCH_BLT.to_le_bytes());
        record.extend_from_slice(&body);
        record
    }

    #[test]
    fn extracts_the_embedded_bitmap_as_png() {
        let dib = make_24bpp_dib(2, 2);
        let record = dib_stretch_blt_record(&dib);
        let mut file = wmf_header(record.len());
        file.extend_from_slice(&record);

        let png = wmf_unwrap(&file).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }

    #[test]
    fn rejects_a_non_wmf_header() {
        let data = vec![0u8; 20];
        assert!(matches!(Wmf::parse(&data), Err(WmfError::NotAWmfFile)));
    }

    #[test]
    fn errors_when_there_is_no_raster_image() {
        let file = wmf_header(0);
        assert!(matches!(wmf_unwrap(&file), Err(WmfError::NoRasterImage)));
    }

    #[test]
    fn picks_the_largest_bitmap_when_there_are_several() {
        let small = make_24bpp_dib(1, 1);
        let large = make_24bpp_dib(4, 4);
        let r1 = dib_stretch_blt_record(&small);
        let r2 = dib_stretch_blt_record(&large);
        let mut file = wmf_header(r1.len() + r2.len());
        file.extend_from_slice(&r1);
        file.extend_from_slice(&r2);

        let png = wmf_unwrap(&file).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(decoded.width(), 4);
        assert_eq!(decoded.height(), 4);
    }
}
