/// Recognize image headers
/// Ported from calibre/utils/imghdr.py

pub fn what(data: &[u8]) -> Option<&'static str> {
    if data.len() < 32 {
        // Minimal check for very short data (though some sigs are short)
        if data.len() >= 2 && data[0] == 0xff && data[1] == 0xd8 {
             return Some("jpeg");
        }
        // Let checks proceed if len is enough for them
    }

    if is_jpeg(data) { return Some("jpeg"); }
    if is_png(data) { return Some("png"); }
    if is_gif(data) { return Some("gif"); }
    if is_tiff(data) { 
        // Check for jxr inside tiff
        if data.len() >= 4 && data[0] == b'I' && data[1] == b'I' && data[2] == 0xbc && data[3] == 0x01 {
            return Some("jxr");
        }
        return Some("tiff"); 
    }
    if is_webp(data) { return Some("webp"); }
    if is_bmp(data) { return Some("bmp"); }
    if is_jpeg2000(data) { return Some("jpeg2000"); }
    // ... add others as needed (pbm, pgm, ppm, ras, xbm, emf, svg)
    // SVG is text-based but has known headers.
    
    // Fallback JPEG check
    if data.len() >= 2 && data[0] == 0xff && data[1] == 0xd8 {
        return Some("jpeg");
    }
    
    None
}

fn is_jpeg(h: &[u8]) -> bool {
    if h.len() < 10 { return false; }
    if &h[6..10] == b"JFIF" || &h[6..10] == b"Exif" {
        return true;
    }
    if h[0] == 0xff && h[1] == 0xd8 {
        // Check first 32 bytes for JFIF or 8BIM
        let limit = std::cmp::min(h.len(), 32);
        let prefix = &h[0..limit];
        // naive byte search
        if prefix.windows(4).any(|w| w == b"JFIF" || w == b"8BIM") {
            return true;
        }
    }
    false
}

fn is_png(h: &[u8]) -> bool {
    h.len() >= 8 && &h[0..8] == b"\x89PNG\r\n\x1a\n"
}

fn is_gif(h: &[u8]) -> bool {
    h.len() >= 6 && (&h[0..6] == b"GIF87a" || &h[0..6] == b"GIF89a")
}

fn is_tiff(h: &[u8]) -> bool {
    h.len() >= 2 && (&h[0..2] == b"MM" || &h[0..2] == b"II")
}

fn is_webp(h: &[u8]) -> bool {
    h.len() >= 12 && &h[0..4] == b"RIFF" && &h[8..12] == b"WEBP"
}

fn is_bmp(h: &[u8]) -> bool {
    h.len() >= 2 && &h[0..2] == b"BM"
}

fn is_jpeg2000(h: &[u8]) -> bool {
    h.len() >= 12 && &h[0..12] == b"\x00\x00\x00\x0cjP  \r\n\x87\n"
}

/// Number of header bytes `identify` inspects, matching Python's `HSIZE`.
const HSIZE: usize = 120;

/// Port of `calibre.utils.imghdr.identify`: recognizes `data`'s image
/// format and, for jpeg/png/gif/jpeg2000, also its pixel dimensions.
/// Returns `(format, width, height)`; `format` is `None` if the header is
/// not recognized, `width`/`height` are `-1` if the format was
/// recognized but its dimensions could not be read (matching Python's
/// documented return convention exactly). Unlike Python, which takes an
/// open, seekable stream, this takes the whole image as a byte slice --
/// every real call site in this crate (as in `cover.py`'s
/// `create_epub_cover`) already has the full file in memory.
pub fn identify(data: &[u8]) -> (Option<&'static str>, i64, i64) {
    let head = &data[..data.len().min(HSIZE)];
    let fmt = what(head);
    let mut width: i64 = -1;
    let mut height: i64 = -1;
    match fmt {
        Some("png") => {
            let s = if head.len() >= 24 && head.get(12..16) == Some(b"IHDR".as_slice()) {
                head.get(16..24)
            } else {
                head.get(8..16)
            };
            if let Some(s) = s {
                width = u32::from_be_bytes(s[0..4].try_into().unwrap()) as i64;
                height = u32::from_be_bytes(s[4..8].try_into().unwrap()) as i64;
            }
        }
        Some("jpeg") => {
            if let Some((h, w)) = jpeg_dimensions(data) {
                height = h;
                width = w;
            }
        }
        Some("gif") => {
            if head.len() >= 10 {
                width = u16::from_le_bytes([head[6], head[7]]) as i64;
                height = u16::from_le_bytes([head[8], head[9]]) as i64;
            }
        }
        Some("jpeg2000") if head.len() >= 56 => {
            height = u32::from_be_bytes(head[48..52].try_into().unwrap()) as i64;
            width = u32::from_be_bytes(head[52..56].try_into().unwrap()) as i64;
        }
        _ => {}
    }
    (fmt, width, height)
}

