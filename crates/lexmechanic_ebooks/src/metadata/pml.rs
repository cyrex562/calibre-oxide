use crate::metadata::MetaInformation;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::bytes::Regex;
use std::io::{Read, Seek, SeekFrom};
use zip::ZipArchive;

lazy_static! {
    static ref COMMENT_PAT: Regex = Regex::new(r"(?ms)\\v.*?\\v").unwrap();
    static ref TITLE_PAT: Regex = Regex::new(r#"TITLE="(.*?)""#).unwrap();
    static ref AUTHOR_PAT: Regex = Regex::new(r#"AUTHOR="(.*?)""#).unwrap();
    static ref PUBLISHER_PAT: Regex = Regex::new(r#"PUBLISHER="(.*?)""#).unwrap();
    static ref ISBN_PAT: Regex = Regex::new(r#"ISBN="(.*?)""#).unwrap();
}

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let start_pos = stream.stream_position()?;

    // Check for ZIP (pmlz)
    let mut signature = [0u8; 4];
    let is_zip = if stream.read_exact(&mut signature).is_ok() {
        &signature == b"PK\x03\x04"
    } else {
        false
    };

    stream.seek(SeekFrom::Start(start_pos))?;

    let pml_content = if is_zip {
        let mut archive = ZipArchive::new(&mut stream)?;
        let mut content = Vec::new();
        for i in 0..archive.len() {
            let mut file = archive.by_index(i)?;
            if file.name().ends_with(".pml") {
                file.read_to_end(&mut content)?;
            }
        }
        content
    } else {
        let mut content = Vec::new();
        stream.read_to_end(&mut content)?;
        content
    };

    extract_metadata(&pml_content)
}

fn extract_metadata(content: &[u8]) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();
    mi.title = "Unknown".to_string();
    mi.authors = vec!["Unknown".to_string()];

    for cap in COMMENT_PAT.find_iter(content) {
        let comment = cap.as_bytes();

        if let Some(m) = TITLE_PAT.captures(comment) {
            mi.title = decode_bytes(&m[1]);
        }
        if let Some(m) = AUTHOR_PAT.captures(comment) {
            let auth = decode_bytes(&m[1]);
            if mi.authors.len() == 1 && mi.authors[0] == "Unknown" {
                mi.authors.clear();
            }
            // Avoid duplicates?
            if !mi.authors.contains(&auth) {
                mi.authors.push(auth);
            }
        }
        if let Some(m) = PUBLISHER_PAT.captures(comment) {
            mi.publisher = Some(decode_bytes(&m[1]));
        }
        if let Some(m) = ISBN_PAT.captures(comment) {
            let isbn = decode_bytes(&m[1]);
            mi.identifiers.insert("isbn".to_string(), isbn);
        }
    }

    Ok(mi)
}

fn decode_bytes(bytes: &[u8]) -> String {
    // Simplified decoding (lossy UTF-8)
    String::from_utf8_lossy(bytes).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;
    use zip::write::FileOptions;

    #[test]
    fn test_pml_metadata() {
        let content =
            b"Some content \\vTITLE=\"My Title\"\\v More content \\vAUTHOR=\"My Author\"\\v";
        let mut stream = Cursor::new(content);
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "My Title");
        assert_eq!(mi.authors, vec!["My Author"]);
    }

    #[test]
    fn test_pmlz_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("book.pml", options)?;
            zip.write_all(b"\\vTITLE=\"Zipped Title\"\\v")?;
            zip.finish()?;
        }

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;
        assert_eq!(mi.title, "Zipped Title");
        Ok(())
    }
}
