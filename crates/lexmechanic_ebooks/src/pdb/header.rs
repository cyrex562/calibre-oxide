use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use lazy_static::lazy_static;
use regex::bytes::Regex;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};

lazy_static! {
    /// Port of `header.py`'s inline `br'[^-A-Za-z0-9 ]+'` pattern.
    static ref UNSAFE_NAME_CHARS: Regex = Regex::new(r"[^-A-Za-z0-9 ]+").unwrap();
}

/// Port of `PdbHeaderReader.name`'s sanitization: strip every NUL byte
/// from the raw 32-byte title field, then collapse each run of
/// characters outside `[-A-Za-z0-9 ]` to a single `_`.
fn sanitize_pdb_name(raw: &[u8]) -> String {
    let no_nulls: Vec<u8> = raw.iter().copied().filter(|&b| b != 0).collect();
    let sanitized = UNSAFE_NAME_CHARS.replace_all(&no_nulls, &b"_"[..]);
    String::from_utf8_lossy(&sanitized).into_owned()
}

pub type SectionRecord = PdbRecordInfo;

#[derive(Debug, Clone)]
pub struct PdbHeader {
    pub name: String,
    pub attributes: u16,
    pub version: u16,
    pub create_time: u32,
    pub modify_time: u32,
    pub backup_time: u32,
    pub modification_number: u32,
    pub app_info_id: u32,
    pub sort_info_id: u32,
    pub type_id: [u8; 4],
    pub creator_id: [u8; 4],
    pub unique_id_seed: u32,
    pub next_record_list_id: u32,
    pub num_records: u16,
    pub records: Vec<PdbRecordInfo>,
}

#[derive(Debug, Clone)]
pub struct PdbRecordInfo {
    pub offset: u32,
    pub attributes: u8,
    pub unique_id: u32,
}

impl PdbHeader {
    pub fn parse<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let mut name_bytes = [0u8; 32];
        reader.read_exact(&mut name_bytes)?;
        let name = sanitize_pdb_name(&name_bytes);

        let attributes = reader.read_u16::<BigEndian>()?;
        let version = reader.read_u16::<BigEndian>()?;
        let create_time = reader.read_u32::<BigEndian>()?;
        let modify_time = reader.read_u32::<BigEndian>()?;
        let backup_time = reader.read_u32::<BigEndian>()?;
        let modification_number = reader.read_u32::<BigEndian>()?;
        let app_info_id = reader.read_u32::<BigEndian>()?;
        let sort_info_id = reader.read_u32::<BigEndian>()?;

        let mut type_id = [0u8; 4];
        reader.read_exact(&mut type_id)?;

        let mut creator_id = [0u8; 4];
        reader.read_exact(&mut creator_id)?;

        let unique_id_seed = reader.read_u32::<BigEndian>()?;
        let next_record_list_id = reader.read_u32::<BigEndian>()?;
        let num_records = reader.read_u16::<BigEndian>()?;

        let mut records = Vec::with_capacity(num_records as usize);
        for _ in 0..num_records {
            let offset = reader.read_u32::<BigEndian>()?;
            let attributes = reader.read_u8()?;
            let mut uid_bytes = [0u8; 3];
            reader.read_exact(&mut uid_bytes)?;
            let unique_id = ((uid_bytes[0] as u32) << 16)
                | ((uid_bytes[1] as u32) << 8)
                | (uid_bytes[2] as u32);
            records.push(PdbRecordInfo {
                offset,
                attributes,
                unique_id,
            });
        }

        Ok(PdbHeader {
            name,
            attributes,
            version,
            create_time,
            modify_time,
            backup_time,
            modification_number,
            app_info_id,
            sort_info_id,
            type_id,
            creator_id,
            unique_id_seed,
            next_record_list_id,
            num_records,
            records,
        })
    }

    pub fn section_data<R: Read + Seek>(&self, reader: &mut R, index: usize) -> Result<Vec<u8>> {
        if index >= self.records.len() {
            bail!("Record index out of bounds: {}", index);
        }

        let start = self.records[index].offset as u64;

        let file_end = reader.seek(SeekFrom::End(0))?;
        let mut next_offset = file_end;

        for rec in &self.records {
            let off = rec.offset as u64;
            if off > start && off < next_offset {
                next_offset = off;
            }
        }

        let len = next_offset - start;
        let mut buffer = vec![0u8; len as usize];

        reader.seek(SeekFrom::Start(start))?;
        reader.read_exact(&mut buffer)?;

        Ok(buffer)
    }
}

/// Port of `header.py`'s `PdbHeaderBuilder`: builds a PDB fixed header
/// (78 bytes) plus record list from an identity (4-letter creator ID,
/// e.g. `"TEXt"`) and a title, given each section's length in bytes.
///
/// `crate::pdb::writer::PdbWriter` has its own inline equivalent of this
/// logic (pre-existing, not rewired to use this builder as part of this
/// port -- out of scope for issue #42's three files).
pub struct PdbHeaderBuilder {
    identity: [u8; 8],
    title: [u8; 32],
}

