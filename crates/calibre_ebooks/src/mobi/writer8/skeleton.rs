//! Split each spine item's XHTML into a "skeleton" (the outer structural
//! markup, under [`CHUNK_SIZE`] bytes) plus a sequence of "chunk"
//! fragments that get spliced back into the skeleton at fixed byte
//! offsets on read. This is KF8's file-reassembly model: the reader
//! ([`crate::mobi::mobi8::Mobi8Reader::build_parts`]) walks the `SKEL`
//! table, and for each skeleton file splices its `CHUNK` table entries
//! back in at their recorded `insert_pos`.
//!
//! Port of `calibre.ebooks.mobi.writer8.skeleton`.
//!
//! # DOM reuse
//!
//! Python walks an lxml tree with `.text`/`.tail` properties on each
//! element. [`crate::mobi::dom::Dom`] instead represents text as ordinary
//! interleaved `Text` children (the same model `writer2::serializer`
//! already uses) -- a `Text` child that comes before any `Element`
//! sibling is exactly lxml's `tag.text`; a `Text` child that comes right
//! after an `Element` sibling is exactly that element's `tag.tail`. The
//! chunking walk below (`step_into_tag`) is written against that model
//! directly rather than reintroducing `.text`/`.tail`.
//!
//! # Byte-for-byte fidelity
//!
//! This does not attempt to reproduce lxml/kindlegen's exact
//! serialization bytes (attribute quoting/ordering, self-closing-tag
//! rewriting, XML declarations). What matters for correctness is
//! *internal* consistency: the same [`crate::mobi::dom::Dom::serialize`]/
//! [`crate::mobi::dom::Dom::serialize_open_tag`] routines are used both to
//! produce the skeleton bytes and to measure the metrics
//! ([`calculate_metrics`]) that place chunks back into it, so
//! [`Skeleton::rebuild`] reconstructs a byte-identical document regardless
//! of the exact serialization format chosen -- verified by the
//! [`crate::mobi::mobi8::Mobi8Reader`] round-trip test in
//! `mobi_writer8_test.rs`.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use lazy_static::lazy_static;
use regex::bytes::Regex as BytesRegex;

use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::mobi::utils::to_base;
use crate::mobi::writer8::index::{ChunkTableEntry, SkelTableEntry};

/// `CHUNK_SIZE` in `skeleton.py`: the target/maximum size of a single
/// chunk fragment.
pub const CHUNK_SIZE: usize = 8192;

/// `to_href` in `skeleton.py`: references in links are stored as 10-digit
/// base-32 numbers.
pub fn to_href(n: i64) -> String {
    to_base(n, 32, Some(10))
}

/// `aid_able_tags` in `skeleton.py`: tags kindlegen (and this port) adds
/// an `aid` attribute to.
pub const AID_ABLE_TAGS: &[&str] = &[
    "a",
    "abbr",
    "address",
    "article",
    "aside",
    "audio",
    "b",
    "bdo",
    "blockquote",
    "body",
    "button",
    "cite",
    "code",
    "dd",
    "del",
    "details",
    "dfn",
    "div",
    "dl",
    "dt",
    "em",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "i",
    "ins",
    "kbd",
    "label",
    "legend",
    "li",
    "map",
    "mark",
    "meter",
    "nav",
    "ol",
    "output",
    "p",
    "pre",
    "progress",
    "q",
    "rp",
    "rt",
    "samp",
    "section",
    "select",
    "small",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "textarea",
    "time",
    "ul",
    "var",
    "video",
];

pub fn is_aid_able(tag: &str) -> bool {
    AID_ABLE_TAGS.contains(&tag)
}

/// `aid -> (chunk sequence number, offset within chunk, absolute offset
/// into the flat flow text)`. Port of `Chunker.aid_offset_map`'s value
/// shape.
pub type AidOffsetMap = HashMap<String, (u64, u64, u64)>;

/// Escape `&`, `<`, `>` -- the default set `xml.sax.saxutils.escape`
/// handles (attribute-quote escaping is not needed here: chunked text
/// never carries a literal `"`/`'` requiring escaping in this context,
/// matching Python's use of the *default* `escape()`, not the
/// quote-aware variant).
fn xml_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Split `s` into `(head, rest)` where `head.len() <= limit` and `head`
/// ends on a UTF-8 character boundary. Port of `split_multibyte_text`.
fn split_utf8_chunk(s: &str, limit: usize) -> (String, String) {
    if s.len() <= limit {
        return (s.to_string(), String::new());
    }
    let mut idx = limit;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    (s[..idx].to_string(), s[idx..].to_string())
}

