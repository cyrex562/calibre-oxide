use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::Result;
use byteorder::{BigEndian, ReadBytesExt};
use chrono::{TimeZone, Utc};
use std::io::{Read, Seek};

const DATATYPE_METADATA: u8 = 10;
const TYPE_AUTHOR: u16 = 4;
const TYPE_TITLE: u16 = 5;
const TYPE_PUBDATE: u16 = 6;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let header = PdbHeader::parse(&mut stream)?;
    get_metadata_from_header(&mut stream, &header)
}

pub fn get_metadata_from_header<R: Read + Seek>(
    stream: &mut R,
    header: &PdbHeader,
) -> Result<MetaInformation> {
    let mut mi = MetaInformation::default();

    // Iterate sections to find Metadata section
    for i in 1..header.num_records as usize {
        let data = header.section_data(stream, i)?;
        if data.len() < 10 {
            // Header(8) + Count(2)
            continue;
        }

        // Parse Section Header
        let mut reader = &data[..];
        let _uid = reader.read_u16::<BigEndian>()?;
        let _paragraphs = reader.read_u16::<BigEndian>()?;
        let _size = reader.read_u16::<BigEndian>()?;
        let type_id = reader.read_u8()?;
        let _flags = reader.read_u8()?;

        if type_id == DATATYPE_METADATA {
            let record_count = reader.read_u16::<BigEndian>()?;

            // reader advanced 10 bytes.
            // But we want to process the REST of the records.
            // section_body starts at 10.
            let section_body = &data[10..];

            let mut cursor = std::io::Cursor::new(section_body);

            for _ in 0..record_count {
                // Check if enough data for header (4 bytes)
                let pos = cursor.position() as usize;
                if pos + 4 > section_body.len() {
                    break;
                }

                let rec_type = cursor.read_u16::<BigEndian>()?;
                let rec_len = cursor.read_u16::<BigEndian>()?;

                let total_bytes = (rec_len as usize) * 2;
                if total_bytes < 4 {
                    continue;
                }
                let payload_len = total_bytes - 4;

                // Check if enough data for payload
                let pos = cursor.position() as usize;
                if pos + payload_len > section_body.len() {
                    break;
                }

                let mut payload = vec![0u8; payload_len];
                cursor.read_exact(&mut payload)?;

                match rec_type {
                    TYPE_TITLE => mi.title = decode_latin1(&payload),
                    TYPE_AUTHOR => {
                        mi.authors = decode_latin1(&payload)
                            .split(',')
                            .map(|s| s.trim().to_string())
                            .collect()
                    }
                    TYPE_PUBDATE => {
                        if payload.len() >= 4 {
                            let mut p = &payload[..4];
                            if let Ok(ts) = p.read_u32::<BigEndian>() {
                                mi.pubdate = Some(Utc.timestamp_opt(ts as i64, 0).unwrap());
                            }
                        }
                    }
                    _ => {}
                }
            }
            break; // Found metadata, done
        }
    }

    Ok(mi)
}

fn decode_latin1(bytes: &[u8]) -> String {
    let v: Vec<u8> = bytes.iter().filter(|&&b| b != 0).cloned().collect();
    String::from_utf8_lossy(&v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdb::header::SectionRecord;
    use byteorder::WriteBytesExt;
    use std::io::Cursor;

    #[test]
    fn test_plucker_metadata() -> Result<()> {
        let mut buffer = Vec::new();
        let header = PdbHeader {
            name: "PluckerBook".to_string(),
            num_records: 2,
            records: vec![
                SectionRecord {
                    offset: 100,
                    attributes: 0,
                    unique_id: 0,
                },
                SectionRecord {
                    offset: 200,
                    attributes: 0,
                    unique_id: 1,
                },
            ],
            attributes: 0,
            version: 0,
            create_time: 0,
            modify_time: 0,
            backup_time: 0,
            modification_number: 0,
            app_info_id: 0,
            sort_info_id: 0,
            type_id: [0; 4],
            creator_id: [0; 4],
            unique_id_seed: 0,
            next_record_list_id: 0,
        };

        buffer.resize(200, 0);

        let mut sec = Vec::new();
        sec.write_u16::<BigEndian>(0)?;
        sec.write_u16::<BigEndian>(0)?;
        sec.write_u16::<BigEndian>(0)?;
        sec.write_u8(DATATYPE_METADATA)?;
        sec.write_u8(0)?;

        sec.write_u16::<BigEndian>(2)?;

        let title = b"Test";
        let rec1_len = (4 + 4) / 2;
        sec.write_u16::<BigEndian>(TYPE_TITLE)?;
        sec.write_u16::<BigEndian>(rec1_len as u16)?;
        sec.extend_from_slice(title);

        let author = b"Me";
        let rec2_len = (2 + 4) / 2;
        sec.write_u16::<BigEndian>(TYPE_AUTHOR)?;
        sec.write_u16::<BigEndian>(rec2_len as u16)?;
        sec.extend_from_slice(author);

        buffer.extend_from_slice(&sec);

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata_from_header(&mut stream, &header)?;

        assert_eq!(mi.title, "Test");
        assert_eq!(mi.authors, vec!["Me"]);

        Ok(())
    }
}
