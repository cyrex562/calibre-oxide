//! DOCX → HTML conversion.
//!
//! [`DOCXToHTML`] is the **provisional sketch** that predates this
//! module's port — paragraphs, runs, hyperlinks and images, with
//! heading levels guessed from `w:pStyle` and no style resolution
//! whatsoever. It stays wired into the DOCX input plugin (see
//! `input/docx_input.rs`) and keeps producing *something* until real
//! `Convert::__call__` orchestration (page properties, the footnote/
//! numbering/table passes, links, frames, TOC, OPF writing -- most of
//! which are still blocked, several on files issue #130 lists
//! alongside `to_html.py` itself: `images.py`, `fields.py`,
//! `toc.py`, `cleanup.py`) is ready to replace it wholesale.
//!
//! [`convert_run`] is the first piece of the real port: `w:r` -> a
//! `<span>` in [`crate::dom`], using the now-real [`super::styles::Styles::resolve_run`]
//! (issue #130's styles/numbering/tables cluster, landed before this)
//! instead of no style resolution at all. Not yet wired into
//! `DOCXToHTML` -- that requires `convert_p` (which calls it) and the
//! surrounding per-document state (`object_map`, `anchor_map`, a
//! per-document uuid, ...) `Convert.__init__` sets up, none of which
//! exist here yet.
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

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::Path;

use roxmltree::{Document, Node};

use crate::dom::{Dom, NodeId, NodeKind};

use super::container::Docx;
use super::error::DocxError;
use super::fonts::{is_symbol_font, map_symbol_text};
use super::footnotes::Footnotes;
use super::names::DocxNamespace;
use super::settings::Settings;
use super::styles::Styles;
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
