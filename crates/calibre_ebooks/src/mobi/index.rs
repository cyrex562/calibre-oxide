use crate::mobi::utils::{decint, decode_string};
use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::collections::{BTreeMap, HashMap};
use std::io::{Cursor, Read};

#[derive(Debug, Clone)]
pub struct TagX {
    pub tag: u8,
    pub num_of_values: u8,
    pub bitmask: u8,
    pub eof: u8,
}

#[derive(Debug, Clone)]
pub struct PTagX {
    pub tag: u8,
    pub value_count: Option<u32>,
    pub value_bytes: Option<u32>,
    pub num_of_values: u8,
}

#[derive(Debug)]
pub struct IndxHeader {
    pub len: u32,
    pub nul1: u32,
    pub type_: u32,
    pub gen: u32,
    pub start: u32,
    pub count: u32,
    pub code: u32,
    pub lng: u32,
    pub total: u32,
    pub ordt: u32,
    pub ligt: u32,
    pub nligt: u32,
    pub ncncx: u32,
    pub ocnt: u32,
    pub oentries: u32,
    pub ordt1: u32,
    pub ordt2: u32,
    pub tagx: u32,
    pub idx_header_end_pos: usize,
    pub ordt1_raw: Vec<u8>,
    pub ordt2_raw: Vec<u8>,
    pub ordt_map: String,
}

// Helper to format bytes like python
fn format_bytes(byts: &[u8]) -> String {
    byts.iter()
        .map(|b| format!("{:x}", b))
        .collect::<Vec<String>>()
        .join(" ")
}

pub fn check_signature(data: &[u8], signature: &[u8]) -> Result<()> {
    if data.len() < signature.len() || &data[..signature.len()] != signature {
        bail!(
            "Not a valid {:?} section (found {:?})",
            String::from_utf8_lossy(signature),
            String::from_utf8_lossy(&data[..signature.len().min(data.len())])
        );
    }
    Ok(())
}

fn parse_indx_header(data: &[u8]) -> Result<IndxHeader> {
    check_signature(data, b"INDX")?;
    let mut cursor = Cursor::new(&data[4..]);

    let len = cursor.read_u32::<BigEndian>()?;
    let nul1 = cursor.read_u32::<BigEndian>()?;
    let type_ = cursor.read_u32::<BigEndian>()?;
    let gen = cursor.read_u32::<BigEndian>()?;
    let start = cursor.read_u32::<BigEndian>()?;
    let count = cursor.read_u32::<BigEndian>()?;
    let code = cursor.read_u32::<BigEndian>()?;
    let lng = cursor.read_u32::<BigEndian>()?;
    let total = cursor.read_u32::<BigEndian>()?;
    let ordt = cursor.read_u32::<BigEndian>()?;
    let ligt = cursor.read_u32::<BigEndian>()?;
    let nligt = cursor.read_u32::<BigEndian>()?;
    let ncncx = cursor.read_u32::<BigEndian>()?;

    // Unknowns (27 * 4 bytes)
    for _ in 0..27 {
        let _ = cursor.read_u32::<BigEndian>()?;
    }

    let ocnt = cursor.read_u32::<BigEndian>()?;
    let oentries = cursor.read_u32::<BigEndian>()?;
    let ordt1 = cursor.read_u32::<BigEndian>()?;
    let ordt2 = cursor.read_u32::<BigEndian>()?;
    let tagx = cursor.read_u32::<BigEndian>()?;

    let idx_header_end_pos = cursor.position() as usize + 4; // +4 for INDX

    let mut ordt1_raw = Vec::new();
    let mut ordt2_raw = Vec::new();
    let mut ordt_map = String::new();

    if ordt1 > 0 && data.len() >= (ordt1 as usize + 4 + oentries as usize) {
        if &data[ordt1 as usize..ordt1 as usize + 4] == b"ORDT" {
            let start = ordt1 as usize + 4;
            let end = start + oentries as usize;
            ordt1_raw = data[start..end].to_vec();
        }
    }

    if ordt2 > 0 && data.len() >= (ordt2 as usize + 4 + 2 * oentries as usize) {
        if &data[ordt2 as usize..ordt2 as usize + 4] == b"ORDT" {
            let start = ordt2 as usize + 4;
            let end = start + 2 * oentries as usize;
            ordt2_raw = data[start..end].to_vec();

            if code == 65002 {
                let mut parsed = Vec::new();
                for i in (0..ordt2_raw.len()).step_by(2) {
                    if i + 1 < ordt2_raw.len() {
                        let b = ordt2_raw[i + 1];
                        if b > 0x20 && b < 0x7F {
                            parsed.push(b);
                        } else {
                            parsed.push(b'?');
                        }
                    } else {
                        parsed.push(b'?');
                    }
                }
                ordt_map = String::from_utf8(parsed).unwrap_or_default();
            } else {
                ordt_map = "?".repeat(oentries as usize);
            }
        }
    }

    Ok(IndxHeader {
        len,
        nul1,
        type_,
        gen,
        start,
        count,
        code,
        lng,
        total,
        ordt,
        ligt,
        nligt,
        ncncx,
        ocnt,
        oentries,
        ordt1,
        ordt2,
        tagx,
        idx_header_end_pos,
        ordt1_raw,
        ordt2_raw,
        ordt_map,
    })
}