/// A single fragment extracted out of a skeleton. Port of `Chunk`.
#[derive(Debug, Clone)]
pub struct Chunk {
    pub raw: Vec<u8>,
    pub starts_tags: Vec<String>,
    pub ends_tags: Vec<String>,
    pub insert_pos: Option<usize>,
    pub is_first_chunk: bool,
    /// `"{P|S}-//*[@aid='{aid}']"` -- the KF8 reader
    /// (`Mobi8Reader::build_parts`) strips this back down to a bare
    /// `aid` via `idtext[12:-2]`.
    pub selector: String,
}

impl Chunk {
    fn new(raw: Vec<u8>, kind: char, aid: &str) -> Self {
        Chunk {
            raw,
            starts_tags: Vec::new(),
            ends_tags: Vec::new(),
            insert_pos: None,
            is_first_chunk: false,
            selector: format!("{kind}-//*[@aid='{aid}']"),
        }
    }

    /// Port of `Chunk.merge`.
    fn merge(&mut self, other: Chunk) {
        self.raw.extend_from_slice(&other.raw);
        self.ends_tags = other.ends_tags;
    }
}

/// Byte length of an aid-bearing element's open tag (+ leading text) and
/// close tag (+ trailing whitespace-only tail still attached), used to
/// place chunks back into the skeleton at the right offset. Port of the
/// `Metric` namedtuple used by `Skeleton.calculate_metrics`.
type Metrics = HashMap<String, (usize, usize)>;

/// Port of `Skeleton.calculate_metrics`, adapted to `Dom`'s
/// interleaved-text-child model (see the module doc).
fn calculate_metrics(dom: &Dom, root: NodeId) -> Metrics {
    let mut metrics = Metrics::new();
    for el in dom.preorder_elements(root) {
        let Some(aid) = dom.node(el).attrs.get("aid").cloned() else {
            continue;
        };
        let open = dom.serialize_open_tag(el);
        let leading_text_len = dom
            .children(el)
            .first()
            .and_then(|&c| match &dom.node(c).kind {
                NodeKind::Text(t) => Some(t.len()),
                _ => None,
            })
            .unwrap_or(0);
        let start_len = open.len() + leading_text_len;

        let close_len = dom.tag(el).map(|t| t.len() + 3).unwrap_or(0); // "</" + tag + ">"
        let tail_len = dom
            .next_sibling(el)
            .and_then(|nid| match &dom.node(nid).kind {
                NodeKind::Text(t) => Some(t.len()),
                _ => None,
            })
            .unwrap_or(0);
        let end_len = close_len + tail_len;

        metrics.insert(aid, (start_len, end_len));
    }
    metrics
}

/// Port of `Skeleton.calculate_insert_positions`.
fn calculate_insert_positions(chunks: &mut [Chunk], body_offset: usize, metrics: &Metrics) {
    let mut pos = body_offset;
    for chunk in chunks.iter_mut() {
        for aid in &chunk.starts_tags {
            pos += metrics.get(aid).map(|m| m.0).unwrap_or(0);
        }
        chunk.insert_pos = Some(pos);
        pos += chunk.raw.len();
        for aid in &chunk.ends_tags {
            pos += metrics.get(aid).map(|m| m.1).unwrap_or(0);
        }
    }
}

/// One spine item's skeleton + its extracted chunks. Port of `Skeleton`.
#[derive(Debug)]
pub struct Skeleton {
    pub file_number: usize,
    pub item_href: String,
    pub chunks: Vec<Chunk>,
    pub skeleton: Vec<u8>,
    pub body_offset: usize,
    /// Filled in by [`Chunker::new`] once every skeleton's size is known
    /// (`Chunker.create_tables` in Python).
    pub start_pos: usize,
}

impl Skeleton {
    fn new(
        file_number: usize,
        item_href: String,
        dom: &Dom,
        root: NodeId,
        mut chunks: Vec<Chunk>,
    ) -> Result<Self> {
        let skeleton = dom.serialize(root).into_bytes();
        let body_offset = find_subsequence(&skeleton, b"<body")
            .with_context(|| format!("no <body> found while rendering skeleton for {item_href}"))?;
        let metrics = calculate_metrics(dom, root);
        calculate_insert_positions(&mut chunks, body_offset, &metrics);
        Ok(Skeleton {
            file_number,
            item_href,
            chunks,
            skeleton,
            body_offset,
            start_pos: 0,
        })
    }

