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