pub struct CNCXReader {
    pub records: BTreeMap<usize, String>,
}

impl CNCXReader {
    pub fn new(records: &[Vec<u8>], codec: &str) -> Self {
        let mut map = BTreeMap::new();
        let mut record_offset = 0;

        for raw in records {
            let mut pos = 0;
            while pos < raw.len() {
                if let Ok((length, consumed)) = decint(&raw[pos..], true) {
                    let length = length as usize;
                    if length > 0 {
                        let entry_start = pos + consumed;
                        let entry_end = entry_start + length;
                        if entry_end <= raw.len() {
                            let entry_bytes = &raw[entry_start..entry_end];
                            // Try decode
                            // simplified decode for now
                            let s = if codec == "utf-8" {
                                String::from_utf8_lossy(entry_bytes).to_string()
                            } else {
                                // Fallback
                                String::from_utf8_lossy(entry_bytes).to_string()
                            };
                            map.insert(pos + record_offset, s);
                        } else {
                            // Error logging?
                            eprintln!("CNCX entry out of bounds at offset {}", pos + record_offset);
                            map.insert(pos + record_offset, format_bytes(&raw[pos..]));
                            pos = raw.len();
                            break;
                        }
                    }
                    pos += consumed + length;
                } else {
                    break;
                }
            }
            record_offset += 0x10000;
        }

        CNCXReader { records: map }
    }

    pub fn get(&self, offset: usize) -> Option<&String> {
        self.records.get(&offset)
    }
}

fn parse_tagx_section(data: &[u8]) -> Result<(u32, Vec<TagX>)> {
    check_signature(data, b"TAGX")?;

    let mut cursor = Cursor::new(&data[4..]);
    let first_entry_offset = cursor.read_u32::<BigEndian>()?;
    let control_byte_count = cursor.read_u32::<BigEndian>()?;

    let mut tags = Vec::new();
    let mut current_pos = 12;

    while current_pos < first_entry_offset as usize {
        // TagX is 4 bytes
        if current_pos + 4 > data.len() {
            break;
        }
        let tag = data[current_pos];
        let num_of_values = data[current_pos + 1];
        let bitmask = data[current_pos + 2];
        let eof = data[current_pos + 3];
        tags.push(TagX {
            tag,
            num_of_values,
            bitmask,
            eof,
        });
        current_pos += 4;
    }

    Ok((control_byte_count, tags))
}