    /// Port of `Skeleton.__len__`: the skeleton's total contribution to
    /// the flat flow-text stream (its own bytes plus every chunk's).
    pub fn total_len(&self) -> usize {
        self.skeleton.len() + self.chunks.iter().map(|c| c.raw.len()).sum::<usize>()
    }

    /// Port of `Skeleton.rebuild`: splice every chunk back into the
    /// skeleton at its recorded `insert_pos`, producing valid markup
    /// (used only to locate `aid`/`cid` attributes by byte offset in
    /// [`set_internal_links`], not part of the final flow text -- see
    /// [`Skeleton::raw_text`] for that).
    fn rebuild(&self) -> Vec<u8> {
        let mut ans = self.skeleton.clone();
        for chunk in &self.chunks {
            let i = chunk.insert_pos.unwrap_or(ans.len()).min(ans.len());
            let mut next = Vec::with_capacity(ans.len() + chunk.raw.len());
            next.extend_from_slice(&ans[..i]);
            next.extend_from_slice(&chunk.raw);
            next.extend_from_slice(&ans[i..]);
            ans = next;
        }
        ans
    }

    /// Port of `Skeleton.raw_text`: the skeleton's actual contribution to
    /// the final flow-text stream (flat concatenation, *not* spliced --
    /// chunk placement is recovered on read via the `SKEL`/`CHUNK` INDX
    /// tables, not by markup structure).
    fn raw_text(&self) -> Vec<u8> {
        let mut out = self.skeleton.clone();
        for c in &self.chunks {
            out.extend_from_slice(&c.raw);
        }
        out
    }
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Strips `Comment`/processing-instruction nodes (`dom::Dom::parse`
/// collapses both into `NodeKind::Comment`) and any `xmlns`/`xmlns:*`
/// attribute tree-wide. Narrow stand-in for `Chunker.remove_namespaces`:
/// `Dom`'s html5ever-backed parser already stores every tag/attribute
/// name as a bare local name (no namespace prefix ever survives parsing,
/// see `dom.rs`'s module doc), so the only remaining namespace-flavored
/// artifact an HTML(5)-mode parse can leave behind is a literal `xmlns`
/// attribute -- stripped here for the same reason Python's fresh-element
/// reconstruction never copies namespace declarations across.
fn strip_namespace_artifacts(dom: &mut Dom) {
    let comments: Vec<NodeId> = dom
        .nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| matches!(n.kind, NodeKind::Comment(_)))
        .map(|(i, _)| i)
        .collect();
    for c in comments {
        dom.detach(c);
    }
    for node in dom.nodes.iter_mut() {
        if matches!(node.kind, NodeKind::Element(_)) {
            node.attrs
                .retain(|k, _| k != "xmlns" && !k.starts_with("xmlns:"));
        }
    }
}

/// Threads the single shared `chunk_selector` state
/// (`Chunker.chunk_selector` in Python) through the recursive
/// `step_into_tag` walk for one spine item.
struct ItemChunker<'a> {
    dom: &'a mut Dom,
    chunk_selector: (char, String),
    warnings: Vec<String>,
}

impl<'a> ItemChunker<'a> {
    fn chunk_up_text(&self, text: &str) -> Vec<Chunk> {
        let escaped = xml_escape_text(text);
        let mut out = Vec::new();
        let (first, mut rest) = split_utf8_chunk(&escaped, CHUNK_SIZE);
        out.push(Chunk::new(
            first.into_bytes(),
            self.chunk_selector.0,
            &self.chunk_selector.1,
        ));
        while !rest.is_empty() {
            let (s, r) = split_utf8_chunk(&rest, CHUNK_SIZE);
            let wrapped = format!("<span class=\"AmznBigTextBlock\">{s}</span>");
            out.push(Chunk::new(
                wrapped.into_bytes(),
                self.chunk_selector.0,
                &self.chunk_selector.1,
            ));
            rest = r;
        }
        out
    }

