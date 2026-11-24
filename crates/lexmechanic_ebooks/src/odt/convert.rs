//! The scoped-down ODT-content -\> XHTML converter this crate's `input.py`
//! port actually needs.
//!
//! `Extract` (the class `input.py` defines) is a *subclass* of
//! `odf.odf2xhtml.ODF2XHTML`; nearly all of the real ODF-\>XHTML structural
//! conversion (walking `office:text`, resolving `style:style` into CSS,
//! numbering lists, ...) happens in that 1754-line base class, which lives
//! in the separately-tracked, not-yet-ported `src/odf` package (see
//! `docs/modules_to_port.md`'s `## src/odf` section). This module is a
//! real, from-scratch implementation of *just enough* of that behavior --
//! extracted from reading `ODF2XHTML`'s handler methods for their
//! behavioral intent, not transliterated -- to make `Extract`'s own logic
//! (ported in [`crate::input::odt_input`]) function on typical documents.
//!
//! ## What's handled
//! - Paragraphs (`text:p`) and headings (`text:h`, outline levels 1-6),
//!   each carrying a `P-<style>` CSS class (or a semantic tag like
//!   `<h1>`/`<pre>`/`<address>` for the handful of `special_styles`
//!   LibreOffice/OpenOffice always defines).
//! - Character spans (`text:span`, `S-<style>` class or a semantic tag
//!   like `<em>`/`<strong>`/`<cite>`).
//! - Hyperlinks (`text:a`) and internal bookmark links
//!   (`text:bookmark[-start]`, `text:bookmark-ref`, `text:reference-*`).
//! - Ordered/unordered lists (`text:list`/`text:list-item`), with list
//!   nesting level tracked positionally and the list's declared numbering
//!   type (`text:list-style` / `text:list-level-style-number|bullet`)
//!   resolved to `<ol>`/`<ul>` plus a `list-style-type` CSS rule. Declared
//!   non-default start values are recorded in [`ConvertOutput::list_starts`]
//!   for [`crate::input::odt_input`]'s `apply_list_starts` step to apply.
//! - Tables (`table:table`/`-row`/`-cell`/`-column`), with row/colspan and
//!   `T-`/`TR-`/`TD-`/`TC-` style classes.
//! - Images (`draw:frame` + `draw:image`), emitted as a styled `<div>`
//!   wrapping an `<img>`, matching `ODF2XHTML.s_draw_frame`/`s_draw_image`'s
//!   position/size handling closely enough for
//!   `Extract.epubify_markup`'s anchored-image fixups (which specifically
//!   look for this `div > div > img` / `div > img` shape) to apply.
//! - Line breaks, tabs (rendered as a single space, like the original),
//!   and `text:s` (repeated non-breaking spaces).
//! - Paragraph/character style resolution -\> CSS classes (see
//!   [`crate::odt::styles`]) for bold/italic/underline/strikethrough,
//!   color, background, font family/size, text-align, margins/padding.
//!
//! ## What's explicitly out of scope
//! Footnotes/endnotes (`text:note*`), embedded objects/OLE/spreadsheets
//! (`draw:object*`), presentation-only elements (`draw:page`,
//! `draw:custom-shape`), tables of contents / indexes (their `*-source`
//! configuration elements are skipped so their metadata doesn't leak into
//! the body as stray text, but no navigable TOC is generated), change
//! tracking, and deep nested-list numbering continuation
//! (`text:continue-numbering`/`text:continue-list`) -- all real gaps in
//! `ODF2XHTML`'s full behavior, left undone because they're niche enough
//! that reproducing them faithfully would mean re-deriving large parts of
//! the 1754-line original, which is exactly the `## src/odf` work tracked
//! separately. A `text:note`/etc. subtree is simply skipped rather than
//! mis-rendered.

