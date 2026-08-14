//! `KF8Writer`: the top-level orchestrator that turns an `OEBBook` into
//! the pieces `KF8Book` assembles into a KF8 payload -- markup cleanup,
//! resource-link/CSS/SVG rewriting, `aid` insertion, skeleton/chunk
//! splitting, text/FDST/index record creation.
//!
//! Port of `calibre.ebooks.mobi.writer8.main.KF8Writer`.
//!
//! # Scope: CSS parsing
//!
//! Python's `extract_css_into_flows`/`replace_resource_links` use the
//! `css_parser` library to parse, condense, and rewrite `url()`/
//! `@import` references inside stylesheets. No CSS parser exists in
//! this workspace (see `docs/AGENT_PORTING_GUIDE.md`'s issue #35 scope
//! note). This port does the parts of those two passes that don't need
//! real CSS parsing for real: `<link rel="stylesheet">`/`<style>` tags
//! are moved into `kindle:flow:` flow records as opaque text blobs
//! (structurally identical to what Python produces), and
//! `<img>`/`<image>` `src`/`href` attributes are rewritten to
//! `kindle:embed:` pointers via plain DOM attribute edits. What's
//! *not* done: rewriting `url(...)` references **inside** CSS text
//! (`replaceUrls`) and `@import` rules to point at `kindle:flow:`
//! (`fix_import_rules`) -- both require walking a parsed stylesheet AST.
//! A stylesheet with no `@import`/`url()` references (the common case)
//! round-trips exactly as Python would produce it; one that has them
//! keeps its original (book-relative) URLs instead of Kindle pointers.
//! `condense_sheet` (a size optimization, not a correctness step) is
//! skipped outright.
//!
//! # Scope: non-spine documents
//!
//! Python's `cleanup_markup`/`replace_resource_links`/
//! `replace_internal_links_with_placeholders`/`insert_aid_attributes`
//! iterate `oeb.spine`; `replace_resource_links`/`extract_css_into_flows`/
//! `extract_svg_into_flows` also sweep the *whole* `oeb.manifest` for
//! non-spine stylesheet/SVG-image resources (a stylesheet or `.svg`
//! file that exists in the manifest but isn't directly in the spine
//! still needs to become a flow record if anything references it). This
//! port keeps that split: markup passes work over spine items (parsed
//! once into a [`Dom`] per item), while stylesheet/SVG-image collection
//! sweeps the manifest. What's narrowed: Python's `replace_resource_links`
//! also rewrites image references inside any non-spine *XHTML* manifest
//! item (e.g. a fragment referenced only via `<object>`); this port only
//! rewrites spine items, since [`crate::mobi::dom::Dom`] instances are
//! only built for spine content here.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::compression::palmdoc::compress;
use crate::mobi::dom::{Dom, NodeId, NodeKind};
use crate::mobi::utils::{create_text_record, to_base};
use crate::mobi::writer2::resources::{ResourceOpts, Resources};
use crate::mobi::writer2::serializer::{abshref, is_guide_ref_start, urldefrag, urlnormalize};
use crate::mobi::writer8::index::{
    chunk_index, guide_index, ncx_index, skel_index, ChunkTableEntry, GuideTableEntry,
    NcxTableEntry,
};
use crate::mobi::writer8::mobi::{KF8Book, KF8BuildInputs, Kf8Opts};
use crate::mobi::writer8::skeleton::{is_aid_able, to_href, Chunker};
use crate::mobi::writer8::tbs::apply_trailing_byte_sequences;
use crate::mobi::writer8::toc::{TocAdder, TocOpts};
use crate::mobi::MobiLog;
use crate::oeb::book::OEBBook;
use crate::oeb::constants::{OEB_STYLES, SVG_MIME};
use crate::oeb::toc::TOCNode;

/// References to record numbers in KF8 are stored as base-32 encoded
/// integers, with 4 digits. `to_ref` in `main.py`.
fn to_ref(n: i64) -> String {
    to_base(n, 32, Some(4))
}

/// Fields `KF8Writer`/`KF8Book` read off Python's `opts`.
#[derive(Debug, Clone)]
pub struct Kf8WriterOpts {
    pub dont_compress: bool,
    pub mobi_periodical: bool,
    pub prefer_author_sort: bool,
    pub share_not_sync: bool,
    pub toc_title: Option<String>,
    pub mobi_toc_at_start: bool,
    pub no_inline_toc: bool,
    pub mobi_passthrough: bool,
    pub extra_css: Option<String>,
    pub mobi_keep_original_images: bool,
}