    /// Port of `Chunker.step_into_tag`.
    fn step_into_tag(&mut self, tag_id: NodeId, chunks: &mut Vec<Chunk>) -> Result<()> {
        let aid = self
            .dom
            .node(tag_id)
            .attrs
            .get("aid")
            .cloned()
            .context("stepped into a tag with no aid attribute")?;
        self.chunk_selector = ('P', aid.clone());
        let first_chunk_idx = chunks.len();

        let children = self.dom.children(tag_id);
        let mut idx = 0usize;
        while idx < children.len() {
            let cid = children[idx];
            let kind = self.dom.node(cid).kind.clone();
            match kind {
                NodeKind::Text(t) => {
                    // Leading text of this tag, or the tail of a
                    // just-recursed (kept) child: leave pure whitespace
                    // in the skeleton, else chunk it.
                    if !t.trim().is_empty() {
                        let subchunks = self.chunk_up_text(&t);
                        chunks.extend(subchunks);
                        self.dom.detach(cid);
                    }
                    idx += 1;
                }
                NodeKind::Element(ref tag_name) => {
                    let raw = self.dom.serialize(cid);
                    let has_aid = self.dom.node(cid).attrs.contains_key("aid");
                    if raw.len() > CHUNK_SIZE && has_aid {
                        self.step_into_tag(cid, chunks)?;
                        idx += 1;
                    } else {
                        if raw.len() > CHUNK_SIZE {
                            self.warnings.push(format!(
                                "Tag {tag_name} has no aid and a too large chunk size. Adding anyway."
                            ));
                        }
                        chunks.push(Chunk::new(
                            raw.into_bytes(),
                            self.chunk_selector.0,
                            &self.chunk_selector.1,
                        ));
                        self.dom.detach(cid);
                        // The child is being removed wholesale, so any
                        // tail text (even pure whitespace) must be
                        // pulled into a chunk or it's lost.
                        if idx + 1 < children.len() {
                            if let NodeKind::Text(t2) =
                                self.dom.node(children[idx + 1]).kind.clone()
                            {
                                if !t2.is_empty() {
                                    chunks.extend(self.chunk_up_text(&t2));
                                }
                                self.dom.detach(children[idx + 1]);
                                idx += 1;
                            }
                        }
                        idx += 1;
                    }
                }
                _ => idx += 1,
            }
        }

        if chunks.len() <= first_chunk_idx && !chunks.is_empty() {
            bail!("Stepped into a tag that generated no chunks.");
        }
        if !chunks.is_empty() && chunks.len() > first_chunk_idx {
            chunks[first_chunk_idx].starts_tags.push(aid.clone());
            let last = chunks.len() - 1;
            chunks[last].ends_tags.push(aid.clone());
            chunks[first_chunk_idx].is_first_chunk = true;
        }
        self.chunk_selector = ('S', aid);
        Ok(())
    }
}

/// Port of `Chunker.merge_small_chunks`.
fn merge_small_chunks(chunks: Vec<Chunk>) -> Vec<Chunk> {
    let mut ans: Vec<Chunk> = Vec::new();
    for chunk in chunks {
        let merge_into_prev = match ans.last() {
            Some(prev) => {
                chunk.starts_tags.is_empty()
                    && chunk.raw.len() + prev.raw.len() <= CHUNK_SIZE
                    && prev.ends_tags.is_empty()
            }
            None => false,
        };
        if merge_into_prev {
            ans.last_mut().unwrap().merge(chunk);
        } else {
            ans.push(chunk);
        }
    }
    ans
}

/// Splits every `(href, Dom)` spine item into a [`Skeleton`], builds the
/// `SKEL`/`CHUNK` tables, and resolves internal-link placeholders into
/// `kindle:pos:fid:XXXX:off:YYYYYYYYYY` locations. Port of `Chunker`.
pub struct Chunker {
    pub skel_table: Vec<SkelTableEntry>,
    pub chunk_table: Vec<ChunkTableEntry>,
    /// Port of `self.aid_offset_map`.
    pub aid_offset_map: AidOffsetMap,
    /// The final `flows[0]` content.
    pub text: Vec<u8>,
    pub warnings: Vec<String>,
}

