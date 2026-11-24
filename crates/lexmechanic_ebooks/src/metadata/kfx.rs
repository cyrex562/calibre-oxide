use crate::metadata::MetaInformation;
use anyhow::{bail, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};

// Magic Constants
const CONTAINER_MAGIC: &[u8] = b"CONT";
const ENTITY_MAGIC: &[u8] = b"ENTY";
const ION_MAGIC: &[u8] = b"\xe0\x01\x00\xea";

// ION Data Types
const DT_BOOLEAN: u8 = 1;
const DT_INTEGER: u8 = 2;
const DT_PROPERTY: u8 = 7;
const DT_STRING: u8 = 8;
const DT_STRUCT: u8 = 11;
const DT_LIST: u8 = 12;
const DT_OBJECT: u8 = 13;
const DT_TYPED_DATA: u8 = 14;

// Properties
const P_METADATA: u64 = 258;
const P_METADATA2: u64 = 490;
const _P_METADATA3: u32 = 494; // Rec 1 metadata type for KFX?
const _P_METADATA_KEY: u32 = 538;
const _P_METADATA_VALUE: u32 = 539;
const _P_IMAGE: u32 = 534;

const P_LANGUAGES: u64 = 10;
const P_TITLE: u64 = 153;
const P_AUTHOR: u64 = 222;
const P_PUBLISHER: u64 = 232;
// const P_DESCRIPTION: u64 = 154;

#[allow(dead_code)]
#[derive(Debug, Clone)]
enum IonValue {
    Boolean(bool),
    Integer(u64),
    String(String),
    Property(u64), // Property ID
    Struct(Vec<IonValue>),
    List(Vec<IonValue>),
    Object(HashMap<u64, IonValue>),
    TypedData(Box<IonValue>), // Simplified
}

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let mut buffer = Vec::new();
    stream.read_to_end(&mut buffer)?;
    let mut cursor = std::io::Cursor::new(&buffer);

    let container = parse_container(&mut cursor)?;
    let metadata = extract_kfx_metadata(&container);

    let mut mi = MetaInformation::default();

    if let Some(title) = metadata.get(&P_TITLE).and_then(|v| v.get(0)) {
        mi.title = title.clone();
    }

    if let Some(authors) = metadata.get(&P_AUTHOR) {
        mi.authors = authors.clone();
    }

    if let Some(langs) = metadata.get(&P_LANGUAGES) {
        mi.languages = langs.clone();
    }

    if let Some(publ) = metadata.get(&P_PUBLISHER).and_then(|v| v.get(0)) {
        mi.publisher = Some(publ.clone());
    }

    Ok(mi)
}

fn parse_container<R: Read + Seek>(stream: &mut R) -> Result<Vec<(u64, u64, IonValue)>> {
    let mut magic = [0u8; 4];
    stream.read_exact(&mut magic)?;
    if magic != CONTAINER_MAGIC {
        bail!("Invalid KFX Container Magic");
    }

    let _version = read_u16_le(stream)?;
    let header_len = read_u32_le(stream)?;

    // Skip unknown 8 bytes
    stream.seek(SeekFrom::Current(8))?;

    let mut entities = Vec::new();

    // Loop until we hit ION_MAGIC at start of next block
    loop {
        // Peek 4 bytes
        let mut peek = [0u8; 4];
        let pos = stream.stream_position()?;
        if stream.read_exact(&mut peek).is_err() {
            break;
        }
        stream.seek(SeekFrom::Start(pos))?;

        if peek == ION_MAGIC {
            break;
        }

        let _entity_id = read_u32_le(stream)?;
        let entity_type = read_u32_le(stream)?;
        let entity_offset = read_u64_le(stream)?;
        let entity_len = read_u64_le(stream)?;

        let current_pos = stream.stream_position()?;

        // Go to entity
        let abs_start = header_len as u64 + entity_offset;
        stream.seek(SeekFrom::Start(abs_start))?;

        // Read Entity Header
        let mut e_magic = [0u8; 4];
        stream.read_exact(&mut e_magic)?;
        if e_magic != ENTITY_MAGIC {
            // Just skip if not valid?
        } else {
            let _e_ver = read_u16_le(stream)?;
            let _e_hdr_len = read_u32_le(stream)?;

            // Body starts after header
            // Data len? entity_len includes header?
            // Assuming entity_len is total length.
            let body_len = entity_len - (stream.stream_position()? - abs_start);

            let mut body = vec![0u8; body_len as usize];
            stream.read_exact(&mut body)?;

            if let Ok(val) = parse_ion_data(&body) {
                entities.push((entity_type.into(), _entity_id.into(), val));
            }
        }

        stream.seek(SeekFrom::Start(current_pos))?;
    }

    Ok(entities)
}

fn parse_ion_data(data: &[u8]) -> Result<IonValue> {
    let mut cursor = std::io::Cursor::new(data);
    let mut magic = [0u8; 4];
    if cursor.read_exact(&mut magic).is_err() || magic != ION_MAGIC {
        // Fallback for raw data?
        // simple return?
        bail!("Not ION data");
    }

    unpack_typed_value(&mut cursor)
}

