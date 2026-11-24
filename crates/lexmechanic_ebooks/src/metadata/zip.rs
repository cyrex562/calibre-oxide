use crate::metadata::archive::{is_comic, parse_comic_comment};
use crate::metadata::MetaInformation;
use anyhow::{bail, Context, Result};
use std::io::{Cursor, Read, Seek};
use zip::ZipArchive;

pub fn get_metadata<R: Read + Seek>(stream: R) -> Result<MetaInformation> {
    let mut zf = ZipArchive::new(stream).context("Failed to open ZIP archive")?;

    // Collect filenames
    let names: Vec<String> = zf.file_names().map(|s| s.to_string()).collect();

    if is_comic(&names) {
        // Comic content - extract metadata from comment
        let comment = zf.comment().to_vec();
        // Default series index "volume" or "issue"? Python uses 'volume' default sort of?
        // archive.py checks both.
        return parse_comic_comment(&comment, "volume");
    }

    // Not a comic, look for supported ebook formats
    // Priority order mimicking python loop (which is just iterator order)
    // But we might want to prioritize certain extensions? Python iterates namelist order.

    // Extensions to look for
    let extensions = [
        "lit", "opf", "prc", "mobi", "fb2", "epub", "rb", "imp", "pdf", "lrf", "lrx", "azw", "azw3",
    ];

    for i in 0..zf.len() {
        // We need to access file by index to avoid borrow issues if we iterated names?
        // zip crate allows by index.
        let ext = {
            let file = zf.by_index(i)?;
            std::path::Path::new(file.name())
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_lowercase())
                .unwrap_or_default()
        };

        if extensions.contains(&ext.as_str()) {
            // Found a candidate
            let mut file = zf.by_index(i)?;
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            let cursor = Cursor::new(buf);

            match ext.as_str() {
                "epub" => return crate::metadata::epub::get_metadata(cursor),
                "mobi" | "prc" | "azw" | "azw3" => {
                    return crate::metadata::mobi::get_metadata(cursor)
                }
                "fb2" => return crate::metadata::fb2::get_metadata(cursor),
                "lit" => return crate::metadata::lit::get_metadata(cursor),
                "pdf" => return crate::metadata::pdf::get_metadata(cursor),
                "rb" => return crate::metadata::rb::get_metadata(cursor),
                "imp" => return crate::metadata::imp::get_metadata(cursor),
                "lrf" | "lrx" => return crate::metadata::lrx::get_metadata(cursor),
                "opf" => {
                    // Special OPF handling
                    // Parse OPF
                    // If cover missing, try to find in zip
                    // We need to re-read the OPF string from buffer
                    // Re-use buf
                    let opf_str = String::from_utf8_lossy(cursor.get_ref());
                    let mi = crate::opf::parse_opf(&opf_str)?;

                    if mi.cover_id.is_none() {
                        // Try to find cover? Python zip_opf_metadata logic
                        // This logic implies the OPF refers to a cover image that is also in the ZIP?
                        // Or checking if the opf object has a cover path?
                        // Python: if getattr(mi, 'cover', None): covername = basename(mi.cover)...
                        // In our Rust struct, we don't have local cover paths usually?
                        // We have cover_id (from meta name="cover" content="id").
                        // We usually resolve that ID to a manifest item, then href.
                        // `parse_opf` doesn't fully resolve manifest/spine to find the href yet, it returns the raw ID.

                        // Enhancing `parse_opf` to resolve cover href is a bigger task.
                        // For now, let's just return what we have.
                    }
                    return Ok(mi);
                }
                // "txt" ?, "rtf"? Not in the python list above but maybe?
                // The python list: 'lit', 'opf', 'prc', 'mobi', 'fb2', 'epub', 'rb', 'imp', 'pdf', 'lrf', 'azw', 'azw1', 'azw3'
                _ => {}
            }
        }
    }

    bail!("No ebook found in ZIP archive")
}
