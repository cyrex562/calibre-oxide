use crate::metadata::{string_to_authors, MetaInformation};
use anyhow::Result;
use std::io::{Read, Seek, SeekFrom};

const MAGIC1: &[u8] = b"\x00\x01BOOKDOUG";
const MAGIC2: &[u8] = b"\x00\x02BOOKDOUG";

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut header = [0u8; 10];
    stream.read_exact(&mut header)?;

    if header != MAGIC1 && header != MAGIC2 {
        return Ok(MetaInformation::default());
    }

    // Skip 38 bytes
    stream.seek(SeekFrom::Current(38))?;

    // cString() - discard 1 string
    read_cstring(&mut stream, 0)?;

    // category = cString()
    let _category = read_cstring(&mut stream, 0)?;

    // title = cString(1)
    let title = read_cstring(&mut stream, 1)?;

    // author = cString(2)
    let author = read_cstring(&mut stream, 2)?;

    let mut mi = MetaInformation::default();
    if !title.is_empty() {
        mi.title = title;
    } else {
        mi.title = "Unknown".to_string();
    }

    if !author.is_empty() {
        mi.authors = string_to_authors(&author);
    } else {
        mi.authors = vec!["Unknown".to_string()];
    }

    Ok(mi)
}

fn read_cstring<R: Read>(stream: &mut R, mut skip: usize) -> Result<String> {
    let mut bytes = Vec::new();
    loop {
        let mut buf = [0u8; 1];
        if stream.read(&mut buf)? == 0 {
            break; // EOF
        }
        let b = buf[0];

        if b == 0 {
            if skip == 0 {
                return Ok(String::from_utf8_lossy(&bytes).to_string());
            }
            skip -= 1;
            bytes.clear();
        } else {
            bytes.push(b);
        }
    }
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::io::Write;

    #[test]
    fn test_imp_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(MAGIC1);
        buffer.extend_from_slice(&[0u8; 38]);

        // 1. Discarded string (cString())
        buffer.write_all(b"Discarded\0")?;

        // 2. Category (cString())
        buffer.write_all(b"My Category\0")?;

        // 3. Title (cString(1))
        // Reads "SkipMe" \0. Skip=1->0. Clear.
        // Reads "Start" \0. Skip=0. Return.
        // Wait, cString(1) reads 2 strings.
        // String 1: "SkipMe"
        // String 2: "My Title"
        buffer.write_all(b"SkipMe\0")?;
        buffer.write_all(b"My Title\0")?;

        // 4. Author (cString(2))
        // String 1: "SkipA"
        // String 2: "SkipB"
        // String 3: "My Author"
        buffer.write_all(b"SkipA\0")?;
        buffer.write_all(b"SkipB\0")?;
        buffer.write_all(b"My Author\0")?;

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "My Title");
        assert_eq!(mi.authors, vec!["My Author"]);

        Ok(())
    }
}