impl Chunker {
    /// `items`: `(href, dom)` pairs in spine order, each `dom` already
    /// mutated by every earlier `KF8Writer` pass (cleanup, resource-link
    /// rewriting, CSS/SVG flow extraction, internal-link placeholder
    /// insertion, `aid` attribute insertion). `placeholder_map`: the
    /// `kindle:pos:fid:0000:off:XXXXXXXXXX` placeholder string ->
    /// resolved target `aid` (`None` for a placeholder whose target
    /// couldn't be resolved to any `aid` -- left unresolved rather than
    /// erroring, a deliberate robustness improvement over Python, which
    /// has no guard here at all and would raise `KeyError` deep inside
    /// `to_placeholder`).
    pub fn new(
        items: Vec<(String, Dom)>,
        placeholder_map: &HashMap<String, Option<String>>,
    ) -> Result<Self> {
        let mut skeletons = Vec::new();
        let mut warnings = Vec::new();

        for (i, (href, mut dom)) in items.into_iter().enumerate() {
            strip_namespace_artifacts(&mut dom);
            let html_id = dom
                .find_first_tag_global("html")
                .with_context(|| format!("item {href} has no <html> root"))?;
            let body = dom
                .find_first_tag_global("body")
                .with_context(|| format!("item {href} has no <body>"))?;

            let mut chunks = Vec::new();
            {
                let mut item_chunker = ItemChunker {
                    dom: &mut dom,
                    chunk_selector: ('P', String::new()),
                    warnings: Vec::new(),
                };
                item_chunker.step_into_tag(body, &mut chunks)?;
                warnings.extend(item_chunker.warnings);
            }
            let chunks = merge_small_chunks(chunks);
            let skeleton = Skeleton::new(i, href, &dom, html_id, chunks)?;
            skeletons.push(skeleton);
        }

        // Create the SKEL and CHUNK tables (`Chunker.create_tables`).
        let mut sp = 0usize;
        for s in skeletons.iter_mut() {
            s.start_pos = sp;
            sp += s.total_len();
        }
        let skel_table: Vec<SkelTableEntry> = skeletons
            .iter()
            .map(|s| SkelTableEntry {
                file_number: s.file_number,
                name: format!("SKEL{:010}", s.file_number),
                chunk_count: s.chunks.len(),
                start_pos: s.start_pos,
                length: s.skeleton.len(),
            })
            .collect();

        let mut chunk_table = Vec::new();
        let mut num = 0usize;
        for skel in &skeletons {
            let mut cp = 0usize;
            for chunk in &skel.chunks {
                chunk_table.push(ChunkTableEntry {
                    insert_pos: chunk.insert_pos.unwrap_or(0) + skel.start_pos,
                    selector: chunk.selector.clone(),
                    file_number: skel.file_number,
                    sequence_number: num,
                    start_pos: cp,
                    length: chunk.raw.len(),
                });
                cp += chunk.raw.len();
                num += 1;
            }
        }

        let text: Vec<u8> = skeletons.iter().flat_map(Skeleton::raw_text).collect();
        let rebuilt: Vec<u8> = skeletons.iter().flat_map(Skeleton::rebuild).collect();

        let (text, aid_offset_map, link_warnings) =
            set_internal_links(&text, &rebuilt, &chunk_table, placeholder_map)?;
        warnings.extend(link_warnings);

        Ok(Chunker {
            skel_table,
            chunk_table,
            aid_offset_map,
            text,
            warnings,
        })
    }
}