impl Default for Kf8WriterOpts {
    fn default() -> Self {
        Kf8WriterOpts {
            dont_compress: false,
            mobi_periodical: false,
            prefer_author_sort: false,
            share_not_sync: true,
            toc_title: None,
            mobi_toc_at_start: false,
            no_inline_toc: false,
            mobi_passthrough: false,
            extra_css: None,
            mobi_keep_original_images: false,
        }
    }
}

/// Port of `KF8Writer.cleanup_markup`. `CSSCleanup` itself
/// (`writer8::cleanup::css_cleanup`) is not invoked here: Python only
/// ever wires it up via a `Stylizer` callback the output plugin drives
/// per-item during an earlier OEB transform pass, not from `KF8Writer`
/// directly -- nothing in `KF8Writer.__init__` calls it either. Kept
/// separate rather than folded in here for the same reason.
fn cleanup_markup(docs: &mut [(String, Dom)]) {
    for (_, dom) in docs.iter_mut() {
        for script in dom.find_all_tag_global("script") {
            let has_text = dom
                .children(script)
                .iter()
                .any(|&c| matches!(&dom.node(c).kind, NodeKind::Text(t) if !t.is_empty()));
            let has_src = dom.node(script).attrs.contains_key("src");
            if !has_text && !has_src {
                dom.detach(script);
            }
        }
        for el in dom.preorder_elements(dom.root) {
            dom.node_mut(el).attrs.shift_remove("aid");
            dom.node_mut(el).attrs.shift_remove("cid");
        }
    }
}

/// Port of `KF8Writer.replace_resource_links`'s image/font
/// `src`/`href` rewriting (the CSS `url()` half is a documented scope
/// gap -- see the module doc).
fn replace_resource_links(
    docs: &mut [(String, Dom)],
    resources: &Resources,
    used_images: &mut HashSet<String>,
) {
    for (href, dom) in docs.iter_mut() {
        let mut targets = dom.find_all_tag_global("img");
        targets.extend(dom.find_all_tag_global("image"));
        for el in targets {
            let attr_names: Vec<String> = dom.node(el).attrs.keys().cloned().collect();
            for attr in attr_names {
                if !attr.eq_ignore_ascii_case("src") && !attr.eq_ignore_ascii_case("href") {
                    continue;
                }
                let raw = dom.node(el).attrs.get(&attr).cloned().unwrap_or_default();
                let resolved = urlnormalize(&abshref(href, &raw));
                let Some(&idx) = resources.item_map.get(&resolved) else {
                    continue;
                };
                let is_image = resources
                    .records
                    .get(idx.saturating_sub(1))
                    .and_then(|r| r.as_ref())
                    .map(|b| !b.starts_with(b"FONT"))
                    .unwrap_or(true);
                let new_val = if is_image {
                    used_images.insert(resolved.clone());
                    let mime = resources
                        .mime_map
                        .get(&resolved)
                        .cloned()
                        .unwrap_or_else(|| "image/gif".to_string());
                    format!("kindle:embed:{}?mime={mime}", to_ref(idx as i64))
                } else {
                    format!("kindle:embed:{}", to_ref(idx as i64))
                };
                dom.node_mut(el).attrs.insert(attr, new_val);
            }
        }
    }
}

