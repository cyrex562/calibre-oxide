//! Basic support for writing LIT files.
//!
//! Port of `src/calibre/ebooks/lit/writer.py`.
//!
//! Writing is the mirror of [`super::reader`]: markup is re-tokenised
//! into LIT's binary form by [`ReBinary`], the results are gathered into
//! four content sections, and [`LitWriter`] wraps those in the ITOLITLS
//! header, directory chunks and DataSpace bookkeeping the format wants.
//!
//! As in the Python, the output is *nominally* DRM-sealed: a fixed
//! all-zero title key is written, so any reader can open the result.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::sync::OnceLock;

use roxmltree::{Document, Node};

use calibre_utils::lzx::Compressor;
use calibre_utils::msdes::{DesKey, EN0};

use super::maps::{TagMap, HTML_MAP, OPF_MAP};
use super::mssha1;
use super::reader::DirectoryEntry;
use super::{urlnormalize, urlunquote, LitError, Result};
use crate::oeb::book::OEBBook;
use crate::oeb::constants::{CSS_MIME, XHTML_MIME};
use crate::oeb::stylizer::StyleProvider;

/// `LIT_MAGIC`.
pub const LIT_MAGIC: &[u8; 8] = b"ITOLITLS";

/// `LIT_IMAGES` — the image types LIT accepts.
pub const LIT_IMAGES: [&str; 3] = ["image/png", "image/jpeg", "image/gif"];

/// The media types calibre keeps when writing a LIT file.
fn is_lit_mime(mime: &str) -> bool {
    is_oeb_doc(mime) || is_oeb_style(mime) || LIT_IMAGES.contains(&mime)
}

/// `OEB_DOCS`.
fn is_oeb_doc(mime: &str) -> bool {
    matches!(
        mime,
        "application/xhtml+xml"
            | "application/x-dtbook+xml"
            | "text/html"
            | "text/x-oeb1-document"
            | "application/x-oeb1-document"
    )
}

/// `OEB_STYLES`.
fn is_oeb_style(mime: &str) -> bool {
    matches!(
        mime,
        "text/css" | "text/x-oeb-css" | "text/x-oeb1-css" | "application/x-oeb1-css"
    )
}

/// `MS_COVER_TYPE`.
pub const MS_COVER_TYPE: &str = "other.ms-coverimage-standard";

/// `ALL_MS_COVER_TYPES`.
pub const ALL_MS_COVER_TYPES: [(&str, &str); 4] = [
    (MS_COVER_TYPE, "Standard cover image"),
    ("other.ms-thumbimage-standard", "Standard thumbnail image"),
    ("other.ms-coverimage", "PocketPC cover image"),
    ("other.ms-thumbimage", "PocketPC thumbnail image"),
];

const LITFILE_GUID: &str = "{0A9007C1-4076-11D3-8789-0000F8105754}";
const PIECE3_GUID: &str = "{0A9007C3-4076-11D3-8789-0000F8105754}";
const PIECE4_GUID: &str = "{0A9007C4-4076-11D3-8789-0000F8105754}";
const DESENCRYPT_GUID: &str = "{67F6E4A2-60BF-11D3-8540-00C04F58C3CF}";
const LZXCOMPRESS_GUID: &str = "{0A9007C6-4076-11D3-8789-0000F8105754}";

const FLAG_OPENING: u32 = 1 << 0;
const FLAG_CLOSING: u32 = 1 << 1;
const FLAG_BLOCK: u32 = 1 << 2;
const FLAG_HEAD: u32 = 1 << 3;
const FLAG_CUSTOM: u32 = 1 << 15;
const ATTR_NUMBER: u32 = 0xffff;

const PIECE_SIZE: u64 = 16;
const PRIMARY_SIZE: u32 = 40;
const SECONDARY_SIZE: u32 = 232;
const DCHUNK_SIZE: usize = 0x2000;
const CCHUNK_SIZE: usize = 0x0200;
const ULL_NEG1: u64 = 0xffff_ffff_ffff_ffff;
const ROOT_OFFSET: u64 = 1_284_508_585_713_721_976;
const ROOT_SIZE: u64 = 4_165_955_342_166_943_123;

const BLOCK_CAOL: &[u8] = &[
    0x43, 0x41, 0x4f, 0x4c, 0x02, 0x00, 0x00, 0x00, 0x50, 0x00, 0x00, 0x00, 0x37, 0x13, 0x03, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x10, 0x00,
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
const BLOCK_ITSF: &[u8] = &[
    0x49, 0x54, 0x53, 0x46, 0x04, 0x00, 0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
];
const MSDES_CONTROL: &[u8] = &[
    0x03, 0x00, 0x00, 0x00, 0x29, 0x17, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0xa5, 0xa5, 0x00, 0x00,
];
const LZXC_CONTROL: &[u8] = &[
    0x07, 0x00, 0x00, 0x00, 0x4c, 0x5a, 0x58, 0x43, 0x03, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00,
    0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];

/// `PAGE_BREAKS`.
const PAGE_BREAKS: [&str; 3] = ["always", "left", "right"];

/// `packguid` — the mixed-endian GUID layout, written out.
fn packguid(guid: &str) -> [u8; 16] {
    let hex = |s: &str| u64::from_str_radix(s, 16).unwrap_or(0);
    let mut out = [0u8; 16];
    if guid.len() < 38 {
        return out;
    }
    out[0..4].copy_from_slice(&(hex(&guid[1..9]) as u32).to_le_bytes());
    out[4..6].copy_from_slice(&(hex(&guid[10..14]) as u16).to_le_bytes());
    out[6..8].copy_from_slice(&(hex(&guid[15..19]) as u16).to_le_bytes());
    for (i, range) in [
        (20, 22),
        (22, 24),
        (25, 27),
        (27, 29),
        (29, 31),
        (31, 33),
        (33, 35),
        (35, 37),
    ]
    .iter()
    .enumerate()
    {
        out[8 + i] = hex(&guid[range.0..range.1]) as u8;
    }
    out
}

/// `decint` — a big-endian base-128 integer, high bit set on every byte
/// but the last.
fn decint(mut value: u64) -> Vec<u8> {
    let mut ans: Vec<u8> = Vec::new();
    loop {
        let mut b = (value & 0x7f) as u8;
        value >>= 7;
        if !ans.is_empty() {
            b |= 0x80;
        }
        ans.push(b);
        if value == 0 {
            break;
        }
    }
    ans.reverse();
    ans
}

/// The name-to-code direction of a [`TagMap`]. `invert_tag_map` in
/// `writer.py`.
pub struct InvertedMap {
    /// Tag name to tag code.
    pub tags: HashMap<&'static str, u32>,
    /// Per-tag attribute name to code, with the default table merged
    /// over the top; index 0 is the default table alone.
    pub tattrs: Vec<HashMap<&'static str, u32>>,
}

/// `invert_tag_map`.
///
/// Duplicate names collapse to the *last* code that carries them, which
/// is what inverting a Python dict does, and the defaults deliberately
/// override the per-tag entries.
fn invert_tag_map(map: &TagMap) -> InvertedMap {
    let mut tags = HashMap::new();
    for (i, name) in map.tags.iter().enumerate() {
        if let Some(name) = name {
            tags.insert(*name, i as u32);
        }
    }
    let mut dattrs: HashMap<&'static str, u32> = HashMap::new();
    for (code, name) in map.attrs {
        dattrs.insert(name, *code);
    }
    let mut tattrs: Vec<HashMap<&'static str, u32>> = map
        .tag_attrs
        .iter()
        .map(|table| {
            let mut m: HashMap<&'static str, u32> = HashMap::new();
            for (code, name) in *table {
                m.insert(name, *code);
            }
            if !m.is_empty() {
                for (k, v) in &dattrs {
                    m.insert(k, *v);
                }
            }
            m
        })
        .collect();
    if tattrs.is_empty() {
        tattrs.push(dattrs.clone());
    } else {
        tattrs[0] = dattrs;
    }
    InvertedMap { tags, tattrs }
}

/// `HTML_MAP` in `writer.py`, inverted once on first use.
pub fn html_write_map() -> &'static InvertedMap {
    static MAP: OnceLock<InvertedMap> = OnceLock::new();
    MAP.get_or_init(|| invert_tag_map(&HTML_MAP))
}