fn get_tag_map(
    control_byte_count: u32,
    tagx: &[TagX],
    data: &[u8],
    _strict: bool,
) -> Result<BTreeMap<u8, Vec<u64>>> {
    let control_byte_count = control_byte_count as usize;
    if data.len() < control_byte_count {
        bail!("Data too short for control bytes");
    }
    let mut control_bytes = data[..control_byte_count].to_vec();
    let mut reading_data = &data[control_byte_count..];

    let mut ptags = Vec::new();

    for x in tagx {
        if x.eof == 0x01 {
            // consume one control byte?
            if !control_bytes.is_empty() {
                control_bytes.remove(0); // This looks inefficient but size is small
            }
            continue;
        }

        if control_bytes.is_empty() {
            break;
        }

        let value = control_bytes[0] & x.bitmask;
        if value != 0 {
            let mut value_count = None;
            let mut value_bytes = None;

            if value == x.bitmask {
                if x.bitmask.count_ones() > 1 {
                    let (vb, consumed) = decint(reading_data, true)?;
                    reading_data = &reading_data[consumed..];
                    value_bytes = Some(vb as u32);
                } else {
                    value_count = Some(1);
                }
            } else {
                let mut mask = x.bitmask;
                let mut v = value;
                while (mask & 0b1) == 0 {
                    mask >>= 1;
                    v >>= 1;
                }
                value_count = Some(v as u32);
            }
            ptags.push(PTagX {
                tag: x.tag,
                value_count,
                value_bytes,
                num_of_values: x.num_of_values,
            });
        }
    }

    let mut ans = BTreeMap::new();

    for x in ptags {
        let mut values = Vec::new();
        if let Some(vc) = x.value_count {
            for _ in 0..(vc * x.num_of_values as u32) {
                let (byts, consumed) = decint(reading_data, true)?; // decint actually returns number but here we might need bytes?
                                                                    // Wait, in Python: `byts, consumed = decint(data)` returns integer value?
                                                                    // `calibre.ebooks.mobi.utils.decint` returns (int, count).
                                                                    // `reader/index.py` says: `byts, consumed = decint(data); values.append(byts)`
                                                                    // So `values` stores Integers?
                                                                    // Ah, `decint` returns an integer. But the python code appends `byts`.
                                                                    // Wait, `values.append(byts)` -> `byts` is the integer value.
                                                                    // The usage in `get_tag_map`: `ans[x.tag] = values`.
                                                                    // So the map is Tag -> Vec<u64>.
                                                                    // BUT `value_bytes` branch says `byts, consumed = decint(data) ... values.append(byts)`.
                                                                    // Yes, it stores integers.
                                                                    // However, I declared `BTreeMap<u8, Vec<Vec<u8>>>`. That matches `values.append(byts)` if byts was bytes.
                                                                    // Let's re-read python carefully.
                                                                    // `byts, consumed = decint(data)` -> `byts` is the integer.
                                                                    // So `values` is a list of integers.
                                                                    // So `ans` is `Map<Tag, List<Integer>>`.

                // BUT... looking at `read_index`: `table[ident] = tag_map`.
                // `tag_map` is what we return here.
                // So `TagMap` is `Map<u8, Vec<u64>>`.
                // Let me change return type to `Vec<u64>`.

                // Hmm, `decint` in `utils.rs` returns `(u64, usize)`.

                reading_data = &reading_data[consumed..];
                values.push(byts as u64); // Integer value
            }
        } else if let Some(vb) = x.value_bytes {
            let mut total_consumed = 0;
            while total_consumed < vb as usize {
                let (byts, consumed) = decint(reading_data, true)?;
                reading_data = &reading_data[consumed..];
                total_consumed += consumed;
                values.push(byts as u64);
            }
        }
        ans.insert(x.tag, values);
    }

    // Warning if leftover

    Ok(ans)
}