/// Port of the real-work half of `KF8Writer.extract_css_into_flows`
/// (`fix_import_rules`/`condense_sheet` omitted, see the module doc).
fn extract_css_into_flows(oeb: &OEBBook, docs: &mut [(String, Dom)], flows: &mut Vec<Vec<u8>>) {
    let mut sheets: HashMap<String, usize> = HashMap::new();
    for item in oeb.manifest.iter() {
        if OEB_STYLES
            .iter()
            .any(|mt| mt.eq_ignore_ascii_case(&item.media_type))
        {
            let raw = oeb.container.read(&item.href).unwrap_or_default();
            sheets.insert(urlnormalize(&item.href), flows.len());
            flows.push(raw);
        }
    }

    let mut inline_dedup: HashMap<String, usize> = HashMap::new();

    for (href, dom) in docs.iter_mut() {
        for link in dom.find_all_tag_global("link") {
            let Some(l_href) = dom.node(link).attrs.get("href").cloned() else {
                continue;
            };
            let resolved = urlnormalize(&abshref(href, &l_href));
            if let Some(&idx) = sheets.get(&resolved) {
                dom.node_mut(link).attrs.insert(
                    "href".to_string(),
                    format!("kindle:flow:{}?mime=text/css", to_ref(idx as i64)),
                );
            }
        }
        for style in dom.find_all_tag_global("style") {
            let text = dom.text_content(style);
            if text.trim().is_empty() {
                dom.detach(style);
                continue;
            }
            let idx = *inline_dedup.entry(text.clone()).or_insert_with(|| {
                let i = flows.len();
                flows.push(text.clone().into_bytes());
                i
            });
            dom.set_tag(style, "link");
            for c in dom.children(style) {
                dom.detach(c);
            }
            let attrs = &mut dom.node_mut(style).attrs;
            attrs.clear();
            attrs.insert("type".to_string(), "text/css".to_string());
            attrs.insert("rel".to_string(), "stylesheet".to_string());
            attrs.insert(
                "href".to_string(),
                format!("kindle:flow:{}?mime=text/css", to_ref(idx as i64)),
            );
        }
    }
}

/// Port of `KF8Writer.extract_svg_into_flows`.
fn extract_svg_into_flows(oeb: &OEBBook, docs: &mut [(String, Dom)], flows: &mut Vec<Vec<u8>>) {
    let mut images: HashMap<String, usize> = HashMap::new();
    for item in oeb.manifest.iter() {
        if item.media_type.eq_ignore_ascii_case(SVG_MIME) {
            let raw = oeb.container.read(&item.href).unwrap_or_default();
            images.insert(urlnormalize(&item.href), flows.len());
            flows.push(raw);
        }
    }

    for (href, dom) in docs.iter_mut() {
        for svg in dom.find_all_tag_global("svg") {
            let raw = dom.serialize(svg).into_bytes();
            let idx = flows.len();
            flows.push(raw);
            let img = dom.new_element("img");
            dom.node_mut(img).attrs.insert(
                "src".to_string(),
                format!("kindle:flow:{}?mime=image/svg+xml", to_ref(idx as i64)),
            );
            if let (Some(parent), Some(pos)) = (dom.parent(svg), dom.index_in_parent(svg)) {
                dom.insert_child(parent, pos, img);
            }
            dom.detach(svg);
        }
        for img in dom.find_all_tag_global("img") {
            let Some(src) = dom.node(img).attrs.get("src").cloned() else {
                continue;
            };
            let resolved = urlnormalize(&abshref(href, &src));
            if let Some(&idx) = images.get(&resolved) {
                dom.node_mut(img).attrs.insert(
                    "src".to_string(),
                    format!("kindle:flow:{}?mime=image/svg+xml", to_ref(idx as i64)),
                );
            }
        }
    }
}

/// Port of `KF8Writer.replace_internal_links_with_placeholders`.
/// Returns `placeholder -> (target href, fragment)`.
fn replace_internal_links_with_placeholders(
    docs: &mut [(String, Dom)],
) -> HashMap<String, (String, String)> {
    let mut link_map = HashMap::new();
    let mut count: i64 = 0;
    let hrefs: HashSet<String> = docs.iter().map(|(h, _)| h.clone()).collect();
    for (href, dom) in docs.iter_mut() {
        for a in dom.find_all_tag_global("a") {
            let Some(raw_href) = dom.node(a).attrs.get("href").cloned() else {
                continue;
            };
            count += 1;
            let resolved = abshref(href, &raw_href);
            let (target_href, frag) = urldefrag(&resolved);
            let target_href = urlnormalize(&target_href);
            if hrefs.contains(&target_href) {
                let placeholder = format!("kindle:pos:fid:0000:off:{}", to_href(count));
                link_map.insert(placeholder.clone(), (target_href, frag));
                dom.node_mut(a)
                    .attrs
                    .insert("href".to_string(), placeholder);
            }
        }
    }
    link_map
}

fn is_in_table(dom: &Dom, mut id: NodeId) -> bool {
    while let Some(p) = dom.parent(id) {
        if dom.tag(p) == Some("table") {
            return true;
        }
        id = p;
    }
    false
}