/// `OPF_MAP` in `writer.py`, inverted once on first use.
pub fn opf_write_map() -> &'static InvertedMap {
    static MAP: OnceLock<InvertedMap> = OnceLock::new();
    MAP.get_or_init(|| invert_tag_map(&OPF_MAP))
}

/// The style questions `ReBinary` asks of `Stylizer`.
///
/// `ResolvedStyle` covers `display` but not the page-break or
/// white-space properties, so this is the seam for those.
pub trait LitStyles {
    /// `not style['display'] in ('inline', 'inline-block')`.
    fn is_block(&self, node: Node) -> bool;
    /// `style['page-break-before']`.
    fn page_break_before(&self, node: Node) -> String;
    /// `style['page-break-after']`.
    fn page_break_after(&self, node: Node) -> String;
    /// `style['white-space']`.
    fn white_space(&self, node: Node) -> String;
}

/// A [`LitStyles`] built on any [`StyleProvider`], reading the
/// page-break and white-space properties straight off the element's
/// `style` attribute.
pub struct ProviderStyles<'a> {
    /// Where `display` comes from.
    pub provider: &'a dyn StyleProvider,
}

impl<'a> ProviderStyles<'a> {
    /// Wrap a provider.
    pub fn new(provider: &'a dyn StyleProvider) -> Self {
        ProviderStyles { provider }
    }

    fn declared(node: Node, property: &str) -> Option<String> {
        let style = node.attribute("style")?;
        for decl in style.split(';') {
            let (name, value) = decl.split_once(':')?;
            if name.trim().eq_ignore_ascii_case(property) {
                return Some(value.trim().to_lowercase());
            }
        }
        None
    }
}

impl LitStyles for ProviderStyles<'_> {
    fn is_block(&self, node: Node) -> bool {
        let display = self.provider.style(node).display;
        !matches!(display.as_str(), "inline" | "inline-block")
    }

    fn page_break_before(&self, node: Node) -> String {
        Self::declared(node, "page-break-before").unwrap_or_else(|| "auto".into())
    }

    fn page_break_after(&self, node: Node) -> String {
        Self::declared(node, "page-break-after").unwrap_or_else(|| "auto".into())
    }

    fn white_space(&self, node: Node) -> String {
        Self::declared(node, "white-space").unwrap_or_else(|| "normal".into())
    }
}

/// Where an anchor sits in the tokenised stream.
/// `ReBinary.anchors`.
pub type Anchor = (String, u32);

/// A page break: byte offset in the tokenised stream, and the offsets
/// of the elements enclosing it. `ReBinary.page_breaks`.
pub type PageBreak = (u32, Vec<u32>);

/// `ReBinary` in `writer.py` — tokenise a document into LIT's binary
/// markup.
pub struct ReBinary {
    /// The tokenised document.
    pub content: Vec<u8>,
    /// `ahc` — the anchor table.
    pub ahc: Vec<u8>,
    /// `aht` — always four zero bytes.
    pub aht: Vec<u8>,
    /// Anchors found while walking.
    pub anchors: Vec<Anchor>,
    /// Page breaks found while walking.
    pub page_breaks: Vec<PageBreak>,
    /// Non-fatal problems, which the Python logs.
    pub warnings: Vec<String>,
}

struct ReBinaryState<'a> {
    buf: Vec<u8>,
    anchors: Vec<Anchor>,
    page_breaks: Vec<PageBreak>,
    warnings: Vec<String>,
    map: &'static InvertedMap,
    is_html: bool,
    styles: Option<&'a dyn LitStyles>,
    /// href to manifest id, for rewriting internal links.
    hrefs: &'a HashMap<String, String>,
    /// The document's own href, for resolving relative links.
    item_href: Option<String>,
}

const XML_NS: &str = "http://www.w3.org/XML/1998/namespace";

impl ReBinary {
    /// `ReBinary.__init__`.
    ///
    /// `styles` is `None` for the OPF, which the Python signals by
    /// passing `map=OPF_MAP` and building no `Stylizer`.
    pub fn new(
        root: Node,
        item_href: Option<&str>,
        hrefs: &HashMap<String, String>,
        map: &'static InvertedMap,
        is_html: bool,
        styles: Option<&dyn LitStyles>,
    ) -> Self {
        let mut state = ReBinaryState {
            buf: Vec::new(),
            anchors: Vec::new(),
            page_breaks: Vec::new(),
            warnings: Vec::new(),
            map,
            is_html,
            styles: if is_html { styles } else { None },
            hrefs,
            item_href: item_href.map(str::to_string),
        };
        let mut nsrmap: HashMap<String, String> = HashMap::new();
        nsrmap.insert(String::new(), String::new());
        nsrmap.insert(XML_NS.to_string(), "xml".to_string());
        let mut parents: Vec<u32> = Vec::new();
        state.tree_to_binary(root, &nsrmap, &mut parents, false, false);

        let content = std::mem::take(&mut state.buf);
        let anchors = state.anchors;
        let page_breaks = state.page_breaks;
        let mut warnings = state.warnings;

        let (ahc, aht) = if is_html {
            (build_ahc(&anchors, &mut warnings, item_href), build_aht())
        } else {
            (Vec::new(), Vec::new())
        };

        ReBinary {
            content,
            ahc,
            aht,
            anchors,
            page_breaks,
            warnings,
        }
    }
}

/// `ReBinary.build_ahc`.
fn build_ahc(anchors: &[Anchor], warnings: &mut Vec<String>, item_href: Option<&str>) -> Vec<u8> {
    if anchors.len() > 6 {
        warnings.push(format!(
            "More than six anchors in file {:?}. Some links may not work properly.",
            item_href.unwrap_or("")
        ));
    }
    let mut data = Vec::new();
    write_char(&mut data, anchors.len() as u32);
    for (anchor, offset) in anchors {
        write_char(&mut data, anchor.chars().count() as u32);
        data.extend_from_slice(anchor.as_bytes());
        data.extend_from_slice(&offset.to_le_bytes());
    }
    data
}

/// `ReBinary.build_aht`.
fn build_aht() -> Vec<u8> {
    0u32.to_le_bytes().to_vec()
}

/// `ReBinary.write` for an integer: `chr(value)` encoded as UTF-8.
fn write_char(buf: &mut Vec<u8>, value: u32) {
    match char::from_u32(value) {
        Some(ch) => {
            let mut tmp = [0u8; 4];
            buf.extend_from_slice(ch.encode_utf8(&mut tmp).as_bytes());
        }
        // `chr()` raised: the Python logs and substitutes '?'.
        None => buf.push(b'?'),
    }
}

/// `COLLAPSE` — runs of whitespace become a single space.
fn collapse(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for ch in s.chars() {
        if matches!(ch, ' ' | '\t' | '\r' | '\n' | '\u{0b}') {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(ch);
            in_ws = false;
        }
    }
    out
}

/// `prefixname` in `oeb.base` — a qualified name rendered with the
/// prefix the current namespace map assigns.
fn prefixname(local: &str, uri: Option<&str>, nsrmap: &HashMap<String, String>) -> String {
    let uri = uri.unwrap_or("");
    match nsrmap.get(uri) {
        Some(prefix) if prefix.is_empty() => local.to_string(),
        Some(prefix) => format!("{prefix}:{local}"),
        None => local.to_string(),
    }
}