use crate::odt::namespaces::{
    class_name_for, sanitize_style_name, special_tag_for_class, DRAWNS, FONS, OFFICENS, SVGNS,
    TABLENS, TEXTNS, XLINKNS,
};
use crate::odt::styles::StyleResolver;
use anyhow::{Context, Result};
use indexmap::IndexMap;
use roxmltree::{Document, Node};
use std::collections::HashMap;

type XmlNode<'a> = Node<'a, 'a>;

/// Elements whose entire subtree we deliberately skip (index/TOC source
/// configuration, change tracking, field declarations, forms) -- matches
/// the intent of `ODF2XHTML`'s `s_ignorexml`/`s_text_x_source` entries for
/// these specific tags.
const SKIP_SUBTREE: &[(&str, &str)] = &[
    (TEXTNS, "alphabetical-index-source"),
    (TEXTNS, "bibliography-source"),
    (TEXTNS, "illustration-index-source"),
    (TEXTNS, "object-index-source"),
    (TEXTNS, "table-index-source"),
    (TEXTNS, "table-of-content-source"),
    (TEXTNS, "user-index-source"),
    (TEXTNS, "bibliography-configuration"),
    (TEXTNS, "linenumbering-configuration"),
    (TEXTNS, "notes-configuration"),
    (TEXTNS, "sequence-decls"),
    (TEXTNS, "variable-decls"),
    (TEXTNS, "user-field-decls"),
    (TEXTNS, "tracked-changes"),
    (TEXTNS, "note"),
    (TEXTNS, "note-citation"),
    (TEXTNS, "note-body"),
    (TABLENS, "covered-table-cell"),
    (OFFICENS, "forms"),
    (OFFICENS, "annotation"),
    (FONS, "desc"), // placeholder entry shape kept explicit even though FONS/svg:desc never collide
];

pub struct ConvertOutput {
    /// The full `<html>...</html>` document, with an inline `<style
    /// type="text/css">` block in `<head>` -- matches the shape
    /// `Extract.fix_markup`/`extract_css` expect to operate on.
    pub xhtml: String,
    /// Port of `ODF2XHTML.list_starts`: CSS class selector (dot-prefixed,
    /// e.g. `".ListName_1"`) -\> declared start value, for elements
    /// carrying that class as one of possibly several space-separated
    /// class tokens.
    pub list_starts: HashMap<String, String>,
}

struct ListFrame {
    /// The *raw* (unsanitized) `text:style-name` this list level resolved
    /// to -- inherited by nested `text:list` elements that omit their own
    /// `text:style-name`, matching `TagStack.rfindattr`.
    raw_name: String,
}

struct WalkCtx<'a> {
    resolver: &'a StyleResolver,
    out: String,
    anchors: HashMap<String, String>,
    list_starts: HashMap<String, String>,
    list_stack: Vec<ListFrame>,
}

/// Converts `content.xml` (and, if available, `styles.xml`, for named
/// styles and font-face declarations defined outside `content.xml`'s own
/// `office:automatic-styles`) into a full XHTML document string.
/// `title` is embedded directly (the caller -- `Extract::__call__`'s
/// port -- always has real metadata by this point, unlike the original
/// which derives a fallback title from the first heading it walks).
pub fn convert_content(
    content_xml: &str,
    styles_xml: Option<&str>,
    title: &str,
) -> Result<ConvertOutput> {
    let content_doc = Document::parse(content_xml).context("parsing content.xml")?;
    let styles_doc = styles_xml
        .map(Document::parse)
        .transpose()
        .context("parsing styles.xml")?;
    let resolver = StyleResolver::build(&content_doc, styles_doc.as_ref());

    let mut ctx = WalkCtx {
        resolver: &resolver,
        out: String::new(),
        anchors: HashMap::new(),
        list_starts: HashMap::new(),
        list_stack: Vec::new(),
    };

    if let Some(text_node) = find_office_text(&content_doc) {
        walk_children(text_node, &mut ctx);
    }

    let stylesheet = render_stylesheet(&resolver);
    let xhtml = format!(
        "<!DOCTYPE html PUBLIC \"-//W3C//DTD XHTML 1.1//EN\" \"http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd\">\n\
<html xmlns=\"http://www.w3.org/1999/xhtml\">\n\
<head>\n\
<meta http-equiv=\"Content-Type\" content=\"text/html;charset=UTF-8\"/>\n\
<title>{title}</title>\n\
<style type=\"text/css\">\n/*<![CDATA[*/\n{stylesheet}/*]]>*/\n</style>\n\
</head>\n\
<body>\n{body}\n</body>\n\
</html>\n",
        title = escape_text(title),
        body = ctx.out,
    );

    Ok(ConvertOutput {
        xhtml,
        list_starts: ctx.list_starts,
    })
}

