//! OEB/XHTML -> RTF markup.
//!
//! Port of `old_src/src/calibre/ebooks/rtf/rtfml.py`'s `RTFMLizer`.
//! Architecturally identical to three modules already ported this
//! session -- [`crate::pml::pmlml::PmlMlizer`] (issue #47),
//! [`crate::rb::rbml::RbMlizer`] (issue #48), and
//! [`crate::fb2::fb2ml::Fb2Mlizer`] (issue #26): walk an XHTML tree
//! with a [`StyleProvider`], maintain a tag stack, emit markup
//! strings. This module mirrors `rbml.rs`'s shape most closely (the
//! most recent, and closest in scope).
//!
//! # Preserved upstream quirks
//!
//! - **`txt2rtf` corrupts `{`/`}` escaping.** Python escapes `{`/`}`/
//!   `\` in that order: `{` -> `\'7b`, `}` -> `\'7d`, `\\` -> `\'5c`.
//!   But each of those replacement strings *itself* contains a literal
//!   backslash, and the backslash-escaping step runs *last* -- so it
//!   also matches (and re-escapes) the backslashes the first two steps
//!   just inserted. Verified against a live run of the Python:
//!   `txt2rtf('a{b}c\\d')` returns `"a\\'5c'7bb\\'5c'7dc\\'5cd"`, not
//!   the intended `"a\\'7bb\\'7dc\\'5cd"` -- every `{`/`}` in the input
//!   comes out as the mangled `\'5c'7b`/`\'5c'7d` rather than a valid
//!   `\'7b`/`\'7d` hex-escape, while a literal backslash in the input
//!   (handled by the *last* replace, so nothing runs after it to
//!   re-escape it) comes out correctly. Ported byte-for-byte
//!   faithfully in [`txt2rtf`] below -- same three sequential
//!   `.replace()` calls, same order, same resulting corruption.
//! - **`header()`'s title/author are not RTF-escaped.** Interpolated
//!   directly into the `\title`/`\author` info-group text with no
//!   `txt2rtf` pass, so a title or author containing `{`, `}`, or `\`
//!   produces structurally invalid RTF. Ported as-is (matches the
//!   Python exactly; `header()` never calls `txt2rtf`).
//!
//! # Where the image pipeline differs from Python
//!
//! `image_to_hexstring`'s role (normalize arbitrary image bytes to a
//! standard re-encoded form before embedding) mirrors
//! `crate::fb2::fb2ml`'s `ImageConverter` seam (see that module's own
//! doc comment for `save_cover_data_to`) -- but unlike FB2 (which
//! natively embeds JPEG *or* PNG and so only converts the rest, and
//! ships no real converter because this crate had no `image`-crate
//! dependency wired to that file yet), RTF's `\jpegblip` embed format
//! takes JPEG only, so [`DefaultImageConverter`] here really does
//! decode-and-re-encode via the `image` crate rather than deferring to
//! an injected no-op. Already-JPEG input passes through unchanged
//! (matching Python's `save_cover_data_to`'s `changed = fmt !=
//! orig_fmt` short-circuit); everything else is decoded and
//! re-encoded at quality 90 (`save_cover_data_to`'s own default
//! `compression_quality`). Full parity with `save_cover_data_to`
//! itself (resizing to `tweaks['maximum_cover_size']`, letterboxing,
//! grayscale/eink modes) is Qt-cover-pipeline territory well outside
//! this issue's three files; [`identify`] (already ported, at
//! `calibre_utils::imghdr::identify`) still supplies the width/height
//! calibre_utils::imghdr::identify` needs for `\picw`/`\pich`.

use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use roxmltree::{Document, Node};

use crate::metadata::authors_to_string;
use crate::oeb::book::OEBBook;
use crate::oeb::constants::OEB_RASTER_IMAGES;
pub use crate::oeb::stylizer::{ResolvedStyle, StyleProvider, TagStylizer};
use calibre_utils::imghdr::identify;

/// The XHTML namespace, which content must be in to be converted.
pub const XHTML_NS: &str = "http://www.w3.org/1999/xhtml";

