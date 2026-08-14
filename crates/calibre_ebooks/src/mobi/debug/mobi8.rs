//! MOBI8/KF8 structural dump: FDST, SKEL/SECT/GUIDE/NCX, containers,
//! reassembled files.
//!
//! Port of `src/calibre/ebooks/mobi/debug/mobi8.py`.
//!
//! One piece is deliberately not reproduced: `read_tbs`'s
//! cross-check against `calculate_all_tbs`/`sequences_to_bytes` from
//! `calibre.ebooks.mobi.writer8.tbs` — that module (`mobi/writer8/`)
//! isn't ported yet (tracked separately in `docs/modules_to_port.md`,
//! entirely unchecked), so re-deriving the TBS bytes the writer
//! *would* produce and diffing them against what's actually in the
//! file is genuinely blocked on that dependency. What this port does
//! instead: decode and show the TBS bytes actually present (the
//! `decode_tbs` sequence walk, which needs nothing from `writer8`),
//! which is the half of `read_tbs` a developer inspecting a specific
//! file's indexing data cares about; the mismatch-detection half
//! becomes available once `writer8/tbs.py` is ported.

use std::fmt;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::mobi::debug::containers::ContainerHeader;
use crate::mobi::debug::format_bytes;
use crate::mobi::debug::index::{GuideIndex, NcxIndex, SectIndex, SkelIndex};
use crate::mobi::headers::NULL_INDEX;
use crate::mobi::utils::{decode_tbs, read_font_record, DEFAULT_FONT_XOR_EXTENT, RECORD_SIZE};

use super::headers::{MobiFile as RawMobiFile, TextRecord};

fn be_u32(b: &[u8]) -> Result<u32> {
    b.get(..4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .context("truncated u32")
}

/// The `FDST` record: the byte ranges each "flow" (raw text section)
/// occupies in the reassembled text. `FDST` in the Python.
pub struct Fdst {
    pub sec_off: u32,
    pub num_sections: u32,
    pub sections: Vec<(u32, u32)>,
}

impl Fdst {
    /// `FDST.__init__`.
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if !raw.starts_with(b"FDST") {
            bail!("KF8 does not have a valid FDST record");
        }
        let sec_off = be_u32(&raw[4..8])?;
        let num_sections = be_u32(&raw[8..12])?;
        if sec_off != 12 {
            bail!("FDST record has unknown extra fields");
        }
        let mut sections = Vec::with_capacity(num_sections as usize);
        let mut pos = sec_off as usize;
        for _ in 0..num_sections {
            let start = be_u32(&raw[pos..pos + 4])?;
            let end = be_u32(&raw[pos + 4..pos + 8])?;
            sections.push((start, end));
            pos += 8;
        }
        if pos != raw.len() {
            bail!(
                "FDST record has trailing data: {}",
                format_bytes(&raw[pos..])
            );
        }
        Ok(Fdst {
            sec_off,
            num_sections,
            sections,
        })
    }
}

impl fmt::Display for Fdst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "FDST record")?;
        writeln!(f, "Offset to sections: {}", self.sec_off)?;
        writeln!(f, "Number of section records: {}", self.num_sections)?;
        writeln!(f, "**** {} Sections ****", self.sections.len())?;
        for (i, (start, end)) in self.sections.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            write!(f, "Start: {start:>20} End: {end}")?;
        }
        Ok(())
    }
}

/// One reassembled HTML part (a chapter/section skeleton with its
/// SECT chunks spliced in). `File` in the Python.
pub struct KfFile {
    pub name: String,
    pub skeleton: Vec<u8>,
    pub text: Vec<u8>,
    pub first_aid: String,
    pub sections: Vec<Vec<u8>>,
}

impl KfFile {
    /// `File.dump`.
    pub fn dump(&self, ddir: &Path) -> Result<()> {
        std::fs::write(ddir.join(format!("{}.html", self.name)), &self.text)?;
        let base = ddir.join(format!("{}-parts", self.name));
        std::fs::create_dir_all(&base)?;
        std::fs::write(base.join("skeleton.html"), &self.skeleton)?;
        for (i, text) in self.sections.iter().enumerate() {
            std::fs::write(base.join(format!("sect-{i:04}.html")), text)?;
        }
        Ok(())
    }
}

