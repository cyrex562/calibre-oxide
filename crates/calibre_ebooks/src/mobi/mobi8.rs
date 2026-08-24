//! Port of `calibre.ebooks.mobi.reader.mobi8.Mobi8Reader` -- the KF8
//! reader. Reassembles the skeleton+fragment tables of a KF8-format
//! `.mobi`/`.azw3` into per-chapter XHTML files, extracts fonts/images
//! from the CONT/CRES resource records, and writes `metadata.opf` +
//! `toc.ncx`.
//!
//! Builds on [`crate::mobi::mobi6::MobiReader`], which already parsed the
//! PDB envelope, decompressed the raw text stream, and (for a "joint"
//! MOBI6+KF8 file) selected the KF8 `BookHeader`.

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use regex::bytes::Regex as BytesRegex;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::dom::Dom;
use crate::metadata::toc::{TOCNode, TOC};
use crate::mobi::containers::{find_imgtype, Container};
use crate::mobi::headers::NULL_INDEX;
use crate::mobi::index::read_index;
use crate::mobi::markup::{expand_mobi8_markup, FlowInfo as MarkupFlowInfo, MobiReaderTrait};
use crate::mobi::mobi6::MobiReader;
use crate::mobi::ncx::{build_toc, read_ncx};
use crate::mobi::utils::{read_font_record, DEFAULT_FONT_XOR_EXTENT};
use crate::mobi::MobiLog;
use crate::opf_writer::{self, GuideRef};