/// Port of `KF8Writer.insert_aid_attributes`. Returns
/// `(href, id_or_empty_for_body) -> aid`.
fn insert_aid_attributes(docs: &mut [(String, Dom)]) -> HashMap<(String, String), String> {
    let mut id_map = HashMap::new();
    let mut cid: u32 = 0;
    for (i, (href, dom)) in docs.iter_mut().enumerate() {
        let aidbase = (i as i64) * 1_000_000;
        let mut j: i64 = 0;
        let html_id = dom.find_first_tag_global("html");
        for el in dom.preorder_elements(dom.root) {
            if Some(el) == html_id {
                continue;
            }
            let mut id_val = dom.node(el).attrs.get("id").cloned();
            if id_val.is_none() && dom.tag(el) == Some("a") {
                if let Some(name) = dom.node(el).attrs.get("name").cloned() {
                    dom.node_mut(el)
                        .attrs
                        .insert("id".to_string(), name.clone());
                    id_val = Some(name);
                }
            }
            let tagname = dom.tag(el).unwrap_or_default().to_string();
            if id_val.is_none() && !is_aid_able(&tagname) {
                continue;
            }
            if tagname == "table" || is_in_table(dom, el) {
                if let Some(id_) = &id_val {
                    cid += 1;
                    let val = format!("c{cid}");
                    id_map.insert((href.clone(), id_.clone()), val.clone());
                    dom.node_mut(el).attrs.insert("cid".to_string(), val);
                }
            } else {
                let aid = to_base(aidbase + j, 32, None);
                dom.node_mut(el)
                    .attrs
                    .insert("aid".to_string(), aid.clone());
                if tagname == "body" {
                    id_map.insert((href.clone(), String::new()), aid.clone());
                }
                if let Some(id_) = &id_val {
                    id_map.insert((href.clone(), id_.clone()), aid.clone());
                }
                j += 1;
            }
        }
    }
    id_map
}

fn create_text_records(
    flows: &[Vec<u8>],
    compress_flag: bool,
) -> (Vec<Vec<u8>>, usize, Vec<usize>, usize, usize) {
    let text: Vec<u8> = flows.concat();
    let text_length = text.len();
    let mut records = Vec::new();
    let mut uncompressed_record_lengths = Vec::new();
    let mut pos = 0usize;
    let mut records_size = 0usize;
    while pos < text_length {
        let (mut data, overlap) = create_text_record(&text, &mut pos);
        uncompressed_record_lengths.push(data.len());
        if compress_flag {
            data = compress(&data).unwrap_or(data);
        }
        data.extend_from_slice(&overlap);
        data.push(overlap.len() as u8);
        records_size += data.len();
        records.push(data);
    }
    let last_text_record_idx = records.len();
    let mut first_non_text_record_idx = last_text_record_idx + 1;
    if !records_size.is_multiple_of(4) {
        records.push(vec![0u8; records_size % 4]);
        first_non_text_record_idx += 1;
    }
    (
        records,
        text_length,
        uncompressed_record_lengths,
        last_text_record_idx,
        first_non_text_record_idx,
    )
}

fn create_fdst_records(flows: &[Vec<u8>]) -> (Vec<Vec<u8>>, usize) {
    let mut entries = Vec::new();
    let mut count = 0usize;
    let mut start = 0u32;
    for flow in flows {
        let end = start + flow.len() as u32;
        entries.push(start);
        entries.push(end);
        count += 1;
        start = end;
    }
    let mut rec = b"FDST".to_vec();
    rec.extend_from_slice(&12u32.to_be_bytes());
    rec.extend_from_slice(&(count as u32).to_be_bytes());
    for e in entries {
        rec.extend_from_slice(&e.to_be_bytes());
    }
    (vec![rec], count)
}

/// A flattened TOC node during [`build_ncx_entries`], keyed by its
/// position in a depth-first flatten (Rust stand-in for Python's
/// `id(item)`-as-hashable-key trick).
struct WorkingNcxNode {
    node_id: usize,
    depth: u64,
    parent_node_id: Option<usize>,
    children_node_ids: Vec<usize>,
    label: String,
    href: Option<String>,
    author: Option<String>,
    description: Option<String>,
    pos_fid: (u64, u64),
    offset: u64,
}

