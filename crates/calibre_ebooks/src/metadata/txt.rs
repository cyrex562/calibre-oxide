use crate::metadata::MetaInformation;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::io::{Read, Seek};

lazy_static! {
    static ref TXT_PAT: Regex = Regex::new(
        r"(?u)^[ ]*(?P<title>.+)[ ]*(\n{3}|(\r\n){3}|\r{3})[ ]*(?P<author>.+)[ ]*(\n|\r\n|\r)$"
    )
    .unwrap();
}

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();

    // Read first 1-4 lines or 1KB
    // Python code: reads 4 lines, decodes, truncates to 1024 chars.
    // Equivalent: Read 4KB bytes, decode lossy, check valid lines?
    // Or just read 1KB bytes.
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf)?;
    let s = String::from_utf8_lossy(&buf[..n]);

    // We need to match the START of the file, up to 1KB.
    // The Python regex matches `(?u)^...`.
    // We should pass the decoded string to regex.

    if let Some(caps) = TXT_PAT.captures(&s) {
        if let Some(title) = caps.name("title") {
            mi.title = title.as_str().trim().to_string();
        }
        if let Some(author) = caps.name("author") {
            // Python: author.split(',')
            mi.authors = author
                .as_str()
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
        }
    }

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_txt_metadata() {
        let content = "My Title   \n\n\n   My Author  \n";
        let mut stream = Cursor::new(content.as_bytes());
        let mi = get_metadata(&mut stream).unwrap();
        assert_eq!(mi.title, "My Title");
        assert_eq!(mi.authors, vec!["My Author"]);
    }
}