/// One decoded-and-verified TBS sequence for a single text record.
/// The non-`writer8`-dependent half of `read_tbs` in the Python.
pub struct TbsSequence {
    pub value: u64,
    pub extra: std::collections::HashMap<String, u64>,
}

/// The whole parsed MOBI8/KF8 structure the debug tool dumps.
/// `MOBIFile` in `debug/mobi8.py`.
pub struct MobiFile {
    pub text_records: Vec<TextRecord>,
    pub raw_text: Vec<u8>,
    pub fdst: Option<Fdst>,
    pub skel_index: SkelIndex,
    pub sect_index: SectIndex,
    pub ncx_index: NcxIndex,
    pub guide_index: GuideIndex,
    pub files: Vec<KfFile>,
    pub resource_map: Vec<(String, Vec<u8>)>,
    pub containers: Vec<ContainerHeader>,
    pub tbs_sequences: Vec<(u32, Vec<TbsSequence>, Vec<u8>)>,
    header_dump: String,
}

const KNOWN_RESOURCE_TYPES: [&[u8; 4]; 14] = [
    b"FLIS", b"FCIS", b"SRCS", b"RESC", b"BOUN", b"FDST", b"DATP", b"AUDI", b"VIDE", b"CRES",
    b"CONT", b"CMET", b"PAGE", b"HUFF",
];

impl MobiFile {
    /// `MOBIFile.__init__`.
    pub fn new(mut mf: RawMobiFile) -> Result<Self> {
        let h = &mf.mobi_header;
        let h8 = &mf.mobi8_header;
        let mut resource_ranges = vec![(
            h8.first_resource_record,
            h8.last_resource_record,
            Some(h8.first_image_index),
        )];
        let mut offset = 0u32;
        if mf.kf8_type == Some("joint") {
            offset = h
                .exth
                .as_ref()
                .and_then(|e| e.kf8_header_index())
                .unwrap_or(0);
            resource_ranges.insert(
                0,
                (
                    h.first_resource_record,
                    h.last_resource_record,
                    Some(h.first_image_index),
                ),
            );
        }

        let first_text_record = 1u32;
        let ntr = u32::from(mf.mobi8_header.number_of_text_records);
        let mut text_records = Vec::new();
        let start = first_text_record + offset;
        for i in 0..ntr {
            let idx = start + i;
            if idx as usize >= mf.records.len() {
                break;
            }
            let extra = mf.mobi8_header.extra_data_flags;
            let raw = mf.records[idx as usize].raw.clone();
            let decompressed = mf.decompress_text8(&raw)?;
            text_records.push(TextRecord::new(i, &raw, extra, decompressed)?);
        }
        let raw_text: Vec<u8> = text_records.iter().flat_map(|r| r.raw.clone()).collect();

        let header_dump = {
            let mut out = String::new();
            out.push_str(&mf.palmdb.to_string());
            out.push_str("\n\nRecord headers:\n");
            for (i, r) in mf.records.iter().enumerate() {
                out.push_str(&format!("{i:>6}. {}\n", r.header_line()));
            }
            out.push('\n');
            out.push_str(&mf.mobi8_header.to_string());
            out
        };

        let mut fdst = None;
        if mf.mobi8_header.fdst_idx != NULL_INDEX {
            let idx = mf.mobi8_header.fdst_idx as usize;
            let f = Fdst::parse(&mf.records[idx].raw)?;
            if f.num_sections != mf.mobi8_header.fdst_count {
                bail!("KF8 Header contains invalid FDST count");
            }
            fdst = Some(f);
        }

        let codec = mf.mobi8_header.encoding.clone();
        let sections: Vec<Vec<u8>> = mf.records.iter().map(|r| r.raw.clone()).collect();
        let skel_index = SkelIndex::read(mf.mobi8_header.skel_idx, &sections, &codec)?;
        let sect_index = SectIndex::read(mf.mobi8_header.sect_idx, &sections, &codec)?;
        let ncx_index = NcxIndex::read(mf.mobi8_header.primary_index_record, &sections, &codec)?;
        let guide_index = GuideIndex::read(mf.mobi8_header.oth_idx, &sections, &codec)?;

        let files = build_files(&raw_text, &skel_index, &sect_index);

        let (resource_map, containers) = extract_resources(&mf.records, &resource_ranges)?;

        let tbs_sequences = decode_tbs_sequences(&text_records);

        Ok(MobiFile {
            text_records,
            raw_text,
            fdst,
            skel_index,
            sect_index,
            ncx_index,
            guide_index,
            files,
            resource_map,
            containers,
            tbs_sequences,
            header_dump,
        })
    }