/// Port of `TAGS`.
fn tag_rtf(tag: &str) -> Option<&'static str> {
    match tag {
        "b" => Some("\\b"),
        "del" => Some("\\deleted"),
        "h1" => Some("\\s1 \\afs32"),
        "h2" => Some("\\s2 \\afs28"),
        "h3" => Some("\\s3 \\afs28"),
        "h4" => Some("\\s4 \\afs23"),
        "h5" => Some("\\s5 \\afs23"),
        "h6" => Some("\\s6 \\afs21"),
        "i" => Some("\\i"),
        "li" => Some("\t"),
        "p" => Some("\t"),
        "sub" => Some("\\sub"),
        "sup" => Some("\\super"),
        "u" => Some("\\ul"),
        _ => None,
    }
}

/// Port of `SINGLE_TAGS`.
fn single_tag_rtf(tag: &str) -> Option<&'static str> {
    if tag == "br" {
        Some("\n{\\line }\n")
    } else {
        None
    }
}

/// Port of `STYLES`, in the Python's declared order:
/// `font-weight`, `font-style`, `text-align`, `text-decoration`.
const STYLE_PROPS: &[&str] = &["font-weight", "font-style", "text-align", "text-decoration"];

fn style_tag_for(prop: &str, value: &str) -> Option<&'static str> {
    match (prop, value) {
        ("font-weight", "bold") | ("font-weight", "bolder") => Some("\\b"),
        ("font-style", "italic") => Some("\\i"),
        ("text-align", "center") => Some("\\qc"),
        ("text-align", "left") => Some("\\ql"),
        ("text-align", "right") => Some("\\qr"),
        ("text-decoration", "line-through") => Some("\\strike"),
        ("text-decoration", "underline") => Some("\\ul"),
        _ => None,
    }
}

/// Port of `BLOCK_TAGS`.
const BLOCK_TAGS: &[&str] = &["div", "p", "h1", "h2", "h3", "h4", "h5", "h6", "li"];

/// Port of `BLOCK_STYLES`.
const BLOCK_STYLES: &[&str] = &["block"];

/// The fixed font-table and stylesheet RTF boilerplate `header()`
/// appends after its dynamic `\info` group. Transcribed verbatim from
/// the Python's `return header + (...)` literal (mechanically
/// extracted and diffed against the live module to guarantee an exact
/// match -- not retyped by hand). Not "cleaned up": this is
/// intentionally opaque legacy RTF boilerplate, ported as data.
const FONT_TABLE_AND_STYLESHEET: &str = "{\\fonttbl{\\f0\\froman\\fprq2\\fcharset128 Times New Roman;}{\\f1\\froman\\fprq2\\fcharset128 Times New Roman;}{\\f2\\fswiss\\fprq2\\fcharset128 Arial;}{\\f3\\fnil\\fprq2\\fcharset128 Arial;}{\\f4\\fnil\\fprq2\\fcharset128 MS Mincho;}{\\f5\\fnil\\fprq2\\fcharset128 Tahoma;}{\\f6\\fnil\\fprq0\\fcharset128 Tahoma;}}\n{\\stylesheet{\\ql \\li0\\ri0\\nowidctlpar\\wrapdefault\\faauto\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\af25\\afs24\\alang1033 \\ltrch\\fcs0 \\fs24\\lang1033\\langfe255\\cgrid\\langnp1033\\langfenp255 \\snext0 Normal;}\n{\\s1\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel0\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\af0\\afs32\\alang1033 \\ltrch\\fcs0 \\b\\fs32\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink21 heading 1;}\n{\\s2\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel1\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\ai\\af0\\afs28\\alang1033 \\ltrch\\fcs0 \\b\\i\\fs28\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink22 heading 2;}\n{\\s3\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel2\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\af0\\afs28\\alang1033 \\ltrch\\fcs0 \\b\\fs28\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink23 heading 3;}\n{\\s4\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel3\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\ai\\af0\\afs23\\alang1033 \\ltrch\\fcs0\\b\\i\\fs23\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink24 heading 4;}\n{\\s5\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel4\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\af0\\afs23\\alang1033 \\ltrch\\fcs0 \\b\\fs23\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink25 heading 5;}\n{\\s6\\ql \\li0\\ri0\\sb240\\sa120\\keepn\\nowidctlpar\\wrapdefault\\faauto\\outlinelevel5\\rin0\\lin0\\itap0 \\rtlch\\fcs1 \\ab\\af0\\afs21\\alang1033 \\ltrch\\fcs0 \\b\\fs21\\lang1033\\langfe255\\loch\\f1\\hich\\af1\\dbch\\af26\\cgrid\\langnp1033\\langfenp255 \\sbasedon15 \\snext16 \\slink26 heading 6;}}\n";

