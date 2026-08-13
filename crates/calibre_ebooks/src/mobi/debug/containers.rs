//! KF8 resource-container header dumping.
//!
//! Port of `src/calibre/ebooks/mobi/debug/containers.py`: the header
//! prefixing each `CONT`/`CRES` group of records — the fonts and
//! high-definition images KindleGen packs alongside a KF8 book.

use std::fmt;

use anyhow::{bail, Context, Result};

use super::headers::ExthHeader;

fn be_u16(b: &[u8]) -> Result<u16> {
    b.get(..2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .context("truncated u16")
}

fn be_u32(b: &[u8]) -> Result<u32> {
    b.get(..4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .context("truncated u32")
}

/// A `CONT` record's header. `ContainerHeader` in the Python.
pub struct ContainerHeader {
    pub ident: Vec<u8>,
    pub record_size: u32,
    pub type_: u16,
    pub count: u16,
    pub encoding: String,
    pub num_of_resource_records: u32,
    pub num_of_non_dummy_resource_records: u32,
    pub offset_to_href_record: u32,
    pub unknowns1: [u32; 2],
    pub unknowns2: u32,
    pub header_length: u32,
    pub title_length: u32,
    /// Populated by [`ContainerHeader::add_hrefs`] once the header
    /// record naming each resource has been located.
    pub hrefs: Vec<String>,
    /// One entry per `CRES` payload seen after this header, `None`
    /// where a `\xa0\xa0\xa0\xa0` end-of-container marker stood in for
    /// a dummy resource. Populated by the caller as it walks records
    /// — mirrors `container.resources.append(...)` in `mobi8.py`.
    pub resources: Vec<Option<Vec<u8>>>,
    pub exth: Option<ExthHeader>,
    pub title: String,
    pub is_image_container: bool,
    pub bytes_after_exth: Vec<u8>,
    pub null_bytes_after_exth: usize,
}

impl ContainerHeader {
    /// `ContainerHeader.__init__`.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 48 {
            bail!("Truncated container header");
        }
        let ident = data[..4].to_vec();
        let record_size = be_u32(&data[4..8])?;
        let type_ = be_u16(&data[8..10])?;
        let count = be_u16(&data[10..12])?;
        let encoding_num = be_u32(&data[12..16])?;
        let encoding = match encoding_num {
            1252 => "cp1252".to_string(),
            65001 => "utf-8".to_string(),
            other => other.to_string(),
        };

        let mut rest = [0u32; 8];
        for (i, slot) in rest.iter_mut().enumerate() {
            *slot = be_u32(&data[16 + i * 4..20 + i * 4])?;
        }
        let num_of_resource_records = rest[2];
        let num_of_non_dummy_resource_records = rest[3];
        let offset_to_href_record = rest[4];
        let unknowns1 = [rest[0], rest[1]];
        let unknowns2 = rest[5];
        let header_length = rest[6];
        let title_length = rest[7];

        let has_exth = data.len() >= 52 && &data[48..52] == b"EXTH";
        let (exth, title, is_image_container) = if has_exth {
            let exth = ExthHeader::parse(&data[48..])?;
            let title_start = 48 + exth.length as usize;
            let title_end = (title_start + title_length as usize).min(data.len());
            let title = String::from_utf8_lossy(&data[title_start..title_end]).into_owned();
            let is_image_container = exth
                .get(539)
                .map(|r| matches!(&r.data, super::headers::ExthValue::Bytes(b) if b == b"application/image"))
                .unwrap_or(false);
            (Some(exth), title, is_image_container)
        } else {
            (None, String::new(), false)
        };

        let bytes_after_exth = data
            .get((header_length as usize + title_length as usize)..)
            .unwrap_or(&[])
            .to_vec();
        let null_bytes_after_exth =
            bytes_after_exth.len() - bytes_after_exth.iter().filter(|&&b| b != 0).count();

        Ok(ContainerHeader {
            ident,
            record_size,
            type_,
            count,
            encoding,
            num_of_resource_records,
            num_of_non_dummy_resource_records,
            offset_to_href_record,
            unknowns1,
            unknowns2,
            header_length,
            title_length,
            hrefs: Vec::new(),
            resources: Vec::new(),
            exth,
            title,
            is_image_container,
            bytes_after_exth,
            null_bytes_after_exth,
        })
    }

    /// `ContainerHeader.add_hrefs` — KindleGen inserts a trailing `|`
    /// after the last href, so the final split element is dropped.
    pub fn add_hrefs(&mut self, data: &[u8]) {
        self.hrefs = String::from_utf8_lossy(data)
            .split('|')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }
}