impl ReBinaryState<'_> {
    /// `ReBinary.tree_to_binary`.
    fn tree_to_binary(
        &mut self,
        elem: Node,
        nsrmap: &HashMap<String, String>,
        parents: &mut Vec<u32>,
        inhead: bool,
        preserve: bool,
    ) {
        if !elem.is_element() {
            // Don't emit any comments or raw entities.
            return;
        }
        let mut nsrmap = nsrmap.clone();
        let mut attrib: Vec<(String, String)> = Vec::new();
        for attr in elem.attributes() {
            attrib.push((
                prefixname(attr.name(), attr.namespace(), &nsrmap),
                attr.value().to_string(),
            ));
        }
        for ns in elem.namespaces() {
            let key = ns.name().unwrap_or("");
            let value = ns.uri();
            if nsrmap.get(value).map(String::as_str) != Some(key) {
                let xmlns = if key.is_empty() {
                    "xmlns".to_string()
                } else {
                    format!("xmlns:{key}")
                };
                attrib.push((xmlns, value.to_string()));
            }
            nsrmap.insert(value.to_string(), key.to_string());
        }

        let tag = prefixname(elem.tag_name().name(), elem.tag_name().namespace(), &nsrmap);
        let tag_offset = self.buf.len() as u32;
        let inhead = inhead || tag == "head";

        let has_text = elem
            .children()
            .any(|c| c.is_text() && !c.text().unwrap_or("").is_empty());
        let child_count = elem.children().filter(Node::is_element).count();

        let mut flags = FLAG_OPENING;
        if !has_text && child_count == 0 {
            flags |= FLAG_CLOSING;
        }
        if inhead {
            flags |= FLAG_HEAD;
        }
        let styled = self.styles.is_some();
        if styled && self.styles.expect("checked").is_block(elem) {
            flags |= FLAG_BLOCK;
        }
        self.buf.push(0);
        write_char(&mut self.buf, flags);

        let mut tattrs_index = 0usize;
        if let Some(index) = self.map.tags.get(tag.as_str()).copied() {
            write_char(&mut self.buf, index);
            if self
                .map
                .tattrs
                .get(index as usize)
                .is_some_and(|m| !m.is_empty())
            {
                tattrs_index = index as usize;
            }
        } else {
            write_char(&mut self.buf, FLAG_CUSTOM);
            write_char(&mut self.buf, tag.chars().count() as u32 + 1);
            self.buf.extend_from_slice(tag.as_bytes());
        }

        let last_break = self.page_breaks.last().map(|(o, _)| *o);
        if styled
            && last_break != Some(tag_offset)
            && PAGE_BREAKS.contains(
                &self
                    .styles
                    .expect("checked")
                    .page_break_before(elem)
                    .as_str(),
            )
        {
            self.page_breaks.push((tag_offset, parents.clone()));
        }

        for (attr, value) in attrib {
            let mut attr = attr;
            let mut value = value;
            if attr == "href" || attr == "src" {
                value = self.rewrite_link(&value);
            } else if attr == "id" || attr == "name" {
                self.anchors.push((value.clone(), tag_offset));
            } else if let Some(rest) = attr.strip_prefix("ms--") {
                attr = format!("%{rest}");
            } else if tag == "link" && attr == "type" && is_oeb_style(&value) {
                value = CSS_MIME.to_string();
            }

            match self.map.tattrs[tattrs_index].get(attr.as_str()).copied() {
                Some(code) => write_char(&mut self.buf, code),
                None => {
                    write_char(&mut self.buf, FLAG_CUSTOM);
                    write_char(&mut self.buf, attr.chars().count() as u32 + 1);
                    self.buf.extend_from_slice(attr.as_bytes());
                }
            }
            match value.parse::<i64>() {
                Ok(n) => {
                    write_char(&mut self.buf, ATTR_NUMBER);
                    write_char(&mut self.buf, (n + 1) as u32);
                }
                Err(_) => {
                    write_char(&mut self.buf, value.chars().count() as u32 + 1);
                    self.buf.extend_from_slice(value.as_bytes());
                }
            }
        }
        self.buf.push(0);

        let old_preserve = preserve;
        let mut preserve = preserve;
        if styled {
            let ws = self.styles.expect("checked").white_space(elem);
            preserve = ws == "pre" || ws == "pre-wrap";
        }
        match elem.attribute((XML_NS, "space")) {
            Some("preserve") => preserve = true,
            Some("normal") => preserve = false,
            _ => {}
        }

        if let Some(text) = first_text(elem) {
            if preserve {
                self.buf.extend_from_slice(text.as_bytes());
            } else if child_count == 0 || !text.trim().is_empty() {
                self.buf.extend_from_slice(collapse(&text).as_bytes());
            }
        }

        parents.push(tag_offset);
        let children: Vec<Node> = elem.children().filter(Node::is_element).collect();
        for (i, child) in children.iter().enumerate() {
            let next = children.get(i + 1);
            // Drop whitespace-only tails between blocks, as the Python
            // does by clearing `child.tail`.
            let tail = element_tail(*child);
            let drop_tail = !preserve
                && (inhead
                    || next.is_none()
                    || !styled
                    || self.styles.expect("checked").is_block(*child)
                    || next.is_some_and(|n| self.styles.expect("checked").is_block(*n)))
                && tail
                    .as_deref()
                    .is_some_and(|t| t.trim().is_empty() && !t.is_empty());
            self.tree_to_binary_child(*child, &nsrmap, parents, inhead, preserve, drop_tail);
        }
        parents.pop();
        let preserve = old_preserve;

        if flags & FLAG_CLOSING == 0 {
            self.buf.push(0);
            write_char(&mut self.buf, (flags & !FLAG_OPENING) | FLAG_CLOSING);
            self.buf.push(0);
        }

        if styled {
            let pba = self.styles.expect("checked").page_break_after(elem);
            if pba != "avoid" && pba != "auto" {
                self.page_breaks
                    .push((self.buf.len() as u32, parents.clone()));
            }
        }
        let _ = preserve;
    }

    /// Recurse into a child and then emit its tail text, which in lxml
    /// belongs to the child rather than the parent.
    fn tree_to_binary_child(
        &mut self,
        child: Node,
        nsrmap: &HashMap<String, String>,
        parents: &mut Vec<u32>,
        inhead: bool,
        preserve: bool,
        drop_tail: bool,
    ) {
        self.tree_to_binary(child, nsrmap, parents, inhead, preserve);
        let tag = prefixname(
            child.tag_name().name(),
            child.tag_name().namespace(),
            nsrmap,
        );
        if drop_tail || tag == "html" {
            return;
        }
        if let Some(tail) = element_tail(child) {
            let tail = if preserve { tail } else { collapse(&tail) };
            self.buf.extend_from_slice(tail.as_bytes());
        }
    }

    /// The `href`/`src` rewriting in `tree_to_binary`: internal targets
    /// become `\x02<id>`, everything else `\x03<url>`.
    fn rewrite_link(&self, value: &str) -> String {
        let value = urlnormalize(value);
        let (path, frag) = match value.split_once('#') {
            Some((p, f)) => (p.to_string(), Some(f.to_string())),
            None => (value.clone(), None),
        };
        let abs = match &self.item_href {
            Some(base) => abshref(base, &path),
            None => path.clone(),
        };
        match self.hrefs.get(&abs) {
            Some(id) => {
                let mut v = String::from('\u{2}');
                v.push_str(id);
                if let Some(frag) = frag.filter(|f| !f.is_empty()) {
                    v.push('#');
                    v.push_str(&frag);
                }
                v
            }
            None => {
                let mut v = String::from('\u{3}');
                v.push_str(&value);
                v
            }
        }
    }
}