/// Port of `txt2rtf`. See the module docs' `{`/`}`-escaping-corruption
/// quirk -- preserved byte-for-byte.
pub fn txt2rtf(text: &str) -> String {
    let text = text.replace('{', "\\'7b");
    let text = text.replace('}', "\\'7d");
    let text = text.replace('\\', "\\'5c");

    let mut buf = String::with_capacity(text.len());
    for ch in text.chars() {
        let val = ch as u32;
        if val == 160 {
            buf.push_str("\\~");
        } else if val <= 127 {
            buf.push(ch);
        } else {
            buf.push_str(&format!("\\u{val}?"));
        }
    }
    buf
}

/// Normalizes arbitrary image bytes into embeddable JPEG bytes. See
/// the module docs for how this differs from `fb2::fb2ml`'s
/// `ImageConverter` seam.
pub trait ImageConverter {
    fn to_jpeg(&self, data: &[u8]) -> Result<Vec<u8>>;
}

/// Real default: passes already-JPEG data through, otherwise decodes
/// and re-encodes via the `image` crate at quality 90.
#[derive(Debug, Default, Clone)]
pub struct DefaultImageConverter;

impl ImageConverter for DefaultImageConverter {
    fn to_jpeg(&self, data: &[u8]) -> Result<Vec<u8>> {
        if identify(data).0 == Some("jpeg") {
            return Ok(data.to_vec());
        }
        let img = image::load_from_memory(data).context("unrecognized image format")?;
        let mut jpeg_bytes = Vec::new();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, 90);
        encoder
            .encode_image(&img)
            .context("failed to encode image as jpeg")?;
        Ok(jpeg_bytes)
    }
}

/// Port of `calibre.ebooks.rtf.rtfml.RTFMLizer`.
#[derive(Debug, Default)]
pub struct RtfMlizer;

impl RtfMlizer {
    pub fn new() -> Self {
        Self
    }

    /// Port of `extract_content` + `mlize_spine`.
    pub fn extract_content(
        &mut self,
        oeb_book: &OEBBook,
        stylizer: &dyn StyleProvider,
        images: &dyn ImageConverter,
    ) -> String {
        // Port of `self.oeb_book.metadata.title[0].value` /
        // `authors_to_string([x.value for x in
        // self.oeb_book.metadata.creator])`. Python indexes `title[0]`
        // unconditionally (an `IndexError` if absent); this falls back
        // to an empty title instead of panicking, per this crate's
        // "avoid unwrap/panics on malformed input" convention.
        let title = oeb_book
            .metadata
            .first("title")
            .map(|i| i.value.clone())
            .unwrap_or_default();
        let authors: Vec<String> = oeb_book
            .metadata
            .get("creator")
            .iter()
            .map(|i| i.value.clone())
            .collect();
        let authors_str = authors_to_string(&authors);

        let mut output = header(&title, &authors_str);

        // Port of the `'titlepage' in self.oeb_book.guide` branch.
        if let Some(titlepage) = oeb_book.guide.get("titlepage") {
            let href = titlepage.href.clone();
            if let Some(item) = oeb_book.manifest.get_by_href(&href) {
                let in_spine = oeb_book.spine.items.iter().any(|si| si.idref == item.id);
                if !in_spine {
                    if let Ok(raw) = oeb_book.container.read(&item.href) {
                        let content = String::from_utf8_lossy(&raw).into_owned();
                        if let Ok(doc) = Document::parse(&content) {
                            if let Some(body) = find_body(&doc) {
                                let mut tag_stack: Vec<String> = Vec::new();
                                output.push_str(&self.dump_text(
                                    body,
                                    stylizer,
                                    &item.href,
                                    &mut tag_stack,
                                ));
                                output.push_str("{\\page }");
                            }
                        }
                    }
                }
            }
        }

        for spine_item in &oeb_book.spine.items {
            let Some(item) = oeb_book.manifest.get_by_id(&spine_item.idref) else {
                continue;
            };
            let Ok(raw) = oeb_book.container.read(&item.href) else {
                continue;
            };
            let content = String::from_utf8_lossy(&raw).into_owned();
            // Removing comments is needed as comments with -- inside
            // them can cause parsing to fail.
            let content = remove_comments(&content);
            let content = remove_newlines(&content);
            let content = remove_tabs(&content);
            if let Ok(doc) = Document::parse(&content) {
                if let Some(body) = find_body(&doc) {
                    let mut tag_stack: Vec<String> = Vec::new();
                    output.push_str(&self.dump_text(body, stylizer, &item.href, &mut tag_stack));
                }
            }
            output.push_str("{\\page }");
        }

        output.push_str(footer());
        let output = insert_images(&output, oeb_book, images);
        clean_text(&output)
    }