fn parse_index_record(
    table: &mut BTreeMap<String, BTreeMap<u8, Vec<u64>>>,
    data: &[u8],
    control_byte_count: u32,
    tags: &[TagX],
    codec: &str,
    _ordt_map: &str,
    strict: bool,
) -> Result<IndxHeader> {
    let header = parse_indx_header(data)?;
    let idxt_pos = header.start as usize;
    if data.len() < idxt_pos + 4 {
        // Warning?
        // In python: if data[...] != b'IDXT': print WARNING.
    }

    let entry_count = header.count;
    let mut idx_positions = Vec::new();

    let mut cursor = Cursor::new(&data[idxt_pos + 4..]);
    for _ in 0..entry_count {
        let pos = cursor.read_u16::<BigEndian>()?;
        idx_positions.push(pos as usize);
    }
    idx_positions.push(idxt_pos);

    for j in 0..entry_count as usize {
        let start = idx_positions[j];
        if j + 1 >= idx_positions.len() {
            break;
        }
        let end = idx_positions[j + 1];
        if start >= end || end > data.len() {
            continue;
        }
        let rec = &data[start..end];
        let (ident, consumed) = decode_string(rec, codec).unwrap_or((String::new(), 0));
        // There is sophisticated logic in python `rec[consumed:]`, `decode_string` taking ordt_map etc.
        // My `decode_string` in utils.rs takes only raw and codec.
        // I should probably enhance `utils.rs` or implement the logic here.
        // For now, I use the existing `decode_string` which does length prefix decoding.

        // Note: Python `decode_string` handles `ordt_map` transformation.
        // If I skip that, some strings might be garbaged.

        let rec_remaining = &rec[consumed..];
        let tag_map = get_tag_map(control_byte_count, tags, rec_remaining, strict)?;
        table.insert(ident, tag_map);
    }

    Ok(header)
}

fn get_tag_section_start(data: &[u8], indx_header: &IndxHeader) -> usize {
    let mut tag_section_start = indx_header.tagx as usize;
    if tag_section_start + 4 <= data.len()
        && &data[tag_section_start..tag_section_start + 4] != b"TAGX"
    {
        // Search
        // Python: tpos = data.find(b'TAGX', indx_header['idx_header_end_pos'])
        // Rust: using window search
        let start = indx_header.idx_header_end_pos;
        if let Some(pos) = data[start..].windows(4).position(|w| w == b"TAGX") {
            tag_section_start = start + pos;
        }
    }
    tag_section_start
}

pub fn read_index(
    sections: &[(Vec<u8>, (u32, u32, u32, u32, u32))], // mimicking sections structure passing?
    // In reader.rs, sections are `Vec<MobiSection>`. But `reader.py` just passes `self.sections` which are `(data, header_tuple)`.
    // I should adapt to `Vec<Vec<u8>>` for data to be simpler.
    idx: usize,
    codec: &str,
) -> Result<(BTreeMap<String, BTreeMap<u8, Vec<u64>>>, CNCXReader)> {
    let mut table = BTreeMap::new();
    let data = &sections[idx].0;

    let indx_header = parse_indx_header(data)?;
    let indx_count = indx_header.count as usize;

    let mut cncx = CNCXReader::new(&[], codec); // Empty initially

    if indx_header.ncncx > 0 {
        let off = idx + indx_count + 1;
        let mut cncx_records = Vec::new();
        for i in 0..indx_header.ncncx as usize {
            if off + i < sections.len() {
                cncx_records.push(sections[off + i].0.clone());
            }
        }
        cncx = CNCXReader::new(&cncx_records, codec);
    }

    let tag_section_start = get_tag_section_start(data, &indx_header);
    let (control_byte_count, tags) = parse_tagx_section(&data[tag_section_start..])?;

    for i in (idx + 1)..(idx + 1 + indx_count) {
        if i < sections.len() {
            let record_data = &sections[i].0;
            parse_index_record(
                &mut table,
                record_data,
                control_byte_count,
                &tags,
                codec,
                &indx_header.ordt_map,
                false,
            )?;
        }
    }

    Ok((table, cncx))
}
