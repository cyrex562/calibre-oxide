use crate::metadata::plucker;
use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::Result;
use std::io::{Read, Seek};

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let header = PdbHeader::parse(&mut stream)?;

    // Check type/creator
    // DataPlkr -> Plucker
    if &header.type_id == b"Data" && &header.creator_id == b"Plkr" {
        return plucker::get_metadata_from_header(&mut stream, &header);
    }

    // Default: Just use the name from header
    let mut mi = MetaInformation::default();
    mi.title = header.name.clone();
    mi.authors = vec!["Unknown".to_string()];

    Ok(mi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::{BigEndian, WriteBytesExt};
    use std::io::Cursor;

    #[test]
    fn test_pdb_metadata_dispatch() -> Result<()> {
        let mut buffer = Vec::new();
        // Construct basic PDB header
        let name = b"GenericPDB\0";
        buffer.extend_from_slice(name);
        buffer.resize(32, 0);

        buffer.write_u16::<BigEndian>(0)?;
        buffer.write_u16::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.extend_from_slice(b"TEST"); // Type
        buffer.extend_from_slice(b"TEST"); // Creator
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u32::<BigEndian>(0)?;
        buffer.write_u16::<BigEndian>(0)?; // Num Records 0

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "GenericPDB");
        Ok(())
    }
}