    /// Port of `dump_text`.
    fn dump_text(
        &mut self,
        elem: Node,
        stylizer: &dyn StyleProvider,
        page_href: &str,
        tag_stack: &mut Vec<String>,
    ) -> String {
        if !elem.is_element() {
            return String::new();
        }

        let ns = elem.tag_name().namespace();
        if !(ns.is_none() || ns == Some(XHTML_NS)) {
            // Port of the early `return elem.tail`: raw, NOT
            // RTF-escaped -- unlike the tail handling at the very end
            // of this function.
            return tail_text(elem).unwrap_or_default();
        }

        let style = stylizer.style(elem);
        if matches!(
            style.display.as_str(),
            "none" | "oeb-page-head" | "oeb-page-foot"
        ) || style.visibility == "hidden"
        {
            return tail_text(elem).unwrap_or_default();
        }

        let tag = elem.tag_name().name().to_string();
        let mut tag_count = 0usize;
        let mut text = String::new();

        // Are we in a paragraph block?
        if (BLOCK_TAGS.contains(&tag.as_str()) || BLOCK_STYLES.contains(&style.display.as_str()))
            && !tag_stack.iter().any(|t| t == "block")
        {
            tag_count += 1;
            tag_stack.push("block".to_string());
        }

        // Process tags that need special processing and that do not
        // have inner text. Usually these require an argument.
        if tag == "img" {
            if let Some(src) = elem.attribute("src").filter(|s| !s.is_empty()) {
                let resolved = resolve_href(page_href, src);
                let (block_start, block_end) = if !tag_stack.iter().any(|t| t == "block") {
                    ("{\\par\\pard\\hyphpar ", "}")
                } else {
                    ("", "")
                };
                text.push_str(&format!(
                    "{block_start} SPECIAL_IMAGE-{resolved}-REPLACE_ME {block_end}"
                ));
            }
        }

        if let Some(single_tag) = single_tag_rtf(&tag) {
            text.push_str(single_tag);
        }

        if let Some(rtf_tag) = tag_rtf(&tag) {
            if !tag_stack.iter().any(|t| t == rtf_tag) {
                tag_count += 1;
                text.push('{');
                text.push_str(rtf_tag);
                text.push('\n');
                tag_stack.push(rtf_tag.to_string());
            }
        }

        // Processes style information.
        for prop in STYLE_PROPS {
            let val: String = match *prop {
                "font-weight" => style.font_weight.clone(),
                "font-style" => style.font_style.clone(),
                "text-align" => {
                    inline_style_prop(&style.css_text, "text-align").unwrap_or_default()
                }
                "text-decoration" => style.text_decoration.clone(),
                _ => unreachable!(),
            };
            if let Some(style_tag) = style_tag_for(prop, &val) {
                if !tag_stack.iter().any(|t| t == style_tag) {
                    tag_count += 1;
                    text.push('{');
                    text.push_str(style_tag);
                    text.push('\n');
                    tag_stack.push(style_tag.to_string());
                }
            }
        }

        // Process tags that contain text.
        if let Some(own) = own_text(elem) {
            text.push_str(&txt2rtf(&own));
        }

        for child in elem.children() {
            text.push_str(&self.dump_text(child, stylizer, page_href, tag_stack));
        }

        for _ in 0..tag_count {
            let Some(end_tag) = tag_stack.pop() else {
                break;
            };
            if end_tag != "block" {
                if BLOCK_TAGS.contains(&tag.as_str()) {
                    text.push_str("\\par\\pard\\plain\\hyphpar}");
                } else {
                    text.push('}');
                }
            }
        }

        if let Some(tail) = tail_text(elem) {
            if tag_stack.iter().any(|t| t == "block") {
                text.push_str(&txt2rtf(&tail));
            } else {
                text.push_str("{\\par\\pard\\hyphpar ");
                text.push_str(&txt2rtf(&tail));
                text.push('}');
            }
        }

        text
    }
}