    pub fn header_dump(&self) -> &str {
        &self.header_dump
    }

    /// `MOBIFile.dump_flows`.
    pub fn dump_flows(&self, ddir: &Path) -> Result<()> {
        let boundaries: Vec<(u32, u32)> = match &self.fdst {
            Some(f) => f.sections.clone(),
            None => vec![(0, self.raw_text.len() as u32)],
        };
        for (i, (start, end)) in boundaries.iter().enumerate() {
            let start = *start as usize;
            let end = (*end as usize).min(self.raw_text.len());
            std::fs::write(
                ddir.join(format!("flow{i:04}.txt")),
                &self.raw_text[start.min(end)..end],
            )?;
        }
        Ok(())
    }
}

/// `MOBIFile.build_files`.
fn build_files(text: &[u8], skel_index: &SkelIndex, sect_index: &SectIndex) -> Vec<KfFile> {
    let mut files = Vec::new();
    for skel in &skel_index.records {
        let sects: Vec<&crate::mobi::debug::index::SectElem> = sect_index
            .records
            .iter()
            .filter(|s| s.file_number == u64::from(skel.file_number))
            .collect();
        let start = skel.start_position as usize;
        let end = (start + skel.length as usize).min(text.len());
        let skeleton = text.get(start..end).unwrap_or(&[]).to_vec();
        let mut ftext = skeleton.clone();
        let first_aid = sects
            .first()
            .map(|s| s.toc_text.clone())
            .unwrap_or_default();
        let mut sections = Vec::new();

        // Insertions must be applied in a stable order to keep offsets
        // meaningful, exactly as the Python's sequential splice does.
        for sect in &sects {
            let start_pos =
                skel.start_position as usize + skel.length as usize + sect.start_pos as usize;
            let sect_end = (start_pos + sect.length as usize).min(text.len());
            let sect_text = text.get(start_pos..sect_end).unwrap_or(&[]).to_vec();
            let insert_pos = (sect.insert_pos as i64 - skel.start_position as i64).max(0) as usize;
            let insert_pos = insert_pos.min(ftext.len());
            let mut spliced = ftext[..insert_pos].to_vec();
            spliced.extend_from_slice(&sect_text);
            spliced.extend_from_slice(&ftext[insert_pos..]);
            ftext = spliced;
            sections.push(sect_text);
        }

        files.push(KfFile {
            name: format!("part{:04}", skel.file_number),
            skeleton,
            text: ftext,
            first_aid,
            sections,
        });
    }
    files
}