fn flatten_toc(
    node: &TOCNode,
    depth: u64,
    parent_id: Option<usize>,
    next_id: &mut usize,
    out: &mut Vec<WorkingNcxNode>,
) -> usize {
    let this_id = *next_id;
    *next_id += 1;
    out.push(WorkingNcxNode {
        node_id: this_id,
        depth,
        parent_node_id: parent_id,
        children_node_ids: Vec::new(),
        label: node.title.clone().unwrap_or_else(|| "Unknown".to_string()),
        href: node.href.clone(),
        author: node.author.clone(),
        description: node.description.clone(),
        pos_fid: (0, 0),
        offset: 0,
    });
    let mut kids = Vec::new();
    for c in &node.children {
        kids.push(flatten_toc(c, depth + 1, Some(this_id), next_id, out));
    }
    out[this_id].children_node_ids = kids;
    this_id
}

/// Port of `KF8Writer.create_indices`'s NCX-entry-list construction
/// (everything up to, but not including, `apply_trailing_byte_sequences`/
/// `NCXIndex` themselves).
fn build_ncx_entries(
    oeb: &OEBBook,
    id_map: &HashMap<(String, String), String>,
    aid_offset_map: &HashMap<String, (u64, u64, u64)>,
    chunk_table: &[ChunkTableEntry],
    flow0_len: usize,
    is_periodical: bool,
) -> Vec<NcxTableEntry> {
    let mut working = Vec::new();
    let mut next_id = 0usize;
    for c in &oeb.toc.root.children {
        flatten_toc(c, 0, None, &mut next_id, &mut working);
    }

    for w in working.iter_mut() {
        let href_full = w.href.clone().unwrap_or_default();
        let (href, frag) = urldefrag(&href_full);
        let aid = id_map
            .get(&(href.clone(), frag))
            .cloned()
            .or_else(|| id_map.get(&(href, String::new())).cloned());
        match aid.as_deref().and_then(|a| aid_offset_map.get(a)) {
            Some(&(pos, fid, offset)) => {
                w.pos_fid = (pos, fid);
                w.offset = offset;
            }
            None => {
                let offset = chunk_table
                    .first()
                    .map(|c| c.insert_pos as u64)
                    .unwrap_or(0);
                w.pos_fid = (0, 0);
                w.offset = offset;
            }
        }
    }

    working.sort_by_key(|w| (w.depth, w.offset));
    let id_to_index: HashMap<usize, u64> = working
        .iter()
        .enumerate()
        .map(|(i, w)| (w.node_id, i as u64))
        .collect();

    let lengths: Vec<u64> = working
        .iter()
        .map(|w| {
            let next_start = working
                .iter()
                .filter(|o| o.depth <= w.depth && o.offset > w.offset)
                .map(|o| o.offset)
                .min()
                .unwrap_or(flow0_len as u64);
            next_start.saturating_sub(w.offset)
        })
        .collect();

    working
        .into_iter()
        .enumerate()
        .map(|(i, w)| NcxTableEntry {
            index: i as u64,
            offset: w.offset,
            length: lengths[i],
            label: w.label,
            depth: w.depth,
            pos_fid: w.pos_fid,
            parent: w.parent_node_id.and_then(|p| id_to_index.get(&p).copied()),
            first_child: w
                .children_node_ids
                .first()
                .and_then(|c| id_to_index.get(c).copied()),
            last_child: w
                .children_node_ids
                .last()
                .and_then(|c| id_to_index.get(c).copied()),
            author: if is_periodical { w.author } else { None },
            description: if is_periodical { w.description } else { None },
            kind: None,
        })
        .collect()
}

/// Port of `KF8Writer.create_guide`.
fn create_guide(
    oeb: &OEBBook,
    id_map: &HashMap<(String, String), String>,
    aid_offset_map: &HashMap<String, (u64, u64, u64)>,
) -> (Option<u32>, Vec<GuideTableEntry>) {
    let mut start_offset = None;
    let mut guide_table = Vec::new();
    for r in oeb.guide.values() {
        let (href, frag) = urldefrag(&r.href);
        let aid = id_map
            .get(&(href.clone(), frag))
            .cloned()
            .or_else(|| id_map.get(&(href, String::new())).cloned());
        let Some(aid) = aid else { continue };
        let Some(&(pos, fid, offset)) = aid_offset_map.get(&aid) else {
            continue;
        };
        if is_guide_ref_start(r.title.as_deref(), Some(&r.type_)) {
            start_offset = Some(offset as u32);
        }
        guide_table.push(GuideTableEntry {
            title: r.title.clone().unwrap_or_else(|| "Unknown".to_string()),
            type_: r.type_.clone(),
            pos_fid: (pos, fid),
        });
    }
    guide_table.sort_by(|a, b| a.type_.cmp(&b.type_));
    (start_offset, guide_table)
}

