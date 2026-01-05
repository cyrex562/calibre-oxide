use crate::rb::header::{RbHeader, RbTocEntry};
use anyhow::Result;
use std::io::{Read, Seek, SeekFrom};

pub struct RbReader<R> {
    reader: R,
    header: RbHeader,
}

impl<R: Read + Seek> RbReader<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let header = RbHeader::parse(&mut reader)?;
        Ok(RbReader { reader, header })
    }

    pub fn get_toc_entries(&mut self) -> Result<Vec<RbTocEntry>> {
        self.reader
            .seek(SeekFrom::Start((self.header.toc_offset + 4) as u64))?;
        let mut entries = Vec::new();
        for _ in 0..self.header.toc_count {
            entries.push(RbTocEntry::read(&mut self.reader)?);
        }
        Ok(entries)
    }

    pub fn read_entry(&mut self, entry: &RbTocEntry) -> Result<Vec<u8>> {
        self.reader.seek(SeekFrom::Start(entry.offset as u64))?;
        let mut buffer = vec![0u8; entry.length as usize];
        self.reader.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    pub fn read_content(&mut self) -> Result<String> {
        let entries = self.get_toc_entries()?;
        // Content usually has flag 0 or 1?
        // Flag 2 is info.
        // Let's assume standard content pages have distinct flags or are not flag 2.
        // Typically NuvoMedia RB has "page" entries.
        // For simple extraction, let's grab all non-info entries?
        // Or specific ones?
        // Usually, .rb content is HTML-like.

        let mut content = String::new();
        for entry in entries {
            if entry.flag == 0 || entry.flag == 1 || entry.flag == 8 {
                // 1 = Encrypted? 0 = Plain? 8 = Image?
                // Rocket eBook format is quirky.
                // Assuming basic unencrypted text for now.
                // Let's read entries that look like text.
                if entry.name.ends_with(".html") || entry.name.ends_with(".htm") {
                    let data = self.read_entry(&entry)?;
                    content.push_str(&String::from_utf8_lossy(&data));
                }
            }
        }
        Ok(content)
    }
}