lazy_static! {
    static ref ID_RE: BytesRegex =
        BytesRegex::new(r#"(?i)<[^>]+\s(?:id)\s*=\s*['"]([^'"]+)['"]"#).unwrap();
    static ref NAME_RE: BytesRegex =
        BytesRegex::new(r#"(?i)<\s*a\s+(?:name)\s*=\s*['"]([^'"]+)['"]"#).unwrap();
    static ref AID_RE: BytesRegex =
        BytesRegex::new(r#"(?i)<[^>]+\s(?:aid)\s*=\s*['"]([^'"]+)['"]"#).unwrap();
}

/// A reconstructed source part (chapter file). Mirrors `Part` in
/// `mobi8.py`.
#[derive(Debug, Clone)]
pub struct Part {
    pub num: usize,
    pub type_: String,
    pub filename: String,
    pub start: u64,
    pub end: u64,
    pub aid: String,
}

#[derive(Debug, Clone)]
struct SkelFile {
    file_number: usize,
    name: String,
    divtbl_count: u64,
    start_position: u64,
    length: u64,
}

#[derive(Debug, Clone)]
struct Elem {
    insert_pos: i64,
    toc_text: String,
    file_number: u64,
    /// `seqnm` in `mobi8.py` -- parsed from the div index (tag 4) for
    /// parity with the Python tuple layout, but unused by the reader
    /// pipeline there too (Python's `get_id_tag_by_pos_fid` unpacks it
    /// into a variable it never reads either).
    #[allow(dead_code)]
    sequence_number: u64,
    start_pos: u64,
    length: u64,
}

#[derive(Debug, Clone)]
struct GuideItem {
    type_: String,
    title: String,
    pos_fid: Option<(u32, u32)>,
}

fn get_first_resource_index(
    first_image_index: u32,
    num_of_text_records: u32,
    first_text_record_number: usize,
) -> usize {
    if first_image_index == NULL_INDEX {
        num_of_text_records as usize + first_text_record_number
    } else {
        first_image_index as usize
    }
}

/// Iterates tags in `block` from the last one to the first.
fn reverse_tag_iter(block: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut end = block.len();
    while let Some(pgt) = block[..end].iter().rposition(|&b| b == b'>') {
        let Some(plt) = block[..pgt].iter().rposition(|&b| b == b'<') else {
            break;
        };
        out.push(block[plt..=pgt].to_vec());
        end = plt;
    }
    out
}

/// Locates the start/end byte offsets of the tag carrying `aid="<aid>"` in
/// `ml`.
fn locate_beg_end_of_tag(ml: &[u8], aid: &[u8]) -> (usize, usize) {
    let mut pattern = Vec::new();
    pattern.extend_from_slice(br#"<[^>]*\said\s*=\s*['"]"#);
    pattern.extend_from_slice(&regex_escape_bytes(aid));
    pattern.extend_from_slice(br#"['"][^>]*>"#);
    let Ok(re) = BytesRegex::new(&String::from_utf8_lossy(&pattern)) else {
        return (0, 0);
    };
    if let Some(m) = re.find(ml) {
        let plt = m.start();
        if let Some(pgt) = ml[plt..].iter().position(|&b| b == b'>') {
            return (plt, plt + pgt);
        }
    }
    (0, 0)
}

fn regex_escape_bytes(raw: &[u8]) -> Vec<u8> {
    regex::escape(&String::from_utf8_lossy(raw)).into_bytes()
}

pub struct Mobi8Reader {
    pub mobi6_reader: MobiReader,
    pub log: MobiLog,
    pub for_tweak: bool,

    pub encrypted_fonts: Vec<String>,
    /// Interior mutability: populated by [`Mobi8Reader::get_id_tag`],
    /// which needs to be callable through the `&self`-only
    /// [`MobiReaderTrait`] (Python's equivalent, `get_id_tag`, freely
    /// mutates `self.linked_aids` because Python has no borrow checker).
    pub linked_aids: RefCell<HashSet<String>>,
    pub aid_anchor_suffix: String,

    resource_offsets: Vec<(usize, usize)>,
    processed_records: Vec<usize>,
    raw_ml: Vec<u8>,
    kf8_sections: Vec<Vec<u8>>,
    cover_offset: Option<u32>,

    flow_table: Vec<(u32, u32)>,
    files: Vec<SkelFile>,
    elems: Vec<Elem>,
    guide: Vec<GuideItem>,

    pub flows: Vec<Vec<u8>>,
    flowinfo: Vec<Option<(String, String, String, String)>>, // (type, format, dir, fname)
    pub parts: Vec<Vec<u8>>,
    partinfo: Vec<Part>,
}

impl Mobi8Reader {
    pub fn new(mobi6_reader: MobiReader, log: MobiLog, for_tweak: bool) -> Self {
        Mobi8Reader {
            mobi6_reader,
            log,
            for_tweak,
            encrypted_fonts: Vec::new(),
            linked_aids: RefCell::new(HashSet::new()),
            aid_anchor_suffix: uuid_hex(),
            resource_offsets: Vec::new(),
            processed_records: Vec::new(),
            raw_ml: Vec::new(),
            kf8_sections: Vec::new(),
            cover_offset: None,
            flow_table: Vec::new(),
            files: Vec::new(),
            elems: Vec::new(),
            guide: Vec::new(),
            flows: Vec::new(),
            flowinfo: Vec::new(),
            parts: Vec::new(),
            partinfo: Vec::new(),
        }
    }

    /// Port of `Mobi8Reader.__call__`: runs the full KF8 extraction
    /// pipeline and writes `metadata.opf`/`toc.ncx`/`text/*`/`images/*`/
    /// `fonts/*` under the current directory, matching Python (which also
    /// writes relative to `os.getcwd()`). Callers should `set_current_dir`
    /// to `output_dir` first (mirroring how `calibre.ebooks.conversion`
    /// invokes the equivalent Python code from a temp working directory).
    pub fn run(&mut self, output_dir: &Path) -> Result<String> {
        self.mobi6_reader.check_for_drm()?;
        let bh = self.mobi6_reader.book_header.clone();

        let offset;
        if self.mobi6_reader.kf8_type.as_deref() == Some("joint") {
            let boundary = self
                .mobi6_reader
                .kf8_boundary
                .context("joint KF8 file missing boundary")?;
            offset = boundary + 2;
            let mobi6_records = bh.mobi6_records.unwrap_or(bh.records) as u32;
            let kf8_first_image_index = bh.kf8_first_image_index.unwrap_or(bh.first_image_index);
            self.resource_offsets = vec![
                (
                    get_first_resource_index(bh.first_image_index, mobi6_records, 1),
                    offset - 2,
                ),
                (
                    get_first_resource_index(kf8_first_image_index, bh.records as u32, offset),
                    self.mobi6_reader.sections.len(),
                ),
            ];
        } else {
            offset = 1;
            self.resource_offsets = vec![(
                get_first_resource_index(bh.first_image_index, bh.records as u32, offset),
                self.mobi6_reader.sections.len(),
            )];
        }

        self.processed_records = self.mobi6_reader.extract_text(offset)?;
        self.raw_ml = self.mobi6_reader.mobi_html.clone();
        self.kf8_sections = self.mobi6_reader.sections[(offset - 1)..].to_vec();
        self.cover_offset = self
            .mobi6_reader
            .book_header
            .exth
            .as_ref()
            .and_then(|e| e.cover_offset);

        self.read_indices()?;
        self.build_parts()?;
        let guide = self.create_guide()?;
        let ncx = self.create_ncx()?;
        let resource_map = self.extract_resources(output_dir)?;
        let spine = self.expand_text(output_dir, &resource_map)?;
        self.write_opf(output_dir, guide, ncx, spine, resource_map)
    }

    /// Port of `read_indices`.
    fn read_indices(&mut self) -> Result<()> {
        let header = self.mobi6_reader.book_header.clone();
        let codec = header.codec.clone();

        self.flow_table.clear();
        if header.fdstidx != NULL_INDEX {
            let hdr = self
                .kf8_sections
                .get(header.fdstidx as usize)
                .context("KF8 FDST index out of range")?;
            if hdr.len() < 4 || &hdr[..4] != b"FDST" {
                bail!("KF8 does not have a valid FDST record");
            }
            let sec_start = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]) as usize;
            let num_sections = u32::from_be_bytes([hdr[8], hdr[9], hdr[10], hdr[11]]) as usize;
            for i in 0..num_sections {
                let off = sec_start + i * 8;
                if off + 8 > hdr.len() {
                    break;
                }
                let a = u32::from_be_bytes([hdr[off], hdr[off + 1], hdr[off + 2], hdr[off + 3]]);
                let b =
                    u32::from_be_bytes([hdr[off + 4], hdr[off + 5], hdr[off + 6], hdr[off + 7]]);
                self.flow_table.push((a, b));
            }
        }

        self.files.clear();
        if header.skelidx != NULL_INDEX {
            let (table, _cncx) = read_index(&self.kf8_sections, header.skelidx as usize, &codec)?;
            for (i, (text, tag_map)) in table.iter().enumerate() {
                let divtbl_count = tag_map
                    .get(&1)
                    .and_then(|v| v.first())
                    .copied()
                    .unwrap_or(0);
                let start_position = tag_map
                    .get(&6)
                    .and_then(|v| v.first())
                    .copied()
                    .unwrap_or(0);
                let length = tag_map.get(&6).and_then(|v| v.get(1)).copied().unwrap_or(0);
                self.files.push(SkelFile {
                    file_number: i,
                    name: text.clone(),
                    divtbl_count,
                    start_position,
                    length,
                });
            }
        }

        self.elems.clear();
        if header.dividx != NULL_INDEX {
            let (table, cncx) = read_index(&self.kf8_sections, header.dividx as usize, &codec)?;
            for (text, tag_map) in table.iter() {
                let toc_text_off = tag_map
                    .get(&2)
                    .and_then(|v| v.first())
                    .copied()
                    .unwrap_or(0) as usize;
                let toc_text = cncx.get(toc_text_off).cloned().unwrap_or_default();
                let insert_pos: i64 = text.parse().unwrap_or(0);
                self.elems.push(Elem {
                    insert_pos,
                    toc_text,
                    file_number: tag_map
                        .get(&3)
                        .and_then(|v| v.first())
                        .copied()
                        .unwrap_or(0),
                    sequence_number: tag_map
                        .get(&4)
                        .and_then(|v| v.first())
                        .copied()
                        .unwrap_or(0),
                    start_pos: tag_map
                        .get(&6)
                        .and_then(|v| v.first())
                        .copied()
                        .unwrap_or(0),
                    length: tag_map.get(&6).and_then(|v| v.get(1)).copied().unwrap_or(0),
                });
            }
        }

        self.guide.clear();
        if header.othidx != NULL_INDEX {
            let (table, cncx) = read_index(&self.kf8_sections, header.othidx as usize, &codec)?;
            for (ref_type, tag_map) in table.iter() {
                let title_off = tag_map
                    .get(&1)
                    .and_then(|v| v.first())
                    .copied()
                    .unwrap_or(0) as usize;
                let title = cncx.get(title_off).cloned().unwrap_or_default();
                let pos_fid = if let Some(v) = tag_map.get(&3) {
                    v.first().map(|&fid| (fid as u32, 0))
                } else {
                    tag_map.get(&6).and_then(|v| {
                        if v.len() >= 2 {
                            Some((v[0] as u32, v[1] as u32))
                        } else {
                            None
                        }
                    })
                };
                self.guide.push(GuideItem {
                    type_: ref_type.clone(),
                    title,
                    pos_fid,
                });
            }
        }

        Ok(())
    }

    /// Port of `build_parts`.
    fn build_parts(&mut self) -> Result<()> {
        let raw_ml = self.mobi6_reader.mobi_html.clone();
        let mut flows: Vec<Vec<u8>> = Vec::new();
        let ft: Vec<(u32, u32)> = if self.flow_table.is_empty() {
            vec![(0, raw_ml.len() as u32)]
        } else {
            self.flow_table.clone()
        };
        for (start, end) in &ft {
            let start = *start as usize;
            let end = (*end as usize).min(raw_ml.len());
            flows.push(if start <= end {
                raw_ml[start..end].to_vec()
            } else {
                Vec::new()
            });
        }

        let text = flows[0].clone();
        flows[0] = Vec::new();

        let mut parts = Vec::new();
        let mut partinfo = Vec::new();
        let mut divptr = 0usize;

        for skel in &self.files {
            let skelpos = skel.start_position as usize;
            let skellen = skel.length as usize;
            let mut baseptr = skelpos + skellen;
            let mut skeleton = text.get(skelpos..baseptr).unwrap_or_default().to_vec();
            let mut inspos_warned = false;
            let mut aidtext = Vec::new();
            let mut filename = String::new();

            for i in 0..skel.divtbl_count as usize {
                let Some(elem) = self.elems.get(divptr) else {
                    break;
                };
                if i == 0 {
                    // Python: `idtext[12:-2]` -- the div-table identifier
                    // is formatted `kindle:embed:XXXX` or similar; strip
                    // the surrounding wrapper to get the bare aid.
                    let idtext = elem.toc_text.as_bytes();
                    aidtext = if idtext.len() > 14 {
                        idtext[12..idtext.len() - 2].to_vec()
                    } else {
                        idtext.to_vec()
                    };
                    filename = format!("part{:04}.html", elem.file_number);
                }
                let part_start = baseptr;
                let part_end = (baseptr + elem.length as usize).min(text.len());
                let part = text.get(part_start..part_end).unwrap_or_default().to_vec();
                let mut insertpos = elem.insert_pos - skelpos as i64;
                if insertpos < 0 || insertpos as usize > skeleton.len() {
                    insertpos = insertpos.clamp(0, skeleton.len() as i64);
                }
                let insertpos_usize = insertpos as usize;
                let head = &skeleton[..insertpos_usize.min(skeleton.len())];
                let tail = &skeleton[insertpos_usize.min(skeleton.len())..];

                let tail_gt = tail.iter().position(|&b| b == b'>');
                let tail_lt = tail.iter().position(|&b| b == b'<');
                let head_gt = head.iter().rposition(|&b| b == b'>');
                let head_lt = head.iter().rposition(|&b| b == b'<');
                let malformed = match (tail_gt, tail_lt) {
                    (Some(g), Some(l)) => g < l,
                    (None, Some(_)) => true,
                    _ => match (head_gt, head_lt) {
                        (Some(g), Some(l)) => g < l,
                        _ => false,
                    },
                };
                if malformed {
                    if !inspos_warned {
                        self.log.warn(format!(
                            "The div table for {} has incorrect insert positions. Calculating manually.",
                            skel.name
                        ));
                        inspos_warned = true;
                    }
                    let (bp, ep) = locate_beg_end_of_tag(&skeleton, &aidtext);
                    if bp != ep {
                        insertpos = ep as i64 + 1 + elem.start_pos as i64;
                    }
                }

                let ip = (insertpos as usize).min(skeleton.len());
                let mut new_skel = skeleton[..ip].to_vec();
                new_skel.extend_from_slice(&part);
                new_skel.extend_from_slice(&skeleton[ip..]);
                skeleton = new_skel;
                baseptr += elem.length as usize;
                divptr += 1;
            }

            parts.push(skeleton);
            if skel.divtbl_count < 1 {
                let uuid = uuid_hex();
                filename = format!("{uuid}.html");
            }
            partinfo.push(Part {
                num: skel.file_number,
                type_: "text".to_string(),
                filename,
                start: skelpos as u64,
                end: baseptr as u64,
                aid: String::from_utf8_lossy(&aidtext).into_owned(),
            });
        }

        self.parts = parts;
        self.partinfo = partinfo;

        let mut flowinfo = vec![None];
        let svg_tag_re = BytesRegex::new(r"(?i)<svg[^>]*>").unwrap();
        let image_tag_re = BytesRegex::new(r"(?i)<(?:svg:)?image[^>]*>").unwrap();
        for (j, flowpart) in flows.iter_mut().enumerate().skip(1) {
            let nstr = format!("{j:04}");
            if let Some(m) = svg_tag_re.find(flowpart) {
                let start = m.start();
                let from_svg = flowpart[start..].to_vec();
                if image_tag_re.is_match(&from_svg) {
                    *flowpart = from_svg;
                    flowinfo.push(Some((
                        "svg".to_string(),
                        "inline".to_string(),
                        String::new(),
                        String::new(),
                    )));
                } else {
                    flowinfo.push(Some((
                        "svg".to_string(),
                        "file".to_string(),
                        "images".to_string(),
                        format!("svgimg{nstr}.svg"),
                    )));
                }
            } else if flowpart.windows(7).any(|w| w == b"[CDATA[") {
                let mut wrapped = b"<style type=\"text/css\">\n".to_vec();
                wrapped.extend_from_slice(flowpart);
                wrapped.extend_from_slice(b"\n</style>\n");
                *flowpart = wrapped;
                flowinfo.push(Some((
                    "css".to_string(),
                    "inline".to_string(),
                    String::new(),
                    String::new(),
                )));
            } else {
                flowinfo.push(Some((
                    "css".to_string(),
                    "file".to_string(),
                    "styles".to_string(),
                    format!("{nstr}.css"),
                )));
            }
        }

        self.flows = flows;
        self.flowinfo = flowinfo;
        Ok(())
    }

    fn get_file_info(&self, pos: i64) -> Option<&Part> {
        self.partinfo
            .iter()
            .find(|p| pos >= p.start as i64 && pos < p.end as i64)
    }

    fn get_id_tag(&self, pos: i64) -> Vec<u8> {
        let Some(fi) = self.get_file_info(pos).cloned() else {
            return Vec::new();
        };
        let Some(textblock) = self.parts.get(fi.num) else {
            return Vec::new();
        };
        let mut npos = (pos - fi.start as i64).max(0) as usize;
        let pgt = textblock[npos.min(textblock.len())..]
            .iter()
            .position(|&b| b == b'>')
            .map(|p| p + npos);
        let plt = textblock[npos.min(textblock.len())..]
            .iter()
            .position(|&b| b == b'<')
            .map(|p| p + npos);
        if plt == Some(npos) || pgt.unwrap_or(usize::MAX) < plt.unwrap_or(usize::MAX) {
            npos = pgt.map(|p| p + 1).unwrap_or(npos);
        }
        let textblock = &textblock[..npos.min(textblock.len())];
        for tag in reverse_tag_iter(textblock) {
            if let Some(caps) = ID_RE.captures(&tag).or_else(|| NAME_RE.captures(&tag)) {
                if let Some(m) = caps.get(1) {
                    return m.as_bytes().to_vec();
                }
            }
            if let Some(caps) = AID_RE.captures(&tag) {
                if let Some(m) = caps.get(1) {
                    let aid = String::from_utf8_lossy(m.as_bytes()).into_owned();
                    self.linked_aids.borrow_mut().insert(aid.clone());
                    let mut out = m.as_bytes().to_vec();
                    out.push(b'-');
                    out.extend_from_slice(self.aid_anchor_suffix.as_bytes());
                    return out;
                }
            }
        }
        Vec::new()
    }

    fn get_id_tag_by_pos_fid_impl(&self, posfid: u32, offset: u32) -> Option<(String, String)> {
        let elem = self.elems.get(posfid as usize)?.clone();
        let pos = elem.insert_pos + offset as i64;
        let fi = self.get_file_info(pos)?.clone();
        let idtext = self.get_id_tag(pos);
        Some((
            format!("{}/{}", fi.type_, fi.filename),
            String::from_utf8_lossy(&idtext).into_owned(),
        ))
    }

    /// Port of `create_guide`.
    fn create_guide(&mut self) -> Result<Vec<GuideRef>> {
        let mut guide = Vec::new();
        let mut has_start = false;
        let codec = self.mobi6_reader.book_header.codec.clone();

        for item in self.guide.clone() {
            let Some((posfid, off)) = item.pos_fid else {
                continue;
            };
            let Some((linktgt, idtext)) = self.get_id_tag_by_pos_fid_impl(posfid, off) else {
                continue;
            };
            let mut linktgt = linktgt;
            if !idtext.is_empty() {
                linktgt = format!("{linktgt}#{idtext}");
            }
            if item.title == "start" || item.type_ == "text" {
                has_start = true;
            }
            guide.push(GuideRef {
                type_: item.type_,
                title: item.title,
                href: linktgt,
            });
        }

        let so = self
            .mobi6_reader
            .book_header
            .exth
            .as_ref()
            .and_then(|e| e.start_offset);
        if let Some(so) = so {
            if so != NULL_INDEX && !has_start {
                if let Some(fi) = self.get_file_info(so as i64).cloned() {
                    let idtext = decode_with(&self.get_id_tag(so as i64), &codec);
                    let mut linktgt = format!("{}/{}", fi.type_, fi.filename);
                    if !idtext.is_empty() {
                        linktgt = format!("{linktgt}#{idtext}");
                    }
                    guide.push(GuideRef {
                        type_: "text".to_string(),
                        title: "start".to_string(),
                        href: linktgt,
                    });
                }
            }
        }

        Ok(guide)
    }

    /// Port of `create_ncx`.
    fn create_ncx(&mut self) -> Result<TOC> {
        let codec = self.mobi6_reader.book_header.codec.clone();
        let mut entries = read_ncx(
            &self.kf8_sections,
            self.mobi6_reader.book_header.ncxidx,
            &codec,
        )?;
        let mut remove = Vec::new();

        for (i, entry) in entries.iter_mut().enumerate() {
            let (href, idtag) = if let Some((posfid, off)) = entry.pos_fid {
                match self.get_id_tag_by_pos_fid_impl(posfid as u32, off as u32) {
                    Some(v) => v,
                    None => {
                        self.log.warn(format!(
                            "Invalid entry in NCX (title: {}), ignoring",
                            entry.text
                        ));
                        remove.push(i);
                        continue;
                    }
                }
            } else {
                let Some(fi) = self.get_file_info(entry.pos).cloned() else {
                    bail!("Index entry has invalid pos: {}", entry.pos);
                };
                let idtag = decode_with(&self.get_id_tag(entry.pos), &codec);
                (format!("{}/{}", fi.type_, fi.filename), idtag)
            };
            entry.href = Some(href);
            entry.idtag = Some(idtag);
        }

        for i in remove.into_iter().rev() {
            entries.remove(i);
        }

        Ok(build_toc(entries))
    }

    /// Port of `extract_resources`.
    fn extract_resources(&mut self, output_dir: &Path) -> Result<Vec<Option<String>>> {
        const PLACEHOLDER_GIF_LEN: usize = 58;
        let mut resource_map = Vec::new();
        let mut container: Option<Container> = None;
        std::fs::create_dir_all(output_dir.join("fonts"))?;
        std::fs::create_dir_all(output_dir.join("images"))?;

        let sections = self.mobi6_reader.sections.clone();
        for &(start, end) in &self.resource_offsets.clone() {
            let end = end.min(sections.len());
            if start >= end {
                continue;
            }
            for (i, sec) in sections[start..end].iter().enumerate() {
                let fname_idx = i + 1;
                let data = sec;
                let typ = if data.len() >= 4 {
                    &data[..4]
                } else {
                    &data[..]
                };
                let mut href: Option<String> = None;

                if matches!(
                    typ,
                    b"FLIS"
                        | b"FCIS"
                        | b"SRCS"
                        | b"\xe9\x8e\r\n"
                        | b"BOUN"
                        | b"FDST"
                        | b"DATP"
                        | b"AUDI"
                        | b"VIDE"
                        | b"RESC"
                        | b"CMET"
                        | b"PAGE"
                ) {
                    // Ignore these records.
                } else if typ == b"FONT" {
                    let font = read_font_record(data, DEFAULT_FONT_XOR_EXTENT);
                    let ext = font.ext;
                    let h = format!("fonts/{fname_idx:05}.{ext}");
                    if let Some(err) = &font.err {
                        self.log
                            .warn(format!("Reading font record {fname_idx} failed: {err}"));
                    }
                    let bytes = font
                        .font_data
                        .clone()
                        .unwrap_or_else(|| font.raw_data.clone());
                    std::fs::write(output_dir.join(&h), &bytes)?;
                    if font.encrypted {
                        self.encrypted_fonts.push(h.clone());
                    }
                    href = Some(h);
                } else if typ == b"CONT" {
                    if data.as_slice() == b"CONTBOUNDARY" {
                        container = None;
                    } else {
                        container = Some(Container::new(data));
                    }
                } else if typ == b"CRES" {
                    if let Some(c) = container.as_mut() {
                        let (img, imgtype) = c.load_image(data);
                        if let (Some(img), Some(imgtype)) = (img, imgtype) {
                            let h = format!("images/{:05}.{}", c.resource_index, imgtype);
                            std::fs::write(output_dir.join(&h), img)?;
                            href = Some(h);
                        }
                    }
                } else if data.len() == 4
                    && data.as_slice() == b"\xa0\xa0\xa0\xa0"
                    && container.is_some()
                {
                    if let Some(c) = container.as_mut() {
                        c.resource_index += 1;
                    }
                } else if container.is_none()
                    && !(data.len() == PLACEHOLDER_GIF_LEN && is_placeholder_gif(data))
                {
                    let imgtype = find_imgtype(data);
                    let h = format!("images/{fname_idx:05}.{imgtype}");
                    std::fs::write(output_dir.join(&h), data)?;
                    href = Some(h);
                }

                resource_map.push(href);
            }
        }

        Ok(resource_map)
    }

    /// Port of `expand_text` + the file-writing tail of
    /// `markup.py`'s `expand_mobi8_markup` (the already-ported
    /// [`crate::mobi::markup::expand_mobi8_markup`] only performs the
    /// in-memory text transforms; writing `text/*.html` and the "file"
    /// format flow pieces to disk is reader-specific I/O that stays here).
    fn expand_text(
        &mut self,
        output_dir: &Path,
        resource_map: &[Option<String>],
    ) -> Result<Vec<String>> {
        let codec = self.mobi6_reader.book_header.codec.clone();

        let mut parts_str: Vec<String> =
            self.parts.iter().map(|p| decode_with(p, &codec)).collect();
        let mut flows_str: Vec<Option<String>> = self
            .flowinfo
            .iter()
            .enumerate()
            .map(|(i, fi)| {
                if fi.is_none() {
                    None
                } else {
                    Some(decode_with(&self.flows[i], &codec))
                }
            })
            .collect();

        let flowinfo_markup: Vec<Option<MarkupFlowInfo>> = self
            .flowinfo
            .iter()
            .map(|fi| {
                fi.as_ref()
                    .map(|(_type, format, dir, fname)| MarkupFlowInfo {
                        dir: dir.clone(),
                        fname: fname.clone(),
                        format: format.clone(),
                    })
            })
            .collect();

        let log_fn = |msg: &str| eprintln!("WARNING: {msg}");

        {
            let mut wrapper = TraitWrapper {
                reader: &*self,
                flowinfo_markup: &flowinfo_markup,
            };
            expand_mobi8_markup(
                &mut parts_str,
                &mut flows_str,
                &mut wrapper,
                resource_map,
                &log_fn,
            );
        }

        std::fs::create_dir_all(output_dir.join("text"))?;
        let mut spine = Vec::new();
        for (i, part) in parts_str.iter().enumerate() {
            let pi = &self.partinfo[i];
            let mut out = crate::chardet::strip_encoding_declarations(
                part.as_bytes(),
                part.len().max(1),
                true,
            );
            let out_str = String::from_utf8_lossy(&out).into_owned();
            let out_str = out_str.replacen("<head>", "<head><meta charset=\"UTF-8\"/>", 1);
            out = out_str.into_bytes();
            let rel = format!("{}/{}", pi.type_, pi.filename);
            std::fs::write(output_dir.join(&rel), &out)?;
            spine.push(rel);
        }

        for (i, flow) in flows_str.iter().enumerate() {
            let Some(flow) = flow else { continue };
            let Some(Some((_type, format, dir, fname))) = self.flowinfo.get(i) else {
                continue;
            };
            if format == "file" {
                std::fs::create_dir_all(output_dir.join(dir))?;
                std::fs::write(output_dir.join(dir).join(fname), flow.as_bytes())?;
            }
        }

        self.parts = parts_str.into_iter().map(|s| s.into_bytes()).collect();
        self.flows = flows_str
            .into_iter()
            .map(|s| s.unwrap_or_default().into_bytes())
            .collect();

        Ok(spine)
    }

    /// Port of `write_opf`.
    fn write_opf(
        &mut self,
        output_dir: &Path,
        guide: Vec<GuideRef>,
        toc: TOC,
        spine: Vec<String>,
        resource_map: Vec<Option<String>>,
    ) -> Result<String> {
        let mut mi = self
            .mobi6_reader
            .book_header
            .exth
            .as_ref()
            .map(|e| e.mi.clone())
            .unwrap_or_else(|| {
                crate::metadata::MetaInformation::new("Unknown", vec!["Unknown".to_string()])
            });

        if let Some(cover_offset) = self.cover_offset {
            if let Some(Some(href)) = resource_map.get(cover_offset as usize) {
                mi.cover_id = Some(href.clone());
            }
        }

        let mut toc = toc;
        if toc.nodes.len() < 2 {
            self.log.warn("KF8 has no metadata Table of Contents");
            for g in &guide {
                if g.type_ == "toc" {
                    let (href, frag) = split_href(&g.href);
                    if output_dir.join(&href).exists() {
                        if let Ok(t) = self.read_inline_toc(output_dir, &href, &frag) {
                            toc = t;
                        }
                    }
                }
            }
        }

        // `create_manifest_from_files_in`: walk the output directory and
        // register every written file.
        let manifest_pairs = crate::opf_writer::scan_directory_manifest(
            output_dir,
            &["metadata.opf", "toc.ncx", "debug-raw.html"],
        );
        let manifest = opf_writer::auto_manifest(&manifest_pairs);
        let spine_ids: Vec<String> = spine
            .iter()
            .filter_map(|href| {
                manifest
                    .iter()
                    .find(|m| m.href == *href)
                    .map(|m| m.id.clone())
            })
            .collect();

        let ncx_manifest_id = if toc.nodes.is_empty() {
            None
        } else {
            Some("ncx")
        };
        let ppd = self
            .mobi6_reader
            .book_header
            .exth
            .as_ref()
            .and_then(|e| e.page_progression_direction.clone());
        let ppd = ppd.filter(|p| matches!(p.as_str(), "ltr" | "rtl" | "default"));
        let pwm = self
            .mobi6_reader
            .book_header
            .exth
            .as_ref()
            .and_then(|e| e.primary_writing_mode.clone());

        let opf_xml = opf_writer::write_opf(
            &mi,
            &manifest,
            &spine_ids,
            &guide,
            ncx_manifest_id,
            None,
            ppd.as_deref(),
            pwm.as_deref(),
        );
        std::fs::write(output_dir.join("metadata.opf"), &opf_xml)?;
        if !toc.nodes.is_empty() {
            let ncx_xml =
                opf_writer::write_ncx(&toc, mi.uuid.as_deref().unwrap_or("unknown"), &mi.title);
            std::fs::write(output_dir.join("toc.ncx"), ncx_xml)?;
        }

        Ok("metadata.opf".to_string())
    }

    /// Port of `read_inline_toc`: builds a fallback TOC from the `<a>`
    /// links in a chapter file's body when KF8's own metadata NCX is
    /// missing/too shallow, using DOM nesting depth to infer hierarchy
    /// (Python does the same via `node_depth`/`getparent()` walks).
    fn read_inline_toc(&mut self, output_dir: &Path, href: &str, frag: &str) -> Result<TOC> {
        let raw = std::fs::read(output_dir.join(href))?;
        let codec = self.mobi6_reader.book_header.codec.clone();
        let html = decode_with(&raw, &codec);
        let dom = Dom::parse(&html);

        let base_href = href
            .rsplit_once('/')
            .map(|(b, _)| b.to_string())
            .unwrap_or_default();
        let start = if !frag.is_empty() {
            dom.find_by_id(frag)
        } else {
            dom.find_first_tag_global("body")
        };

        let mut reached = start.is_none();
        let mut seen = HashSet::new();
        let mut links: Vec<(String, String, String, usize)> = Vec::new();

        for el in dom.preorder_elements(dom.root) {
            if Some(el) == start {
                reached = true;
                continue;
            }
            if reached && dom.tag(el) == Some("a") {
                if let Some(href_attr) = dom.node(el).attrs.get("href") {
                    if !href_attr.is_empty() {
                        let (h, f) = split_href(href_attr);
                        let full_href = format!("{base_href}/{h}");
                        let text = dom.text_content(el).trim().to_string();
                        let key = (text.clone(), full_href.clone(), f.clone());
                        if seen.insert(key) {
                            let depth = node_depth(&dom, el);
                            links.push((text, full_href, f, depth));
                        }
                    }
                }
            }
        }

        let mut depths: Vec<usize> = links.iter().map(|l| l.3).collect();
        depths.sort_unstable();
        depths.dedup();
        let depth_map: HashMap<usize, usize> =
            depths.iter().enumerate().map(|(i, &d)| (d, i)).collect();

        let mut toc = TOC::new();
        // Path of indices into `toc.nodes` locating the most recently
        // added node, so subsequent entries can be nested under it.
        let mut path: Vec<usize> = Vec::new();
        let mut current_depth: Option<usize> = None;

        fn children_at<'a>(root: &'a mut Vec<TOCNode>, path: &[usize]) -> &'a mut Vec<TOCNode> {
            let mut cur = root;
            for &idx in path {
                cur = &mut cur[idx].children;
            }
            cur
        }

        for (text, href, frag, depth) in links {
            let depth = *depth_map.get(&depth).unwrap_or(&0);
            let node = TOCNode {
                title: text,
                src: join_href(&href, &frag),
                children: Vec::new(),
            };
            match current_depth {
                None => {
                    toc.nodes.push(node);
                    path = vec![toc.nodes.len() - 1];
                    current_depth = Some(0);
                }
                Some(cur) if cur == depth => {
                    let parent_path = &path[..path.len().saturating_sub(1)];
                    let siblings = children_at(&mut toc.nodes, parent_path);
                    siblings.push(node);
                    let mut np = parent_path.to_vec();
                    np.push(siblings.len() - 1);
                    path = np;
                }
                Some(cur) if cur < depth => {
                    let siblings = children_at(&mut toc.nodes, &path);
                    siblings.push(node);
                    path.push(siblings.len() - 1);
                    current_depth = Some(cur + 1);
                }
                Some(cur) => {
                    let mut delta = cur - depth;
                    let mut p = path.clone();
                    while delta > 0 && !p.is_empty() {
                        p.pop();
                        delta -= 1;
                    }
                    let parent_path = &p[..p.len().saturating_sub(1)];
                    let siblings = children_at(&mut toc.nodes, parent_path);
                    siblings.push(node);
                    let mut np = parent_path.to_vec();
                    np.push(siblings.len() - 1);
                    path = np;
                    current_depth = Some(depth);
                }
            }
        }

        Ok(toc)
    }
}

fn node_depth(dom: &Dom, el: crate::dom::NodeId) -> usize {
    let mut depth = 0;
    let mut parent = dom.parent(el);
    while let Some(p) = parent {
        depth += 1;
        parent = dom.parent(p);
    }
    depth
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

fn is_placeholder_gif(data: &[u8]) -> bool {
    data.len() > 6 && &data[..6] == b"GIF89a"
}

fn decode_with(data: &[u8], codec: &str) -> String {
    if codec.eq_ignore_ascii_case("utf-8") {
        String::from_utf8_lossy(data).into_owned()
    } else {
        match encoding_rs::Encoding::for_label(codec.as_bytes()) {
            Some(enc) => enc.decode(data).0.into_owned(),
            None => String::from_utf8_lossy(data).into_owned(),
        }
    }
}

fn uuid_hex() -> String {
    // A dependency-free UUIDv4-shaped hex string (32 lowercase hex
    // chars), matching Python's `uuid4().hex`. Cryptographic-grade
    // randomness isn't required here -- it's only used as an anchor-id
    // disambiguation suffix, never a security boundary.
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut state = seed as u64 ^ 0x9E3779B97F4A7C15;
    let mut out = String::with_capacity(32);
    for _ in 0..32 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push(std::char::from_digit((state & 0xF) as u32, 16).unwrap_or('0'));
    }
    out
}

/// Adapts [`Mobi8Reader`] to [`MobiReaderTrait`] for
/// [`crate::mobi::markup::expand_mobi8_markup`]. A separate wrapper
/// (rather than implementing the trait on `Mobi8Reader` directly) because
/// the trait's `get_flow_info` needs data in `crate::mobi::markup`'s
/// `FlowInfo` shape, which `Mobi8Reader` doesn't store directly (it keeps
/// the richer `(type, format, dir, fname)` tuple `mobi8.py` does).
struct TraitWrapper<'a> {
    reader: &'a Mobi8Reader,
    flowinfo_markup: &'a [Option<MarkupFlowInfo>],
}