/// Port of `header`.
fn header(title: &str, authors: &str) -> String {
    let mut s = String::new();
    s.push_str("{\\rtf1{\\info{\\title ");
    s.push_str(title);
    s.push_str("}{\\author ");
    s.push_str(authors);
    s.push_str("}}\\ansi\\ansicpg1252\\deff0\\deflang1033\n");
    s.push_str(FONT_TABLE_AND_STYLESHEET);
    s
}

/// Port of `footer`.
fn footer() -> &'static str {
    " }"
}

/// Port of `remove_newlines`.
fn remove_newlines(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\n', '\r'], " ")
}

/// Port of `remove_tabs`.
fn remove_tabs(text: &str) -> String {
    text.replace('\t', " ")
}

/// Port of the `re.sub(r'<!--.*?-->', '', ..., flags=re.DOTALL)` call
/// inline in `mlize_spine` (not its own Python function, but split out
/// here for testability).
fn remove_comments(text: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    re.replace_all(text, "").into_owned()
}

/// Port of `insert_images`.
fn insert_images(text: &str, oeb_book: &OEBBook, images: &dyn ImageConverter) -> String {
    let mut text = text.to_string();
    for item in oeb_book.manifest.iter() {
        if !OEB_RASTER_IMAGES.contains(&item.media_type.as_str()) {
            continue;
        }
        let src = &item.href;
        let repl = match oeb_book
            .container
            .read(&item.href)
            .context("could not read image data")
            .and_then(|data| image_to_hexstring(&data, images))
        {
            Ok((data, width, height)) => {
                format!("\n\n{{\\*\\shppict{{\\pict\\jpegblip\\picw{width}\\pich{height} \n{data}\n}}}}\n\n")
            }
            Err(_) => "\n\n".to_string(),
        };
        text = text.replace(&format!("SPECIAL_IMAGE-{src}-REPLACE_ME"), &repl);
    }
    text
}