impl PdbHeaderBuilder {
    /// `identity` is padded/truncated to 8 bytes (matching Python's
    /// `identity.ljust(3, '\x00')[:8]` -- yes, `ljust(3, ...)` on an
    /// already-longer string is a no-op, the real truncation is the
    /// trailing `[:8]`). `title` is sanitized the same way
    /// [`PdbHeader::parse`]'s name field is, then null-padded to 31
    /// bytes plus a trailing NUL (32 bytes total).
    pub fn new(identity: &str, title: &str) -> Self {
        let mut identity_bytes = [0u8; 8];
        let id_src = identity.as_bytes();
        let id_len = id_src.len().min(8);
        identity_bytes[..id_len].copy_from_slice(&id_src[..id_len]);

        let sanitized = UNSAFE_NAME_CHARS.replace_all(title.as_bytes(), &b"_"[..]);
        let mut title_bytes = [0u8; 32];
        let t_len = sanitized.len().min(31);
        title_bytes[..t_len].copy_from_slice(&sanitized[..t_len]);
        // title_bytes[31] stays 0, matching the Python's trailing `\x00`.

        PdbHeaderBuilder {
            identity: identity_bytes,
            title: title_bytes,
        }
    }

    /// Writes the 78-byte fixed header, the `section_lengths.len()`
    /// record-list entries (8 bytes each, offsets computed by summing
    /// preceding section lengths), and the trailing 2-byte pad -- port
    /// of `build_header`.
    pub fn build_header<W: Write>(&self, section_lengths: &[usize], out: &mut W) -> Result<()> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let nrecords = section_lengths.len() as u32;

        out.write_all(&self.title)?;
        out.write_u16::<BigEndian>(0)?; // attributes
        out.write_u16::<BigEndian>(0)?; // version
        out.write_u32::<BigEndian>(now)?; // create_time
        out.write_u32::<BigEndian>(now)?; // modify_time
        out.write_u32::<BigEndian>(0)?; // backup_time
        out.write_u32::<BigEndian>(0)?; // modification_number
        out.write_u32::<BigEndian>(0)?; // app_info_id
        out.write_u32::<BigEndian>(0)?; // sort_info_id

        out.write_all(&self.identity)?;
        out.write_u32::<BigEndian>(nrecords)?; // unique_id_seed (matches Python exactly)
        out.write_u32::<BigEndian>(0)?; // next_record_list_id
        out.write_u16::<BigEndian>(nrecords as u16)?; // num_records

        let mut offset = 78u32 + (8 * nrecords) + 2;
        for &len in section_lengths {
            out.write_u32::<BigEndian>(offset)?;
            out.write_all(&[0u8, 0, 0, 0])?; // attributes + 3-byte unique_id
            offset += len as u32;
        }
        out.write_all(&[0u8, 0])?; // trailing pad

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn sanitize_strips_nulls_and_collapses_unsafe_runs() {
        let mut raw = vec![b'M', b'y', 0, b'B', b'o', b'o', b'k', b':', b':', b'!'];
        raw.resize(32, 0);
        assert_eq!(sanitize_pdb_name(&raw), "MyBook_");
    }

    #[test]
    fn sanitize_keeps_hyphen_digits_and_spaces() {
        let mut raw = b"Vol-2 1999".to_vec();
        raw.resize(32, 0);
        assert_eq!(sanitize_pdb_name(&raw), "Vol-2 1999");
    }

    #[test]
    fn build_header_round_trips_through_parse() {
        let builder = PdbHeaderBuilder::new("TEXt", "My Book!!");
        let mut out = Vec::new();
        let section_lengths = [10usize, 20, 5];
        builder.build_header(&section_lengths, &mut out).unwrap();
        // Section bytes so `section_data` has real content to bound.
        out.extend(std::iter::repeat_n(0u8, section_lengths.iter().sum()));

        let mut cursor = Cursor::new(out);
        let header = PdbHeader::parse(&mut cursor).unwrap();

        assert_eq!(header.name, "My Book_");
        assert_eq!(&header.type_id, b"TEXt");
        assert_eq!(header.num_records, 3);
        assert_eq!(header.records.len(), 3);
        // Each record's offset should exactly bound its declared length.
        for (i, &len) in section_lengths.iter().enumerate() {
            let data = header.section_data(&mut cursor, i).unwrap();
            assert_eq!(data.len(), len);
        }
    }

    #[test]
    fn header_builder_truncates_long_identity_and_title() {
        let builder = PdbHeaderBuilder::new("TOOLONGIDENT", &"x".repeat(50));
        let mut out = Vec::new();
        builder.build_header(&[], &mut out).unwrap();
        let mut cursor = Cursor::new(out);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        assert_eq!(header.type_id.len() + header.creator_id.len(), 8);
        assert_eq!(header.name.len(), 31);
    }
}