fn read_byte(data: &[u8], pos: &mut usize) -> Option<u8> {
    let b = *data.get(*pos)?;
    *pos += 1;
    Some(b)
}

/// Port of `imghdr.jpeg_dimensions`: scans JPEG markers starting just
/// past the initial `0xffd8` SOI marker (matching Python's
/// `stream.seek(2, SEEK_CUR)`) for the first `SOFn` marker, returning
/// `(height, width)`. `None` means the data was truncated before a
/// marker could be fully read (Python's `ValueError('Truncated JPEG
/// data')`); `Some((-1, -1))` means a corrupted JPEG (a `0x00` marker) or
/// that scanning ran off the end of the segment table without finding an
/// `SOFn` marker.
fn jpeg_dimensions(data: &[u8]) -> Option<(i64, i64)> {
    let mut pos: usize = 2;
    loop {
        // Find the next 0xff byte (a marker prefix).
        loop {
            if read_byte(data, &mut pos)? == 0xff {
                break;
            }
        }
        // Soak up any padding (repeated 0xff bytes).
        let mut marker = 0xffu8;
        while marker == 0xff {
            marker = read_byte(data, &mut pos)?;
        }
        let q = marker;
        if (0xc0..=0xcf).contains(&q) && q != 0xc4 && q != 0xcc {
            // SOFn marker: 2 bytes segment length + 1 byte precision,
            // then 2 bytes height + 2 bytes width.
            pos += 3;
            if pos + 4 > data.len() {
                return None;
            }
            let h = u16::from_be_bytes([data[pos], data[pos + 1]]);
            let w = u16::from_be_bytes([data[pos + 2], data[pos + 3]]);
            return Some((h as i64, w as i64));
        } else if (0xd8..=0xda).contains(&q) {
            // SOI/EOI/SOS: no point continuing.
            return Some((-1, -1));
        } else if q == 0 {
            return Some((-1, -1)); // Corrupted JPEG.
        } else if q == 0x01 || (0xd0..=0xd7).contains(&q) {
            // Standalone marker, keep scanning.
            continue;
        } else {
            if pos + 2 > data.len() {
                return None;
            }
            let size = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
            pos += 2;
            if size < 2 {
                return None;
            }
            pos += size - 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identify_png_reads_dimensions() {
        let mut data = b"\x89PNG\r\n\x1a\n".to_vec();
        data.extend_from_slice(&[0, 0, 0, 13]); // IHDR chunk length
        data.extend_from_slice(b"IHDR");
        data.extend_from_slice(&100u32.to_be_bytes()); // width
        data.extend_from_slice(&200u32.to_be_bytes()); // height
        let (fmt, w, h) = identify(&data);
        assert_eq!(fmt, Some("png"));
        assert_eq!((w, h), (100, 200));
    }

    #[test]
    fn identify_gif_reads_dimensions() {
        let mut data = b"GIF89a".to_vec();
        data.extend_from_slice(&300u16.to_le_bytes()); // width
        data.extend_from_slice(&150u16.to_le_bytes()); // height
        let (fmt, w, h) = identify(&data);
        assert_eq!(fmt, Some("gif"));
        assert_eq!((w, h), (300, 150));
    }

    #[test]
    fn identify_jpeg_reads_dimensions_from_sof0() {
        // SOI, then an APP0/JFIF marker (skipped), then SOF0 with
        // precision=8, height=480, width=640.
        let mut data = vec![0xff, 0xd8];
        data.extend_from_slice(&[0xff, 0xe0]); // APP0
        data.extend_from_slice(&[0, 16]); // segment length (including these 2 bytes)
        data.extend_from_slice(b"JFIF\0");
        data.extend_from_slice(&[1, 1, 0, 0, 1, 0, 1, 0, 0]);
        data.extend_from_slice(&[0xff, 0xc0]); // SOF0
        data.extend_from_slice(&[0, 17]); // segment length (unused by our scan)
        data.push(8); // precision
        data.extend_from_slice(&480u16.to_be_bytes()); // height
        data.extend_from_slice(&640u16.to_be_bytes()); // width
        let (fmt, w, h) = identify(&data);
        assert_eq!(fmt, Some("jpeg"));
        assert_eq!((w, h), (640, 480));
    }

    #[test]
    fn identify_unrecognized_format_returns_none_and_negative_dims() {
        let (fmt, w, h) = identify(b"not an image");
        assert_eq!(fmt, None);
        assert_eq!((w, h), (-1, -1));
    }
}