impl fmt::Display for ContainerHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{} Container Header {}", "*".repeat(10), "*".repeat(10))?;
        writeln!(f, "Record size: {}", self.record_size)?;
        writeln!(f, "Type: {}", self.type_)?;
        writeln!(
            f,
            "Total number of records in this container: {}",
            self.count
        )?;
        writeln!(f, "Encoding: {}", self.encoding)?;
        writeln!(f, "Unknowns1: {:?}", self.unknowns1)?;
        writeln!(
            f,
            "Num of resource records: {}",
            self.num_of_resource_records
        )?;
        writeln!(
            f,
            "Num of non-dummy resource records: {}",
            self.num_of_non_dummy_resource_records
        )?;
        writeln!(f, "Offset to href record: {}", self.offset_to_href_record)?;
        writeln!(f, "Unknowns2: {}", self.unknowns2)?;
        writeln!(f, "Header length: {}", self.header_length)?;
        writeln!(f, "Title Length: {}", self.title_length)?;
        writeln!(f, "hrefs: {:?}", self.hrefs)?;
        writeln!(f, "Null bytes after EXTH: {}", self.null_bytes_after_exth)?;
        if self.bytes_after_exth.len() != self.null_bytes_after_exth {
            writeln!(f, "Non-null bytes present after EXTH header!!!!")?;
        }
        write!(f, "\n")?;
        match &self.exth {
            Some(exth) => write!(f, "{exth}")?,
            None => write!(f, " No EXTH header present ")?,
        }
        write!(f, "\n\nTitle: {}", self.title)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_exth(records: &[(u32, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, data) in records {
            body.extend_from_slice(&t.to_be_bytes());
            body.extend_from_slice(&((data.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(data);
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"EXTH");
        out.extend_from_slice(&((body.len() + 12) as u32).to_be_bytes());
        out.extend_from_slice(&(records.len() as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    fn build_container(exth: &[u8], title: &[u8]) -> Vec<u8> {
        let header_length = (48 + exth.len()) as u32;
        let mut data = Vec::new();
        data.extend_from_slice(b"CONT");
        data.extend_from_slice(&100u32.to_be_bytes()); // record_size
        data.extend_from_slice(&1u16.to_be_bytes()); // type
        data.extend_from_slice(&2u16.to_be_bytes()); // count
        data.extend_from_slice(&65001u32.to_be_bytes()); // encoding
        let rest = [0u32, 0, 3, 2, 5, 0, header_length, title.len() as u32];
        for r in rest {
            data.extend_from_slice(&r.to_be_bytes());
        }
        data.extend_from_slice(exth);
        data.extend_from_slice(title);
        data
    }

    #[test]
    fn parses_a_container_header_with_exth_and_title() {
        let exth = build_exth(&[(539, b"application/image")]);
        let data = build_container(&exth, b"A Title");
        let ch = ContainerHeader::parse(&data).expect("parses");
        assert_eq!(ch.ident, b"CONT");
        assert_eq!(ch.count, 2);
        assert_eq!(ch.num_of_resource_records, 3);
        assert_eq!(ch.num_of_non_dummy_resource_records, 2);
        assert_eq!(ch.title, "A Title");
        assert!(ch.is_image_container);
    }

    #[test]
    fn parses_a_container_header_without_exth() {
        let mut data = Vec::new();
        data.extend_from_slice(b"CONT");
        data.extend_from_slice(&100u32.to_be_bytes());
        data.extend_from_slice(&1u16.to_be_bytes());
        data.extend_from_slice(&2u16.to_be_bytes());
        data.extend_from_slice(&65001u32.to_be_bytes());
        for _ in 0..8 {
            data.extend_from_slice(&0u32.to_be_bytes());
        }
        let ch = ContainerHeader::parse(&data).expect("parses");
        assert!(ch.exth.is_none());
        assert!(!ch.is_image_container);
        assert_eq!(ch.title, "");
    }

    #[test]
    fn add_hrefs_drops_the_trailing_pipe_separated_empty_element() {
        let mut ch = ContainerHeader::parse(&build_container(&[], b"")).expect("parses");
        ch.add_hrefs(b"a.jpg|b.jpg|c.jpg|");
        assert_eq!(ch.hrefs, vec!["a.jpg", "b.jpg", "c.jpg"]);
    }
}