/// Port of `Chunker.set_internal_links`. Scans `rebuilt_text` (valid
/// spliced markup) for every `[ac]id="..."` attribute to build a map of
/// `aid -> (chunk sequence number, offset within chunk, absolute
/// offset)`, then substitutes every `kindle:pos:fid:0000:off:XXXXXXXXXX`
/// placeholder found in `text` (the *flat*, unspliced flow content) with
/// the real `kindle:pos:fid:<seq>:off:<offset>` location for its target
/// `aid`, per `placeholder_map`.
fn set_internal_links(
    text: &[u8],
    rebuilt_text: &[u8],
    chunk_table: &[ChunkTableEntry],
    placeholder_map: &HashMap<String, Option<String>>,
) -> Result<(Vec<u8>, AidOffsetMap, Vec<String>)> {
    lazy_static! {
        static ref AID_ATTR_RE: BytesRegex =
            BytesRegex::new(r#"<[^>]+? [ac]id=['"]([cA-Z0-9]+)['"]"#).unwrap();
        static ref LINK_RE: BytesRegex =
            BytesRegex::new(r#"<[^>]+(kindle:pos:fid:0000:off:[0-9A-Za-z]{10})"#).unwrap();
    }

    let mut warnings = Vec::new();
    let mut aid_map: AidOffsetMap = HashMap::new();
    for m in AID_ATTR_RE.captures_iter(rebuilt_text) {
        let whole = m.get(0).unwrap();
        let offset = whole.start();
        let mut pos_fid: Option<(u64, u64, u64)> = None;
        for chunk in chunk_table {
            if chunk.insert_pos <= offset && offset < chunk.insert_pos + chunk.length {
                pos_fid = Some((
                    chunk.sequence_number as u64,
                    (offset - chunk.insert_pos) as u64,
                    offset as u64,
                ));
                break;
            }
            if chunk.insert_pos > offset {
                pos_fid = Some((chunk.sequence_number as u64, 0, offset as u64));
                break;
            }
        }
        if pos_fid.is_none() {
            // aids very close to the end of the text
            // (https://bugs.launchpad.net/bugs/1011330).
            if let Some(last) = chunk_table.last() {
                pos_fid = Some((
                    last.sequence_number as u64,
                    offset.saturating_sub(last.insert_pos) as u64,
                    offset as u64,
                ));
            }
        }
        let Some(pf) = pos_fid else {
            let aid = String::from_utf8_lossy(m.get(1).unwrap().as_bytes()).into_owned();
            warnings.push(format!("Could not find chunk for aid: {aid:?}"));
            continue;
        };
        let aid = String::from_utf8_lossy(m.get(1).unwrap().as_bytes()).into_owned();
        aid_map.insert(aid, pf);
    }

    let mut resolved_placeholders: HashMap<String, (u64, u64)> = HashMap::new();
    for (placeholder, aid_opt) in placeholder_map {
        let Some(aid) = aid_opt else { continue };
        let Some(&(seq, off, _)) = aid_map.get(aid) else {
            continue;
        };
        resolved_placeholders.insert(placeholder.clone(), (seq, off));
    }

    let result = LINK_RE.replace_all(text, |caps: &regex::bytes::Captures| -> Vec<u8> {
        let whole = caps.get(0).unwrap();
        let group = caps.get(1).unwrap();
        let group_text = String::from_utf8_lossy(group.as_bytes()).into_owned();
        match resolved_placeholders.get(&group_text) {
            Some(&(seq, off)) => {
                let prefix = &text[whole.start()..group.start()];
                let mut out = prefix.to_vec();
                out.extend_from_slice(b"kindle:pos:fid:");
                out.extend_from_slice(to_base(seq as i64, 32, Some(4)).as_bytes());
                out.extend_from_slice(b":off:");
                out.extend_from_slice(to_href(off as i64).as_bytes());
                out
            }
            None => whole.as_bytes().to_vec(),
        }
    });

    Ok((result.into_owned(), aid_map, warnings))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dom_from(html: &str) -> Dom {
        Dom::parse(html)
    }

    fn set_aid(dom: &mut Dom, tag: &str, aid: &str) {
        let id = dom.find_first_tag_global(tag).unwrap();
        dom.node_mut(id)
            .attrs
            .insert("aid".to_string(), aid.to_string());
    }

    #[test]
    fn chunks_a_small_document_into_one_fragment_per_paragraph() {
        let mut dom = dom_from("<html><body><p>hello</p><p>world</p></body></html>");
        set_aid(&mut dom, "body", "0");
        let items = vec![("c1.html".to_string(), dom)];
        let chunker = Chunker::new(items, &HashMap::new()).unwrap();
        assert_eq!(chunker.skel_table.len(), 1);
        assert!(!chunker.chunk_table.is_empty());
        let text = String::from_utf8_lossy(&chunker.text);
        assert!(text.contains("hello"));
        assert!(text.contains("world"));
    }

    #[test]
    fn merge_small_chunks_combines_adjacent_small_fragments() {
        let a = Chunk::new(vec![b'a'; 10], 'P', "x");
        let b = Chunk::new(vec![b'b'; 10], 'P', "x");
        let merged = merge_small_chunks(vec![a, b]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].raw.len(), 20);
    }

    #[test]
    fn split_utf8_chunk_never_splits_a_multibyte_character() {
        let s = "a".repeat(9) + "\u{1F600}"; // 9 ascii + 4-byte emoji = 13 bytes
        let (head, tail) = split_utf8_chunk(&s, 10);
        assert!(s.is_char_boundary(head.len()));
        assert_eq!(head.len() + tail.len(), s.len());
    }
}