fn find_office_text<'a>(doc: &'a Document<'a>) -> Option<XmlNode<'a>> {
    doc.descendants().find(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(OFFICENS)
            && n.tag_name().name() == "text"
    })
}

fn walk_children(node: XmlNode, ctx: &mut WalkCtx) {
    for child in node.children() {
        walk_node(child, ctx);
    }
}

fn walk_node(node: XmlNode, ctx: &mut WalkCtx) {
    if node.is_text() {
        if let Some(text) = node.text() {
            ctx.out.push_str(&escape_text(text));
        }
        return;
    }
    if !node.is_element() {
        return;
    }
    let ns = node.tag_name().namespace();
    let local = node.tag_name().name();

    if SKIP_SUBTREE
        .iter()
        .any(|(skip_ns, skip_local)| ns == Some(skip_ns) && local == *skip_local)
    {
        return;
    }

    match (ns, local) {
        (Some(TEXTNS), "p") => walk_paragraph(node, ctx),
        (Some(TEXTNS), "h") => walk_heading(node, ctx),
        (Some(TEXTNS), "span") => walk_span(node, ctx),
        (Some(TEXTNS), "a") => walk_anchor(node, ctx),
        (Some(TEXTNS), "list") => walk_list(node, ctx),
        (Some(TEXTNS), "list-item") => walk_list_item(node, ctx),
        (Some(TEXTNS), "line-break") => ctx.out.push_str("<br/>"),
        (Some(TEXTNS), "tab") => ctx.out.push(' '),
        (Some(TEXTNS), "s") => {
            let c: u32 = node
                .attribute((TEXTNS, "c"))
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            for _ in 0..c {
                ctx.out.push('\u{a0}');
            }
        }
        (Some(TEXTNS), "bookmark")
        | (Some(TEXTNS), "bookmark-start")
        | (Some(TEXTNS), "reference-mark-start") => {
            if let Some(name) = node.attribute((TEXTNS, "name")) {
                let id = get_anchor(ctx, name);
                ctx.out
                    .push_str(&format!("<span id=\"{}\"></span>", escape_attr(&id)));
            }
        }
        (Some(TEXTNS), "bookmark-ref") | (Some(TEXTNS), "reference-ref") => {
            if let Some(name) = node.attribute((TEXTNS, "ref-name")) {
                let id = get_anchor(ctx, name);
                ctx.out
                    .push_str(&format!("<a href=\"#{}\">", escape_attr(&id)));
                walk_children(node, ctx);
                ctx.out.push_str("</a>");
            } else {
                walk_children(node, ctx);
            }
        }
        (Some(TABLENS), "table") => walk_table(node, ctx),
        (Some(TABLENS), "table-row") => walk_table_row(node, ctx),
        (Some(TABLENS), "table-cell") => walk_table_cell(node, ctx),
        (Some(TABLENS), "table-column") => walk_table_column(node, ctx),
        (Some(DRAWNS), "frame") => walk_frame(node, ctx),
        (Some(DRAWNS), "image") => walk_image(node, ctx),
        (Some(DRAWNS), "text-box") => {
            ctx.out.push_str("<div>");
            walk_children(node, ctx);
            ctx.out.push_str("</div>");
        }
        _ => walk_children(node, ctx),
    }
}