/// `MOBIFile.extract_resources`.
fn extract_resources(
    records: &[super::headers::Record],
    resource_ranges: &[(u32, u32, Option<u32>)],
) -> Result<(Vec<(String, Vec<u8>)>, Vec<ContainerHeader>)> {
    let mut resource_map = Vec::new();
    let mut containers: Vec<ContainerHeader> = Vec::new();
    let mut current: Option<ContainerHeader> = None;

    for (i, rec) in records.iter().enumerate() {
        let i = i as u32;
        let in_range = resource_ranges.iter().find(|(l, r, _)| *l <= i && i <= *r);
        let Some((_, _, image_offset)) = in_range else {
            continue;
        };
        let mut resource_index = i + 1;
        if let Some(off) = image_offset {
            if resource_index >= *off {
                resource_index -= off;
            }
        }

        let sig = &rec.raw[..rec.raw.len().min(4)];
        let mut payload = rec.raw.clone();
        let mut ext = "dat".to_string();
        let mut prefix = "binary".to_string();
        let mut suffix = String::new();

        if matches!(sig, b"HUFF" | b"CDIC" | b"INDX") {
            continue;
        }
        if sig == b"FONT" {
            let font = read_font_record(&rec.raw, DEFAULT_FONT_XOR_EXTENT);
            if let Some(err) = &font.err {
                bail!("Failed to read font record: {err}");
            }
            payload = font.font_data.unwrap_or(font.raw_data);
            prefix = "fonts".to_string();
            ext = font.ext.to_string();
        } else if sig == b"CONT" {
            if payload == b"CONTBOUNDARY" {
                if let Some(c) = current.take() {
                    containers.push(c);
                }
                continue;
            }
            current = Some(ContainerHeader::parse(&payload)?);
            continue;
        } else if sig == b"CRES" {
            if let Some(c) = current.as_mut() {
                let is_image_container = c.is_image_container;
                c.resources.push(Some(payload.clone()));
                if is_image_container {
                    let img_payload = &payload[12.min(payload.len())..];
                    if let Some(q) = calibre_utils::imghdr::what(img_payload) {
                        prefix = "hd-images".to_string();
                        ext = q.to_string();
                        payload = img_payload.to_vec();
                        resource_index = c.resources.len() as u32;
                    }
                }
            }
        } else if sig == b"\xa0\xa0\xa0\xa0" && payload.len() == 4 {
            if let Some(c) = current.as_mut() {
                c.resources.push(None);
            }
            continue;
        } else if !KNOWN_RESOURCE_TYPES.contains(&sig.try_into().unwrap_or(b"\0\0\0\0")) {
            if let Some(c) = current.as_mut() {
                if c.resources.len() == c.num_of_resource_records as usize {
                    c.add_hrefs(&payload);
                    continue;
                }
            }
            if let Some(q) = calibre_utils::imghdr::what(&rec.raw) {
                prefix = "images".to_string();
                ext = q.to_string();
            }
        }

        if prefix == "binary" {
            if sig == b"\xe9\x8e\r\n" {
                suffix = "-EOF".to_string();
            } else if sig.len() == 4 && KNOWN_RESOURCE_TYPES.contains(&sig.try_into().unwrap()) {
                suffix = format!("-{}", String::from_utf8_lossy(sig));
            }
        }

        resource_map.push((
            format!("{prefix}/{resource_index:06}{suffix}.{ext}"),
            payload,
        ));
    }
    if let Some(c) = current {
        containers.push(c);
    }
    Ok((resource_map, containers))
}

/// The non-`writer8`-dependent half of `MOBIFile.read_tbs`: walk each
/// text record's `indexing` trailing bytes as a sequence of TBS
/// values, without cross-checking against a re-derived encoding.
fn decode_tbs_sequences(text_records: &[TextRecord]) -> Vec<(u32, Vec<TbsSequence>, Vec<u8>)> {
    let mut out = Vec::new();
    for rec in text_records {
        let mut bytes = rec
            .trailing_data
            .get("indexing")
            .cloned()
            .unwrap_or_default();
        let mut sequences = Vec::new();
        let mut flag_size = 3u32;
        while !bytes.is_empty() {
            match decode_tbs(&bytes, flag_size) {
                Ok((val, extra, consumed)) => {
                    flag_size = 4;
                    bytes = bytes[consumed..].to_vec();
                    let extra = extra
                        .into_iter()
                        .map(|(k, v)| (format!("{k:b}"), v))
                        .collect();
                    sequences.push(TbsSequence { value: val, extra });
                }
                Err(_) => break,
            }
        }
        out.push((rec.idx, sequences, bytes));
    }
    out
}

