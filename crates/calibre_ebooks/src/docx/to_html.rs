//! DOCX → HTML conversion.
//!
//! [`DOCXToHTML`] is the **provisional sketch** that predates this
//! module's port — paragraphs, runs, hyperlinks and images, with
//! heading levels guessed from `w:pStyle` and no style resolution
//! whatsoever. It stays wired into the DOCX input plugin (see
//! `input/docx_input.rs`) and keeps producing *something* until real
//! `Convert::__call__` orchestration is ready to replace it wholesale
//! -- see issue #130's tracked follow-ups (#283-293) for exactly
//! what's still missing.
//!
//! The real port, tracked as issue #130 with per-piece follow-ups
//! #283-293, so far covers:
//!
//! - [`convert_run`]/[`convert_p`]: `w:r` -> `<span>`, `w:p` ->
//!   `<p>`/`<h1>`..`<h6>`, using the real
//!   [`super::styles::Styles::resolve_run`]/`resolve_paragraph`
//!   cascade (issue #130's styles/numbering/tables cluster). Carries
//!   the per-document state ([`ConvertState`]) both need across the
//!   whole body walk, and tracks `w:hyperlink` runs into
//!   `ConvertState::link_map`/`link_source_map`/`is_link` for
//!   [`resolve_links`].
//! - [`read_page_properties`]: the paragraph/table -> [`PageProperties`]
//!   map (plus `w:tbl` registration) [`convert_body`] walks.
//! - [`convert_body`]: builds a `<body>`, converts and appends every
//!   `w:p` in document order via `convert_p`, then applies
//!   [`super::styles::Styles::apply_contextual_spacing`]/
//!   [`super::styles::Styles::apply_section_page_breaks`] -- the whole
//!   of `Convert.__call__`'s main paragraph loop.
//! - [`read_block_anchors`]: ids paragraphs a top-level bookmark
//!   precedes.
//! - [`apply_tab_indentation`]: folds leading `w:tab`-based paragraph
//!   indentation into a `text-indent` CSS value.
//! - [`mark_block_runs`]: collapses consecutive, same-frame,
//!   identically-bordered paragraphs into one visual block
//!   (`ConvertState::block_runs`, for the not-yet-ported `apply_frames`
//!   -- issue #287).
//! - [`resolve_links`]: turns tracked `w:hyperlink`s into real `<a>`
//!   elements (issue #283's `link_map`-driven first block only --
//!   `fields.py`/`images.py`'s link sources are separate, still-open
//!   issues #290/#289).
//!
//! Not yet wired into `DOCXToHTML` -- that still needs the footnote
//! body conversion (#284) and everything downstream (frames,
//! tables/numbering markup, TOC, OPF writing), none of which exist
//! here yet.
//!
//! # What `convert_run` defers, and why
//!
//! - `w:drawing`/`w:pict` (embedded images): skipped entirely --
//!   `images.py` isn't ported (issue #130).
//! - `style.lang`: passed through as-is rather than reduced to an
//!   ISO 639-1 code via calibre's language-tag database
//!   (`canonicalize_lang`/`lang_as_iso639_1`, private to `oeb::polish::opf`
//!   and not a general-purpose utility here). A raw BCP-47 tag
//!   (`"en-US"`) is still valid in an HTML `lang` attribute -- this
//!   loses calibre's own normalization preference, not HTML
//!   correctness. See [`docx_lang_to_html`].
//!
//! # No `Text` buffering
//!
//! Python's `Text` helper buffers text fragments before flushing them
//! onto an element's `.text`/`.tail` (the two-fields-per-element lxml
//! model). [`crate::dom`]'s sibling-text-node model needs no such
//! buffering: each text/element fragment is simply appended as the
//! next child, and adjacent text nodes read identically to one merged
//! string once serialized -- see `crate::dom`'s own module docs for
//! why this representation was chosen.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::path::Path;

use indexmap::IndexMap;
use roxmltree::{Document, Node};

use crate::dom::{Dom, NodeId, NodeKind};

use super::block_styles::{pt, Edge, Frame, ParagraphStyle};
use super::container::{Docx, Relationships};
use super::error::DocxError;
use super::fonts::{is_symbol_font, map_symbol_text};
use super::footnotes::Footnotes;
use super::names::DocxNamespace;
use super::settings::Settings;
use super::styles::{PageProperties, Styles};
use super::theme::Theme;

pub struct DOCXToHTML;

impl DOCXToHTML {
    pub fn convert<R: Read + Seek>(
        docx: &mut Docx<R>,
        dest_dir: &Path,
    ) -> Result<String, DocxError> {
        let ns = DocxNamespace::new(docx.is_transitional());
        let doc_name = docx.document_name()?;

        // 1. Read Document Relationships
        // Construct path: word/_rels/document.xml.rels
        // Simple logic: assume doc_name has a parent dir
        let path_obj = Path::new(&doc_name);
        let file_name = path_obj.file_name().unwrap_or_default().to_string_lossy();
        let parent = path_obj.parent().unwrap_or(Path::new(""));
        let rels_path = parent.join("_rels").join(format!("{}.rels", file_name));
        let rels_path_str = rels_path.to_string_lossy().replace("\\", "/");

        let mut doc_rels = HashMap::new();
        if let Ok(content) = docx.read(&rels_path_str) {
            let text = String::from_utf8(content).unwrap_or_default();
            if let Ok(doc) = Document::parse(&text) {
                for node in doc.descendants() {
                    if node.has_tag_name("Relationship") {
                        let id = node.attribute("Id").unwrap_or_default().to_string();
                        let target = node.attribute("Target").unwrap_or_default().to_string();
                        doc_rels.insert(id, target);
                    }
                }
            }
        }

        // 2. Read Document Content
        let content = docx.read(&doc_name)?;
        let text = String::from_utf8(content).map_err(|e| DocxError::InvalidDocx(e.to_string()))?;
        let doc = Document::parse(&text)?;

        // 3. Generate HTML
        let mut html = String::from("<html><head><meta charset=\"utf-8\"/></head><body>");

        for node in doc.descendants() {
            if node.tag_name().name() == "p" {
                Self::process_paragraph(node, &mut html, &doc_rels, docx, dest_dir, &ns);
            }
        }

        html.push_str("</body></html>");
        Ok(html)
    }

    fn process_paragraph<R: Read + Seek>(
        node: Node,
        html: &mut String,
        rels: &HashMap<String, String>,
        docx: &mut Docx<R>,
        dest_dir: &Path,
        ns: &DocxNamespace,
    ) {
        // Determine Tag (p, h1-h6) based on pPr/pStyle
        let mut tag = "p";

        for child in node.children() {
            if child.tag_name().name() == "pPr" {
                for p_prop in child.children() {
                    if p_prop.tag_name().name() == "pStyle" {
                        if let Some(val) = p_prop.attribute("val") {
                            match val {
                                "Heading1" => tag = "h1",
                                "Heading2" => tag = "h2",
                                "Heading3" => tag = "h3",
                                "Heading4" => tag = "h4",
                                "Heading5" => tag = "h5",
                                "Heading6" => tag = "h6",
                                _ => {}
                            }
                        }
                    }
                }
            }
        }

        html.push('<');
        html.push_str(tag);
        html.push('>');

        for child in node.children() {
            if child.tag_name().name() == "r" {
                Self::process_run(child, html, rels, docx, dest_dir, ns);
            } else if child.tag_name().name() == "hyperlink" {
                // Handle hyperlink
                let rid = ns.get(child, "r:id");
                if let Some(rid) = rid {
                    if let Some(target) = rels.get(rid) {
                        html.push_str(&format!("<a href=\"{}\">", target));
                        for sub in child.children() {
                            if sub.tag_name().name() == "r" {
                                Self::process_run(sub, html, rels, docx, dest_dir, ns);
                            }
                        }
                        html.push_str("</a>");
                    }
                }
            }
        }

        html.push_str(&format!("</{}>", tag));
    }