fn walk_paragraph(node: XmlNode, ctx: &mut WalkCtx) {
    let style_name = node.attribute((TEXTNS, "style-name"));
    let (class_name, special) = class_and_special("paragraph", style_name);
    let tag = special.unwrap_or("p");
    open_tag(
        ctx,
        tag,
        if special.is_none() {
            class_name.as_deref()
        } else {
            None
        },
    );
    let start = ctx.out.len();
    walk_children(node, ctx);
    if ctx.out.len() == start {
        // Give substance to empty paragraphs, matching `e_text_p`.
        ctx.out.push('\u{a0}');
    }
    close_tag(ctx, tag);
}

fn walk_heading(node: XmlNode, ctx: &mut WalkCtx) {
    let level: u32 = node
        .attribute((TEXTNS, "outline-level"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .clamp(1, 6);
    let tag = format!("h{level}");
    let style_name = node.attribute((TEXTNS, "style-name"));
    let (class_name, special) = class_and_special("paragraph", style_name);
    open_tag(
        ctx,
        &tag,
        if special.is_none() {
            class_name.as_deref()
        } else {
            None
        },
    );
    walk_children(node, ctx);
    close_tag(ctx, &tag);
}

fn walk_span(node: XmlNode, ctx: &mut WalkCtx) {
    let style_name = node.attribute((TEXTNS, "style-name"));
    let (class_name, special) = class_and_special("text", style_name);
    let tag = special.unwrap_or("span");
    open_tag(
        ctx,
        tag,
        if special.is_none() {
            class_name.as_deref()
        } else {
            None
        },
    );
    walk_children(node, ctx);
    close_tag(ctx, tag);
}

fn walk_anchor(node: XmlNode, ctx: &mut WalkCtx) {
    let href = node.attribute((XLINKNS, "href")).unwrap_or("");
    let href = href.split('|').next().unwrap_or("");
    let resolved = match href.strip_prefix('#') {
        Some(frag) => format!("#{}", get_anchor(ctx, frag)),
        None => href.to_string(),
    };
    ctx.out
        .push_str(&format!("<a href=\"{}\">", escape_attr(&resolved)));
    walk_children(node, ctx);
    ctx.out.push_str("</a>");
}

fn walk_list(node: XmlNode, ctx: &mut WalkCtx) {
    let level = ctx.list_stack.len() as u32 + 1;
    let raw_name = node
        .attribute((TEXTNS, "style-name"))
        .map(|s| s.to_string())
        .or_else(|| ctx.list_stack.last().map(|f| f.raw_name.clone()))
        .unwrap_or_default();
    let sanitized = sanitize_style_name(&raw_name);
    let number_class = format!("{sanitized}_{level}");

    let level_def = ctx.resolver.list_level(&raw_name, level);
    let tag: &'static str = match &level_def {
        Some(def) if def.ordered => "ol",
        _ => "ul",
    };
    if let Some(def) = &level_def {
        if let Some(start) = &def.start_value {
            ctx.list_starts
                .insert(format!(".{number_class}"), start.clone());
        }
    }

    open_tag(ctx, tag, Some(&number_class));
    ctx.list_stack.push(ListFrame { raw_name });
    walk_children(node, ctx);
    ctx.list_stack.pop();
    close_tag(ctx, tag);
}

fn walk_list_item(node: XmlNode, ctx: &mut WalkCtx) {
    open_tag(ctx, "li", None);
    walk_children(node, ctx);
    close_tag(ctx, "li");
}

fn walk_table(node: XmlNode, ctx: &mut WalkCtx) {
    let class_name = node
        .attribute((TABLENS, "style-name"))
        .map(|s| format!("T-{}", sanitize_style_name(s)));
    open_tag(ctx, "table", class_name.as_deref());
    walk_children(node, ctx);
    close_tag(ctx, "table");
}

fn walk_table_row(node: XmlNode, ctx: &mut WalkCtx) {
    let class_name = node
        .attribute((TABLENS, "style-name"))
        .map(|s| format!("TR-{}", sanitize_style_name(s)));
    open_tag(ctx, "tr", class_name.as_deref());
    walk_children(node, ctx);
    close_tag(ctx, "tr");
}

fn walk_table_column(node: XmlNode, ctx: &mut WalkCtx) {
    let class_name = node
        .attribute((TABLENS, "style-name"))
        .map(|s| format!("TC-{}", sanitize_style_name(s)));
    let repeated: u32 = node
        .attribute((TABLENS, "number-columns-repeated"))
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    for _ in 0..repeated {
        empty_tag(ctx, "col", class_name.as_deref());
    }
}

fn walk_table_cell(node: XmlNode, ctx: &mut WalkCtx) {
    let class_name = node
        .attribute((TABLENS, "style-name"))
        .map(|s| format!("TD-{}", sanitize_style_name(s)));
    let mut attrs: Vec<(&str, String)> = Vec::new();
    if let Some(r) = node.attribute((TABLENS, "number-rows-spanned")) {
        attrs.push(("rowspan", r.to_string()));
    }
    if let Some(c) = node.attribute((TABLENS, "number-columns-spanned")) {
        attrs.push(("colspan", c.to_string()));
    }
    if let Some(c) = class_name {
        attrs.push(("class", c));
    }
    open_tag_with_attrs(ctx, "td", &attrs);
    walk_children(node, ctx);
    close_tag(ctx, "td");
}

fn walk_frame(node: XmlNode, ctx: &mut WalkCtx) {
    let anchor_type = node
        .attribute((TEXTNS, "anchor-type"))
        .unwrap_or("notfound");
    let class_name = node
        .attribute((DRAWNS, "style-name"))
        .map(|s| format!("G-{}", sanitize_style_name(s)))
        .unwrap_or_else(|| "G-".to_string());
    let mut style = match anchor_type {
        "paragraph" | "char" => "position:relative;".to_string(),
        "as-char" => String::new(),
        _ => "position:absolute;".to_string(),
    };
    if let Some(w) = node.attribute((SVGNS, "width")) {
        style.push_str(&format!("width:{w};"));
    }
    if let Some(h) = node.attribute((SVGNS, "height")) {
        style.push_str(&format!("height:{h};"));
    }
    if let Some(x) = node.attribute((SVGNS, "x")) {
        style.push_str(&format!("left:{x};"));
    }
    if let Some(y) = node.attribute((SVGNS, "y")) {
        style.push_str(&format!("top:{y};"));
    }
    let mut attrs = vec![("class", class_name)];
    if !style.is_empty() {
        attrs.push(("style", style));
    }
    open_tag_with_attrs(ctx, "div", &attrs);
    walk_children(node, ctx);
    close_tag(ctx, "div");
}

fn walk_image(node: XmlNode, ctx: &mut WalkCtx) {
    let href = node.attribute((XLINKNS, "href")).unwrap_or("");
    let parent_anchor = node
        .parent()
        .and_then(|p| p.attribute((TEXTNS, "anchor-type")));
    let mut attrs = vec![("alt", String::new()), ("src", href.to_string())];
    if parent_anchor != Some("char") {
        attrs.push(("style", "display: block;".to_string()));
    }
    empty_tag_with_attrs(ctx, "img", &attrs);
}

/// Returns `(css class name, semantic tag override)` for a
/// `text:style-name` on a paragraph/heading (`family = "paragraph"`) or
/// span (`family = "text"`), matching `classname`/`special_styles` lookup.
fn class_and_special(
    family: &str,
    style_name: Option<&str>,
) -> (Option<String>, Option<&'static str>) {
    let Some(name) = style_name else {
        return (None, None);
    };
    let class_name = class_name_for(family, name);
    let special = special_tag_for_class(&class_name);
    (Some(class_name), special)
}

fn get_anchor(ctx: &mut WalkCtx, name: &str) -> String {
    if let Some(existing) = ctx.anchors.get(name) {
        return existing.clone();
    }
    let id = format!("anchor{}", ctx.anchors.len() + 1);
    ctx.anchors.insert(name.to_string(), id.clone());
    id
}

fn open_tag(ctx: &mut WalkCtx, tag: &str, class_attr: Option<&str>) {
    match class_attr {
        Some(c) if !c.is_empty() => {
            ctx.out
                .push_str(&format!("<{tag} class=\"{}\">", escape_attr(c)));
        }
        _ => ctx.out.push_str(&format!("<{tag}>")),
    }
}

fn open_tag_with_attrs(ctx: &mut WalkCtx, tag: &str, attrs: &[(&str, String)]) {
    ctx.out.push('<');
    ctx.out.push_str(tag);
    for (k, v) in attrs {
        ctx.out.push(' ');
        ctx.out.push_str(k);
        ctx.out.push_str("=\"");
        ctx.out.push_str(&escape_attr(v));
        ctx.out.push('"');
    }
    ctx.out.push('>');
}

fn empty_tag(ctx: &mut WalkCtx, tag: &str, class_attr: Option<&str>) {
    match class_attr {
        Some(c) if !c.is_empty() => {
            ctx.out
                .push_str(&format!("<{tag} class=\"{}\"/>", escape_attr(c)));
        }
        _ => ctx.out.push_str(&format!("<{tag}/>")),
    }
}

fn empty_tag_with_attrs(ctx: &mut WalkCtx, tag: &str, attrs: &[(&str, String)]) {
    ctx.out.push('<');
    ctx.out.push_str(tag);
    for (k, v) in attrs {
        ctx.out.push(' ');
        ctx.out.push_str(k);
        ctx.out.push_str("=\"");
        ctx.out.push_str(&escape_attr(v));
        ctx.out.push('"');
    }
    ctx.out.push_str("/>");
}

fn close_tag(ctx: &mut WalkCtx, tag: &str) {
    ctx.out.push_str(&format!("</{tag}>"));
}

fn escape_text(s: &str) -> std::borrow::Cow<'_, str> {
    html_escape::encode_text(s)
}