fn unpack_typed_value<R: Read>(stream: &mut R) -> Result<IonValue> {
    let mut byte = [0u8; 1];
    if stream.read_exact(&mut byte).is_err() {
        bail!("EOF in ION");
    }
    let cmd = byte[0];
    let data_type = cmd >> 4;
    let mut data_len = (cmd & 0x0F) as u64;

    if data_len == 14 {
        data_len = unpack_number(stream)?;
    }

    match data_type {
        DT_BOOLEAN => Ok(IonValue::Boolean(data_len != 0)),
        DT_INTEGER => {
            // unpack unsigned int
            let val = unpack_uint(stream, data_len as usize)?;
            Ok(IonValue::Integer(val))
        }
        DT_PROPERTY => {
            let val = unpack_uint(stream, data_len as usize)?;
            Ok(IonValue::Property(val))
        }
        DT_STRING => {
            let mut buf = vec![0u8; data_len as usize];
            stream.read_exact(&mut buf)?;
            Ok(IonValue::String(String::from_utf8_lossy(&buf).to_string()))
        }
        DT_STRUCT | DT_LIST => {
            // Nested ION
            let mut buf = vec![0u8; data_len as usize];
            stream.read_exact(&mut buf)?;
            let mut inner = std::io::Cursor::new(buf);
            let mut list = Vec::new();
            while inner.position() < inner.get_ref().len() as u64 {
                list.push(unpack_typed_value(&mut inner)?);
            }
            if data_type == DT_STRUCT {
                Ok(IonValue::Struct(list))
            } else {
                Ok(IonValue::List(list))
            }
        }
        DT_OBJECT => {
            let mut buf = vec![0u8; data_len as usize];
            stream.read_exact(&mut buf)?;
            let mut inner = std::io::Cursor::new(buf);
            let mut map = HashMap::new();
            while inner.position() < inner.get_ref().len() as u64 {
                let sym = unpack_number(&mut inner)?;
                let val = unpack_typed_value(&mut inner)?;
                map.insert(sym, val);
            }
            Ok(IonValue::Object(map))
        }
        DT_TYPED_DATA => {
            let mut buf = vec![0u8; data_len as usize];
            stream.read_exact(&mut buf)?;
            let mut inner = std::io::Cursor::new(buf);
            unpack_number(&mut inner)?; // Ignored
            unpack_number(&mut inner)?; // Ignored
            let val = unpack_typed_value(&mut inner)?;
            Ok(IonValue::TypedData(Box::new(val)))
        }
        _ => {
            // Skip unknown
            skip(stream, data_len as usize)?;
            Ok(IonValue::Boolean(false)) // Dummy
        }
    }
}

// Variable length number, MSB first, 7 bits per byte, last byte flagged by MSB set.
fn unpack_number<R: Read>(stream: &mut R) -> Result<u64> {
    let mut val: u64 = 0;
    loop {
        let mut b = [0u8; 1];
        stream.read_exact(&mut b)?;
        let byte = b[0];
        val = (val << 7) | ((byte & 0x7F) as u64);
        if (byte & 0x80) != 0 {
            break;
        }
    }
    Ok(val)
}

fn unpack_uint<R: Read>(stream: &mut R, len: usize) -> Result<u64> {
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    let mut val = 0u64;
    for b in buf {
        val = (val << 8) | (b as u64);
    }
    Ok(val)
}

fn read_u16_le<R: Read>(stream: &mut R) -> Result<u16> {
    let mut b = [0u8; 2];
    stream.read_exact(&mut b)?;
    Ok(u16::from_le_bytes(b))
}

fn read_u32_le<R: Read>(stream: &mut R) -> Result<u32> {
    let mut b = [0u8; 4];
    stream.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn read_u64_le<R: Read>(stream: &mut R) -> Result<u64> {
    let mut b = [0u8; 8];
    stream.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn skip<R: Read>(stream: &mut R, n: usize) -> Result<()> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    Ok(())
}

fn extract_kfx_metadata(entities: &[(u64, u64, IonValue)]) -> HashMap<u64, Vec<String>> {
    let mut metadata = HashMap::new();
    let mut metadata_entity = HashMap::new();

    for (etype, _eid, eval) in entities {
        if *etype == P_METADATA {
            // Found raw metadata entity (Object or List?)
            if let IonValue::Object(map) = eval {
                metadata_entity = map.clone();
            }
        } else if *etype == P_METADATA2 {
            // Complex metadata
            // Not implementing full recursion for now, relies on simple extraction
        }
    }

    // Map known properties
    for (key, val) in metadata_entity {
        let s_val = match val {
            IonValue::String(s) => s,
            _ => continue,
        };
        metadata.entry(key).or_insert_with(Vec::new).push(s_val);
    }

    metadata
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_vwi_decode() {
        // 0x81 -> 1 (0000001 | flag)
        let data = [0x81];
        let val = unpack_number(&mut Cursor::new(&data)).unwrap();
        assert_eq!(val, 1);

        // 0x01 0x82 -> (1 << 7) | 2 = 128 + 2 = 130
        let data = [0x01, 0x82];
        let val = unpack_number(&mut Cursor::new(&data)).unwrap();
        assert_eq!(val, 130);
    }
}