impl MobiReaderTrait for TraitWrapper<'_> {
    fn get_id_tag_by_pos_fid(&self, pos: u32, off: u32) -> Option<(String, String)> {
        self.reader.get_id_tag_by_pos_fid_impl(pos, off)
    }

    fn get_flow_info(&self, num: usize) -> Option<&MarkupFlowInfo> {
        self.flowinfo_markup.get(num).and_then(|fi| fi.as_ref())
    }

    fn get_flow(&self, num: usize) -> Option<&String> {
        // Flows are decoded to `String` only inside `expand_text` (which
        // owns a separate `Vec<Option<String>>`); this trait method isn't
        // exercised by `expand_mobi8_markup`'s current call pattern
        // (`get_flow` is used by `update_flow_links`'s "inline" case,
        // which reads from the `flows` argument passed in directly, not
        // via the reader), so returning `None` here is safe.
        let _ = num;
        None
    }

    fn get_header_codec(&self) -> &str {
        &self.reader.mobi6_reader.book_header.codec
    }

    fn get_aid_anchor_suffix(&self) -> Option<&str> {
        Some(&self.reader.aid_anchor_suffix)
    }

    fn is_aid_linked(&self, aid: &str) -> bool {
        self.reader.linked_aids.borrow().contains(aid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use byteorder::WriteBytesExt;

    #[test]
    fn test_get_first_resource_index() {
        // Explicit first_image_index wins.
        assert_eq!(get_first_resource_index(5, 100, 1), 5);
        // NULL_INDEX falls back to num_of_text_records + first_text_record_number.
        assert_eq!(get_first_resource_index(NULL_INDEX, 40, 1), 41);
    }

    #[test]
    fn test_reverse_tag_iter_order() {
        let block = b"<a>text<b>more</b></a>";
        let tags: Vec<String> = reverse_tag_iter(block)
            .into_iter()
            .map(|t| String::from_utf8(t).unwrap())
            .collect();
        // Should yield tags from the end of the block backwards.
        assert_eq!(tags, vec!["</a>", "</b>", "<b>", "<a>"]);
    }

    #[test]
    fn test_locate_beg_end_of_tag_finds_aid() {
        let ml = br#"<div>before</div><p aid="xyz123">content</p><div>after</div>"#;
        let (bp, ep) = locate_beg_end_of_tag(ml, b"xyz123");
        assert!(bp < ep);
        assert_eq!(&ml[bp..=ep], &br#"<p aid="xyz123">"#[..]);
    }

    #[test]
    fn test_locate_beg_end_of_tag_missing() {
        let ml = b"<div>no match here</div>";
        assert_eq!(locate_beg_end_of_tag(ml, b"nope"), (0, 0));
    }

    #[test]
    fn test_split_join_href() {
        assert_eq!(
            split_href("text/part0001.html#anchor1"),
            ("text/part0001.html".to_string(), "anchor1".to_string())
        );
        assert_eq!(
            split_href("text/part0001.html"),
            ("text/part0001.html".to_string(), String::new())
        );
        assert_eq!(join_href("a.html", "frag"), "a.html#frag");
        assert_eq!(join_href("a.html", ""), "a.html");
    }

    #[test]
    fn test_is_placeholder_gif() {
        assert!(is_placeholder_gif(b"GIF89a\x01\x00\x01\x00"));
        assert!(!is_placeholder_gif(b"GIF87a\x01\x00\x01\x00"));
        assert!(!is_placeholder_gif(b"notagif"));
    }

    #[test]
    fn test_uuid_hex_looks_like_uuid() {
        let h = uuid_hex();
        assert_eq!(h.len(), 32);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    /// Builds a minimal valid MOBI6 PDB (mobi_version=8, single empty
    /// text record) -- just enough for `mobi6::MobiReader::new` to
    /// succeed, so `Mobi8Reader` tests below have something to wrap.
    /// `read_indices`/`build_parts` are exercised separately at the
    /// `mobi8_test.rs` integration level with a real skeleton/div INDX
    /// table; this fixture intentionally has none (`dividx`/`skelidx` end
    /// up `NULL_INDEX`), so it's only useful for testing pieces of
    /// `Mobi8Reader` that don't depend on those tables, like
    /// `extract_resources`.
    fn minimal_kf8_pdb() -> Vec<u8> {
        let mut rec0 = Vec::new();
        rec0.write_u16::<byteorder::BigEndian>(1).unwrap(); // compression = none
        rec0.write_u16::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u16::<byteorder::BigEndian>(1).unwrap(); // record_count
        rec0.write_u16::<byteorder::BigEndian>(4096).unwrap();
        rec0.write_u16::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u16::<byteorder::BigEndian>(0).unwrap();
        rec0.extend_from_slice(b"MOBI");
        rec0.write_u32::<byteorder::BigEndian>(232).unwrap(); // header_length
        rec0.write_u32::<byteorder::BigEndian>(2).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(65001).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(6).unwrap();
        for _ in 0..10 {
            rec0.write_u32::<byteorder::BigEndian>(NULL_INDEX).unwrap();
        }
        rec0.write_u32::<byteorder::BigEndian>(NULL_INDEX).unwrap(); // first_non_book_index
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap(); // full_name_offset
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap(); // full_name_length
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        rec0.write_u32::<byteorder::BigEndian>(8).unwrap(); // min_version -> mobi_version 8
        rec0.write_u32::<byteorder::BigEndian>(NULL_INDEX).unwrap(); // first_image_index
        for _ in 0..4 {
            rec0.write_u32::<byteorder::BigEndian>(0).unwrap();
        }
        rec0.write_u32::<byteorder::BigEndian>(0).unwrap(); // exth_flags
        while rec0.len() < 16 + 232 {
            rec0.push(0);
        }

        let mut records = vec![rec0, Vec::new()];
        let mut out = Vec::new();
        let mut name = [0u8; 32];
        name[..4].copy_from_slice(b"Test");
        out.extend_from_slice(&name);
        for _ in 0..2 {
            out.write_u16::<byteorder::BigEndian>(0).unwrap();
        }
        for _ in 0..6 {
            out.write_u32::<byteorder::BigEndian>(0).unwrap();
        }
        out.extend_from_slice(b"BOOK");
        out.extend_from_slice(b"MOBI");
        out.write_u32::<byteorder::BigEndian>(0).unwrap();
        out.write_u32::<byteorder::BigEndian>(0).unwrap();
        out.write_u16::<byteorder::BigEndian>(records.len() as u16)
            .unwrap();
        let header_and_list_len = 78 + records.len() * 8;
        let mut offset = header_and_list_len as u32;
        let mut offsets = Vec::new();
        for r in &records {
            offsets.push(offset);
            offset += r.len() as u32;
        }
        for off in &offsets {
            out.write_u32::<byteorder::BigEndian>(*off).unwrap();
            out.extend_from_slice(&[0u8; 4]);
        }
        for r in records.drain(..) {
            out.extend_from_slice(&r);
        }
        out
    }

    #[test]
    fn test_extract_resources_font_and_plain_image() {
        let pdb = minimal_kf8_pdb();
        let mobi6_reader = MobiReader::new(&pdb).expect("minimal KF8 PDB should parse");
        assert_eq!(mobi6_reader.book_header.mobi_version, 8);

        let mut m8 = Mobi8Reader::new(mobi6_reader, MobiLog::default(), false);

        // A plain (non-container) PNG record and a FONT record.
        let png = b"\x89PNG\r\n\x1a\nrestofpngdata".to_vec();
        let mut font = Vec::new();
        font.extend_from_slice(b"FONT");
        font.write_u32::<byteorder::BigEndian>(11).unwrap(); // usize
        font.write_u32::<byteorder::BigEndian>(0).unwrap(); // flags: no compression, no obfuscation
        font.write_u32::<byteorder::BigEndian>(24).unwrap(); // dstart
        font.write_u32::<byteorder::BigEndian>(0).unwrap(); // xor_len
        font.write_u32::<byteorder::BigEndian>(0).unwrap(); // xor_start
        font.extend_from_slice(b"OTTOfontbyte"); // 12 bytes of "font data" (OTTO signature)

        m8.mobi6_reader.sections = vec![vec![0u8; 4], png.clone(), font.clone()];
        m8.resource_offsets = vec![(1, 3)];

        let tmp = tempfile::tempdir().unwrap();
        let resource_map = m8
            .extract_resources(tmp.path())
            .expect("extract_resources should succeed");

        assert_eq!(resource_map.len(), 2);
        let img_href = resource_map[0].clone().expect("image should be recognized");
        assert!(img_href.starts_with("images/"), "{img_href}");
        assert!(tmp.path().join(&img_href).exists());

        let font_href = resource_map[1].clone().expect("font should be recognized");
        assert!(font_href.starts_with("fonts/"), "{font_href}");
        assert!(tmp.path().join(&font_href).exists());
    }
}
