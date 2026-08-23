//! Port of `calibre.ebooks.mobi.reader.mobi6.MobiReader` -- the MOBI6
//! (PalmDOC/HUFF-CDIC text, `mbp:`-flavoured HTML) reader. Turns the
//! decompressed text stream of a `.mobi` file into `index.html` +
//! `images/*` + `metadata.opf` (+ `toc.ncx`) on disk.
//!
//! This is also the entry point `Mobi8Reader` (see
//! [`crate::mobi::mobi8`]) is built on top of: a KF8 file's outer PDB
//! envelope, PalmDOC/HUFF-CDIC decompression, and (for "joint" MOBI6+KF8
//! files) `BOOKMOBI` parsing are all shared, so `Mobi8Reader::new` takes a
//! `MobiReader` that has already parsed headers.

use anyhow::{bail, Context, Result};
use byteorder::{BigEndian, ReadBytesExt};
use indexmap::IndexMap;
use lazy_static::lazy_static;
use regex::bytes::Regex as BytesRegex;
use regex::Regex;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use crate::beautiful_soup::clean_xml_chars;
use crate::chardet::strip_encoding_declarations;
use crate::compression::palmdoc::decompress as decompress_doc;
use crate::html_entities::{decode_entities, xml_replace_entities};
use crate::metadata::toc::{TOCNode, TOC};
use crate::metadata::MetaInformation;
use crate::dom::{Dom, NodeId};
use crate::mobi::headers::BookHeader;
use crate::mobi::huffcdic::HuffReader;
use crate::mobi::opf_writer::{self, GuideRef};
use crate::mobi::{MobiError, MobiLog};