fn escape_attr(s: &str) -> std::borrow::Cow<'_, str> {
    html_escape::encode_double_quoted_attribute(s)
}

/// Renders every declared style class's resolved CSS (grouping classes
/// that resolve to byte-identical declarations under one comma-separated
/// selector, matching `generate_stylesheet`'s deduplication) plus a small
/// baseline reset and each list style's `list-style-type`.
fn render_stylesheet(resolver: &StyleResolver) -> String {
    let mut groups: IndexMap<Vec<(String, String)>, Vec<String>> = IndexMap::new();
    for class_name in resolver.class_names() {
        let mut props = resolver.resolve_css(class_name);
        filter_margin_shorthand(&mut props);
        if props.is_empty() {
            continue;
        }
        let mut sorted: Vec<(String, String)> = props.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        groups
            .entry(sorted)
            .or_default()
            .push(format!(".{class_name}"));
    }

    let mut out = String::new();
    out.push_str(
        "* { padding: 0; margin: 0; }\nbody { margin: 0 1em; }\nol, ul { padding-left: 2em; }\n",
    );
    for (props, names) in &groups {
        out.push_str(&names.join(", "));
        out.push_str(" {\n");
        for (k, v) in props {
            out.push('\t');
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str(";\n");
        }
        out.push_str("}\n");
    }
    for (class_token, list_style_type) in resolver.list_class_rules() {
        out.push_str(&format!(
            ".{class_token} {{\n\tlist-style-type: {list_style_type};\n}}\n"
        ));
    }
    out
}