    fn process_run<R: Read + Seek>(
        node: Node,
        html: &mut String,
        rels: &HashMap<String, String>,
        docx: &mut Docx<R>,
        dest_dir: &Path,
        ns: &DocxNamespace,
    ) {
        for child in node.children() {
            match child.tag_name().name() {
                "t" => {
                    if let Some(text) = child.text() {
                        html.push_str(&html_escape::encode_text(text));
                    }
                }
                "br" => html.push_str("<br/>"),
                "drawing" => {
                    // Extract image
                    // This is complex in OOXML. drawing -> inline -> graphic -> graphicData -> pic -> blipFill -> blip -> embed
                    // Or similar structure
                    for desc in child.descendants() {
                        if desc.tag_name().name() == "blip" {
                            if let Some(rid) = ns.get(desc, "r:embed") {
                                if let Some(target) = rels.get(rid) {
                                    // target is relative to document.xml usually, e.g. "media/image1.jpeg"
                                    // We need to resolve it relative to DOCX root (word/media/image1.jpeg)
                                    // Assuming document is at word/document.xml
                                    let image_path = Path::new("word")
                                        .join(target)
                                        .to_string_lossy()
                                        .replace("\\", "/");

                                    if let Ok(data) = docx.read(&image_path) {
                                        // Write to dest_dir
                                        let file_name =
                                            Path::new(target).file_name().unwrap_or_default();
                                        let dest_path = dest_dir.join(file_name);
                                        if std::fs::write(&dest_path, data).is_ok() {
                                            html.push_str(&format!(
                                                "<img src=\"{}\" />",
                                                file_name.to_string_lossy()
                                            ));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

pub mod html_escape {
    pub fn encode_text(s: &str) -> String {
        s.replace("&", "&amp;")
            .replace("<", "&lt;")
            .replace(">", "&gt;")
            .replace("\"", "&quot;")
            .replace("'", "&#39;")
    }
}

/// Converts one `w:r` into a `<span>` in `dom`, returning its `NodeId`.
/// See the module docs for what's deferred.
///
/// `uuid` is the per-document identifier Python's `Convert.__init__`
/// generates once (`self.uuid = uuid.uuid4().hex`) and stamps onto
/// `data-noteref-container` so `cleanup.py`'s later pass can find every
/// noteref span belonging to *this* conversion run; callers of this
/// function own that lifetime and pass it in.
///
/// Port of the Python `Convert.convert_run`.
#[allow(clippy::too_many_arguments)]
pub fn convert_run<'a, 'i>(
    dom: &mut Dom,
    run: Node<'a, 'i>,
    styles: &mut Styles<'a, 'i>,
    footnotes: &mut Footnotes<'a, 'i>,
    settings: &Settings,
    theme: &Theme,
    doc_lang: Option<&str>,
    uuid: &str,
    ns: &DocxNamespace,
) -> NodeId {
    let span = dom.new_element("span");

    for child in run.children().filter(|c| c.is_element()) {
        if ns.is_tag(child, "w:t") {
            append_run_text(dom, span, child, ns);
        } else if ns.is_tag(child, "w:cr") {
            let br = dom.new_element("br");
            dom.append_child(span, br);
        } else if ns.is_tag(child, "w:br") {
            let br = dom.new_element("br");
            let typ = ns.get(child, "w:type");
            if matches!(typ, Some("column") | Some("page")) {
                dom.node_mut(br)
                    .attrs
                    .insert("style".to_string(), "page-break-after:always".to_string());
            } else if let Some(clear) = ns
                .get(child, "clear")
                .filter(|c| matches!(*c, "all" | "left" | "right"))
            {
                let side = if clear == "all" { "both" } else { clear };
                dom.node_mut(br)
                    .attrs
                    .insert("style".to_string(), format!("clear:{side}"));
            }
            dom.append_child(span, br);
        } else if ns.is_tag(child, "w:footnoteReference") || ns.is_tag(child, "w:endnoteReference")
        {
            append_note_ref(dom, span, child, footnotes, uuid, ns);
        } else if ns.is_tag(child, "w:tab") {
            let spaces = ((settings.default_tab_stop / 36.0) * 6.0).ceil().max(0.0) as usize;
            let tab = dom.new_element("span");
            dom.node_mut(tab)
                .attrs
                .insert("class".to_string(), "tab".to_string());
            let t = dom.new_text(&"\u{a0}".repeat(spaces));
            dom.append_child(tab, t);
            dom.append_child(span, tab);
        } else if ns.is_tag(child, "w:noBreakHyphen") {
            let t = dom.new_text("\u{2011}");
            dom.append_child(span, t);
        } else if ns.is_tag(child, "w:softHyphen") {
            let t = dom.new_text("\u{ad}");
            dom.append_child(span, t);
        }
        // w:drawing / w:pict: deferred, see module docs.
    }

    let style = styles.resolve_run(run, theme, ns);
    if matches!(
        style.vert_align.as_deref(),
        Some("superscript") | Some("subscript")
    ) {
        // Python's `ans.text or len(ans)`: any leading text or any
        // child element. crate::dom represents text as an ordinary
        // child too, so "has any child at all" covers both.
        if !dom.children(span).is_empty() {
            let val = if style.vert_align.as_deref() == Some("superscript") {
                "sup"
            } else {
                "sub"
            };
            dom.node_mut(span)
                .attrs
                .insert("data-docx-vert".to_string(), val.to_string());
        }
    }
    if let Some(lang) = style.lang.as_deref() {
        if let Some(html_lang) = docx_lang_to_html(lang) {
            if Some(html_lang.as_str()) != doc_lang {
                dom.node_mut(span)
                    .attrs
                    .insert("lang".to_string(), html_lang);
            }
        }
    }
    if style.rtl == Some(true) {
        dom.node_mut(span)
            .attrs
            .insert("dir".to_string(), "rtl".to_string());
    }
    if let Some(font_family) = style.font_family.as_deref() {
        if is_symbol_font(font_family) {
            remap_symbol_text(dom, span, font_family);
            styles.set_run_font_family(run, "sans-serif".to_string());
        }
    }

    span
}

/// Port of the `w:t` branch of `Convert.convert_run`'s loop.
fn append_run_text<'a, 'i>(dom: &mut Dom, parent: NodeId, w_t: Node<'a, 'i>, ns: &DocxNamespace) {
    let Some(raw) = w_t.text() else { return };
    if raw.is_empty() {
        return;
    }
    let preserve = ns.get(w_t, "xml:space") == Some("preserve");
    let mut text = raw.to_string();
    if !preserve {
        let trimmed = text.trim_matches(|c: char| matches!(c, ' ' | '\n' | '\r' | '\t'));
        if !trimmed.is_empty() {
            text = trimmed.to_string();
        }
    }
    let needs_pre_wrap = has_consecutive_whitespace(&text) || text.contains(['\n', '\r', '\t']);
    if needs_pre_wrap {
        let wrapper = dom.new_element("span");
        dom.node_mut(wrapper)
            .attrs
            .insert("style".to_string(), "white-space:pre-wrap".to_string());
        let t = dom.new_text(&text);
        dom.append_child(wrapper, t);
        dom.append_child(parent, wrapper);
    } else {
        let t = dom.new_text(&text);
        dom.append_child(parent, t);
    }
}

/// Port of the Python `ms_pat = re.compile(r'\s{2,}')` check.
fn has_consecutive_whitespace(s: &str) -> bool {
    let mut prev_was_whitespace = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if prev_was_whitespace {
                return true;
            }
            prev_was_whitespace = true;
        } else {
            prev_was_whitespace = false;
        }
    }
    false
}

/// Port of the `w:footnoteReference`/`w:endnoteReference` branch.
fn append_note_ref<'a, 'i>(
    dom: &mut Dom,
    parent: NodeId,
    reference: Node<'a, 'i>,
    footnotes: &mut Footnotes<'a, 'i>,
    uuid: &str,
    ns: &DocxNamespace,
) {
    let Some((anchor, number)) = footnotes.get_ref(reference, ns) else {
        return;
    };
    let a = dom.new_element("a");
    {
        let attrs = &mut dom.node_mut(a).attrs;
        attrs.insert("id".to_string(), format!("back_{anchor}"));
        attrs.insert("href".to_string(), format!("#{anchor}"));
        attrs.insert("title".to_string(), number.clone());
        attrs.insert("class".to_string(), "noteref".to_string());
        attrs.insert("role".to_string(), "doc-noteref".to_string());
    }
    let t = dom.new_text(&number);
    dom.append_child(a, t);
    dom.append_child(parent, a);
    dom.node_mut(parent)
        .attrs
        .insert("data-noteref-container".to_string(), uuid.to_string());
}

/// A deliberately simplified stand-in for the Python `html_lang`: does
/// not reduce a full BCP-47 tag to its ISO 639-1 form via calibre's
/// language-tag database, just filters out empty/`"und"` (undetermined)
/// tags. See the module docs.
fn docx_lang_to_html(lang: &str) -> Option<String> {
    let lang = lang.trim();
    if lang.is_empty() || lang.eq_ignore_ascii_case("und") {
        None
    } else {
        Some(lang.to_string())
    }
}

/// Remaps every text descendant of `id` through `font`'s symbol-glyph
/// table in place.
///
/// Port of the Python `for elem in text: elem.text = map_symbol_text(...)`
/// loop, walking [`crate::dom`]'s sibling-text-nodes instead of the
/// `Text` helper's tracked `(elem, attr)` list -- see the module docs.
fn remap_symbol_text(dom: &mut Dom, id: NodeId, font: &str) {
    for child in dom.children(id) {
        let is_text = matches!(&dom.node(child).kind, NodeKind::Text(_));
        if is_text {
            if let NodeKind::Text(t) = &mut dom.node_mut(child).kind {
                *t = map_symbol_text(t, font);
            }
        } else {
            remap_symbol_text(dom, child, font);
        }
    }
}

/// Per-document state `Convert.__init__` sets up and `convert_p`
/// (transitively `convert_run`) reads/writes across the whole body
/// walk. Deliberately narrower than `Convert`'s full instance state --
/// see [`convert_p`]'s docs for what's tracked here vs. left for the
/// (not yet ported) consumer that would need it.
///
/// Port of the relevant slice of the Python `Convert.__init__`.
#[derive(Debug, Default)]
pub struct ConvertState<'a, 'i> {
    /// HTML element -> the source node it was built from. Order
    /// matters to later phases (an `IndexMap`, matching Python's
    /// `OrderedDict`), even though nothing here yet consumes that
    /// order.
    pub object_map: IndexMap<NodeId, Node<'a, 'i>>,
    /// Bookmark/TOC name -> the generated HTML anchor id. Also used,
    /// per Python's own idiom, as a *redirect* table: a generated
    /// anchor id used as a key maps to whatever id ended up actually
    /// applied to an element, when the anchor itself didn't get its
    /// own -- see the "trailing pending anchor" step in `convert_p`.
    pub anchor_map: HashMap<String, String>,
    /// Paragraph -> its runs, in document order. Needed by the
    /// (not yet ported) `Styles::cascade`.
    pub layers: IndexMap<Node<'a, 'i>, Vec<Node<'a, 'i>>>,
    /// The anchor a `TOC ` field's `w:instrText` generated, if any --
    /// `write()` (not yet ported) uses this for the OPF guide's `toc`
    /// reference.
    pub toc_anchor: Option<String>,
    /// Paragraph -> its resolved `w:framePr` frame (`None` when it has
    /// none, Python's `inherit`). Populated once per paragraph in
    /// [`convert_p`]; read by [`mark_block_runs`] to decide whether
    /// two adjacent paragraphs belong to the same frame (almost always
    /// trivially true -- `None == None` -- outside `w:framePr`
    /// documents). The other consumer of a paragraph's frame,
    /// `add_frame`/`apply_frames`'s *separate* framing mechanism, is
    /// not yet ported.
    pub frame_map: HashMap<Node<'a, 'i>, Option<Frame>>,
    /// `(border_style, run)` pairs [`mark_block_runs`] appends one of
    /// for each maximal run of 2+ consecutive, same-frame,
    /// identically-bordered paragraphs -- `border_style` carries the
    /// merged run's outer border (for the wrapping `<div>` the
    /// not-yet-ported `apply_frames` builds from it).
    pub block_runs: Vec<(ParagraphStyle, Vec<Node<'a, 'i>>)>,
    /// `w:hyperlink` source element -> the HTML spans [`convert_p`]
    /// built for each run inside it, in document order. Populated in
    /// [`convert_p`]; consumed by [`resolve_links`], which merges
    /// multi-run hyperlinks into one wrapping `<a>` and sets its
    /// `href`.
    pub link_map: HashMap<Node<'a, 'i>, Vec<NodeId>>,
    /// `w:hyperlink` source element -> the relationships map active
    /// when [`convert_p`] processed it (the main document's, or a
    /// footnote/endnote's own -- see #284). Needed because `r:id`
    /// hyperlink targets are only meaningful relative to whichever
    /// part's relationships the hyperlink actually belongs to.
    pub link_source_map: HashMap<Node<'a, 'i>, Relationships>,
    /// Source `w:r` nodes inside a `w:hyperlink`, i.e. Python's
    /// `x.set('is-link', '1')` -- a source-tree mutation, here a
    /// tracked side-table for the same reason `calibre_num_ids`/
    /// `removed_cells` are (see `docx::styles`/`docx::tables`'s module
    /// docs). Consumed by the not-yet-ported `Styles::cascade` (#285).
    pub is_link: HashSet<Node<'a, 'i>>,
}

impl<'a, 'i> ConvertState<'a, 'i> {
    pub fn new() -> Self {
        Self::default()
    }
}

static HEADING_STYLE_NAME_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^heading\s+(\d+)$").unwrap());

/// Converts one `w:p` into an HTML block element (`<p>`, or `<h1>`..`<h6>`
/// once a `"Heading N"` style name is detected), appending its
/// converted runs via [`convert_run`]. Returns the element's `NodeId`;
/// the caller is responsible for attaching it wherever the body's own
/// structure calls for (matching Python's `self.body.append(p)`).
///
/// # What's tracked here vs. left for later
///
/// Bookmark/anchor generation (`w:bookmarkStart`) and TOC-field
/// detection (`w:instrText` starting with `"TOC "`) are fully ported:
/// both are self-contained and produce real, immediately-visible `id`
/// attributes. Heading-level retagging, `dir="rtl"`, and the
/// same-bordered-run-merging pass (into a single wrapping `<span>`
/// with a `text_border` CSS class) are ported too.
///
/// `w:hyperlink` handling is **not** ported: Python's `convert_p`
/// tracks `current_hyperlink`/`link_map`/`link_source_map` and stamps
/// a synthetic `is-link` marker on the source run purely so a later
/// `resolve_links` pass (not yet ported) can retag spans into `<a>`
/// elements. Unlike `calibre_num_id`/`removed_cells` elsewhere in this
/// crate's docx port, `resolve_links`'s own Rust shape hasn't been
/// designed yet, so building tracking for it now risks getting that
/// shape wrong and redoing it -- deferred to `resolve_links`'s own
/// future PR, along with the tracking it needs. `w:r` elements nested
/// inside a `w:hyperlink` are still visited and converted normally
/// (the surrounding `w:hyperlink` element is just never itself
/// inspected), so plain text content is unaffected; only the
/// eventual `<a href=...>` wrapping is missing. `add_frame`/`frame_map`
/// (`apply_frames`'s bookkeeping) are deferred for the same reason.
///
/// # Two reproduced Python quirks in the border-run merge
///
/// - A span whose border *doesn't* match the run in progress is
///   silently dropped from every group: Python's `border_runs = []`
///   reset (on a mismatch) does not also re-seed the list with the
///   mismatching span, so it starts neither the old group nor the new
///   one.
/// - The final group is never flushed to `common_borders`: Python's
///   loop only ever appends to `common_borders` from inside the
///   mismatch branch, so a paragraph whose *last* run(s) share a
///   border with their predecessors never get merged.
///
/// Both are reproduced as-is rather than fixed -- see this crate's
/// established practice for calibre's own bugs (`docx::numbering`'s
/// module docs have another example).
///
/// Port of the Python `Convert.convert_p`.
#[allow(clippy::too_many_arguments)]
pub fn convert_p<'a, 'i>(
    dom: &mut Dom,
    state: &mut ConvertState<'a, 'i>,
    p: Node<'a, 'i>,
    styles: &mut Styles<'a, 'i>,
    footnotes: &mut Footnotes<'a, 'i>,
    settings: &Settings,
    theme: &Theme,
    doc_lang: Option<&str>,
    uuid: &str,
    rels: &Relationships,
    ns: &DocxNamespace,
) -> NodeId {
    let dest = dom.new_element("p");
    state.object_map.insert(dest, p);
    let style = styles.resolve_paragraph(p, ns);
    state.layers.insert(p, Vec::new());
    state.frame_map.insert(p, style.frame.clone());

    let mut current_anchor: Option<String> = None;

    for x in ns.descendants(p, &["w:r", "w:bookmarkStart", "w:instrText"]) {
        if ns.ancestor(x, "w:p") != Some(p) {
            // Nested `<w:p>` (a text box inside this paragraph, say)
            // -- its own descendants belong to *its* conversion, not
            // this one.
            continue;
        }
        if ns.is_tag(x, "w:r") {
            let span = convert_run(
                dom, x, styles, footnotes, settings, theme, doc_lang, uuid, ns,
            );
            if let Some(anchor) = current_anchor.take() {
                let target = if dom.children(dest).is_empty() {
                    dest
                } else {
                    span
                };
                dom.node_mut(target).attrs.insert("id".to_string(), anchor);
            }
            dom.append_child(dest, span);
            state.object_map.insert(span, x);
            state.layers.get_mut(&p).unwrap().push(x);
            // Port of the `current_hyperlink`/`hl_xpath` dance in
            // Python: it tracks a `current_hyperlink` flag across
            // iterations purely to skip an XPath call when a run is
            // obviously not inside any hyperlink, resetting the flag
            // lazily (only once a subsequent run's `ancestor::
            // w:hyperlink[1]` lookup comes back empty) rather than at
            // the hyperlink's actual closing tag. The flag never
            // substitutes for the lookup's own result -- it's a pure
            // performance guard with no observable effect on which
            // hyperlink (if any) a run gets attributed to. Calling
            // `ns.ancestor` unconditionally here is behaviorally
            // identical and needs no extra state.
            if let Some(hyperlink) = ns.ancestor(x, "w:hyperlink") {
                state.link_map.entry(hyperlink).or_default().push(span);
                state.link_source_map.insert(hyperlink, rels.clone());
                state.is_link.insert(x);
            }
        } else if ns.is_tag(x, "w:bookmarkStart") {
            if let Some(anchor) = ns
                .get(x, "w:name")
                .filter(|a| !a.is_empty() && *a != "_GoBack")
            {
                if !state.anchor_map.contains_key(anchor) {
                    apply_new_anchor(state, anchor.to_string(), &mut current_anchor);
                }
            }
        } else if ns.is_tag(x, "w:instrText") {
            if let Some(text) = x.text() {
                if text.trim_start().starts_with("TOC ") {
                    // Python keys this entry with a fresh `uuid.uuid4()`
                    // purely for a guaranteed-unique dict slot -- the
                    // key itself is never looked up again, only
                    // `self.toc_anchor` (the *value*) matters. A
                    // monotonic counter serves the same purpose without
                    // needing real randomness.
                    let synthetic_key = format!("\u{0}toc-instr-{}", state.anchor_map.len());
                    let generated = apply_new_anchor(state, synthetic_key, &mut current_anchor);
                    state.toc_anchor = Some(generated);
                }
            }
        }
    }

    if let Some(anchor) = current_anchor.take() {
        if dom.node(dest).attrs.contains_key("id") {
            let children = dom.children(dest);
            if let Some(&last) = children.last() {
                if let Some(existing_id) = dom.node(last).attrs.get("id").cloned() {
                    state.anchor_map.insert(anchor, existing_id);
                } else {
                    dom.node_mut(last).attrs.insert("id".to_string(), anchor);
                }
            } else {
                let dest_id = dom.node(dest).attrs.get("id").cloned().unwrap();
                state.anchor_map.insert(anchor, dest_id);
            }
        } else {
            dom.node_mut(dest).attrs.insert("id".to_string(), anchor);
        }
    }

    if let Some(name) = &style.style_name {
        if let Some(caps) = HEADING_STYLE_NAME_RE.captures(name) {
            if let Ok(n) = caps[1].parse::<i32>() {
                let n = n.clamp(1, 6);
                dom.set_tag(dest, &format!("h{n}"));
                dom.node_mut(dest)
                    .attrs
                    .insert("data-heading-level".to_string(), n.to_string());
            }
        }
    }

    if style.bidi == Some(true) {
        dom.node_mut(dest)
            .attrs
            .insert("dir".to_string(), "rtl".to_string());
    }

    let mut border_runs: Vec<(NodeId, Node<'a, 'i>, super::char_styles::RunStyle)> = Vec::new();
    let mut common_borders: Vec<Vec<(NodeId, Node<'a, 'i>, super::char_styles::RunStyle)>> =
        Vec::new();
    for span in dom.children(dest) {
        let run = state.object_map[&span];
        let run_style = styles.resolve_run(run, theme, ns);
        let matches = border_runs
            .last()
            .map(|(_, _, s)| s.same_border(&run_style))
            .unwrap_or(true);
        if matches {
            border_runs.push((span, run, run_style));
        } else if !border_runs.is_empty() {
            if border_runs.len() > 1 {
                common_borders.push(std::mem::take(&mut border_runs));
            } else {
                border_runs.clear();
            }
            // The mismatching span is dropped here, not re-seeded into
            // the fresh group -- see the module docs.
        }
    }

    for border_run in &common_borders {
        let mut bs = super::block_styles::Css::new();
        let mut spans = Vec::new();
        for (span, run, run_style) in border_run {
            run_style.border_css(&mut bs);
            styles.clear_run_border(*run);
            spans.push(*span);
        }
        if !bs.is_empty() {
            let cls = styles.register(bs, "text_border");
            let wrapper = wrap_elems(dom, dest, &spans);
            dom.node_mut(wrapper).attrs.insert("class".to_string(), cls);
        }
    }

    if dom.children(dest).is_empty() && !style.has_visible_border() {
        let t = dom.new_text("\u{a0}");
        dom.append_child(dest, t);
    }

    let children = dom.children(dest);
    if let Some(&last) = children.last() {
        if dom.tag(last) == Some("br") {
            // Unreachable in practice -- `dest`'s direct children are
            // always the `<span>`s `convert_run` returns, never a bare
            // `<br>` -- but Python's own check is structured this way,
            // so it's reproduced rather than trimmed.
            let t = dom.new_text("\u{a0}");
            dom.append_child(dest, t);
        } else {
            let inner_children = dom.children(last);
            if let Some(&inner_last) = inner_children.last() {
                if dom.tag(inner_last) == Some("br") {
                    let t = dom.new_text("\u{a0}");
                    dom.append_child(last, t);
                }
            }
        }
    }

    dest
}

/// Generates a fresh anchor for `key` (a bookmark name or synthetic
/// TOC-field key), records it in `state.anchor_map`, sets it as the
/// pending `current_anchor`, and redirects any earlier entry that
/// pointed at the previous pending anchor (which never got applied to
/// an element) onto the new one.
///
/// Port of the repeated `old_anchor = current_anchor; ...; if
/// old_anchor is not None: for a, t in ...` block in Python's
/// `convert_p`.
fn apply_new_anchor(
    state: &mut ConvertState,
    key: String,
    current_anchor: &mut Option<String>,
) -> String {
    let old_anchor = current_anchor.clone();
    let existing: HashSet<String> = state.anchor_map.values().cloned().collect();
    let new_anchor = super::names::generate_anchor(&key, &existing);
    state.anchor_map.insert(key, new_anchor.clone());
    *current_anchor = Some(new_anchor.clone());
    if let Some(old) = old_anchor {
        for t in state.anchor_map.values_mut() {
            if *t == old {
                *t = new_anchor.clone();
            }
        }
    }
    new_anchor
}

/// Moves `elems` (all direct children of `parent`) into a new
/// `wrapper` element inserted at the position of `elems[0]`.
///
/// Port of the Python `Convert.wrap_elems`. `crate::dom::Dom::append_child`
/// already detaches a node from its previous parent before attaching
/// it, so this needs no explicit removal step (unlike lxml, where
/// `parent.remove(elem)` and `.tail` redistribution are the caller's
/// job) -- and no `.tail`-carrying logic at all, per the module docs'
/// "No `Text` buffering" note.
fn wrap_elems(dom: &mut Dom, parent: NodeId, elems: &[NodeId]) -> NodeId {
    let idx = dom.index_in_parent(elems[0]).unwrap_or(0);
    let wrapper = dom.new_element("span");
    dom.insert_child(parent, idx, wrapper);
    for &e in elems {
        dom.append_child(wrapper, e);
    }
    wrapper
}

/// Walks `doc` (the `w:document` root) for every `w:p`/`w:tbl`,
/// grouping consecutive elements into the [`PageProperties`] of the
/// `w:sectPr` that ends their section, registering each `w:tbl`
/// encountered along the way, and recording each section's first
/// element in `section_starts`.
///
/// Returns `(page_map, section_starts)` in document order --
/// `page_map` an [`IndexMap`] since later callers (the main body-walk
/// loop, `apply_section_page_breaks`) depend on iteration order
/// matching Python's `OrderedDict`.
///
/// Port of the Python `Convert.read_page_properties`.
pub fn read_page_properties<'a, 'i>(
    doc: Node<'a, 'i>,
    styles: &mut Styles<'a, 'i>,
    ns: &DocxNamespace,
) -> (IndexMap<Node<'a, 'i>, PageProperties>, Vec<Node<'a, 'i>>) {
    let mut current: Vec<Node<'a, 'i>> = Vec::new();
    let mut page_map: IndexMap<Node<'a, 'i>, PageProperties> = IndexMap::new();
    let mut section_starts: Vec<Node<'a, 'i>> = Vec::new();

    for p in ns.descendants(doc, &["w:p", "w:tbl"]) {
        if ns.is_tag(p, "w:tbl") {
            styles.register_table(p, ns);
            current.push(p);
            continue;
        }
        let sect = ns.descendants(p, &["w:sectPr"]);
        if !sect.is_empty() {
            let pr = PageProperties::from_sect_prs(&sect, ns);
            current.push(p);
            for &x in &current {
                page_map.insert(x, pr);
            }
            section_starts.push(current[0]);
            current.clear();
        } else {
            current.push(p);
        }
    }

    if !current.is_empty() {
        section_starts.push(current[0]);
        let body = ns.first_child(doc, "w:body");
        let last = body
            .map(|b| ns.children(b, &["w:sectPr"]))
            .unwrap_or_default();
        let pr = PageProperties::from_sect_prs(&last, ns);
        for &x in &current {
            page_map.insert(x, pr);
        }
    }

    (page_map, section_starts)
}

/// Builds a `<body>` element under `dom`'s root and walks `doc` via
/// [`read_page_properties`], converting every `w:p` encountered (in
/// document order) via [`convert_p`] and appending it as a child --
/// skipping `w:tbl` entries, which `page_map` also carries but which
/// are only handled later, via `Tables::apply_markup` (not yet
/// ported). Finally applies [`Styles::apply_contextual_spacing`] to
/// every converted paragraph and [`Styles::apply_section_page_breaks`]
/// to every section but the first (matching Python's
/// `self.section_starts[1:]` -- the first section is already the
/// start of the document, so it needs no explicit page break).
///
/// Returns the `<body>` [`NodeId`] and the paragraphs converted, in
/// document order (Python's local `paras` list).
///
/// Port of the paragraph-walking half of `Convert.__call__`:
/// ```text
/// self.read_page_properties(doc)
/// self.current_rels = relationships_by_id
/// for wp, page_properties in self.page_map.items():
///     self.current_page = page_properties
///     if wp.tag.endswith('}p'):
///         p = self.convert_p(wp)
///         self.body.append(p)
///         paras.append(wp)
/// self.read_block_anchors(doc)
/// self.styles.apply_contextual_spacing(paras)
/// self.mark_block_runs(paras)
/// self.styles.apply_section_page_breaks(self.section_starts[1:])
/// ```
/// `self.current_page`/`page_properties` is threaded through as an
/// instance attribute but not read by any code ported so far (see the
/// module docs), so it isn't returned here. `read_block_anchors`
/// (resolves `w:bookmarkStart`/`w:bookmarkEnd` pairs that span
/// multiple paragraphs into cross-reference anchors) and
/// `mark_block_runs` (numbering-related run bookkeeping) are both
/// unported -- neither has a designed Rust shape yet, so, per this
/// module's established scoping principle, they're deferred rather
/// than guessed at here.
#[allow(clippy::too_many_arguments)]
pub fn convert_body<'a, 'i>(
    dom: &mut Dom,
    doc: Node<'a, 'i>,
    state: &mut ConvertState<'a, 'i>,
    styles: &mut Styles<'a, 'i>,
    footnotes: &mut Footnotes<'a, 'i>,
    settings: &Settings,
    theme: &Theme,
    doc_lang: Option<&str>,
    uuid: &str,
    rels: &Relationships,
    ns: &DocxNamespace,
) -> (NodeId, Vec<Node<'a, 'i>>) {
    let (page_map, section_starts) = read_page_properties(doc, styles, ns);

    let body = dom.new_element("body");
    dom.append_child(dom.root, body);

    let mut paras: Vec<Node<'a, 'i>> = Vec::new();
    for &wp in page_map.keys() {
        if ns.is_tag(wp, "w:p") {
            let p = convert_p(
                dom, state, wp, styles, footnotes, settings, theme, doc_lang, uuid, rels, ns,
            );
            dom.append_child(body, p);
            paras.push(wp);
        }
    }

    styles.apply_contextual_spacing(&paras, ns);
    if section_starts.len() > 1 {
        styles.apply_section_page_breaks(&section_starts[1..], ns);
    }

    (body, paras)
}

/// Assigns an `id` to every converted paragraph that a top-level (a
/// direct child of `w:body`) `w:bookmarkStart[@w:name]` immediately
/// precedes, and records each such bookmark name in
/// `state.anchor_map` pointing at that id -- letting later
/// cross-reference/hyperlink resolution (not yet ported) turn an
/// internal `w:anchor` reference into `href="#id"`.
///
/// A bookmark that lands *inside* a paragraph (on a run, mid-text) is
/// a different case entirely, already handled by [`convert_p`] itself
/// -- this only covers bookmarks Word placed as siblings of the
/// paragraphs they name, which is how Word marks e.g. a whole
/// heading. Skips (but does not drop) any pending bookmark names when
/// the immediately-following `w:p` wasn't itself converted (not
/// present in `state.object_map` -- e.g. it lives inside a table cell,
/// handled later by the unported `Tables::apply_markup`): the names
/// simply carry over to whichever converted paragraph comes next,
/// matching Python's own carry-over behaviour.
///
/// Port of the Python `Convert.read_block_anchors`. One deliberate
/// difference: Python picks an arbitrary name from a `set` (hash
/// order, effectively undefined) to seed the generated id when the
/// paragraph doesn't already have one; this always picks the first
/// bookmark name encountered in document order, a deterministic
/// choice with no semantic effect on the *set* of names mapped to
/// that id afterward -- only on which name happens to also become the
/// literal id text.
pub fn read_block_anchors<'a, 'i>(
    dom: &mut Dom,
    doc: Node<'a, 'i>,
    state: &mut ConvertState<'a, 'i>,
    ns: &DocxNamespace,
) {
    let Some(body) = ns.first_child(doc, "w:body") else {
        return;
    };
    let doc_anchors: HashSet<Node<'a, 'i>> = ns
        .children(body, &["w:bookmarkStart"])
        .into_iter()
        .filter(|&n| ns.get(n, "w:name").is_some())
        .collect();
    if doc_anchors.is_empty() {
        return;
    }

    let rmap: HashMap<Node<'a, 'i>, NodeId> =
        state.object_map.iter().map(|(&id, &n)| (n, id)).collect();

    let mut current_bm: Vec<String> = Vec::new();
    for p in ns.descendants(doc, &["w:p", "w:bookmarkStart"]) {
        if ns.is_tag(p, "w:p") {
            if current_bm.is_empty() {
                continue;
            }
            let Some(&para) = rmap.get(&p) else {
                continue;
            };
            if !dom.node(para).attrs.contains_key("id") {
                let existing: HashSet<String> = state.anchor_map.values().cloned().collect();
                let id = super::names::generate_anchor(&current_bm[0], &existing);
                dom.node_mut(para).attrs.insert("id".to_string(), id);
            }
            if let Some(id) = dom.node(para).attrs.get("id").cloned() {
                for name in current_bm.drain(..) {
                    state.anchor_map.insert(name, id.clone());
                }
            }
        } else if doc_anchors.contains(&p) {
            if let Some(anchor) = ns.get(p, "w:name") {
                if !current_bm.iter().any(|n| n == anchor) {
                    current_bm.push(anchor.to_string());
                }
            }
        }
    }
}

/// Rewrites a paragraph that starts with one or more `w:tab`-rendered
/// `<span class="tab">` elements (and nothing else before them) into
/// an equivalent `text-indent` CSS value, removing the now-redundant
/// leading tab spans -- Word documents commonly use leading tabs for
/// paragraph indentation instead of an actual `w:ind` setting.
///
/// Walks every converted paragraph in `state.object_map`. For each
/// one whose first run's first child is a `class="tab"` span, collects
/// the *leading run* of same-condition tab spans (stopping at the
/// first non-tab child, or at a tab span immediately followed by real
/// text -- that text becomes the paragraph's new leading text once the
/// tabs are gone). If the paragraph's resolved `text_indent` is either
/// unset or already a `"...pt"` value (added to, not replaced by, the
/// new indent), the leading tabs are removed and `text_indent` is
/// updated via [`Styles::set_paragraph_text_indent`]; otherwise (some
/// other CSS unit) the paragraph is left untouched.
///
/// Port of the tab-to-`text-indent` loop inside `Convert.__call__`:
/// ```text
/// for p, wp in self.object_map.items():
///     if len(p) > 0 and not p.text and len(p[0]) > 0 and not p[0].text \
///             and p[0][0].get('class', None) == 'tab':
///         parent = p[0]
///         tabs = []
///         for child in parent:
///             if child.get('class', None) == 'tab':
///                 tabs.append(child)
///                 if child.tail:
///                     break
///             else:
///                 break
///         indent = len(tabs) * self.settings.default_tab_stop
///         style = self.styles.resolve(wp)
///         if style.text_indent is inherit or (... and style.text_indent.endswith('pt')):
///             if style.text_indent is not inherit:
///                 indent = float(style.text_indent[:-2]) + indent
///             style.text_indent = f'{indent:.3g}pt'
///             parent.text = tabs[-1].tail or ''
///             for i in tabs:
///                 parent.remove(i)
/// ```
/// `crate::dom`'s sibling-text-node model has no `.text`/`.tail`
/// fields (see the module docs' "No `Text` buffering" note): a
/// lxml-style "does `p` have leading text before its first child
/// element" check becomes "is `p`'s first child, if any, itself an
/// element" here, and a tab span's "tail" is simply its next sibling,
/// when that sibling is a non-empty text node.
pub fn apply_tab_indentation<'a, 'i>(
    dom: &mut Dom,
    state: &ConvertState<'a, 'i>,
    styles: &mut Styles<'a, 'i>,
    settings: &Settings,
    ns: &DocxNamespace,
) {
    let entries: Vec<(NodeId, Node<'a, 'i>)> =
        state.object_map.iter().map(|(&id, &n)| (id, n)).collect();

    for (p, wp) in entries {
        let Some(&run_span) = dom.children(p).first() else {
            continue;
        };
        if !matches!(dom.node(run_span).kind, NodeKind::Element(_)) {
            continue;
        }
        let run_children = dom.children(run_span);
        let Some(&first) = run_children.first() else {
            continue;
        };
        if !matches!(dom.node(first).kind, NodeKind::Element(_)) {
            continue;
        }
        if dom.node(first).attrs.get("class").map(String::as_str) != Some("tab") {
            continue;
        }

        let mut tabs: Vec<NodeId> = Vec::new();
        let mut tail_node: Option<NodeId> = None;
        for (pos, &child) in run_children.iter().enumerate() {
            if !matches!(dom.node(child).kind, NodeKind::Element(_)) {
                continue;
            }
            if dom.node(child).attrs.get("class").map(String::as_str) != Some("tab") {
                break;
            }
            tabs.push(child);
            if let Some(&next) = run_children.get(pos + 1) {
                if let NodeKind::Text(t) = &dom.node(next).kind {
                    if !t.is_empty() {
                        tail_node = Some(next);
                        break;
                    }
                }
            }
        }
        if tabs.is_empty() {
            continue;
        }

        let mut indent = tabs.len() as f64 * settings.default_tab_stop;
        let style = styles.resolve_paragraph(wp, ns);
        let eligible = match &style.text_indent {
            None => true,
            Some(v) => v.ends_with("pt"),
        };
        if !eligible {
            continue;
        }
        if let Some(existing) = style
            .text_indent
            .as_deref()
            .and_then(|v| v.strip_suffix("pt"))
            .and_then(|n| n.parse::<f64>().ok())
        {
            indent += existing;
        }
        styles.set_paragraph_text_indent(wp, pt(indent));

        for &t in &tabs {
            dom.detach(t);
        }
        if let Some(tail) = tail_node {
            dom.insert_child(run_span, 0, tail);
        }
    }
}

/// Groups consecutive, same-frame, identically-bordered paragraphs
/// into maximal runs of 2+, and for each such run, collapses the
/// borders shared between adjacent paragraphs down to one visual
/// block: strips the redundant top/bottom border+margin from every
/// paragraph but the run's own first/last, replaces each internal
/// paragraph-to-paragraph boundary with a `between` rule, and records
/// `(border_style, run)` in `state.block_runs` -- consumed by the
/// not-yet-ported `apply_frames`, which wraps `run` in a `<div>`
/// carrying `border_style`'s CSS as its own border.
///
/// Port of `Convert.mark_block_runs`. `max_left`/`max_right`
/// (Python's `isinstance(style.margin_left, numbers.Number)` guard)
/// are omitted: `ParagraphStyle::margin_left`/`margin_right` are never
/// numeric in this codebase (`read_indent` always produces `None` or
/// an already-unit-formatted string), so that branch is provably dead
/// code here just as it is in Python -- `border_style.margin_left`/
/// `margin_right` always end up `"0"`, which this port sets directly.
pub fn mark_block_runs<'a, 'i>(
    state: &mut ConvertState<'a, 'i>,
    paras: &[Node<'a, 'i>],
    styles: &mut Styles<'a, 'i>,
    ns: &DocxNamespace,
) {
    let mut run: Vec<Node<'a, 'i>> = Vec::new();
    for &p in paras {
        if let Some(&last) = run.last() {
            if state.frame_map.get(&p) == state.frame_map.get(&last) {
                let style = styles.resolve_paragraph(p, ns);
                let last_style = styles.resolve_paragraph(last, ns);
                if style.has_identical_borders(&last_style) {
                    run.push(p);
                    continue;
                }
            }
        }
        if run.len() > 1 {
            process_block_run(&run, styles, ns, &mut state.block_runs);
        }
        run = vec![p];
    }
    if run.len() > 1 {
        process_block_run(&run, styles, ns, &mut state.block_runs);
    }
}

/// Port of `mark_block_runs`'s nested `process_run` closure. A free
/// function here since Rust closures can't capture `&mut Styles` and
/// `&mut Vec<_>` from the same enclosing scope across repeated calls
/// the way Python's closure captures `self`.
fn process_block_run<'a, 'i>(
    run: &[Node<'a, 'i>],
    styles: &mut Styles<'a, 'i>,
    ns: &DocxNamespace,
    block_runs: &mut Vec<(ParagraphStyle, Vec<Node<'a, 'i>>)>,
) {
    let mut has_visible_border: Option<bool> = None;
    let mut border_style: Option<ParagraphStyle> = None;
    let last = run.len() - 1;

    for (i, &p) in run.iter().enumerate() {
        let mut style = styles.resolve_paragraph(p, ns);
        let visible = *has_visible_border.get_or_insert_with(|| style.has_visible_border());

        if visible {
            style.margin_left = None;
            style.margin_right = None;
        }
        if i != 0 {
            style.borders.edge_mut(Edge::Top).padding = Some(0.0);
        } else {
            let mut bs = style.clone_border_styles();
            if visible {
                bs.margin_top = style.margin_top.take();
            }
            border_style = Some(bs);
        }
        if i != last {
            style.borders.edge_mut(Edge::Bottom).padding = Some(0.0);
        } else if visible {
            if let Some(bs) = border_style.as_mut() {
                bs.margin_bottom = style.margin_bottom.take();
            }
        }
        style.clear_borders();
        if i != last {
            style.apply_between_border();
        }

        styles.set_paragraph_style(p, style);
    }

    if has_visible_border == Some(true) {
        if let Some(mut bs) = border_style {
            bs.margin_left = Some("0".to_string());
            bs.margin_right = Some("0".to_string());
            block_runs.push((bs, run.to_vec()));
        }
    }
}

/// Turns every tracked `w:hyperlink` into a real `<a>`: merges its
/// runs' spans into one element (wrapping them if there's more than
/// one, matching the run's own span directly if there's only one),
/// relabels it `<a>`, and sets `href` from either the relationship
/// its `r:id` points at or an internal `w:anchor` resolved against
/// `state.anchor_map`.
///
/// Returns `hyperlink -> <a>` (Python's `self.resolved_link_map`),
/// which `toc.py`'s `create_toc` (issue #292) will need.
///
/// Port of the `self.link_map`-driven first block of
/// `Convert.resolve_links`. Two of Python's three link sources are
/// deliberately not ported here (see issue #283): `self.fields.
/// hyperlink_fields` (needs `fields.py`, issue #290) and
/// `self.images.links` (needs `images.py`, issue #289) -- both are
/// separate loops appended to this same method in Python once those
/// files exist, not a rewrite of this one. A hyperlink whose `r:id`/
/// `w:anchor` resolves to nothing is silently left without an `href`
/// rather than logged, since no logger is threaded through this
/// module yet.
pub fn resolve_links<'a, 'i>(
    dom: &mut Dom,
    state: &ConvertState<'a, 'i>,
    ns: &DocxNamespace,
) -> HashMap<Node<'a, 'i>, NodeId> {
    let mut resolved_link_map = HashMap::new();

    for (&hyperlink, spans) in &state.link_map {
        let Some(&first) = spans.first() else {
            continue;
        };
        let span = if spans.len() > 1 {
            let Some(parent) = dom.parent(first) else {
                continue;
            };
            wrap_elems(dom, parent, spans)
        } else {
            first
        };
        dom.set_tag(span, "a");
        resolved_link_map.insert(hyperlink, span);

        if let Some(tgt) = ns.get(hyperlink, "w:tgtFrame") {
            dom.node_mut(span)
                .attrs
                .insert("target".to_string(), tgt.to_string());
        }
        if let Some(tt) = ns.get(hyperlink, "w:tooltip") {
            dom.node_mut(span)
                .attrs
                .insert("title".to_string(), tt.to_string());
        }

        let href = ns.get(hyperlink, "r:id").and_then(|rid| {
            state
                .link_source_map
                .get(&hyperlink)
                .and_then(|rels| rels.by_id.get(rid))
                .cloned()
        });
        let href = href.or_else(|| {
            ns.get(hyperlink, "w:anchor")
                .and_then(|anchor| state.anchor_map.get(anchor))
                .map(|id| format!("#{id}"))
        });
        if let Some(href) = href {
            dom.node_mut(span).attrs.insert("href".to_string(), href);
        }
    }

    resolved_link_map
}

#[cfg(test)]
mod convert_run_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;

    fn parse_run(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(format!("<w:r {DOC_OPEN}>{body}</w:r>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn convert(&mut self, run: Node<'a, 'i>, ns: &DocxNamespace) -> NodeId {
            convert_run(
                &mut self.dom,
                run,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                ns,
            )
        }
    }

    #[test]
    fn plain_text_becomes_a_single_text_child() {
        let (doc, ns) = parse_run(r#"<w:t>hello world</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span>hello world</span>");
    }

    #[test]
    fn leading_and_trailing_whitespace_is_stripped_without_preserve() {
        let (doc, ns) = parse_run(r#"<w:t>  hi  </w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span>hi</span>");
    }

    #[test]
    fn xml_space_preserve_keeps_the_whitespace() {
        let (doc, ns) = parse_run(r#"<w:t xml:space="preserve">  hi  </w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        // Multiple spaces trigger the pre-wrap span wrapper.
        assert!(h.dom.serialize(span).contains("white-space:pre-wrap"));
        assert!(h.dom.serialize(span).contains("  hi  "));
    }

    #[test]
    fn consecutive_spaces_get_a_pre_wrap_span() {
        let (doc, ns) = parse_run(r#"<w:t xml:space="preserve">a  b</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.serialize(span),
            r#"<span><span style="white-space:pre-wrap">a  b</span></span>"#
        );
    }

    #[test]
    fn a_single_space_does_not_need_pre_wrap() {
        let (doc, ns) = parse_run(r#"<w:t xml:space="preserve">a b</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span>a b</span>");
    }

    #[test]
    fn cr_and_plain_br_render_as_br() {
        let (doc, ns) = parse_run("<w:cr/><w:br/>");
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span><br /><br /></span>");
    }

    #[test]
    fn page_break_br_gets_a_style() {
        let (doc, ns) = parse_run(r#"<w:br w:type="page"/>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.serialize(span),
            r#"<span><br style="page-break-after:always" /></span>"#
        );
    }

    #[test]
    fn clear_br_gets_a_clear_style() {
        let (doc, ns) = parse_run(r#"<w:br clear="left"/>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.serialize(span),
            r#"<span><br style="clear:left" /></span>"#
        );

        let (doc, ns) = parse_run(r#"<w:br clear="all"/>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.serialize(span),
            r#"<span><br style="clear:both" /></span>"#
        );
    }

    #[test]
    fn tab_renders_nbsp_count_from_default_tab_stop() {
        let (doc, ns) = parse_run("<w:tab/>");
        let mut h = Harness::new();
        // default_tab_stop defaults to 36pt: ceil((36/36)*6) = 6 nbsp.
        let span = h.convert(doc.root_element(), &ns);
        let nbsp = "\u{a0}".repeat(6);
        assert_eq!(
            h.dom.serialize(span),
            format!(r#"<span><span class="tab">{nbsp}</span></span>"#)
        );
    }

    #[test]
    fn hyphen_variants_render_the_special_characters() {
        let (doc, ns) = parse_run("<w:noBreakHyphen/><w:softHyphen/>");
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span>\u{2011}\u{ad}</span>");
    }

    #[test]
    fn drawing_and_pict_children_are_skipped() {
        let (doc, ns) = parse_run("<w:drawing/><w:pict/><w:t>after</w:t>");
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span>after</span>");
    }

    #[test]
    fn footnote_reference_becomes_a_noteref_link() {
        let (doc, ns) = parse_run(r#"<w:footnoteReference w:id="7"/>"#);
        let mut h = Harness::new();

        // Register a real footnote with id "7" so get_ref resolves it.
        let notes_xml: &'static str = Box::leak(
            format!(
                r#"<w:footnotes {DOC_OPEN}><w:footnote w:id="7"><w:p/></w:footnote></w:footnotes>"#
            )
            .into_boxed_str(),
        );
        let notes_doc = Box::leak(Box::new(Document::parse(notes_xml).unwrap()));
        h.footnotes.load(
            Some(notes_doc.root_element()),
            std::rc::Rc::new(Default::default()),
            None,
            std::rc::Rc::new(Default::default()),
            &ns,
        );

        let span = h.convert(doc.root_element(), &ns);
        let html = h.dom.serialize(span);
        assert!(html.contains(r#"class="noteref""#));
        assert!(html.contains(r#"role="doc-noteref""#));
        assert!(html.contains(r#"id="back_note_1""#));
        assert!(html.contains("href=\"#note_1\""));
        assert!(html.contains(">1<"), "displays the assigned note number");
        assert_eq!(
            h.dom
                .node(span)
                .attrs
                .get("data-noteref-container")
                .map(String::as_str),
            Some("test-uuid")
        );
    }

    #[test]
    fn an_unresolvable_footnote_reference_produces_nothing() {
        let (doc, ns) = parse_run(r#"<w:footnoteReference w:id="99"/>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(span), "<span></span>");
    }

    #[test]
    fn vert_align_is_only_set_when_the_span_has_content() {
        let (doc, ns) =
            parse_run(r#"<w:rPr><w:vertAlign w:val="superscript"/></w:rPr><w:t>x</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom
                .node(span)
                .attrs
                .get("data-docx-vert")
                .map(String::as_str),
            Some("sup")
        );

        let (doc, ns) = parse_run(r#"<w:rPr><w:vertAlign w:val="subscript"/></w:rPr>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.node(span).attrs.get("data-docx-vert"),
            None,
            "an empty run gets no vert-align marker"
        );
    }

    #[test]
    fn rtl_sets_dir_attribute() {
        let (doc, ns) = parse_run(r#"<w:rPr><w:rtl/></w:rPr><w:t>x</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.node(span).attrs.get("dir").map(String::as_str),
            Some("rtl")
        );
    }

    #[test]
    fn lang_is_set_when_it_differs_from_the_document_language() {
        let (doc, ns) = parse_run(r#"<w:rPr><w:lang w:val="fr-FR"/></w:rPr><w:t>x</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.node(span).attrs.get("lang").map(String::as_str),
            Some("fr-FR")
        );
    }

    #[test]
    fn lang_matching_the_document_language_is_not_repeated() {
        let (doc, ns) = parse_run(r#"<w:rPr><w:lang w:val="en-US"/></w:rPr><w:t>x</w:t>"#);
        let mut h = Harness::new();
        let span = convert_run(
            &mut h.dom,
            doc.root_element(),
            &mut h.styles,
            &mut h.footnotes,
            &h.settings,
            &h.theme,
            Some("en-US"),
            "test-uuid",
            &ns,
        );
        assert_eq!(h.dom.node(span).attrs.get("lang"), None);
    }

    #[test]
    fn symbol_font_text_is_remapped_and_font_family_becomes_sans_serif() {
        let (doc, ns) =
            parse_run(r#"<w:rPr><w:rFonts w:ascii="Wingdings"/></w:rPr><w:t>&#xf0fc;</w:t>"#);
        let mut h = Harness::new();
        let span = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.serialize(span),
            "<span>\u{2713}</span>",
            "U+F0FC maps to a checkmark"
        );

        // The persisted cache reflects the sans-serif override, not the
        // literal "Wingdings" -- confirms set_run_font_family wrote back.
        let cached = h.styles.resolve_run(doc.root_element(), &h.theme, &ns);
        assert_eq!(cached.font_family.as_deref(), Some("sans-serif"));
    }
}

#[cfg(test)]
mod convert_p_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;

    fn parse_para(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(format!("<w:p {DOC_OPEN}>{body}</w:p>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn convert(&mut self, p: Node<'a, 'i>, ns: &DocxNamespace) -> NodeId {
            convert_p(
                &mut self.dom,
                &mut self.state,
                p,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                &Relationships::default(),
                ns,
            )
        }
    }

    #[test]
    fn a_paragraph_with_two_runs_becomes_a_p_with_two_spans() {
        let (doc, ns) = parse_para("<w:r><w:t>hello </w:t></w:r><w:r><w:t>world</w:t></w:r>");
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.tag(dest), Some("p"));
        assert_eq!(h.dom.children(dest).len(), 2);
        assert_eq!(
            h.dom.serialize(dest),
            "<p><span>hello</span><span>world</span></p>"
        );
        assert_eq!(h.state.object_map.get(&dest), Some(&doc.root_element()));
        assert_eq!(h.state.layers[&doc.root_element()].len(), 2);
    }

    #[test]
    fn heading_style_name_retags_the_element() {
        let (doc, ns) =
            parse_para(r#"<w:pPr><w:pStyle w:val="H1"/></w:pPr><w:r><w:t>Title</w:t></w:r>"#);
        let mut h = Harness::new();
        let mut named = std::collections::HashMap::new();
        let mut style = super::super::styles::Style::default();
        style.name = Some("Heading 1".to_string());
        named.insert("H1".to_string(), style);
        h.styles.call(None, &ns);
        h.styles
            .id_map
            .insert("H1".to_string(), named.remove("H1").unwrap());

        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.tag(dest), Some("h1"));
        assert_eq!(
            h.dom
                .node(dest)
                .attrs
                .get("data-heading-level")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn non_heading_style_name_leaves_the_tag_as_p() {
        let (doc, ns) = parse_para(r#"<w:r><w:t>Body</w:t></w:r>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.tag(dest), Some("p"));
        assert_eq!(h.dom.node(dest).attrs.get("data-heading-level"), None);
    }

    #[test]
    fn bidi_paragraph_gets_rtl_dir() {
        let (doc, ns) = parse_para(r#"<w:pPr><w:bidi/></w:pPr><w:r><w:t>x</w:t></w:r>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.node(dest).attrs.get("dir").map(String::as_str),
            Some("rtl")
        );
    }

    #[test]
    fn an_empty_paragraph_with_no_visible_border_gets_a_nbsp() {
        let (doc, ns) = parse_para("");
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(h.dom.serialize(dest), "<p>\u{a0}</p>");
    }

    #[test]
    fn bookmark_start_before_the_first_run_sets_id_on_dest_itself() {
        // Python checks `len(dest) == 0` *before* appending the span
        // that triggered the pending anchor -- so a bookmark at the
        // very start of a paragraph lands on `dest` (the `<p>`
        // itself), not the first span.
        let (doc, ns) =
            parse_para(r#"<w:bookmarkStart w:id="0" w:name="anchor1"/><w:r><w:t>x</w:t></w:r>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert!(h.dom.node(dest).attrs.contains_key("id"));
        assert_eq!(
            h.state.anchor_map.get("anchor1"),
            h.dom.node(dest).attrs.get("id")
        );
    }

    #[test]
    fn bookmark_start_between_two_runs_sets_id_on_the_following_span() {
        let (doc, ns) = parse_para(
            r#"<w:r><w:t>a</w:t></w:r><w:bookmarkStart w:id="0" w:name="anchor1"/><w:r><w:t>b</w:t></w:r>"#,
        );
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        let second_span = h.dom.children(dest)[1];
        assert!(h.dom.node(second_span).attrs.contains_key("id"));
        assert_eq!(
            h.state.anchor_map.get("anchor1"),
            h.dom.node(second_span).attrs.get("id")
        );
    }

    #[test]
    fn bookmark_start_with_no_following_run_sets_id_on_dest_itself() {
        let (doc, ns) = parse_para(r#"<w:bookmarkStart w:id="0" w:name="anchor1"/>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        // Empty-paragraph NBSP still applies; the anchor lands on dest.
        assert!(h.dom.node(dest).attrs.contains_key("id"));
    }

    #[test]
    fn go_back_bookmark_is_ignored() {
        let (doc, ns) =
            parse_para(r#"<w:bookmarkStart w:id="0" w:name="_GoBack"/><w:r><w:t>x</w:t></w:r>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        let span = h.dom.children(dest)[0];
        assert!(!h.dom.node(span).attrs.contains_key("id"));
        assert!(h.state.anchor_map.is_empty());
    }

    #[test]
    fn toc_instr_text_sets_toc_anchor() {
        let (doc, ns) = parse_para(
            r#"<w:r><w:instrText>TOC \o "1-3" \h \z \u</w:instrText></w:r><w:r><w:t>x</w:t></w:r>"#,
        );
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns);
        assert!(h.state.toc_anchor.is_some());
    }

    #[test]
    fn nested_paragraph_content_is_not_absorbed() {
        // A `w:p` nested inside this one (a textbox) shouldn't have its
        // own runs pulled into the outer conversion.
        let (doc, ns) = parse_para(
            r#"<w:r><w:t>outer</w:t></w:r><w:pict><w:p><w:r><w:t>inner</w:t></w:r></w:p></w:pict>"#,
        );
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        assert_eq!(
            h.dom.children(dest).len(),
            1,
            "only the outer run is converted"
        );
        assert_eq!(h.dom.serialize(dest), "<p><span>outer</span></p>");
    }

    #[test]
    fn a_trailing_br_inside_the_last_span_gets_a_nbsp_tail() {
        let (doc, ns) = parse_para(r#"<w:r><w:t>text</w:t><w:br/></w:r>"#);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        let last_span = *h.dom.children(dest).last().unwrap();
        let span_children = h.dom.children(last_span);
        // text, br, then a trailing nbsp text node.
        assert_eq!(span_children.len(), 3);
        let last = *span_children.last().unwrap();
        assert!(matches!(&h.dom.node(last).kind, NodeKind::Text(t) if t == "\u{a0}"));
    }

    #[test]
    fn two_runs_sharing_a_border_are_wrapped_but_the_trailing_group_is_not() {
        let border =
            r#"<w:rPr><w:bdr w:val="single" w:sz="8" w:space="0" w:color="000000"/></w:rPr>"#;
        let (doc, ns) = parse_para(&format!(
            "<w:r>{border}<w:t>a</w:t></w:r><w:r>{border}<w:t>b</w:t></w:r>"
        ));
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        // Both runs share a border and are the *only* group -- since it
        // never hits a mismatch, Python's loop never flushes it either.
        // No wrapper is produced; this documents the reproduced quirk.
        let html = h.dom.serialize(dest);
        assert!(
            !html.contains("text_border"),
            "trailing-only group is never flushed: {html}"
        );
    }

    #[test]
    fn a_bordered_group_followed_by_a_mismatch_and_more_bordered_runs_only_wraps_the_first_group() {
        let border =
            r#"<w:rPr><w:bdr w:val="single" w:sz="8" w:space="0" w:color="000000"/></w:rPr>"#;
        let plain = "<w:rPr/>";
        let (doc, ns) = parse_para(&format!(
            "<w:r>{border}<w:t>a</w:t></w:r><w:r>{border}<w:t>b</w:t></w:r><w:r>{plain}<w:t>c</w:t></w:r><w:r>{border}<w:t>d</w:t></w:r><w:r>{border}<w:t>e</w:t></w:r>"
        ));
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns);
        let html = h.dom.serialize(dest);
        // First group (a, b) flushes on hitting the mismatch (c) and
        // gets wrapped. `c` itself is dropped from every group (the
        // "mismatching span" quirk). The trailing group (d, e) is
        // never flushed (the "no final flush" quirk), so only one
        // text_border class is registered.
        assert_eq!(html.matches("text_border").count(), 1, "{html}");
        assert!(
            html.contains(">c<"),
            "the mismatching run's own text still renders: {html}"
        );
    }
}

#[cfg(test)]
mod read_page_properties_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str =
        r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    fn parse_doc(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(
            format!("<w:document {DOC_OPEN}><w:body>{body}</w:body></w:document>").into_boxed_str(),
        );
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    fn sect_pr(width: u32) -> String {
        format!(r#"<w:sectPr><w:pgSz w:w="{width}" w:h="16838"/></w:sectPr>"#)
    }

    #[test]
    fn every_paragraph_before_the_final_section_break_maps_to_that_sections_properties() {
        let (doc, ns) = parse_doc(&format!(
            "<w:p><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r>{}</w:p>",
            sect_pr(1000)
        ));
        let mut styles = Styles::new(Tables::default());
        let root = doc.root_element();
        let (page_map, section_starts) = read_page_properties(root, &mut styles, &ns);

        let ps: Vec<Node> = ns.descendants(root, &["w:p"]);
        assert_eq!(page_map.len(), 2);
        assert!((page_map[&ps[0]].width - 50.0).abs() < 0.01);
        assert!((page_map[&ps[1]].width - 50.0).abs() < 0.01);
        assert_eq!(section_starts, vec![ps[0]]);
    }

    #[test]
    fn trailing_paragraphs_after_the_last_section_break_use_the_body_level_sectpr() {
        let (doc, ns) = parse_doc(&format!(
            "<w:p><w:r><w:t>a</w:t></w:r>{}</w:p><w:p><w:r><w:t>b</w:t></w:r></w:p>{}",
            sect_pr(1000),
            sect_pr(2000)
        ));
        let mut styles = Styles::new(Tables::default());
        let root = doc.root_element();
        let (page_map, section_starts) = read_page_properties(root, &mut styles, &ns);

        let ps: Vec<Node> = ns.descendants(root, &["w:p"]);
        assert!((page_map[&ps[0]].width - 50.0).abs() < 0.01);
        assert!((page_map[&ps[1]].width - 100.0).abs() < 0.01);
        assert_eq!(section_starts, vec![ps[0], ps[1]]);
    }

    #[test]
    fn a_table_before_a_section_break_paragraph_is_registered_and_mapped() {
        let (doc, ns) = parse_doc(&format!(
            "<w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p><w:r><w:t>a</w:t></w:r>{}</w:p>",
            sect_pr(1000)
        ));
        let mut styles = Styles::new(Tables::default());
        let root = doc.root_element();
        let (page_map, section_starts) = read_page_properties(root, &mut styles, &ns);

        let tbl = ns.descendants(root, &["w:tbl"])[0];
        // `descendants` walks the whole tree, so the table's own inner
        // (empty) cell paragraph is a separate match from the
        // top-level paragraph carrying the section break -- both land
        // in `page_map`, matching Python's `namespace.descendants`
        // (not clipped to skip a matched `w:tbl`'s own children).
        let ps: Vec<Node> = ns.descendants(root, &["w:p"]);
        assert_eq!(ps.len(), 2);
        assert_eq!(page_map.len(), 3);
        assert!(page_map.contains_key(&tbl));
        assert!((page_map[&ps[0]].width - 50.0).abs() < 0.01);
        assert!((page_map[&ps[1]].width - 50.0).abs() < 0.01);
        assert_eq!(section_starts, vec![tbl]);
    }

    #[test]
    fn no_section_breaks_at_all_maps_everything_to_the_body_sectpr() {
        let (doc, ns) = parse_doc(&format!(
            "<w:p><w:r><w:t>a</w:t></w:r></w:p><w:p><w:r><w:t>b</w:t></w:r></w:p>{}",
            sect_pr(3000)
        ));
        let mut styles = Styles::new(Tables::default());
        let root = doc.root_element();
        let (page_map, section_starts) = read_page_properties(root, &mut styles, &ns);

        let ps: Vec<Node> = ns.descendants(root, &["w:p"]);
        assert_eq!(page_map.len(), 2);
        assert!((page_map[&ps[0]].width - 150.0).abs() < 0.01);
        assert!((page_map[&ps[1]].width - 150.0).abs() < 0.01);
        assert_eq!(section_starts, vec![ps[0]]);
    }
}

#[cfg(test)]
mod convert_body_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;

    fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:root {DOC_OPEN}>{body}</w:root>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn convert(
            &mut self,
            doc: Node<'a, 'i>,
            ns: &DocxNamespace,
        ) -> (NodeId, Vec<Node<'a, 'i>>) {
            convert_body(
                &mut self.dom,
                doc,
                &mut self.state,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                &Relationships::default(),
                ns,
            )
        }
    }

    #[test]
    fn every_paragraph_becomes_a_body_child_in_document_order() {
        let (doc, ns) = parse_root(
            "<w:document><w:body>\
               <w:p><w:r><w:t>one</w:t></w:r></w:p>\
               <w:p><w:r><w:t>two</w:t></w:r></w:p>\
             </w:body></w:document>",
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (body, paras) = h.convert(document, &ns);

        assert_eq!(h.dom.tag(body), Some("body"));
        let children = h.dom.children(body);
        assert_eq!(children.len(), 2);
        assert_eq!(h.dom.serialize(children[0]), "<p><span>one</span></p>");
        assert_eq!(h.dom.serialize(children[1]), "<p><span>two</span></p>");
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn a_table_is_present_in_page_map_but_produces_no_body_child() {
        let (doc, ns) = parse_root(
            "<w:document><w:body>\
               <w:tbl><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>\
               <w:p><w:r><w:t>after</w:t></w:r></w:p>\
             </w:body></w:document>",
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (body, paras) = h.convert(document, &ns);

        // The table's own (empty) cell paragraph is a real `w:p` and
        // does get converted -- only the `w:tbl` element itself is
        // skipped by the body walk (`wp.tag.endswith('}p')` in
        // Python), matching `read_page_properties`'s own descendants
        // walk finding both.
        let children = h.dom.children(body);
        assert_eq!(children.len(), 2);
        // Empty paragraphs get a non-breaking space so they don't
        // visually collapse -- see `convert_p`'s own tests.
        assert_eq!(h.dom.serialize(children[0]), "<p>\u{a0}</p>");
        assert_eq!(h.dom.serialize(children[1]), "<p><span>after</span></p>");
        assert_eq!(paras.len(), 2);
    }

    #[test]
    fn contextual_spacing_is_applied_across_the_converted_paragraphs() {
        let xml: &'static str = Box::leak(
            format!(
                r#"<w:root {DOC_OPEN}>
                     <w:styles>
                       <w:style w:type="paragraph" w:styleId="Body">
                         <w:pPr><w:contextualSpacing/></w:pPr>
                       </w:style>
                     </w:styles>
                     <w:document><w:body>
                       <w:p><w:pPr><w:pStyle w:val="Body"/></w:pPr></w:p>
                       <w:p><w:pPr><w:pStyle w:val="Body"/></w:pPr></w:p>
                     </w:body></w:document>
                   </w:root>"#
            )
            .into_boxed_str(),
        );
        let doc = Document::parse(xml).expect("valid XML");
        let ns = DocxNamespace::default();
        let styles_root = ns.first_child(doc.root_element(), "w:styles").unwrap();
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();

        let mut h = Harness::new();
        h.styles.call(Some(styles_root), &ns);
        let (_body, paras) = h.convert(document, &ns);

        assert_eq!(paras.len(), 2);
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        let second = h.styles.resolve_paragraph(paras[1], &ns);
        assert_eq!(first.margin_bottom.as_deref(), Some("0"));
        assert_eq!(second.margin_top.as_deref(), Some("0"));
    }

    #[test]
    fn every_section_but_the_first_gets_a_leading_page_break() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:t>a</w:t></w:r><w:sectPr><w:pgSz w:w="1000" w:h="16838"/></w:sectPr></w:p>
                 <w:p><w:r><w:t>b</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (_body, paras) = h.convert(document, &ns);

        assert_eq!(paras.len(), 2);
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        let second = h.styles.resolve_paragraph(paras[1], &ns);
        assert_ne!(
            first.page_break_before,
            Some(true),
            "the first section starts the document -- no leading break"
        );
        assert_eq!(
            second.page_break_before,
            Some(true),
            "the second section's start paragraph gets a leading break"
        );
    }
}

#[cfg(test)]
mod read_block_anchors_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;

    fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:root {DOC_OPEN}>{body}</w:root>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn body(&mut self, doc: Node<'a, 'i>, ns: &DocxNamespace) -> NodeId {
            convert_body(
                &mut self.dom,
                doc,
                &mut self.state,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                &Relationships::default(),
                ns,
            )
            .0
        }
    }

    #[test]
    fn a_top_level_bookmark_before_a_paragraph_assigns_an_id_and_maps_the_name() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:bookmarkStart w:id="0" w:name="chap1"/>
                 <w:p><w:r><w:t>hi</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        let p = h.dom.children(body)[0];
        let id = h.dom.node(p).attrs.get("id").cloned().expect("id was set");
        assert_eq!(h.state.anchor_map.get("chap1"), Some(&id));
    }

    #[test]
    fn two_top_level_bookmarks_before_one_paragraph_both_map_to_its_single_id() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:bookmarkStart w:id="0" w:name="chap1"/>
                 <w:bookmarkStart w:id="1" w:name="intro"/>
                 <w:p><w:r><w:t>hi</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        let p = h.dom.children(body)[0];
        let id = h.dom.node(p).attrs.get("id").cloned().expect("id was set");
        assert_eq!(h.state.anchor_map.get("chap1"), Some(&id));
        assert_eq!(h.state.anchor_map.get("intro"), Some(&id));
    }

    #[test]
    fn a_paragraph_that_already_has_an_id_is_not_overwritten() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:bookmarkStart w:id="0" w:name="chap1"/>
                 <w:p><w:bookmarkStart w:id="1" w:name="inner"/><w:r><w:t>hi</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        let p = h.dom.children(body)[0];
        let existing_id = h
            .dom
            .node(p)
            .attrs
            .get("id")
            .cloned()
            .expect("convert_p already set an id from the inline bookmark");

        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        assert_eq!(h.dom.node(p).attrs.get("id"), Some(&existing_id));
        assert_eq!(h.state.anchor_map.get("chap1"), Some(&existing_id));
    }

    #[test]
    fn a_pending_bookmark_carries_over_to_the_next_converted_paragraph_when_one_is_skipped() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:bookmarkStart w:id="0" w:name="chap1"/>
                 <w:p><w:r><w:t>skipped</w:t></w:r></w:p>
                 <w:p><w:r><w:t>kept</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        let children = h.dom.children(body);
        let (first_p, second_p) = (children[0], children[1]);
        // Simulate the first paragraph never having been converted
        // (e.g. content `object_map` never reaches) by dropping its
        // entry -- proves the pending bookmark carries over rather
        // than being discarded, matching Python's own carry-over.
        h.state.object_map.shift_remove(&first_p);

        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        assert!(h.dom.node(first_p).attrs.get("id").is_none());
        let id = h
            .dom
            .node(second_p)
            .attrs
            .get("id")
            .cloned()
            .expect("id was set");
        assert_eq!(h.state.anchor_map.get("chap1"), Some(&id));
    }

    #[test]
    fn a_bookmark_nested_inside_a_paragraph_is_not_treated_as_a_top_level_anchor() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:bookmarkStart w:id="0" w:name="inner"/><w:r><w:t>x</w:t></w:r></w:p>
                 <w:p><w:r><w:t>y</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        // "inner" was already resolved by `convert_p` onto the first
        // paragraph -- it was never a pending top-level bookmark, so
        // the second paragraph gets no id from `read_block_anchors`.
        let second_p = h.dom.children(body)[1];
        assert!(h.dom.node(second_p).attrs.get("id").is_none());
    }

    #[test]
    fn no_top_level_bookmarks_is_a_no_op() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:t>a</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        read_block_anchors(&mut h.dom, document, &mut h.state, &ns);

        let p = h.dom.children(body)[0];
        assert!(h.dom.node(p).attrs.get("id").is_none());
        assert!(h.state.anchor_map.is_empty());
    }
}

#[cfg(test)]
mod apply_tab_indentation_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;

    fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:root {DOC_OPEN}>{body}</w:root>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn body(&mut self, doc: Node<'a, 'i>, ns: &DocxNamespace) -> NodeId {
            convert_body(
                &mut self.dom,
                doc,
                &mut self.state,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                &Relationships::default(),
                ns,
            )
            .0
        }
    }

    #[test]
    fn two_leading_tabs_become_a_text_indent_and_are_removed() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:tab/><w:tab/><w:t>hello</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        apply_tab_indentation(&mut h.dom, &h.state, &mut h.styles, &h.settings, &ns);

        let p = h.dom.children(body)[0];
        assert_eq!(h.dom.serialize(p), "<p><span>hello</span></p>");

        let wp = *h.state.object_map.get(&p).unwrap();
        let style = h.styles.resolve_paragraph(wp, &ns);
        assert_eq!(style.text_indent.as_deref(), Some("72pt"));
    }

    #[test]
    fn an_existing_pt_text_indent_gets_the_tab_based_indent_added_to_it() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:pPr><w:ind w:firstLine="240"/></w:pPr>
                   <w:r><w:tab/><w:t>hello</w:t></w:r>
                 </w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        apply_tab_indentation(&mut h.dom, &h.state, &mut h.styles, &h.settings, &ns);

        let p = h.dom.children(body)[0];
        assert_eq!(h.dom.serialize(p), "<p><span>hello</span></p>");

        let wp = *h.state.object_map.get(&p).unwrap();
        let style = h.styles.resolve_paragraph(wp, &ns);
        // 12pt (w:firstLine="240" twips) + 36pt (one tab) = 48pt.
        assert_eq!(style.text_indent.as_deref(), Some("48pt"));
    }

    #[test]
    fn a_non_pt_text_indent_is_left_untouched() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:pPr><w:ind w:firstLineChars="100"/></w:pPr>
                   <w:r><w:tab/><w:t>hello</w:t></w:r>
                 </w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        apply_tab_indentation(&mut h.dom, &h.state, &mut h.styles, &h.settings, &ns);

        let p = h.dom.children(body)[0];
        // The tab span survives untouched -- an "em" indent isn't
        // eligible for the tab-merge, matching Python.
        let run_span = h.dom.children(p)[0];
        assert_eq!(h.dom.tag(h.dom.children(run_span)[0]), Some("span"));
        assert_eq!(
            h.dom
                .node(h.dom.children(run_span)[0])
                .attrs
                .get("class")
                .map(String::as_str),
            Some("tab")
        );

        let wp = *h.state.object_map.get(&p).unwrap();
        let style = h.styles.resolve_paragraph(wp, &ns);
        assert_eq!(style.text_indent.as_deref(), Some("1em"));
    }

    #[test]
    fn a_paragraph_not_starting_with_a_tab_is_left_untouched() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:t>hello</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        apply_tab_indentation(&mut h.dom, &h.state, &mut h.styles, &h.settings, &ns);

        let p = h.dom.children(body)[0];
        assert_eq!(h.dom.serialize(p), "<p><span>hello</span></p>");
    }

    #[test]
    fn a_tab_followed_by_a_non_tab_element_stops_collection_there() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:tab/><w:br/><w:t>after</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let body = h.body(document, &ns);
        apply_tab_indentation(&mut h.dom, &h.state, &mut h.styles, &h.settings, &ns);

        let p = h.dom.children(body)[0];
        let run_span = h.dom.children(p)[0];
        let run_children = h.dom.children(run_span);
        // The tab is gone, but the <br> (a non-tab element) that
        // stopped collection -- and anything after it -- is left in
        // place rather than being folded into a new leading text node
        // (there was no tail text on the removed tab to fold in).
        assert_eq!(run_children.len(), 2);
        assert_eq!(h.dom.tag(run_children[0]), Some("br"));

        let wp = *h.state.object_map.get(&p).unwrap();
        let style = h.styles.resolve_paragraph(wp, &ns);
        assert_eq!(style.text_indent.as_deref(), Some("36pt"));
    }
}

#[cfg(test)]
mod mark_block_runs_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace""#;
    const BORDER: &str = r#"<w:pBdr><w:top w:val="single" w:sz="8" w:color="FF0000"/><w:bottom w:val="single" w:sz="8" w:color="FF0000"/></w:pBdr>"#;

    fn parse_root(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str =
            Box::leak(format!("<w:root {DOC_OPEN}>{body}</w:root>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn body(&mut self, doc: Node<'a, 'i>, ns: &DocxNamespace) -> (NodeId, Vec<Node<'a, 'i>>) {
            convert_body(
                &mut self.dom,
                doc,
                &mut self.state,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                &Relationships::default(),
                ns,
            )
        }
    }

    #[test]
    fn two_identically_bordered_paragraphs_merge_into_one_block_run() {
        let (doc, ns) = parse_root(&format!(
            r#"<w:document><w:body>
                 <w:p><w:pPr>{BORDER}</w:pPr><w:r><w:t>a</w:t></w:r></w:p>
                 <w:p><w:pPr>{BORDER}</w:pPr><w:r><w:t>b</w:t></w:r></w:p>
               </w:body></w:document>"#
        ));
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (_body, paras) = h.body(document, &ns);
        mark_block_runs(&mut h.state, &paras, &mut h.styles, &ns);

        assert_eq!(h.state.block_runs.len(), 1);
        let (border_style, run) = &h.state.block_runs[0];
        assert_eq!(run, &paras);
        assert!(border_style.has_visible_border());
        assert_eq!(border_style.margin_left.as_deref(), Some("0"));
        assert_eq!(border_style.margin_right.as_deref(), Some("0"));

        // Each paragraph's own border was stripped -- it now lives on
        // border_style, for the not-yet-ported apply_frames to render
        // as the wrapping <div>'s border instead.
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        let second = h.styles.resolve_paragraph(paras[1], &ns);
        assert!(!first.has_visible_border());
        assert!(!second.has_visible_border());
        // The internal boundary's padding is zeroed on both sides.
        assert_eq!(first.borders.bottom.padding, Some(0.0));
        assert_eq!(second.borders.top.padding, Some(0.0));
    }

    #[test]
    fn differently_bordered_paragraphs_do_not_merge() {
        let (doc, ns) = parse_root(&format!(
            r#"<w:document><w:body>
                 <w:p><w:pPr>{BORDER}</w:pPr><w:r><w:t>a</w:t></w:r></w:p>
                 <w:p><w:r><w:t>b</w:t></w:r></w:p>
               </w:body></w:document>"#
        ));
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (_body, paras) = h.body(document, &ns);
        mark_block_runs(&mut h.state, &paras, &mut h.styles, &ns);

        assert!(h.state.block_runs.is_empty());
        // Untouched -- a run of length 1 is never handed to
        // process_block_run at all.
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        assert!(first.has_visible_border());
    }

    #[test]
    fn identical_but_invisible_borders_still_merge_and_mutate_without_recording_a_block_run() {
        let (doc, ns) = parse_root(
            r#"<w:document><w:body>
                 <w:p><w:r><w:t>a</w:t></w:r></w:p>
                 <w:p><w:r><w:t>b</w:t></w:r></w:p>
               </w:body></w:document>"#,
        );
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (_body, paras) = h.body(document, &ns);
        mark_block_runs(&mut h.state, &paras, &mut h.styles, &ns);

        // No visible border anywhere -> nothing to record in
        // block_runs, but the two paragraphs still formed a run (both
        // have identical -- empty -- borders) and were mutated:
        // padding between them was still zeroed.
        assert!(h.state.block_runs.is_empty());
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        let second = h.styles.resolve_paragraph(paras[1], &ns);
        assert_eq!(first.borders.bottom.padding, Some(0.0));
        assert_eq!(second.borders.top.padding, Some(0.0));
    }

    #[test]
    fn a_different_frame_prevents_merging_despite_identical_borders() {
        let (doc, ns) = parse_root(&format!(
            r#"<w:document><w:body>
                 <w:p><w:pPr>{BORDER}</w:pPr><w:r><w:t>a</w:t></w:r></w:p>
                 <w:p><w:pPr>{BORDER}<w:framePr/></w:pPr><w:r><w:t>b</w:t></w:r></w:p>
               </w:body></w:document>"#
        ));
        let document = ns.first_child(doc.root_element(), "w:document").unwrap();
        let mut h = Harness::new();
        let (_body, paras) = h.body(document, &ns);
        assert_ne!(
            h.state.frame_map.get(&paras[0]),
            h.state.frame_map.get(&paras[1])
        );

        mark_block_runs(&mut h.state, &paras, &mut h.styles, &ns);

        assert!(h.state.block_runs.is_empty());
        let first = h.styles.resolve_paragraph(paras[0], &ns);
        assert!(first.has_visible_border(), "left untouched, not merged");
    }
}

#[cfg(test)]
mod resolve_links_tests {
    use super::*;
    use crate::docx::tables::Tables;
    use roxmltree::Document;

    const DOC_OPEN: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:xml="http://www.w3.org/XML/1998/namespace" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

    fn parse_para(body: &str) -> (Document<'static>, DocxNamespace) {
        let xml: &'static str = Box::leak(format!("<w:p {DOC_OPEN}>{body}</w:p>").into_boxed_str());
        (
            Document::parse(xml).expect("valid XML"),
            DocxNamespace::default(),
        )
    }

    struct Harness<'a, 'i> {
        dom: Dom,
        state: ConvertState<'a, 'i>,
        styles: Styles<'a, 'i>,
        footnotes: Footnotes<'a, 'i>,
        settings: Settings,
        theme: Theme,
    }

    impl<'a, 'i> Harness<'a, 'i> {
        fn new() -> Self {
            Harness {
                dom: Dom::empty(),
                state: ConvertState::new(),
                styles: Styles::new(Tables::default()),
                footnotes: Footnotes::new(),
                settings: Settings::new(),
                theme: Theme::new(),
            }
        }

        fn convert(&mut self, p: Node<'a, 'i>, ns: &DocxNamespace, rels: &Relationships) -> NodeId {
            convert_p(
                &mut self.dom,
                &mut self.state,
                p,
                &mut self.styles,
                &mut self.footnotes,
                &self.settings,
                &self.theme,
                None,
                "test-uuid",
                rels,
                ns,
            )
        }
    }

    fn relationships(pairs: &[(&str, &str)]) -> Relationships {
        let mut rels = Relationships::default();
        for &(k, v) in pairs {
            rels.by_id.insert(k.to_string(), v.to_string());
        }
        rels
    }

    #[test]
    fn a_hyperlink_with_an_rid_resolves_to_the_relationship_target() {
        let (doc, ns) =
            parse_para(r#"<w:hyperlink r:id="rId1"><w:r><w:t>click</w:t></w:r></w:hyperlink>"#);
        let rels = relationships(&[("rId1", "https://example.com/")]);
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns, &rels);

        let resolved = resolve_links(&mut h.dom, &h.state, &ns);
        assert_eq!(resolved.len(), 1);
        let &span = resolved.values().next().unwrap();
        assert_eq!(h.dom.tag(span), Some("a"));
        assert_eq!(
            h.dom.node(span).attrs.get("href").map(String::as_str),
            Some("https://example.com/")
        );
    }

    #[test]
    fn a_hyperlink_with_a_w_anchor_resolves_against_the_anchor_map() {
        let (doc, ns) = parse_para(
            r#"<w:hyperlink w:anchor="chap1"><w:r><w:t>click</w:t></w:r></w:hyperlink>"#,
        );
        let mut h = Harness::new();
        h.state
            .anchor_map
            .insert("chap1".to_string(), "id_chap1".to_string());
        h.convert(doc.root_element(), &ns, &Relationships::default());

        let resolved = resolve_links(&mut h.dom, &h.state, &ns);
        let &span = resolved.values().next().unwrap();
        assert_eq!(
            h.dom.node(span).attrs.get("href").map(String::as_str),
            Some("#id_chap1")
        );
    }

    #[test]
    fn multiple_runs_in_one_hyperlink_are_wrapped_into_a_single_a() {
        let (doc, ns) = parse_para(
            r#"<w:hyperlink r:id="rId1"><w:r><w:t>a</w:t></w:r><w:r><w:t>b</w:t></w:r></w:hyperlink>"#,
        );
        let rels = relationships(&[("rId1", "https://example.com/")]);
        let mut h = Harness::new();
        let dest = h.convert(doc.root_element(), &ns, &rels);
        assert_eq!(h.dom.children(dest).len(), 2, "still two separate spans");

        resolve_links(&mut h.dom, &h.state, &ns);

        let children = h.dom.children(dest);
        assert_eq!(children.len(), 1, "merged into one wrapper");
        assert_eq!(h.dom.tag(children[0]), Some("a"));
        assert_eq!(h.dom.children(children[0]).len(), 2);
        assert_eq!(
            h.dom.serialize(dest),
            r#"<p><a href="https://example.com/"><span>a</span><span>b</span></a></p>"#
        );
    }

    #[test]
    fn the_source_run_is_marked_is_link() {
        let (doc, ns) =
            parse_para(r#"<w:hyperlink r:id="rId1"><w:r><w:t>click</w:t></w:r></w:hyperlink>"#);
        let rels = relationships(&[("rId1", "https://example.com/")]);
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns, &rels);

        let run = ns
            .descendants(doc.root_element(), &["w:r"])
            .into_iter()
            .next()
            .unwrap();
        assert!(h.state.is_link.contains(&run));
    }

    #[test]
    fn an_unresolvable_hyperlink_is_relabeled_but_gets_no_href() {
        let (doc, ns) = parse_para(r#"<w:hyperlink><w:r><w:t>click</w:t></w:r></w:hyperlink>"#);
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns, &Relationships::default());

        let resolved = resolve_links(&mut h.dom, &h.state, &ns);
        let &span = resolved.values().next().unwrap();
        assert_eq!(h.dom.tag(span), Some("a"));
        assert!(h.dom.node(span).attrs.get("href").is_none());
    }

    #[test]
    fn target_and_tooltip_become_target_and_title_attributes() {
        let (doc, ns) = parse_para(
            r#"<w:hyperlink r:id="rId1" w:tgtFrame="_blank" w:tooltip="See also"><w:r><w:t>click</w:t></w:r></w:hyperlink>"#,
        );
        let rels = relationships(&[("rId1", "https://example.com/")]);
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns, &rels);

        let resolved = resolve_links(&mut h.dom, &h.state, &ns);
        let &span = resolved.values().next().unwrap();
        assert_eq!(
            h.dom.node(span).attrs.get("target").map(String::as_str),
            Some("_blank")
        );
        assert_eq!(
            h.dom.node(span).attrs.get("title").map(String::as_str),
            Some("See also")
        );
    }

    #[test]
    fn runs_outside_any_hyperlink_are_not_tracked() {
        let (doc, ns) = parse_para(
            r#"<w:hyperlink r:id="rId1"><w:r><w:t>link</w:t></w:r></w:hyperlink><w:r><w:t> plain</w:t></w:r>"#,
        );
        let rels = relationships(&[("rId1", "https://example.com/")]);
        let mut h = Harness::new();
        h.convert(doc.root_element(), &ns, &rels);

        assert_eq!(h.state.link_map.len(), 1);
        let spans_tracked: usize = h.state.link_map.values().map(Vec::len).sum();
        assert_eq!(spans_tracked, 1, "the trailing plain run isn't tracked");
    }
}
