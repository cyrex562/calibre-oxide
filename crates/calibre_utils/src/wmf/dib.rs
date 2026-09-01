//! Port of `old_src/src/calibre/utils/wmf/__init__.py`: raw DIB
//! (Device-Independent Bitmap, the pixel format WMF/EMF metafiles
//! embed) handling shared by both the WMF and EMF parsers.

use std::io::Cursor;

#[derive(Debug, thiserror::Error)]
pub enum DibError {
    #[error("DIB data too short to contain a header")]
    Truncated,
    #[error("Unsupported DIB header type of size: {0}")]
    UnsupportedHeaderSize(u32),
    #[error("failed to decode/encode image: {0}")]
    Image(#[from] image::ImageError),
}

/// Port of `DIBHeader`. See <http://en.wikipedia.org/wiki/BMP_file_format>.
#[derive(Debug, Clone, Copy)]
pub struct DibHeader {
    pub header_size: u32,
    pub width: i64,
    pub height: i64,
    pub color_planes: u16,
    pub bits_per_pixel: u16,
    pub compression: u32,
    pub bitmasks_size: u32,
    pub color_table_size: u32,
}

impl DibHeader {
    pub fn parse(raw: &[u8]) -> Result<Self, DibError> {
        if raw.len() < 4 {
            return Err(DibError::Truncated);
        }
        let header_size = u32::from_le_bytes(raw[0..4].try_into().unwrap());
        let (width, height, color_planes, bits_per_pixel, compression, ncols) = match header_size {
            // BITMAPINFOHEADER: <IiiHHIIIIIIatop 40 bytes.
            40 => {
                if raw.len() < 40 {
                    return Err(DibError::Truncated);
                }
                let width = i32::from_le_bytes(raw[4..8].try_into().unwrap()) as i64;
                let height = i32::from_le_bytes(raw[8..12].try_into().unwrap()) as i64;
                let color_planes = u16::from_le_bytes(raw[12..14].try_into().unwrap());
                let bits_per_pixel = u16::from_le_bytes(raw[14..16].try_into().unwrap());
                let compression = u32::from_le_bytes(raw[16..20].try_into().unwrap());
                let ncols = u32::from_le_bytes(raw[32..36].try_into().unwrap());
                (width, height, color_planes, bits_per_pixel, compression, ncols)
            }
            // BITMAPCOREHEADER: <IHHHH, 12 bytes.
            12 => {
                if raw.len() < 12 {
                    return Err(DibError::Truncated);
                }
                let width = u16::from_le_bytes(raw[4..6].try_into().unwrap()) as i64;
                let height = u16::from_le_bytes(raw[6..8].try_into().unwrap()) as i64;
                let color_planes = u16::from_le_bytes(raw[8..10].try_into().unwrap());
                let bits_per_pixel = u16::from_le_bytes(raw[10..12].try_into().unwrap());
                (width, height, color_planes, bits_per_pixel, 0, 0)
            }
            other => return Err(DibError::UnsupportedHeaderSize(other)),
        };

        let bitmasks_size = if compression == 3 { 12 } else { 0 };
        // See http://support.microsoft.com/kb/q81498/ for the gory details.
        let color_table_size = if bits_per_pixel != 24 { ncols * 4 } else { 0 };

        Ok(DibHeader { header_size, width, height, color_planes, bits_per_pixel, compression, bitmasks_size, color_table_size })
    }
}

/// Port of `create_bmp_from_dib`: prepends a 14-byte `BITMAPFILEHEADER`
/// to a raw DIB blob, producing a standalone `.bmp` file.
pub fn create_bmp_from_dib(raw: &[u8]) -> Result<Vec<u8>, DibError> {
    let dh = DibHeader::parse(raw)?;
    let size = raw.len() as u32 + 14;
    let pixel_array_offset = dh.header_size + dh.bitmasks_size + dh.color_table_size;

    let mut out = Vec::with_capacity(14 + raw.len());
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&pixel_array_offset.to_le_bytes());
    out.extend_from_slice(raw);
    Ok(out)
}

/// Port of `to_png`: decodes a `.bmp` file and re-encodes it as PNG.
/// Upstream does this via Qt (`QImage`/`QBuffer`); this uses the
/// `image` crate instead, which needs no GUI toolkit.
pub fn bmp_to_png(bmp: &[u8]) -> Result<Vec<u8>, DibError> {
    let img = image::load_from_memory_with_format(bmp, image::ImageFormat::Bmp)?;
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageOutputFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
pub(crate) mod test_fixtures {
    /// Builds a minimal uncompressed 24bpp BITMAPINFOHEADER DIB blob
    /// (header + row-padded pixel data) for a `width`x`height` image,
    /// for use as a raster payload embedded in a synthetic WMF/EMF
    /// test fixture.
    pub fn make_24bpp_dib(width: u32, height: u32) -> Vec<u8> {
        let row_bytes = (width * 3).div_ceil(4) * 4;
        let pixel_data_size = row_bytes * height;

        let mut dib = Vec::with_capacity(40 + pixel_data_size as usize);
        dib.extend_from_slice(&40u32.to_le_bytes()); // header_size
        dib.extend_from_slice(&(width as i32).to_le_bytes());
        dib.extend_from_slice(&(height as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes()); // color_planes
        dib.extend_from_slice(&24u16.to_le_bytes()); // bits_per_pixel
        dib.extend_from_slice(&0u32.to_le_bytes()); // compression (BI_RGB)
        dib.extend_from_slice(&pixel_data_size.to_le_bytes()); // image_size
        dib.extend_from_slice(&0u32.to_le_bytes()); // hres
        dib.extend_from_slice(&0u32.to_le_bytes()); // vres
        dib.extend_from_slice(&0u32.to_le_bytes()); // ncols
        dib.extend_from_slice(&0u32.to_le_bytes()); // nimpcols
        for i in 0..pixel_data_size {
            dib.push((i % 256) as u8);
        }
        dib
    }
}

#[cfg(test)]
mod tests {
    use super::test_fixtures::make_24bpp_dib;
    use super::*;

    #[test]
    fn parses_a_bitmapinfoheader() {
        let dib = make_24bpp_dib(2, 2);
        let h = DibHeader::parse(&dib).unwrap();
        assert_eq!(h.header_size, 40);
        assert_eq!(h.width, 2);
        assert_eq!(h.height, 2);
        assert_eq!(h.bits_per_pixel, 24);
        assert_eq!(h.color_table_size, 0, "24bpp images have no color table");
    }

    #[test]
    fn rejects_an_unsupported_header_size() {
        let bad = [7u8, 0, 0, 0];
        assert!(matches!(DibHeader::parse(&bad), Err(DibError::UnsupportedHeaderSize(7))));
    }

    #[test]
    fn wraps_a_dib_into_a_decodable_bmp() {
        let dib = make_24bpp_dib(2, 2);
        let bmp = create_bmp_from_dib(&dib).unwrap();
        assert_eq!(&bmp[0..2], b"BM");
        let png = bmp_to_png(&bmp).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!(decoded.width(), 2);
        assert_eq!(decoded.height(), 2);
    }
}