fn filter_margin_shorthand(props: &mut IndexMap<String, String>) {
    let has_all = ["margin-left", "margin-right", "margin-top", "margin-bottom"]
        .iter()
        .all(|k| props.contains_key(*k));
    if has_all {
        props.shift_remove("margin");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_CONTENT_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
  xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
  xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
  xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
  xmlns:xlink="http://www.w3.org/1999/xlink"
  office:version="1.2">
 <office:automatic-styles>
  <style:style style:name="P1" style:family="paragraph">
   <style:text-properties fo:font-weight="bold"/>
  </style:style>
  <style:style style:name="T1" style:family="text">
   <style:text-properties fo:font-style="italic"/>
  </style:style>
  <text:list-style style:name="L1">
   <text:list-level-style-number text:level="1" style:num-format="1" text:start-value="3"/>
  </text:list-style>
 </office:automatic-styles>
 <office:body>
  <office:text>
   <text:h text:outline-level="1">Chapter One</text:h>
   <text:p text:style-name="P1">Some <text:span text:style-name="T1">italic</text:span> text.</text:p>
   <text:list text:style-name="L1">
    <text:list-item><text:p>Item A</text:p></text:list-item>
    <text:list-item><text:p>Item B</text:p></text:list-item>
   </text:list>
   <table:table table:style-name="Table1">
    <table:table-column/>
    <table:table-row>
     <table:table-cell><text:p>Cell 1</text:p></table:table-cell>
     <table:table-cell><text:p>Cell 2</text:p></table:table-cell>
    </table:table-row>
   </table:table>
   <text:p><draw:frame draw:name="Frame1" svg:width="5cm" svg:height="3cm"><draw:image xlink:href="Pictures/100000000000012C.png"/></draw:frame></text:p>
   <text:p><text:a xlink:href="http://example.com">a link</text:a></text:p>
  </office:text>
 </office:body>
</office:document-content>"#;

    #[test]
    fn converts_headings_paragraphs_and_inline_styles() {
        let out = convert_content(SAMPLE_CONTENT_XML, None, "My Doc").unwrap();
        assert!(out.xhtml.contains("<h1"), "{}", out.xhtml);
        assert!(out.xhtml.contains("Chapter One"));
        assert!(out.xhtml.contains("class=\"P-P1\""));
        assert!(out.xhtml.contains("class=\"S-T1\""));
        assert!(out.xhtml.contains("font-weight: bold"));
        assert!(out.xhtml.contains("font-style: italic"));
        assert!(out.xhtml.contains("<title>My Doc</title>"));
    }

    #[test]
    fn converts_ordered_list_with_declared_start() {
        let out = convert_content(SAMPLE_CONTENT_XML, None, "t").unwrap();
        assert!(out.xhtml.contains("<ol class=\"L1_1\">"), "{}", out.xhtml);
        assert!(out.xhtml.contains("<li><p>Item A</p></li>"));
        assert_eq!(out.list_starts.get(".L1_1"), Some(&"3".to_string()));
    }

    #[test]
    fn converts_table_with_style_class() {
        let out = convert_content(SAMPLE_CONTENT_XML, None, "t").unwrap();
        assert!(
            out.xhtml.contains("<table class=\"T-Table1\">"),
            "{}",
            out.xhtml
        );
        assert!(out.xhtml.contains("Cell 1"));
        assert!(out.xhtml.contains("Cell 2"));
    }

    #[test]
    fn converts_image_frame_and_hyperlink() {
        let out = convert_content(SAMPLE_CONTENT_XML, None, "t").unwrap();
        assert!(
            out.xhtml.contains("src=\"Pictures/100000000000012C.png\""),
            "{}",
            out.xhtml
        );
        assert!(out.xhtml.contains("href=\"http://example.com\""));
        assert!(out.xhtml.contains("a link"));
    }

    #[test]
    fn empty_paragraph_gets_nbsp() {
        let xml = r#"<office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
<office:body><office:text><text:p></text:p></office:text></office:body></office:document-content>"#;
        let out = convert_content(xml, None, "t").unwrap();
        assert!(out.xhtml.contains("<p>\u{a0}</p>"), "{}", out.xhtml);
    }
}
