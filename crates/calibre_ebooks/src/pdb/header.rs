use anyhow::{bail, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

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
        let end = name_bytes.iter().position(|&c| c == 0).unwrap_or(32);
        let name = String::from_utf8_lossy(&name_bytes[..end]).to_string();

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
