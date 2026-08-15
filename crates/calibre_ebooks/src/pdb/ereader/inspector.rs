//! Port of `calibre.ebooks.pdb.ereader.inspector`: dump an eReader
//! file's PDB header and record-0 header fields, primarily for
//! debugging. Mirrors this crate's `mobi::debug` precedent (issue #32):
//! a library function that formats a report, not a standalone binary --
//! callers (a CLI, a test, a future `debug_ereader` bin) decide what to
//! do with the string.

use std::fmt::Write as _;
use std::io::{Read, Seek};
use std::path::Path;

use anyhow::{Context, Result};
use byteorder::{BigEndian, ByteOrder};

use crate::pdb::ereader::EreaderError;
use crate::pdb::header::PdbHeader;

fn u16_at(raw: &[u8], offset: usize) -> Option<u16> {
    if raw.len() < offset + 2 {
        return None;
    }
    Some(BigEndian::read_u16(&raw[offset..offset + 2]))
}

fn fmt_u16(raw: &[u8], offset: usize) -> String {
    match u16_at(raw, offset) {
        Some(v) => v.to_string(),
        None => "<truncated>".to_string(),
    }
}

/// Port of `pdb_header_info`.
pub fn pdb_header_info(header: &PdbHeader) -> String {
    let mut ident = String::new();
    ident.push_str(&String::from_utf8_lossy(&header.type_id));
    ident.push_str(&String::from_utf8_lossy(&header.creator_id));

    let mut out = String::new();
    let _ = writeln!(out, "PDB Header Info:");
    let _ = writeln!(out);
    let _ = writeln!(out, "Identity:        {ident}");
    let _ = writeln!(out, "Total Sections:   {}", header.num_records);
    let _ = writeln!(out, "Title:           {}", header.name);
    let _ = writeln!(out);
    out
}

/// Port of `ereader_header_info132`.
pub fn ereader_header_info132(h0: &[u8]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Ereader Record 0 (Header) Info:");
    let _ = writeln!(out);
    let _ = writeln!(out, "0-2 Version:             {}", fmt_u16(h0, 0));
    let _ = writeln!(out, "2-4:                     {}", fmt_u16(h0, 2));
    let _ = writeln!(out, "4-6:                     {}", fmt_u16(h0, 4));
    let _ = writeln!(out, "6-8 Codepage:            {}", fmt_u16(h0, 6));
    let _ = writeln!(out, "8-10:                    {}", fmt_u16(h0, 8));
    let _ = writeln!(out, "10-12:                   {}", fmt_u16(h0, 10));
    let _ = writeln!(out, "12-14 Non-Text offset:   {}", fmt_u16(h0, 12));
    let _ = writeln!(out, "14-16:                   {}", fmt_u16(h0, 14));
    let _ = writeln!(out, "16-18:                   {}", fmt_u16(h0, 16));
    let _ = writeln!(out, "18-20:                   {}", fmt_u16(h0, 18));
    let _ = writeln!(out, "20-22 Image Count:       {}", fmt_u16(h0, 20));
    let _ = writeln!(out, "22-24:                   {}", fmt_u16(h0, 22));
    let _ = writeln!(out, "24-26 Has Metadata?:     {}", fmt_u16(h0, 24));
    let _ = writeln!(out, "26-28:                   {}", fmt_u16(h0, 26));
    let _ = writeln!(out, "28-30 Footnote Count:    {}", fmt_u16(h0, 28));
    let _ = writeln!(out, "30-32 Sidebar Count:     {}", fmt_u16(h0, 30));
    let _ = writeln!(out, "32-34 Bookmark Offset:   {}", fmt_u16(h0, 32));
    let _ = writeln!(out, "34-36 MAGIC:             {}", fmt_u16(h0, 34));
    let _ = writeln!(out, "36-38:                   {}", fmt_u16(h0, 36));
    let _ = writeln!(out, "38-40:                   {}", fmt_u16(h0, 38));
    let _ = writeln!(out, "40-42 Image Data Offset: {}", fmt_u16(h0, 40));
    let _ = writeln!(out, "42-44:                   {}", fmt_u16(h0, 42));
    let _ = writeln!(out, "44-46 Metadata Offset:   {}", fmt_u16(h0, 44));
    let _ = writeln!(out, "46-48:                   {}", fmt_u16(h0, 46));
    let _ = writeln!(out, "48-50 Footnote Offset:   {}", fmt_u16(h0, 48));
    let _ = writeln!(out, "50-52 Sidebar Offset:    {}", fmt_u16(h0, 50));
    let _ = writeln!(out, "52-54 Last Data Offset:  {}", fmt_u16(h0, 52));

    let mut i = 54;
    while i < 131 {
        let _ = writeln!(out, "{i}-{}:                   {}", i + 2, fmt_u16(h0, i));
        i += 2;
    }
    let _ = writeln!(out);
    out
}

