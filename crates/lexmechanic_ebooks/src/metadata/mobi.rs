use crate::metadata::MetaInformation;
use crate::pdb::header::PdbHeader;
use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub fn get_metadata<R: Read + Seek>(mut stream: R) -> Result<MetaInformation> {
    let pdb = PdbHeader::parse(&mut stream)?;

    if pdb.num_records == 0 {
        bail!("No records in PDB");
    }

    // Record 0 contains PalmDOC Header + MOBI Header + EXTH Header
    let rec0_offset = pdb.records[0].offset as u64;
    stream.seek(SeekFrom::Start(rec0_offset))?;

    // Read header buffer (up to 512 bytes to cover EXTH start)
    let mut header_buf = [0u8; 512];
    let bytes_read = stream.read(&mut header_buf)?;

    // Check minimal size (PalmDOC 16 + MOBI 4 + Len 4 = 24)
    if bytes_read < 24 {
        bail!("MOBI header too short");
    }

    // Offset 16: Signature
    if &header_buf[16..20] != b"MOBI" {
        bail!("Invalid MOBI signature");
    }

    // Offset 20: MOBI Header Length
    let mobi_header_len = (&header_buf[20..24]).read_u32::<BigEndian>()?;

    let mut mi = MetaInformation::default();
    mi.title = pdb.name.clone();

    // EXTH Flag at 128 (0x80)
    // Offset relative to Rec0 start.
    // Check if we read enough
    let has_exth = if bytes_read >= 132 {
        let flags = (&header_buf[128..132]).read_u32::<BigEndian>()?;
        (flags & 0x40) != 0
    } else {
        // Fallback seek
        stream.seek(SeekFrom::Start(rec0_offset + 128))?;
        let flags = stream.read_u32::<BigEndian>()?;
        (flags & 0x40) != 0
    };

    // First Image Index at 108
    let first_image_index = if bytes_read >= 112 {
        (&header_buf[108..112]).read_u32::<BigEndian>()?
    } else {
        stream.seek(SeekFrom::Start(rec0_offset + 108))?;
        stream.read_u32::<BigEndian>()?
    };

    if has_exth {
        // EXTH starts after MOBI header (16 + len)
        let exth_offset = rec0_offset + 16 + mobi_header_len as u64;
        stream.seek(SeekFrom::Start(exth_offset))?;

        let mut exth_sig = [0u8; 4];
        stream.read_exact(&mut exth_sig)?;
        if &exth_sig == b"EXTH" {
            let _len = stream.read_u32::<BigEndian>()?;
            let count = stream.read_u32::<BigEndian>()?;

            for _ in 0..count {
                let id = stream.read_u32::<BigEndian>()?;
                let size = stream.read_u32::<BigEndian>()?;
                if size < 8 {
                    break;
                }
                let data_len = size - 8;
                let mut data = vec![0u8; data_len as usize];
                stream.read_exact(&mut data)?;

                // Bytes are decoded lossily rather than dropped on invalid
                // UTF-8: a MOBI written in CP1252 is common, and showing a
                // replacement character beats showing nothing.
                let text = || String::from_utf8_lossy(&data).trim().to_string();

                match id {
                    100 => {
                        // *Accumulated*, not replaced. A Kindle book carries
                        // one EXTH 100 per author, so assigning here kept
                        // only the last and silently lost every co-author.
                        let author = text();
                        if !author.is_empty() {
                            if mi.authors.len() == 1 && mi.authors[0] == "Unknown" {
                                mi.authors.clear();
                            }
                            if !mi.authors.contains(&author) {
                                mi.authors.push(author);
                            }
                        }
                    }
                    // None of the five below were read at all, so a MOBI's
                    // publisher, description, ISBN, tags and publication
                    // date never reached the library -- for every Kindle
                    // book, not only ones this project writes.
                    101 => {
                        let publisher = text();
                        if !publisher.is_empty() {
                            mi.publisher = Some(publisher);
                        }
                    }
                    103 => {
                        let comments = text();
                        if !comments.is_empty() {
                            mi.comments = Some(comments);
                        }
                    }
                    104 => {
                        let isbn = text();
                        if !isbn.is_empty() {
                            mi.identifiers.insert("isbn".to_string(), isbn);
                        }
                    }
                    105 => {
                        // Subjects are `"; "`-joined by the writer, but real
                        // files use `;` or `,`, so both separate.
                        for tag in text().split([';', ',']) {
                            let tag = tag.trim();
                            if !tag.is_empty() && !mi.tags.iter().any(|t| t == tag) {
                                mi.tags.push(tag.to_string());
                            }
                        }
                    }
                    106 => {
                        // Free-form in practice; only a plain ISO date is
                        // parsed, and anything else is left unset rather
                        // than guessed at.
                        if let Ok(date) = chrono::NaiveDate::parse_from_str(text().get(..10).unwrap_or_default(), "%Y-%m-%d") {
                            mi.pubdate = date.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
                        }
                    }
                    503 => {
                        let title = text();
                        if !title.is_empty() {
                            mi.title = title;
                        }
                    }
                    201 => {
                        // Cover Offset
                        if data.len() >= 4 {
                            let off = (&data[0..4]).read_u32::<BigEndian>()?;
                            let cover_idx = first_image_index + off;
                            if (cover_idx as usize) < pdb.records.len() {
                                let cover_off = pdb.records[cover_idx as usize].offset;
                                let end_off = if (cover_idx as usize) < pdb.records.len() - 1 {
                                    pdb.records[cover_idx as usize + 1].offset
                                } else {
                                    stream.seek(SeekFrom::End(0))? as u32
                                };
                                let len = end_off - cover_off;
                                if len > 0 {
                                    let pos = stream.stream_position()?;
                                    stream.seek(SeekFrom::Start(cover_off as u64))?;
                                    let mut img = vec![0u8; len as usize];
                                    stream.read_exact(&mut img)?;
                                    mi.cover_data = (Some("jpg".to_string()), img);
                                    stream.seek(SeekFrom::Start(pos))?;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(mi)
}

/// Writes `mi` into a MOBI file, in place (#834).
///
/// Port of `metadata/mobi.py`'s `MetadataUpdater`. Metadata lives in an
/// EXTH block inside record 0, so changing it changes record 0's length --
/// which shifts every subsequent record's offset in the Palm database
/// header. That bookkeeping is the whole difficulty; the EXTH records
/// themselves are simple `(code, payload)` pairs.
///
/// **Disclosed narrowings**, both matching where upstream needs machinery
/// this port does not have:
///
/// - The cover is not replaced. Upstream rescales the new image to fit the
///   *existing* record's byte length, since a record cannot grow without
///   reshuffling the file again, and refuses if it will not fit.
/// - DRM'd files are refused outright. Upstream preserves the DRM block
///   across the rewrite; getting that wrong produces a file the device
///   rejects, and there is no way to test it here.
pub fn set_metadata(path: &Path, mi: &MetaInformation) -> Result<()> {
    let mut data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;

    if data.len() < 78 {
        bail!("{} is too short to be a MOBI file", path.display());
    }
    if &data[60..68] != b"BOOKMOBI" {
        bail!(
            "setting metadata is only supported for MOBI files of type BOOK; this is {:?}",
            String::from_utf8_lossy(&data[60..68])
        );
    }

    let num_records = u16::from_be_bytes([data[76], data[77]]) as usize;
    if num_records < 2 {
        bail!("{} has too few records to update", path.display());
    }

    // Every record's offset lives in the 8-byte entries starting at 78.
    let record_offset = |data: &[u8], i: usize| -> usize {
        let at = 78 + i * 8;
        u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
    };
    let record0_start = record_offset(&data, 0);
    let record0_end = record_offset(&data, 1);
    if record0_end <= record0_start || record0_end > data.len() {
        bail!("{}'s record 0 offsets are inconsistent", path.display());
    }
    let record0 = data[record0_start..record0_end].to_vec();

    if record0.len() < 0x60 || &record0[16..20] != b"MOBI" {
        bail!("{} has no MOBI header in record 0", path.display());
    }
    // Offset 12 of record 0 is the PalmDOC encryption type.
    let encryption_type = u16::from_be_bytes([record0[12], record0[13]]);
    if encryption_type != 0 {
        bail!("{} is DRM-encrypted; its metadata cannot be rewritten safely", path.display());
    }

    let mobi_header_length = u32::from_be_bytes([record0[0x14], record0[0x15], record0[0x16], record0[0x17]]) as usize;
    if mobi_header_length == 0 || 0x10 + mobi_header_length > record0.len() {
        bail!("{} has a non-standard MOBI header length", path.display());
    }

    // 65001 is UTF-8; anything else is treated as CP1252, which for the
    // ASCII range -- the part that matters for round-tripping -- is the
    // same bytes. Matching upstream's own two-way choice.
    let codepage = u32::from_be_bytes([record0[28], record0[29], record0[30], record0[31]]);
    let encode = |text: &str| -> Vec<u8> {
        if codepage == 65001 {
            text.as_bytes().to_vec()
        } else {
            text.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect()
        }
    };

    // Merged with what is already there, not regenerated. Regenerating
    // drops every record this does not write -- including 201/202, the
    // cover and thumbnail pointers, so a book would lose its cover on a
    // publisher edit. Upstream keeps the originals for exactly this reason.
    let mut records = exth_records(mi, &encode);
    let written: std::collections::HashSet<u32> = records.iter().map(|(code, _)| *code).collect();
    for (code, payload) in existing_exth_records(&record0, mobi_header_length) {
        if !written.contains(&code) {
            records.push((code, payload));
        }
    }
    records.sort_by_key(|(code, _)| *code);
    let exth = build_exth(&records);
    let new_record0 = rebuild_record0(&record0, mobi_header_length, &exth, mi.title.as_str(), &encode);

    // Splice the new record 0 in and re-point every later record.
    let tail = data[record0_end..].to_vec();
    data.truncate(record0_start);
    data.extend_from_slice(&new_record0);
    data.extend_from_slice(&tail);

    let shift = new_record0.len() as i64 - record0.len() as i64;
    for i in 1..num_records {
        let at = 78 + i * 8;
        if at + 4 > data.len() {
            break;
        }
        let old = u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as i64;
        let updated = (old + shift) as u32;
        data[at..at + 4].copy_from_slice(&updated.to_be_bytes());
    }

    let staging = tempfile::Builder::new().prefix("set-metadata").suffix(".mobi").tempfile_in(path.parent().unwrap_or(Path::new(".")))?;
    std::fs::write(staging.path(), &data)?;
    staging.persist(path).map_err(|e| anyhow::anyhow!("replacing {}: {e}", path.display()))?;
    Ok(())
}

/// The EXTH records a file already carries.
///
/// Returned so [`set_metadata`] can keep the ones it is not replacing.
/// A malformed block yields what was parsed before the damage rather than
/// an error: losing the records after a bad one is better than refusing to
/// write metadata at all, and the block is rebuilt from scratch anyway.
fn existing_exth_records(record0: &[u8], mobi_header_length: usize) -> Vec<(u32, Vec<u8>)> {
    let mut out = Vec::new();
    let start = 0x10 + mobi_header_length;
    if record0.len() < start + 12 || &record0[start..start + 4] != b"EXTH" {
        return out;
    }
    let count = u32::from_be_bytes([record0[start + 8], record0[start + 9], record0[start + 10], record0[start + 11]]) as usize;

    let mut at = start + 12;
    for _ in 0..count {
        if at + 8 > record0.len() {
            break;
        }
        let code = u32::from_be_bytes([record0[at], record0[at + 1], record0[at + 2], record0[at + 3]]);
        let length = u32::from_be_bytes([record0[at + 4], record0[at + 5], record0[at + 6], record0[at + 7]]) as usize;
        // The length counts the two header fields, so anything under 8 is
        // corrupt and would loop forever.
        if length < 8 || at + length > record0.len() {
            break;
        }
        out.push((code, record0[at + 8..at + length].to_vec()));
        at += length;
    }
    out
}

/// The EXTH records to write, as `(code, payload)`.
///
/// Codes are upstream's: 100 author (one record *per* author, not joined),
/// 101 publisher, 103 description, 104 ISBN, 105 subjects (`"; "`-joined),
/// 106 publication date, 503 title.
fn exth_records(mi: &MetaInformation, encode: &impl Fn(&str) -> Vec<u8>) -> Vec<(u32, Vec<u8>)> {
    let mut records: Vec<(u32, Vec<u8>)> = Vec::new();

    if let Some(authors) = crate::metadata::zip_edit::placeholders::real_authors(&mi.authors) {
        // One record each: a Kindle shows multiple authors from repeated
        // 100s, and joining them into one would display as a single name.
        for author in authors {
            records.push((100, encode(author)));
        }
    }
    if let Some(publisher) = mi.publisher.as_deref().filter(|p| !p.trim().is_empty()) {
        records.push((101, encode(publisher)));
    }
    if let Some(comments) = mi.comments.as_deref().filter(|c| !c.trim().is_empty()) {
        records.push((103, encode(comments)));
    }
    if let Some(isbn) = mi.identifiers.get("isbn").filter(|i| !i.trim().is_empty()) {
        records.push((104, encode(isbn)));
    }
    let tags: Vec<&str> = mi.tags.iter().map(|t| t.trim()).filter(|t| !t.is_empty()).collect();
    if !tags.is_empty() {
        records.push((105, encode(&tags.join("; "))));
    }
    if let Some(pubdate) = mi.pubdate {
        records.push((106, encode(&pubdate.format("%Y-%m-%d").to_string())));
    }
    if let Some(title) = crate::metadata::zip_edit::placeholders::real_title(&mi.title) {
        records.push((503, encode(title)));
    }
    records
}

/// Serialises an EXTH block: the tag, its total length, the record count,
/// then each record as `(code, length, payload)`.
///
/// Padded to a 4-byte boundary with **at least one** byte, as upstream
/// does -- a block that happens to be aligned still gets four.
fn build_exth(records: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (code, payload) in records {
        body.extend_from_slice(&code.to_be_bytes());
        // The length field counts the two 4-byte fields as well.
        body.extend_from_slice(&((payload.len() + 8) as u32).to_be_bytes());
        body.extend_from_slice(payload);
    }

    let mut exth = Vec::new();
    exth.extend_from_slice(b"EXTH");
    exth.extend_from_slice(&((body.len() + 12) as u32).to_be_bytes());
    exth.extend_from_slice(&(records.len() as u32).to_be_bytes());
    exth.extend_from_slice(&body);
    exth.resize(exth.len() + (4 - exth.len() % 4), 0);
    exth
}

/// Rebuilds record 0 around a new EXTH block.
fn rebuild_record0(record0: &[u8], mobi_header_length: usize, exth: &[u8], title: &str, encode: &impl Fn(&str) -> Vec<u8>) -> Vec<u8> {
    let mut header = record0[..0x10 + mobi_header_length].to_vec();

    // The existing title, in case `mi` has none worth writing.
    let title_offset = u32::from_be_bytes([record0[0x54], record0[0x55], record0[0x56], record0[0x57]]) as usize;
    let title_length = u32::from_be_bytes([record0[0x58], record0[0x59], record0[0x5a], record0[0x5b]]) as usize;
    let existing_title = record0.get(title_offset..title_offset + title_length).unwrap_or_default().to_vec();

    let new_title = match crate::metadata::zip_edit::placeholders::real_title(title) {
        Some(title) => encode(title),
        None => existing_title,
    };

    // The EXTH flag, bit 6 of the flags word at 0x80.
    let flags = u32::from_be_bytes([header[0x80], header[0x81], header[0x82], header[0x83]]);
    header[0x80..0x84].copy_from_slice(&(flags | 0x40).to_be_bytes());

    // The title now sits after the header and the new EXTH block.
    let new_title_offset = (0x10 + mobi_header_length + exth.len()) as u32;
    header[0x54..0x58].copy_from_slice(&new_title_offset.to_be_bytes());
    header[0x58..0x5c].copy_from_slice(&(new_title.len() as u32).to_be_bytes());

    let mut out = header;
    out.extend_from_slice(exth);
    out.extend_from_slice(&new_title);
    out.resize(out.len() + (4 - out.len() % 4), 0);
    // Upstream appends 8KB of slack so the *next* metadata edit usually
    // fits without moving anything. Preserved: a MOBI from calibre has
    // this block, and matching it keeps our files indistinguishable.
    out.resize(out.len() + 8 * 1024, 0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;
    use std::io::Cursor;

    // Helper to build PDB
    fn create_test_pdb(records: Vec<Vec<u8>>) -> Vec<u8> {
        let mut buffer = Vec::new();
        // Header (78 bytes)
        buffer.extend_from_slice(b"Test Book Title\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap(); // mod_num
        buffer.write_u32::<BigEndian>(0).unwrap(); // app_info
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.extend_from_slice(b"BOOKMOBI");
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u32::<BigEndian>(0).unwrap();
        buffer.write_u16::<BigEndian>(records.len() as u16).unwrap();

        // Record List
        let base_offset = 78 + (records.len() as u32 * 8) + 2;
        let mut offsets = Vec::new();
        let mut running_off = base_offset;

        for r in &records {
            offsets.push(running_off);
            running_off += r.len() as u32;
        }

        // Write Rec List
        for off in offsets {
            buffer.write_u32::<BigEndian>(off).unwrap();
            buffer.write_u32::<BigEndian>(0).unwrap();
        }

        // Pad to base_offset
        while buffer.len() < base_offset as usize {
            buffer.push(0);
        }

        // Write Records
        for r in records {
            buffer.extend_from_slice(&r);
        }

        buffer
    }

    #[test]
    fn test_mobi_metadata() -> Result<()> {
        // println!("START TEST: test_mobi_metadata");
        // 1. Construct Rec 0 (Header + EXTH)
        let mut rec0 = Vec::new();
        // PalmDOC (16)
        rec0.write_u16::<BigEndian>(1).unwrap();
        rec0.write_u16::<BigEndian>(0)?;
        rec0.write_u32::<BigEndian>(0)?;
        rec0.write_u16::<BigEndian>(0)?;
        rec0.write_u16::<BigEndian>(0)?;
        rec0.write_u16::<BigEndian>(0)?;
        rec0.write_u16::<BigEndian>(0)?;

        // MOBI Header
        rec0.extend_from_slice(b"MOBI");
        let mobi_len = 232u32;
        rec0.write_u32::<BigEndian>(mobi_len)?; // Len matches what we write
        rec0.write_u32::<BigEndian>(2)?;
        rec0.write_u32::<BigEndian>(65001)?;
        rec0.write_u32::<BigEndian>(0)?;
        rec0.write_u32::<BigEndian>(0)?;

        // Pad to 108 (Image Index)
        while rec0.len() < 108 {
            rec0.push(0);
        }
        rec0.write_u32::<BigEndian>(1)?; // First Image Index maps to Record 1 (0+1)

        // Pad to 128 (Flags)
        while rec0.len() < 128 {
            rec0.push(0);
        }
        rec0.write_u32::<BigEndian>(0x40)?; // EXTH

        // Pad to end of MOBI Header (Starts at 16, Len 232 -> Ends at 248)
        while rec0.len() < 248 {
            rec0.push(0);
        }

        // EXTH
        rec0.extend_from_slice(b"EXTH");
        rec0.write_u32::<BigEndian>(0)?; // Len
        rec0.write_u32::<BigEndian>(2)?; // Count

        // Title
        let title = "MOBI Title";
        rec0.write_u32::<BigEndian>(503)?;
        rec0.write_u32::<BigEndian>(8 + title.len() as u32)?;
        rec0.extend_from_slice(title.as_bytes());

        // Cover Offset (201)
        // Data 4 bytes.
        rec0.write_u32::<BigEndian>(201)?;
        rec0.write_u32::<BigEndian>(8 + 4)?;
        rec0.write_u32::<BigEndian>(0)?; // Offset 0 from First Image Index (1) -> Record 1

        // 2. Cover Record
        let cover = b"fake cover image".to_vec();

        // Build PDB
        let buffer = create_test_pdb(vec![rec0, cover]);

        let mut stream = Cursor::new(buffer);
        let mi = get_metadata(&mut stream)?;

        assert_eq!(mi.title, "MOBI Title");
        assert!(mi.cover_data.1.starts_with(b"fake cover image"));

        Ok(())
    }
}

#[cfg(test)]
mod set_metadata_tests {
    use super::*;
    use crate::oeb::container::DirContainer;

    /// A real MOBI, produced by this crate's own writer through the
    /// conversion pipeline -- the only honest fixture, since hand-building
    /// a valid one means reimplementing the writer in the test.
    fn a_mobi(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("c1.html"), "<html><body><h1>One</h1><p>The quick brown fox jumps over the lazy dog.</p></body></html>").unwrap();

        let mut book = crate::oeb::book::OEBBook::new(Box::new(DirContainer::new(&src)));
        book.manifest.add("c1", "c1.html", "application/xhtml+xml");
        book.spine.add("c1", true);
        book.metadata.add("title", "Original Title");
        book.metadata.add("creator", "Original Author");
        // The MOBI writer's EXTH builder requires a publication date and
        // refuses without one -- faithfully, upstream raises the same
        // "missing date or timestamp". A real conversion always has one by
        // the time it reaches an output plugin.
        book.metadata.add("date", "2026-09-28");

        let path = dir.path().join("book.mobi");
        crate::output::mobi_output::MOBIOutput::new()
            .convert(&book, &path, &crate::conversion::options::ConversionOptions::default())
            .unwrap();
        path
    }

    fn read_back(path: &Path) -> MetaInformation {
        get_metadata(std::fs::File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn title_and_authors_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "Corrected Title".to_string();
        mi.authors = vec!["Ann Author".to_string(), "Bob Writer".to_string()];
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.title, "Corrected Title");
        // One EXTH 100 per author, so both come back separately rather than
        // as a single joined name.
        assert!(got.authors.contains(&"Ann Author".to_string()), "{:?}", got.authors);
        assert!(got.authors.contains(&"Bob Writer".to_string()), "{:?}", got.authors);
    }

    /// The file has to remain a valid Palm database: every record offset
    /// after 0 shifts when the EXTH block changes size, and getting that
    /// arithmetic wrong produces a file nothing can open.
    #[test]
    fn the_record_offsets_stay_consistent() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        let mut mi = MetaInformation::default();
        // A long value, to force the EXTH block to grow substantially.
        mi.title = "A Considerably Longer Title Than The Original One".to_string();
        mi.comments = Some("A description long enough to change record 0's length by a good margin.".repeat(4));
        set_metadata(&path, &mi).unwrap();

        let data = std::fs::read(&path).unwrap();
        assert_eq!(&data[60..68], b"BOOKMOBI", "no longer a MOBI Palm database");

        let num_records = u16::from_be_bytes([data[76], data[77]]) as usize;
        let offset = |i: usize| {
            let at = 78 + i * 8;
            u32::from_be_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as usize
        };

        // Offsets must be strictly increasing and inside the file.
        let mut previous = offset(0);
        for i in 1..num_records {
            let current = offset(i);
            assert!(current > previous, "record {i} at {current} does not follow {previous}");
            assert!(current <= data.len(), "record {i} at {current} is past the end of a {}-byte file", data.len());
            previous = current;
        }

        // Record 0 still carries a MOBI header at its own offset.
        let record0 = offset(0);
        assert_eq!(&data[record0 + 16..record0 + 20], b"MOBI");
    }

    #[test]
    fn publisher_tags_and_comments_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        let mut mi = MetaInformation::default();
        mi.publisher = Some("Real Publisher".to_string());
        mi.tags = vec!["Science Fiction".to_string(), "Classics".to_string()];
        mi.comments = Some("A short description.".to_string());
        set_metadata(&path, &mi).unwrap();

        let got = read_back(&path);
        assert_eq!(got.publisher.as_deref(), Some("Real Publisher"));
        assert_eq!(got.comments.as_deref(), Some("A short description."));
        assert!(got.tags.contains(&"Science Fiction".to_string()), "{:?}", got.tags);
    }

    /// The same placeholder rule as every other writer: a default
    /// `MetaInformation` must not write "Unknown" over a real title.
    #[test]
    fn placeholder_metadata_does_not_overwrite_the_real_title() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);
        let before = read_back(&path).title;
        assert_eq!(before, "Original Title", "the fixture should start with a real title");

        set_metadata(&path, &MetaInformation::default()).unwrap();

        assert_eq!(read_back(&path).title, "Original Title", "the real title was overwritten with a placeholder");
    }

    #[test]
    fn repeated_edits_keep_the_file_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        for title in ["First", "Second", "Third"] {
            let mut mi = MetaInformation::default();
            mi.title = title.to_string();
            set_metadata(&path, &mi).unwrap();
            assert_eq!(read_back(&path).title, title, "after setting {title:?}");
        }
    }

    /// The book's text must survive, which is what the offset shifting is
    /// for -- a correct header over shifted content is still a broken book.
    #[test]
    fn the_books_text_survives() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        let mut mi = MetaInformation::default();
        mi.title = "Corrected".to_string();
        set_metadata(&path, &mi).unwrap();

        // Read the book back through the real MOBI input plugin.
        let out = dir.path().join("oeb");
        let book = crate::input::mobi_input::MOBIInput::new().convert(&path, &out).expect("the rewritten MOBI should still convert");
        let mut found = false;
        for item in book.manifest.items.values() {
            if let Ok(data) = book.container.read(&item.href) {
                if String::from_utf8_lossy(&data).contains("quick brown fox") {
                    found = true;
                }
            }
        }
        assert!(found, "the book's text did not survive the metadata rewrite");
    }

    /// EXTH records this does not write must survive. Regenerating the
    /// block drops them -- including **201/202, the cover and thumbnail
    /// pointers** -- so a book would lose its cover on a publisher edit.
    ///
    /// Caught by the placeholder test: setting nothing wiped the existing
    /// title record, and the reader fell back to the Palm name, which is
    /// the underscored filename-safe form (`Original_Title`).
    #[test]
    fn exth_records_that_are_not_rewritten_survive() {
        let dir = tempfile::tempdir().unwrap();
        let path = a_mobi(&dir);

        // What the file starts with.
        let before = {
            let data = std::fs::read(&path).unwrap();
            let record0_start = u32::from_be_bytes([data[78], data[79], data[80], data[81]]) as usize;
            let record0_end = u32::from_be_bytes([data[86], data[87], data[88], data[89]]) as usize;
            let record0 = &data[record0_start..record0_end];
            let header_length = u32::from_be_bytes([record0[0x14], record0[0x15], record0[0x16], record0[0x17]]) as usize;
            existing_exth_records(record0, header_length)
        };
        assert!(!before.is_empty(), "the fixture should carry EXTH records to preserve");

        // Set only a publisher -- nothing else.
        let mut mi = MetaInformation::default();
        mi.publisher = Some("Real Publisher".to_string());
        set_metadata(&path, &mi).unwrap();

        let after = {
            let data = std::fs::read(&path).unwrap();
            let record0_start = u32::from_be_bytes([data[78], data[79], data[80], data[81]]) as usize;
            let record0_end = u32::from_be_bytes([data[86], data[87], data[88], data[89]]) as usize;
            let record0 = &data[record0_start..record0_end];
            let header_length = u32::from_be_bytes([record0[0x14], record0[0x15], record0[0x16], record0[0x17]]) as usize;
            existing_exth_records(record0, header_length)
        };

        // Every original code is still present, and the publisher is new.
        for (code, _) in &before {
            assert!(after.iter().any(|(c, _)| c == code), "EXTH {code} was dropped by a write that did not touch it");
        }
        assert!(after.iter().any(|(c, _)| *c == 101), "the publisher was not written");
    }

    /// The same file, declared KF8.
    ///
    /// `file_version` lives at record 0 offset 0x24; 8 marks the file as
    /// KF8, which is what an `.azw3` is. Nothing else about the Palm
    /// database or the EXTH block differs, which is the point: the writer
    /// touches only those, so a KF8 file needs no separate code path --
    /// and upstream's MOBI metadata writer likewise claims `azw3` and
    /// routes it to the same function.
    fn declare_kf8(path: &Path) {
        let mut data = std::fs::read(path).unwrap();
        let record0 = u32::from_be_bytes([data[78], data[79], data[80], data[81]]) as usize;
        data[record0 + 0x24..record0 + 0x28].copy_from_slice(&8u32.to_be_bytes());
        std::fs::write(path, data).unwrap();
    }

    /// #834: `azw3` reported "no metadata writer for azw3 yet" while
    /// `get_metadata` had always read it through this very module. The
    /// exclusion was not true of the code.
    #[test]
    fn an_azw3_round_trips_through_the_public_dispatcher() {
        let dir = tempfile::tempdir().unwrap();
        let mobi = a_mobi(&dir);
        let path = dir.path().join("book.azw3");
        std::fs::rename(&mobi, &path).unwrap();
        declare_kf8(&path);

        assert!(crate::metadata::can_set_metadata("azw3"), "azw3 is still advertised as unwritable");

        let mut mi = MetaInformation::default();
        mi.title = "Corrected KF8 Title".to_string();
        mi.authors = vec!["Ann Author".to_string()];
        crate::metadata::set_metadata(&path, &mi).unwrap();

        // Read back through the public reader, which routes azw3 here too.
        let got = crate::metadata::get_metadata(&path).unwrap();
        assert_eq!(got.title, "Corrected KF8 Title");
        assert!(got.authors.contains(&"Ann Author".to_string()), "{:?}", got.authors);
    }

    /// The KF8 file has to stay a valid Palm database as well -- the same
    /// offset arithmetic that `the_record_offsets_stay_consistent` covers
    /// for MOBI6, checked through the file the dispatcher actually wrote.
    #[test]
    fn an_azw3_stays_readable_after_repeated_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mobi = a_mobi(&dir);
        let path = dir.path().join("book.azw3");
        std::fs::rename(&mobi, &path).unwrap();
        declare_kf8(&path);

        for n in 0..3 {
            let mut mi = MetaInformation::default();
            mi.title = format!("Pass {n}");
            crate::metadata::set_metadata(&path, &mi).unwrap();
            assert_eq!(crate::metadata::get_metadata(&path).unwrap().title, format!("Pass {n}"));
        }
    }

    #[test]
    fn a_file_that_is_not_a_mobi_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("book.mobi");
        std::fs::write(&fake, b"this is not a Palm database at all, but it is long enough to pass the length check easily").unwrap();

        let err = set_metadata(&fake, &MetaInformation::default()).unwrap_err();
        assert!(format!("{err:#}").contains("BOOK"), "{err:#}");
    }
}