/// The text directly inside an element, before its first child.
/// `elem.text` in lxml.
fn first_text(elem: Node) -> Option<String> {
    let first = elem.first_child()?;
    if first.is_text() {
        first.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

/// The text following an element, up to its next sibling.
/// `elem.tail` in lxml.
fn element_tail(elem: Node) -> Option<String> {
    let next = elem.next_sibling()?;
    if next.is_text() {
        next.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

/// `item.abshref` — resolve `href` against the directory of `base`.
fn abshref(base: &str, href: &str) -> String {
    if href.is_empty() || href.contains("://") {
        return href.to_string();
    }
    let dir = match base.rfind('/') {
        Some(i) => &base[..i + 1],
        None => "",
    };
    let joined = format!("{dir}{href}");
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `LitWriter` in `writer.py`.
pub struct LitWriter {
    sections: [Vec<u8>; 4],
    directory: Vec<DirectoryEntry>,
    meta: Vec<u8>,
    bookkey: [u8; 8],
    /// Each spine item's running byte offset, as recorded in the
    /// manifest and reused by the page-break tables.
    item_offsets: HashMap<String, u32>,
    /// Non-fatal problems noticed while writing.
    pub warnings: Vec<String>,
    /// The timestamp stamped into the ITSF block. Settable so output is
    /// reproducible; defaults to the Unix epoch.
    pub timestamp: u32,
    /// The GUID stamped into the OPF's `ms--guid` attribute.
    pub guid: String,
}

impl Default for LitWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl LitWriter {
    /// `LitWriter.__init__`.
    pub fn new() -> Self {
        LitWriter {
            sections: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            directory: Vec::new(),
            meta: Vec::new(),
            bookkey: [0u8; 8],
            item_offsets: HashMap::new(),
            warnings: Vec::new(),
            timestamp: 0,
            guid: "{00000000-0000-0000-0000-000000000000}".to_string(),
        }
    }

    /// `LitWriter.__call__` — write `oeb` to `stream`.
    pub fn write<W: Write + Seek>(
        &mut self,
        oeb: &OEBBook,
        styles: Option<&dyn LitStyles>,
        stream: &mut W,
    ) -> Result<()> {
        self.build_sections(oeb, styles)?;
        self.write_content(stream)
    }

    /// `LitWriter._add_file`.
    fn add_file(&mut self, name: &str, data: &[u8], secnum: usize) {
        let offset = if data.is_empty() {
            0
        } else {
            let section = &mut self.sections[secnum];
            let offset = section.len() as u64;
            section.extend_from_slice(data);
            offset
        };
        self.directory.push(DirectoryEntry::new(
            name,
            secnum as u64,
            offset,
            data.len() as u64,
        ));
    }

    /// `LitWriter._add_folder`.
    fn add_folder(&mut self, name: &str, offset: u64, size: u64) {
        let name = if name.ends_with('/') {
            name.to_string()
        } else {
            format!("{name}/")
        };
        self.directory
            .push(DirectoryEntry::new(name, 0, offset, size));
    }

    /// `LitWriter._build_sections`.
    fn build_sections(&mut self, oeb: &OEBBook, styles: Option<&dyn LitStyles>) -> Result<()> {
        self.add_folder("/", ROOT_OFFSET, ROOT_SIZE);
        let sizes = self.build_data(oeb, styles)?;
        self.build_manifest(oeb, &sizes);
        self.build_page_breaks(oeb, &sizes);
        self.build_meta(oeb)?;
        self.build_drm_storage();
        self.build_version();
        self.build_namelist();
        self.build_storage()?;
        self.build_transforms();
        Ok(())
    }

    /// `LitWriter._build_data`.
    ///
    /// Returns each item's stored size, which the manifest and page
    /// break tables need.
    fn build_data(
        &mut self,
        oeb: &OEBBook,
        styles: Option<&dyn LitStyles>,
    ) -> Result<HashMap<String, ItemInfo>> {
        self.add_folder("/data", 0, 0);
        let mut infos: HashMap<String, ItemInfo> = HashMap::new();

        let mut ids: Vec<&String> = oeb.manifest.items.keys().collect();
        ids.sort();
        for id in ids {
            let item = &oeb.manifest.items[id];
            if !is_lit_mime(&item.media_type) {
                self.warnings.push(format!(
                    "File {:?} of unknown media-type {:?} excluded from output.",
                    item.href, item.media_type
                ));
                continue;
            }
            let name = format!("/data/{}", item.id);
            let data = oeb
                .container
                .read(&item.href)
                .map_err(|e| LitError::msg(format!("Could not read {:?}: {e}", item.href)))?;

            if is_oeb_doc(&item.media_type) {
                self.add_folder(&name, 0, 0);
                let text = String::from_utf8_lossy(&data).into_owned();
                let doc = Document::parse(&text)
                    .map_err(|e| LitError::msg(format!("Could not parse {:?}: {e}", item.href)))?;
                let rebin = ReBinary::new(
                    doc.root_element(),
                    Some(&item.href),
                    &oeb.manifest.hrefs,
                    html_write_map(),
                    true,
                    styles,
                );
                self.warnings.extend(rebin.warnings.iter().cloned());
                self.add_file(&format!("{name}/ahc"), &rebin.ahc, 0);
                self.add_file(&format!("{name}/aht"), &rebin.aht, 0);
                let size = rebin.content.len();
                self.add_file(&format!("{name}/content"), &rebin.content, 1);
                infos.insert(
                    item.id.clone(),
                    ItemInfo {
                        size,
                        page_breaks: rebin.page_breaks,
                    },
                );
            } else {
                let size = data.len();
                self.add_file(&name, &data, 0);
                infos.insert(
                    item.id.clone(),
                    ItemInfo {
                        size,
                        page_breaks: Vec::new(),
                    },
                );
            }
        }
        Ok(infos)
    }

    /// `LitWriter._build_manifest`.
    fn build_manifest(&mut self, oeb: &OEBBook, sizes: &HashMap<String, ItemInfo>) {
        let mut buckets: HashMap<&str, Vec<&crate::oeb::manifest::ManifestItem>> = HashMap::new();
        for state in ["linear", "nonlinear", "css", "images"] {
            buckets.insert(state, Vec::new());
        }
        let spine_pos: HashMap<&str, (usize, bool)> = oeb
            .spine
            .items
            .iter()
            .enumerate()
            .map(|(i, s)| (s.idref.as_str(), (i, s.linear)))
            .collect();

        for item in oeb.manifest.items.values() {
            if !sizes.contains_key(&item.id) {
                continue;
            }
            if let Some((_, linear)) = spine_pos.get(item.id.as_str()) {
                let key = if *linear { "linear" } else { "nonlinear" };
                buckets.get_mut(key).expect("bucket").push(item);
            } else if is_oeb_style(&item.media_type) {
                buckets.get_mut("css").expect("bucket").push(item);
            } else if LIT_IMAGES.contains(&item.media_type.as_str()) {
                buckets.get_mut("images").expect("bucket").push(item);
            }
        }

        let mut data: Vec<u8> = Vec::new();
        data.push(1);
        data.push(b'\\');
        let mut offset: u32 = 0;
        let mut offsets: HashMap<String, u32> = HashMap::new();
        for state in ["linear", "nonlinear", "css", "images"] {
            let mut items = buckets.remove(state).unwrap_or_default();
            // `sort_key` on Item is (spine position, href).
            items.sort_by(|a, b| {
                let ka = spine_pos.get(a.id.as_str()).map(|(i, _)| *i);
                let kb = spine_pos.get(b.id.as_str()).map(|(i, _)| *i);
                ka.cmp(&kb).then_with(|| a.href.cmp(&b.href))
            });
            data.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                let media_type = if is_oeb_doc(&item.media_type) {
                    // Needs to have 'html' in the media type.
                    XHTML_MIME
                } else if is_oeb_style(&item.media_type) {
                    CSS_MIME
                } else {
                    item.media_type.as_str()
                };
                let href = urlunquote(&item.href);
                let item_offset = if state == "linear" || state == "nonlinear" {
                    offset
                } else {
                    0
                };
                offsets.insert(item.id.clone(), item_offset);
                data.extend_from_slice(&item_offset.to_le_bytes());
                for value in [item.id.as_str(), href.as_str(), media_type] {
                    write_char(&mut data, value.chars().count() as u32);
                    data.extend_from_slice(value.as_bytes());
                }
                data.push(0);
                offset += sizes.get(&item.id).map_or(0, |i| i.size as u32);
            }
        }
        self.item_offsets = offsets;
        self.add_file("/manifest", &data, 0);
    }

    /// `LitWriter._build_page_breaks`.
    fn build_page_breaks(&mut self, oeb: &OEBBook, sizes: &HashMap<String, ItemInfo>) {
        let mut pb1: Vec<u8> = Vec::new();
        let mut pb2: Vec<u8> = Vec::new();
        let mut pb3: Vec<u8> = Vec::new();
        let mut pb3cur: u32 = 0;
        let mut bits = 0u32;

        let mut order: Vec<&crate::oeb::spine::SpineItem> =
            oeb.spine.items.iter().filter(|s| s.linear).collect();
        order.extend(oeb.spine.items.iter().filter(|s| !s.linear));

        for spine_item in order {
            let Some(info) = sizes.get(&spine_item.idref) else {
                continue;
            };
            let item_offset = self
                .item_offsets
                .get(&spine_item.idref)
                .copied()
                .unwrap_or(0);
            let mut page_breaks = info.page_breaks.clone();
            if !spine_item.linear {
                page_breaks.insert(0, (0, Vec::new()));
            }
            for (pbreak, parents) in page_breaks {
                pb3cur = (pb3cur << 2) | 1;
                if parents.len() > 1 {
                    pb3cur |= 0x2;
                }
                bits += 2;
                if bits >= 8 {
                    pb3.push(pb3cur as u8);
                    pb3cur = 0;
                    bits = 0;
                }
                let pbreak = pbreak + item_offset;
                pb1.extend_from_slice(&pbreak.to_le_bytes());
                pb1.extend_from_slice(&(pb2.len() as u32).to_le_bytes());
                pb2.extend_from_slice(&(parents.len() as u32).to_le_bytes());
                for parent in parents {
                    pb2.extend_from_slice(&parent.to_le_bytes());
                }
            }
        }
        if bits != 0 {
            pb3cur <<= 8 - bits;
            pb3.push(pb3cur as u8);
        }
        self.add_file("/pb1", &pb1, 0);
        self.add_file("/pb2", &pb2, 0);
        self.add_file("/pb3", &pb3, 0);
    }

    /// `LitWriter._build_meta` — tokenise the OEB 1.0.1 package.
    fn build_meta(&mut self, oeb: &OEBBook) -> Result<()> {
        let opf = to_opf1(oeb, &self.guid);
        let doc = Document::parse(&opf)
            .map_err(|e| LitError::msg(format!("Could not parse generated OPF: {e}")))?;
        let hrefs = HashMap::new();
        let rebin = ReBinary::new(
            doc.root_element(),
            None,
            &hrefs,
            opf_write_map(),
            false,
            None,
        );
        self.meta = rebin.content.clone();
        self.add_file("/meta", &rebin.content, 0);
        Ok(())
    }

    /// `LitWriter._build_drm_storage`.
    ///
    /// "Free as in freedom": the title key is all zeroes, so the result
    /// is readable by anything.
    fn build_drm_storage(&mut self) {
        let drmsource: Vec<u8> = "Free as in freedom\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        self.add_file("/DRMStorage/DRMSource", &drmsource, 0);
        let tempkey = mssha1::calculate_deskey(&[&self.meta, &drmsource]);
        let sealed = DesKey::new(&tempkey, EN0)
            .process(&[0u8; 16])
            .expect("two whole DES blocks");
        self.add_file("/DRMStorage/DRMSealed", &sealed, 0);
        self.bookkey = [0u8; 8];
        self.add_file("/DRMStorage/ValidationStream", b"MSReader", 3);
    }

    /// `LitWriter._build_version`.
    fn build_version(&mut self) {
        let mut data = Vec::new();
        data.extend_from_slice(&8u16.to_le_bytes());
        data.extend_from_slice(&1u16.to_le_bytes());
        self.add_file("/Version", &data, 0);
    }

    /// `LitWriter._build_namelist`.
    fn build_namelist(&mut self) {
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&0x3cu16.to_le_bytes());
        data.extend_from_slice(&(self.sections.len() as u16).to_le_bytes());
        for name in [
            "Uncompressed",
            "MSCompressed",
            "EbEncryptDS",
            "EbEncryptOnlyDS",
        ] {
            data.extend_from_slice(&(name.len() as u16).to_le_bytes());
            for unit in name.encode_utf16() {
                data.extend_from_slice(&unit.to_le_bytes());
            }
            data.extend_from_slice(&[0, 0]);
        }
        self.add_file("::DataSpace/NameList", &data, 0);
    }

    /// `LitWriter._build_storage` — apply each section's transforms and
    /// record the bookkeeping the reader walks back.
    fn build_storage(&mut self) -> Result<()> {
        let mapping: [(usize, &str, &[&str]); 3] = [
            (1, "MSCompressed", &[LZXCOMPRESS_GUID]),
            (2, "EbEncryptDS", &[LZXCOMPRESS_GUID, DESENCRYPT_GUID]),
            (3, "EbEncryptOnlyDS", &[DESENCRYPT_GUID]),
        ];
        for (secnum, name, transforms) in mapping {
            let root = format!("::DataSpace/Storage/{name}");
            let mut data = self.sections[secnum].clone();
            let mut cdata: Vec<u8> = Vec::new();
            let mut sdata: Vec<u8> = Vec::new();
            let mut tdata: Vec<u8> = Vec::new();
            let mut rdata: Vec<u8> = Vec::new();

            for guid in transforms {
                let mut next = packguid(guid).to_vec();
                next.extend_from_slice(&tdata);
                tdata = next;
                sdata.extend_from_slice(&(data.len() as u64).to_le_bytes());

                if *guid == DESENCRYPT_GUID {
                    let mut next = MSDES_CONTROL.to_vec();
                    next.extend_from_slice(&cdata);
                    cdata = next;
                    if data.is_empty() {
                        continue;
                    }
                    let pad = 8 - (data.len() & 0x7);
                    if pad != 8 {
                        data.resize(data.len() + pad, 0);
                    }
                    data = DesKey::new(&self.bookkey, EN0)
                        .process(&data)
                        .ok_or_else(|| LitError::msg("Section is not a whole number of blocks"))?;
                } else if *guid == LZXCOMPRESS_GUID {
                    let mut next = LZXC_CONTROL.to_vec();
                    next.extend_from_slice(&cdata);
                    cdata = next;
                    if data.is_empty() {
                        continue;
                    }
                    let unlen = data.len();
                    let mut lzx = Compressor::new(17)
                        .map_err(|e| LitError::msg(format!("Could not start LZX: {e}")))?;
                    let (compressed, rtable) = lzx.compress(&data, true);
                    rdata.clear();
                    rdata.extend_from_slice(&3u32.to_le_bytes());
                    rdata.extend_from_slice(&(rtable.len() as u32).to_le_bytes());
                    rdata.extend_from_slice(&8u32.to_le_bytes());
                    rdata.extend_from_slice(&0x28u32.to_le_bytes());
                    rdata.extend_from_slice(&(unlen as u64).to_le_bytes());
                    rdata.extend_from_slice(&(compressed.len() as u64).to_le_bytes());
                    rdata.extend_from_slice(&0x8000u64.to_le_bytes());
                    rdata.extend_from_slice(&0u64.to_le_bytes());
                    for (_, comp) in rtable.iter().take(rtable.len().saturating_sub(1)) {
                        rdata.extend_from_slice(&u64::from(*comp).to_le_bytes());
                    }
                    data = compressed;
                }
            }
            self.add_file(&format!("{root}/Content"), &data, 0);
            self.add_file(&format!("{root}/ControlData"), &cdata, 0);
            self.add_file(&format!("{root}/SpanInfo"), &sdata, 0);
            self.add_file(&format!("{root}/Transform/List"), &tdata, 0);
            let troot = format!("{root}/Transform");
            for guid in transforms {
                let dname = format!("{troot}/{guid}/InstanceData");
                self.add_folder(&dname, 0, 0);
                if *guid == LZXCOMPRESS_GUID {
                    self.add_file(&format!("{dname}/ResetTable"), &rdata, 0);
                }
            }
        }
        Ok(())
    }

    /// `LitWriter._build_transforms`.
    fn build_transforms(&mut self) {
        for guid in [LZXCOMPRESS_GUID, DESENCRYPT_GUID] {
            self.add_folder(&format!("::Transform/{guid}"), 0, 0);
        }
    }

    /// `LitWriter._build_dchunks` — pack the directory into 8K chunks,
    /// with an index chunk when more than one is needed.
    fn build_dchunks(&self) -> (Vec<u32>, Vec<Vec<u8>>, Option<Vec<u8>>) {
        let mut directory = self.directory.clone();
        directory.sort_by_key(|e| e.name.to_lowercase());
        let qrn = 1 + (1 << 2);

        struct Pending {
            content: Vec<u8>,
            quickref: Vec<u16>,
            dcount: u32,
            name: Vec<u8>,
        }
        let mut ddata: Vec<Pending> = Vec::new();
        let mut dchunk: Vec<u8> = Vec::new();
        let mut dcount: u32 = 0;
        let mut quickref: Vec<u16> = Vec::new();
        let mut name: Vec<u8> = directory
            .first()
            .map(|e| e.name.as_bytes().to_vec())
            .unwrap_or_default();

        for entry in &directory {
            let en = entry.name.as_bytes();
            let mut next: Vec<u8> = Vec::new();
            next.extend_from_slice(&decint(en.len() as u64));
            next.extend_from_slice(en);
            next.extend_from_slice(&decint(entry.section));
            next.extend_from_slice(&decint(entry.offset));
            next.extend_from_slice(&decint(entry.size));

            let usedlen = dchunk.len() + next.len() + (quickref.len() * 2) + 52;
            if usedlen >= DCHUNK_SIZE {
                ddata.push(Pending {
                    content: std::mem::take(&mut dchunk),
                    quickref: std::mem::take(&mut quickref),
                    dcount,
                    name: std::mem::replace(&mut name, en.to_vec()),
                });
                dcount = 0;
            }
            if dcount % qrn == 0 {
                quickref.push(dchunk.len() as u16);
            }
            dchunk.extend_from_slice(&next);
            dcount += 1;
        }
        ddata.push(Pending {
            content: dchunk,
            quickref,
            dcount,
            name,
        });

        let cidmax = ddata.len() - 1;
        let mut rdcount: u64 = 0;
        let mut dchunks: Vec<Vec<u8>> = Vec::new();
        let mut dcounts: Vec<u32> = Vec::new();
        let mut ichunk: Option<Vec<u8>> = if ddata.len() > 1 {
            Some(Vec::new())
        } else {
            None
        };

        for (cid, pending) in ddata.iter().enumerate() {
            let mut out: Vec<u8> = Vec::new();
            let prev = if cid > 0 { (cid - 1) as u64 } else { ULL_NEG1 };
            let next = if cid < cidmax {
                (cid + 1) as u64
            } else {
                ULL_NEG1
            };
            let rem = DCHUNK_SIZE as i64 - (pending.content.len() as i64 + 50);
            let pad = rem - (pending.quickref.len() as i64 * 2);
            out.extend_from_slice(b"AOLL");
            out.extend_from_slice(&(rem as u32).to_le_bytes());
            out.extend_from_slice(&(cid as u64).to_le_bytes());
            out.extend_from_slice(&prev.to_le_bytes());
            out.extend_from_slice(&next.to_le_bytes());
            out.extend_from_slice(&rdcount.to_le_bytes());
            out.extend_from_slice(&1u64.to_le_bytes());
            out.extend_from_slice(&pending.content);
            out.resize(out.len() + pad.max(0) as usize, 0);
            for reference in pending.quickref.iter().rev() {
                out.extend_from_slice(&reference.to_le_bytes());
            }
            out.extend_from_slice(&(pending.dcount as u16).to_le_bytes());
            rdcount += u64::from(pending.dcount);
            dchunks.push(out);
            dcounts.push(pending.dcount);
            if let Some(ic) = ichunk.as_mut() {
                ic.extend_from_slice(&decint(pending.name.len() as u64));
                ic.extend_from_slice(&pending.name);
                ic.extend_from_slice(&decint(cid as u64));
            }
        }

        let ichunk = ichunk.map(|body| {
            let rem = DCHUNK_SIZE as i64 - (body.len() as i64 + 16);
            let pad = (rem - 2).max(0) as usize;
            let mut out: Vec<u8> = Vec::new();
            out.extend_from_slice(b"AOLI");
            out.extend_from_slice(&(rem as u32).to_le_bytes());
            out.extend_from_slice(&(dchunks.len() as u64).to_le_bytes());
            out.extend_from_slice(&body);
            out.resize(out.len() + pad, 0);
            out.extend_from_slice(&(dchunks.len() as u16).to_le_bytes());
            out
        });

        (dcounts, dchunks, ichunk)
    }

    /// `LitWriter._write_content`.
    fn write_content<W: Write + Seek>(&mut self, stream: &mut W) -> Result<()> {
        let (dcounts, dchunks, ichunk) = self.build_dchunks();
        let io = |e: std::io::Error| LitError::msg(format!("Write failed: {e}"));

        stream.write_all(LIT_MAGIC).map_err(io)?;
        for v in [1u32, PRIMARY_SIZE, 5, SECONDARY_SIZE] {
            stream.write_all(&v.to_le_bytes()).map_err(io)?;
        }
        stream.write_all(&packguid(LITFILE_GUID)).map_err(io)?;
        let piece_base = stream.stream_position().map_err(io)?;
        let pieces: Vec<u64> = (0..5).map(|i| piece_base + i * PIECE_SIZE).collect();
        stream
            .write_all(&vec![0u8; 5 * PIECE_SIZE as usize])
            .map_err(io)?;

        let aoli1 = if ichunk.is_some() {
            dchunks.len() as u64
        } else {
            ULL_NEG1
        };
        let last = (dchunks.len() - 1) as u64;
        let ddepth: u32 = if ichunk.is_some() { 2 } else { 1 };

        // '<IIQQQQIIIIQIIQQQQIIIIQIIIIQ'
        write_u32(stream, 2)?;
        write_u32(stream, 0x98)?;
        write_u64(stream, aoli1)?;
        write_u64(stream, 0)?;
        write_u64(stream, last)?;
        write_u64(stream, 0)?;
        write_u32(stream, DCHUNK_SIZE as u32)?;
        write_u32(stream, 2)?;
        write_u32(stream, 0)?;
        write_u32(stream, ddepth)?;
        write_u64(stream, 0)?;
        write_u32(stream, self.directory.len() as u32)?;
        write_u32(stream, 0)?;
        write_u64(stream, ULL_NEG1)?;
        write_u64(stream, 0)?;
        write_u64(stream, 0)?;
        write_u64(stream, 0)?;
        write_u32(stream, CCHUNK_SIZE as u32)?;
        write_u32(stream, 2)?;
        write_u32(stream, 0)?;
        write_u32(stream, 1)?;
        write_u64(stream, 0)?;
        write_u32(stream, dcounts.len() as u32)?;
        write_u32(stream, 0)?;
        write_u32(stream, 0x100000)?;
        write_u32(stream, 0x20000)?;
        write_u64(stream, 0)?;

        stream.write_all(BLOCK_CAOL).map_err(io)?;
        stream.write_all(BLOCK_ITSF).map_err(io)?;
        let conoff_offset = stream.stream_position().map_err(io)?;
        write_u64(stream, 0)?;
        write_u32(stream, self.timestamp)?;
        write_u32(stream, 0x409)?;

        // Piece #0
        let piece0_offset = stream.stream_position().map_err(io)?;
        write_u32(stream, 0x1fe)?;
        write_u32(stream, 0)?;
        let filesz_offset = stream.stream_position().map_err(io)?;
        write_u64(stream, 0)?;
        write_u64(stream, 0)?;
        let here = stream.stream_position().map_err(io)?;
        write_at(stream, pieces[0], piece0_offset, here - piece0_offset)?;

        // Piece #1: directory chunks
        let piece1_offset = stream.stream_position().map_err(io)?;
        stream.write_all(b"IFCM").map_err(io)?;
        write_u32(stream, 1)?;
        write_u32(stream, DCHUNK_SIZE as u32)?;
        write_u32(stream, 0x100000)?;
        write_u64(stream, ULL_NEG1)?;
        write_u64(stream, dchunks.len() as u64 + u64::from(ichunk.is_some()))?;
        for dchunk in &dchunks {
            stream.write_all(dchunk).map_err(io)?;
        }
        if let Some(ic) = &ichunk {
            stream.write_all(ic).map_err(io)?;
        }
        let here = stream.stream_position().map_err(io)?;
        write_at(stream, pieces[1], piece1_offset, here - piece1_offset)?;

        // Piece #2: count chunks
        let piece2_offset = stream.stream_position().map_err(io)?;
        stream.write_all(b"IFCM").map_err(io)?;
        write_u32(stream, 1)?;
        write_u32(stream, CCHUNK_SIZE as u32)?;
        write_u32(stream, 0x20000)?;
        write_u64(stream, ULL_NEG1)?;
        write_u64(stream, 1)?;
        let mut cchunk: Vec<u8> = Vec::new();
        let mut last_count: u64 = 0;
        for (i, dcount) in dcounts.iter().enumerate() {
            cchunk.extend_from_slice(&decint(last_count));
            cchunk.extend_from_slice(&decint(u64::from(*dcount)));
            cchunk.extend_from_slice(&decint(i as u64));
            last_count = u64::from(*dcount);
        }
        let rem = CCHUNK_SIZE as i64 - (cchunk.len() as i64 + 50);
        stream.write_all(b"AOLL").map_err(io)?;
        write_u32(stream, rem as u32)?;
        write_u64(stream, 0)?;
        write_u64(stream, ULL_NEG1)?;
        write_u64(stream, ULL_NEG1)?;
        write_u64(stream, 0)?;
        write_u64(stream, 1)?;
        stream.write_all(&cchunk).map_err(io)?;
        stream
            .write_all(&vec![0u8; rem.max(0) as usize])
            .map_err(io)?;
        stream
            .write_all(&(dcounts.len() as u16).to_le_bytes())
            .map_err(io)?;
        let here = stream.stream_position().map_err(io)?;
        write_at(stream, pieces[2], piece2_offset, here - piece2_offset)?;

        // Pieces #3 and #4: GUIDs
        for (index, guid) in [(3usize, PIECE3_GUID), (4, PIECE4_GUID)] {
            let offset = stream.stream_position().map_err(io)?;
            stream.write_all(&packguid(guid)).map_err(io)?;
            let here = stream.stream_position().map_err(io)?;
            write_at(stream, pieces[index], offset, here - offset)?;
        }

        // The actual section content.
        let content_offset = stream.stream_position().map_err(io)?;
        let pos = stream.stream_position().map_err(io)?;
        stream.seek(SeekFrom::Start(conoff_offset)).map_err(io)?;
        stream
            .write_all(&content_offset.to_le_bytes())
            .map_err(io)?;
        stream.seek(SeekFrom::Start(pos)).map_err(io)?;
        let section0 = std::mem::take(&mut self.sections[0]);
        stream.write_all(&section0).map_err(io)?;
        self.sections[0] = section0;
        let total = stream.stream_position().map_err(io)?;
        stream.seek(SeekFrom::Start(filesz_offset)).map_err(io)?;
        stream.write_all(&total.to_le_bytes()).map_err(io)?;
        stream.seek(SeekFrom::Start(total)).map_err(io)?;
        Ok(())
    }
}

/// Bookkeeping `_build_data` hands to the manifest and page-break
/// builders.
struct ItemInfo {
    size: usize,
    page_breaks: Vec<PageBreak>,
}

fn write_u32<W: Write>(stream: &mut W, v: u32) -> Result<()> {
    stream
        .write_all(&v.to_le_bytes())
        .map_err(|e| LitError::msg(format!("Write failed: {e}")))
}

fn write_u64<W: Write>(stream: &mut W, v: u64) -> Result<()> {
    stream
        .write_all(&v.to_le_bytes())
        .map_err(|e| LitError::msg(format!("Write failed: {e}")))
}

/// `LitWriter._writeat` — write two 64-bit values at `pos` and restore
/// the stream position.
fn write_at<W: Write + Seek>(stream: &mut W, pos: u64, a: u64, b: u64) -> Result<()> {
    let io = |e: std::io::Error| LitError::msg(format!("Write failed: {e}"));
    let opos = stream.stream_position().map_err(io)?;
    stream.seek(SeekFrom::Start(pos)).map_err(io)?;
    stream.write_all(&a.to_le_bytes()).map_err(io)?;
    stream.write_all(&b.to_le_bytes()).map_err(io)?;
    stream.seek(SeekFrom::Start(opos)).map_err(io)?;
    Ok(())
}

/// Serialize the book as an OEB 1.0.1 package, the shape `OPF_MAP`
/// tokenises.
///
/// Stands in for `oeb.to_opf1()[OPF_MIME]`, which `_build_meta` feeds to
/// `ReBinary` after stamping on the three `ms--` attributes.
fn to_opf1(oeb: &OEBBook, guid: &str) -> String {
    use crate::oeb::parse_utils::escape_xml;
    let mut out = String::new();
    out.push_str("<package xmlns:dc=\"http://purl.org/dc/elements/1.1/\"");
    out.push_str(" ms--minimum_level=\"0\" ms--attr5=\"1\"");
    out.push_str(&format!(" ms--guid=\"{}\"", escape_xml(guid)));
    if let Some(uid) = &oeb.uid {
        out.push_str(&format!(" unique-identifier=\"{}\"", escape_xml(uid)));
    }
    out.push_str(">\n<metadata>\n<dc-metadata>\n");

    let dc_terms = [
        ("title", "dc:Title"),
        ("creator", "dc:Creator"),
        ("subject", "dc:Subject"),
        ("description", "dc:Description"),
        ("publisher", "dc:Publisher"),
        ("contributor", "dc:Contributor"),
        ("date", "dc:Date"),
        ("type", "dc:Type"),
        ("format", "dc:Format"),
        ("identifier", "dc:Identifier"),
        ("source", "dc:Source"),
        ("language", "dc:Language"),
        ("relation", "dc:Relation"),
        ("coverage", "dc:Coverage"),
        ("rights", "dc:Rights"),
    ];
    for (term, tag) in dc_terms {
        for item in oeb.metadata.get(term) {
            out.push_str(&format!("<{tag}>{}</{tag}>\n", escape_xml(&item.value)));
        }
    }
    out.push_str("</dc-metadata>\n<x-metadata>\n");
    out.push_str("</x-metadata>\n</metadata>\n<manifest>\n");

    let mut ids: Vec<&String> = oeb.manifest.items.keys().collect();
    ids.sort();
    for id in &ids {
        let item = &oeb.manifest.items[*id];
        out.push_str(&format!(
            "<item id=\"{}\" href=\"{}\" media-type=\"{}\" />\n",
            escape_xml(&item.id),
            escape_xml(&urlnormalize(&item.href)),
            escape_xml(&item.media_type)
        ));
    }
    out.push_str("</manifest>\n<spine>\n");
    for spine_item in &oeb.spine.items {
        out.push_str(&format!(
            "<itemref idref=\"{}\" />\n",
            escape_xml(&spine_item.idref)
        ));
    }
    out.push_str("</spine>\n");
    if !oeb.guide.references.is_empty() {
        out.push_str("<guide>\n");
        let mut types: Vec<&String> = oeb.guide.references.keys().collect();
        types.sort();
        for type_ in types {
            let reference = &oeb.guide.references[type_];
            out.push_str(&format!(
                "<reference type=\"{}\" title=\"{}\" href=\"{}\" />\n",
                escape_xml(&reference.type_),
                escape_xml(reference.title.as_deref().unwrap_or("")),
                escape_xml(&urlnormalize(&reference.href))
            ));
        }
        out.push_str("</guide>\n");
    }
    out.push_str("</package>\n");
    out
}

/// `LitWriter._litize_oeb` — add the Microsoft cover guide references.
pub fn litize_oeb(oeb: &mut OEBBook) -> Vec<String> {
    let mut warnings = Vec::new();
    let cover_id = oeb
        .metadata
        .get("cover")
        .first()
        .map(|item| item.value.clone());
    match cover_id.and_then(|id| oeb.manifest.items.get(&id).map(|i| i.href.clone())) {
        Some(href) => {
            for (type_, title) in ALL_MS_COVER_TYPES {
                if oeb.guide.get(type_).is_none() {
                    oeb.guide.add(type_, Some(title.to_string()), &href);
                }
            }
        }
        None => warnings.push("No suitable cover image found.".to_string()),
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::DirContainer;
    use crate::oeb::stylizer::TagStylizer;
    use std::io::Cursor;

    #[test]
    fn decint_is_base128_big_endian() {
        assert_eq!(decint(0), vec![0x00]);
        assert_eq!(decint(127), vec![0x7f]);
        assert_eq!(decint(128), vec![0x81, 0x00]);
        assert_eq!(decint(16383), vec![0xff, 0x7f]);
    }

    #[test]
    fn decint_round_trips_through_the_readers_encint() {
        // The two are exact inverses; that is what the directory
        // depends on.
        for value in [0u64, 1, 127, 128, 300, 65535, 1 << 20, u32::MAX as u64] {
            let encoded = decint(value);
            let mut acc: u64 = 0;
            for (i, b) in encoded.iter().enumerate() {
                acc = (acc << 7) | u64::from(b & 0x7f);
                assert_eq!(
                    b & 0x80 != 0,
                    i + 1 < encoded.len(),
                    "continuation bit for {value}"
                );
            }
            assert_eq!(acc, value);
        }
    }

    #[test]
    fn packguid_is_the_inverse_of_the_readers_msguid() {
        let packed = packguid(LZXCOMPRESS_GUID);
        assert_eq!(
            packed,
            [
                0xC6, 0x07, 0x90, 0x0A, 0x76, 0x40, 0xD3, 0x11, 0x87, 0x89, 0x00, 0x00, 0xF8, 0x10,
                0x57, 0x54
            ]
        );
    }

    #[test]
    fn inverting_a_map_lets_the_last_duplicate_win() {
        let map = opf_write_map();
        // OPF ATTRS has 'href' at both 0x0001 and 0x0007.
        assert_eq!(map.tattrs[0].get("href"), Some(&0x0007));
        assert_eq!(map.tags.get("package"), Some(&1));
        assert_eq!(map.tags.get("item"), Some(&17));
    }

    #[test]
    fn html_tag_codes_round_trip_against_the_reader_tables() {
        let map = html_write_map();
        for (code, name) in HTML_MAP.tags.iter().enumerate() {
            if let Some(name) = name {
                // Names appear once in the HTML table, so inversion is
                // lossless there.
                assert_eq!(map.tags.get(name), Some(&(code as u32)), "{name}");
            }
        }
    }

    #[test]
    fn collapse_squashes_runs_of_whitespace() {
        assert_eq!(collapse("a  \t\n b"), "a b");
        assert_eq!(collapse("  "), " ");
        assert_eq!(collapse("ab"), "ab");
    }

    #[test]
    fn abshref_resolves_against_the_documents_directory() {
        assert_eq!(abshref("text/ch1.htm", "ch2.htm"), "text/ch2.htm");
        assert_eq!(abshref("text/ch1.htm", "../img/a.png"), "img/a.png");
        assert_eq!(abshref("ch1.htm", "a.png"), "a.png");
        assert_eq!(
            abshref("ch1.htm", "http://example.com/x"),
            "http://example.com/x"
        );
    }

    fn tokenise(xml: &str, hrefs: &HashMap<String, String>) -> ReBinary {
        let doc = Document::parse(xml).expect("valid XML");
        let styles = TagStylizer;
        let lit = ProviderStyles::new(&styles);
        ReBinary::new(
            doc.root_element(),
            Some("text/ch1.htm"),
            hrefs,
            html_write_map(),
            true,
            Some(&lit),
        )
    }

    #[test]
    fn rebinary_emits_the_expected_token_stream() {
        let hrefs = HashMap::new();
        let rebin = tokenise("<html><body><p>Hi</p></body></html>", &hrefs);
        // Every element opens with NUL, flags, tag code, then NUL to
        // end the attribute list.
        assert_eq!(rebin.content[0], 0);
        let html_code = HTML_MAP
            .tags
            .iter()
            .position(|t| *t == Some("html"))
            .expect("html") as u8;
        assert_eq!(rebin.content[2], html_code);
        assert!(rebin.content.windows(2).any(|w| w == b"Hi"));
    }

    #[test]
    fn rebinary_records_anchors() {
        let hrefs = HashMap::new();
        let rebin = tokenise(
            "<html><body><p id=\"top\">Hi</p><a name=\"x\">y</a></body></html>",
            &hrefs,
        );
        let names: Vec<&str> = rebin.anchors.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["top", "x"]);
        assert!(!rebin.ahc.is_empty());
        assert_eq!(rebin.aht, vec![0, 0, 0, 0]);
    }

    #[test]
    fn rebinary_warns_about_more_than_six_anchors() {
        let mut body = String::from("<html><body>");
        for i in 0..7 {
            body.push_str(&format!("<p id=\"a{i}\">x</p>"));
        }
        body.push_str("</body></html>");
        let hrefs = HashMap::new();
        let rebin = tokenise(&body, &hrefs);
        assert_eq!(rebin.anchors.len(), 7);
        assert!(
            rebin.warnings.iter().any(|w| w.contains("six anchors")),
            "{:?}",
            rebin.warnings
        );
    }

    #[test]
    fn rebinary_rewrites_internal_links_to_manifest_ids() {
        let mut hrefs = HashMap::new();
        hrefs.insert("text/ch2.htm".to_string(), "ch2".to_string());
        let rebin = tokenise(
            "<html><body><a href=\"ch2.htm#part\">next</a><a href=\"http://x/\">out</a></body></html>",
            &hrefs,
        );
        let content = String::from_utf8_lossy(&rebin.content);
        assert!(content.contains("\u{2}ch2#part"), "{content:?}");
        assert!(content.contains("\u{3}http://x/"), "{content:?}");
    }

    #[test]
    fn rebinary_marks_page_breaks_from_style() {
        let hrefs = HashMap::new();
        let rebin = tokenise(
            "<html><body><p style=\"page-break-before: always\">x</p></body></html>",
            &hrefs,
        );
        assert_eq!(rebin.page_breaks.len(), 1);
    }

    #[test]
    fn rebinary_produces_no_anchor_tables_for_the_opf() {
        let doc = Document::parse("<package><metadata /></package>").expect("valid XML");
        let hrefs = HashMap::new();
        let rebin = ReBinary::new(
            doc.root_element(),
            None,
            &hrefs,
            opf_write_map(),
            false,
            None,
        );
        assert!(rebin.ahc.is_empty());
        assert!(rebin.aht.is_empty());
        assert!(!rebin.content.is_empty());
    }

    /// A minimal book on disk, for the end-to-end writer tests.
    fn sample_book(dir: &std::path::Path) -> OEBBook {
        std::fs::write(
            dir.join("ch1.htm"),
            "<html><body><p>Chapter one.</p></body></html>",
        )
        .expect("write");
        std::fs::write(dir.join("style.css"), "p { margin: 0 }").expect("write");
        let container = Box::new(DirContainer::new(dir));
        let mut book = OEBBook::new(container);
        book.manifest.add("ch1", "ch1.htm", "application/xhtml+xml");
        book.manifest.add("css", "style.css", "text/css");
        book.spine.add("ch1", true);
        book.metadata.add("title", "Test Book");
        book.metadata.add("creator", "A. Author");
        book
    }

    #[test]
    fn writes_a_file_with_the_lit_magic_and_a_plausible_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let book = sample_book(dir.path());
        let styles = TagStylizer;
        let lit = ProviderStyles::new(&styles);
        let mut out = Cursor::new(Vec::new());
        LitWriter::new()
            .write(&book, Some(&lit), &mut out)
            .expect("writes");
        let data = out.into_inner();
        assert_eq!(&data[..8], LIT_MAGIC);
        assert_eq!(
            u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            1
        );
        // The recorded file size must match what was actually written.
        let hdr_len = i32::from_le_bytes([data[12], data[13], data[14], data[15]]);
        assert_eq!(hdr_len, PRIMARY_SIZE as i32);
        assert!(data.len() > 4096, "got {} bytes", data.len());
    }

    #[test]
    fn excluded_media_types_are_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut book = sample_book(dir.path());
        std::fs::write(dir.path().join("movie.mp4"), b"not really a movie").expect("write");
        book.manifest.add("mov", "movie.mp4", "video/mp4");
        let styles = TagStylizer;
        let lit = ProviderStyles::new(&styles);
        let mut out = Cursor::new(Vec::new());
        let mut writer = LitWriter::new();
        writer.write(&book, Some(&lit), &mut out).expect("writes");
        assert!(
            writer
                .warnings
                .iter()
                .any(|w| w.contains("movie.mp4") && w.contains("excluded")),
            "{:?}",
            writer.warnings
        );
    }

    #[test]
    fn litize_oeb_warns_when_there_is_no_cover() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut book = sample_book(dir.path());
        let warnings = litize_oeb(&mut book);
        assert_eq!(warnings, vec!["No suitable cover image found.".to_string()]);
        assert!(book.guide.get(MS_COVER_TYPE).is_none());
    }

    #[test]
    fn litize_oeb_adds_the_microsoft_cover_references() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut book = sample_book(dir.path());
        std::fs::write(dir.path().join("cover.jpg"), b"jpegdata").expect("write");
        book.manifest.add("cover-img", "cover.jpg", "image/jpeg");
        book.metadata.add("cover", "cover-img");
        let warnings = litize_oeb(&mut book);
        assert!(warnings.is_empty(), "{warnings:?}");
        for (type_, _) in ALL_MS_COVER_TYPES {
            assert_eq!(
                book.guide.get(type_).map(|r| r.href.as_str()),
                Some("cover.jpg"),
                "{type_}"
            );
        }
    }
}