/// Port of `ereader_header_info202`.
pub fn ereader_header_info202(h0: &[u8]) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Ereader Record 0 (Header) Info:");
    let _ = writeln!(out);
    let _ = writeln!(out, "0-2 Version:             {}", fmt_u16(h0, 0));
    let _ = writeln!(out, "2-4 Garbage:             {}", fmt_u16(h0, 2));
    let _ = writeln!(out, "4-6 Garbage:             {}", fmt_u16(h0, 4));
    let _ = writeln!(out, "6-8 Garbage:             {}", fmt_u16(h0, 6));
    let _ = writeln!(out, "8-10 Non-Text Offset:    {}", fmt_u16(h0, 8));
    let _ = writeln!(out, "10-12:                   {}", fmt_u16(h0, 10));
    let _ = writeln!(out, "12-14:                   {}", fmt_u16(h0, 12));
    let _ = writeln!(out, "14-16 Garbage:           {}", fmt_u16(h0, 14));
    let _ = writeln!(out, "16-18 Garbage:           {}", fmt_u16(h0, 16));
    let _ = writeln!(out, "18-20 Garbage:           {}", fmt_u16(h0, 18));
    let _ = writeln!(out, "20-22 Garbage:           {}", fmt_u16(h0, 20));
    let _ = writeln!(out, "22-24 Garbage:           {}", fmt_u16(h0, 22));
    let _ = writeln!(out, "24-26:                   {}", fmt_u16(h0, 24));
    let _ = writeln!(out, "26-28:                   {}", fmt_u16(h0, 26));

    let mut i = 28;
    while i < 98 {
        let _ = writeln!(out, "{i}-{} Garbage:           {}", i + 2, fmt_u16(h0, i));
        i += 2;
    }
    let _ = writeln!(out, "98-100:                  {}", fmt_u16(h0, 98));

    let mut i = 100;
    while i < 110 {
        let _ = writeln!(out, "{i}-{} Garbage:         {}", i + 2, fmt_u16(h0, i));
        i += 2;
    }
    let _ = writeln!(out, "110-112:                 {}", fmt_u16(h0, 110));
    let _ = writeln!(out, "112-114:                 {}", fmt_u16(h0, 112));
    let _ = writeln!(out, "114-116 Garbage:         {}", fmt_u16(h0, 114));

    let mut i = 116;
    while i < 202 {
        let _ = writeln!(out, "{i}-{}:                 {}", i + 2, fmt_u16(h0, i));
        i += 2;
    }

    let _ = writeln!(out);
    let _ = writeln!(out, "* Garbage: Random values.");
    let _ = writeln!(out);
    out
}

/// Port of `ereader_header_info`.
pub fn ereader_header_info<R: Read + Seek>(header: &PdbHeader, stream: &mut R) -> Result<String> {
    let h0 = header.section_data(stream, 0)?;

    let mut out = String::new();
    let _ = writeln!(out, "Header Size:     {}", h0.len());

    match h0.len() {
        132 => {
            let _ = writeln!(out, "Header Type:     Dropbook compatible");
            let _ = writeln!(out);
            out.push_str(&ereader_header_info132(&h0));
        }
        202 => {
            let _ = writeln!(out, "Header Type:     Makebook compatible");
            let _ = writeln!(out);
            out.push_str(&ereader_header_info202(&h0));
        }
        other => {
            return Err(EreaderError::msg(format!(
                "Size mismatch. eReader header record size {other} KB is not supported."
            ))
            .into());
        }
    }

    Ok(out)
}