/// Port of `image_to_hexstring`. Images are hex-encoded in 128
/// hex-character lines: the Python slices the source 64 *bytes* at a
/// time and `hexlify`s each slice separately before joining with
/// `\n`, so each output line is 128 hex *characters* (2 hex digits per
/// byte * 64 bytes), not 64. Ported the same way: `chunks(64)` over
/// the byte buffer, each chunk hex-encoded independently.
fn image_to_hexstring(data: &[u8], images: &dyn ImageConverter) -> Result<(String, i64, i64)> {
    let jpeg = images.to_jpeg(data)?;
    let (_fmt, width, height) = identify(&jpeg);
    if width < 0 || height < 0 {
        anyhow::bail!("could not determine image dimensions");
    }
    let lines: Vec<String> = jpeg.chunks(64).map(hex_encode).collect();
    Ok((lines.join("\n"), width, height))
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Port of `clean_text`. Uses a literal `\n` for `os.linesep` (this
/// crate's target platform, matching how other ported modules in this
/// crate do not special-case Windows line endings).
fn clean_text(text: &str) -> String {
    static EXCESS_NEWLINES: OnceLock<Regex> = OnceLock::new();
    let excess_newlines = EXCESS_NEWLINES.get_or_init(|| Regex::new(r"\n{3,}").unwrap());
    let text = excess_newlines.replace_all(text, "\n\n");

    static EXCESS_SPACES: OnceLock<Regex> = OnceLock::new();
    let excess_spaces = EXCESS_SPACES.get_or_init(|| Regex::new(r"[ ]{2,}").unwrap());
    let text = excess_spaces.replace_all(&text, " ");

    static EXCESS_TABS: OnceLock<Regex> = OnceLock::new();
    let excess_tabs = EXCESS_TABS.get_or_init(|| Regex::new(r"\t{2,}").unwrap());
    let text = excess_tabs.replace_all(&text, "\t");

    let text = text.replace("\t ", "\t");

    static EXCESS_LINE_BREAKS: OnceLock<Regex> = OnceLock::new();
    let excess_line_breaks =
        EXCESS_LINE_BREAKS.get_or_init(|| Regex::new(r"(\{\\line \}\s*){3,}").unwrap());
    let text = excess_line_breaks.replace_all(&text, r"{\line }{\line }");

    let text = text.replace('\u{a0}', " ");
    text.replace("\n\r", "\n")
}

/// Extract one declaration's value out of an inline `style="..."`
/// CSS text (as carried by `ResolvedStyle::css_text`). Mirrors
/// `rb/rbml.rs`'s helper of the same name.
fn inline_style_prop(css_text: &str, prop: &str) -> Option<String> {
    for decl in css_text.split(';') {
        let mut parts = decl.splitn(2, ':');
        let p = parts.next()?.trim();
        let v = parts.next()?.trim();
        if p.eq_ignore_ascii_case(prop) {
            return Some(v.to_string());
        }
    }
    None
}

fn find_body<'a, 'input>(doc: &'a Document<'input>) -> Option<Node<'a, 'input>> {
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "body")
        .or_else(|| Some(doc.root_element()))
}

fn own_text(elem: Node) -> Option<String> {
    let first = elem.first_child()?;
    if first.is_text() {
        first.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

fn tail_text(elem: Node) -> Option<String> {
    let next = elem.next_sibling()?;
    if next.is_text() {
        next.text().map(str::to_string).filter(|t| !t.is_empty())
    } else {
        None
    }
}

/// Resolve an `src`/`href` against the page that carried it. Stands in
/// for the Python's `page.abshref` + `urlnormalize`. Mirrors
/// `rb/rbml.rs`'s helper of the same name/shape.
fn resolve_href(page_href: &str, src: &str) -> String {
    if src.starts_with('/') || src.contains("://") {
        return src.to_string();
    }
    let base = match page_href.rfind('/') {
        Some(i) => &page_href[..i + 1],
        None => "",
    };
    let joined = format!("{base}{src}");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oeb::container::Container;
    use crate::oeb::manifest::ManifestItem;
    use crate::oeb::spine::SpineItem;
    use std::collections::HashMap as Map;

    #[derive(Default)]
    struct MemContainer(Map<String, Vec<u8>>);

    impl Container for MemContainer {
        fn read(&self, path: &str) -> anyhow::Result<Vec<u8>> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("no such part: {path}"))
        }
        fn write(&mut self, path: &str, data: &[u8]) -> anyhow::Result<()> {
            self.0.insert(path.to_string(), data.to_vec());
            Ok(())
        }
        fn exists(&self, path: &str) -> bool {
            self.0.contains_key(path)
        }
        fn namelist(&self) -> anyhow::Result<Vec<String>> {
            Ok(self.0.keys().cloned().collect())
        }
    }

    fn book(parts: &[(&str, &str)]) -> OEBBook {
        let mut container = MemContainer::default();
        for (name, content) in parts {
            container
                .0
                .insert((*name).to_string(), content.as_bytes().to_vec());
        }
        let mut oeb = OEBBook::new(Box::new(container));
        for (i, (name, _)) in parts.iter().enumerate() {
            let id = format!("item{i}");
            oeb.manifest.items.insert(
                id.clone(),
                ManifestItem::new(&id, name, "application/xhtml+xml"),
            );
            oeb.manifest.hrefs.insert((*name).to_string(), id.clone());
            oeb.spine.items.push(SpineItem::new(&id, true));
        }
        oeb
    }

    fn convert(oeb: &OEBBook) -> String {
        RtfMlizer::new().extract_content(oeb, &TagStylizer, &DefaultImageConverter)
    }

    const PAGE: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body>
<p>Hello <b>bold</b> and <i>italic</i>.</p>
</body></html>"#;

    // ---- txt2rtf ----

    #[test]
    fn txt2rtf_escapes_braces_and_backslash_with_the_preserved_corruption() {
        // See the module docs: the intended `\'7b`/`\'7d` for `{`/`}`
        // come out mangled because the backslash-escaping step runs
        // last and re-escapes the backslash the brace-escaping just
        // inserted. Verified against the live Python.
        assert_eq!(txt2rtf("a{b}c\\d"), "a\\'5c'7bb\\'5c'7dc\\'5cd");
    }

    #[test]
    fn txt2rtf_passes_plain_ascii_through() {
        assert_eq!(txt2rtf("hello"), "hello");
    }

    #[test]
    fn txt2rtf_escapes_non_ascii_as_unicode_control_words() {
        assert_eq!(txt2rtf("caf\u{e9}"), "caf\\u233?");
    }

    #[test]
    fn txt2rtf_escapes_non_breaking_space_as_tilde() {
        assert_eq!(txt2rtf("\u{a0}"), "\\~");
    }

    // ---- header / footer ----

    #[test]
    fn header_interpolates_title_and_author_unescaped() {
        let h = header("My Book", "Jane Doe");
        assert!(h.starts_with("{\\rtf1{\\info{\\title My Book}{\\author Jane Doe}}\\ansi"));
        assert!(h.contains("\\fonttbl"));
        assert!(h.contains("\\stylesheet"));
    }

    #[test]
    fn footer_closes_the_outstanding_rtf1_group() {
        assert_eq!(footer(), " }");
    }

    #[test]
    fn extract_content_wraps_the_whole_document_in_one_balanced_group() {
        let oeb = book(&[("index.html", PAGE)]);
        let out = convert(&oeb);
        assert!(out.starts_with("{\\rtf1"), "{out}");
        assert!(out.trim_end().ends_with('}'), "{out}");
        assert!(out.contains("Hello"), "{out}");
        assert!(out.contains("{\\b"), "{out}");
        assert!(out.contains("{\\i"), "{out}");
    }

    // ---- image_to_hexstring / 128-hex-char chunking ----

    fn one_pixel_png() -> Vec<u8> {
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([200, 100, 50]));
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut buf = std::io::Cursor::new(Vec::new());
        dynamic.write_to(&mut buf, image::ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn image_to_hexstring_chunks_64_bytes_into_128_hex_chars_per_line() {
        // A real, `identify`-recognizable JPEG bigger than 64 bytes
        // (a solid PNG cover re-encodes to a tiny JPEG that
        // `identify` can't always reliably size), so the chunking
        // itself -- not `DefaultImageConverter`'s pass-through path --
        // is what's under test here.
        let img = image::RgbImage::from_fn(24, 24, |x, y| {
            image::Rgb([(x * 10) as u8, (y * 10) as u8, 128])
        });
        let dynamic = image::DynamicImage::ImageRgb8(img);
        let mut buf = std::io::Cursor::new(Vec::new());
        dynamic
            .write_to(&mut buf, image::ImageFormat::Jpeg)
            .unwrap();
        let jpeg_bytes = buf.into_inner();
        assert!(
            jpeg_bytes.len() > 64,
            "test fixture too small to exercise chunking: {} bytes",
            jpeg_bytes.len()
        );

        let (hex, _w, _h) = image_to_hexstring(&jpeg_bytes, &DefaultImageConverter).unwrap();
        let lines: Vec<&str> = hex.split('\n').collect();
        let expected_lines = jpeg_bytes.len().div_ceil(64);
        assert_eq!(lines.len(), expected_lines);
        for (i, line) in lines.iter().enumerate() {
            let start = i * 64;
            let end = (start + 64).min(jpeg_bytes.len());
            assert_eq!(*line, hex_encode(&jpeg_bytes[start..end]), "line {i}");
        }
        // Every non-final line is exactly 128 hex chars (2 hex digits
        // * 64 bytes); only the last may be shorter.
        for line in &lines[..lines.len() - 1] {
            assert_eq!(line.len(), 128);
        }
    }

    #[test]
    fn default_image_converter_passes_through_existing_jpeg() {
        // A minimal JPEG magic-number-only header is enough for
        // `identify` to say "jpeg" and short-circuit re-encoding.
        let jpeg_like: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];
        let out = DefaultImageConverter.to_jpeg(&jpeg_like).unwrap();
        assert_eq!(out, jpeg_like);
    }

    #[test]
    fn default_image_converter_re_encodes_png_as_jpeg() {
        let png = one_pixel_png();
        let out = DefaultImageConverter.to_jpeg(&png).unwrap();
        assert_eq!(identify(&out).0, Some("jpeg"));
    }

    // ---- dump_text: tag/style mapping ----

    #[test]
    fn heading_tags_map_to_their_style_control_words() {
        let page =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><h1>Title</h1></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("\\s1 \\afs32"), "{out}");
    }

    #[test]
    fn br_emits_a_line_control_word() {
        let page =
            r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>a<br/>b</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("{\\line }"), "{out}");
    }

    #[test]
    fn block_tags_close_with_par_pard_plain_hyphpar() {
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p>text</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let out = convert(&oeb);
        assert!(out.contains("\\par\\pard\\plain\\hyphpar}"), "{out}");
    }

    #[test]
    fn bold_style_property_maps_to_b_control_word() {
        use crate::oeb::stylizer::Stylizer as ConcreteStylizer;
        let page = r#"<html xmlns="http://www.w3.org/1999/xhtml"><body><p style="font-weight: bold">bold text</p></body></html>"#;
        let oeb = book(&[("index.html", page)]);
        let stylizer = ConcreteStylizer::new(96.0, 12.0);
        let out = RtfMlizer::new().extract_content(&oeb, &stylizer, &DefaultImageConverter);
        assert!(out.contains("{\\b"), "{out}");
        assert!(out.contains("bold text"), "{out}");
    }

    // ---- clean_text ----

    #[test]
    fn clean_text_collapses_excess_blank_lines() {
        assert_eq!(clean_text("a\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn clean_text_collapses_excess_spaces_and_tabs() {
        assert_eq!(clean_text("a   b"), "a b");
        assert_eq!(clean_text("a\t\t\tb"), "a\tb");
    }

    #[test]
    fn clean_text_collapses_tab_space_sequences() {
        assert_eq!(clean_text("a\t b"), "a\tb");
    }

    #[test]
    fn clean_text_collapses_excess_line_breaks() {
        let input = r"a{\line }{\line }{\line }{\line }b";
        assert_eq!(clean_text(input), r"a{\line }{\line }b");
    }

    #[test]
    fn clean_text_replaces_non_breaking_space_and_crlf_artifacts() {
        assert_eq!(clean_text("a\u{a0}b"), "a b");
        assert_eq!(clean_text("a\n\rb"), "a\nb");
    }

    // ---- remove_newlines / remove_tabs ----

    #[test]
    fn remove_newlines_replaces_all_line_ending_styles_with_a_space() {
        assert_eq!(remove_newlines("a\r\nb\nc\rd"), "a b c d");
    }

    #[test]
    fn remove_tabs_replaces_tabs_with_spaces() {
        assert_eq!(remove_tabs("a\tb"), "a b");
    }

    // ---- resolve_href ----

    #[test]
    fn resolve_href_resolves_relative_and_leaves_absolute_alone() {
        assert_eq!(
            resolve_href("text/index.html", "images/a.png"),
            "text/images/a.png"
        );
        assert_eq!(
            resolve_href("index.html", "http://x.com/a.png"),
            "http://x.com/a.png"
        );
    }

    // ---- images end-to-end ----

    #[test]
    fn images_are_embedded_as_jpegblip_with_dimensions() {
        let mut container = MemContainer::default();
        container.0.insert(
            "index.html".to_string(),
            br#"<html xmlns="http://www.w3.org/1999/xhtml"><body><img src="cover.png"/></body></html>"#
                .to_vec(),
        );
        container.0.insert("cover.png".to_string(), one_pixel_png());
        let mut oeb = OEBBook::new(Box::new(container));
        oeb.manifest.items.insert(
            "item0".to_string(),
            ManifestItem::new("item0", "index.html", "application/xhtml+xml"),
        );
        oeb.manifest
            .hrefs
            .insert("index.html".to_string(), "item0".to_string());
        oeb.spine.items.push(SpineItem::new("item0", true));
        oeb.manifest.items.insert(
            "cover".to_string(),
            ManifestItem::new("cover", "cover.png", "image/png"),
        );
        oeb.manifest
            .hrefs
            .insert("cover.png".to_string(), "cover".to_string());

        let out = convert(&oeb);
        assert!(out.contains("\\jpegblip"), "{out}");
        assert!(out.contains("\\picw1\\pich1"), "{out}");
        assert!(!out.contains("SPECIAL_IMAGE"), "{out}");
    }
}