/// Assembles a standalone KF8 payload from an `OEBBook`. Port of
/// `KF8Writer` (+ `create_kf8_book` for the top-level entry point).
pub struct KF8Writer {
    opts: Kf8WriterOpts,
    pub log: MobiLog,
}

impl KF8Writer {
    pub fn new(opts: Kf8WriterOpts) -> Self {
        KF8Writer {
            opts,
            log: MobiLog::default(),
        }
    }

    /// Port of `KF8Writer.__init__` followed by `KF8Book(writer,
    /// for_joint=False)` (`create_kf8_book(oeb, opts, resources,
    /// for_joint=False)` with a `KF8Writer`-owned `Resources` rather
    /// than a caller-supplied shared one -- see the `for_joint` note on
    /// [`KF8Book::new`] for what a real joint-output caller would need
    /// to do differently).
    pub fn write(&mut self, oeb: &mut OEBBook) -> Result<KF8Book> {
        let compress_flag = !self.opts.dont_compress;

        let toc_opts = TocOpts {
            toc_title: self.opts.toc_title.clone(),
            mobi_toc_at_start: self.opts.mobi_toc_at_start,
            no_inline_toc: self.opts.no_inline_toc,
            mobi_passthrough: self.opts.mobi_passthrough,
            extra_css: self.opts.extra_css.clone(),
        };
        let mut toc_adder = TocAdder::new(oeb, &toc_opts, true, false)?;

        let resource_opts = ResourceOpts {
            mobi_keep_original_images: self.opts.mobi_keep_original_images,
        };
        let mut resources = Resources::new(oeb, resource_opts, self.opts.mobi_periodical, true);

        let mut docs: Vec<(String, Dom)> = Vec::new();
        for item in oeb.spine.iter() {
            let Some(m) = oeb.manifest.get_by_id(&item.idref) else {
                self.log
                    .warn(format!("Spine idref {:?} not in manifest", item.idref));
                continue;
            };
            let raw = oeb.container.read(&m.href).unwrap_or_default();
            let html = String::from_utf8_lossy(&raw).into_owned();
            docs.push((m.href.clone(), Dom::parse(&html)));
        }

        cleanup_markup(&mut docs);
        let mut used_images: HashSet<String> = HashSet::new();
        replace_resource_links(&mut docs, &resources, &mut used_images);

        let mut flows: Vec<Vec<u8>> = vec![Vec::new()];
        extract_css_into_flows(oeb, &mut docs, &mut flows);
        extract_svg_into_flows(oeb, &mut docs, &mut flows);

        let link_map = replace_internal_links_with_placeholders(&mut docs);
        let id_map = insert_aid_attributes(&mut docs);

        let mut placeholder_map: HashMap<String, Option<String>> = HashMap::new();
        for (placeholder, (href, frag)) in &link_map {
            let aid = id_map
                .get(&(href.clone(), frag.clone()))
                .cloned()
                .or_else(|| id_map.get(&(href.clone(), String::new())).cloned());
            placeholder_map.insert(placeholder.clone(), aid);
        }

        let chunker = Chunker::new(docs, &placeholder_map)?;
        for w in &chunker.warnings {
            self.log.warn(w.clone());
        }
        flows[0] = chunker.text.clone();

        let (
            text_records,
            text_length,
            uncompressed_lengths,
            last_text_record_idx,
            first_non_text_record_idx,
        ) = create_text_records(&flows, compress_flag);
        let (fdst_records, fdst_count) = create_fdst_records(&flows);

        let skel_records = skel_index(&chunker.skel_table)?;
        let (chunk_records, _chunk_cncx) = chunk_index(&chunker.chunk_table)?;

        let mut records: Vec<Vec<u8>> = vec![Vec::new()];
        records.extend(text_records);

        let mut ncx_records = Vec::new();
        let mut has_tbs = false;
        if oeb.toc.count() >= 1 {
            let entries = build_ncx_entries(
                oeb,
                &id_map,
                &chunker.aid_offset_map,
                &chunker.chunk_table,
                flows[0].len(),
                self.opts.mobi_periodical,
            );
            has_tbs = apply_trailing_byte_sequences(&entries, &mut records, &uncompressed_lengths)?;
            let (recs, _cncx) = ncx_index(&entries)?;
            ncx_records = recs;
        } else {
            self.log
                .warn("Document has no ToC, MOBI will have no NCX index");
        }

        let (start_offset, guide_table) = create_guide(oeb, &id_map, &chunker.aid_offset_map);
        let guide_records = if guide_table.is_empty() {
            Vec::new()
        } else {
            guide_index(&guide_table)?
        };

        // `KF8Writer` generates its own inline ToC; a MOBI 6 sibling
        // writer must not see it (matches Python's
        // `self.toc_adder.remove_generated_toc()`).
        toc_adder.remove_generated_toc(oeb);

        let kf8_opts = Kf8Opts {
            prefer_author_sort: self.opts.prefer_author_sort,
            share_not_sync: self.opts.share_not_sync,
            mobi_periodical: self.opts.mobi_periodical,
        };
        let primary_writing_mode = oeb
            .metadata
            .get("primary_writing_mode")
            .first()
            .map(|i| i.value.clone());

        let inputs = KF8BuildInputs {
            last_text_record_idx,
            first_non_text_record_idx,
            records,
            text_length,
            chunk_records,
            skel_records,
            guide_records,
            ncx_records,
            resources: &mut resources,
            used_images,
            fdst_count,
            fdst_records,
            compress: compress_flag,
            has_tbs,
            start_offset,
            metadata: &oeb.metadata,
            opts: kf8_opts,
            page_progression_direction: oeb.spine.page_progression_direction.clone(),
            primary_writing_mode,
        };

        KF8Book::new(inputs, false)
    }
}