/// `inspect_mobi` in `debug/mobi8.py`.
pub fn inspect_mobi(mf: RawMobiFile, ddir: &Path) -> Result<()> {
    let f = MobiFile::new(mf)?;
    std::fs::write(ddir.join("header.txt"), f.header_dump())?;
    std::fs::write(ddir.join("raw_text.html"), &f.raw_text)?;

    for x in [
        "text_records",
        "images",
        "fonts",
        "binary",
        "files",
        "flows",
        "hd-images",
    ] {
        std::fs::create_dir_all(ddir.join(x))?;
    }

    for rec in &f.text_records {
        rec.dump(&ddir.join("text_records"))?;
    }

    for (href, payload) in &f.resource_map {
        let path = ddir.join(href);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, payload)?;
    }

    for (i, container) in f.containers.iter().enumerate() {
        std::fs::write(
            ddir.join(format!("container{}.txt", i + 1)),
            container.to_string(),
        )?;
    }

    if let Some(fdst) = &f.fdst {
        std::fs::write(ddir.join("fdst.record"), fdst.to_string())?;
    }

    std::fs::write(ddir.join("skel.record"), f.skel_index.to_string())?;
    std::fs::write(ddir.join("chunks.record"), f.sect_index.to_string())?;
    std::fs::write(ddir.join("ncx.record"), f.ncx_index.to_string())?;
    std::fs::write(ddir.join("guide.record"), f.guide_index.to_string())?;

    let mut tbs_out = String::new();
    tbs_out.push_str(
        "Index Entry lines are of the form:\n\
         depth:index_number [action] parent (index_num-parent) Geometry\n\n\
         Where Geometry is the start and end of the index entry w.r.t\n\
         the start of the text record.\n\n\
         Note: this port shows the TBS bytes actually present in each\n\
         record and their decoded sequence values; it does not\n\
         re-derive and diff against calculated TBS bytes (see the\n\
         module doc comment for why).\n\n",
    );
    for (idx, sequences, remaining) in &f.tbs_sequences {
        tbs_out.push_str(&format!("Record #{idx}\n"));
        for (j, seq) in sequences.iter().enumerate() {
            tbs_out.push_str(&format!("Sequence #{j}: {} {:?}\n", seq.value, seq.extra));
        }
        if !remaining.is_empty() {
            tbs_out.push_str(&format!("Remaining bytes: {}\n", format_bytes(remaining)));
        }
        tbs_out.push('\n');
    }
    std::fs::write(ddir.join("tbs.txt"), tbs_out)?;

    for part in &f.files {
        part.dump(&ddir.join("files"))?;
    }

    f.dump_flows(&ddir.join("flows"))?;

    Ok(())
}

/// Text record size, re-exported for callers building offset math the
/// way `mobi8.py`'s `read_tbs` does (`e.start - i*RECORD_SIZE`).
pub const TEXT_RECORD_SIZE: usize = RECORD_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fdst_parses_a_minimal_record() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"FDST");
        raw.extend_from_slice(&12u32.to_be_bytes());
        raw.extend_from_slice(&2u32.to_be_bytes());
        raw.extend_from_slice(&0u32.to_be_bytes());
        raw.extend_from_slice(&100u32.to_be_bytes());
        raw.extend_from_slice(&100u32.to_be_bytes());
        raw.extend_from_slice(&250u32.to_be_bytes());
        let fdst = Fdst::parse(&raw).expect("parses");
        assert_eq!(fdst.sections, vec![(0, 100), (100, 250)]);
    }

    #[test]
    fn fdst_rejects_a_bad_signature() {
        assert!(Fdst::parse(b"NOPE").is_err());
    }

    #[test]
    fn fdst_rejects_trailing_data() {
        let mut raw = Vec::new();
        raw.extend_from_slice(b"FDST");
        raw.extend_from_slice(&12u32.to_be_bytes());
        raw.extend_from_slice(&1u32.to_be_bytes());
        raw.extend_from_slice(&0u32.to_be_bytes());
        raw.extend_from_slice(&10u32.to_be_bytes());
        raw.push(0xff); // trailing byte
        assert!(Fdst::parse(&raw).is_err());
    }

    #[test]
    fn text_record_size_matches_utils() {
        assert_eq!(TEXT_RECORD_SIZE, RECORD_SIZE);
    }
}