/// Port of `section_lengths`.
pub fn section_lengths<R: Read + Seek>(header: &PdbHeader, stream: &mut R) -> Result<String> {
    let mut out = String::new();
    let _ = writeln!(out, "Section Sizes");
    let _ = writeln!(out);

    for i in 0..header.records.len() {
        let size = header.section_data(stream, i)?.len();
        let message = if size > 65505 { "<--- Over!" } else { "" };
        let _ = writeln!(out, "Section {i}:   {size} {message}");
    }

    Ok(out)
}

/// Port of `main`: read `path`, print the PDB header, eReader record-0
/// header, and per-section sizes.
pub fn inspect_ereader_file(path: &Path) -> Result<String> {
    let mut file =
        std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let header = PdbHeader::parse(&mut file).context("parsing PDB header")?;

    let mut out = String::new();
    out.push_str(&pdb_header_info(&header));
    out.push_str(&ereader_header_info(&header, &mut file)?);
    out.push_str(&section_lengths(&header, &mut file)?);

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use std::io::{Cursor, Write};

    fn build_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut buffer = Vec::new();
        let mut name = b"Inspector Book".to_vec();
        name.resize(32, 0);
        buffer.extend_from_slice(&name);
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.extend_from_slice(b"PNRd");
        buffer.extend_from_slice(b"PPrs");
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(records.len() as u16).unwrap();

        let base_offset = 78 + (records.len() as u32 * 8) + 2;
        let mut offset = base_offset;
        for r in &records {
            buffer.write_u32::<BigEndian>(offset).unwrap();
            buffer.write_all(&[0u8, 0, 0, 0]).unwrap();
            offset += r.len() as u32;
        }
        buffer.write_u16::<BigEndian>(0).unwrap();
        for r in &records {
            buffer.extend_from_slice(r);
        }
        buffer
    }

    #[test]
    fn pdb_header_info_reports_identity_and_title() {
        let raw = build_pdb(vec![vec![0u8; 132]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let out = pdb_header_info(&header);
        assert!(out.contains("PNRdPPrs"), "{out}");
        assert!(out.contains("Inspector Book"), "{out}");
        assert!(out.contains("Total Sections:   1"), "{out}");
    }

    #[test]
    fn ereader_header_info_detects_132_byte_dropbook_header() {
        let raw = build_pdb(vec![vec![0u8; 132]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let out = ereader_header_info(&header, &mut cursor).unwrap();
        assert!(out.contains("Dropbook compatible"), "{out}");
        assert!(out.contains("Header Size:     132"), "{out}");
    }

    #[test]
    fn ereader_header_info_detects_202_byte_makebook_header() {
        let raw = build_pdb(vec![vec![0u8; 202]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let out = ereader_header_info(&header, &mut cursor).unwrap();
        assert!(out.contains("Makebook compatible"), "{out}");
    }

    #[test]
    fn ereader_header_info_rejects_unsupported_size() {
        let raw = build_pdb(vec![vec![0u8; 40]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let err = ereader_header_info(&header, &mut cursor).unwrap_err();
        assert!(err.to_string().contains("Size mismatch"), "{err}");
    }

    #[test]
    fn section_lengths_flags_oversized_sections() {
        let raw = build_pdb(vec![vec![0u8; 132], vec![1u8; 70000]]);
        let mut cursor = Cursor::new(raw);
        let header = PdbHeader::parse(&mut cursor).unwrap();
        let out = section_lengths(&header, &mut cursor).unwrap();
        assert!(out.contains("Section 1:   70000 <--- Over!"), "{out}");
    }

    #[test]
    fn inspect_ereader_file_reads_from_disk() {
        let raw = build_pdb(vec![vec![0u8; 132]]);
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("book.pdb");
        std::fs::write(&path, raw).unwrap();
        let out = inspect_ereader_file(&path).unwrap();
        assert!(out.contains("PDB Header Info"), "{out}");
        assert!(out.contains("Dropbook compatible"), "{out}");
        assert!(out.contains("Section Sizes"), "{out}");
    }
}