lazy_static! {
    static ref PAGE_BREAK_PAT: Regex = Regex::new(
        r"(?i)<\s*/{0,1}\s*mbp:pagebreak((?:\s+[^/>]*){0,1})/{0,1}\s*>\s*(?:<\s*/{0,1}\s*mbp:pagebreak\s*/{0,1}\s*>)*"
    )
    .unwrap();
    static ref LINK_PATTERN: BytesRegex =
        BytesRegex::new(r#"(?i)<[^<>]+filepos=['"]?(\d+)[^<>]*>"#).unwrap();
    static ref END_TAG_RE: BytesRegex = BytesRegex::new(r"^<\s*/").unwrap();
    static ref ENTITY_FIX_RE: BytesRegex =
        BytesRegex::new(r#"&([^;]*?)(<a id="filepos\d+"></a>)([^;]*);"#).unwrap();
    static ref RANDOM_BYTES_RE: Regex =
        Regex::new("[\u{14}\u{15}\u{19}\u{1c}\u{1d}\u{ef}\u{12}\u{13}\u{ec}\u{08}\u{01}\u{02}\u{03}\u{04}\u{05}\u{06}\u{07}]").unwrap();
    static ref DIV_HEIGHT0_RE: Regex = Regex::new(r#"<div height="0(pt|px|ex|em|%)?"></div>"#).unwrap();
    static ref XML_DECL_RE: Regex = Regex::new(r"<\?xml[^>]*>").unwrap();
    static ref OP_TAG_RE: Regex = Regex::new(r"<\s*(/?)\s*o:p[^>]*>").unwrap();
    static ref NS_TAG_RE: Regex = Regex::new(r"</?[a-zA-Z]+:\s+[^>]*>").unwrap();
    static ref DOUBLE_CLOSE_RE: Regex = Regex::new(r"</([a-zA-Z]+)<").unwrap();
    static ref STYLE_BEFORE_P_RE: Regex = Regex::new(
        r"(?i)(?P<styletags>(<(h\d+|i|b|u|em|small|big|strong|tt)>\s*){1,})(?P<para><p[^>]*>)"
    )
    .unwrap();
    static ref STYLE_AFTER_P_CLOSE_RE: Regex = Regex::new(
        r"(?i)(?P<para></p[^>]*>)\s*(?P<styletags>(</(h\d+|i|b|u|em|small|big|strong|tt)>\s*){1,})"
    )
    .unwrap();
    static ref BQ_BEFORE_PCLOSE_RE: Regex = Regex::new(
        r"(?i)(?P<blockquote>(</(blockquote|div)[^>]*>\s*){1,})(?P<para></p[^>]*>)"
    )
    .unwrap();
    static ref BQ_AFTER_POPEN_RE: Regex = Regex::new(
        r"(?i)(?P<para><p[^>]*>)\s*(?P<blockquote>(<(blockquote|div)[^>]*>\s*){1,})"
    )
    .unwrap();
    static ref UNIT_RE: Regex =
        Regex::new(r"^(-*[0-9]*[.]?[0-9]*)\s*(%|em|ex|en|px|mm|cm|in|pt|pc|rem|q)$").unwrap();
    static ref HAS_DIGIT_RE: Regex = Regex::new(r"\d+").unwrap();
    static ref TRAILING_DIGIT_RE: Regex = Regex::new(r"\d+$").unwrap();
    static ref URL_SCHEME_RE: Regex = Regex::new(r"^\w+://").unwrap();
}

pub const BASE_CSS_RULES: &str = "\
body { text-align: justify }

blockquote { margin: 0em 0em 0em 2em; }

p { margin: 0em; text-indent: 1.5em }

.bold { font-weight: bold }

.italic { font-style: italic }

.underline { text-decoration: underline }

.mbp_pagebreak {
    page-break-after: always; margin: 0; display: block
}
";

const IMAGE_ATTRS: [&str; 3] = ["lowrecindex", "recindex", "hirecindex"];

/// Port of `calibre.ebooks.__init__.unit_convert`: converts a CSS length
/// (already unit-suffixed, e.g. `"12px"`) to points.
fn unit_convert(value: &str, base: f64, font: f64, dpi: f64) -> Option<f64> {
    if let Ok(v) = value.parse::<f64>() {
        return Some(v);
    }
    let caps = UNIT_RE.captures(value)?;
    let num: f64 = caps.get(1)?.as_str().parse().ok()?;
    let unit = caps.get(2)?.as_str();
    Some(match unit {
        "%" => (num / 100.0) * base,
        "px" => num * 72.0 / dpi,
        "in" => num * 72.0,
        "pt" => num,
        "em" => num * font,
        "ex" | "en" => num * font * 0.5,
        "pc" => num * 12.0,
        "mm" => num * 2.8346456693,
        "cm" => num * 28.346456693,
        "rem" => num * 12.0, // body_font_size default
        "q" => num * 0.708661417325,
        _ => return None,
    })
}

/// True if the sibling immediately following `tag` (lxml's `tag.tail`) is
/// either absent or whitespace-only text.
fn tail_is_whitespace(dom: &Dom, tag: NodeId) -> bool {
    match dom.next_sibling(tag) {
        None => true,
        Some(n) => match &dom.node(n).kind {
            crate::dom::NodeKind::Text(t) => t.trim().is_empty(),
            _ => false,
        },
    }
}

fn find_byte_from(data: &[u8], byte: u8, start: usize) -> Option<usize> {
    if start >= data.len() {
        return None;
    }
    data[start..]
        .iter()
        .position(|&b| b == byte)
        .map(|p| p + start)
}

fn rfind_byte_upto(data: &[u8], byte: u8, end_exclusive: usize) -> Option<usize> {
    let end = end_exclusive.min(data.len());
    data[..end].iter().rposition(|&b| b == byte)
}

#[derive(Debug, Clone)]
pub struct SectionHeader {
    pub offset: u32,
    pub flags: u8,
    pub val: u32,
}

/// Port of `mobi6.py`'s `MobiReader` class.
pub struct MobiReader {
    pub name: String,
    pub num_sections: u16,
    pub sections: Vec<Vec<u8>>,
    pub section_headers: Vec<SectionHeader>,
    pub book_header: BookHeader,
    pub kf8_type: Option<String>,
    pub kf8_boundary: Option<usize>,
    pub embedded_mi: Option<MetaInformation>,
    pub log: MobiLog,

    pub tag_css_rules: IndexMap<String, String>,
    left_margins: HashMap<NodeId, f64>,
    text_indents: HashMap<NodeId, f64>,

    warned_about_trailing_entry_corruption: bool,

    pub mobi_html: Vec<u8>,
    pub processed_html: String,
    pub image_names: Vec<String>,
    pub htmlfile: Option<PathBuf>,
    pub created_opf_path: Option<PathBuf>,
}

impl MobiReader {
    /// Port of `MobiReader.__init__`. `raw` is the entire `.mobi`/`.azw3`
    /// file's bytes.
    pub fn new(raw: &[u8]) -> Result<Self> {
        if raw.starts_with(b"TPZ") {
            return Err(MobiError::Topaz.into());
        }
        if raw.starts_with(b"\xeaDRMION\xee") {
            return Err(MobiError::Kfx.into());
        }
        if raw.len() < 78 {
            bail!("File too small to be a MOBI file");
        }

        let name_raw = &raw[0..32];
        let name_stripped: Vec<u8> = name_raw.iter().copied().filter(|&b| b != 0).collect();

        let mut cursor = Cursor::new(&raw[76..78]);
        let num_sections = cursor.read_u16::<BigEndian>()?;

        let ident = raw[0x3C..0x3C + 8].to_ascii_uppercase();
        if ident != b"BOOKMOBI" && ident != b"TEXTREAD" {
            bail!("Unknown book type: {:?}", String::from_utf8_lossy(&ident));
        }

        let mut section_headers = Vec::with_capacity(num_sections as usize);
        for i in 0..num_sections as usize {
            let base = 78 + i * 8;
            if base + 8 > raw.len() {
                bail!("Truncated MOBI section header table");
            }
            let mut c = Cursor::new(&raw[base..base + 8]);
            let offset = c.read_u32::<BigEndian>()?;
            let flags = c.read_u8()?;
            let a2 = c.read_u8()? as u32;
            let a3 = c.read_u8()? as u32;
            let a4 = c.read_u8()? as u32;
            let val = (a2 << 16) | (a3 << 8) | a4;
            section_headers.push(SectionHeader { offset, flags, val });
        }

        let mut sections = Vec::with_capacity(num_sections as usize);
        for i in 0..num_sections as usize {
            let start = section_headers[i].offset as usize;
            let end = if i == num_sections as usize - 1 {
                raw.len()
            } else {
                section_headers[i + 1].offset as usize
            };
            if start > end || end > raw.len() {
                bail!("Invalid MOBI section offsets");
            }
            sections.push(raw[start..end].to_vec());
        }
        if sections.is_empty() {
            bail!("No sections found in MOBI file");
        }

        let bh = BookHeader::parse(&sections[0], &ident, None, false)?;
        let name = decode_with_codec_owned(&name_stripped, &bh.codec);
        let mut book_header = bh.clone();
        let mut kf8_type = None;
        let mut kf8_boundary = None;
        let k8i = book_header.exth.as_ref().and_then(|e| e.kf8_header);

        // Ancient PRC files from Baen can have random values for
        // mobi_version, so be conservative.
        if book_header.mobi_version == 8 {
            kf8_type = Some("standalone".to_string());
        } else if let Some(k8i) = k8i {
            let k8i = k8i as usize;
            let prev = k8i.checked_sub(1).and_then(|i| sections.get(i));
            if prev.map(|s| s.as_slice()) == Some(b"BOUNDARY".as_slice()) {
                if let Ok(mut kf8_bh) = BookHeader::parse(&sections[k8i], &ident, None, false) {
                    kf8_bh.kf8_first_image_index =
                        Some(kf8_bh.first_image_index.wrapping_add(k8i as u32));
                    kf8_bh.mobi6_records = Some(bh.records);
                    kf8_bh.first_image_index = bh.first_image_index;
                    if let Some(ho) = kf8_bh.huff_offset {
                        kf8_bh.huff_offset = Some(ho + k8i as u32);
                    }
                    kf8_type = Some("joint".to_string());
                    kf8_boundary = Some(k8i - 1);
                    book_header = kf8_bh;
                }
            }
        }

        Ok(MobiReader {
            name,
            num_sections,
            sections,
            section_headers,
            book_header,
            kf8_type,
            kf8_boundary,
            embedded_mi: None,
            log: MobiLog::default(),
            tag_css_rules: IndexMap::new(),
            left_margins: HashMap::new(),
            text_indents: HashMap::new(),
            warned_about_trailing_entry_corruption: false,
            mobi_html: Vec::new(),
            processed_html: String::new(),
            image_names: Vec::new(),
            htmlfile: None,
            created_opf_path: None,
        })
    }

    pub fn check_for_drm(&self) -> Result<()> {
        if self.book_header.encryption_type != 0 {
            let name = self
                .book_header
                .exth
                .as_ref()
                .map(|e| e.mi.title.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| self.name.clone());
            return Err(MobiError::Drm(name).into());
        }
        Ok(())
    }

    fn warn_about_trailing_entry_corruption(&mut self) {
        if !self.warned_about_trailing_entry_corruption {
            self.warned_about_trailing_entry_corruption = true;
            self.log
                .warn("The trailing data entries in this MOBI file are corrupted, you might see corrupted text in the output");
        }
    }

    /// Port of `sizeof_trailing_entries`.
    fn sizeof_trailing_entries(&mut self, data: &[u8]) -> usize {
        fn sizeof_trailing_entry(data: &[u8], mut psize: usize) -> Option<usize> {
            let mut bitpos: u32 = 0;
            let mut result: usize = 0;
            loop {
                if psize == 0 {
                    return None;
                }
                let v = data[psize - 1];
                result |= ((v & 0x7F) as usize) << bitpos;
                bitpos += 7;
                psize -= 1;
                if (v & 0x80) != 0 || bitpos >= 28 || psize == 0 {
                    return Some(result);
                }
            }
        }

        let mut num: usize = 0;
        let size = data.len();
        let mut flags = self.book_header.extra_flags >> 1;
        while flags != 0 {
            if flags & 1 != 0 {
                if size < num {
                    self.warn_about_trailing_entry_corruption();
                    return 0;
                }
                match sizeof_trailing_entry(data, size - num) {
                    Some(v) => num += v,
                    None => {
                        self.warn_about_trailing_entry_corruption();
                        return 0;
                    }
                }
            }
            flags >>= 1;
        }
        if self.book_header.extra_flags & 1 != 0 {
            if size > num {
                let off = size - num - 1;
                num += (data[off] & 0x3) as usize + 1;
            } else {
                self.log.warn("Invalid sizeof trailing entries");
                num += 1;
            }
        }
        num
    }

    fn text_section(&mut self, index: usize) -> Vec<u8> {
        let data = self.sections[index].clone();
        let trail_size = self.sizeof_trailing_entries(&data);
        let keep = data.len().saturating_sub(trail_size);
        data[..keep].to_vec()
    }

    /// Port of `extract_text`. Returns the list of section indices already
    /// consumed as text/HUFF-CDIC records (`processed_records`).
    pub fn extract_text(&mut self, offset: usize) -> Result<Vec<usize>> {
        self.log.debug("Extracting text...");
        let end = (self.book_header.records as usize + offset).min(self.sections.len());
        let mut text_sections = Vec::new();
        for i in offset..end {
            text_sections.push(self.text_section(i));
        }
        let mut processed_records: Vec<usize> =
            (offset.saturating_sub(1)..(self.book_header.records as usize + offset)).collect();

        let mut mobi_html = Vec::new();
        match self.book_header.compression_type {
            17480 => {
                let huff_offset = self
                    .book_header
                    .huff_offset
                    .context("HUFF/CDIC compression flagged but huff_offset missing")?
                    as usize;
                let huff_number = self
                    .book_header
                    .huff_number
                    .context("HUFF/CDIC compression flagged but huff_number missing")?
                    as usize;
                let mut huffs = Vec::new();
                for i in huff_offset..huff_offset + huff_number {
                    huffs.push(
                        self.sections
                            .get(i)
                            .cloned()
                            .with_context(|| format!("Missing HUFF/CDIC record {i}"))?,
                    );
                }
                processed_records.extend(huff_offset..huff_offset + huff_number);
                let mut huff = HuffReader::new(&huffs)?;
                for sec in &text_sections {
                    mobi_html.extend(huff.unpack(sec)?);
                }
            }
            2 => {
                for sec in &text_sections {
                    mobi_html.extend(decompress_doc(sec)?);
                }
            }
            1 => {
                for sec in &text_sections {
                    mobi_html.extend_from_slice(sec);
                }
            }
            other => bail!("Unknown compression algorithm: {:?}", other),
        }
        if mobi_html.last() == Some(&b'#') {
            mobi_html.pop();
        }

        if self.book_header.ancient {
            let head_lower: Vec<u8> = mobi_html
                .iter()
                .take(300)
                .map(|b| b.to_ascii_lowercase())
                .collect();
            if !head_lower.windows(5).any(|w| w == b"<html") {
                mobi_html = replace_bytes(&mobi_html, b"\r ", b"\n\n ");
            }
        }
        mobi_html.retain(|&b| b != 0);
        if self.book_header.codec == "cp1252" {
            mobi_html.retain(|&b| b != 0x1e && b != 0x02);
        }
        self.mobi_html = mobi_html;
        Ok(processed_records)
    }

    /// Port of `replace_page_breaks`.
    pub fn replace_page_breaks(&mut self) {
        self.processed_html = PAGE_BREAK_PAT
            .replace_all(&self.processed_html, "<div $1 class=\"mbp_pagebreak\" />")
            .into_owned();
    }

    /// Port of `add_anchors`. Returns the anchor-annotated raw markup
    /// bytes (Python assigns this to `self.processed_html` while it is
    /// still `bytes`; here it stays a separate return value until the
    /// caller decodes it).
    pub fn add_anchors(&mut self) -> Vec<u8> {
        self.log.debug("Adding anchors...");
        let mut positions = std::collections::BTreeSet::new();
        for caps in LINK_PATTERN.captures_iter(&self.mobi_html) {
            if let Some(m) = caps.get(1) {
                if let Ok(s) = std::str::from_utf8(m.as_bytes()) {
                    if let Ok(n) = s.parse::<usize>() {
                        positions.insert(n);
                    }
                }
            }
        }

        let mut pos = 0usize;
        let mut out: Vec<u8> = Vec::new();
        for end0 in positions {
            if end0 == 0 {
                continue;
            }
            let oend = end0;
            let mut end = end0;
            let l = find_byte_from(&self.mobi_html, b'<', end0);
            let r = find_byte_from(&self.mobi_html, b'>', end0);
            let mut use_attr_form = false;

            if let Some(r_val) = r {
                let cond = match l {
                    None => true,
                    Some(l_val) => r_val < l_val || l_val == end0,
                };
                if cond {
                    let p = rfind_byte_upto(&self.mobi_html, b'<', end0 + 1);
                    match p {
                        Some(p_val) if pos < end0 => {
                            let slice_pr = &self.mobi_html[p_val..r_val];
                            let pr1_end = (r_val + 1).min(self.mobi_html.len());
                            let ends_self_close = self.mobi_html[p_val..pr1_end].ends_with(b"/>");
                            if !END_TAG_RE.is_match(slice_pr) && !ends_self_close {
                                use_attr_form = true;
                                end = r_val;
                            } else {
                                end = r_val + 1;
                            }
                        }
                        _ => end = r_val + 1,
                    }
                }
            }

            out.extend_from_slice(&self.mobi_html[pos..end]);
            if use_attr_form {
                out.extend_from_slice(format!(" filepos-id=\"filepos{oend}\"").as_bytes());
            } else {
                out.extend_from_slice(format!("<a id=\"filepos{oend}\"></a>").as_bytes());
            }
            pos = end;
        }
        out.extend_from_slice(&self.mobi_html[pos..]);

        ENTITY_FIX_RE
            .replace_all(&out, &b"&$1$3;$2"[..])
            .into_owned()
    }

    /// Port of `remove_random_bytes`.
    pub fn remove_random_bytes(&self, html: &str) -> String {
        RANDOM_BYTES_RE.replace_all(html, "").into_owned()
    }

    /// Port of `cleanup_html`.
    pub fn cleanup_html(&mut self) {
        self.log.debug("Cleaning up HTML...");
        self.processed_html = DIV_HEIGHT0_RE
            .replace_all(&self.processed_html, "")
            .into_owned();
        if self.book_header.ancient {
            let head_lower = self
                .mobi_html
                .iter()
                .take(300)
                .map(|b| b.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if !head_lower.windows(5).any(|w| w == b"<html") {
                self.processed_html = format!(
                    "<html><p>{}</html>",
                    self.processed_html.replace("\n\n", "<p>")
                );
            }
        }
        self.processed_html = self.processed_html.replace("\r\n", "\n");
        self.processed_html = self.processed_html.replace("> <", ">\n<");
        self.processed_html = self.processed_html.replace("<mbp: ", "<mbp:");
        self.processed_html = XML_DECL_RE
            .replace_all(&self.processed_html, "")
            .into_owned();
        self.processed_html = OP_TAG_RE.replace_all(&self.processed_html, "").into_owned();
        self.processed_html = STYLE_BEFORE_P_RE
            .replace_all(&self.processed_html, "$para$styletags")
            .into_owned();
        self.processed_html = STYLE_AFTER_P_CLOSE_RE
            .replace_all(&self.processed_html, "$styletags$para")
            .into_owned();
        self.processed_html = BQ_BEFORE_PCLOSE_RE
            .replace_all(&self.processed_html, "$para$blockquote")
            .into_owned();
        self.processed_html = BQ_AFTER_POPEN_RE
            .replace_all(&self.processed_html, "$blockquote$para")
            .into_owned();

        let bods = self.processed_html.matches("</body>").count();
        let htmls = self.processed_html.matches("</html>").count();
        if bods > 1 {
            self.processed_html = self.processed_html.replace("</body>", "");
        }
        if htmls > 1 {
            self.processed_html = self.processed_html.replace("</html>", "");
        }
    }

    /// Port of `extract_images`. `processed_records` gets image record
    /// indices appended to it (matching Python's in-place mutation of the
    /// list). Returns the 1-based `image_index -> filename` map used by
    /// `upshift_markup` to resolve `recindex` attributes.
    pub fn extract_images(
        &mut self,
        processed_records: &mut Vec<usize>,
        output_dir: &Path,
    ) -> Result<HashMap<usize, String>> {
        self.log.debug("Extracting images...");
        let images_dir = output_dir.join("images");
        std::fs::create_dir_all(&images_dir)?;

        let mut image_index = 0usize;
        self.image_names.clear();
        let mut image_name_map = HashMap::new();

        let mut start = self.book_header.first_image_index as usize;
        if start > self.sections.len() {
            start = 0;
        }

        for i in start..self.sections.len() {
            if processed_records.contains(&i) {
                continue;
            }
            processed_records.push(i);
            let data = self.sections[i].clone();
            image_index += 1;

            const NON_IMAGE_SIGS: [&[u8]; 10] = [
                b"FLIS",
                b"FCIS",
                b"SRCS",
                b"\xe9\x8e\r\n",
                b"RESC",
                b"BOUN",
                b"FDST",
                b"DATP",
                b"AUDI",
                b"VIDE",
            ];
            if NON_IMAGE_SIGS.iter().any(|sig| data.starts_with(sig)) {
                continue;
            }

            let mut imgfmt = crate::mobi::containers::find_imgtype(&data).to_string();
            if imgfmt == "unknown" {
                continue;
            }
            if imgfmt == "jpeg" {
                imgfmt = "jpg".to_string();
            }
            if !matches!(imgfmt.as_str(), "jpg" | "gif" | "png" | "bmp") {
                continue;
            }
            // NOTE: Python transcodes GIF -> PNG here (`gif_data_to_png_data`)
            // and minifies covers via `save_cover_data_to`. Neither an image
            // codec nor a resize routine is available in this workspace
            // (no C toolchain / image crate wired up for calibre_ebooks), so
            // this is a documented, narrow gap: GIFs are written as-is
            // (`.gif`) instead of being re-encoded to PNG, and no
            // downscaling is applied. Image *extraction* itself -- sniffing,
            // sequencing, and writing files -- is fully real.
            let filename = format!("{image_index:05}.{imgfmt}");
            let path = images_dir.join(&filename);
            if std::fs::write(&path, &data).is_err() {
                continue;
            }
            image_name_map.insert(image_index, filename.clone());
            self.image_names.push(filename);
        }

        Ok(image_name_map)
    }

    fn ensure_unit(raw: &str, unit: &str) -> String {
        if TRAILING_DIGIT_RE.is_match(raw) {
            format!("{raw}{unit}")
        } else {
            raw.to_string()
        }
    }

    /// Port of `upshift_markup`: walks every element of the parsed DOM and
    /// rewrites presentational MOBI markup (`<font size=...>`,
    /// `height=`/`width=`/`align=` attributes, `filepos`/`filepos-id`,
    /// `<i>`/`<u>`/`<b>`, image `recindex`, `<svg>` unwrapping, anchor
    /// forwarding) into CSS-driven XHTML.
    pub fn upshift_markup(&mut self, dom: &mut Dom, image_name_map: &HashMap<usize, String>) {
        self.log.debug("Converting style information to CSS...");
        let size_map: HashMap<&str, &str> = [
            ("xx-small", "0.5"),
            ("x-small", "1"),
            ("small", "2"),
            ("medium", "3"),
            ("large", "4"),
            ("x-large", "5"),
            ("xx-large", "6"),
        ]
        .into_iter()
        .collect();

        let mobi_version = self.book_header.mobi_version;
        const BLOCK_TAGS: [&str; 7] = ["h1", "h2", "h3", "h4", "h5", "h6", "div"];

        for ncx_el in dom.find_all_tag_global("ncx") {
            dom.remove_promoting_children(ncx_el);
        }

        let mut svg_tags = Vec::new();
        let mut forwardable_anchors = Vec::new();
        let mut pagebreak_anchors = Vec::new();

        let elements = dom.preorder_elements(dom.root);
        for (i, &tag) in elements.iter().enumerate() {
            let tag_name = dom.tag(tag).unwrap_or("").to_string();

            dom.node_mut(tag).attrs.shift_remove("xmlns");
            let colon_keys: Vec<String> = dom
                .node(tag)
                .attrs
                .keys()
                .filter(|k| k.contains(':'))
                .cloned()
                .collect();
            for k in colon_keys {
                dom.node_mut(tag).attrs.shift_remove(&k);
            }

            if tag_name.eq_ignore_ascii_case("svg") {
                svg_tags.push(tag);
            }

            if matches!(
                tag_name.to_lowercase().as_str(),
                "country-region"
                    | "place"
                    | "placetype"
                    | "placename"
                    | "state"
                    | "city"
                    | "street"
                    | "address"
                    | "content"
                    | "form"
            ) {
                let new_tag = if tag_name == "content" || tag_name == "form" {
                    "div"
                } else {
                    "span"
                };
                dom.set_tag(tag, new_tag);
                dom.node_mut(tag).attrs.clear();
                continue;
            }

            let mut styles: Vec<String> = Vec::new();

            if let Some(style) = dom.node_mut(tag).attrs.shift_remove("style") {
                let style = style.trim().to_string();
                if !style.is_empty() {
                    styles.push(style);
                }
            }

            let current_tag = dom.tag(tag).unwrap_or("").to_string();

            if let Some(height) = dom.node_mut(tag).attrs.shift_remove("height") {
                let height = height.trim().to_string();
                if !height.is_empty()
                    && !height.contains('<')
                    && !height.contains('>')
                    && HAS_DIGIT_RE.is_match(&height)
                {
                    if matches!(current_tag.as_str(), "table" | "td" | "tr") {
                        // no-op
                    } else if current_tag == "img" {
                        dom.node_mut(tag).attrs.insert("height".to_string(), height);
                    } else if current_tag == "div"
                        && dom.children(tag).is_empty()
                        && tail_is_whitespace(dom, tag)
                    {
                        let nbsp = dom.new_text("\u{a0}");
                        dom.insert_child(tag, 0, nbsp);
                        styles.push(format!("height: {}", Self::ensure_unit(&height, "px")));
                    } else {
                        styles.push(format!("margin-top: {}", Self::ensure_unit(&height, "px")));
                    }
                }
            }

            if let Some(width) = dom.node_mut(tag).attrs.shift_remove("width") {
                let width = width.trim().to_string();
                if !width.is_empty() && HAS_DIGIT_RE.is_match(&width) {
                    if matches!(current_tag.as_str(), "table" | "td" | "tr") {
                        // no-op
                    } else if current_tag == "img" {
                        dom.node_mut(tag).attrs.insert("width".to_string(), width);
                    } else {
                        let ewidth = Self::ensure_unit(&width, "px");
                        styles.push(format!("text-indent: {ewidth}"));
                        if let Some(v) = unit_convert(&ewidth, 12.0, 500.0, 166.0) {
                            self.text_indents.insert(tag, v);
                        }
                        if let Some(stripped) = width.strip_prefix('-') {
                            styles.push(format!(
                                "margin-left: {}",
                                Self::ensure_unit(stripped, "px")
                            ));
                            if let Some(v) =
                                unit_convert(&Self::ensure_unit(stripped, "px"), 12.0, 500.0, 166.0)
                            {
                                self.left_margins.insert(tag, v);
                            }
                        }
                    }
                }
            }

            if let Some(align) = dom.node_mut(tag).attrs.shift_remove("align") {
                let align = align.trim().to_lowercase();
                if !align.is_empty() {
                    if align == "baseline" {
                        styles.push(format!("vertical-align: {align}"));
                    } else {
                        styles.push(format!("text-align: {align}"));
                    }
                }
            }

            match current_tag.as_str() {
                "hr" => {
                    if mobi_version == 1 {
                        dom.set_tag(tag, "div");
                        styles.push("page-break-before: always".to_string());
                        styles.push("display: block".to_string());
                        styles.push("margin: 0".to_string());
                    }
                }
                "i" => {
                    dom.set_tag(tag, "span");
                    dom.node_mut(tag)
                        .attrs
                        .insert("class".to_string(), "italic".to_string());
                }
                "u" => {
                    dom.set_tag(tag, "span");
                    dom.node_mut(tag)
                        .attrs
                        .insert("class".to_string(), "underline".to_string());
                }
                "b" => {
                    dom.set_tag(tag, "span");
                    dom.node_mut(tag)
                        .attrs
                        .insert("class".to_string(), "bold".to_string());
                }
                "font" => {
                    let sz = dom
                        .node(tag)
                        .attrs
                        .get("size")
                        .cloned()
                        .unwrap_or_default()
                        .to_lowercase();
                    if sz.parse::<f64>().is_err() {
                        if let Some(mapped) = size_map.get(sz.as_str()) {
                            dom.node_mut(tag)
                                .attrs
                                .insert("size".to_string(), (*mapped).to_string());
                        }
                    }
                }
                "img" => {
                    let mut recindex: Option<String> = None;
                    for attr in IMAGE_ATTRS {
                        if let Some(v) = dom.node_mut(tag).attrs.shift_remove(attr) {
                            recindex = Some(v);
                        }
                    }
                    if let Some(recindex) = recindex {
                        if let Ok(idx) = recindex.parse::<usize>() {
                            let fname = image_name_map
                                .get(&idx)
                                .cloned()
                                .unwrap_or_else(|| format!("{idx:05}.jpg"));
                            dom.node_mut(tag)
                                .attrs
                                .insert("src".to_string(), format!("images/{fname}"));
                        }
                    }
                    for attr in ["width", "height"] {
                        if let Some(val) = dom.node(tag).attrs.get(attr).cloned() {
                            let lower = val.to_lowercase();
                            if let Some(stripped) = lower.strip_suffix("em") {
                                if let Ok(nval) = stripped.parse::<f64>() {
                                    let nval = nval * 16.0 * (168.451 / 72.0);
                                    dom.node_mut(tag)
                                        .attrs
                                        .insert(attr.to_string(), format!("{}px", nval as i64));
                                } else {
                                    dom.node_mut(tag).attrs.shift_remove(attr);
                                }
                            } else if lower.ends_with('%') {
                                dom.node_mut(tag).attrs.shift_remove(attr);
                            }
                        }
                    }
                }
                "pre" => {
                    let has_text = dom.children(tag).iter().any(|&c| matches!(&dom.node(c).kind, crate::dom::NodeKind::Text(t) if !t.is_empty()));
                    if !has_text {
                        dom.set_tag(tag, "div");
                    }
                }
                _ => {}
            }

            let attrs_snapshot = dom.node(tag).attrs.clone();
            if attrs_snapshot.get("class").map(|s| s.as_str()) == Some("mbp_pagebreak")
                && dom.tag(tag) == Some("div")
                && attrs_snapshot.contains_key("filepos-id")
            {
                pagebreak_anchors.push(tag);
            }

            if let Some(color) = dom.node_mut(tag).attrs.shift_remove("color") {
                styles.push(format!("color: {color}"));
            }
            if let Some(bgcolor) = dom.node_mut(tag).attrs.shift_remove("bgcolor") {
                styles.push(format!("background-color: {bgcolor}"));
            }

            if let Some(fpid) = dom.node_mut(tag).attrs.shift_remove("filepos-id") {
                let name_matches = dom
                    .node(tag)
                    .attrs
                    .get("name")
                    .map(|n| n != &fpid)
                    .unwrap_or(false);
                dom.node_mut(tag)
                    .attrs
                    .insert("id".to_string(), fpid.clone());
                if name_matches {
                    dom.node_mut(tag).attrs.insert("name".to_string(), fpid);
                }
            }
            if let Some(filepos) = dom.node_mut(tag).attrs.shift_remove("filepos") {
                if let Ok(n) = filepos.parse::<i64>() {
                    dom.node_mut(tag)
                        .attrs
                        .insert("href".to_string(), format!("#filepos{n}"));
                }
            }

            let is_a = dom.tag(tag) == Some("a");
            if is_a {
                let id_attr = dom.node(tag).attrs.get("id").cloned().unwrap_or_default();
                let no_text = dom.children(tag).is_empty();
                if id_attr.starts_with("filepos") && no_text && tail_is_whitespace(dom, tag) {
                    if let Some(next) = dom.next_element_sibling(tag) {
                        if let Some(nt) = dom.tag(next) {
                            if BLOCK_TAGS.contains(&nt) {
                                forwardable_anchors.push(tag);
                            }
                        }
                    }
                }
            }

            if !styles.is_empty() {
                let rule = styles.join("; ");
                let ncls = self
                    .tag_css_rules
                    .iter()
                    .find(|(_, v)| **v == rule)
                    .map(|(k, _)| k.clone())
                    .unwrap_or_else(|| {
                        let cls = format!("calibre_{i}");
                        self.tag_css_rules.insert(cls.clone(), rule);
                        cls
                    });
                let cls = dom
                    .node(tag)
                    .attrs
                    .get("class")
                    .cloned()
                    .unwrap_or_default();
                let new_cls = if cls.is_empty() {
                    ncls
                } else {
                    format!("{cls} {ncls}")
                };
                dom.node_mut(tag).attrs.insert("class".to_string(), new_cls);
            }
        }

        for tag in svg_tags {
            let images = dom.find_all_tag(tag, "img");
            if let Some(parent) = dom.parent(tag) {
                if !images.is_empty() {
                    if let Some(index) = dom.index_in_parent(tag) {
                        for (offset, img) in images.into_iter().enumerate() {
                            dom.insert_child(parent, index + offset, img);
                        }
                    }
                }
                dom.remove_promoting_children(tag);
            }
        }

        for tag in pagebreak_anchors {
            let anchor = dom.node_mut(tag).attrs.shift_remove("id");
            dom.node_mut(tag).attrs.shift_remove("name");
            if let (Some(anchor), Some(parent)) = (anchor, dom.parent(tag)) {
                let a = dom.new_element("a");
                dom.node_mut(a).attrs.insert("id".to_string(), anchor);
                if let Some(idx) = dom.index_in_parent(tag) {
                    dom.insert_child(parent, idx + 1, a);
                }
                if let Some(next) = dom.next_element_sibling(a) {
                    if let Some(nt) = dom.tag(next) {
                        if BLOCK_TAGS.contains(&nt) {
                            forwardable_anchors.push(a);
                        }
                    }
                }
            }
        }

        for tag in forwardable_anchors {
            let Some(block) = dom.next_element_sibling(tag) else {
                continue;
            };
            let anchor_id = dom.node(tag).attrs.get("id").cloned();
            dom.detach(tag);
            if dom.node(block).attrs.contains_key("id") {
                dom.insert_child(block, 0, tag);
            } else if let Some(id) = anchor_id {
                dom.node_mut(block).attrs.insert("id".to_string(), id);
            }
        }

        // WebKit fails to navigate to anchors located on <br> tags.
        for br in dom.find_all_tag_global("br") {
            if dom.node(br).attrs.contains_key("id") {
                dom.set_tag(br, "div");
            }
        }
    }

    fn get_left_whitespace(&self, dom: &Dom, tag: NodeId) -> f64 {
        fn whitespace(reader: &MobiReader, dom: &Dom, tag: NodeId) -> f64 {
            let mut lm = 0.0;
            let mut ti = 0.0;
            if dom.tag(tag) == Some("p") {
                ti = unit_convert("1.5em", 12.0, 500.0, 166.0).unwrap_or(0.0);
            }
            if dom.tag(tag) == Some("blockquote") {
                lm = unit_convert("2em", 12.0, 500.0, 166.0).unwrap_or(0.0);
            }
            lm = *reader.left_margins.get(&tag).unwrap_or(&lm);
            ti = *reader.text_indents.get(&tag).unwrap_or(&ti);
            lm + ti
        }

        let mut ans = 0.0;
        let mut parent = Some(tag);
        while let Some(p) = parent {
            ans += whitespace(self, dom, p);
            parent = dom.parent(p);
        }
        ans
    }

    /// Port of `read_embedded_metadata`: parses a `<metadata>` element (an
    /// OEB-embedded OPF `<package>`, present when the source file has no
    /// EXTH header) directly off the DOM rather than round-tripping
    /// through `lxml`+`OPF.to_book_metadata()`. Covers the fields real
    /// callers rely on: title, creators, and (via the `guide`) cover.
    pub fn read_embedded_metadata(&mut self, dom: &mut Dom, elem: NodeId, guide: Option<NodeId>) {
        let mut mi = MetaInformation::new("Unknown", vec!["Unknown".to_string()]);
        if let Some(title_el) = dom
            .find_all_tag(elem, "title")
            .into_iter()
            .chain(dom.find_all_tag(elem, "dc:title"))
            .next()
        {
            let t = dom.text_content(title_el).trim().to_string();
            if !t.is_empty() {
                mi.title = t;
            }
        }
        let mut authors = Vec::new();
        for tag in ["creator", "dc:creator"] {
            for c in dom.find_all_tag(elem, tag) {
                let t = dom.text_content(c).trim().to_string();
                if !t.is_empty() {
                    authors.push(t);
                }
            }
        }
        if !authors.is_empty() {
            mi.authors = authors;
        }

        if let Some(guide) = guide {
            for reference in dom.find_all_tag(guide, "reference") {
                let type_ = dom
                    .node(reference)
                    .attrs
                    .get("type")
                    .cloned()
                    .unwrap_or_default();
                if type_.to_lowercase().contains("cover") {
                    let href = dom
                        .node(reference)
                        .attrs
                        .get("href")
                        .cloned()
                        .unwrap_or_default();
                    let href = href.strip_prefix('#').unwrap_or(&href).to_string();
                    if let Some(anchor) = dom.find_by_id(&href) {
                        // Walk forward from the anchor for the first <img>.
                        let all = dom.preorder_elements(dom.root);
                        if let Some(pos) = all.iter().position(|&n| n == anchor) {
                            for &cand in &all[pos..] {
                                if dom.tag(cand) == Some("img") {
                                    if let Some(src) = dom.node(cand).attrs.get("src").cloned() {
                                        mi.cover_id = Some(src);
                                    }
                                    dom.remove_promoting_children(cand);
                                    break;
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
        self.embedded_mi = Some(mi);
    }

    /// Port of `structure_toc`: re-nests a flat TOC (each entry tagged
    /// with its rendered left-indentation) into a tree, using the set of
    /// distinct indentation values as level boundaries. Gives up (returns
    /// `flat` unchanged) if there are too few/many distinct levels to be a
    /// meaningful hierarchy, matching Python.
    fn structure_toc(flat: &[(String, String, String, i64)]) -> TOC {
        let mut indent_vals: Vec<i64> = flat.iter().map(|(_, _, _, l)| *l).collect();
        indent_vals.sort_unstable();
        indent_vals.dedup();
        if indent_vals.len() > 6 || indent_vals.len() < 2 {
            let mut toc = TOC::new();
            toc.nodes = flat
                .iter()
                .map(|(href, frag, text, _)| TOCNode {
                    title: text.clone(),
                    src: join_href(href, frag),
                    children: Vec::new(),
                })
                .collect();
            return toc;
        }

        let mut root_children: Vec<TOCNode> = Vec::new();
        // `last_found[level]` holds the *path* of indices into the tree
        // built so far that leads to the most-recently-added node at that
        // level.
        let mut last_found: Vec<Option<Vec<usize>>> = vec![None; indent_vals.len()];

        fn get_mut<'a>(root: &'a mut Vec<TOCNode>, path: &[usize]) -> &'a mut Vec<TOCNode> {
            let mut cur = root;
            for &idx in path {
                cur = &mut cur[idx].children;
            }
            cur
        }

        for (href, frag, text, left) in flat {
            let level = indent_vals.iter().position(|v| v == left).unwrap_or(0);
            let parent_path: Vec<usize> = (0..level)
                .rev()
                .find_map(|l| last_found[l].clone())
                .unwrap_or_default();
            let siblings = get_mut(&mut root_children, &parent_path);
            siblings.push(TOCNode {
                title: text.clone(),
                src: join_href(href, frag),
                children: Vec::new(),
            });
            let mut new_path = parent_path;
            new_path.push(siblings.len() - 1);
            last_found[level] = Some(new_path);
        }

        TOC {
            nodes: root_children,
        }
    }

    /// Port of `create_opf`. Returns the rendered OPF XML and, if a TOC
    /// was found, the rendered NCX XML.
    pub fn create_opf(
        &mut self,
        dom: &Dom,
        guide: Option<NodeId>,
    ) -> Result<(String, Option<String>)> {
        let mi = self
            .book_header
            .exth
            .as_ref()
            .map(|e| e.mi.clone())
            .or_else(|| self.embedded_mi.clone())
            .unwrap_or_else(|| {
                MetaInformation::new(&self.book_header.title, vec!["Unknown".to_string()])
            });

        let cover_href = self
            .book_header
            .exth
            .as_ref()
            .and_then(|e| e.cover_offset)
            .map(|off| format!("images/{:05}.jpg", off + 1))
            .or_else(|| {
                if self.image_names.iter().any(|n| n == "00001.jpg") {
                    Some("images/00001.jpg".to_string())
                } else {
                    None
                }
            });

        let mut manifest_pairs = vec![
            (
                "index.html".to_string(),
                "application/xhtml+xml".to_string(),
            ),
            ("styles.css".to_string(), "text/css".to_string()),
        ];
        for name in &self.image_names {
            let mt = mime_guess::from_path(name)
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "image/jpeg".to_string());
            manifest_pairs.push((format!("images/{name}"), mt));
        }
        let manifest = opf_writer::auto_manifest(&manifest_pairs);
        let spine = vec!["id1".to_string()];

        let mut guide_refs = Vec::new();
        let mut toc_href: Option<String> = None;
        if let Some(guide) = guide {
            for reference in dom.find_all_tag(guide, "reference") {
                let attrs = &dom.node(reference).attrs;
                let type_ = attrs.get("type").cloned().unwrap_or_default();
                let title = attrs.get("title").cloned().unwrap_or_default();
                let href = attrs.get("href").cloned().unwrap_or_default();
                if type_.eq_ignore_ascii_case("toc") {
                    toc_href = Some(href.clone());
                }
                guide_refs.push(GuideRef { type_, title, href });
            }
        }

        let mut ncx_xml = None;
        let ncx_manifest_id: Option<&str> = if toc_href.is_some() {
            Some("ncx")
        } else {
            None
        };

        if let Some(toc_href) = &toc_href {
            let frag = toc_href.split('#').nth(1).unwrap_or("");
            if let Some(anchor) = dom.find_by_id(frag) {
                let all = dom.preorder_elements(dom.root);
                if let Some(pos) = all.iter().position(|&n| n == anchor) {
                    let mut flat = Vec::new();
                    let mut found = false;
                    for &x in &all[pos + 1..] {
                        if dom.tag(x) == Some("a") {
                            if let Some(href) = dom.node(x).attrs.get("href") {
                                if !href.is_empty() && !URL_SCHEME_RE.is_match(href) {
                                    let text = decode_entities(dom.text_content(x).trim());
                                    let (base, frag) = split_href(href);
                                    let left = self.get_left_whitespace(dom, x) as i64;
                                    flat.push((base, frag, text, left));
                                    found = true;
                                }
                            }
                        }
                        if found
                            && dom.node(x).attrs.get("class").map(|s| s.as_str())
                                == Some("mbp_pagebreak")
                        {
                            break;
                        }
                    }
                    if !flat.is_empty() {
                        let toc = Self::structure_toc(&flat);
                        ncx_xml = Some(opf_writer::write_ncx(
                            &toc,
                            mi.uuid.as_deref().unwrap_or("unknown"),
                            &mi.title,
                        ));
                    }
                }
            }
        }

        let opf = opf_writer::write_opf(
            &mi,
            &manifest,
            &spine,
            &guide_refs,
            ncx_manifest_id,
            cover_href.as_deref(),
            None,
            None,
        );
        Ok((opf, ncx_xml))
    }

    /// Port of `MobiReader.extract_content`: the top-level orchestration
    /// that produces `index.html`, `styles.css`, `images/*`,
    /// `metadata.opf` and (if present) `toc.ncx` under `output_dir`.
    ///
    /// Python drives a multi-tier parser fallback ladder here
    /// (`lxml.html` -> strip control bytes -> `html5-parser` -> strip
    /// `<metadata>`/`<guide>` -> strip stray `</html>`) because `lxml`'s
    /// HTML parser is comparatively strict. `html5ever`'s tree
    /// construction is HTML5-spec tag-soup tolerant by design and always
    /// yields a well-formed `<html><head/><body/></html>` skeleton, so
    /// that ladder collapses to a single parse here; `remove_random_bytes`
    /// is still applied unconditionally as a cheap pre-pass since it only
    /// strips characters that have no legitimate use in the markup anyway.
    pub fn extract_content(&mut self, output_dir: &Path) -> Result<()> {
        let output_dir =
            std::fs::canonicalize(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
        self.check_for_drm()?;
        let mut processed_records = self.extract_text(1)?;
        let raw_anchored = self.add_anchors();

        let codec_name = self.book_header.codec.clone();
        let mut processed = decode_with_codec_owned(&raw_anchored, &codec_name);
        processed = processed.replace("</</", "</");
        processed = DOUBLE_CLOSE_RE
            .replace_all(&processed, "</$1><")
            .into_owned();
        processed = processed.replace('\u{feff}', "");
        processed = NS_TAG_RE.replace_all(&processed, "").into_owned();
        processed = String::from_utf8_lossy(&strip_encoding_declarations(
            processed.as_bytes(),
            processed.len().max(1),
            true,
        ))
        .into_owned();
        processed = xml_replace_entities(&processed);
        self.processed_html = self.remove_random_bytes(&processed);

        let image_name_map = self.extract_images(&mut processed_records, &output_dir)?;
        self.replace_page_breaks();
        self.cleanup_html();

        self.processed_html = clean_xml_chars(&self.processed_html);
        let mut dom = Dom::parse(&self.processed_html);

        for script in dom.find_all_tag_global("script") {
            dom.detach(script);
        }

        let head = dom.find_first_tag_global("head").unwrap_or_else(|| {
            let h = dom.new_element("head");
            dom.insert_child(dom.root_html().unwrap_or(dom.root), 0, h);
            h
        });
        let link = dom.new_element("link");
        dom.node_mut(link)
            .attrs
            .insert("type".to_string(), "text/css".to_string());
        dom.node_mut(link)
            .attrs
            .insert("href".to_string(), "styles.css".to_string());
        dom.node_mut(link)
            .attrs
            .insert("rel".to_string(), "stylesheet".to_string());
        dom.insert_child(head, 0, link);

        let meta = dom.new_element("meta");
        dom.node_mut(meta)
            .attrs
            .insert("http-equiv".to_string(), "Content-Type".to_string());
        dom.node_mut(meta).attrs.insert(
            "content".to_string(),
            "text/html; charset=utf-8".to_string(),
        );
        dom.insert_child(head, 0, meta);

        if dom.find_all_tag(head, "title").is_empty() {
            let title = dom.new_element("title");
            let text = dom.new_text(&self.book_header.title);
            dom.append_child(title, text);
            dom.insert_child(head, 0, title);
        }

        self.upshift_markup(&mut dom, &image_name_map);

        let guide = dom.find_first_tag_global("guide");
        let metadata_elems = dom.find_all_tag_global("metadata");
        if !metadata_elems.is_empty() && self.book_header.exth.is_none() {
            self.read_embedded_metadata(&mut dom, metadata_elems[0], guide);
        }
        for elem in metadata_elems.into_iter().chain(guide) {
            dom.detach(elem);
        }

        let htmlfile = output_dir.join("index.html");
        self.htmlfile = Some(htmlfile.clone());

        let (opf_xml, ncx_xml) = self.create_opf(&dom, guide)?;
        let opf_path = htmlfile.with_extension("opf");
        self.created_opf_path = Some(opf_path.clone());

        let body = dom.find_first_tag_global("body").unwrap_or(dom.root);
        let html_out = dom.serialize(dom.root_html().unwrap_or(body));
        std::fs::write(&htmlfile, html_out)?;

        std::fs::write(&opf_path, &opf_xml)?;
        if let Some(ncx) = &ncx_xml {
            std::fs::write(output_dir.join("toc.ncx"), ncx)?;
        }

        let mut css = String::new();
        css.push_str(BASE_CSS_RULES);
        css.push_str("\n\n");
        for (cls, rule) in &self.tag_css_rules {
            css.push_str(&format!(".{cls} {{ {rule} }}\n\n"));
        }
        std::fs::write(output_dir.join("styles.css"), css)?;

        Ok(())
    }
}

fn decode_with_codec_owned(data: &[u8], codec: &str) -> String {
    if codec.eq_ignore_ascii_case("utf-8") {
        String::from_utf8_lossy(data).into_owned()
    } else {
        match encoding_rs::Encoding::for_label(codec.as_bytes()) {
            Some(enc) => enc.decode(data).0.into_owned(),
            None => String::from_utf8_lossy(data).into_owned(),
        }
    }
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return data.to_vec();
    }
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

fn split_href(href: &str) -> (String, String) {
    match href.split_once('#') {
        Some((b, f)) => (b.to_string(), f.to_string()),
        None => (href.to_string(), String::new()),
    }
}

fn join_href(href: &str, frag: &str) -> String {
    if frag.is_empty() {
        href.to_string()
    } else {
        format!("{href}#{frag}")
    }
}

impl Dom {
    /// The `<html>` root element, if the document has one (it always will
    /// after `html5ever`'s tree construction, but callers that build a
    /// `Dom` by hand -- e.g. tests -- may not have one).
    pub fn root_html(&self) -> Option<NodeId> {
        self.find_first_tag_global("html")
    }
}
