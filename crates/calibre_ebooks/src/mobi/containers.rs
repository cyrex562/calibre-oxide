use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Cursor, Read, Seek, SeekFrom};

/// Detects image type from data signature.
/// A simple replacement for `calibre.utils.imghdr.what`.
pub fn find_imgtype(data: &[u8]) -> Option<&str> {
    if data.len() < 4 {
        return None;
    }
    if &data[0..3] == b"\xFF\xD8\xFF" {
        return Some("jpeg");
    }
    if &data[0..4] == b"\x89PNG" {
        return Some("png");
    }
    if &data[0..4] == b"GIF8" {
        return Some("gif");
    }
    // Add more if needed
    None
}

pub struct Container {
    pub is_image_container: bool,
    pub resource_index: usize,
}

impl Container {
    pub fn new(data: &[u8]) -> Self {
        let mut is_image_container = false;

        // Check for EXTH header at offset 48 + 4 (identifier) check?
        // Python: if len(data) > 60 and data[48:52] == b'EXTH':
        // Note: 48 is usually where the identifier starts in PalmDoc if there is no gap?
        // Actually, in MOBI PDB, record 0 usually has PalmDoc (16) + MOBI Header (variable).
        // If this `data` is the MOBI header record, we need to be careful about offsets.
        // However, the python code hardcodes 48:52.
        // Let's assume `data` is what they pass in Python.

        if data.len() > 60 && &data[48..52] == b"EXTH" {
            let mut cursor = Cursor::new(&data[52..]);
            if let (Ok(length), Ok(_num_items)) = (
                cursor.read_u32::<BigEndian>(),
                cursor.read_u32::<BigEndian>(),
            ) {
                // length includes the EXTH header itself (12 bytes usually? Identifier(4)+Len(4)+Count(4))
                // Python loop starts at pos = 60 (which is 48 + 12).
                // 52 + 8 = 60. So we effectively skipped Identifier(48..52), read Len(52..56), Count(56..60).
                // Cursor is now at 60 relative to start of data.

                let start_pos = 60;
                let end_pos = 60 + length as usize - 8; // Python: 60 + length - 8.
                                                        // Wait, if length includes headers, why -8?
                                                        // The loop reads type(4) + len(4) = 8 bytes.

                let mut current_pos = start_pos;
                let mut cursor = Cursor::new(data);
                cursor.set_position(current_pos as u64);

                while current_pos < end_pos {
                    if let (Ok(idx), Ok(size)) = (
                        cursor.read_u32::<BigEndian>(),
                        cursor.read_u32::<BigEndian>(),
                    ) {
                        let size = size as usize;
                        if size < 8 {
                            break;
                        }

                        if idx == 539 {
                            // EXTH 539: Creator Software? No, 539 might be a specific tag.
                            // Check content
                            // Content is at current_pos + 8
                            // Length is size - 8
                            let content_start = current_pos + 8;
                            let content_end = current_pos + size;
                            if content_end <= data.len() {
                                let content = &data[content_start..content_end];
                                if content == b"application/image" {
                                    is_image_container = true;
                                    break;
                                }
                            }
                        }

                        current_pos += size;
                        if let Err(_) = cursor.seek(SeekFrom::Start(current_pos as u64)) {
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }

        Container {
            is_image_container,
            resource_index: 0,
        }
    }

    pub fn load_image<'a>(&mut self, data: &'a [u8]) -> (Option<&'a [u8]>, Option<&'static str>) {
        self.resource_index += 1;
        if self.is_image_container {
            // Python checks check_signature for 'USE ' at start? No, it just does data[12:]
            // Python code: data = data[12:]
            if data.len() > 12 {
                let img_data = &data[12..];
                if let Some(imgtype) = find_imgtype(img_data) {
                    return (
                        Some(img_data),
                        Some(match imgtype {
                            "jpeg" => "jpeg",
                            "png" => "png",
                            "gif" => "gif",
                            _ => "unknown",
                        }),
                    );
                }
            }
        }
        (None, None)
    }
}
