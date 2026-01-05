use crate::compression::palmdoc;
use crate::compression::palmdoc::compress;
use crate::oeb::book::OEBBook;
use anyhow::{Context, Result};
use byteorder::{BigEndian, WriteBytesExt};
use chrono::Local;
use std::io::{Cursor, Write};

pub struct MobiWriter;

impl MobiWriter {
    pub fn new() -> Self {
        MobiWriter
    }

    pub fn write<W: Write>(&self, book: &OEBBook, writer: &mut W) -> Result<()> {
        // 1. Flatten Content
        // Very basic: Concatenate all spine items regardless of tags, strip tags?
        // Or keep tags? MOBI supports HTML-like markup.
        // We will concat raw content of spine items.
        // Assuming spine items are HTML.

        let mut text_content = Vec::new();
        // Add minimal HTML wrapper?
        text_content.extend_from_slice(b"<html><head><guide></guide></head><body>");

        for itemref in &book.spine.items {
            if let Some(item) = book.manifest.items.get(&itemref.idref) {
                if let Ok(data) = book.container.read(&item.href) {
                    // Filter? assume raw HTML is fine for now
                    text_content.extend_from_slice(&data);
                }
            }
        }
        text_content.extend_from_slice(b"</body></html>");

        // 2. Compress Records
        let mut records = Vec::new();
        let chunk_size = 4096;
        for chunk in text_content.chunks(chunk_size) {
            let compressed = compress(chunk)?;
            records.push(compressed);
        }

        // 3. Construct Headers

        // PDB Header
        let name = book
            .metadata
            .items
            .iter()
            .find(|i| i.term == "title")
            .map(|i| i.value.clone())
            .unwrap_or("Unknown".to_string());
        let safe_name = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        let mut db_name = [0u8; 32];
        let name_bytes = safe_name.as_bytes();
        let len = std::cmp::min(name_bytes.len(), 31);
        db_name[..len].copy_from_slice(&name_bytes[..len]);

        writer.write_all(&db_name)?; // 0-31: Name
        writer.write_u16::<BigEndian>(0)?; // 32-33: Attributes
        writer.write_u16::<BigEndian>(0)?; // 34-35: Version

        let now = Local::now().timestamp() as u32;
        writer.write_u32::<BigEndian>(now)?; // 36-39: Creation Date
        writer.write_u32::<BigEndian>(now)?; // 40-43: Modification Date

        writer.write_u32::<BigEndian>(0)?; // 44-47: Backup Checksum
        writer.write_u32::<BigEndian>(0)?; // 48-51: Modification Number
        writer.write_u32::<BigEndian>(0)?; // 52-55: App Info ID
        writer.write_u32::<BigEndian>(0)?; // 56-59: Sort Info ID

        writer.write_all(b"BOOK")?; // 60-63: Type
        writer.write_all(b"MOBI")?; // 64-67: Creator

        writer.write_u32::<BigEndian>(0)?; // 68-71: Unique ID Seed
        writer.write_u32::<BigEndian>(0)?; // 72-75: Next Record List ID

        // Record List Record
        // Count: records.len() + 1 (MOBI Header) + 1 (EOF) + ? (EXTH?)
        // Actually:
        // 0: MOBI Header (includes PalmDoc header, EXTH header is trailing usually or separate record?)
        //    MOBI docs: Record 0 is the logical header.
        //    It contains: PalmDocHeader + MobiHeader + EXTHHeader(optional).
        // 1..N: Text Records.
        // N+1: FLIS (optional)
        // N+2: FCIS (optional)
        // EOF Record (usually just clean stop)

        // Let's build Record 0 Buffer.
        let mut record0 = Vec::new();

        // -- PalmDoc Header (16 bytes) --
        // Compression: 2 (PalmDoc)
        // Unused: 0
        // Text Length: total uncompressed
        // Record Count: number of text records
        // Record Size: 4096
        // Encryption: 0

        record0.write_u16::<BigEndian>(2)?; // Compression
        record0.write_u16::<BigEndian>(0)?; // Unused
        record0.write_u32::<BigEndian>(text_content.len() as u32)?; // Text Length
        record0.write_u16::<BigEndian>(records.len() as u16)?; // Record Count
        record0.write_u16::<BigEndian>(4096)?; // Record Size
        record0.write_u16::<BigEndian>(0)?; // Encryption
        record0.write_u16::<BigEndian>(0)?; // Unknown

        // -- MOBI Header --
        // Base offset for relative check: 16
        // Identifier: MOBI
        // Header Length: ? (exclude PalmDoc part? or total?)
        //   "Length of the MOBI header, including the previous 14 bytes?" - No, typically starts after PalmDoc.
        //   Standard: Length is usually 232 or similar for version 6.
        let mobi_header_start = record0.len();
        record0.write_all(b"MOBI")?; // Identifier
        record0.write_u32::<BigEndian>(232)?; // Header Length (Target)
        record0.write_u32::<BigEndian>(2)?; // Mobi Type (2=Book)
        record0.write_u32::<BigEndian>(65001)?; // Text Encoding (UTF-8)
        record0.write_u32::<BigEndian>(0)?; // Unique ID
        record0.write_u32::<BigEndian>(6)?; // Generator Version

        // ... Fill reset with zeros until standard length (232 + 16 = 248? No, 232 from start of MOBI)
        // 232 - (current_len - mobi_start)
        // current written: 4+4+4+4+4+4 = 24 bytes.
        // Need to pad.
        // Note: We need EXTH flag: 0x40 at offset 0x80 + header start (128).
        // Let's write explicitly up to flags.

        for _ in 0..40 {
            record0.write_u32::<BigEndian>(0xFFFFFFFF)?;
        } // Padding/Reserved placeholders (fill random/ones)
          // Re-write useful fields.
          // Doing strict offset writing is safer.
          // Let's resize and seek.
        record0.resize(mobi_header_start + 232, 0);
        let mut r0_curs = Cursor::new(&mut record0);
        r0_curs.set_position((mobi_header_start + 80) as u64); // First Non-Book Index?
                                                               // ..

        // Flag for EXTH: bit 6 (0x40) at offset 0x80 (128)??
        // Offset 128 (0x80): EXTH Flags.
        // 0x40 = EXTH exists.
        r0_curs.set_position((mobi_header_start + 0x80) as u64);
        r0_curs.write_u32::<BigEndian>(0x40)?;

        // Title Offset/Len (Offset relative to PDB record 0 start) (Legacy header style)
        // For simple MOBI, we rely on EXTH usually.
        // But legacy readers need Full Name Offset (offset 84).

        // Simple path: Append EXTH after MOBI header.

        // -- EXTH Header --
        let mut exth = Vec::new();
        exth.write_all(b"EXTH")?;
        exth.write_u32::<BigEndian>(0)?; // Length placeholder
        exth.write_u32::<BigEndian>(1)?; // Count (1 record: Title)

        // Title Record (Type 500)
        let title_bytes = name.as_bytes();
        exth.write_u32::<BigEndian>(500)?;
        exth.write_u32::<BigEndian>((8 + title_bytes.len()) as u32)?; // Len (8 header + data)
        exth.write_all(title_bytes)?;

        // Padding EXTH to 4 bytes?
        while exth.len() % 4 != 0 {
            exth.push(0);
        }

        // Update EXTH Length
        let exth_len = exth.len();
        let mut ec = Cursor::new(&mut exth);
        ec.set_position(4);
        ec.write_u32::<BigEndian>(exth_len as u32)?;

        // Append EXTH to Record 0
        record0.extend_from_slice(&exth);

        // 4. Record Offsets (PDB Header continued)
        // 76-77: Number of Records
        let num_records = 1 + records.len(); // Rec0 + Content
        writer.write_u16::<BigEndian>(num_records as u16)?;

        // Record List (Offset Table)
        // Offset (4 bytes) + Attributes (1 byte) + UniqueID (3 bytes) = 8 bytes per record.
        // Start offset = 78 + (num_records * 8) + 2 (pad).
        let mut current_offset = 78 + (num_records as u32 * 8) + 2;

        // Rec 0
        writer.write_u32::<BigEndian>(current_offset)?;
        writer.write_u8(0)?; // Attr
        writer.write_u24::<BigEndian>(0)?; // UID (using u24 helper... wait, manual)
                                           // Manual u24
        writer.write_all(&[0, 0, 0])?;
        current_offset += record0.len() as u32;

        // Text Records
        for record in &records {
            writer.write_u32::<BigEndian>(current_offset)?;
            writer.write_u8(0)?;
            writer.write_all(&[0, 0, 0])?;
            current_offset += record.len() as u32;
        }

        writer.write_u16::<BigEndian>(0)?; // Gap/Pad

        // Write Data
        writer.write_all(&record0)?;
        for record in &records {
            writer.write_all(record)?;
        }

        Ok(())
    }
}