/// Port of the module-level `create_kf8_book(oeb, opts, resources,
/// for_joint=False)` convenience function.
pub fn create_kf8_book(oeb: &mut OEBBook, opts: Kf8WriterOpts) -> Result<KF8Book> {
    KF8Writer::new(opts).write(oeb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::constants::XHTML_MIME;
    use crate::oeb::container::DirContainer;

    fn simple_book(dir: &std::path::Path) -> OEBBook {
        std::fs::write(
            dir.join("c1.html"),
            "<html><body><h1 id=\"top\">Chapter One</h1><p>Hello <a href=\"c2.html\">next</a></p></body></html>",
        )
        .unwrap();
        std::fs::write(
            dir.join("c2.html"),
            "<html><body><h1 id=\"top2\">Chapter Two</h1><p>World <a href=\"c1.html#top\">back</a></p></body></html>",
        )
        .unwrap();
        let mut oeb = OEBBook::new(Box::new(DirContainer::new(dir)));
        oeb.manifest.add("c1", "c1.html", XHTML_MIME);
        oeb.manifest.add("c2", "c2.html", XHTML_MIME);
        oeb.spine.add("c1", true);
        oeb.spine.add("c2", true);
        oeb.metadata.add("title", "A KF8 Book");
        oeb.metadata.add("creator", "Jane Author");
        oeb.metadata.add("date", "2020-01-01T00:00:00+00:00");
        oeb.metadata.add("language", "en");
        oeb.toc.root.add(TOCNode::new(
            Some("Chapter One".into()),
            Some("c1.html".into()),
        ));
        oeb.toc.root.add(TOCNode::new(
            Some("Chapter Two".into()),
            Some("c2.html".into()),
        ));
        oeb
    }

    #[test]
    fn writes_a_kf8_book_with_a_wellformed_record0() {
        let dir = tempfile::tempdir().unwrap();
        let mut oeb = simple_book(dir.path());
        let mut writer = KF8Writer::new(Kf8WriterOpts::default());
        let book = writer.write(&mut oeb).unwrap();
        let record0 = book.record0(&oeb.metadata).unwrap();
        assert_eq!(&record0[16..20], b"MOBI");
        let bytes = book.to_bytes(&oeb.metadata).unwrap();
        assert_eq!(&bytes[60..68], b"BOOKMOBI");
    }
}
